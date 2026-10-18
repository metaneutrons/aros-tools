//! Bounded, data-only transport for measured Mesa configure results.
//!
//! This format binds serialized configuration values to a caller-supplied
//! identity. It does not establish who produced the values, prove that a probe
//! ran, or prove parity with another build system.

use crate::{ArosCompilerIdentity, ArosError, Result, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CONFIGURATION_FILE: &str = "mesa-configuration.json";
const MAX_CONFIGURATION_BYTES: usize = 32 * 1024;
const MAX_TARGET_ISA_FLAGS: usize = 64;
const MAX_TARGET_ISA_FLAG_BYTES: usize = 256;

/// Caller-measured identity for one configured Mesa target.
///
/// `source_inputs_sha256` identifies the source and configuration closure
/// measured by the caller. `configure_invocation_sha256` identifies the full
/// configure invocation, including feature options and the selected compiler
/// and sysroot, as measured by the caller. Neither digest is independently
/// measured or authenticated by this data structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaConfigurationIdentity {
    pub target_profile: String,
    pub target_triple: String,
    pub compiler: ArosCompilerIdentity,
    pub compiler_tree_sha256: Sha256Digest,
    pub source_inputs_sha256: Sha256Digest,
    pub configure_invocation_sha256: Sha256Digest,
}

impl MesaConfigurationIdentity {
    fn validate(&self) -> Result<()> {
        validate_identifier("target_profile", &self.target_profile, 64)?;
        validate_identifier("target_triple", &self.target_triple, 128)?;
        self.compiler
            .validate_for_target(&self.target_triple)
            .map_err(|message| configuration_error(format!("invalid compiler identity: {message}")))
    }
}

/// A validated Mesa configuration bound to the caller's expected identity.
///
/// Values are private so callers can only obtain this type through
/// [`Self::parse_bound`]. The identity comparison is a data binding, not
/// producer authentication, a compiler probe, or build-system parity proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMesaConfiguration {
    identity: MesaConfigurationIdentity,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
}

impl ValidatedMesaConfiguration {
    /// Parse bounded JSON and bind it to a fresh identity measured by the caller.
    ///
    /// The expected identity must describe the current source/configuration
    /// closure and complete configure invocation. Equality checks every field,
    /// including compiler version, compiler tree, source inputs, and invocation
    /// digests. A successful parse establishes only that the transported data
    /// matches that supplied identity; it does not authenticate a producer,
    /// prove a probe ran, or establish Make/CMake parity.
    ///
    /// # Errors
    /// Rejects oversized or malformed JSON, unsupported versions, invalid
    /// identities, identity mismatches, unsafe ISA flag atoms, and selector
    /// values outside the closed schema.
    pub fn parse_bound(bytes: &[u8], expected: &MesaConfigurationIdentity) -> Result<Self> {
        if bytes.len() > MAX_CONFIGURATION_BYTES {
            return Err(configuration_error(format!(
                "input exceeds the {MAX_CONFIGURATION_BYTES}-byte limit"
            )));
        }
        expected.validate()?;

        let wire: MesaConfigurationWire = serde_json::from_slice(bytes)
            .map_err(|error| configuration_error(format!("invalid configuration JSON: {error}")))?;
        if wire.version != 1 {
            return Err(configuration_error(format!(
                "unsupported Mesa configuration version {}",
                wire.version
            )));
        }
        wire.identity.validate()?;
        if wire.identity != *expected {
            return Err(configuration_error(
                "configuration identity does not match the caller-provided expected identity",
            ));
        }
        validate_isa_flags(&wire.target_isa_flags)?;

        Ok(Self {
            identity: wire.identity,
            target_isa_flags: wire.target_isa_flags,
            builtin_selectors: wire.builtin_selectors,
        })
    }

    /// Return the identity checked against the caller's expected identity.
    #[must_use]
    pub const fn identity(&self) -> &MesaConfigurationIdentity {
        &self.identity
    }

    /// Return ISA flag atoms in their original order, including duplicates.
    #[must_use]
    pub fn target_isa_flags(&self) -> &[String] {
        &self.target_isa_flags
    }

    /// Map every recognized selector to its strict Make value.
    ///
    /// True selectors produce their fixed `-fno-builtin-*` option; false
    /// selectors produce the known empty value. `TARGET_ISA_CFLAGS` preserves
    /// the input atom order and joins atoms with one ASCII space.
    #[must_use]
    pub fn make_variables(&self) -> BTreeMap<String, String> {
        let selectors = &self.builtin_selectors;
        BTreeMap::from([
            (
                "CFLAGS_NO_BUILTIN_FFS".into(),
                selector_value(selectors.ffs, "-fno-builtin-ffs"),
            ),
            (
                "CFLAGS_NO_BUILTIN_FFSLL".into(),
                selector_value(selectors.ffsll, "-fno-builtin-ffsll"),
            ),
            (
                "CFLAGS_NO_BUILTIN_BSWAP32".into(),
                selector_value(selectors.bswap32, "-fno-builtin-bswap32"),
            ),
            (
                "CFLAGS_NO_BUILTIN_BSWAP64".into(),
                selector_value(selectors.bswap64, "-fno-builtin-bswap64"),
            ),
            (
                "CFLAGS_NO_BUILTIN_CLZ".into(),
                selector_value(selectors.clz, "-fno-builtin-clz"),
            ),
            (
                "CFLAGS_NO_BUILTIN_CLZLL".into(),
                selector_value(selectors.clzll, "-fno-builtin-clzll"),
            ),
            (
                "CFLAGS_NO_BUILTIN_CTZ".into(),
                selector_value(selectors.ctz, "-fno-builtin-ctz"),
            ),
            (
                "CFLAGS_NO_BUILTIN_EXPECT".into(),
                selector_value(selectors.expect, "-fno-builtin-expect"),
            ),
            (
                "CFLAGS_NO_BUILTIN_POPCOUNT".into(),
                selector_value(selectors.popcount, "-fno-builtin-popcount"),
            ),
            (
                "CFLAGS_NO_BUILTIN_POPCOUNTLL".into(),
                selector_value(selectors.popcountll, "-fno-builtin-popcountll"),
            ),
            (
                "CFLAGS_NO_BUILTIN_UNREACHABLE".into(),
                selector_value(selectors.unreachable, "-fno-builtin-unreachable"),
            ),
            ("TARGET_ISA_CFLAGS".into(), self.target_isa_flags.join(" ")),
        ])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MesaConfigurationWire {
    version: u32,
    identity: MesaConfigurationIdentity,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
enum BuiltinProbeResult {
    Accepted,
    Rejected,
}

impl From<bool> for BuiltinProbeResult {
    fn from(accepted: bool) -> Self {
        if accepted {
            Self::Accepted
        } else {
            Self::Rejected
        }
    }
}

impl From<BuiltinProbeResult> for bool {
    fn from(result: BuiltinProbeResult) -> Self {
        result == BuiltinProbeResult::Accepted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuiltinSelectors {
    #[serde(rename = "CFLAGS_NO_BUILTIN_FFS")]
    ffs: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_FFSLL")]
    ffsll: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_BSWAP32")]
    bswap32: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_BSWAP64")]
    bswap64: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_CLZ")]
    clz: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_CLZLL")]
    clzll: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_CTZ")]
    ctz: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_EXPECT")]
    expect: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_POPCOUNT")]
    popcount: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_POPCOUNTLL")]
    popcountll: BuiltinProbeResult,
    #[serde(rename = "CFLAGS_NO_BUILTIN_UNREACHABLE")]
    unreachable: BuiltinProbeResult,
}

fn validate_identifier(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return Err(configuration_error(format!(
            "{field} must be nonempty ASCII identifier text of at most {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn validate_isa_flags(flags: &[String]) -> Result<()> {
    if flags.len() > MAX_TARGET_ISA_FLAGS {
        return Err(configuration_error(format!(
            "target_isa_flags exceeds its {MAX_TARGET_ISA_FLAGS}-atom limit"
        )));
    }
    for (index, flag) in flags.iter().enumerate() {
        if flag.is_empty()
            || flag.len() > MAX_TARGET_ISA_FLAG_BYTES
            || !flag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.,:/=+-".contains(&byte))
        {
            return Err(configuration_error(format!(
                "target_isa_flags atom {index} is empty, too long, or contains unsafe syntax"
            )));
        }
    }
    Ok(())
}

fn selector_value(result: BuiltinProbeResult, option: &str) -> String {
    if result == BuiltinProbeResult::Accepted {
        option.into()
    } else {
        String::new()
    }
}

fn configuration_error(message: impl Into<String>) -> ArosError {
    ArosError::Configuration {
        file: CONFIGURATION_FILE.into(),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "mesa_configuration_tests.rs"]
mod tests;
