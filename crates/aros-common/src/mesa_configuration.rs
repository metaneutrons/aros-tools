//! Bounded, data-only transport for measured Mesa configure results.
//!
//! This format binds serialized configuration values to a caller-supplied
//! identity. It does not establish who produced the values, prove that a probe
//! ran, or prove parity with another build system.

use crate::make_initial_values::{MakeInitialValuesObservation, ValidatedMakeInitialValues};
use crate::make_namespace::{MakeNamespaceObservation, ValidatedMakeNamespace};
use crate::{ArosCompilerIdentity, ArosError, Result, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[path = "mesa_configuration_llvm.rs"]
mod mesa_configuration_llvm;
pub use mesa_configuration_llvm::{MesaLlvmConfiguration, MesaMakeVersionValue};
#[path = "mesa_configuration_recipe.rs"]
mod mesa_configuration_recipe;
pub use mesa_configuration_recipe::{
    MesaMakeCoordinates, MesaRecipeConfiguration, MesaRecipeFlagValue, MesaTargetLto,
};

const CONFIGURATION_FILE: &str = "mesa-configuration.json";
const MAX_CONFIGURATION_BYTES: usize = 32 * 1024;
const MAX_NAMESPACE_CONFIGURATION_BYTES: usize = 512 * 1024;
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
    "NOWARN_UNUSED_VARIABLE",
    "NOWARN_UNUSED_VALUE",
    "NOWARN_UNUSED_FUNCTION",
    "NOWARN_MISLEADING_INDENTATION",
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

/// Mesa variables that the configure preflight must establish are globally undefined.
///
/// This is an explicit scope assertion. It is not producer authentication.
pub const MESA_UNDEFINED_SCOPE_VARIABLES: &[&str] = &["CUR_MESADIR"];

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
    initial_namespace: Option<ValidatedMakeNamespace>,
    initial_values: Option<ValidatedMakeInitialValues>,
    identity: MesaConfigurationIdentity,
    undefined_variables: Vec<String>,
    llvm_configuration: MesaLlvmConfiguration,
    recipe_configuration: MesaRecipeConfiguration,
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
    /// identities, identity mismatches, undefined-scope assertions outside
    /// the reviewed contract, invalid LLVM version projections, unsafe
    /// baseline/ISA/compiler-option/bundle atoms, and selector values outside
    /// the closed schema.
    pub fn parse_bound(bytes: &[u8], expected: &MesaConfigurationIdentity) -> Result<Self> {
        if bytes.len() > MAX_NAMESPACE_CONFIGURATION_BYTES {
            return Err(configuration_error(format!(
                "input exceeds the {MAX_NAMESPACE_CONFIGURATION_BYTES}-byte limit"
            )));
        }
        expected.validate()?;

        let wire: MesaConfigurationWire = serde_json::from_slice(bytes).map_err(|error| {
            if bytes.len() > MAX_CONFIGURATION_BYTES {
                configuration_error(format!("input exceeds the {MAX_CONFIGURATION_BYTES}-byte limit or is invalid version-2 JSON"))
            } else {
                configuration_error(format!("invalid configuration JSON: {error}"))
            }
        })?;
        if wire.version == 1 && bytes.len() > MAX_CONFIGURATION_BYTES {
            return Err(configuration_error(format!(
                "input exceeds the {MAX_CONFIGURATION_BYTES}-byte limit"
            )));
        }
        if !matches!(
            (
                wire.version,
                wire.initial_namespace.is_some(),
                wire.initial_values.is_some()
            ),
            (1, false, false) | (2, true, false) | (3, true, true)
        ) {
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
        validate_undefined_variables(&wire.undefined_variables)?;
        wire.llvm_configuration.validate()?;
        wire.recipe_configuration.validate()?;
        validate_baseline_variables(&wire.baseline_variables)?;
        validate_isa_flags(&wire.target_isa_flags)?;
        validate_compiler_options(&wire.compiler_options)?;
        validate_isa_flags(&wire.mesa_compile_options)?;
        let initial_namespace = wire
            .initial_namespace
            .map(ValidatedMakeNamespace::validate)
            .transpose()?;
        let initial_values = wire
            .initial_values
            .map(|values| {
                ValidatedMakeInitialValues::validate(
                    values,
                    initial_namespace.as_ref().ok_or_else(|| {
                        configuration_error("initial values require their namespace")
                    })?,
                )
            })
            .transpose()?;
        if initial_namespace.as_ref().is_some_and(|namespace| {
            wire.undefined_variables
                .iter()
                .any(|name| namespace.contains(name))
        }) {
            return Err(configuration_error(
                "undefined scope conflicts with initial Make namespace",
            ));
        }

        Ok(Self {
            initial_namespace,
            initial_values,
            identity: wire.identity,
            undefined_variables: wire.undefined_variables,
            llvm_configuration: wire.llvm_configuration,
            recipe_configuration: wire.recipe_configuration,
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

    /// Names/origins observed before source evaluation, when explicitly supplied
    /// by configuration version 2. This is data binding, not execution proof.
    #[must_use]
    pub const fn initial_namespace(&self) -> Option<&ValidatedMakeNamespace> {
        self.initial_namespace.as_ref()
    }

    /// Raw initial values and flavors; source/producer authority is external.
    #[must_use]
    pub const fn initial_values(&self) -> Option<&ValidatedMakeInitialValues> {
        self.initial_values.as_ref()
    }

    /// Return the exact list of Mesa variables asserted globally undefined.
    #[must_use]
    pub fn undefined_variables(&self) -> &[String] {
        &self.undefined_variables
    }

    /// Return the exact configure-measured LLVM version projection.
    #[must_use]
    pub const fn llvm_configuration(&self) -> &MesaLlvmConfiguration {
        &self.llvm_configuration
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
        variables.extend(self.llvm_configuration.defined_variables());
        variables.extend(self.recipe_configuration.make_variables());
        variables
    }

    /// Required measured Make recipe inputs; this is data binding, not execution proof.
    #[must_use]
    pub const fn recipe_configuration(&self) -> &MesaRecipeConfiguration {
        &self.recipe_configuration
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MesaConfigurationWire {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial_namespace: Option<MakeNamespaceObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial_values: Option<MakeInitialValuesObservation>,
    identity: MesaConfigurationIdentity,
    undefined_variables: Vec<String>,
    llvm_configuration: MesaLlvmConfiguration,
    recipe_configuration: MesaRecipeConfiguration,
    #[serde(deserialize_with = "deserialize_baseline_variables")]
    baseline_variables: BTreeMap<String, Vec<String>>,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
    #[serde(deserialize_with = "deserialize_compiler_options")]
    compiler_options: BTreeMap<String, Vec<String>>,
    mesa_compile_options: Vec<String>,
}

fn validate_undefined_variables(variables: &[String]) -> Result<()> {
    if variables.len() != MESA_UNDEFINED_SCOPE_VARIABLES.len()
        || variables
            .iter()
            .zip(MESA_UNDEFINED_SCOPE_VARIABLES)
            .any(|(actual, expected)| actual.as_str() != *expected)
    {
        return Err(configuration_error(
            "undefined_variables must exactly match the reviewed Mesa undefined-scope variables",
        ));
    }
    Ok(())
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
