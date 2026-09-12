//! Closed v1 toolchain-release inventory and byte-comparison checks.
//!
//! This module has no network, credentials, tag, or release-promotion
//! authority. It accepts only a complete local regular-file inventory and
//! makes the pre-attestation/final checksum boundary explicit.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use aros_common::{
    finish_sha256, open_regular_file_nofollow, parse_credential_free_https_url,
    publish_atomic_file, sha256_bytes, sha256_reader, ArosToolchainManifest, AtomicFilePolicy,
    Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::package::{canonical_asset_name, pretty_json};
use crate::package_verify::{verify_members, PackageAssetPaths, PackageVerificationRequest};
use crate::profiles::{Profile, Profiles};
use crate::recipe::Recipe;
use crate::source_lock::SourceLock;
use crate::{canonical, ContractError};

const INDEX_NAME: &str = "toolchain-index-v1.json";
const CHECKSUMS_NAME: &str = "SHA256SUMS";
const PROVENANCE_NAME: &str = "toolchain-provenance.sigstore.json";
const RECIPE_NAME: &str = "toolchain-recipe-v2.json";
const PROFILES_NAME: &str = "profiles-v1.json";
const MANIFEST_SCHEMA_NAME: &str = "toolchain-manifest-v1.schema.json";
const TREE_FIXTURE_NAME: &str = "tree-digest-v1.fixture.json";
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

const MANIFEST_SCHEMA_BYTES: &[u8] =
    include_bytes!("../../aros-common/tests/fixtures/toolchain-manifest-v1.schema.json");
const TREE_FIXTURE_BYTES: &[u8] =
    include_bytes!("../../aros-common/tests/fixtures/tree-digest-v1.fixture.json");

/// All host selectors accepted by the v1 archive format.
///
/// This set includes the historical Intel macOS releases. It must remain
/// stable so existing published v1 indexes remain readable.
pub const V1_HOSTS: &[&str] = &[
    "linux-aarch64",
    "linux-x86_64",
    "macos-aarch64",
    "macos-x86_64",
];
/// Hosts actively qualified for newly produced v1 releases.
///
/// Intel macOS qualification is intentionally suspended until
/// metaneutrons/aros-toolchains#27 is completed. The release producer emits
/// exactly this matrix, while the parser continues to accept the historical
/// four-host matrix above.
pub const ACTIVE_V1_HOSTS: &[&str] = &["linux-aarch64", "linux-x86_64", "macos-aarch64"];
/// Closed target-profile selectors of the v1 release matrix.
pub const V1_PROFILES: &[&str] = &["arm-raspi", "pc-x86_64", "rpi-aarch64"];
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// The closed active or historical v1 host/profile package matrix.
    pub artifacts: Vec<NativeReleaseArtifact>,
}

/// One v1 archive reference in [`NativeReleaseIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl NativeReleaseIndex {
    /// Parse and validate one bounded, closed v1 release-index document.
    ///
    /// This validates only the serialized index's internal release contract.
    /// Callers still need the M4 package/read-back verifier and measured outer
    /// asset hashes before accepting local or downloaded release material.
    ///
    /// # Errors
    ///
    /// Returns AX0701 for malformed, incomplete, noncanonical, or mixed v1
    /// index material. It performs no filesystem, network, credential, tag,
    /// provenance, or release-promotion operation.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > MAX_METADATA_BYTES as usize {
            return Err(ContractError::index(
                "release index exceeds the configured metadata limit",
            ));
        }
        let index: Self = serde_json::from_slice(input)
            .map_err(|_| ContractError::index("release index is not a closed v1 JSON document"))?;
        index.validate()?;
        Ok(index)
    }

    fn validate(&self) -> Result<(), ContractError> {
        if self.schema != 1 || !safe_segment(&self.release_id) {
            return Err(ContractError::index(
                "release index has an unsupported schema or unsafe release identifier",
            ));
        }
        if self.base_url != self.base_url.trim_end_matches('/') {
            return Err(ContractError::index(
                "release index base URL is not in canonical form",
            ));
        }
        let base_url = parse_credential_free_https_url(&self.base_url)
            .map_err(|_| ContractError::index("release index has an invalid base URL"))?;
        if base_url.path() == "/" {
            return Err(ContractError::index(
                "release index base URL must name a release path",
            ));
        }
        for identity in [
            &self.source_commit,
            &self.producer_commit,
            &self.tools_commit,
        ] {
            let _: crate::recipe::GitObjectId = identity.clone().try_into().map_err(|_| {
                ContractError::index("release index has a noncanonical Git identity")
            })?;
        }
        let active_expected = expected_matrix(ACTIVE_V1_HOSTS);
        let historical_expected = expected_matrix(V1_HOSTS);
        let mut actual = BTreeSet::new();
        let mut previous_asset: Option<&str> = None;
        for artifact in &self.artifacts {
            validate_index_artifact(artifact)?;
            if previous_asset.is_some_and(|previous| previous >= artifact.asset.as_str())
                || !actual.insert((artifact.host.clone(), artifact.target_profile.clone()))
            {
                return Err(ContractError::index(
                    "release index artifacts are unsorted or contain duplicate selectors",
                ));
            }
            previous_asset = Some(&artifact.asset);
        }
        if actual != active_expected && actual != historical_expected {
            return Err(ContractError::index(
                "release index host/profile selectors differ from an accepted v1 matrix",
            ));
        }
        Ok(())
    }
}

fn expected_matrix(hosts: &[&str]) -> BTreeSet<(String, String)> {
    hosts
        .iter()
        .flat_map(|host| {
            V1_PROFILES
                .iter()
                .map(move |profile| ((*host).to_owned(), (*profile).to_owned()))
        })
        .collect()
}

fn validate_index_artifact(artifact: &NativeReleaseArtifact) -> Result<(), ContractError> {
    if artifact.size == 0
        || !artifact.enabled
        || artifact.strip_components != 1
        || artifact.target_triple.is_empty()
        || artifact.target_triple.len() > 128
        || artifact
            .target_triple
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_'))
        || Sha256Digest::parse(&artifact.sha256).is_err()
        || Sha256Digest::parse(&artifact.tree_sha256).is_err()
    {
        return Err(ContractError::index(
            "release index artifact has invalid measured identity or consumer layout",
        ));
    }
    let expected_asset = canonical_asset_name(
        &artifact.llvm_version,
        &artifact.host,
        &artifact.target_profile,
    )
    .map_err(|_| ContractError::index("release index artifact name is not canonical"))?;
    if artifact.asset != expected_asset
        || artifact.required_paths.is_empty()
        || artifact
            .required_paths
            .iter()
            .any(|path| !safe_payload_path(path))
    {
        return Err(ContractError::index(
            "release index artifact has unsafe or incomplete required payload paths",
        ));
    }
    Ok(())
}

fn safe_payload_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.contains(['\\', '\0'])
        && !value.split('/').any(str::is_empty)
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(segment) if !segment.is_empty()))
}

/// One measured package-set member proven byte-identical across two builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSetComparisonMember {
    /// Portable package-set filename.
    pub name: String,
    /// SHA-256 measured while the two members were compared.
    pub sha256: Sha256Digest,
    /// Byte length measured while the two members were compared.
    pub size: u64,
}

/// Durable in-memory result of one complete byte-identical package-set comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSetComparison {
    /// Content address of the canonical measured member identity sequence.
    pub package_set_sha256: Sha256Digest,
    /// Every member of the exact four-file package set, sorted by name.
    pub members: Vec<PackageSetComparisonMember>,
}

/// Canonical durable evidence for one successful independent package comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageComparisonReport {
    /// Closed producer receipt schema revision.
    pub schema: u32,
    /// Fixed operation marker; no workflow-selected operation is accepted.
    pub operation: String,
    /// The comparison admitted byte equality.
    pub byte_identical: bool,
    /// Content address of the complete measured package set.
    pub package_set_sha256: Sha256Digest,
    /// Every compared member in portable lexical order.
    pub members: Vec<PackageSetComparisonMember>,
}

/// Freshly persisted and read-back-verified comparison evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageComparisonReportOutput {
    /// Canonical absolute receipt path.
    pub path: PathBuf,
    /// SHA-256 of the exact persisted receipt bytes.
    pub sha256: Sha256Digest,
    /// Strictly validated parsed receipt.
    pub report: PackageComparisonReport,
}

/// Compare two complete package sets byte-for-byte without copying either set.
///
/// # Errors
///
/// Returns AX0702 unless both directories contain the same one closed package
/// set and every corresponding member is byte-identical.  The returned member
/// identities are measured from the exact byte streams that passed comparison,
/// so a workflow can persist durable admission evidence without a shell pipe.
pub fn compare_package_sets(
    left: &Path,
    right: &Path,
) -> Result<PackageSetComparison, ContractError> {
    let left = comparison_directory(left)?;
    let right = comparison_directory(right)?;
    if left == right {
        return Err(ContractError::comparison(
            "package comparison requires two distinct canonical directories",
        ));
    }
    let left_members = package_set_members(&left)?;
    let right_members = package_set_members(&right)?;
    if left_members != right_members {
        return Err(ContractError::comparison(
            "package-set member names differ between independent outputs",
        ));
    }
    let mut members = Vec::with_capacity(left_members.len());
    for name in left_members {
        let left_path = left.join(&name);
        let right_path = right.join(&name);
        members.push(compare_member(&name, &left_path, &right_path)?);
    }
    let package_set_sha256 = sha256_bytes(&comparison_canonical_bytes(&members)?);
    Ok(PackageSetComparison {
        package_set_sha256,
        members,
    })
}

/// Persist canonical, read-back-verified admission evidence for a comparison.
///
/// The caller supplies the in-memory result returned by
/// [`compare_package_sets`]. The receipt never records local paths, so its
/// bytes are portable evidence of the compared four-member package identity.
/// Existing outputs are never adopted or replaced.
///
/// # Errors
///
/// Returns AX0702 when the comparison result is noncanonical, the destination
/// is unsafe or already exists, or durable write/read-back validation fails.
pub fn write_package_comparison_report(
    output: &Path,
    comparison: &PackageSetComparison,
) -> Result<PackageComparisonReportOutput, ContractError> {
    let report = PackageComparisonReport {
        schema: 1,
        operation: "compare".into(),
        byte_identical: true,
        package_set_sha256: comparison.package_set_sha256.clone(),
        members: comparison.members.clone(),
    };
    validate_comparison_report(&report)?;
    let output = comparison_report_path(output)?;
    let mut bytes = comparison_canonical_bytes(&report)?;
    bytes.push(b'\n');
    publish_atomic_file(&output, &bytes, AtomicFilePolicy::NoClobber)
        .map_err(|_| ContractError::comparison("cannot durably create native comparison report"))?;
    let persisted = read_comparison_report(&output)?;
    if persisted != bytes {
        return Err(ContractError::comparison(
            "persisted native comparison report bytes changed after publication",
        ));
    }
    let parsed: PackageComparisonReport = serde_json::from_slice(&persisted).map_err(|_| {
        ContractError::comparison("persisted native comparison report is not valid JSON")
    })?;
    validate_comparison_report(&parsed)?;
    if parsed != report {
        return Err(ContractError::comparison(
            "persisted native comparison report differs from its requested evidence",
        ));
    }
    Ok(PackageComparisonReportOutput {
        path: output,
        sha256: sha256_bytes(&persisted),
        report: parsed,
    })
}

/// Validate and advance one complete active v1 local release inventory.
///
/// At the pre-attestation stage, the directory must contain 36 package assets
/// plus five support files. This function then writes the index and a checksum
/// file covering those 42 attestation subjects. At the final stage it requires
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
    let mut names = Vec::with_capacity(ACTIVE_V1_HOSTS.len() * V1_PROFILES.len());
    for host in ACTIVE_V1_HOSTS {
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
    for (name, kind, expected) in [
        (
            MANIFEST_SCHEMA_NAME,
            "toolchain manifest schema",
            MANIFEST_SCHEMA_BYTES,
        ),
        (TREE_FIXTURE_NAME, "tree digest fixture", TREE_FIXTURE_BYTES),
    ] {
        let actual = read_metadata(&directory.join(name), kind)?;
        if actual != expected {
            return Err(ContractError::index(format!(
                "{kind} does not match the versioned native conformance material"
            )));
        }
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
    let verification = PackageVerificationRequest {
        package_dir: directory.to_path_buf(),
        release_id: request.release_id.clone(),
        host: manifest.host.clone(),
        recipe: recipe.clone(),
        source_lock: source_lock.clone(),
        profile: profile.clone(),
        build_environment: manifest.build_environment.clone(),
        forbidden_prefixes: vec![],
    };
    let verified = verify_members(
        &verification,
        &PackageAssetPaths::for_asset(directory, asset),
    )
    .map_err(|_| {
        ContractError::index(
            "package set did not pass bounded native archive read-back verification",
        )
    })?;
    let required_paths = required_paths(source_lock.version(), profile)?;
    verify_required_paths(&verified.manifest, &required_paths)?;
    Ok(NativeReleaseArtifact {
        asset: asset.into(),
        sha256: verified.archive_sha256.to_string(),
        size: verified.archive_size,
        host: verified.manifest.host,
        target_profile: verified.manifest.target_profile,
        target_triple: verified.manifest.target_triple,
        tree_sha256: verified.manifest.tree_sha256,
        llvm_version: verified.manifest.llvm_version.unwrap_or_default(),
        enabled: true,
        strip_components: 1,
        required_paths,
    })
}

fn verify_required_paths(
    manifest: &ArosToolchainManifest,
    required_paths: &[String],
) -> Result<(), ContractError> {
    for path in required_paths {
        if path == AROS_TOOLCHAIN_MANIFEST_FILE {
            continue;
        }
        let Some(entry) = manifest.files.iter().find(|entry| entry.path == *path) else {
            return Err(ContractError::index(
                "verified package payload is missing an indexed required path",
            ));
        };
        if !matches!(entry.kind.as_str(), "file" | "symlink") {
            return Err(ContractError::index(
                "indexed required path is not a package file or symbolic link",
            ));
        }
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

fn comparison_directory(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::comparison(
            "package comparison directories must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::comparison("package comparison directory is inaccessible"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::comparison(
            "package comparison directory must be a real directory",
        ));
    }
    path.canonicalize().map_err(|_| {
        ContractError::comparison("package comparison directory cannot be canonicalized")
    })
}

fn comparison_report_path(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::comparison(
            "native comparison report output must be absolute",
        ));
    }
    let leaf = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ContractError::comparison("native comparison report output has no UTF-8 file name")
        })?;
    if !safe_segment(leaf)
        || Path::new(leaf)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
    {
        return Err(ContractError::comparison(
            "native comparison report output must use one safe JSON basename",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ContractError::comparison("native comparison report output has no parent directory")
    })?;
    let parent = parent.canonicalize().map_err(|_| {
        ContractError::comparison("native comparison report output parent is unavailable")
    })?;
    let metadata = fs::symlink_metadata(&parent).map_err(|_| {
        ContractError::comparison("native comparison report output parent is unavailable")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::comparison(
            "native comparison report output parent must be a real directory",
        ));
    }
    let output = parent.join(leaf);
    match fs::symlink_metadata(&output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Ok(_) => Err(ContractError::comparison(
            "native comparison report output already exists and cannot be adopted",
        )),
        Err(_) => Err(ContractError::comparison(
            "cannot inspect native comparison report output",
        )),
    }
}

fn read_comparison_report(path: &Path) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::comparison("cannot safely open native comparison report"))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::comparison("cannot inspect native comparison report"))?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES {
        return Err(ContractError::comparison(
            "native comparison report is not a regular bounded file",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).map_err(|_| {
        ContractError::comparison("native comparison report exceeds addressable memory")
    })?);
    Read::by_ref(&mut file)
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::comparison("cannot read native comparison report"))?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(ContractError::comparison(
            "native comparison report changed or exceeded its limit while it was read",
        ));
    }
    Ok(bytes)
}

fn validate_comparison_report(report: &PackageComparisonReport) -> Result<(), ContractError> {
    if report.schema != 1 || report.operation != "compare" || !report.byte_identical {
        return Err(ContractError::comparison(
            "native comparison report has an unsupported schema or operation",
        ));
    }
    if report.members.len() != 4
        || report
            .members
            .windows(2)
            .any(|pair| pair[0].name >= pair[1].name)
        || report.members.iter().any(|member| {
            member.size == 0 || !safe_segment(&member.name) || member.name.contains('/')
        })
    {
        return Err(ContractError::comparison(
            "native comparison report members are incomplete or noncanonical",
        ));
    }
    let names = report
        .members
        .iter()
        .map(|member| member.name.clone())
        .collect::<BTreeSet<_>>();
    let archives = names
        .iter()
        .filter(|name| name.ends_with(".tar.xz"))
        .collect::<Vec<_>>();
    if archives.len() != 1
        || names
            != [
                archives[0].clone(),
                format!("{}.manifest.json", archives[0]),
                format!("{}.sha256", archives[0]),
                format!("{}.spdx.json", archives[0]),
            ]
            .into_iter()
            .collect()
    {
        return Err(ContractError::comparison(
            "native comparison report does not describe one exact package set",
        ));
    }
    if sha256_bytes(&comparison_canonical_bytes(&report.members)?) != report.package_set_sha256 {
        return Err(ContractError::comparison(
            "native comparison report package-set digest is inconsistent",
        ));
    }
    Ok(())
}

fn comparison_canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    let value = serde_json::to_value(value).map_err(|_| {
        ContractError::comparison("cannot serialize native comparison report material")
    })?;
    canonical::bytes(&value)
}

fn compare_member(
    name: &str,
    left: &Path,
    right: &Path,
) -> Result<PackageSetComparisonMember, ContractError> {
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
        return Err(ContractError::comparison(
            "package-set members differ between independent outputs",
        ));
    }
    let mut left_buffer = vec![0_u8; 128 * 1024].into_boxed_slice();
    let mut right_buffer = vec![0_u8; 128 * 1024].into_boxed_slice();
    let mut hasher = Sha256::new();
    let mut read_total = 0_u64;
    loop {
        let left_read = left
            .read(&mut left_buffer)
            .map_err(|_| ContractError::comparison("cannot read left package member"))?;
        let right_read = right
            .read(&mut right_buffer)
            .map_err(|_| ContractError::comparison("cannot read right package member"))?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..left_read] {
            return Err(ContractError::comparison(
                "package-set members differ between independent outputs",
            ));
        }
        hasher.update(&left_buffer[..left_read]);
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
            if read_total != left_size {
                return Err(ContractError::comparison(
                    "package comparison member changed while it was read",
                ));
            }
            return Ok(PackageSetComparisonMember {
                name: name.to_owned(),
                sha256: finish_sha256(hasher),
                size: read_total,
            });
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
    use aros_common::sha256_bytes;
    use serde_json::{json, Map};

    use crate::package::{package, PackageRequest};

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
            "source_commit": "7".repeat(40), "source_tree": "2".repeat(40),
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

    fn write_candidate(root: &Path, llvm_version: &str, profile: &Profile) {
        for path in required_paths(llvm_version, profile).unwrap() {
            if path == AROS_TOOLCHAIN_MANIFEST_FILE {
                continue;
            }
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(destination, b"native release-index fixture\n").unwrap();
        }
    }

    fn copy_package_to_release(root: &Path, output: &crate::package::PackageOutput) {
        for source in [
            &output.archive,
            &output.manifest,
            &output.checksum,
            &output.sbom,
        ] {
            let name = source.file_name().unwrap();
            fs::copy(source, root.join(name)).unwrap();
        }
    }

    fn write_complete_pre_attestation_fixture(root: &Path) -> IndexRequest {
        let source_lock_bytes = fixture_source_lock();
        let profiles_bytes = fixture_profiles();
        let recipe_bytes = fixture_recipe(&source_lock_bytes, &profiles_bytes);
        fs::write(root.join(RECIPE_NAME), &recipe_bytes).unwrap();
        fs::write(root.join("llvm-11.sources.json"), &source_lock_bytes).unwrap();
        fs::write(root.join(PROFILES_NAME), &profiles_bytes).unwrap();
        fs::write(root.join(MANIFEST_SCHEMA_NAME), MANIFEST_SCHEMA_BYTES).unwrap();
        fs::write(root.join(TREE_FIXTURE_NAME), TREE_FIXTURE_BYTES).unwrap();
        let source_lock = SourceLock::parse(&source_lock_bytes).unwrap();
        let profiles = Profiles::parse(&profiles_bytes).unwrap();
        let recipe = Recipe::parse(&recipe_bytes).unwrap();
        let staging = tempfile::tempdir().unwrap();
        for host in ACTIVE_V1_HOSTS {
            for profile_name in V1_PROFILES {
                let profile = profiles.select(profile_name).unwrap().clone();
                let candidate = staging
                    .path()
                    .join(format!("candidate-{host}-{profile_name}"));
                fs::create_dir(&candidate).unwrap();
                write_candidate(&candidate, source_lock.version(), &profile);
                let output = package(&PackageRequest {
                    candidate_root: candidate,
                    output_dir: staging
                        .path()
                        .join(format!("package-{host}-{profile_name}")),
                    release_id: "fixture-release".into(),
                    host: (*host).into(),
                    recipe: recipe.clone(),
                    source_lock: source_lock.clone(),
                    profile,
                    build_environment: Map::new(),
                    forbidden_prefixes: vec![],
                })
                .unwrap();
                copy_package_to_release(root, &output);
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
        let comparison = compare_package_sets(&left, &right).unwrap();
        assert_eq!(comparison.members.len(), 4);
        assert_eq!(
            comparison.package_set_sha256,
            sha256_bytes(&comparison_canonical_bytes(&comparison.members).unwrap())
        );
        assert_eq!(
            comparison.members[0],
            PackageSetComparisonMember {
                name: "archive.tar.xz".into(),
                sha256: sha256_bytes(b"archive"),
                size: 7,
            }
        );
        fs::write(right.join("archive.tar.xz.spdx.json"), b"changed").unwrap();
        assert!(compare_package_sets(&left, &right).is_err());
        fs::write(right.join("extra"), b"unexpected").unwrap();
        assert!(compare_package_sets(&left, &right).is_err());
    }

    #[test]
    fn comparison_report_is_canonical_and_non_overwriting() {
        let temporary = tempfile::tempdir().unwrap();
        let left = temporary.path().join("left");
        let right = temporary.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        for directory in [&left, &right] {
            fs::write(directory.join("archive.tar.xz"), b"archive").unwrap();
            fs::write(directory.join("archive.tar.xz.manifest.json"), b"manifest").unwrap();
            fs::write(directory.join("archive.tar.xz.sha256"), b"checksum").unwrap();
            fs::write(directory.join("archive.tar.xz.spdx.json"), b"sbom").unwrap();
        }
        let comparison = compare_package_sets(&left, &right).unwrap();
        let output = temporary.path().join("comparison.json");
        let persisted = write_package_comparison_report(&output, &comparison).unwrap();
        assert_eq!(persisted.path, output.canonicalize().unwrap());
        assert_eq!(
            persisted.report.package_set_sha256,
            comparison.package_set_sha256
        );
        assert_eq!(persisted.sha256, sha256_bytes(&fs::read(&output).unwrap()));
        assert!(write_package_comparison_report(&output, &comparison).is_err());
        assert!(compare_package_sets(&left, &left).is_err());
    }

    #[test]
    fn complete_matrix_advances_only_from_pre_attestation_to_final_inventory() {
        let temporary = tempfile::tempdir().unwrap();
        let mut request = write_complete_pre_attestation_fixture(temporary.path());
        let pre = index_complete_v1(&request).unwrap();
        assert_eq!(pre.index.artifacts.len(), 9);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 43);
        assert_eq!(
            String::from_utf8(fs::read(&pre.checksums_path).unwrap())
                .unwrap()
                .lines()
                .count(),
            42
        );

        fs::write(temporary.path().join(PROVENANCE_NAME), b"{}").unwrap();
        request.stage = IndexStage::Final;
        let final_output = index_complete_v1(&request).unwrap();
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 44);
        assert_eq!(
            String::from_utf8(fs::read(&final_output.checksums_path).unwrap())
                .unwrap()
                .lines()
                .count(),
            43
        );
        let index: Value =
            serde_json::from_slice(&fs::read(final_output.index_path).unwrap()).unwrap();
        assert_eq!(index["release_id"], "fixture-release");
        assert_eq!(index["artifacts"].as_array().unwrap().len(), 9);
        assert!(index["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|artifact| artifact["required_paths"].as_array().unwrap().len() >= 26));
    }

    #[test]
    fn complete_matrix_binds_a_distinct_upstream_compatibility_reference() {
        let temporary = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(temporary.path());
        let recipe = parse_recipe(temporary.path()).unwrap();
        let profiles = parse_profiles(temporary.path(), &recipe).unwrap();

        assert_ne!(profiles.upstream_commit(), recipe.source().0);
        assert!(index_complete_v1(&request).is_ok());
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

    #[test]
    fn complete_matrix_rejects_invalid_native_archive_and_support_drift() {
        let temporary = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(temporary.path());
        let asset = canonical_asset_name("11.0.0", "linux-x86_64", "pc-x86_64").unwrap();
        let invalid_archive = b"this is not an XZ archive\n";
        fs::write(temporary.path().join(&asset), invalid_archive).unwrap();
        fs::write(
            temporary.path().join(format!("{asset}.sha256")),
            format!("{}  {asset}\n", sha256_bytes(invalid_archive)),
        )
        .unwrap();
        assert!(index_complete_v1(&request).is_err());

        let temporary = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(temporary.path());
        fs::write(temporary.path().join(MANIFEST_SCHEMA_NAME), b"{}").unwrap();
        assert!(index_complete_v1(&request).is_err());
    }

    #[test]
    fn serialized_index_parser_rejects_unknown_and_noncanonical_inventory_claims() {
        let release = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(release.path());
        let output = index_complete_v1(&request).unwrap();
        let encoded = pretty_json(&output.index).unwrap();
        assert_eq!(NativeReleaseIndex::parse(&encoded).unwrap(), output.index);

        let text = String::from_utf8(encoded).unwrap();
        assert!(NativeReleaseIndex::parse(
            text.replacen('{', "{\"unexpected\":true,", 1).as_bytes()
        )
        .is_err());
        assert!(NativeReleaseIndex::parse(
            text.replace(
                "\"release_id\": \"fixture-release\"",
                "\"release_id\": \"..\""
            )
            .as_bytes()
        )
        .is_err());
        assert!(NativeReleaseIndex::parse(
            text.replace(
                "\"base_url\": \"https://example.invalid/toolchains/fixture-release\"",
                "\"base_url\": \"https://example.invalid/toolchains/fixture-release/\""
            )
            .as_bytes()
        )
        .is_err());
    }

    #[test]
    fn serialized_index_parser_accepts_the_historical_four_host_matrix() {
        let release = tempfile::tempdir().unwrap();
        let request = write_complete_pre_attestation_fixture(release.path());
        let output = index_complete_v1(&request).unwrap();
        let mut historical = output.index;
        for profile in V1_PROFILES {
            let mut artifact = historical
                .artifacts
                .iter()
                .find(|artifact| {
                    artifact.host == "macos-aarch64" && artifact.target_profile == *profile
                })
                .unwrap()
                .clone();
            artifact.host = "macos-x86_64".into();
            artifact.asset = canonical_asset_name("11.0.0", &artifact.host, profile).unwrap();
            historical.artifacts.push(artifact);
        }
        historical
            .artifacts
            .sort_by(|left, right| left.asset.cmp(&right.asset));
        let encoded = pretty_json(&historical).unwrap();
        assert_eq!(NativeReleaseIndex::parse(&encoded).unwrap(), historical);
    }
}
