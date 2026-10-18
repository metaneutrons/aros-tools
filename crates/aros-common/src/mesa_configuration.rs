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

/// Compiler feature variables read by the reviewed Mesa compilation recipes.
///
/// Every entry is required, even when its measured value is empty. Values are
/// compiler results, not defaults inferred from the toolchain family.
pub const MESA_COMPILER_OPTION_VARIABLES: &[&str] = &[
    "CFLAGS_GNU11",
    "CXXFLAGS_CXX17",
    "CXXFLAGS_LIFETIME_DSE1",
    "CFLAGS_NO_STRICT_ALIASING",
    "CFLAGS_NO_MATH_ERRNO",
    "CFLAGS_NO_TRAPPING_MATH",
    "CFLAGS_VISIBILITY_HIDDEN",
    "CFLAGS_MERGE_CONSTANTS",
    "CFLAGS_FUNCTION_SECTIONS",
    "NOWARN_UNINITIALIZED",
    "NOWARN_STRICT_ALIASING",
    "NOWARN_MAYBE_UNINITIALIZED",
    "NOWARN_MISSING_FIELD_INITIALIZERS",
    "NOWARN_FORMAT_TRUNCATION",
    "NOWARN_RETURN_TYPE",
    "NOWARN_UNUSED_CONST_VARIABLE",
    "NOWARN_ARRAY_BOUNDS",
];

/// Required configure-measured baseline variables transported to Make.
///
/// Each value is present even when its measured atom list is empty. This is a
/// closed transport contract, not a defaulting rule or proof of probe
/// provenance.
pub const MESA_BASELINE_VARIABLES: &[&str] = &[
    "TARGET_ISA_CXXFLAGS",
    "CONFIG_CFLAGS",
    "CONFIG_CXXFLAGS",
    "CONFIG_CPPFLAGS",
    "TARGET_CFLAGS",
    "TARGET_CXXFLAGS",
    "TARGET_CPPFLAGS",
    "DEBUG_CFLAGS",
    "DEBUG_CXXFLAGS",
    "OPTIMIZATION_CFLAGS",
    "LLVM_NOWARN",
];

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
    baseline_variables: BTreeMap<String, Vec<String>>,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
    compiler_options: BTreeMap<String, Vec<String>>,
    mesa_compile_options: Vec<String>,
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
    /// identities, identity mismatches, unsafe baseline/ISA/compiler-option/
    /// bundle atoms, and selector values outside the closed schema.
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
        validate_baseline_variables(&wire.baseline_variables)?;
        validate_isa_flags(&wire.target_isa_flags)?;
        validate_compiler_options(&wire.compiler_options)?;
        validate_isa_flags(&wire.mesa_compile_options)?;

        Ok(Self {
            identity: wire.identity,
            baseline_variables: wire.baseline_variables,
            target_isa_flags: wire.target_isa_flags,
            builtin_selectors: wire.builtin_selectors,
            compiler_options: wire.compiler_options,
            mesa_compile_options: wire.mesa_compile_options,
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

    /// Project the complete validated configuration to strict Make values.
    ///
    /// True selectors produce their fixed `-fno-builtin-*` option; false
    /// selectors produce the known empty value. Baseline and compiler-option
    /// atoms preserve their input order and join with one ASCII space;
    /// `TARGET_ISA_CFLAGS` follows the same rule.
    #[must_use]
    pub fn make_variables(&self) -> BTreeMap<String, String> {
        let selectors = &self.builtin_selectors;
        let mut variables = BTreeMap::from([
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
        ]);
        variables.extend(
            self.baseline_variables
                .iter()
                .map(|(name, atoms)| (name.clone(), atoms.join(" "))),
        );
        variables.extend(
            self.compiler_options
                .iter()
                .map(|(name, atoms)| (name.clone(), atoms.join(" "))),
        );
        variables.insert(
            "MESA_COMPILEOPTIONS".into(),
            self.mesa_compile_options.join(" "),
        );
        variables
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MesaConfigurationWire {
    version: u32,
    identity: MesaConfigurationIdentity,
    #[serde(deserialize_with = "deserialize_baseline_variables")]
    baseline_variables: BTreeMap<String, Vec<String>>,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
    #[serde(deserialize_with = "deserialize_compiler_options")]
    compiler_options: BTreeMap<String, Vec<String>>,
    mesa_compile_options: Vec<String>,
}

fn deserialize_baseline_variables<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct UniqueBaselineVariables;
    impl<'de> serde::de::Visitor<'de> for UniqueBaselineVariables {
        type Value = BTreeMap<String, Vec<String>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a baseline-variable object with unique variable names")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((name, atoms)) = map.next_entry::<String, Vec<String>>()? {
                if values.insert(name.clone(), atoms).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate baseline variable {name}"
                    )));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(UniqueBaselineVariables)
}

fn validate_baseline_variables(variables: &BTreeMap<String, Vec<String>>) -> Result<()> {
    if variables.len() != MESA_BASELINE_VARIABLES.len()
        || MESA_BASELINE_VARIABLES
            .iter()
            .any(|name| !variables.contains_key(*name))
    {
        return Err(configuration_error(
            "baseline_variables must contain every required Mesa baseline variable and no others",
        ));
    }
    for (name, atoms) in variables {
        validate_flag_atoms(name, atoms)?;
    }
    Ok(())
}

fn deserialize_compiler_options<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct UniqueOptions;
    impl<'de> serde::de::Visitor<'de> for UniqueOptions {
        type Value = BTreeMap<String, Vec<String>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a compiler-option object with unique variable names")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((name, atoms)) = map.next_entry::<String, Vec<String>>()? {
                if values.insert(name.clone(), atoms).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate compiler-option variable {name}"
                    )));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(UniqueOptions)
}

fn validate_compiler_options(options: &BTreeMap<String, Vec<String>>) -> Result<()> {
    if options.len() != MESA_COMPILER_OPTION_VARIABLES.len()
        || MESA_COMPILER_OPTION_VARIABLES
            .iter()
            .any(|name| !options.contains_key(*name))
    {
        return Err(configuration_error(
            "compiler_options must contain every reviewed Mesa compiler variable and no others",
        ));
    }
    for (name, atoms) in options {
        validate_isa_flags(atoms).map_err(|error| {
            configuration_error(format!("invalid compiler_options variable {name}: {error}"))
        })?;
    }
    Ok(())
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
    validate_flag_atoms("target_isa_flags", flags)
}

fn validate_flag_atoms(name: &str, flags: &[String]) -> Result<()> {
    if flags.len() > MAX_TARGET_ISA_FLAGS {
        return Err(configuration_error(format!(
            "{name} exceeds its {MAX_TARGET_ISA_FLAGS}-atom limit"
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
                "{name} atom {index} is empty, too long, or contains unsafe syntax"
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
