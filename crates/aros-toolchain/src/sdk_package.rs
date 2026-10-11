//! Local packaging and byte-consistency read-back for one source-produced SDK.
//!
//! The module handles only a closed local archive, manifest and checksum set.
//! It does not execute SDK tools, establish provenance, or authorize a release.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

use aros_common::sdk_manifest::{
    SdkIdentity, SdkManifest, SDK_MANIFEST_FILE, SDK_MANIFEST_SCHEMA, SDK_MAX_ENTRIES,
    SDK_MAX_METADATA_BYTES, SDK_MAX_REGULAR_BYTES,
};
use aros_common::sdk_release_lock::SdkReleaseLock;
use aros_common::{
    directory_entry_names_nofollow_bounded, finish_sha256, measure_tree_content_cas_bounded,
    open_regular_file_nofollow, publish_prepared_tree_noclobber, sha256_bytes,
    toolchain_inventory_sha256, ArosToolchainManifestEntry, Sha256Digest, TreeContentCas,
    TreeTraversalLimits,
};
use sha2::{Digest, Sha256};
use tempfile::{Builder as TempDirBuilder, TempDir};

use crate::package::{pretty_json, write_payload_archive};
use crate::package_verify::{
    extract_payload_archive, verify_payload_archive, PayloadArchiveContract, PayloadArchiveReadback,
};
use crate::ContractError;

const ARCHIVE_ROOT: &str = "sdk";
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SCAN_BUFFER_BYTES: usize = 128 * 1024;
const MAX_FORBIDDEN_PREFIX_BYTES: usize = 4096;

/// Inputs for packaging one complete source-produced SDK tree.
#[derive(Debug, Clone)]
pub struct SdkPackageRequest {
    /// Existing SDK candidate root. The operation snapshots it and never mutates it.
    pub candidate_root: PathBuf,
    /// Absent final directory for the archive, manifest and checksum.
    pub output_dir: PathBuf,
    /// Source-selected SDK identity recorded in the package manifest.
    pub identity: SdkIdentity,
    /// Independently selected digest of the complete normalized SDK inventory.
    pub expected_tree_sha256: Sha256Digest,
    /// Absolute byte prefixes that must not occur in regular SDK payload files.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Measured result of one complete local SDK package set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkPackageOutput {
    /// Atomically published package-set directory.
    pub output_dir: PathBuf,
    /// Deterministic XZ-compressed tar archive.
    pub archive: PathBuf,
    /// External copy of the archive's embedded manifest.
    pub manifest: PathBuf,
    /// SHA-256 sidecar for the archive.
    pub checksum: PathBuf,
    /// Archive SHA-256 measured after durable write and read-back.
    pub archive_sha256: Sha256Digest,
    /// Archive byte count measured after durable write and read-back.
    pub archive_size: u64,
}

/// Inputs for verifying a complete local SDK package set.
#[derive(Debug, Clone)]
pub struct SdkVerificationRequest {
    /// Directory containing exactly one archive, manifest and checksum.
    pub package_dir: PathBuf,
    /// Explicit expected source-owned SDK identity.
    pub identity: SdkIdentity,
    /// Independently selected digest of the complete normalized SDK inventory.
    pub expected_tree_sha256: Sha256Digest,
    /// Independently selected archive SHA-256.
    pub expected_archive_sha256: Sha256Digest,
    /// Independently selected archive size in bytes.
    pub expected_archive_size: u64,
    /// Absolute byte prefixes forbidden in every regular payload member.
    pub forbidden_prefixes: Vec<PathBuf>,
}

/// Byte-consistency read-back for a verified SDK package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSdkPackage {
    /// Parsed SDK manifest proven identical to both external and embedded bytes.
    pub manifest: SdkManifest,
    /// Measured archive SHA-256.
    pub archive_sha256: Sha256Digest,
    /// Measured archive byte count.
    pub archive_size: u64,
}

/// One archive selected by an explicit SDK release lock.
#[derive(Debug, Clone)]
pub struct SdkLockedArchiveRequest {
    /// No-follow regular archive input selected by `lock`.
    pub archive: PathBuf,
    /// Independently selected release identity and artifact byte digests.
    pub lock: SdkReleaseLock,
}

/// Package an SDK candidate into a deterministic local artifact set.
///
/// Packaging validates identity and roots before creating a private sibling
/// stage, snapshots the candidate with bounded no-follow traversal, and checks
/// its actual SDK inventory against `expected_tree_sha256` before writing the
/// manifest. The output directory is published atomically and is never reused.
///
/// This operation proves byte consistency only. It makes no execution,
/// provenance or release-readiness claim.
///
/// # Errors
///
/// Returns an error if the identity or roots are invalid, the bounded candidate
/// snapshot is unsafe or differs from the selected tree digest, or any package
/// write, read-back or publication check fails.
pub fn package(request: &SdkPackageRequest) -> Result<SdkPackageOutput, ContractError> {
    request
        .identity
        .validate()
        .map_err(|error| ContractError::package(format!("invalid SDK identity: {error}")))?;
    require_nonzero_digest(&request.expected_tree_sha256, true)?;
    let forbidden = validate_forbidden_prefixes(&request.forbidden_prefixes, true)?;
    let candidate_root = checked_existing_directory(&request.candidate_root, true)?;
    let output_dir = checked_absent_root(&request.output_dir, true)?;
    require_disjoint(&candidate_root, &output_dir, true)?;

    let limits = sdk_snapshot_limits()?;
    let source_snapshot = measure_tree_content_cas_bounded(&candidate_root, limits)
        .map_err(|_| ContractError::package("cannot safely measure bounded SDK candidate"))?;
    if source_snapshot.entry_count() > SDK_MAX_ENTRIES
        || source_snapshot
            .regular_file_bytes()
            .is_none_or(|size| size > SDK_MAX_REGULAR_BYTES)
    {
        return Err(ContractError::package(
            "SDK candidate exceeds its entry or regular-byte limit",
        ));
    }
    validate_candidate_source(&candidate_root, &source_snapshot)?;
    revalidate_source_snapshot(&candidate_root, &source_snapshot)?;

    let parent = output_dir
        .parent()
        .ok_or_else(|| ContractError::package("SDK output directory has no parent"))?;
    let stage = private_sibling_stage(parent, ".aros-sdk-package-")?;
    // Keep the archive's source tree outside the exact three-file artifact
    // stage. Both private roots are siblings under the validated output parent.
    let payload_stage = private_sibling_stage(parent, ".aros-sdk-payload-")?;
    let sdk_root = payload_stage.path().join(ARCHIVE_ROOT);
    fs::create_dir(&sdk_root)
        .map_err(|_| ContractError::package("cannot create private SDK payload stage"))?;
    aros_common::copy_tree_from_snapshot_nofollow(
        &candidate_root,
        &sdk_root,
        &source_snapshot,
        limits,
    )
    .map_err(|_| {
        ContractError::package("SDK candidate changed or could not be copied from its snapshot")
    })?;

    let candidate_scan = scan_sdk_tree(&sdk_root, false, &forbidden, SDK_MAX_REGULAR_BYTES)
        .map_err(|error| {
            ContractError::package(format!(
                "cannot safely inventory staged SDK candidate: {error}"
            ))
        })?;
    let actual_tree_sha256 = toolchain_inventory_sha256(&candidate_scan.entries)
        .map_err(|_| ContractError::package("cannot serialize SDK inventory"))?;
    let actual_tree_sha256 = Sha256Digest::parse(&actual_tree_sha256)
        .map_err(|_| ContractError::package("cannot represent SDK inventory digest"))?;
    if actual_tree_sha256 != request.expected_tree_sha256 {
        return Err(ContractError::package(
            "SDK candidate inventory differs from the independently selected tree digest",
        ));
    }

    let manifest = SdkManifest {
        schema: SDK_MANIFEST_SCHEMA.into(),
        identity: request.identity.clone(),
        tree_sha256: actual_tree_sha256,
        files: candidate_scan.entries,
    };
    manifest.validate().map_err(|error| {
        ContractError::package(format!("invalid generated SDK manifest: {error}"))
    })?;
    let manifest_bytes = bounded_pretty_manifest(&manifest, true)?;
    check_forbidden_bytes(&manifest_bytes, &forbidden, true)?;
    let sdk_manifest_path = sdk_root.join(SDK_MANIFEST_FILE);
    write_new_file(&sdk_manifest_path, &manifest_bytes, true)?;

    let asset = request
        .identity
        .asset_name()
        .map_err(|error| ContractError::package(format!("invalid SDK asset identity: {error}")))?;
    let archive = stage.path().join(&asset);
    write_payload_archive(
        &archive,
        &sdk_root,
        request.identity.source_date_epoch,
        ARCHIVE_ROOT,
    )?;
    let (_, archive_measurement) = measure_file(&archive, MAX_ARCHIVE_BYTES, None, true)?;
    let archive_sha256 = archive_measurement.digest;
    let archive_size = archive_measurement.size;
    let manifest_path = stage.path().join(format!("{asset}.manifest.json"));
    write_new_file(&manifest_path, &manifest_bytes, true)?;
    let checksum = stage.path().join(format!("{asset}.sha256"));
    let sidecar = format!("{archive_sha256}  {asset}\n");
    write_new_file(&checksum, sidecar.as_bytes(), true)?;

    revalidate_source_snapshot(&candidate_root, &source_snapshot)?;
    let verification = SdkVerificationRequest {
        package_dir: stage.path().to_path_buf(),
        identity: request.identity.clone(),
        expected_tree_sha256: request.expected_tree_sha256.clone(),
        expected_archive_sha256: archive_sha256.clone(),
        expected_archive_size: archive_size,
        forbidden_prefixes: request.forbidden_prefixes.clone(),
    };
    verify(&verification)?;
    revalidate_source_snapshot(&candidate_root, &source_snapshot)?;

    match fs::symlink_metadata(&output_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(ContractError::package(
                "SDK output directory appeared before publication",
            ));
        }
        Err(_) => {
            return Err(ContractError::package(
                "cannot recheck SDK output directory",
            ));
        }
    }
    publish_prepared_tree_noclobber(stage.path(), &output_dir)
        .map_err(|_| ContractError::package("cannot publish complete SDK package directory"))?;

    Ok(SdkPackageOutput {
        archive: output_dir.join(&asset),
        manifest: output_dir.join(format!("{asset}.manifest.json")),
        checksum: output_dir.join(format!("{asset}.sha256")),
        output_dir,
        archive_sha256,
        archive_size,
    })
}

/// Verify one complete local SDK package set without extracting it.
///
/// All three outer files must be regular, single-link files. The external
/// manifest must be canonical JSON and byte-identical to the embedded copy.
/// The caller supplies both archive hash and size; the sidecar is checked in
/// addition to those independent selections.
///
/// # Errors
///
/// Returns an error if the package directory or any member is unsafe, the
/// selected archive hash or size does not match, the manifest or checksum is
/// invalid, or the archive contents differ from the verified inventory.
pub fn verify(request: &SdkVerificationRequest) -> Result<VerifiedSdkPackage, ContractError> {
    request
        .identity
        .validate()
        .map_err(|error| ContractError::verification(format!("invalid SDK identity: {error}")))?;
    require_nonzero_digest(&request.expected_tree_sha256, false)?;
    require_nonzero_digest(&request.expected_archive_sha256, false)?;
    validate_forbidden_prefixes(&request.forbidden_prefixes, false)?;
    let package_dir = checked_existing_directory(&request.package_dir, false)?;
    let initial_directory_identity = directory_identity(&package_dir, false)?;
    let expected_names = selected_outer_names(request)?;
    require_selected_outer_members(&package_dir, &expected_names, true, false)?;
    let verified = verify_selected_members_in_release_directory(request)?;
    require_selected_outer_members(&package_dir, &expected_names, true, false)?;
    if directory_identity(&package_dir, false)? != initial_directory_identity {
        return Err(ContractError::verification(
            "SDK package directory changed during verification",
        ));
    }
    Ok(verified)
}

/// Verify the selected SDK archive, manifest and checksum in a shared release
/// directory that may contain other artifacts.
///
/// This keeps the same SDK identity, byte, inventory and mutation checks as
/// [`verify`], but deliberately omits only the containing directory's exact
/// three-member enumeration. A caller validating a shared release directory
/// must verify its exact union inventory before and after this call. This is a
/// byte-consistency check, not publication provenance or release authorization.
pub(crate) fn verify_selected_members_in_release_directory(
    request: &SdkVerificationRequest,
) -> Result<VerifiedSdkPackage, ContractError> {
    request
        .identity
        .validate()
        .map_err(|error| ContractError::verification(format!("invalid SDK identity: {error}")))?;
    require_nonzero_digest(&request.expected_tree_sha256, false)?;
    require_nonzero_digest(&request.expected_archive_sha256, false)?;
    let forbidden = validate_forbidden_prefixes(&request.forbidden_prefixes, false)?;
    let package_dir = checked_existing_directory(&request.package_dir, false)?;
    let package_identity = directory_identity(&package_dir, false)?;
    let (asset, expected_names) = selected_outer_names_for_identity(&request.identity)?;
    let archive_path = package_dir.join(&asset);
    let manifest_path = package_dir.join(format!("{asset}.manifest.json"));
    let checksum_path = package_dir.join(format!("{asset}.sha256"));
    require_selected_outer_members(&package_dir, &expected_names, false, false)?;

    let archive_file = open_outer_regular(&archive_path, false)?;
    let (archive_file, _, archive_measurement) =
        measure_open_file(archive_file, &archive_path, MAX_ARCHIVE_BYTES, None, false)?;
    if archive_measurement.size != request.expected_archive_size
        || archive_measurement.digest != request.expected_archive_sha256
    {
        return Err(ContractError::verification(
            "SDK archive differs from its independently selected hash or size",
        ));
    }
    let expected_sidecar = format!("{}  {}\n", archive_measurement.digest, asset);
    let sidecar = read_metadata(&checksum_path, SDK_MAX_METADATA_BYTES as u64, false)?;
    if sidecar != expected_sidecar.as_bytes() {
        return Err(ContractError::verification(
            "SDK checksum sidecar does not match the measured archive",
        ));
    }
    let manifest_bytes = read_metadata(&manifest_path, SDK_MAX_METADATA_BYTES as u64, false)?;
    let manifest = SdkManifest::from_bytes(&manifest_bytes)
        .map_err(|_| ContractError::verification("external SDK manifest is invalid"))?;
    if pretty_json(&manifest)
        .map_err(|_| ContractError::verification("cannot canonicalize external SDK manifest"))?
        != manifest_bytes
    {
        return Err(ContractError::verification(
            "external SDK manifest is not in canonical JSON form",
        ));
    }
    check_forbidden_bytes(&manifest_bytes, &forbidden, false)?;
    if manifest.identity != request.identity {
        return Err(ContractError::verification(
            "SDK manifest identity differs from the selected identity",
        ));
    }
    if manifest.tree_sha256 != request.expected_tree_sha256 {
        return Err(ContractError::verification(
            "SDK manifest tree digest differs from the selected tree digest",
        ));
    }

    let readback = verify_payload_archive(
        archive_file,
        payload_archive_contract(&request.identity),
        &request.forbidden_prefixes,
    )?;
    if readback.embedded_manifest != manifest_bytes {
        return Err(ContractError::verification(
            "external and embedded SDK manifests are not byte-identical",
        ));
    }
    if readback.entries != manifest.files || readback.tree_sha256 != manifest.tree_sha256.as_str() {
        return Err(ContractError::verification(
            "SDK archive payload inventory differs from its manifest",
        ));
    }

    // Re-read every selected outer input after archive parsing so a pathname
    // substitution or in-place update cannot be accepted on a stale first read.
    require_selected_outer_members(&package_dir, &expected_names, false, false)?;
    if read_metadata(&manifest_path, SDK_MAX_METADATA_BYTES as u64, false)? != manifest_bytes
        || read_metadata(&checksum_path, SDK_MAX_METADATA_BYTES as u64, false)?
            != expected_sidecar.as_bytes()
    {
        return Err(ContractError::verification(
            "SDK package metadata changed during verification",
        ));
    }
    let (_, final_archive) = measure_file(&archive_path, MAX_ARCHIVE_BYTES, None, false)?;
    if final_archive != archive_measurement
        || directory_identity(&package_dir, false)? != package_identity
    {
        return Err(ContractError::verification(
            "SDK archive changed during verification",
        ));
    }
    Ok(VerifiedSdkPackage {
        manifest,
        archive_sha256: archive_measurement.digest,
        archive_size: archive_measurement.size,
    })
}

fn selected_outer_names(
    request: &SdkVerificationRequest,
) -> Result<BTreeSet<String>, ContractError> {
    selected_outer_names_for_identity(&request.identity).map(|(_, names)| names)
}

fn selected_outer_names_for_identity(
    identity: &SdkIdentity,
) -> Result<(String, BTreeSet<String>), ContractError> {
    let asset = identity.asset_name().map_err(|error| {
        ContractError::verification(format!("invalid SDK asset identity: {error}"))
    })?;
    let names = BTreeSet::from([
        asset.clone(),
        format!("{asset}.manifest.json"),
        format!("{asset}.sha256"),
    ]);
    Ok((asset, names))
}

/// Verify a package, then extract its SDK members into one fresh retained root.
///
/// The output root is created only after package verification succeeds. Any
/// post-creation failure retains that root for inspection; existing material
/// is never adopted, replaced or removed.
///
/// # Errors
///
/// Returns an error if verification fails, the output root is unsafe or not
/// fresh and disjoint, or extraction and its post-extraction inventory checks
/// fail.
pub fn verify_and_extract(
    request: &SdkVerificationRequest,
    output_root: &Path,
) -> Result<VerifiedSdkPackage, ContractError> {
    let verified = verify(request)?;
    let package_dir = checked_existing_directory(&request.package_dir, false)?;
    let root = checked_absent_root(output_root, false)?;
    require_disjoint(&package_dir, &root, false)?;

    // Bind extraction to a second complete read-back and the measured archive
    // opened immediately before creating the retained destination.
    let verified_again = verify(request)?;
    if verified_again != verified {
        return Err(ContractError::verification(
            "SDK package changed between verification and extraction",
        ));
    }
    with_fresh_retained_root(&root, |root| {
        let asset = request
            .identity
            .asset_name()
            .map_err(|_| ContractError::verification("cannot derive SDK asset name"))?;
        let archive_path = package_dir.join(asset);
        let archive_file = open_outer_regular(&archive_path, false)?;
        let (archive_file, _, archive_measurement) =
            measure_open_file(archive_file, &archive_path, MAX_ARCHIVE_BYTES, None, false)?;
        if archive_measurement.digest != verified.archive_sha256
            || archive_measurement.size != verified.archive_size
        {
            return Err(ContractError::verification(
                "SDK archive changed before extraction",
            ));
        }
        let readback = extract_payload_archive(
            archive_file,
            payload_archive_contract(&request.identity),
            &request.forbidden_prefixes,
            root,
        )?;
        let manifest_path = root.join(SDK_MANIFEST_FILE);
        let extracted_manifest_bytes =
            read_metadata(&manifest_path, SDK_MAX_METADATA_BYTES as u64, false)?;
        if extracted_manifest_bytes != readback.embedded_manifest
            || extracted_manifest_bytes
                != pretty_json(&verified.manifest).map_err(|_| {
                    ContractError::verification("cannot canonicalize verified SDK manifest")
                })?
        {
            return Err(ContractError::verification(
                "extracted SDK manifest differs from verified archive metadata",
            ));
        }
        let extracted_scan = scan_sdk_tree(
            root,
            true,
            &validate_forbidden_prefixes(&request.forbidden_prefixes, false)?,
            SDK_MAX_REGULAR_BYTES + SDK_MAX_METADATA_BYTES as u64,
        )
        .map_err(|_| ContractError::verification("cannot safely inventory extracted SDK tree"))?;
        if extracted_scan.entries != verified.manifest.files
            || toolchain_inventory_sha256(&extracted_scan.entries).map_err(|_| {
                ContractError::verification("cannot serialize extracted SDK inventory")
            })? != verified.manifest.tree_sha256.as_str()
            || readback.entries != verified.manifest.files
            || readback.tree_sha256 != verified.manifest.tree_sha256.as_str()
        {
            return Err(ContractError::verification(
                "extracted SDK contents differ from the verified package inventory",
            ));
        }
        // Recheck the complete package after extraction; the retained
        // extraction remains available if selected input changed during read.
        if verify(request)? != verified {
            return Err(ContractError::verification(
                "SDK package changed while its verified contents were extracted",
            ));
        }
        Ok(verified)
    })
}

/// Verify a locally available SDK archive against an explicit release lock.
///
/// This checks archive and embedded-manifest bytes only. It does not establish
/// provenance, publisher authenticity or release authorization. Producer-side
/// forbidden-prefix checks remain the responsibility of packaging; this
/// consumer checks the exact bytes selected by the lock.
///
/// # Errors
///
/// Returns an error if the lock is invalid, the archive cannot be opened and
/// measured safely, its bytes differ from the selected archive digest/size,
/// or its structure, manifest or inventory differs from the selected lock.
pub fn verify_locked_archive(
    request: &SdkLockedArchiveRequest,
) -> Result<VerifiedSdkPackage, ContractError> {
    validate_sdk_release_lock(&request.lock)?;
    let (archive_file, opened_metadata, archive_measurement) =
        measure_locked_archive(&request.archive, &request.lock)?;
    let readback = verify_payload_archive(
        archive_file,
        payload_archive_contract(&request.lock.identity),
        &[],
    )?;
    let manifest = validate_locked_payload_readback(&request.lock, &readback)?;
    revalidate_archive_path(&request.archive, &opened_metadata, &archive_measurement)?;
    Ok(VerifiedSdkPackage {
        manifest,
        archive_sha256: archive_measurement.digest,
        archive_size: archive_measurement.size,
    })
}

/// Verify a locked archive and extract its payload into a fresh retained root.
///
/// The archive descriptor used for extraction is independently measured and
/// its own read-back is matched to the lock. Extraction does not rely on an
/// earlier pathname verification as authority for a reopened file. This is a
/// byte-consistency operation, not a provenance or release-authorization
/// check.
///
/// # Errors
///
/// Returns an error if lock verification fails, the output root is unsafe,
/// overlapping or already exists, or archive extraction, installed-tree
/// read-back or final archive revalidation fails. A root created before a
/// subsequent failure is retained for inspection.
pub fn extract_locked_archive(
    request: &SdkLockedArchiveRequest,
    output_root: &Path,
) -> Result<VerifiedSdkPackage, ContractError> {
    validate_sdk_release_lock(&request.lock)?;
    let verified = verify_locked_archive(request)?;
    let root = checked_absent_root(output_root, false)?;
    require_disjoint(&request.archive, &root, false)?;

    // Bind the extracting parser to a newly opened descriptor that itself
    // matches the lock, rather than relying on the earlier verification pass.
    let (archive_file, opened_metadata, archive_measurement) =
        measure_locked_archive(&request.archive, &request.lock)?;
    with_fresh_retained_root(&root, |root| {
        let readback = extract_payload_archive(
            archive_file,
            payload_archive_contract(&request.lock.identity),
            &[],
            root,
        )?;
        let manifest = validate_locked_payload_readback(&request.lock, &readback)?;
        if manifest != verified.manifest {
            return Err(ContractError::verification(
                "extracted SDK archive differs from its earlier locked read-back",
            ));
        }
        let installed = verify_installed_sdk(root, &request.lock)?;
        if installed != manifest {
            return Err(ContractError::verification(
                "extracted SDK tree differs from its archive manifest",
            ));
        }
        revalidate_archive_path(&request.archive, &opened_metadata, &archive_measurement)?;
        Ok(VerifiedSdkPackage {
            manifest: installed,
            archive_sha256: archive_measurement.digest,
            archive_size: archive_measurement.size,
        })
    })
}

/// Verify a fully installed SDK tree against its release lock.
///
/// The root must include the canonical bounded `sdk-manifest.json`; every
/// other filesystem object is inventoried from the installed bytes. This
/// proves local byte consistency only and makes no provenance or authenticity
/// claim.
///
/// # Errors
///
/// Returns an error if the lock, root, manifest bytes or installed filesystem
/// objects are invalid, exceed SDK resource bounds, change during measurement,
/// or differ from the selected identity, tree digest or manifest digest.
pub fn verify_installed_sdk(
    root: &Path,
    lock: &SdkReleaseLock,
) -> Result<SdkManifest, ContractError> {
    validate_sdk_release_lock(lock)?;
    let root = checked_existing_directory(root, false)?;
    let root_identity = directory_identity(&root, false)?;
    let limits = installed_sdk_snapshot_limits()?;
    let before = measure_tree_content_cas_bounded(&root, limits).map_err(|_| {
        ContractError::verification("cannot safely measure bounded installed SDK tree")
    })?;
    let manifest_path = root.join(SDK_MANIFEST_FILE);
    let manifest_bytes = read_metadata(&manifest_path, SDK_MAX_METADATA_BYTES as u64, false)?;
    let manifest = validate_locked_manifest_bytes(lock, &manifest_bytes)?;
    let scan = scan_sdk_tree(
        &root,
        true,
        &[],
        SDK_MAX_REGULAR_BYTES + SDK_MAX_METADATA_BYTES as u64,
    )
    .map_err(|_| ContractError::verification("cannot safely inventory installed SDK tree"))?;
    if scan.entries != manifest.files {
        return Err(ContractError::verification(
            "installed SDK inventory differs from its selected manifest",
        ));
    }
    let tree_sha256 = toolchain_inventory_sha256(&scan.entries)
        .map_err(|_| ContractError::verification("cannot serialize installed SDK inventory"))?;
    if tree_sha256 != lock.tree_sha256.as_str() {
        return Err(ContractError::verification(
            "installed SDK inventory differs from the selected tree digest",
        ));
    }

    let after = measure_tree_content_cas_bounded(&root, limits)
        .map_err(|_| ContractError::verification("cannot recheck bounded installed SDK tree"))?;
    if before != after
        || directory_identity(&root, false)? != root_identity
        || read_metadata(&manifest_path, SDK_MAX_METADATA_BYTES as u64, false)? != manifest_bytes
    {
        return Err(ContractError::verification(
            "installed SDK tree or manifest changed during verification",
        ));
    }
    Ok(manifest)
}

/// Run extraction work against a newly created destination while retaining it
/// on every subsequent error for caller inspection.
fn with_fresh_retained_root<T>(
    root: &Path,
    operation: impl FnOnce(&Path) -> Result<T, ContractError>,
) -> Result<T, ContractError> {
    fs::create_dir(root)
        .map_err(|_| ContractError::verification("cannot create fresh SDK extraction root"))?;
    aros_common::ensure_directory_nofollow(root).map_err(|_| {
        ContractError::verification("fresh SDK extraction root is not a real directory")
    })?;
    operation(root)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileMeasurement {
    digest: Sha256Digest,
    size: u64,
}

fn sdk_snapshot_limits() -> Result<TreeTraversalLimits, ContractError> {
    TreeTraversalLimits::new(SDK_MAX_ENTRIES, SDK_MAX_REGULAR_BYTES)
        .map_err(|_| ContractError::package("invalid bounded SDK snapshot policy"))
}

const fn payload_archive_contract(identity: &SdkIdentity) -> PayloadArchiveContract<'_> {
    PayloadArchiveContract {
        archive_root: ARCHIVE_ROOT,
        manifest_file: SDK_MANIFEST_FILE,
        source_date_epoch: identity.source_date_epoch,
        capture_toolchain_tools: false,
        max_entries: (SDK_MAX_ENTRIES as u64) + 2,
        max_regular_file_bytes: SDK_MAX_REGULAR_BYTES + SDK_MAX_METADATA_BYTES as u64,
    }
}

fn validate_sdk_release_lock(lock: &SdkReleaseLock) -> Result<(), ContractError> {
    lock.validate()
        .map_err(|error| ContractError::verification(format!("invalid SDK release lock: {error}")))
}

fn measure_locked_archive(
    path: &Path,
    lock: &SdkReleaseLock,
) -> Result<(File, fs::Metadata, FileMeasurement), ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::verification(
            "locked SDK archive path must be absolute",
        ));
    }
    let archive_file = open_outer_regular(path, false)?;
    let (archive_file, metadata, measurement) =
        measure_open_file(archive_file, path, MAX_ARCHIVE_BYTES, None, false)?;
    if measurement.digest != lock.archive.sha256 || measurement.size != lock.archive.size {
        return Err(ContractError::verification(
            "SDK archive differs from its locked hash or size",
        ));
    }
    Ok((archive_file, metadata, measurement))
}

fn revalidate_archive_path(
    path: &Path,
    opened_metadata: &fs::Metadata,
    opened_measurement: &FileMeasurement,
) -> Result<(), ContractError> {
    let (current_metadata, current_measurement) =
        measure_file(path, MAX_ARCHIVE_BYTES, None, false)?;
    if metadata_identity(opened_metadata) != metadata_identity(&current_metadata)
        || &current_measurement != opened_measurement
    {
        return Err(ContractError::verification(
            "locked SDK archive path or bytes changed during read-back",
        ));
    }
    Ok(())
}

fn validate_locked_payload_readback(
    lock: &SdkReleaseLock,
    readback: &PayloadArchiveReadback,
) -> Result<SdkManifest, ContractError> {
    let manifest = validate_locked_manifest_bytes(lock, &readback.embedded_manifest)?;
    if readback.entries != manifest.files {
        return Err(ContractError::verification(
            "SDK archive payload inventory differs from its locked manifest",
        ));
    }
    if readback.tree_sha256 != manifest.tree_sha256.as_str() {
        return Err(ContractError::verification(
            "SDK archive payload tree digest differs from its locked manifest",
        ));
    }
    Ok(manifest)
}

fn validate_locked_manifest_bytes(
    lock: &SdkReleaseLock,
    bytes: &[u8],
) -> Result<SdkManifest, ContractError> {
    if u64::try_from(bytes.len()).ok() != Some(lock.manifest.size)
        || sha256_bytes(bytes) != lock.manifest.sha256
    {
        return Err(ContractError::verification(
            "SDK manifest bytes differ from their locked hash or size",
        ));
    }
    let manifest = SdkManifest::from_bytes(bytes)
        .map_err(|_| ContractError::verification("locked SDK manifest is invalid"))?;
    if bounded_pretty_manifest(&manifest, false)? != bytes {
        return Err(ContractError::verification(
            "locked SDK manifest is not canonical JSON",
        ));
    }
    if manifest.identity != lock.identity {
        return Err(ContractError::verification(
            "locked SDK manifest identity differs from the selected identity",
        ));
    }
    if manifest.tree_sha256 != lock.tree_sha256 {
        return Err(ContractError::verification(
            "locked SDK manifest tree digest differs from the selected tree digest",
        ));
    }
    Ok(manifest)
}

fn installed_sdk_snapshot_limits() -> Result<TreeTraversalLimits, ContractError> {
    let max_entries = SDK_MAX_ENTRIES.saturating_add(1);
    let max_regular_bytes = SDK_MAX_REGULAR_BYTES
        .checked_add(SDK_MAX_METADATA_BYTES as u64)
        .ok_or_else(|| ContractError::verification("installed SDK byte limit overflow"))?;
    TreeTraversalLimits::new(max_entries, max_regular_bytes)
        .map_err(|_| ContractError::verification("invalid bounded installed SDK policy"))
}

/// Bound the SDK manifest's canonical pretty-JSON size before the shared
/// serializer builds a whole-document `serde_json::Value` tree. The estimate
/// computes each inventory entry independently, so an oversized manifest is
/// rejected before memory grows with the complete `files` array.
pub(crate) fn bounded_pretty_manifest(
    manifest: &SdkManifest,
    package: bool,
) -> Result<Vec<u8>, ContractError> {
    // Every serialized string contributes at least its original UTF-8 byte
    // length. Reject clearly oversized inventories before serde allocates a
    // rendered entry (and before the whole-document serializer runs).
    let raw_string_bytes = manifest.files.iter().try_fold(0_u64, |total, entry| {
        [
            Some(entry.path.as_str()),
            Some(entry.mode.as_str()),
            Some(entry.kind.as_str()),
            entry.sha256.as_deref(),
            entry.target.as_deref(),
        ]
        .into_iter()
        .flatten()
        .try_fold(total, |subtotal, value| {
            subtotal.checked_add(value.len() as u64)
        })
    });
    if raw_string_bytes.is_none_or(|size| size > SDK_MAX_METADATA_BYTES as u64) {
        return Err(operation_error(
            package,
            "SDK manifest exceeds its metadata byte limit",
        ));
    }

    let identity = serde_json::to_value(&manifest.identity)
        .map_err(|_| operation_error(package, "cannot encode SDK identity metadata"))?;
    let mut skeleton = serde_json::Map::new();
    skeleton.insert("files".into(), serde_json::Value::Array(Vec::new()));
    skeleton.insert("identity".into(), identity);
    skeleton.insert(
        "schema".into(),
        serde_json::Value::String(manifest.schema.clone()),
    );
    skeleton.insert(
        "tree_sha256".into(),
        serde_json::to_value(&manifest.tree_sha256)
            .map_err(|_| operation_error(package, "cannot encode SDK tree digest metadata"))?,
    );
    let skeleton = serde_json::to_string_pretty(&serde_json::Value::Object(skeleton))
        .map_err(|_| operation_error(package, "cannot bound SDK manifest metadata"))?;
    let marker = skeleton
        .find("\"files\": []")
        .map(|offset| offset + "\"files\": ".len())
        .ok_or_else(|| operation_error(package, "cannot locate SDK manifest inventory"))?;
    if skeleton.as_bytes().get(marker..marker + 2) != Some(b"[]") {
        return Err(operation_error(
            package,
            "cannot bound SDK manifest inventory serialization",
        ));
    }

    let mut array_size = if manifest.files.is_empty() {
        2_u64
    } else {
        6_u64
    };
    for (index, entry) in manifest.files.iter().enumerate() {
        let value = serde_json::to_value(entry)
            .map_err(|_| operation_error(package, "cannot encode SDK inventory metadata"))?;
        let rendered = serde_json::to_string_pretty(&value)
            .map_err(|_| operation_error(package, "cannot bound SDK inventory metadata"))?;
        let lines = u64::try_from(rendered.bytes().filter(|byte| *byte == b'\n').count() + 1)
            .map_err(|_| operation_error(package, "SDK manifest metadata size overflow"))?;
        let entry_size = canonical_pretty_json_len(&rendered)
            .and_then(|size| size.checked_add(lines.checked_mul(4)?))
            .ok_or_else(|| operation_error(package, "SDK manifest metadata size overflow"))?;
        array_size = array_size
            .checked_add(entry_size)
            .and_then(|size| size.checked_add(if index > 0 { 2 } else { 0 }))
            .filter(|size| *size <= SDK_MAX_METADATA_BYTES as u64 + 2)
            .ok_or_else(|| {
                operation_error(package, "SDK manifest exceeds its metadata byte limit")
            })?;
    }
    let complete_size = canonical_pretty_json_len(&skeleton)
        .and_then(|size| size.checked_sub(2))
        .and_then(|size| size.checked_add(array_size))
        .and_then(|size| size.checked_add(1)) // final newline from `pretty_json`
        .filter(|size| *size <= SDK_MAX_METADATA_BYTES as u64)
        .ok_or_else(|| operation_error(package, "SDK manifest exceeds its metadata byte limit"))?;
    let bytes = pretty_json(manifest)?;
    if u64::try_from(bytes.len()).ok() != Some(complete_size)
        || bytes.len() > SDK_MAX_METADATA_BYTES
    {
        return Err(operation_error(
            package,
            "serialized SDK manifest exceeds or differs from its bounded size estimate",
        ));
    }
    Ok(bytes)
}

/// Match `pretty_json`'s non-ASCII-to-UTF-16-escape transformation without
/// allocating another copy of the serialized document.
fn canonical_pretty_json_len(rendered: &str) -> Option<u64> {
    rendered.chars().try_fold(0_u64, |total, character| {
        let addition = if character.is_ascii() {
            1
        } else {
            u64::try_from(character.encode_utf16(&mut [0_u16; 2]).len())
                .ok()?
                .checked_mul(6)?
        };
        total.checked_add(addition)
    })
}

fn checked_existing_directory(path: &Path, package: bool) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(operation_error(
            package,
            "SDK directory must be an absolute path",
        ));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| operation_error(package, "SDK directory is inaccessible"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(operation_error(
            package,
            "SDK directory must be a real directory",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| operation_error(package, "SDK directory cannot be canonicalized"))?;
    aros_common::ensure_directory_nofollow(&canonical)
        .map_err(|_| operation_error(package, "SDK directory has an unsafe path component"))?;
    Ok(canonical)
}

fn directory_identity(path: &Path, package: bool) -> Result<(u64, u64), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| operation_error(package, "cannot recheck SDK package directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(operation_error(
            package,
            "SDK package directory changed or is not a real directory",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn checked_absent_root(path: &Path, package: bool) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(operation_error(
            package,
            "SDK output directory must be absolute",
        ));
    }
    let name = path
        .file_name()
        .filter(|name| {
            matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        })
        .ok_or_else(|| operation_error(package, "SDK output directory needs a safe final name"))?;
    let parent = path
        .parent()
        .ok_or_else(|| operation_error(package, "SDK output directory has no parent"))?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| operation_error(package, "SDK output parent must already exist"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(operation_error(
            package,
            "SDK output parent must be a real directory",
        ));
    }
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| operation_error(package, "SDK output parent cannot be canonicalized"))?;
    aros_common::ensure_directory_nofollow(&canonical_parent)
        .map_err(|_| operation_error(package, "SDK output parent has an unsafe path component"))?;
    let output = canonical_parent.join(name);
    match fs::symlink_metadata(&output) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(output),
        Ok(_) => Err(operation_error(
            package,
            "SDK output directory already exists",
        )),
        Err(_) => Err(operation_error(
            package,
            "cannot inspect SDK output directory",
        )),
    }
}

fn require_disjoint(left: &Path, right: &Path, package: bool) -> Result<(), ContractError> {
    if left.starts_with(right) || right.starts_with(left) {
        return Err(operation_error(
            package,
            "SDK input and output roots must be disjoint",
        ));
    }
    Ok(())
}

fn require_nonzero_digest(digest: &Sha256Digest, package: bool) -> Result<(), ContractError> {
    if digest.as_str().bytes().all(|byte| byte == b'0') {
        return Err(operation_error(
            package,
            "selected SDK digest must be nonzero",
        ));
    }
    Ok(())
}

fn validate_forbidden_prefixes(
    prefixes: &[PathBuf],
    package: bool,
) -> Result<Vec<Vec<u8>>, ContractError> {
    prefixes
        .iter()
        .map(|path| {
            let value = path
                .to_str()
                .ok_or_else(|| operation_error(package, "forbidden SDK prefixes must be UTF-8"))?;
            if !path.is_absolute() || value.is_empty() || value.len() > MAX_FORBIDDEN_PREFIX_BYTES {
                return Err(operation_error(
                    package,
                    "forbidden SDK prefixes must be bounded nonempty absolute paths",
                ));
            }
            Ok(value.as_bytes().to_vec())
        })
        .collect()
}

fn validate_candidate_source(root: &Path, expected: &TreeContentCas) -> Result<(), ContractError> {
    let mut count = 0_usize;
    let limits = sdk_snapshot_limits()?;
    visit_source_for_links(root, Path::new(""), &mut count, limits.max_entries)?;
    if count != expected.entry_count() {
        return Err(ContractError::package(
            "SDK candidate changed while source hardlinks were checked",
        ));
    }
    Ok(())
}

fn visit_source_for_links(
    root: &Path,
    relative: &Path,
    count: &mut usize,
    max_entries: usize,
) -> Result<(), ContractError> {
    let directory = root.join(relative);
    let remaining = max_entries.saturating_sub(*count).max(1);
    let names = directory_entry_names_nofollow_bounded(&directory, remaining)
        .map_err(|_| ContractError::package("cannot safely enumerate SDK candidate"))?;
    for name in names {
        *count = count
            .checked_add(1)
            .ok_or_else(|| ContractError::package("SDK candidate entry count overflow"))?;
        if *count > max_entries {
            return Err(ContractError::package(
                "SDK candidate exceeds its entry limit",
            ));
        }
        let child = relative.join(&name);
        if relative.as_os_str().is_empty() && child == Path::new(SDK_MANIFEST_FILE) {
            return Err(ContractError::package(
                "SDK candidate already contains reserved SDK manifest metadata",
            ));
        }
        let path = root.join(&child);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ContractError::package("cannot inspect SDK candidate entry"))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            visit_source_for_links(root, &child, count, max_entries)?;
        } else if metadata.is_file() {
            if metadata.nlink() != 1 {
                return Err(ContractError::package(
                    "SDK candidate contains a hardlinked regular file",
                ));
            }
        } else {
            return Err(ContractError::package(
                "SDK candidate contains an unsupported filesystem object",
            ));
        }
    }
    Ok(())
}

fn revalidate_source_snapshot(root: &Path, expected: &TreeContentCas) -> Result<(), ContractError> {
    let actual = measure_tree_content_cas_bounded(root, sdk_snapshot_limits()?)
        .map_err(|_| ContractError::package("cannot revalidate SDK candidate snapshot"))?;
    if &actual != expected {
        return Err(ContractError::package(
            "SDK candidate changed during package preparation",
        ));
    }
    Ok(())
}

struct InventoryScan {
    entries: Vec<ArosToolchainManifestEntry>,
}

fn scan_sdk_tree(
    root: &Path,
    exclude_manifest: bool,
    prefixes: &[Vec<u8>],
    max_regular_bytes: u64,
) -> Result<InventoryScan, String> {
    let mut entries = Vec::new();
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    scan_directory(
        root,
        Path::new(""),
        exclude_manifest,
        prefixes,
        &mut count,
        &mut bytes,
        if exclude_manifest {
            SDK_MAX_ENTRIES + 1
        } else {
            SDK_MAX_ENTRIES
        },
        max_regular_bytes,
        &mut entries,
    )?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    aros_common::sdk_manifest::validate_inventory(&entries)?;
    Ok(InventoryScan { entries })
}

#[allow(clippy::too_many_arguments)]
fn scan_directory(
    root: &Path,
    relative: &Path,
    exclude_manifest: bool,
    prefixes: &[Vec<u8>],
    count: &mut usize,
    bytes: &mut u64,
    max_entries: usize,
    max_regular_bytes: u64,
    output: &mut Vec<ArosToolchainManifestEntry>,
) -> Result<(), String> {
    let directory = root.join(relative);
    let remaining = max_entries.saturating_sub(*count).max(1);
    let names = directory_entry_names_nofollow_bounded(&directory, remaining)
        .map_err(|_| "cannot safely enumerate SDK directory")?;
    for name in names {
        *count = count.checked_add(1).ok_or("SDK entry count overflow")?;
        if *count > max_entries {
            return Err("SDK exceeds its entry limit".into());
        }
        let child = relative.join(&name);
        let path_text = child
            .to_str()
            .ok_or("SDK path is not UTF-8")?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let path = root.join(&child);
        let metadata = fs::symlink_metadata(&path).map_err(|_| "cannot inspect SDK entry")?;
        let is_manifest = exclude_manifest && path_text == SDK_MANIFEST_FILE;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|_| "cannot read SDK symbolic link")?;
            let target = target
                .to_str()
                .ok_or("SDK symbolic-link target is not UTF-8")?;
            if !is_manifest {
                output.push(ArosToolchainManifestEntry {
                    path: path_text,
                    mode: "0777".into(),
                    kind: "symlink".into(),
                    sha256: None,
                    size: None,
                    target: Some(target.into()),
                });
            }
        } else if metadata.is_dir() {
            // Source candidates are normalized by the archive writer. An
            // extracted/installed tree must retain those exact payload modes,
            // rather than treating chmod (including privilege bits) as benign.
            if exclude_manifest && metadata.permissions().mode() & 0o7777 != 0o755 {
                return Err(
                    "installed SDK directory mode differs from normalized archive metadata".into(),
                );
            }
            output.push(ArosToolchainManifestEntry {
                path: path_text,
                mode: "0755".into(),
                kind: "directory".into(),
                sha256: None,
                size: None,
                target: None,
            });
            scan_directory(
                root,
                &child,
                exclude_manifest,
                prefixes,
                count,
                bytes,
                max_entries,
                max_regular_bytes,
                output,
            )?;
        } else if metadata.is_file() {
            let file_limit = if is_manifest {
                SDK_MAX_METADATA_BYTES as u64
            } else {
                max_regular_bytes
            };
            let (mode, measurement) = measure_file(&path, file_limit, Some(prefixes), false)
                .map_err(|error| {
                    format!(
                        "cannot safely hash SDK regular file '{path_text}': {}",
                        error.diagnostics().diagnostics[0].message
                    )
                })?;
            if exclude_manifest {
                let expected_mode = if is_manifest || mode.permissions().mode() & 0o111 == 0 {
                    0o644
                } else {
                    0o755
                };
                if mode.permissions().mode() & 0o7777 != expected_mode {
                    return Err(
                        "installed SDK regular-file mode differs from normalized archive metadata"
                            .into(),
                    );
                }
            }
            *bytes = bytes
                .checked_add(measurement.size)
                .filter(|total| *total <= max_regular_bytes)
                .ok_or("SDK regular files exceed their byte limit")?;
            if !is_manifest {
                output.push(ArosToolchainManifestEntry {
                    path: path_text,
                    mode: if mode.permissions().mode() & 0o111 == 0 {
                        "0644".into()
                    } else {
                        "0755".into()
                    },
                    kind: "file".into(),
                    sha256: Some(measurement.digest.to_string()),
                    size: Some(measurement.size),
                    target: None,
                });
            }
        } else {
            return Err("SDK contains an unsupported filesystem object".into());
        }
    }
    Ok(())
}

/// Open a regular file without following any path component, stream its hash,
/// enforce an optional content scan, and prove its descriptor/path metadata
/// stayed stable throughout the read.
fn measure_file(
    path: &Path,
    max_bytes: u64,
    prefixes: Option<&[Vec<u8>]>,
    package: bool,
) -> Result<(fs::Metadata, FileMeasurement), ContractError> {
    let file = open_regular_file_nofollow(path)
        .map_err(|_| operation_error(package, "cannot safely open regular SDK file"))?;
    let (_, metadata, measurement) = measure_open_file(file, path, max_bytes, prefixes, package)?;
    Ok((metadata, measurement))
}

/// Measure one already opened no-follow file and return the same descriptor
/// rewound to byte zero. Archive verification and extraction consume this
/// descriptor after its hash/size precondition has been established.
fn measure_open_file(
    file: File,
    path: &Path,
    max_bytes: u64,
    prefixes: Option<&[Vec<u8>]>,
    package: bool,
) -> Result<(File, fs::Metadata, FileMeasurement), ContractError> {
    let before = file
        .metadata()
        .map_err(|_| operation_error(package, "cannot inspect regular SDK file"))?;
    if !before.is_file() || before.nlink() != 1 || before.len() > max_bytes {
        return Err(operation_error(
            package,
            "SDK regular file is linked, non-regular or over its byte limit",
        ));
    }
    let mut file = file;
    let mut hasher = Sha256::new();
    let mut read_size = 0_u64;
    let mut buffer = vec![0_u8; SCAN_BUFFER_BYTES];
    let needles = prefixes.unwrap_or_default();
    let max_overlap = needles
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut overlap = Vec::new();
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| operation_error(package, "cannot read regular SDK file"))?;
        if read == 0 {
            break;
        }
        read_size = read_size
            .checked_add(read as u64)
            .filter(|size| *size <= max_bytes)
            .ok_or_else(|| operation_error(package, "SDK regular file exceeds its byte limit"))?;
        hasher.update(&buffer[..read]);
        if !needles.is_empty() {
            let mut window = Vec::with_capacity(overlap.len() + read);
            window.extend_from_slice(&overlap);
            window.extend_from_slice(&buffer[..read]);
            if needles
                .iter()
                .any(|needle| !needle.is_empty() && contains_bytes(&window, needle))
            {
                return Err(operation_error(
                    package,
                    "SDK regular file contains a forbidden build prefix",
                ));
            }
            let keep = max_overlap.min(window.len());
            overlap.clear();
            overlap.extend_from_slice(&window[window.len() - keep..]);
        }
    }
    let after = file
        .metadata()
        .map_err(|_| operation_error(package, "cannot recheck regular SDK file"))?;
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|_| operation_error(package, "regular SDK file path changed while reading"))?;
    if metadata_identity(&before) != metadata_identity(&after)
        || metadata_identity(&after) != metadata_identity(&path_metadata)
        || read_size != before.len()
    {
        return Err(operation_error(
            package,
            "SDK regular file changed while it was read",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| operation_error(package, "cannot rewind regular SDK file"))?;
    Ok((
        file,
        before,
        FileMeasurement {
            digest: finish_sha256(hasher),
            size: read_size,
        },
    ))
}

fn metadata_identity(metadata: &fs::Metadata) -> (u64, u64, u64, u32, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.nlink(),
        metadata.mode(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn read_metadata(path: &Path, max_bytes: u64, package: bool) -> Result<Vec<u8>, ContractError> {
    let mut file = open_outer_regular(path, package)?;
    let before = file
        .metadata()
        .map_err(|_| operation_error(package, "cannot inspect SDK metadata"))?;
    if before.len() > max_bytes {
        return Err(operation_error(
            package,
            "SDK metadata exceeds its byte limit",
        ));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(before.len())
            .map_err(|_| operation_error(package, "SDK metadata exceeds addressable memory"))?,
    );
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| operation_error(package, "cannot read SDK metadata"))?;
    let after = file
        .metadata()
        .map_err(|_| operation_error(package, "cannot recheck SDK metadata"))?;
    if bytes.len() as u64 != before.len() || metadata_identity(&before) != metadata_identity(&after)
    {
        return Err(operation_error(
            package,
            "SDK metadata changed while it was read",
        ));
    }
    Ok(bytes)
}

fn open_outer_regular(path: &Path, package: bool) -> Result<File, ContractError> {
    let file = open_regular_file_nofollow(path)
        .map_err(|_| operation_error(package, "cannot safely open SDK package member"))?;
    let metadata = file
        .metadata()
        .map_err(|_| operation_error(package, "cannot inspect SDK package member"))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(operation_error(
            package,
            "SDK package member must be a single-link regular file",
        ));
    }
    Ok(file)
}

fn require_selected_outer_members(
    directory: &Path,
    expected: &BTreeSet<String>,
    require_exact_directory: bool,
    package: bool,
) -> Result<(), ContractError> {
    if require_exact_directory {
        let names = directory_entry_names_nofollow_bounded(directory, expected.len() + 1)
            .map_err(|_| operation_error(package, "cannot enumerate SDK package directory"))?;
        let actual = names
            .into_iter()
            .map(|name| {
                name.into_string()
                    .map_err(|_| operation_error(package, "SDK package member name is not UTF-8"))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if actual != *expected {
            return Err(operation_error(
                package,
                "SDK package directory must contain exactly the three required files",
            ));
        }
    }
    for name in expected {
        open_outer_regular(&directory.join(name), package)?;
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8], package: bool) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| operation_error(package, "cannot create staged SDK package file"))?;
    file.write_all(bytes)
        .map_err(|_| operation_error(package, "cannot write staged SDK package file"))?;
    file.sync_all()
        .map_err(|_| operation_error(package, "cannot synchronize staged SDK package file"))
}

fn private_sibling_stage(parent: &Path, prefix: &str) -> Result<TempDir, ContractError> {
    TempDirBuilder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .map_err(|_| ContractError::package("cannot create private SDK package staging directory"))
}

fn check_forbidden_bytes(
    bytes: &[u8],
    prefixes: &[Vec<u8>],
    package: bool,
) -> Result<(), ContractError> {
    if prefixes
        .iter()
        .any(|needle| !needle.is_empty() && contains_bytes(bytes, needle))
    {
        return Err(operation_error(
            package,
            "SDK metadata contains a forbidden build prefix",
        ));
    }
    Ok(())
}

fn operation_error(package: bool, message: impl Into<String>) -> ContractError {
    if package {
        ContractError::package(message)
    } else {
        ContractError::verification(message)
    }
}

#[cfg(test)]
mod manifest_size_tests {
    use std::collections::BTreeMap;

    use aros_common::sdk_manifest::{SdkAbiValue, SdkSourceIdentity, SdkTarget};

    use super::*;

    #[test]
    fn canonical_pretty_json_size_counts_ascii_and_unicode_escapes() {
        assert_eq!(canonical_pretty_json_len("ascii"), Some(5));
        assert_eq!(canonical_pretty_json_len("é"), Some(6));
        // A non-BMP scalar becomes the two UTF-16 surrogate escapes emitted by
        // `pretty_json`'s ensure-ASCII encoding.
        assert_eq!(canonical_pretty_json_len("😀"), Some(12));
    }

    #[test]
    fn bounded_manifest_serialization_accounts_for_ascii_and_unicode() {
        for path in ["bin/tool", "bin/é😀"] {
            let bytes = bounded_pretty_manifest(&manifest_with_path(path), true)
                .expect("small manifest should serialize within its limit");
            assert!(bytes.ends_with(b"\n"));
            if path.contains('é') {
                assert!(bytes
                    .windows(b"\\u00e9\\ud83d\\ude00".len())
                    .any(|window| window == b"\\u00e9\\ud83d\\ude00"));
            }
        }
    }

    #[test]
    fn bounded_manifest_rejects_oversized_inventory_before_rendering_document() {
        let oversized_path = "x".repeat(SDK_MAX_METADATA_BYTES + 1);
        let error = bounded_pretty_manifest(&manifest_with_path(&oversized_path), true)
            .expect_err("oversized inventory string must exceed metadata ceiling");
        assert!(error.to_string().contains("metadata byte limit"));
    }

    #[test]
    fn failed_extraction_retains_its_fresh_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root =
            checked_absent_root(&temporary.path().join("retained-extraction"), false).unwrap();
        let result: Result<(), ContractError> = with_fresh_retained_root(&root, |_| {
            Err(ContractError::verification("injected extraction failure"))
        });
        assert!(result.is_err());
        assert!(root.is_dir());
    }

    fn manifest_with_path(path: &str) -> SdkManifest {
        let digest = |byte: char| Sha256Digest::parse(&byte.to_string().repeat(64)).unwrap();
        SdkManifest {
            schema: SDK_MANIFEST_SCHEMA.into(),
            identity: SdkIdentity {
                sdk_id: "test-sdk".into(),
                version: "1.0.0".into(),
                target: SdkTarget {
                    target_triple: "riscv32-aros".into(),
                    abi: BTreeMap::from([("little_endian".into(), SdkAbiValue::Boolean(true))]),
                },
                source: SdkSourceIdentity {
                    commit: "1".repeat(40),
                    git_tree: "2".repeat(40),
                    content_sha256: digest('3'),
                    contract_sha256: digest('4'),
                },
                source_date_epoch: 0,
            },
            tree_sha256: digest('5'),
            files: vec![ArosToolchainManifestEntry {
                path: path.into(),
                mode: "0755".into(),
                kind: "file".into(),
                sha256: Some(digest('6').to_string()),
                size: Some(1),
                target: None,
            }],
        }
    }
}

#[cfg(test)]
mod selected_member_verifier_tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;

    use aros_common::sdk_manifest::{SdkAbiValue, SdkSourceIdentity, SdkTarget};
    use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest};

    use super::*;

    struct Fixture {
        temporary: tempfile::TempDir,
        package_dir: PathBuf,
        request: SdkVerificationRequest,
    }

    fn fixture() -> Fixture {
        let temporary = tempfile::tempdir().unwrap();
        let candidate = temporary.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        fs::create_dir(candidate.join("bin")).unwrap();
        fs::create_dir(candidate.join("include")).unwrap();
        fs::write(candidate.join("bin/aros"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            candidate.join("bin/aros"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::write(candidate.join("include/aros.h"), b"#define AROS_SDK 1\n").unwrap();
        fs::write(
            candidate.join("toolchain-manifest.json"),
            b"synthetic source\n",
        )
        .unwrap();
        symlink("bin/aros", candidate.join("aros")).unwrap();

        let (tree, _) =
            toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"]).unwrap();
        let identity = SdkIdentity {
            sdk_id: "core".into(),
            version: "1.2.3".into(),
            target: SdkTarget {
                target_triple: "i686-aros".into(),
                abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
            },
            source: SdkSourceIdentity {
                commit: "1".repeat(40),
                git_tree: "2".repeat(40),
                content_sha256: sha256_bytes(b"synthetic SDK source"),
                contract_sha256: sha256_bytes(b"synthetic SDK contract"),
            },
            source_date_epoch: 946_684_800,
        };
        let package_dir = temporary.path().join("package");
        let output = package(&SdkPackageRequest {
            candidate_root: candidate,
            output_dir: package_dir.clone(),
            identity: identity.clone(),
            expected_tree_sha256: Sha256Digest::parse(&tree).unwrap(),
            forbidden_prefixes: vec![PathBuf::from("/tmp/synthetic-sdk-private-root")],
        })
        .unwrap();
        Fixture {
            temporary,
            package_dir: package_dir.clone(),
            request: SdkVerificationRequest {
                package_dir,
                identity,
                expected_tree_sha256: Sha256Digest::parse(&tree).unwrap(),
                expected_archive_sha256: output.archive_sha256,
                expected_archive_size: output.archive_size,
                forbidden_prefixes: vec![PathBuf::from("/tmp/synthetic-sdk-private-root")],
            },
        }
    }

    fn archive_path(request: &SdkVerificationRequest) -> PathBuf {
        request
            .package_dir
            .join(request.identity.asset_name().unwrap())
    }

    fn assert_internal_rejects_with(request: &SdkVerificationRequest, expected: &str) {
        let error = verify_selected_members_in_release_directory(request)
            .expect_err("selected SDK package mutation must be rejected");
        assert!(
            error.to_string().contains(expected),
            "expected rejection containing {expected:?}, got: {error}"
        );
    }

    fn assert_public_rejects_with(request: &SdkVerificationRequest, expected: &str) {
        let error = verify(request).expect_err("invalid SDK package must be rejected");
        assert!(
            error.to_string().contains(expected),
            "expected rejection containing {expected:?}, got: {error}"
        );
    }

    #[test]
    fn exact_sdk_package_passes_public_and_selected_member_verifiers() {
        let fixture = fixture();
        assert_eq!(
            verify(&fixture.request).unwrap(),
            verify_selected_members_in_release_directory(&fixture.request).unwrap()
        );
    }

    #[test]
    fn selected_member_verifier_allows_only_unselected_siblings() {
        let fixture = fixture();
        fs::write(
            fixture.package_dir.join("toolchain-index-v2.json"),
            b"sibling release artifact\n",
        )
        .unwrap();

        let error = verify(&fixture.request).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exactly the three required files"),
            "unexpected public inventory rejection: {error}"
        );
        let verified = verify_selected_members_in_release_directory(&fixture.request).unwrap();
        assert_eq!(
            verified.archive_sha256,
            fixture.request.expected_archive_sha256
        );
        assert_eq!(verified.archive_size, fixture.request.expected_archive_size);
    }

    #[test]
    fn selected_member_verifier_rejects_hardlinks_to_each_selected_member() {
        for member in ["archive", "manifest", "checksum"] {
            let fixture = fixture();
            let asset = fixture.request.identity.asset_name().unwrap();
            let path = match member {
                "archive" => fixture.package_dir.join(&asset),
                "manifest" => fixture.package_dir.join(format!("{asset}.manifest.json")),
                "checksum" => fixture.package_dir.join(format!("{asset}.sha256")),
                _ => unreachable!(),
            };
            fs::hard_link(
                &path,
                fixture.temporary.path().join(format!("{member}-hardlink")),
            )
            .unwrap();
            assert_internal_rejects_with(&fixture.request, "single-link regular file");
        }
    }

    #[test]
    fn selected_member_verifier_rejects_altered_archive_and_missing_member() {
        let altered = fixture();
        let archive = archive_path(&altered.request);
        let mut bytes = fs::read(&archive).unwrap();
        bytes[0] ^= 1;
        fs::write(archive, bytes).unwrap();
        assert_internal_rejects_with(&altered.request, "independently selected hash or size");

        let missing = fixture();
        let manifest = missing.package_dir.join(format!(
            "{}.manifest.json",
            missing.request.identity.asset_name().unwrap()
        ));
        fs::remove_file(manifest).unwrap();
        assert_internal_rejects_with(&missing.request, "cannot safely open SDK package member");
    }

    #[test]
    fn public_and_selected_member_verifiers_reject_wrong_nonzero_tree_digest() {
        let fixture = fixture();
        let mut wrong_tree = fixture.request;
        wrong_tree.expected_tree_sha256 = Sha256Digest::parse(&"a".repeat(64)).unwrap();

        let expected = "SDK manifest tree digest differs from the selected tree digest";
        assert_public_rejects_with(&wrong_tree, expected);
        assert_internal_rejects_with(&wrong_tree, expected);
    }

    #[test]
    fn public_and_selected_member_verifiers_reject_symlinks_for_each_selected_member() {
        for member in ["archive", "manifest", "checksum"] {
            let fixture = fixture();
            let asset = fixture.request.identity.asset_name().unwrap();
            let path = match member {
                "archive" => fixture.package_dir.join(&asset),
                "manifest" => fixture.package_dir.join(format!("{asset}.manifest.json")),
                "checksum" => fixture.package_dir.join(format!("{asset}.sha256")),
                _ => unreachable!(),
            };
            let saved = fixture.temporary.path().join(format!("{member}-original"));
            fs::rename(&path, &saved).unwrap();
            symlink(&saved, &path).unwrap();

            assert_public_rejects_with(&fixture.request, "cannot safely open SDK package member");
            assert_internal_rejects_with(&fixture.request, "cannot safely open SDK package member");
        }
    }

    #[test]
    fn selected_member_verifier_rejects_wrong_target_abi_and_triple() {
        let fixture = fixture();
        let mut wrong_abi = fixture.request.clone();
        wrong_abi
            .identity
            .target
            .abi
            .insert("data_model".into(), SdkAbiValue::Text("lp64".into()));
        assert_internal_rejects_with(
            &wrong_abi,
            "SDK manifest identity differs from the selected identity",
        );

        let mut wrong_target_triple = fixture.request;
        wrong_target_triple.identity.target.target_triple = "riscv32-aros".into();
        assert_internal_rejects_with(
            &wrong_target_triple,
            "SDK manifest identity differs from the selected identity",
        );
    }

    #[test]
    fn selected_member_verifier_rejects_corrupt_sidecar_and_malformed_manifest() {
        let sidecar_fixture = fixture();
        let asset = sidecar_fixture.request.identity.asset_name().unwrap();
        fs::write(
            sidecar_fixture.package_dir.join(format!("{asset}.sha256")),
            b"corrupt sidecar\n",
        )
        .unwrap();
        assert_internal_rejects_with(&sidecar_fixture.request, "checksum sidecar does not match");

        let manifest_fixture = fixture();
        let asset = manifest_fixture.request.identity.asset_name().unwrap();
        fs::write(
            manifest_fixture
                .package_dir
                .join(format!("{asset}.manifest.json")),
            b"not-json\n",
        )
        .unwrap();
        assert_internal_rejects_with(
            &manifest_fixture.request,
            "external SDK manifest is invalid",
        );
    }

    #[test]
    fn wrong_product_version_selects_a_missing_asset_name() {
        let fixture = fixture();
        let mut wrong_version = fixture.request;
        wrong_version.identity.version = "1.2.4".into();
        assert_internal_rejects_with(&wrong_version, "cannot safely open SDK package member");
    }
}
