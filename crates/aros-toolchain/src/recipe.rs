//! Closed recipe-v2 parsing; self-consistency is not source/origin verification.

use aros_common::digest::{sha256_bytes, Sha256Digest};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{canonical, ContractError};

/// A lowercase SHA-1 Git object name from the current producer contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GitObjectId(String);

impl GitObjectId {
    /// Return the validated spelling, without implying the object exists.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GitObjectId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 40 && lowercase_hex(&value) {
            Ok(Self(value))
        } else {
            Err("expected a lowercase 40-hex Git object identity")
        }
    }
}

impl From<GitObjectId> for String {
    fn from(value: GitObjectId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Schema {
    #[serde(rename = "aros-toolchain-recipe-v2")]
    V2,
}

/// One declared patch identity, not a request to apply a patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    path: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
}

impl Patch {
    /// Canonical relative source path; verified by [`Recipe::parse`].
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Declared SHA-256; the selected source must still be hashed separately.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

// Deserialize the closed structure directly, never via Value: serde's
// duplicate-field rejection must see the original JSON, including patches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: Schema,
    source_commit: GitObjectId,
    source_tree: GitObjectId,
    producer_commit: GitObjectId,
    producer_tree: GitObjectId,
    tools_commit: GitObjectId,
    tools_tree: GitObjectId,
    source_date_epoch: u64,
    #[serde(deserialize_with = "digest")]
    source_lock_sha256: Sha256Digest,
    #[serde(deserialize_with = "digest")]
    profiles_sha256: Sha256Digest,
    patches: Vec<Patch>,
    #[serde(deserialize_with = "digest")]
    recipe_sha256: Sha256Digest,
}

/// A syntactically valid, self-consistent recipe, created only by validation.
///
/// This wrapper intentionally does not implement `Deserialize`: callers may
/// not accidentally bypass semantic/path/digest checks through a generic parser.
#[derive(Debug, Clone)]
pub struct Recipe(Record);

impl Recipe {
    /// Parse one bounded UTF-8 document and verify its complete self-digest.
    ///
    /// Performs no filesystem, process, environment or transport operations.
    /// Does not validate Git objects, source-lock contents, actual patches,
    /// executor evidence or eligibility for a build/release.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for invalid structure, unsupported versions or unsafe
    /// patch paths; AX0102 for a mismatching self-digest. Unknown/duplicate
    /// fields, noncanonical identities and bool/float integer coercion fail.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("producer recipe exceeds 1 MiB"));
        }
        let record: Record = serde_json::from_slice(input).map_err(|error| {
            // serde's prose can contain arbitrary input values. Preserve the
            // precise location without echoing a potentially private document.
            ContractError::invalid(format!(
                "invalid recipe-v2 document: {} at line {}, column {}",
                safe_parse_reason(&error),
                error.line(),
                error.column()
            ))
        })?;
        let mut previous: Option<&str> = None;
        for patch in &record.patches {
            if !safe_relative_path(&patch.path) {
                return Err(ContractError::invalid(
                    "recipe patch path is not a canonical relative path",
                ));
            }
            if previous.is_some_and(|path| path >= patch.path.as_str()) {
                return Err(ContractError::invalid(
                    "recipe patch paths must be sorted and unique",
                ));
            }
            previous = Some(&patch.path);
        }
        let mut material = serde_json::to_value(&record)
            .map_err(|_| ContractError::invalid("cannot encode validated recipe fields"))?;
        material
            .as_object_mut()
            .ok_or_else(|| ContractError::invalid("recipe material is not an object"))?
            .remove("recipe_sha256");
        let actual = sha256_bytes(&canonical::bytes(&material)?);
        if actual != record.recipe_sha256 {
            return Err(ContractError::identity(format!(
                "recipe self-digest mismatch: expected {}, measured {actual}",
                record.recipe_sha256
            )));
        }
        Ok(Self(record))
    }

    /// Recipe-declared source commit and tree; not a verified checkout.
    #[must_use]
    pub const fn source(&self) -> (&GitObjectId, &GitObjectId) {
        (&self.0.source_commit, &self.0.source_tree)
    }

    /// Recipe-declared workflow/recipe repository commit and tree.
    #[must_use]
    pub const fn producer(&self) -> (&GitObjectId, &GitObjectId) {
        (&self.0.producer_commit, &self.0.producer_tree)
    }

    /// Recipe-declared tools commit and tree, not the running executor's origin.
    #[must_use]
    pub const fn tools(&self) -> (&GitObjectId, &GitObjectId) {
        (&self.0.tools_commit, &self.0.tools_tree)
    }

    /// Verified self-digest, not an attestation.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.0.recipe_sha256
    }

    /// Declared source-lock file digest; contents are checked in a later phase.
    #[must_use]
    pub const fn source_lock_sha256(&self) -> &Sha256Digest {
        &self.0.source_lock_sha256
    }

    /// Declared profiles file digest; no profile recipe is duplicated here.
    #[must_use]
    pub const fn profiles_sha256(&self) -> &Sha256Digest {
        &self.0.profiles_sha256
    }

    /// Declared reproducibility epoch; its derivation requires source auditing.
    #[must_use]
    pub const fn source_date_epoch(&self) -> u64 {
        self.0.source_date_epoch
    }

    /// Sorted, unique patch declarations; no patch is applied by this library.
    #[must_use]
    pub fn patches(&self) -> &[Patch] {
        &self.0.patches
    }
}

fn lowercase_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_parse_reason(error: &serde_json::Error) -> String {
    let detail = error.to_string();
    let position = format!(" at line {} column {}", error.line(), error.column());
    let detail = detail.strip_suffix(&position).unwrap_or(&detail);
    // Only names from the closed schema are permitted in rendered context.
    // Never forward serde's invalid values or arbitrary unknown field names.
    for field in [
        "schema",
        "source_commit",
        "source_tree",
        "producer_commit",
        "producer_tree",
        "tools_commit",
        "tools_tree",
        "source_date_epoch",
        "source_lock_sha256",
        "profiles_sha256",
        "patches",
        "recipe_sha256",
        "path",
        "sha256",
    ] {
        for problem in ["missing", "duplicate"] {
            let expected = format!("{problem} field `{field}`");
            if detail.starts_with(&expected) {
                return expected;
            }
        }
    }
    let reason = if detail.starts_with("unknown field") {
        "unknown field; this recipe and its patch records have a closed schema"
    } else if detail.ends_with(", expected `aros-toolchain-recipe-v2`") {
        "unsupported schema; expected aros-toolchain-recipe-v2"
    } else if detail.starts_with("expected a lowercase 40-hex Git object identity") {
        "expected a lowercase 40-hex Git object identity"
    } else if detail.starts_with("expected a lowercase SHA-256 digest")
        || detail.starts_with("expected a 64-character hexadecimal SHA-256 digest")
    {
        "expected a lowercase 64-hex SHA-256 digest"
    } else if detail.ends_with(", expected u64") || detail.starts_with("number out of range") {
        "expected an unsigned 64-bit source_date_epoch integer, not a boolean, float or string"
    } else {
        match error.classify() {
            serde_json::error::Category::Eof => "truncated JSON document",
            serde_json::error::Category::Syntax => "invalid JSON syntax, encoding or trailing data",
            serde_json::error::Category::Data => "invalid field type; check the recipe-v2 schema",
            serde_json::error::Category::Io => "cannot read recipe JSON",
        }
    };
    reason.to_owned()
}

fn digest<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Sha256Digest, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !lowercase_hex(&value) {
        return Err(serde::de::Error::custom(
            "expected a lowercase SHA-256 digest",
        ));
    }
    Sha256Digest::parse(&value).map_err(serde::de::Error::custom)
}

pub(crate) fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch == '\\' || ch == ':')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
