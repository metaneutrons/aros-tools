//! Separately versioned release-input collection and in-memory binding.
//!
//! A validated collection closes exact recipe, source-lock and profiles bytes
//! for each compiler family. It describes expected host/profile lanes only; it
//! is not evidence that any compiler was built, signed or released.
//! [`ReleaseInputs::parse`] is the pure in-memory boundary;
//! [`ReleaseInputs::load`] adds bounded local filesystem reads before applying
//! that same binding.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aros_common::{measure_regular_file_bounded, sha256_bytes, FileIdentity, Sha256Digest};
use serde::{Deserialize, Deserializer};

use crate::package::PackageFormat;
use crate::profiles::Profiles;
use crate::recipe::GitObjectId;
use crate::source_lock::SourceLock;
use crate::{canonical, package_identity, ContractError, Recipe};

/// The complete host set currently admitted by release-inputs-v2.
pub const ACTIVE_HOSTS: &[&str] = &["linux-aarch64", "linux-x86_64", "macos-aarch64"];

const MAX_GROUPS: usize = 32;
const MAX_FILE_NAME_BYTES: usize = 128;
const COLLECTION_FILE_NAME: &str = "toolchain-release-inputs-v2.json";
const MAX_INPUT_SET_BYTES: usize = canonical::MAX_DOCUMENT_BYTES * (1 + MAX_GROUPS * 3);
const RESERVED_INVENTORY_NAMES: &[&str] = &[
    "SHA256SUMS",
    "toolchain-index-v1.json",
    "toolchain-index-v2.json",
    "toolchain-release-inputs-v2.json",
    "toolchain-provenance.sigstore.json",
    "toolchain-recipe-v2.json",
    "profiles-v1.json",
    "toolchain-manifest-v1.schema.json",
    "toolchain-manifest-v2.schema.json",
    "tree-digest-v1.fixture.json",
];

#[derive(Debug, Clone, Copy, Deserialize)]
enum Schema {
    #[serde(rename = "aros-toolchain-release-inputs-v2")]
    V2,
}

// Deserialize the closed structures directly so duplicate object keys remain
// visible to serde and are rejected before semantic binding.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "schema")]
    _schema: Schema,
    producer_commit: GitObjectId,
    tools_commit: GitObjectId,
    hosts: Vec<String>,
    groups: Vec<GroupRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRecord {
    id: String,
    recipe: DocumentReferenceRecord,
    source_lock: DocumentReferenceRecord,
    profiles: DocumentReferenceRecord,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentReferenceRecord {
    file: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
}

/// One safe basename and the SHA-256 of its exact document bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentReference {
    file: String,
    sha256: Sha256Digest,
}

impl DocumentReference {
    /// Canonical lower-case JSON basename.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Declared digest of the exact referenced bytes.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

/// One fully parsed compiler-family input group.
#[derive(Debug, Clone)]
pub struct BoundGroup {
    id: String,
    recipe_reference: DocumentReference,
    source_lock_reference: DocumentReference,
    profiles_reference: DocumentReference,
    recipe: Recipe,
    source_lock: SourceLock,
    profiles: Profiles,
    recipe_bytes: Vec<u8>,
    source_lock_bytes: Vec<u8>,
    profiles_bytes: Vec<u8>,
}

impl BoundGroup {
    /// Sorted, unique collection group identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Exact recipe document reference.
    #[must_use]
    pub const fn recipe_reference(&self) -> &DocumentReference {
        &self.recipe_reference
    }

    /// Exact source-lock document reference.
    #[must_use]
    pub const fn source_lock_reference(&self) -> &DocumentReference {
        &self.source_lock_reference
    }

    /// Exact profiles document reference.
    #[must_use]
    pub const fn profiles_reference(&self) -> &DocumentReference {
        &self.profiles_reference
    }

    /// Parsed recipe bound to this group's exact input documents.
    #[must_use]
    pub const fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// Parsed compiler source closure.
    #[must_use]
    pub const fn source_lock(&self) -> &SourceLock {
        &self.source_lock
    }

    /// All parsed profiles in the referenced document.
    #[must_use]
    pub const fn profiles(&self) -> &Profiles {
        &self.profiles
    }

    /// Exact recipe bytes whose digest and self-consistency were checked.
    #[must_use]
    pub fn recipe_bytes(&self) -> &[u8] {
        &self.recipe_bytes
    }

    /// Exact source-lock bytes whose digest and semantics were checked.
    #[must_use]
    pub fn source_lock_bytes(&self) -> &[u8] {
        &self.source_lock_bytes
    }

    /// Exact profiles bytes whose digest and semantics were checked.
    #[must_use]
    pub fn profiles_bytes(&self) -> &[u8] {
        &self.profiles_bytes
    }
}

/// One expected group/profile/host lane derived from all bound profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedLane {
    group_id: String,
    host: String,
    profile: String,
}

impl ExpectedLane {
    /// Group supplying the recipe, source lock and profiles document.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// One of the exact active release hosts.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Producer-owned profile identifier.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

/// A parsed collection whose referenced bytes and cross-document bindings passed.
///
/// This type intentionally does not implement `Deserialize`. Construction is
/// limited to [`ReleaseInputs::parse`] and [`ReleaseInputs::load`]; both bind
/// exact documents and validate every profile in every group. `parse` is pure
/// in-memory validation, while `load` performs bounded local filesystem reads.
#[derive(Debug, Clone)]
pub struct ReleaseInputs {
    collection_sha256: Sha256Digest,
    producer_commit: GitObjectId,
    tools_commit: GitObjectId,
    hosts: Vec<String>,
    groups: Vec<BoundGroup>,
    lanes: Vec<ExpectedLane>,
}

impl ReleaseInputs {
    /// Load the fixed release-input collection and its referenced documents from a directory.
    ///
    /// The directory may contain unrelated files. The collection and each
    /// referenced document are read as bounded regular files without following
    /// symbolic links, then rebound through [`Self::parse`]. File identities,
    /// exact bytes and the release directory are rechecked before returning.
    /// These checks reject observed changes during the load but do not promise
    /// a stable snapshot against a malicious concurrent writer.
    ///
    /// # Errors
    ///
    /// Returns a sanitized contract error for an unsafe directory, malformed
    /// collection, unsafe reference, missing or changed document, or any input
    /// that fails the in-memory binding rules.
    pub fn load(directory: &Path) -> Result<Self, ContractError> {
        let directory_identity = input_directory_identity(directory)?;
        let collection_path = directory.join(COLLECTION_FILE_NAME);
        let (collection_identity, collection_bytes) =
            read_bounded_input_file(&collection_path, "release-input collection")?;
        let (_, referenced_files) = parse_collection_record(&collection_bytes)?;

        let mut total_bytes = collection_bytes.len();
        let mut documents = BTreeMap::new();
        let mut document_identities = BTreeMap::new();
        for file in referenced_files {
            let path = directory.join(&file);
            let (identity, bytes) =
                read_bounded_input_file(&path, "referenced release-input document")?;
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= MAX_INPUT_SET_BYTES)
                .ok_or_else(|| {
                    ContractError::invalid("release-input set exceeds its byte bound")
                })?;
            documents.insert(file.clone(), bytes);
            document_identities.insert(file, identity);
        }

        let mut revalidated_bytes = total_bytes;
        let (current_collection_identity, current_collection_bytes) =
            read_bounded_input_file(&collection_path, "release-input collection")?;
        revalidated_bytes = revalidated_bytes
            .checked_add(current_collection_bytes.len())
            .filter(|total| *total <= MAX_INPUT_SET_BYTES * 2)
            .ok_or_else(|| ContractError::invalid("release-input reread exceeds its byte bound"))?;
        if current_collection_identity != collection_identity
            || current_collection_bytes != collection_bytes
        {
            return Err(ContractError::identity(
                "release-input collection changed while it was loaded",
            ));
        }

        for (file, expected_identity) in document_identities {
            let path = directory.join(&file);
            let (identity, bytes) =
                read_bounded_input_file(&path, "referenced release-input document")?;
            revalidated_bytes = revalidated_bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= MAX_INPUT_SET_BYTES * 2)
                .ok_or_else(|| {
                    ContractError::invalid("release-input reread exceeds its byte bound")
                })?;
            if identity != expected_identity || documents.get(&file) != Some(&bytes) {
                return Err(ContractError::identity(
                    "referenced release-input document changed while it was loaded",
                ));
            }
        }

        require_same_input_directory(directory, directory_identity)?;
        Self::parse(&collection_bytes, &documents)
    }

    /// Parse one bounded collection and bind the exact referenced document set.
    ///
    /// `documents` must contain exactly the referenced filenames. Extra and
    /// missing entries are rejected before any referenced document is parsed.
    /// All three documents are hashed as raw bytes before their existing
    /// closed parsers are called. No filesystem, network or publication work is
    /// performed.
    ///
    /// # Errors
    ///
    /// Returns a sanitized producer-contract or identity error for malformed,
    /// unbounded, ambiguous or mutually inconsistent input material.
    pub fn parse(
        input: &[u8],
        documents: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, ContractError> {
        let (record, expected_files) = parse_collection_record(input)?;

        if documents.len() != expected_files.len() {
            return Err(ContractError::invalid(
                "bound document map must contain exactly the referenced filenames",
            ));
        }
        let supplied_files = documents.keys().cloned().collect::<BTreeSet<_>>();
        if supplied_files != expected_files {
            return Err(ContractError::invalid(
                "bound document map must contain exactly the referenced filenames",
            ));
        }

        // Check every byte slice before invoking any document parser. This
        // makes the input-map digest boundary independent of parser order.
        for group in &record.groups {
            for reference in [&group.recipe, &group.source_lock, &group.profiles] {
                let borrowed = DocumentReference {
                    file: reference.file.clone(),
                    sha256: reference.sha256.clone(),
                };
                referenced_bytes(documents, &borrowed)?;
            }
        }

        let mut groups = Vec::with_capacity(record.groups.len());
        let mut lanes = Vec::new();
        let mut profile_ids = BTreeSet::new();
        let mut source_trees = BTreeMap::<String, GitObjectId>::new();
        let mut producer_tree: Option<GitObjectId> = None;
        let mut tools_tree: Option<GitObjectId> = None;

        for group in record.groups {
            let recipe_reference = own_reference(group.recipe);
            let source_lock_reference = own_reference(group.source_lock);
            let profiles_reference = own_reference(group.profiles);
            let recipe_bytes = referenced_bytes(documents, &recipe_reference)?;
            let source_lock_bytes = referenced_bytes(documents, &source_lock_reference)?;
            let profiles_bytes = referenced_bytes(documents, &profiles_reference)?;

            let recipe = Recipe::parse(recipe_bytes)?;
            let source_lock = SourceLock::parse(source_lock_bytes)?;
            let profiles = Profiles::parse(profiles_bytes)?;
            if profiles.entries().is_empty() {
                return Err(ContractError::invalid(
                    "release-inputs-v2 group profiles document contains no profile lanes",
                ));
            }

            if recipe.producer().0 != &record.producer_commit {
                return Err(ContractError::identity(
                    "recipe producer commit differs from release-inputs-v2",
                ));
            }
            if recipe.tools().0 != &record.tools_commit {
                return Err(ContractError::identity(
                    "recipe tools commit differs from release-inputs-v2",
                ));
            }
            if let Some(expected) = &producer_tree {
                if recipe.producer().1 != expected {
                    return Err(ContractError::identity(
                        "recipe producer tree differs across release-inputs-v2 groups",
                    ));
                }
            } else {
                producer_tree = Some(recipe.producer().1.clone());
            }
            if let Some(expected) = &tools_tree {
                if recipe.tools().1 != expected {
                    return Err(ContractError::identity(
                        "recipe tools tree differs across release-inputs-v2 groups",
                    ));
                }
            } else {
                tools_tree = Some(recipe.tools().1.clone());
            }
            if recipe.source().0 != profiles.upstream_commit() {
                return Err(ContractError::identity(
                    "recipe source commit differs from profiles upstream commit",
                ));
            }
            let (source_commit, source_tree) = recipe.source();
            if let Some(expected) = source_trees.get(source_commit.as_str()) {
                if source_tree != expected {
                    return Err(ContractError::identity(
                        "recipe source tree differs for a repeated source commit across release-inputs-v2 groups",
                    ));
                }
            } else {
                source_trees.insert(source_commit.as_str().to_owned(), source_tree.clone());
            }
            if source_lock.family() != profiles.family() {
                return Err(ContractError::identity(
                    "source lock and profiles select different compiler families",
                ));
            }

            for profile in profiles.entries() {
                if !profile_ids.insert(profile.name().to_owned()) {
                    return Err(ContractError::invalid(
                        "profile IDs must be globally unique across release-inputs-v2 groups",
                    ));
                }
                package_identity::compiler_identity_for_format(
                    &source_lock,
                    profile,
                    PackageFormat::CompilerFamilyV2,
                )
                .map_err(|_| {
                    ContractError::identity(
                        "profile does not form a valid compiler-family-v2 identity",
                    )
                })?;
                package_identity::require_recipe_binding(
                    &recipe,
                    &source_lock,
                    profile,
                    PackageFormat::CompilerFamilyV2,
                )
                .map_err(|_| {
                    ContractError::identity(
                        "recipe does not bind the exact source lock, profiles and patch closure",
                    )
                })?;
                for host in ACTIVE_HOSTS {
                    lanes.push(ExpectedLane {
                        group_id: group.id.clone(),
                        host: (*host).to_owned(),
                        profile: profile.name().to_owned(),
                    });
                }
            }

            groups.push(BoundGroup {
                id: group.id,
                recipe_reference,
                source_lock_reference,
                profiles_reference,
                recipe,
                source_lock,
                profiles,
                recipe_bytes: recipe_bytes.to_vec(),
                source_lock_bytes: source_lock_bytes.to_vec(),
                profiles_bytes: profiles_bytes.to_vec(),
            });
        }

        Ok(Self {
            collection_sha256: sha256_bytes(input),
            producer_commit: record.producer_commit,
            tools_commit: record.tools_commit,
            hosts: record.hosts,
            groups,
            lanes,
        })
    }

    /// SHA-256 measured from the exact collection bytes supplied to `parse`.
    #[must_use]
    pub const fn collection_sha256(&self) -> &Sha256Digest {
        &self.collection_sha256
    }

    /// Collection-level producer commit bound by every recipe.
    #[must_use]
    pub const fn producer_commit(&self) -> &GitObjectId {
        &self.producer_commit
    }

    /// Collection-level tools commit bound by every recipe.
    #[must_use]
    pub const fn tools_commit(&self) -> &GitObjectId {
        &self.tools_commit
    }

    /// Exact sorted active host set.
    #[must_use]
    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    /// All sorted, fully bound input groups.
    #[must_use]
    pub fn groups(&self) -> &[BoundGroup] {
        &self.groups
    }

    /// Full group/profile/host matrix derived from every bound profile.
    #[must_use]
    pub fn expected_lanes(&self) -> &[ExpectedLane] {
        &self.lanes
    }
}

fn parse_collection_record(input: &[u8]) -> Result<(Record, BTreeSet<String>), ContractError> {
    if input.len() > canonical::MAX_DOCUMENT_BYTES {
        return Err(ContractError::invalid(
            "release-inputs-v2 collection exceeds 1 MiB",
        ));
    }
    let record: Record = serde_json::from_slice(input).map_err(|error| {
        ContractError::invalid(format!(
            "invalid closed release-inputs-v2 collection at line {}, column {}",
            error.line(),
            error.column()
        ))
    })?;
    if !record
        .hosts
        .iter()
        .map(String::as_str)
        .eq(ACTIVE_HOSTS.iter().copied())
    {
        return Err(ContractError::invalid(
            "release-inputs-v2 hosts must equal the sorted active host set",
        ));
    }
    if record.groups.is_empty() || record.groups.len() > MAX_GROUPS {
        return Err(ContractError::invalid(
            "release-inputs-v2 requires 1..32 input groups",
        ));
    }

    let mut expected_files = BTreeSet::new();
    let mut previous_group: Option<&str> = None;
    for group in &record.groups {
        if !safe_group_id(&group.id)
            || previous_group.is_some_and(|previous| previous >= group.id.as_str())
        {
            return Err(ContractError::invalid(
                "release-inputs-v2 group IDs must be sorted, unique lower-case kebab IDs",
            ));
        }
        previous_group = Some(&group.id);
        for reference in [&group.recipe, &group.source_lock, &group.profiles] {
            if !safe_document_filename(&reference.file) {
                return Err(ContractError::invalid(
                    "release-inputs-v2 contains an unsafe or reserved document filename",
                ));
            }
            if !expected_files.insert(reference.file.clone()) {
                return Err(ContractError::invalid(
                    "release-inputs-v2 document filenames must be globally unique",
                ));
            }
        }
    }
    Ok((record, expected_files))
}

fn read_bounded_input_file(
    path: &Path,
    description: &str,
) -> Result<(FileIdentity, Vec<u8>), ContractError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ContractError::invalid(format!("required {description} is missing"))
        } else {
            ContractError::invalid(format!("cannot safely read {description}"))
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(ContractError::invalid(format!(
            "cannot safely read {description}"
        )));
    }
    if metadata.len() > canonical::MAX_DOCUMENT_BYTES as u64 {
        return Err(ContractError::invalid(format!(
            "{description} exceeds the 1 MiB input bound"
        )));
    }
    let (identity, bytes) =
        measure_regular_file_bounded(path, canonical::MAX_DOCUMENT_BYTES as u64)
            .map_err(|_| ContractError::invalid(format!("cannot safely read {description}")))?
            .ok_or_else(|| ContractError::invalid(format!("required {description} is missing")))?;
    if bytes.len() > canonical::MAX_DOCUMENT_BYTES {
        return Err(ContractError::invalid(format!(
            "{description} exceeds the 1 MiB input bound"
        )));
    }
    Ok((identity, bytes))
}

#[cfg(unix)]
fn input_directory_identity(directory: &Path) -> Result<(u64, u64), ContractError> {
    use std::os::unix::fs::MetadataExt as _;

    crate::release_index_v2_readback::validate_directory_path(directory).map_err(|_| {
        ContractError::invalid("release-input directory must have real absolute ancestors")
    })?;
    let handle = crate::filesystem::open_directory(directory)
        .map_err(|_| ContractError::invalid("cannot safely open release-input directory"))?;
    let metadata = handle
        .metadata()
        .map_err(|_| ContractError::invalid("cannot inspect release-input directory"))?;
    if !metadata.is_dir() {
        return Err(ContractError::invalid(
            "release-input path is not a real directory",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn input_directory_identity(_directory: &Path) -> Result<(u64, u64), ContractError> {
    Err(ContractError::invalid(
        "safe release-input loading is unavailable on this platform",
    ))
}

fn require_same_input_directory(
    directory: &Path,
    expected: (u64, u64),
) -> Result<(), ContractError> {
    if input_directory_identity(directory)? != expected {
        return Err(ContractError::identity(
            "release-input directory identity changed while loading",
        ));
    }
    Ok(())
}

fn own_reference(record: DocumentReferenceRecord) -> DocumentReference {
    DocumentReference {
        file: record.file,
        sha256: record.sha256,
    }
}

fn referenced_bytes<'a>(
    documents: &'a BTreeMap<String, Vec<u8>>,
    reference: &DocumentReference,
) -> Result<&'a [u8], ContractError> {
    let bytes = documents
        .get(&reference.file)
        .ok_or_else(|| ContractError::invalid("referenced document is missing"))?;
    if bytes.len() > canonical::MAX_DOCUMENT_BYTES {
        return Err(ContractError::invalid(
            "referenced release input document exceeds 1 MiB",
        ));
    }
    if sha256_bytes(bytes) != reference.sha256 {
        return Err(ContractError::identity(
            "referenced document bytes do not match their declared SHA-256",
        ));
    }
    Ok(bytes)
}

fn safe_group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn safe_document_filename(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_FILE_NAME_BYTES
        || value.starts_with('.')
        || value
            .rsplit_once('.')
            .is_none_or(|(_, extension)| extension != "json")
        || RESERVED_INVENTORY_NAMES.contains(&value)
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte)
        })
        || value.split('.').any(str::is_empty)
    {
        return false;
    }
    true
}

fn digest<'de, D>(deserializer: D) -> Result<Sha256Digest, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "expected a lowercase 64-hex SHA-256 digest",
        ));
    }
    Sha256Digest::parse(&value)
        .map_err(|_| serde::de::Error::custom("expected a lowercase 64-hex SHA-256 digest"))
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use aros_common::diagnostic::DiagnosticCode;
    use serde_json::{json, Value};

    use super::*;

    const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
    const LLVM_SOURCE_COMMIT: &str = "2222222222222222222222222222222222222222";
    const PRODUCER_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOOLS_COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const RECIPE_FILE: &str = "gnu-rv32-recipe.json";
    const SOURCE_LOCK_FILE: &str = "gnu-rv32-source-lock.json";
    const PROFILES_FILE: &str = "gnu-rv32-profiles.json";

    struct Fixture {
        collection: Vec<u8>,
        documents: BTreeMap<String, Vec<u8>>,
    }

    fn target() -> Value {
        json!({
            "schema": "aros-riscv-target-v1",
            "isa": "rv32imafc",
            "abi": "ilp32f",
            "code_model": "medany",
            "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        })
    }

    fn fixture() -> Fixture {
        let source_lock = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v3",
            "family": "gnu",
            "version": "16.2.0",
            "sources": [
                {
                    "component": "gcc", "version": "16.2.0", "purpose": "toolchain-component",
                    "filename": "gcc.tar.xz", "url": "https://example.invalid/gcc.tar.xz",
                    "sha256": "1".repeat(64), "size": 1
                },
                {
                    "component": "binutils", "version": "2.47", "purpose": "toolchain-component",
                    "filename": "binutils.tar.xz", "url": "https://example.invalid/binutils.tar.xz",
                    "sha256": "2".repeat(64), "size": 1
                }
            ],
            "host_python_packages": [{
                "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
                "url": "https://example.invalid/mako.tar.gz", "sha256": "3".repeat(64),
                "size": 1, "source_root": "mako", "python_path": "."
            }]
        }))
        .unwrap();
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2",
            "family": "gnu",
            "upstream_commit": SOURCE_COMMIT,
            "profiles": [{
                "name": "rv32-aros", "configure_target": "riscv-aros",
                "upstream_output_target": "riscv-aros", "target_triple": "riscv-aros",
                "cpu": "riscv", "platform": "fixture", "float_abi": "ilp32f",
                "capabilities": ["c", "libgcc", "standalone-collector"],
                "target": target()
            }]
        }))
        .unwrap();
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": SOURCE_COMMIT,
            "source_tree": "7".repeat(40),
            "producer_commit": PRODUCER_COMMIT,
            "producer_tree": "e".repeat(40),
            "tools_commit": TOOLS_COMMIT,
            "tools_tree": "f".repeat(40),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(&source_lock),
            "profiles_sha256": sha256_bytes(&profiles),
            "patches": []
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        let recipe = serde_json::to_vec(&recipe).unwrap();

        let documents = BTreeMap::from([
            (RECIPE_FILE.to_owned(), recipe.clone()),
            (SOURCE_LOCK_FILE.to_owned(), source_lock.clone()),
            (PROFILES_FILE.to_owned(), profiles.clone()),
        ]);
        let collection = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": ACTIVE_HOSTS,
            "groups": [{
                "id": "gnu-rv32",
                "recipe": reference(RECIPE_FILE, &recipe),
                "source_lock": reference(SOURCE_LOCK_FILE, &source_lock),
                "profiles": reference(PROFILES_FILE, &profiles)
            }]
        }))
        .unwrap();
        Fixture {
            collection,
            documents,
        }
    }

    fn two_family_fixture() -> Fixture {
        let mut fixture = fixture();
        let source_lock = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2",
            "family": "llvm",
            "version": "17.0.6",
            "sources": [{
                "component": "llvm", "version": "17.0.6", "purpose": "toolchain-component",
                "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "4".repeat(64), "size": 1
            }],
            "host_python_packages": [{
                "name": "wheel", "version": "0.43.0", "filename": "wheel.tar.gz",
                "url": "https://example.invalid/wheel.tar.gz", "sha256": "5".repeat(64),
                "size": 1, "source_root": "wheel", "python_path": "."
            }]
        }))
        .unwrap();
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2",
            "family": "llvm",
            "upstream_commit": LLVM_SOURCE_COMMIT,
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64",
                "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
                "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap();
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": LLVM_SOURCE_COMMIT,
            "source_tree": "8".repeat(40),
            "producer_commit": PRODUCER_COMMIT,
            "producer_tree": "e".repeat(40),
            "tools_commit": TOOLS_COMMIT,
            "tools_tree": "f".repeat(40),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(&source_lock),
            "profiles_sha256": sha256_bytes(&profiles),
            "patches": []
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        let recipe = serde_json::to_vec(&recipe).unwrap();
        let names = [
            "llvm-pc-recipe.json",
            "llvm-pc-source-lock.json",
            "llvm-pc-profiles.json",
        ];
        for (name, bytes) in names.iter().zip([&recipe, &source_lock, &profiles]) {
            fixture.documents.insert((*name).to_owned(), bytes.clone());
        }

        let mut collection: Value = serde_json::from_slice(&fixture.collection).unwrap();
        collection["groups"].as_array_mut().unwrap().push(json!({
            "id": "llvm-pc",
            "recipe": reference(names[0], &recipe),
            "source_lock": reference(names[1], &source_lock),
            "profiles": reference(names[2], &profiles)
        }));
        fixture.collection = serde_json::to_vec(&collection).unwrap();
        fixture
    }

    fn reference(file: &str, bytes: &[u8]) -> Value {
        json!({"file": file, "sha256": sha256_bytes(bytes)})
    }

    fn write_fixture(directory: &Path, fixture: &Fixture) {
        fs::write(directory.join(COLLECTION_FILE_NAME), &fixture.collection).unwrap();
        for (name, bytes) in &fixture.documents {
            fs::write(directory.join(name), bytes).unwrap();
        }
    }

    fn assert_load_error(
        result: Result<ReleaseInputs, ContractError>,
        expected_code: DiagnosticCode,
        expected_reason: &str,
    ) {
        let Err(error) = result else {
            panic!("expected release-input loading to fail");
        };
        let diagnostic = &error.diagnostics().diagnostics[0];
        assert_eq!(diagnostic.code, expected_code);
        assert!(
            diagnostic.message.contains(expected_reason),
            "expected reason {expected_reason:?}, got {:?}",
            diagnostic.message
        );
    }

    fn real_temp_path(directory: &tempfile::TempDir) -> PathBuf {
        directory.path().canonicalize().unwrap()
    }

    #[test]
    fn loads_bound_inputs_and_allows_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let fixture = fixture();
        write_fixture(&root, &fixture);
        fs::write(root.join("notes.txt"), b"unrelated").unwrap();

        let inputs = ReleaseInputs::load(&root).unwrap();
        assert_eq!(
            inputs.collection_sha256(),
            &sha256_bytes(&fixture.collection)
        );
        assert_eq!(inputs.groups().len(), 1);
        assert_eq!(inputs.groups()[0].id(), "gnu-rv32");
        assert_eq!(inputs.expected_lanes().len(), ACTIVE_HOSTS.len());
        assert!(inputs
            .expected_lanes()
            .iter()
            .all(|lane| lane.profile() == "rv32-aros"));
    }

    #[test]
    fn loads_mixed_gnu_and_llvm_groups_without_family_special_casing() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let fixture = two_family_fixture();
        write_fixture(&root, &fixture);

        let inputs = ReleaseInputs::load(&root).unwrap();
        assert_eq!(inputs.groups().len(), 2);
        assert_eq!(inputs.groups()[0].id(), "gnu-rv32");
        assert_eq!(
            inputs.groups()[0].source_lock().family(),
            crate::source_lock::CompilerFamily::Gnu
        );
        assert_eq!(inputs.groups()[1].id(), "llvm-pc");
        assert_eq!(
            inputs.groups()[1].source_lock().family(),
            crate::source_lock::CompilerFamily::Llvm
        );
        assert_eq!(inputs.expected_lanes().len(), 2 * ACTIVE_HOSTS.len());
        assert!(inputs
            .expected_lanes()
            .iter()
            .any(|lane| lane.profile() == "rv32-aros"));
        assert!(inputs
            .expected_lanes()
            .iter()
            .any(|lane| lane.profile() == "pc-x86_64"));
    }

    #[test]
    fn rejects_a_changed_referenced_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let mut fixture = fixture();
        fixture.documents.get_mut(PROFILES_FILE).unwrap().push(b' ');
        write_fixture(&root, &fixture);

        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerIdentity,
            "referenced document bytes do not match their declared SHA-256",
        );
    }

    #[test]
    fn rejects_path_traversal_and_duplicate_document_references_before_reads() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let fixture = fixture();
        let mut traversal: Value = serde_json::from_slice(&fixture.collection).unwrap();
        traversal["groups"][0]["recipe"]["file"] = json!("../outside.json");
        fs::write(
            root.join(COLLECTION_FILE_NAME),
            serde_json::to_vec(&traversal).unwrap(),
        )
        .unwrap();
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "unsafe or reserved document filename",
        );

        let mut duplicate: Value = serde_json::from_slice(&fixture.collection).unwrap();
        duplicate["groups"][0]["source_lock"]["file"] = json!(RECIPE_FILE);
        fs::write(
            root.join(COLLECTION_FILE_NAME),
            serde_json::to_vec(&duplicate).unwrap(),
        )
        .unwrap();
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "document filenames must be globally unique",
        );
    }

    #[test]
    fn rejects_a_duplicate_collection_key() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let fixture = fixture();
        let collection = String::from_utf8(fixture.collection).unwrap();
        let duplicate = collection.replacen(
            "\"schema\":\"aros-toolchain-release-inputs-v2\"",
            "\"schema\":\"aros-toolchain-release-inputs-v2\",\"schema\":\"aros-toolchain-release-inputs-v2\"",
            1,
        );
        assert_ne!(duplicate, collection);
        fs::write(root.join(COLLECTION_FILE_NAME), duplicate).unwrap();

        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "invalid closed release-inputs-v2 collection",
        );
    }

    #[test]
    fn rejects_oversized_collection_document_and_missing_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        fs::write(
            root.join(COLLECTION_FILE_NAME),
            vec![b' '; canonical::MAX_DOCUMENT_BYTES + 1],
        )
        .unwrap();
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "release-input collection exceeds the 1 MiB input bound",
        );

        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let mut oversized_fixture = fixture();
        oversized_fixture.documents.insert(
            PROFILES_FILE.to_owned(),
            vec![b' '; canonical::MAX_DOCUMENT_BYTES + 1],
        );
        write_fixture(&root, &oversized_fixture);
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "referenced release-input document exceeds the 1 MiB input bound",
        );

        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let fixture = fixture();
        fs::write(root.join(COLLECTION_FILE_NAME), &fixture.collection).unwrap();
        for (name, bytes) in &fixture.documents {
            if name != PROFILES_FILE {
                fs::write(root.join(name), bytes).unwrap();
            }
        }
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "required referenced release-input document is missing",
        );
    }

    #[test]
    fn rejects_symlink_collection_document_and_directory_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let collection_fixture = fixture();
        let external_collection = root.join("external-collection.json");
        fs::write(&external_collection, &collection_fixture.collection).unwrap();
        symlink(&external_collection, root.join(COLLECTION_FILE_NAME)).unwrap();
        for (name, bytes) in &collection_fixture.documents {
            fs::write(root.join(name), bytes).unwrap();
        }
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "cannot safely read release-input collection",
        );

        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let document_fixture = fixture();
        fs::write(
            root.join(COLLECTION_FILE_NAME),
            &document_fixture.collection,
        )
        .unwrap();
        let external_document = root.join("external-profiles.json");
        fs::write(
            &external_document,
            document_fixture.documents.get(PROFILES_FILE).unwrap(),
        )
        .unwrap();
        for (name, bytes) in &document_fixture.documents {
            if name == PROFILES_FILE {
                symlink(&external_document, root.join(name)).unwrap();
            } else {
                fs::write(root.join(name), bytes).unwrap();
            }
        }
        assert_load_error(
            ReleaseInputs::load(&root),
            DiagnosticCode::ProducerContract,
            "cannot safely read referenced release-input document",
        );

        let directory = tempfile::tempdir().unwrap();
        let root = real_temp_path(&directory);
        let real_directory = root.join("real");
        fs::create_dir(&real_directory).unwrap();
        write_fixture(&real_directory, &document_fixture);
        let aliased_directory = root.join("alias");
        symlink(&real_directory, &aliased_directory).unwrap();
        assert_load_error(
            ReleaseInputs::load(&aliased_directory),
            DiagnosticCode::ProducerContract,
            "release-input directory must have real absolute ancestors",
        );
    }
}
