//! Pure, bounded release-index-v2 parsing against validated release inputs.
//!
//! The index declares an expected inventory and producer-supplied artifact
//! identities. It does not measure files, verify archives or signatures, or
//! establish that a release is ready for publication.

use std::collections::BTreeSet;

use aros_common::{parse_credential_free_https_url, ArosCompilerIdentity, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::package::PackageFormat;
use crate::recipe::GitObjectId;
use crate::release_inputs::ReleaseInputs;
use crate::{package_identity, ContractError};

const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_REQUIRED_PATHS: usize = 500_000;
const MAX_REQUIRED_PATH_BYTES: usize = 1024;

pub(crate) const MANIFEST_SCHEMA_NAME: &str = "toolchain-manifest-v2.schema.json";
pub(crate) const TREE_FIXTURE_NAME: &str = "tree-digest-v1.fixture.json";
pub(crate) const CHECKSUMS_NAME: &str = "SHA256SUMS";
/// Canonical compiler-family release index basename.
pub const INDEX_NAME: &str = "toolchain-index-v2.json";
/// Canonical provenance bundle basename; parsing an index does not authenticate it.
pub const PROVENANCE_NAME: &str = "toolchain-provenance.sigstore.json";

const SUPPORT_FILES: &[&str] = &[
    "toolchain-release-inputs-v2.json",
    INDEX_NAME,
    CHECKSUMS_NAME,
    PROVENANCE_NAME,
    MANIFEST_SCHEMA_NAME,
    TREE_FIXTURE_NAME,
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    release_id: String,
    base_url: String,
    inputs_sha256: String,
    producer_commit: GitObjectId,
    tools_commit: GitObjectId,
    artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactRecord {
    group_id: String,
    asset: String,
    sha256: String,
    size: u64,
    host: String,
    target_profile: String,
    target_triple: String,
    source_commit: GitObjectId,
    compiler: ArosCompilerIdentity,
    tree_sha256: String,
    enabled: bool,
    strip_components: usize,
    required_paths: Vec<String>,
}

/// A validated v2 release index bound to one parsed [`ReleaseInputs`] value.
///
/// This type has no public constructor or `Deserialize` implementation.
/// Construction is limited to [`Self::parse`], and its fields remain private
/// so callers cannot bypass the input-derived lane and inventory checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReleaseIndexV2 {
    schema: u32,
    release_id: String,
    base_url: String,
    inputs_sha256: Sha256Digest,
    producer_commit: GitObjectId,
    tools_commit: GitObjectId,
    artifacts: Vec<NativeReleaseArtifactV2>,
    #[serde(skip)]
    expected_inventory: BTreeSet<String>,
}

impl NativeReleaseIndexV2 {
    /// Parse a bounded, closed v2 index and bind it to validated release inputs.
    ///
    /// Artifact lanes and package names are derived from the exact bound
    /// groups and profiles in `inputs`. The returned inventory is a declared
    /// expectation only; this method performs no filesystem, network, archive,
    /// checksum, signature, provenance or publication operation.
    ///
    /// # Errors
    ///
    /// Returns a sanitized index-contract error for malformed, unbounded,
    /// noncanonical, incomplete or input-inconsistent metadata.
    pub fn parse(input: &[u8], inputs: &ReleaseInputs) -> Result<Self, ContractError> {
        if input.len() > MAX_METADATA_BYTES {
            return Err(index_error(
                "release index v2 exceeds the configured metadata limit",
            ));
        }
        let record: Record = serde_json::from_slice(input).map_err(|error| {
            index_error(format!(
                "release index v2 is not a closed JSON document at line {}, column {}",
                error.line(),
                error.column()
            ))
        })?;
        if record.schema != 2 {
            return Err(index_error("release index has an unsupported schema"));
        }
        if !safe_release_id(&record.release_id) {
            return Err(index_error(
                "release index has an unsafe release identifier",
            ));
        }
        validate_base_url(&record.base_url)?;

        let inputs_sha256 = parse_lower_sha256(&record.inputs_sha256)?;
        if &inputs_sha256 != inputs.collection_sha256() {
            return Err(index_error(
                "release index does not identify the exact release-inputs bytes",
            ));
        }
        if &record.producer_commit != inputs.producer_commit()
            || &record.tools_commit != inputs.tools_commit()
        {
            return Err(index_error(
                "release index producer or tools commit differs from release inputs",
            ));
        }

        let expected = expected_artifacts(inputs)?;
        if record.artifacts.len() != expected.len() {
            return Err(index_error(
                "release index artifacts do not cover the complete input-derived lane matrix",
            ));
        }

        let mut artifacts = Vec::with_capacity(record.artifacts.len());
        let mut previous_asset: Option<String> = None;
        for (artifact, expected) in record.artifacts.into_iter().zip(expected) {
            if previous_asset
                .as_deref()
                .is_some_and(|previous| previous >= artifact.asset.as_str())
            {
                return Err(index_error(
                    "release index artifacts must be sorted by unique canonical asset name",
                ));
            }
            previous_asset = Some(artifact.asset.clone());

            if artifact.asset != expected.asset
                || artifact.group_id != expected.group_id
                || artifact.host != expected.host
                || artifact.target_profile != expected.target_profile
                || artifact.target_triple != expected.target_triple
                || artifact.source_commit != expected.source_commit
                || artifact.compiler != expected.compiler
            {
                return Err(index_error(
                    "release index artifact identity differs from its bound input lane",
                ));
            }
            if artifact.size == 0
                || artifact.size > MAX_ARCHIVE_BYTES
                || !artifact.enabled
                || artifact.strip_components != 1
            {
                return Err(index_error(
                    "release index artifact has invalid declared size or consumer layout",
                ));
            }
            let sha256 = parse_lower_sha256(&artifact.sha256)?;
            let tree_sha256 = parse_lower_sha256(&artifact.tree_sha256)?;
            validate_required_paths(&artifact.required_paths)?;

            artifacts.push(NativeReleaseArtifactV2 {
                group_id: artifact.group_id,
                asset: artifact.asset,
                sha256,
                size: artifact.size,
                host: artifact.host,
                target_profile: artifact.target_profile,
                target_triple: artifact.target_triple,
                source_commit: artifact.source_commit,
                compiler: artifact.compiler,
                tree_sha256,
                enabled: artifact.enabled,
                strip_components: artifact.strip_components,
                required_paths: artifact.required_paths,
            });
        }

        let expected_inventory = expected_inventory(inputs, &artifacts)?;
        Ok(Self {
            schema: record.schema,
            release_id: record.release_id,
            base_url: record.base_url,
            inputs_sha256,
            producer_commit: record.producer_commit,
            tools_commit: record.tools_commit,
            artifacts,
            expected_inventory,
        })
    }

    /// Immutable release identifier declared by the index.
    #[must_use]
    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    /// Canonical credential-free HTTPS release base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// SHA-256 of the exact release-inputs collection bytes.
    #[must_use]
    pub const fn inputs_sha256(&self) -> &Sha256Digest {
        &self.inputs_sha256
    }

    /// Producer Git object identity bound by the release inputs.
    #[must_use]
    pub const fn producer_commit(&self) -> &GitObjectId {
        &self.producer_commit
    }

    /// Tools Git object identity bound by the release inputs.
    #[must_use]
    pub const fn tools_commit(&self) -> &GitObjectId {
        &self.tools_commit
    }

    /// Complete, asset-sorted compiler-family artifact declarations.
    #[must_use]
    pub fn artifacts(&self) -> &[NativeReleaseArtifactV2] {
        &self.artifacts
    }

    /// Declared package and support filenames expected for this release.
    ///
    /// This set is derived from the bound inputs and canonical package names;
    /// it is not a measurement of files present on disk.
    #[must_use]
    pub const fn expected_inventory(&self) -> &BTreeSet<String> {
        &self.expected_inventory
    }

    /// Serialize the validated index to deterministic compact JSON bytes.
    ///
    /// The output contains only the wire fields; the derived expected
    /// inventory is not serialized.
    ///
    /// # Errors
    ///
    /// Returns a sanitized index-contract error if serialization fails or the
    /// encoded document exceeds the metadata bound.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, ContractError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| index_error("cannot encode validated release index v2"))?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(index_error(
                "encoded release index v2 exceeds the configured metadata limit",
            ));
        }
        Ok(bytes)
    }
}

/// One validated artifact declaration in [`NativeReleaseIndexV2`].
///
/// Fields are private and the type intentionally does not implement
/// `Deserialize`; callers can obtain values only from a validated index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReleaseArtifactV2 {
    group_id: String,
    asset: String,
    sha256: Sha256Digest,
    size: u64,
    host: String,
    target_profile: String,
    target_triple: String,
    source_commit: GitObjectId,
    compiler: ArosCompilerIdentity,
    tree_sha256: Sha256Digest,
    enabled: bool,
    strip_components: usize,
    required_paths: Vec<String>,
}

impl NativeReleaseArtifactV2 {
    /// Bound release-input group supplying this artifact lane.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Canonical compiler-family archive basename.
    #[must_use]
    pub fn asset(&self) -> &str {
        &self.asset
    }

    /// Producer-declared archive SHA-256; no local bytes were measured here.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Producer-declared archive byte length.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Input-derived build host selector.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Producer-owned profile identifier bound to the selected input group.
    #[must_use]
    pub fn target_profile(&self) -> &str {
        &self.target_profile
    }

    /// Target triple declared by the bound profile.
    #[must_use]
    pub fn target_triple(&self) -> &str {
        &self.target_triple
    }

    /// Source Git object identity declared by the bound recipe.
    #[must_use]
    pub const fn source_commit(&self) -> &GitObjectId {
        &self.source_commit
    }

    /// Compiler identity derived from the bound source lock and profile.
    #[must_use]
    pub const fn compiler(&self) -> &ArosCompilerIdentity {
        &self.compiler
    }

    /// Producer-declared canonical payload tree SHA-256.
    #[must_use]
    pub const fn tree_sha256(&self) -> &Sha256Digest {
        &self.tree_sha256
    }

    /// Consumer availability marker, required to be `true` by this contract.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Archive root depth to remove when extracting this package.
    #[must_use]
    pub const fn strip_components(&self) -> usize {
        self.strip_components
    }

    /// Sorted required paths declared below the archive payload root.
    #[must_use]
    pub fn required_paths(&self) -> &[String] {
        &self.required_paths
    }
}

#[derive(Debug)]
pub(crate) struct ExpectedArtifact {
    pub(crate) group_id: String,
    pub(crate) asset: String,
    pub(crate) host: String,
    pub(crate) target_profile: String,
    pub(crate) target_triple: String,
    pub(crate) source_commit: GitObjectId,
    pub(crate) compiler: ArosCompilerIdentity,
}

pub(crate) fn expected_artifacts(
    inputs: &ReleaseInputs,
) -> Result<Vec<ExpectedArtifact>, ContractError> {
    let mut expected = Vec::with_capacity(inputs.expected_lanes().len());
    for lane in inputs.expected_lanes() {
        let group = inputs
            .groups()
            .iter()
            .find(|group| group.id() == lane.group_id())
            .ok_or_else(|| index_error("release inputs contain an unbound artifact group"))?;
        let profile = group
            .profiles()
            .select(lane.profile())
            .map_err(|_| index_error("release inputs contain an unbound artifact profile"))?;
        let asset = package_identity::asset_name_for_format(
            group.source_lock(),
            profile,
            lane.host(),
            PackageFormat::CompilerFamilyV2,
        )
        .map_err(|_| index_error("release input lane has no canonical package name"))?;
        let compiler = package_identity::compiler_identity_for_format(
            group.source_lock(),
            profile,
            PackageFormat::CompilerFamilyV2,
        )
        .map_err(|_| index_error("release input lane has no valid compiler identity"))?;
        expected.push(ExpectedArtifact {
            group_id: group.id().to_owned(),
            asset,
            host: lane.host().to_owned(),
            target_profile: profile.name().to_owned(),
            target_triple: profile.target_triple().to_owned(),
            source_commit: group.recipe().source().0.clone(),
            compiler,
        });
    }
    expected.sort_by(|left, right| left.asset.cmp(&right.asset));
    if expected
        .windows(2)
        .any(|pair| pair[0].asset == pair[1].asset)
    {
        return Err(index_error(
            "release inputs derive colliding compiler package names",
        ));
    }
    Ok(expected)
}

fn expected_inventory(
    inputs: &ReleaseInputs,
    artifacts: &[NativeReleaseArtifactV2],
) -> Result<BTreeSet<String>, ContractError> {
    expected_inventory_for_assets(inputs, artifacts.iter().map(NativeReleaseArtifactV2::asset))
}

pub(crate) fn expected_inventory_for_assets<'a>(
    inputs: &ReleaseInputs,
    assets: impl Iterator<Item = &'a str>,
) -> Result<BTreeSet<String>, ContractError> {
    let mut exact_names = BTreeSet::new();
    let mut folded_names = BTreeSet::new();
    for name in SUPPORT_FILES {
        insert_inventory_name(name.to_string(), &mut exact_names, &mut folded_names)?;
    }
    for group in inputs.groups() {
        for reference in [
            group.recipe_reference(),
            group.source_lock_reference(),
            group.profiles_reference(),
        ] {
            insert_inventory_name(
                reference.file().to_owned(),
                &mut exact_names,
                &mut folded_names,
            )?;
        }
    }
    for asset in assets {
        for name in [
            asset.to_owned(),
            format!("{asset}.manifest.json"),
            format!("{asset}.sha256"),
            format!("{asset}.spdx.json"),
        ] {
            insert_inventory_name(name, &mut exact_names, &mut folded_names)?;
        }
    }
    Ok(exact_names)
}

fn insert_inventory_name(
    name: String,
    exact_names: &mut BTreeSet<String>,
    folded_names: &mut BTreeSet<String>,
) -> Result<(), ContractError> {
    let folded = name.to_ascii_lowercase();
    if exact_names.contains(&name) || folded_names.contains(&folded) {
        return Err(index_error(
            "release index derived inventory contains a filename collision",
        ));
    }
    exact_names.insert(name);
    folded_names.insert(folded);
    Ok(())
}

pub(crate) fn validate_base_url(value: &str) -> Result<(), ContractError> {
    let parsed = parse_credential_free_https_url(value)
        .map_err(|_| index_error("release index has an invalid credential-free HTTPS base URL"))?;
    if parsed.as_str() != value || parsed.path() == "/" || value.ends_with('/') {
        return Err(index_error(
            "release index base URL is not canonical or has no release path",
        ));
    }
    Ok(())
}

pub(crate) fn safe_release_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte)
        })
}

fn parse_lower_sha256(value: &str) -> Result<Sha256Digest, ContractError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(index_error(
            "release index contains a noncanonical SHA-256 digest",
        ));
    }
    Sha256Digest::parse(value)
        .map_err(|_| index_error("release index contains an invalid SHA-256 digest"))
}

pub(crate) fn validate_required_paths(paths: &[String]) -> Result<(), ContractError> {
    if paths.is_empty() || paths.len() > MAX_REQUIRED_PATHS {
        return Err(index_error(
            "release index artifact required paths are empty or exceed the configured bound",
        ));
    }
    let mut previous: Option<&str> = None;
    for path in paths {
        if !safe_payload_path(path) || previous.is_some_and(|previous| previous >= path.as_str()) {
            return Err(index_error(
                "release index artifact paths must be sorted, unique safe payload-relative paths",
            ));
        }
        previous = Some(path);
    }
    Ok(())
}

fn safe_payload_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUIRED_PATH_BYTES
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn index_error(message: impl Into<String>) -> ContractError {
    ContractError::index(message)
}
