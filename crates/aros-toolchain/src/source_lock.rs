//! Closed, source-owned input declarations for the native toolchain producer.
//!
//! A source lock is not an instruction to download or execute anything.  It
//! describes the payload closure that later M2/M3 phases must acquire and
//! revalidate through `aros-fetch`.  Keeping this parser here prevents a
//! second, drift-prone lock interpretation in the CLI or workflow.

use std::collections::BTreeSet;

use aros_common::Sha256Digest;
use serde::{Deserialize, Deserializer};
use url::Url;

use crate::recipe::safe_relative_path;
use crate::{ContractError, Recipe};

const MAX_PAYLOADS: usize = 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// One semantically valid source-lock v2 document.
#[derive(Debug, Clone)]
pub struct SourceLock(Record);

/// A declared archive whose exact bytes must be present in a verified cache.
#[derive(Debug, Clone, Copy)]
pub struct Payload<'a> {
    filename: &'a str,
    url: &'a str,
    sha256: &'a Sha256Digest,
    size: u64,
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

impl SourceLock {
    /// Parse and close the source-lock v2 schema without accessing the network
    /// or the filesystem.
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
        let record: Record = serde_json::from_slice(input)
            .map_err(|_| ContractError::invalid("invalid source-lock-v2 document"))?;
        validate(&record)?;
        Ok(Self(record))
    }

    /// LLVM family version selected by this lock.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.0.version
    }

    /// Every source archive, in the source-owned declaration order.
    #[must_use]
    pub fn sources(&self) -> impl ExactSizeIterator<Item = Payload<'_>> {
        self.0.sources.iter().map(|source| source.payload())
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
    family: Family,
    version: String,
    sources: Vec<Source>,
    host_python_packages: Vec<HostPythonPackage>,
}

#[derive(Debug, Clone, Deserialize)]
enum Schema {
    #[serde(rename = "aros-toolchain-source-lock-v2")]
    V2,
}

#[derive(Debug, Clone, Deserialize)]
enum Family {
    #[serde(rename = "llvm")]
    Llvm,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Purpose {
    ToolchainComponent,
    TargetBuildDependency,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    component: String,
    version: String,
    purpose: Purpose,
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
    let _ = (&record.schema, &record.family);
    if !llvm_version(&record.version)
        || record.sources.is_empty()
        || record.host_python_packages.is_empty()
        || record
            .sources
            .len()
            .saturating_add(record.host_python_packages.len())
            > MAX_PAYLOADS
    {
        return Err(ContractError::invalid(
            "source lock must declare a supported LLVM version and 1..1024 payloads",
        ));
    }
    let mut filenames = BTreeSet::new();
    let mut components = BTreeSet::new();
    let mut patches = BTreeSet::new();
    let mut total = 0_u64;
    for source in &record.sources {
        if !token(&source.component)
            || !token(&source.version)
            || !portable_filename(&source.filename)
            || !https_url(&source.url)
            || source.size == 0
            || !components.insert((&source.component, &source.version))
            || !filenames.insert(&source.filename)
        {
            return Err(ContractError::invalid(
                "source lock contains duplicate or invalid source declarations",
            ));
        }
        if let Some(patch) = &source.patch {
            if !safe_relative_path(patch)
                || !patch.starts_with("tools/crosstools/llvm/")
                || !patch.ends_with("-aros.diff")
                || !patches.insert(patch)
            {
                return Err(ContractError::invalid(
                    "source lock contains an unsafe or duplicate LLVM patch path",
                ));
            }
        }
        total = total.checked_add(source.size).ok_or_else(|| {
            ContractError::invalid("source-lock payload sizes overflow the supported limit")
        })?;
        let _ = &source.purpose;
    }
    let mut names = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for package in &record.host_python_packages {
        if !token(&package.name)
            || !token(&package.version)
            || !portable_filename(&package.filename)
            || !https_url(&package.url)
            || package.size == 0
            || !single_segment(&package.source_root)
            || !safe_python_path(&package.python_path)
            || !names.insert(&package.name)
            || !roots.insert(&package.source_root)
            || !filenames.insert(&package.filename)
        {
            return Err(ContractError::invalid(
                "source lock contains duplicate or invalid host Python package declarations",
            ));
        }
        total = total.checked_add(package.size).ok_or_else(|| {
            ContractError::invalid("source-lock payload sizes overflow the supported limit")
        })?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(ContractError::invalid(
            "source-lock payload closure exceeds the 8 GiB safety limit",
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
    value.split('.').count() == 3
        && value
            .split('.')
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
