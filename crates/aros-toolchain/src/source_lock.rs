//! Closed, source-owned input declarations for the native toolchain producer.
//!
//! A source lock is not an instruction to download or execute anything.  It
//! describes the payload closure that later M2/M3 phases must acquire and
//! revalidate through `aros-fetch`.  Keeping this parser here prevents a
//! second, drift-prone lock interpretation in the CLI or workflow.

use std::collections::{BTreeMap, BTreeSet};

use aros_common::{sha256_bytes, validate_gnu_compiler_versions, Sha256Digest};
use serde::{Deserialize, Deserializer};
use url::Url;

use crate::recipe::safe_relative_path;
use crate::{ContractError, Recipe};

const MAX_PAYLOADS: usize = 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// One semantically valid source-lock v2 or v3 document.
#[derive(Debug, Clone)]
pub struct SourceLock(Record, Sha256Digest);

/// One semantically valid standalone host-Python package lock.
#[derive(Debug, Clone)]
pub struct HostPythonLock(HostPythonLockRecord, Sha256Digest);

/// A declared archive whose exact bytes must be present in a verified cache.
#[derive(Debug, Clone, Copy)]
pub struct Payload<'a> {
    filename: &'a str,
    url: &'a str,
    sha256: &'a Sha256Digest,
    size: u64,
}

/// One source component recorded in the SPDX material relationship graph.
#[derive(Debug, Clone, Copy)]
pub struct SourceComponent<'a> {
    component: &'a str,
    version: &'a str,
    purpose: &'a SourcePurpose,
    payload: Payload<'a>,
}

impl<'a> SourceComponent<'a> {
    /// Producer-declared component name.
    #[must_use]
    pub const fn component(self) -> &'a str {
        self.component
    }

    /// Producer-declared component version.
    #[must_use]
    pub const fn version(self) -> &'a str {
        self.version
    }

    /// Role of this source in the native toolchain build.
    #[must_use]
    pub const fn purpose(self) -> SourcePurpose {
        *self.purpose
    }

    /// Exact downloaded source payload.
    #[must_use]
    pub const fn payload(self) -> Payload<'a> {
        self.payload
    }
}

impl<'a> Payload<'a> {
    /// Portable basename selected by the lock.
    #[must_use]
    pub const fn filename(self) -> &'a str {
        self.filename
    }

    /// Exact HTTPS origin selected by the source lock.
    #[must_use]
    pub const fn url(self) -> &'a str {
        self.url
    }

    /// Required SHA-256 digest.
    #[must_use]
    pub const fn sha256(self) -> &'a Sha256Digest {
        self.sha256
    }

    /// Required byte length.
    #[must_use]
    pub const fn size(self) -> u64 {
        self.size
    }
}

/// One isolated pure-Python package declaration.
#[derive(Debug, Clone)]
pub struct HostPythonPackage {
    name: String,
    version: String,
    filename: String,
    url: String,
    sha256: Sha256Digest,
    size: u64,
    source_root: String,
    python_path: String,
}

impl HostPythonPackage {
    /// Import/module name declared by the selected source contract.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Package version as declared by the lock.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Exact archive payload contract.
    #[must_use]
    pub fn payload(&self) -> Payload<'_> {
        Payload {
            filename: &self.filename,
            url: &self.url,
            sha256: &self.sha256,
            size: self.size,
        }
    }

    /// Expected archive top-level directory.
    #[must_use]
    pub fn source_root(&self) -> &str {
        &self.source_root
    }

    /// Import root below [`Self::source_root`].
    #[must_use]
    pub fn python_path(&self) -> &str {
        &self.python_path
    }
}

impl HostPythonLock {
    /// Parse a standalone, closed host-Python package lock without accessing
    /// the network or filesystem.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for an unsupported schema or invalid package metadata.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("host Python lock exceeds 1 MiB"));
        }
        let record: HostPythonLockRecord = serde_json::from_slice(input).map_err(|_| {
            ContractError::invalid("invalid or unsupported closed host Python lock document")
        })?;
        debug_assert_eq!(record.schema, HostPythonLockSchema::V1);
        if record.packages.is_empty() || record.packages.len() > MAX_PAYLOADS {
            return Err(ContractError::invalid(
                "host Python lock must declare 1..1024 packages",
            ));
        }
        let mut filenames = BTreeSet::new();
        let mut total = 0;
        validate_host_python_packages(&record.packages, &mut filenames, &mut total, false)?;
        Ok(Self(record, sha256_bytes(input)))
    }

    /// Packages in source-owned declaration order.
    #[must_use]
    pub fn packages(&self) -> &[HostPythonPackage] {
        &self.0.packages
    }

    /// Digest of the exact parsed document bytes, not a reserialization.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.1
    }
}

impl SourceLock {
    /// Parse and close a supported source-lock schema without accessing the
    /// network or the filesystem.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for invalid/ambiguous source declarations.  The parser
    /// deliberately does not trust a schema file from an input checkout: the
    /// supported schema and semantic invariants are part of this binary.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("source lock exceeds 1 MiB"));
        }
        let record: Record = serde_json::from_slice(input).map_err(|_| {
            ContractError::invalid("invalid or unsupported closed source-lock document")
        })?;
        if record.schema == Schema::V2 && record.family == CompilerFamily::Gnu {
            return Err(ContractError::invalid("invalid source-lock-v2 document"));
        }
        validate(&record)?;
        Ok(Self(record, sha256_bytes(input)))
    }

    /// Digest of the exact parsed document bytes, not a reserialization.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.1
    }

    /// Compiler-family version selected by this lock.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.0.version
    }

    /// Compiler family selected by this lock.
    #[must_use]
    pub const fn family(&self) -> CompilerFamily {
        self.0.family
    }

    /// Every source archive, in the source-owned declaration order.
    pub fn sources(&self) -> impl ExactSizeIterator<Item = Payload<'_>> {
        self.0.sources.iter().map(|source| source.payload())
    }

    /// Source components and their semantic build roles, in declaration order.
    pub fn source_components(&self) -> impl ExactSizeIterator<Item = SourceComponent<'_>> {
        self.0.sources.iter().map(Source::component)
    }

    /// Source-root-relative patch paths required by the selected source closure.
    ///
    /// The parser has already rejected unsafe and duplicate declarations.  The
    /// returned paths still need to be read as committed regular files from an
    /// exact source checkout before a recipe can bind their measured bytes.
    pub fn source_patch_paths(&self) -> impl Iterator<Item = &str> {
        self.0
            .sources
            .iter()
            .filter_map(|source| source.patch.as_deref())
    }

    /// Every host Python package, in the source-owned declaration order.
    #[must_use]
    pub fn host_python_packages(&self) -> &[HostPythonPackage] {
        &self.0.host_python_packages
    }

    /// Complete archive closure.  Filename uniqueness is guaranteed by parse.
    pub fn payloads(&self) -> impl Iterator<Item = Payload<'_>> {
        self.0.sources.iter().map(Source::payload).chain(
            self.0
                .host_python_packages
                .iter()
                .map(HostPythonPackage::payload),
        )
    }

    /// Bind source-declared patches to the exact recipe selected for this run.
    ///
    /// This prevents a valid lock from being combined with a recipe that
    /// silently omits, adds or substitutes source modifications.
    ///
    /// # Errors
    ///
    /// Returns AX0102 when the source lock and selected recipe do not describe
    /// the exact same set of source patch paths.
    pub fn verify_recipe_patches(&self, recipe: &Recipe) -> Result<(), ContractError> {
        let declared = self
            .0
            .sources
            .iter()
            .filter_map(|source| source.patch.as_deref())
            .collect::<BTreeSet<_>>();
        let recipe_patches = recipe
            .patches()
            .iter()
            .map(crate::recipe::Patch::path)
            .collect::<BTreeSet<_>>();
        if declared != recipe_patches {
            return Err(ContractError::identity(
                "source-lock patches and recipe patches do not describe the same closure",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: Schema,
    family: CompilerFamily,
    version: String,
    sources: Vec<Source>,
    host_python_packages: Vec<HostPythonPackage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostPythonLockRecord {
    schema: HostPythonLockSchema,
    packages: Vec<HostPythonPackage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum HostPythonLockSchema {
    #[serde(rename = "aros-host-python-lock-v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Schema {
    #[serde(rename = "aros-toolchain-source-lock-v2")]
    V2,
    #[serde(rename = "aros-toolchain-source-lock-v3")]
    V3,
}

/// Compiler family described by a source lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerFamily {
    #[serde(rename = "llvm")]
    Llvm,
    #[serde(rename = "gnu")]
    Gnu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourcePurpose {
    ToolchainComponent,
    TargetBuildDependency,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    component: String,
    version: String,
    purpose: SourcePurpose,
    patch: Option<String>,
    filename: String,
    url: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
    size: u64,
}

impl Source {
    fn payload(&self) -> Payload<'_> {
        Payload {
            filename: &self.filename,
            url: &self.url,
            sha256: &self.sha256,
            size: self.size,
        }
    }

    fn component(&self) -> SourceComponent<'_> {
        SourceComponent {
            component: &self.component,
            version: &self.version,
            purpose: &self.purpose,
            payload: self.payload(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostPythonPackageRecord {
    name: String,
    version: String,
    filename: String,
    url: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
    size: u64,
    source_root: String,
    python_path: String,
}

impl<'de> Deserialize<'de> for HostPythonPackage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let record = HostPythonPackageRecord::deserialize(deserializer)?;
        Ok(Self {
            name: record.name,
            version: record.version,
            filename: record.filename,
            url: record.url,
            sha256: record.sha256,
            size: record.size,
            source_root: record.source_root,
            python_path: record.python_path,
        })
    }
}

fn validate(record: &Record) -> Result<(), ContractError> {
    let supported_version = match (record.schema, record.family) {
        (Schema::V2 | Schema::V3, CompilerFamily::Llvm) => llvm_version(&record.version),
        (Schema::V3, CompilerFamily::Gnu) => gcc_version(&record.version),
        (Schema::V2, CompilerFamily::Gnu) => false,
    };
    if !supported_version
        || record.sources.is_empty()
        || record.host_python_packages.is_empty()
        || record
            .sources
            .len()
            .saturating_add(record.host_python_packages.len())
            > MAX_PAYLOADS
    {
        let message = if record.family == CompilerFamily::Llvm {
            "source lock must declare a supported LLVM version and 1..1024 payloads"
        } else {
            "source lock must declare a supported GNU version and 1..1024 payloads"
        };
        return Err(ContractError::invalid(message));
    }
    let mut filenames = BTreeSet::new();
    let mut components = BTreeSet::new();
    let mut component_versions = BTreeMap::new();
    let mut patches = BTreeSet::new();
    let mut total = 0_u64;
    for source in &record.sources {
        if !token(&source.component)
            || !token(&source.version)
            || !portable_filename(&source.filename)
            || !https_url(&source.url)
            || source.size == 0
            || !components.insert((&source.component, &source.version))
            || !filenames.insert(source.filename.clone())
        {
            return Err(ContractError::invalid(
                "source lock contains duplicate or invalid source declarations",
            ));
        }
        if record.schema == Schema::V3
            && component_versions
                .insert(source.component.as_str(), source.version.as_str())
                .is_some_and(|version| version != source.version)
        {
            return Err(ContractError::invalid(
                "source lock declares conflicting versions for one component",
            ));
        }
        if let Some(patch) = &source.patch {
            let patch_namespace = match record.family {
                CompilerFamily::Llvm => "tools/crosstools/llvm/",
                CompilerFamily::Gnu => "tools/crosstools/gnu/",
            };
            let source_owned_dependency = record.schema == Schema::V3
                && source.purpose == SourcePurpose::TargetBuildDependency;
            if !safe_relative_path(patch)
                || (!source_owned_dependency && !patch.starts_with(patch_namespace))
                || !patch.ends_with("-aros.diff")
                || !patches.insert(patch)
            {
                let message = if record.schema == Schema::V2 {
                    "source lock contains an unsafe or duplicate LLVM patch path"
                } else {
                    "source lock contains an unsafe or duplicate compiler-family patch path"
                };
                return Err(ContractError::invalid(message));
            }
        }
        total = total.checked_add(source.size).ok_or_else(|| {
            ContractError::invalid("source-lock payload sizes overflow the supported limit")
        })?;
        let _ = &source.purpose;
    }
    if record.schema == Schema::V3 && record.family == CompilerFamily::Gnu {
        validate_gnu_components(record)?;
    }
    validate_host_python_packages(
        &record.host_python_packages,
        &mut filenames,
        &mut total,
        true,
    )?;
    Ok(())
}

fn validate_host_python_packages(
    packages: &[HostPythonPackage],
    filenames: &mut BTreeSet<String>,
    total: &mut u64,
    source_lock: bool,
) -> Result<(), ContractError> {
    let mut names = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for package in packages {
        if !token(&package.name)
            || !token(&package.version)
            || !portable_filename(&package.filename)
            || !https_url(&package.url)
            || package.size == 0
            || !single_segment(&package.source_root)
            || !safe_python_path(&package.python_path)
            || !names.insert(package.name.as_str())
            || !roots.insert(package.source_root.as_str())
            || !filenames.insert(package.filename.clone())
        {
            let message = if source_lock {
                "source lock contains duplicate or invalid host Python package declarations"
            } else {
                "host Python lock contains duplicate or invalid package declarations"
            };
            return Err(ContractError::invalid(message));
        }
        *total = total.checked_add(package.size).ok_or_else(|| {
            let message = if source_lock {
                "source-lock payload sizes overflow the supported limit"
            } else {
                "host Python payload sizes overflow the supported limit"
            };
            ContractError::invalid(message)
        })?;
    }
    if *total > MAX_TOTAL_BYTES {
        let message = if source_lock {
            "source-lock payload closure exceeds the 8 GiB safety limit"
        } else {
            "host Python payload closure exceeds the 8 GiB safety limit"
        };
        return Err(ContractError::invalid(message));
    }
    Ok(())
}

fn validate_gnu_components(record: &Record) -> Result<(), ContractError> {
    let gcc = record
        .sources
        .iter()
        .filter(|source| {
            source.component == "gcc" && source.purpose == SourcePurpose::ToolchainComponent
        })
        .collect::<Vec<_>>();
    let binutils = record
        .sources
        .iter()
        .filter(|source| {
            source.component == "binutils" && source.purpose == SourcePurpose::ToolchainComponent
        })
        .collect::<Vec<_>>();
    if gcc.len() != 1
        || gcc[0].version != record.version.as_str()
        || binutils.len() != 1
        || validate_gnu_compiler_versions(&gcc[0].version, &binutils[0].version).is_err()
    {
        return Err(ContractError::invalid(
            "GNU source lock requires one matching GCC and one supported binutils toolchain component",
        ));
    }
    Ok(())
}

fn digest<'de, D>(deserializer: D) -> Result<Sha256Digest, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "expected a lowercase SHA-256 digest",
        ));
    }
    Sha256Digest::parse(&value).map_err(serde::de::Error::custom)
}

fn llvm_version(value: &str) -> bool {
    numeric_version(value, 3, 3)
}

fn gcc_version(value: &str) -> bool {
    numeric_version(value, 3, 3)
}

fn numeric_version(value: &str, min_segments: usize, max_segments: usize) -> bool {
    let segments = value.split('.').collect::<Vec<_>>();
    (min_segments..=max_segments).contains(&segments.len())
        && segments
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+._-".contains(&byte))
}

fn portable_filename(value: &str) -> bool {
    token(value) && value != "." && value != ".."
}

fn https_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn single_segment(value: &str) -> bool {
    token(value) && value != "." && value != ".."
}

fn safe_python_path(value: &str) -> bool {
    value == "."
        || (!value.is_empty()
            && !value.starts_with('/')
            && value
                .split('/')
                .all(|part| token(part) && part != "." && part != ".."))
}

#[cfg(test)]
mod host_python_lock_tests {
    use aros_common::sha256_bytes;
    use serde_json::{json, Value};

    use super::HostPythonLock;

    fn lock() -> Value {
        json!({
            "schema": "aros-host-python-lock-v1",
            "packages": [
                {
                    "name": "mako", "version": "1.3.10", "filename": "mako-1.3.10.tar.gz",
                    "url": "https://example.invalid/mako.tar.gz", "sha256": "a".repeat(64),
                    "size": 7, "source_root": "mako-1.3.10", "python_path": "."
                },
                {
                    "name": "markupsafe", "version": "3.0.2", "filename": "markupsafe-3.0.2.tar.gz",
                    "url": "https://example.invalid/markupsafe.tar.gz", "sha256": "b".repeat(64),
                    "size": 8, "source_root": "markupsafe-3.0.2", "python_path": "src"
                }
            ]
        })
    }

    fn bytes(document: &Value) -> Vec<u8> {
        serde_json::to_vec(document).unwrap()
    }

    #[test]
    fn parses_closed_standalone_lock_and_hashes_exact_bytes() {
        let mut input = bytes(&lock());
        input.push(b' ');
        let parsed = HostPythonLock::parse(&input).unwrap();

        assert_eq!(parsed.sha256(), &sha256_bytes(&input));
        assert_eq!(parsed.packages().len(), 2);
        assert_eq!(parsed.packages()[0].name(), "mako");
        assert_eq!(parsed.packages()[1].source_root(), "markupsafe-3.0.2");
        assert_eq!(parsed.packages()[1].python_path(), "src");
    }

    #[test]
    fn rejects_unknown_schema_fields_and_oversized_documents() {
        let mut unknown_schema = lock();
        unknown_schema["schema"] = json!("aros-host-python-lock-v2");
        assert!(HostPythonLock::parse(&bytes(&unknown_schema)).is_err());

        let mut unknown_field = lock();
        unknown_field["extra"] = json!(true);
        assert!(HostPythonLock::parse(&bytes(&unknown_field)).is_err());

        let mut unknown_package_field = lock();
        unknown_package_field["packages"][0]["extra"] = json!(true);
        assert!(HostPythonLock::parse(&bytes(&unknown_package_field)).is_err());

        assert!(
            HostPythonLock::parse(&vec![b' '; crate::canonical::MAX_DOCUMENT_BYTES + 1]).is_err()
        );
    }

    #[test]
    fn rejects_duplicate_package_identities_and_unsafe_paths() {
        let mut duplicate_name = lock();
        duplicate_name["packages"][1]["name"] = json!("mako");
        assert!(HostPythonLock::parse(&bytes(&duplicate_name)).is_err());

        let mut duplicate_root = lock();
        duplicate_root["packages"][1]["source_root"] = json!("mako-1.3.10");
        assert!(HostPythonLock::parse(&bytes(&duplicate_root)).is_err());

        let mut duplicate_filename = lock();
        duplicate_filename["packages"][1]["filename"] = json!("mako-1.3.10.tar.gz");
        assert!(HostPythonLock::parse(&bytes(&duplicate_filename)).is_err());

        for path in ["../outside", "/absolute", "safe/../../outside"] {
            let mut unsafe_root = lock();
            unsafe_root["packages"][0]["source_root"] = json!(path);
            assert!(HostPythonLock::parse(&bytes(&unsafe_root)).is_err());

            let mut unsafe_python_path = lock();
            unsafe_python_path["packages"][0]["python_path"] = json!(path);
            assert!(HostPythonLock::parse(&bytes(&unsafe_python_path)).is_err());
        }
    }

    #[test]
    fn rejects_invalid_package_payload_metadata_and_total_size() {
        let mut invalid = lock();
        invalid["packages"][0]["version"] = json!(" ");
        assert!(HostPythonLock::parse(&bytes(&invalid)).is_err());

        let mut invalid = lock();
        invalid["packages"][0]["url"] = json!("http://example.invalid/mako.tar.gz");
        assert!(HostPythonLock::parse(&bytes(&invalid)).is_err());

        let mut invalid = lock();
        invalid["packages"][0]["size"] = json!(0);
        assert!(HostPythonLock::parse(&bytes(&invalid)).is_err());

        let mut invalid = lock();
        invalid["packages"][0]["sha256"] = json!("A".repeat(64));
        assert!(HostPythonLock::parse(&bytes(&invalid)).is_err());

        let mut over_limit = lock();
        over_limit["packages"][0]["size"] = json!(8_u64 * 1024 * 1024 * 1024);
        over_limit["packages"][1]["size"] = json!(1);
        assert!(HostPythonLock::parse(&bytes(&over_limit)).is_err());
    }

    #[test]
    fn rejects_empty_and_overpopulated_package_lists() {
        let mut empty = lock();
        empty["packages"] = json!([]);
        assert!(HostPythonLock::parse(&bytes(&empty)).is_err());

        let mut too_many = lock();
        let mut packages = Vec::new();
        for index in 0..=1024 {
            let mut package = too_many["packages"][0].clone();
            package["name"] = json!(format!("package{index}"));
            package["filename"] = json!(format!("package{index}.tar.gz"));
            package["source_root"] = json!(format!("package{index}"));
            packages.push(package);
        }
        too_many["packages"] = json!(packages);
        assert!(HostPythonLock::parse(&bytes(&too_many)).is_err());
    }

    #[test]
    fn rejects_non_object_package_records() {
        let mut non_object_package: Value = lock();
        non_object_package["packages"][0] = json!("mako");
        assert!(HostPythonLock::parse(&bytes(&non_object_package)).is_err());
    }
}
