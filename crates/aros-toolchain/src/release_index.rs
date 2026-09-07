//! Closed v1 toolchain-release inventory and byte-comparison checks.
//!
//! This module has no network, credentials, tag, or release-promotion
//! authority. It accepts only a complete local regular-file inventory and
//! makes the pre-attestation/final checksum boundary explicit.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aros_common::{
    open_regular_file_nofollow, parse_credential_free_https_url, sha256_bytes, sha256_reader,
    ArosToolchainManifest, Sha256Digest,
};
use serde::Serialize;
use serde_json::Value;

use crate::package::{canonical_asset_name, pretty_json, spdx_bytes};
use crate::profiles::{Profile, Profiles};
use crate::recipe::Recipe;
use crate::source_lock::SourceLock;
use crate::ContractError;

const INDEX_NAME: &str = "toolchain-index-v1.json";
const CHECKSUMS_NAME: &str = "SHA256SUMS";
const PROVENANCE_NAME: &str = "toolchain-provenance.sigstore.json";
const RECIPE_NAME: &str = "toolchain-recipe-v2.json";
const PROFILES_NAME: &str = "profiles-v1.json";
const MANIFEST_SCHEMA_NAME: &str = "toolchain-manifest-v1.schema.json";
const TREE_FIXTURE_NAME: &str = "tree-digest-v1.fixture.json";
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

const V1_HOSTS: &[&str] = &[
    "linux-aarch64",
    "linux-x86_64",
    "macos-aarch64",
    "macos-x86_64",
];
const V1_PROFILES: &[&str] = &["arm-raspi", "pc-x86_64", "rpi-aarch64"];
const REQUIRED_TOOLS: &[&str] = &[
    "clang",
    "clang++",
    "ld.lld",
    "llvm-ar",
    "llvm-ranlib",
    "llvm-nm",
    "llvm-strip",
    "llvm-objcopy",
    "llvm-objdump",
    "aros-collect",
    "collect-aros",
];
const REQUIRED_CXX_HEADERS: &[&str] = &[
    "algorithm",
    "cerrno",
    "cinttypes",
    "cstddef",
    "cstdint",
    "deque",
    "memory",
    "string",
    "system_error",
    "vector",
];

/// The state of the local release inventory at index time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStage {
    /// Generate an index and checksum set before the external provenance bundle.
    PreAttestation,
    /// Verify the pre-attestation checksum set, then bind the provenance bundle.
    Final,
}

/// Explicit local inputs for one closed v1 release index operation.
#[derive(Debug, Clone)]
pub struct IndexRequest {
    /// Absolute local directory containing release candidates and support files.
    pub directory: PathBuf,
    /// Immutable release identifier expected in every selected manifest.
    pub release_id: String,
    /// Credential-free HTTPS release download root written into the index.
    pub base_url: String,
    /// Exact basename of the one published source-lock document.
    pub source_lock_filename: String,
    /// Explicit pre-attestation or final inventory boundary.
    pub stage: IndexStage,
}

/// Deterministic v1 release index written beside the release assets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReleaseIndex {
    /// Index schema version.
    pub schema: u32,
    /// Immutable release name.
    pub release_id: String,
    /// Credential-free archive download base URL.
    pub base_url: String,
    /// AROS source revision shared by all package manifests.
    pub source_commit: String,
    /// Producer revision shared by all package manifests.
    pub producer_commit: String,
    /// aros-tools revision shared by all package manifests.
    pub tools_commit: String,
    /// The closed four-host, three-profile package matrix.
    pub artifacts: Vec<NativeReleaseArtifact>,
}

/// One v1 archive reference in [`NativeReleaseIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReleaseArtifact {
    /// Archive filename.
    pub asset: String,
    /// Archive SHA-256 measured from the local package.
    pub sha256: String,
    /// Archive byte length measured from the local package.
    pub size: u64,
    /// Build host selector.
    pub host: String,
    /// Producer profile selector.
    pub target_profile: String,
    /// AROS target triple.
    pub target_triple: String,
    /// Canonical payload tree SHA-256.
    pub tree_sha256: String,
    /// LLVM version selected by the verified source lock.
    pub llvm_version: String,
    /// Consumer availability marker.
    pub enabled: bool,
    /// Archive's `toolchain/` root depth.
    pub strip_components: usize,
    /// Required runtime, compiler, and collector paths for this profile.
    pub required_paths: Vec<String>,
}

/// Measured result of an index operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOutput {
    /// Generated, closed index document.
    pub index: NativeReleaseIndex,
    /// Index path in the validated release directory.
    pub index_path: PathBuf,
    /// Final or pre-attestation checksum path in the validated release directory.
    pub checksums_path: PathBuf,
    /// SHA-256 of the exact checksum document written by this operation.
    pub checksums_sha256: Sha256Digest,
}

/// Compare two complete package sets byte-for-byte without copying either set.
///
/// # Errors
///
/// Returns AX0702 unless both directories contain the same one closed package
/// set and every corresponding member is byte-identical.
pub fn compare_package_sets(left: &Path, right: &Path) -> Result<(), ContractError> {
    let left_members = package_set_members(left)?;
    let right_members = package_set_members(right)?;
    if left_members != right_members {
        return Err(ContractError::comparison(
            "package-set member names differ between independent outputs",
        ));
    }
    for name in left_members {
        let left_path = left.join(&name);
        let right_path = right.join(&name);
        if !files_equal(&left_path, &right_path)? {
            return Err(ContractError::comparison(
                "package-set members differ between independent outputs",
            ));
        }
    }
    Ok(())
}

/// Validate and advance one complete v1 local release inventory.
///
/// At the pre-attestation stage, the directory must contain 48 package assets
/// plus five support files. This function then writes the index and a checksum
/// file covering those 54 attestation subjects. At the final stage it requires
/// the exact existing pre-attestation set plus one provenance bundle, verifies
/// the old checksum document, and atomically replaces it with a checksum set
/// covering every other final asset.
///
/// # Errors
///
/// Returns AX0701 for incomplete, mixed, malformed, non-regular, unexpected,
/// or otherwise unverifiable local release material.
pub fn index_complete_v1(request: &IndexRequest) -> Result<IndexOutput, ContractError> {
    validate_request(request)?;
    let directory = &request.directory;
    let recipe = parse_recipe(directory)?;
    let source_lock = parse_source_lock(directory, request, &recipe)?;
    let profiles = parse_profiles(directory, &recipe)?;
    let core_support = core_support_names(request);
    let package_names = expected_package_names(&source_lock, &profiles)?;
    let pre_subjects = names_with_packages(&core_support, &package_names);
    let mut required_input = pre_subjects.clone();
    if request.stage == IndexStage::Final {
        required_input.insert(INDEX_NAME.into());
        required_input.insert(PROVENANCE_NAME.into());
        required_input.insert(CHECKSUMS_NAME.into());
    }
    require_exact_regular_members(directory, &required_input)?;
    validate_static_support(directory)?;

    let mut artifacts = Vec::with_capacity(package_names.len());
    for asset in package_names {
        artifacts.push(validate_package_asset(
            directory,
            &asset,
            request,
            &recipe,
            &source_lock,
            &profiles,
        )?);
    }
    artifacts.sort_by(|left, right| left.asset.cmp(&right.asset));
    let index = NativeReleaseIndex {
        schema: 1,
        release_id: request.release_id.clone(),
        base_url: request.base_url.trim_end_matches('/').into(),
        source_commit: recipe.source().0.as_str().into(),
        producer_commit: recipe.producer().0.as_str().into(),
        tools_commit: recipe.tools().0.as_str().into(),
        artifacts,
    };
    let index_bytes = pretty_json(&index)
        .map_err(|_| ContractError::index("cannot serialize the deterministic release index"))?;
    let index_path = directory.join(INDEX_NAME);
    let checksums_path = directory.join(CHECKSUMS_NAME);

    match request.stage {
        IndexStage::PreAttestation => {
            publish_new(&index_path, &index_bytes)?;
            let mut pre_checksum_members = pre_subjects;
            pre_checksum_members.insert(INDEX_NAME.into());
            let checksums = checksums_bytes(directory, &pre_checksum_members)?;
            publish_new(&checksums_path, &checksums)?;
            Ok(IndexOutput {
                index,
                index_path,
                checksums_path,
                checksums_sha256: sha256_bytes(&checksums),
            })
        }
        IndexStage::Final => {
            if read_metadata(&index_path, "release index")? != index_bytes {
                return Err(ContractError::index(
                    "pre-attestation release index differs from the closed package inventory",
                ));
            }
            let mut pre_checksum_members = pre_subjects;
            pre_checksum_members.insert(INDEX_NAME.into());
            let expected_pre_checksums = checksums_bytes(directory, &pre_checksum_members)?;
            let existing = read_metadata(&checksums_path, "pre-attestation checksums")?;
            if existing != expected_pre_checksums {
                return Err(ContractError::index(
                    "pre-attestation checksum document does not cover the exact attested subjects",
                ));
            }
            let provenance = read_metadata(&directory.join(PROVENANCE_NAME), "provenance bundle")?;
            require_json_object(&provenance, "provenance bundle")?;
            let final_members = required_input
                .into_iter()
                .filter(|name| name != CHECKSUMS_NAME)
                .collect::<BTreeSet<_>>();
            let final_checksums = checksums_bytes(directory, &final_members)?;
            replace_existing(&checksums_path, &existing, &final_checksums)?;
            Ok(IndexOutput {
                index,
                index_path,
                checksums_path,
                checksums_sha256: sha256_bytes(&final_checksums),
            })
        }
    }
}

fn validate_request(request: &IndexRequest) -> Result<(), ContractError> {
    if !request.directory.is_absolute() || !safe_segment(&request.release_id) {
        return Err(ContractError::index(
            "release index requires an absolute directory and a safe release identifier",
        ));
    }
    if !safe_source_lock_name(&request.source_lock_filename) {
        return Err(ContractError::index(
            "release index source-lock filename is not a safe v1 support filename",
        ));
    }
    parse_credential_free_https_url(request.base_url.trim_end_matches('/')).map_err(|_| {
        ContractError::index("release index base URL must be credential-free HTTPS")
    })?;
    let metadata = fs::symlink_metadata(&request.directory)
        .map_err(|_| ContractError::index("release index directory is inaccessible"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::index(
            "release index directory must be a real directory",
        ));
    }
    Ok(())
}

fn parse_recipe(directory: &Path) -> Result<Recipe, ContractError> {
    let bytes = read_metadata(&directory.join(RECIPE_NAME), "release recipe")?;
    Recipe::parse(&bytes).map_err(|_| ContractError::index("release recipe is invalid"))
}

fn parse_source_lock(
    directory: &Path,
    request: &IndexRequest,
    recipe: &Recipe,
) -> Result<SourceLock, ContractError> {
    let path = directory.join(&request.source_lock_filename);
    let (digest, _) = measure_asset(&path, MAX_METADATA_BYTES)?;
    if digest.as_str() != recipe.source_lock_sha256().as_str() {
        return Err(ContractError::index(
            "published source lock digest does not match the release recipe",
        ));
    }
    let bytes = read_metadata(&path, "published source lock")?;
    let source_lock = SourceLock::parse(&bytes)
        .map_err(|_| ContractError::index("published source lock is invalid"))?;
    source_lock.verify_recipe_patches(recipe).map_err(|_| {
        ContractError::index("published source lock does not bind the release recipe patches")
    })?;
    Ok(source_lock)
}

fn parse_profiles(directory: &Path, recipe: &Recipe) -> Result<Profiles, ContractError> {
    let path = directory.join(PROFILES_NAME);
    let (digest, _) = measure_asset(&path, MAX_METADATA_BYTES)?;
    if digest.as_str() != recipe.profiles_sha256().as_str() {
        return Err(ContractError::index(
            "published profiles digest does not match the release recipe",
        ));
    }
    let bytes = read_metadata(&path, "published profiles")?;
    let profiles = Profiles::parse(&bytes)
        .map_err(|_| ContractError::index("published profiles are invalid"))?;
    if profiles.upstream_commit().as_str() != recipe.source().0.as_str() {
        return Err(ContractError::index(
            "published profiles upstream revision does not match the release source",
        ));
    }
    Ok(profiles)
}

fn core_support_names(request: &IndexRequest) -> BTreeSet<String> {
    [
        RECIPE_NAME.into(),
        request.source_lock_filename.clone(),
        PROFILES_NAME.into(),
        MANIFEST_SCHEMA_NAME.into(),
        TREE_FIXTURE_NAME.into(),
    ]
    .into_iter()
    .collect()
}

fn expected_package_names(
    source_lock: &SourceLock,
    profiles: &Profiles,
) -> Result<Vec<String>, ContractError> {
    let mut names = Vec::with_capacity(V1_HOSTS.len() * V1_PROFILES.len());
    for host in V1_HOSTS {
        for profile_name in V1_PROFILES {
            profiles.select(profile_name).map_err(|_| {
                ContractError::index("published profiles do not provide the complete v1 matrix")
            })?;
            names.push(
                canonical_asset_name(source_lock.version(), host, profile_name).map_err(|_| {
                    ContractError::index("published source lock cannot select a v1 package name")
                })?,
            );
        }
    }
    Ok(names)
}

fn names_with_packages(core: &BTreeSet<String>, packages: &[String]) -> BTreeSet<String> {
    let mut names = core.clone();
    for asset in packages {
        names.insert(asset.clone());
        names.insert(format!("{asset}.manifest.json"));
        names.insert(format!("{asset}.sha256"));
        names.insert(format!("{asset}.spdx.json"));
    }
    names
}

fn require_exact_regular_members(
    directory: &Path,
    expected: &BTreeSet<String>,
) -> Result<(), ContractError> {
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| ContractError::index("cannot enumerate release inventory"))?
    {
        let entry =
            entry.map_err(|_| ContractError::index("cannot read release inventory member"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ContractError::index("release inventory member is not UTF-8"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| ContractError::index("cannot inspect release inventory member"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ContractError::index(
                "complete v1 release inventory contains a non-regular member",
            ));
        }
        actual.insert(name);
    }
    if &actual != expected {
        return Err(ContractError::index(
            "complete v1 release inventory does not contain the exact required members",
        ));
    }
    Ok(())
}

fn validate_static_support(directory: &Path) -> Result<(), ContractError> {
    for (name, kind) in [
        (MANIFEST_SCHEMA_NAME, "toolchain manifest schema"),
        (TREE_FIXTURE_NAME, "tree digest fixture"),
    ] {
        require_json_object(&read_metadata(&directory.join(name), kind)?, kind)?;
    }
    Ok(())
}

fn validate_package_asset(
    directory: &Path,
    asset: &str,
    request: &IndexRequest,
    recipe: &Recipe,
    source_lock: &SourceLock,
    profiles: &Profiles,
) -> Result<NativeReleaseArtifact, ContractError> {
    let archive = directory.join(asset);
    let (archive_sha256, size) = measure_asset(&archive, MAX_ARCHIVE_BYTES)?;
    let manifest_path = directory.join(format!("{asset}.manifest.json"));
    let manifest_bytes = read_metadata(&manifest_path, "external package manifest")?;
    let manifest: ArosToolchainManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| ContractError::index("external package manifest is not valid JSON"))?;
    manifest
        .validate()
        .map_err(|_| ContractError::index("external package manifest violates the v1 contract"))?;
    let expected_asset = canonical_asset_name(
        source_lock.version(),
        &manifest.host,
        &manifest.target_profile,
    )
    .map_err(|_| ContractError::index("package manifest has an unsupported v1 selector"))?;
    if expected_asset != asset {
        return Err(ContractError::index(
            "package archive name is not canonical for its manifest selector",
        ));
    }
    let profile = profiles.select(&manifest.target_profile).map_err(|_| {
        ContractError::index("package manifest profile is absent from the published profiles")
    })?;
    validate_manifest_identity(&manifest, request, recipe, source_lock, profile)?;
    let checksum = read_metadata(
        &directory.join(format!("{asset}.sha256")),
        "archive checksum",
    )?;
    if checksum != format!("{archive_sha256}  {asset}\n").as_bytes() {
        return Err(ContractError::index(
            "package checksum sidecar does not match its measured archive",
        ));
    }
    let expected_sbom = spdx_bytes(source_lock, &manifest)?;
    if read_metadata(
        &directory.join(format!("{asset}.spdx.json")),
        "package SPDX SBOM",
    )? != expected_sbom
    {
        return Err(ContractError::index(
            "package SPDX SBOM does not match the closed source and manifest inputs",
        ));
    }
    Ok(NativeReleaseArtifact {
        asset: asset.into(),
        sha256: archive_sha256.to_string(),
        size,
        host: manifest.host,
        target_profile: manifest.target_profile,
        target_triple: manifest.target_triple,
        tree_sha256: manifest.tree_sha256,
        llvm_version: manifest.llvm_version.unwrap_or_default(),
        enabled: true,
        strip_components: 1,
        required_paths: required_paths(source_lock.version(), profile)?,
    })
}

fn validate_manifest_identity(
    manifest: &ArosToolchainManifest,
    request: &IndexRequest,
    recipe: &Recipe,
    source_lock: &SourceLock,
    profile: &Profile,
) -> Result<(), ContractError> {
    let valid = manifest.release_id == request.release_id
        && manifest.llvm_version.as_deref() == Some(source_lock.version())
        && manifest.target_triple == profile.target_triple()
        && manifest.capabilities == profile.capabilities()
        && manifest.recipe_sha256 == recipe.sha256().as_str()
        && manifest.source_lock_sha256 == recipe.source_lock_sha256().as_str()
        && manifest.profiles_sha256 == recipe.profiles_sha256().as_str()
        && manifest.source_commit == recipe.source().0.as_str()
        && manifest.producer_commit == recipe.producer().0.as_str()
        && manifest.tools_commit == recipe.tools().0.as_str()
        && manifest.source_date_epoch == recipe.source_date_epoch();
    if !valid {
        return Err(ContractError::index(
            "package manifest identity is not bound to the closed release inputs",
        ));
    }
    Ok(())
}

fn required_paths(llvm_version: &str, profile: &Profile) -> Result<Vec<String>, ContractError> {
    let mut paths = REQUIRED_TOOLS
        .iter()
        .map(|tool| format!("bin/{tool}"))
        .collect::<Vec<_>>();
    if profile.name() == "pc-x86_64" {
        paths.push("bin/collect-aros32".into());
    }
    paths.extend(
        REQUIRED_CXX_HEADERS
            .iter()
            .map(|header| format!("include/c++/v1/{header}")),
    );
    paths.extend([
        "lib/libc++.a".into(),
        "lib/libc++abi.a".into(),
        "lib/libunwind.a".into(),
        "toolchain-manifest.json".into(),
    ]);
    let builtins: &[&str] = match profile.name() {
        "pc-x86_64" => &["x86_64", "i386"],
        "arm-raspi" => &["armhf"],
        "rpi-aarch64" => &["aarch64"],
        _ => {
            return Err(ContractError::index(
                "published profiles do not provide a known v1 builtins contract",
            ));
        }
    };
    paths.extend(builtins.iter().map(|architecture| {
        format!("lib/clang/{llvm_version}/lib/aros/libclang_rt.builtins-{architecture}.a")
    }));
    Ok(paths)
}

fn measure_asset(path: &Path, limit: u64) -> Result<(Sha256Digest, u64), ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::index("cannot safely open release asset"))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::index("cannot inspect release asset"))?;
    if metadata.len() > limit {
        return Err(ContractError::index(
            "release asset exceeds its configured resource limit",
        ));
    }
    let measured = sha256_reader(&mut Read::by_ref(&mut file).take(limit + 1))
        .map_err(|_| ContractError::index("cannot measure release asset"))?;
    if measured.size != metadata.len() {
        return Err(ContractError::index(
            "release asset changed while it was measured",
        ));
    }
    Ok((measured.digest, measured.size))
}

fn read_metadata(path: &Path, kind: &str) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::index(format!("cannot safely open {kind}")))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::index(format!("cannot inspect {kind}")))?;
    if metadata.len() > MAX_METADATA_BYTES {
        return Err(ContractError::index(format!(
            "{kind} exceeds the configured metadata resource limit"
        )));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| ContractError::index(format!("{kind} exceeds addressable memory")))?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(&mut file)
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::index(format!("cannot read {kind}")))?;
    let Ok(size) = u64::try_from(bytes.len()) else {
        return Err(ContractError::index(format!(
            "{kind} changed or exceeded its resource limit while it was read"
        )));
    };
    if size > MAX_METADATA_BYTES || size != metadata.len() {
        return Err(ContractError::index(format!(
            "{kind} changed or exceeded its resource limit while it was read"
        )));
    }
    Ok(bytes)
}

fn require_json_object(bytes: &[u8], kind: &str) -> Result<(), ContractError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| ContractError::index(format!("{kind} is not valid JSON")))?;
    if !value.is_object() {
        return Err(ContractError::index(format!(
            "{kind} must be a JSON object"
        )));
    }
    Ok(())
}

fn checksums_bytes(directory: &Path, names: &BTreeSet<String>) -> Result<Vec<u8>, ContractError> {
    let mut lines = Vec::with_capacity(names.len());
    for name in names {
        let (digest, _) = measure_asset(&directory.join(name), MAX_ARCHIVE_BYTES)?;
        lines.push(format!("{digest}  {name}"));
    }
    lines.sort_unstable();
    Ok(format!("{}\n", lines.join("\n")).into_bytes())
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let staged = stage_bytes(path, bytes)?;
    fs::hard_link(staged.path(), path).map_err(|_| {
        ContractError::index("cannot atomically create absent release inventory output")
    })?;
    sync_parent(path)?;
    staged
        .close()
        .map_err(|_| ContractError::index("cannot remove release inventory staging file"))?;
    sync_parent(path)
}

fn replace_existing(path: &Path, expected: &[u8], desired: &[u8]) -> Result<(), ContractError> {
    if read_metadata(path, "release checksums")? != expected {
        return Err(ContractError::index(
            "release checksums changed before finalization",
        ));
    }
    let staged = stage_bytes(path, desired)?;
    if read_metadata(path, "release checksums")? != expected {
        return Err(ContractError::index(
            "release checksums changed before atomic finalization",
        ));
    }
    staged
        .persist(path)
        .map_err(|_| ContractError::index("cannot atomically replace final release checksums"))?;
    sync_parent(path)
}

fn stage_bytes(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, ContractError> {
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::index("release output path has no parent"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".aros-toolchain-index-stage-")
        .tempfile_in(parent)
        .map_err(|_| ContractError::index("cannot reserve release inventory staging file"))?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|_| ContractError::index("cannot synchronize release inventory staging file"))?;
    Ok(staged)
}

fn sync_parent(path: &Path) -> Result<(), ContractError> {
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::index("release output path has no parent"))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ContractError::index("cannot synchronize release inventory directory"))?;
    Ok(())
}

fn package_set_members(directory: &Path) -> Result<BTreeSet<String>, ContractError> {
    if !directory.is_absolute() {
        return Err(ContractError::comparison(
            "package comparison directories must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| ContractError::comparison("package comparison directory is inaccessible"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::comparison(
            "package comparison directory must be a real directory",
        ));
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| ContractError::comparison("cannot enumerate package comparison directory"))?
    {
        let entry = entry
            .map_err(|_| ContractError::comparison("cannot read package comparison member"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ContractError::comparison("package comparison member is not UTF-8"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| ContractError::comparison("cannot inspect package comparison member"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ContractError::comparison(
                "package comparison set contains a non-regular member",
            ));
        }
        names.insert(name);
    }
    let archives = names
        .iter()
        .filter(|name| name.ends_with(".tar.xz"))
        .collect::<Vec<_>>();
    if archives.len() != 1 {
        return Err(ContractError::comparison(
            "package comparison set must contain exactly one archive",
        ));
    }
    let archive = archives[0];
    let expected = [
        archive.clone(),
        format!("{archive}.manifest.json"),
        format!("{archive}.sha256"),
        format!("{archive}.spdx.json"),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if names != expected {
        return Err(ContractError::comparison(
            "package comparison set does not contain the exact four required members",
        ));
    }
    Ok(names)
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, ContractError> {
    let mut left = open_regular_file_nofollow(left)
        .map_err(|_| ContractError::comparison("cannot safely open left package member"))?;
    let mut right = open_regular_file_nofollow(right)
        .map_err(|_| ContractError::comparison("cannot safely open right package member"))?;
    let left_size = left
        .metadata()
        .map_err(|_| ContractError::comparison("cannot inspect left package member"))?
        .len();
    let right_size = right
        .metadata()
        .map_err(|_| ContractError::comparison("cannot inspect right package member"))?
        .len();
    if left_size > MAX_ARCHIVE_BYTES || right_size > MAX_ARCHIVE_BYTES {
        return Err(ContractError::comparison(
            "package comparison member exceeds the configured resource limit",
        ));
    }
    if left_size != right_size {
        return Ok(false);
    }
    let mut left_buffer = vec![0_u8; 128 * 1024].into_boxed_slice();
    let mut right_buffer = vec![0_u8; 128 * 1024].into_boxed_slice();
    let mut read_total = 0_u64;
    loop {
        let left_read = left
            .read(&mut left_buffer)
            .map_err(|_| ContractError::comparison("cannot read left package member"))?;
        let right_read = right
            .read(&mut right_buffer)
            .map_err(|_| ContractError::comparison("cannot read right package member"))?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..left_read] {
            return Ok(false);
        }
        read_total = read_total
            .checked_add(u64::try_from(left_read).map_err(|_| {
                ContractError::comparison("package comparison byte count overflowed")
            })?)
            .ok_or_else(|| ContractError::comparison("package comparison byte count overflowed"))?;
        if read_total > left_size {
            return Err(ContractError::comparison(
                "package comparison member changed while it was read",
            ));
        }
        if left_read == 0 {
            return Ok(read_total == left_size);
        }
    }
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn safe_source_lock_name(value: &str) -> bool {
    safe_segment(value) && value.ends_with(".sources.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::{sha256_bytes, ArosToolchainManifestEntry};
    use serde_json::{json, Map};

    fn fixture_profiles() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1",
            "upstream_commit": "1".repeat(40),
            "profiles": [
                {"name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "", "capabilities": ["c"]},
                {"name": "arm-raspi", "configure_target": "arm-raspi", "upstream_output_target": "arm-raspi", "target_triple": "arm-unknown-aros", "cpu": "arm", "platform": "raspi", "float_abi": "soft", "capabilities": ["c"]},
                {"name": "rpi-aarch64", "configure_target": "rpi-aarch64", "upstream_output_target": "rpi-aarch64", "target_triple": "aarch64-unknown-aros", "cpu": "aarch64", "platform": "raspi", "float_abi": "", "capabilities": ["c"]}
            ]
        }))
        .unwrap()
    }

    fn fixture_source_lock() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "c".repeat(64), "size": 1
            }],
            "host_python_packages": [{
                "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
                "url": "https://example.invalid/mako.tar.gz", "sha256": "d".repeat(64), "size": 1,
                "source_root": "mako", "python_path": "."
            }]
        }))
        .unwrap()
    }

    fn fixture_recipe(source_lock: &[u8], profiles: &[u8]) -> Vec<u8> {
        let mut value = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
            "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
            "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
            "source_date_epoch": 946_684_800_u64,
            "source_lock_sha256": sha256_bytes(source_lock).to_string(),
            "profiles_sha256": sha256_bytes(profiles).to_string(),
            "patches": []
        });
        let digest = sha256_bytes(&crate::canonical::bytes(&value).unwrap());
        value["recipe_sha256"] = json!(digest.to_string());
        serde_json::to_vec(&value).unwrap()
    }

    fn write_complete_pre_attestation_fixture(root: &Path) -> IndexRequest {
        let source_lock_bytes = fixture_source_lock();
        let profiles_bytes = fixture_profiles();
        let recipe_bytes = fixture_recipe(&source_lock_bytes, &profiles_bytes);
        fs::write(root.join(RECIPE_NAME), &recipe_bytes).unwrap();
        fs::write(root.join("llvm-11.sources.json"), &source_lock_bytes).unwrap();
        fs::write(root.join(PROFILES_NAME), &profiles_bytes).unwrap();
        fs::write(root.join(MANIFEST_SCHEMA_NAME), b"{}").unwrap();
        fs::write(root.join(TREE_FIXTURE_NAME), b"{}").unwrap();
        let source_lock = SourceLock::parse(&source_lock_bytes).unwrap();
        let profiles = Profiles::parse(&profiles_bytes).unwrap();
        let recipe = Recipe::parse(&recipe_bytes).unwrap();
        for host in V1_HOSTS {
            for profile_name in V1_PROFILES {
                let profile = profiles.select(profile_name).unwrap();
                let asset =
                    canonical_asset_name(source_lock.version(), host, profile_name).unwrap();
                let payload = format!("fixture archive {host}/{profile_name}\n");
                fs::write(root.join(&asset), &payload).unwrap();
                let manifest = ArosToolchainManifest {
                    schema: 1,
                    release_id: "fixture-release".into(),
                    host: (*host).into(),
                    target_profile: (*profile_name).into(),
                    target_triple: profile.target_triple().into(),
                    tree_sha256: "a".repeat(64),
                    llvm_version: Some(source_lock.version().into()),
                    recipe_sha256: recipe.sha256().to_string(),
                    source_lock_sha256: recipe.source_lock_sha256().to_string(),
                    profiles_sha256: recipe.profiles_sha256().to_string(),
                    source_commit: recipe.source().0.as_str().into(),
                    producer_commit: recipe.producer().0.as_str().into(),
                    tools_commit: recipe.tools().0.as_str().into(),
                    source_date_epoch: recipe.source_date_epoch(),
                    capabilities: profile.capabilities().to_vec(),
                    build_environment: Map::new(),
                    files: vec![ArosToolchainManifestEntry {
                        path: "bin/clang".into(),
                        mode: "0755".into(),
                        kind: "file".into(),
                        sha256: Some("b".repeat(64)),
                        size: Some(1),
                        target: None,
                    }],
                };
                let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
                fs::write(root.join(format!("{asset}.manifest.json")), &manifest_bytes).unwrap();
                fs::write(
                    root.join(format!("{asset}.sha256")),
                    format!("{}  {asset}\n", sha256_bytes(payload.as_bytes())),
                )
                .unwrap();
                fs::write(
                    root.join(format!("{asset}.spdx.json")),
                    spdx_bytes(&source_lock, &manifest).unwrap(),
                )
                .unwrap();
            }
        }
        IndexRequest {
            directory: root.to_path_buf(),
            release_id: "fixture-release".into(),
            base_url: "https://example.invalid/toolchains/fixture-release".into(),
            source_lock_filename: "llvm-11.sources.json".into(),
            stage: IndexStage::PreAttestation,
        }
    }

    #[test]
    fn required_paths_preserve_the_closed_v1_multilib_contract() {
        let profiles = Profiles::parse(
            br#"{"schema":"aros-toolchain-profiles-v1","upstream_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","profiles":[{"name":"pc-x86_64","configure_target":"pc-x86_64","upstream_output_target":"pc-x86_64","target_triple":"x86_64-unknown-aros","cpu":"x86_64","platform":"pc","float_abi":"","capabilities":["c"]}]}"#,
        )
        .unwrap();
        let paths = required_paths("11.0.0", profiles.select("pc-x86_64").unwrap()).unwrap();
        assert!(paths.contains(&"bin/collect-aros32".into()));
        assert!(paths.contains(&"lib/clang/11.0.0/lib/aros/libclang_rt.builtins-i386.a".into()));
        assert!(paths.contains(&"lib/clang/11.0.0/lib/aros/libclang_rt.builtins-x86_64.a".into()));
    }

    #[test]
    fn comparison_requires_the_exact_closed_four_member_set() {
        let temporary = tempfile::tempdir().unwrap();
        let left = temporary.path().join("left");
        let right = temporary.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        for directory in [&left, &right] {
            fs::write(directory.join("archive.tar.xz"), b"archive").unwrap();
            fs::write(directory.join("archive.tar.xz.manifest.json"), b"manifest").unwrap();
            fs::write(directory.join("archive.tar.xz.sha256"), b"checksum").unwrap();
            fs::write(directory.join("archive.tar.xz.spdx.json"), b"spdx").unwrap();
        }
        assert!(compare_package_sets(&left, &right).is_ok());
        fs::write(right.join("archive.tar.xz.spdx.json"), b"changed").unwrap();
        assert!(compare_package_sets(&left, &right).is_err());
        fs::write(right.join("extra"), b"unexpected").unwrap();
        assert!(compare_package_sets(&left, &right).is_err());
    }

    #[test]
    fn complete_matrix_advances_only_from_pre_attestation_to_final_inventory() {
        let temporary = tempfile::tempdir().unwrap();
        let mut request = write_complete_pre_attestation_fixture(temporary.path());
        let pre = index_complete_v1(&request).unwrap();
        assert_eq!(pre.index.artifacts.len(), 12);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 55);
        assert_eq!(
            String::from_utf8(fs::read(&pre.checksums_path).unwrap())
                .unwrap()
                .lines()
                .count(),
            54
        );

        fs::write(temporary.path().join(PROVENANCE_NAME), b"{}").unwrap();
        request.stage = IndexStage::Final;
        let final_output = index_complete_v1(&request).unwrap();
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 56);
        assert_eq!(
            String::from_utf8(fs::read(&final_output.checksums_path).unwrap())
                .unwrap()
                .lines()
                .count(),
            55
        );
        let index: Value =
            serde_json::from_slice(&fs::read(final_output.index_path).unwrap()).unwrap();
        assert_eq!(index["release_id"], "fixture-release");
        assert_eq!(index["artifacts"].as_array().unwrap().len(), 12);
    }

    #[test]
    fn complete_matrix_rejects_mixed_manifest_identity_and_missing_sbom() {
        let temporary = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(temporary.path());
        let asset = canonical_asset_name("11.0.0", "linux-x86_64", "pc-x86_64").unwrap();
        let manifest_path = temporary.path().join(format!("{asset}.manifest.json"));
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["release_id"] = json!("other-release");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(index_complete_v1(&request).is_err());

        let temporary = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(temporary.path());
        fs::remove_file(temporary.path().join(format!("{asset}.spdx.json"))).unwrap();
        assert!(index_complete_v1(&request).is_err());
    }
}
