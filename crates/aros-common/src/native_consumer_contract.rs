//! Source-owned native compiler-consumer graphs, separate from boot/media builds.
//!
//! A consumer declares real MetaMake roots and measured invocation inputs. It
//! cannot manufacture board, core, package or media facts, omit a failed
//! selected producer, or qualify a compiler merely by parsing successfully.

use crate::native_build_contract::{
    self, NativeBuildAbi, NativeBuildInput, NativeInvocationConfiguration,
    NativeOptionalMetaDependency,
};
use crate::{ArosCompilerIdentity, ArosError, Result, Sha256Digest, TargetProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SCHEMA_V1: &str = "aros-native-consumer-contract-v1";
const SCHEMA_V2: &str = "aros-native-consumer-contract-v2";
const MAX_BYTES: u64 = 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NATIVE_SDK_PROBE_SOURCE_BYTES: usize = 1024;
const MAX_NATIVE_SDK_PROBE_LIBRARIES: usize = 16;
const MAX_NATIVE_SDK_PROBE_LIBRARY_BYTES: usize = 128;

/// Exact source facts for a selected native SDK/consumer graph.
///
/// The ABI and source selectors are explicit. Roots name existing source
/// endpoints; the transpiler must independently prove their complete closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeConsumerContract {
    pub schema: String,
    pub profile: String,
    /// Export baseline, not proof of the current Git checkout identity.
    pub source_baseline: String,
    pub roots: Vec<String>,
    pub inputs: Vec<NativeBuildInput>,
    pub abi: NativeBuildAbi,
    /// Mandatory sealed source invocation/discovery policy. No implicit scope.
    pub metamake_projection: String,
    #[serde(
        default,
        deserialize_with = "native_build_contract::deserialize_make_variables"
    )]
    pub make_variables: BTreeMap<String, String>,
    #[serde(
        default,
        deserialize_with = "native_build_contract::deserialize_host_make_variables"
    )]
    pub host_make_variables: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(
        default,
        deserialize_with = "native_build_contract::deserialize_make_include_bindings"
    )]
    pub make_include_bindings: BTreeMap<String, String>,
    #[serde(
        default,
        deserialize_with = "crate::native_make_template::deserialize_bindings"
    )]
    pub generated_make_templates:
        BTreeMap<String, crate::native_make_template::GeneratedMakeTemplateBinding>,
    #[serde(default)]
    pub optional_meta_dependencies: Vec<NativeOptionalMetaDependency>,
    #[serde(default)]
    pub host_file_generators: Vec<crate::native_host_generator::NativeHostFileGenerator>,
    #[serde(default)]
    pub kernel_compiler_role: Option<String>,
    /// Ordinary default-driver links against the selected source-owned SDK.
    /// Required only by v2 contracts; historical v1 graph contracts remain
    /// loadable without this field.
    #[serde(
        default,
        deserialize_with = "deserialize_native_sdk_link_probes",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_sdk_link_probes: Option<NativeSdkLinkProbes>,
}

/// The two ordinary application links required by a v2 native SDK contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSdkLinkProbes {
    pub c: NativeSdkLinkProbe,
    pub cxx: NativeSdkLinkProbe,
}

/// One sealed application source and its ordered, explicit additional libraries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSdkLinkProbe {
    pub source: String,
    pub libraries: Vec<String>,
}

// A custom deserializer distinguishes an omitted field from explicit JSON
// null. Omission is the v1-compatible default; a present field must be an
// actual, closed probe object.
fn deserialize_native_sdk_link_probes<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<NativeSdkLinkProbes>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    NativeSdkLinkProbes::deserialize(deserializer).map(Some)
}

/// Contract measured and validated against one exact source target profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedNativeConsumerContract {
    pub path: PathBuf,
    pub sha256: Sha256Digest,
    pub contract: NativeConsumerContract,
}

impl NativeConsumerContract {
    /// Require the ordinary native SDK link probes introduced by contract v2.
    ///
    /// # Errors
    /// Rejects historical v1 contracts and v2 contracts without probe inputs.
    pub fn require_native_sdk_link_probes(&self) -> Result<&NativeSdkLinkProbes> {
        if self.schema != SCHEMA_V2 {
            return Err(invalid(
                "native_sdk_link_probes",
                "ordinary SDK links require contract v2",
            ));
        }
        self.native_sdk_link_probes.as_ref().ok_or_else(|| {
            invalid(
                "native_sdk_link_probes",
                "v2 contract is missing ordinary SDK link probes",
            )
        })
    }

    /// Merge explicit shared and actual-host configuration without fallback.
    ///
    /// # Errors
    /// Rejects an absent host in a declared host table or duplicate bindings.
    pub fn make_variables_for_host(&self, host: &str) -> Result<BTreeMap<String, String>> {
        let mut variables = self.make_variables.clone();
        if self.host_make_variables.is_empty() {
            return Ok(variables);
        }
        let selected = self.host_make_variables.get(host).ok_or_else(|| {
            invalid(
                "host_make_variables",
                "actual host has no explicit configuration",
            )
        })?;
        for (name, value) in selected {
            if variables.insert(name.clone(), value.clone()).is_some() {
                return Err(invalid(
                    "host_make_variables",
                    "ambiguous shared and host binding",
                ));
            }
        }
        Ok(variables)
    }
}

/// Read a bounded source-owned consumer contract and verify every input.
///
/// # Errors
/// Rejects unsafe paths, malformed/ambiguous JSON, altered source inputs,
/// missing invocation policy, invalid roots, or ABI/profile disagreement.
pub fn load_bound_native_consumer_contract(
    root: &Path,
    relative: &Path,
    profile: &TargetProfile,
) -> Result<LoadedNativeConsumerContract> {
    let relative_text = relative
        .to_str()
        .ok_or_else(|| invalid("contract", "path is not UTF-8"))?;
    native_build_contract::validate_source_relative_path(relative_text)
        .map_err(|reason| invalid("contract", reason))?;
    if profile.native_consumer_contract.as_deref() != Some(relative_text) {
        return Err(invalid(
            "contract",
            "path differs from the source profile declaration",
        ));
    }
    // Resolve the caller's root (for example macOS /var -> /private/var),
    // never a source-relative component. The descriptor-relative reader
    // rejects symlink leaves and parents rather than following aliases.
    let root = root
        .canonicalize()
        .map_err(|_| invalid("contract", "cannot resolve source root"))?;
    let path = root.join(relative);
    let (_, bytes) = crate::measure_regular_file_bounded(&path, MAX_BYTES)?
        .ok_or_else(|| invalid("contract", "source contract is absent"))?;
    let contract: NativeConsumerContract = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("contract", "invalid closed JSON document"))?;
    validate_contract(&root, &contract, profile)?;
    Ok(LoadedNativeConsumerContract {
        path,
        sha256: crate::sha256_bytes(&bytes),
        contract,
    })
}

fn validate_contract(
    root: &Path,
    contract: &NativeConsumerContract,
    profile: &TargetProfile,
) -> Result<()> {
    let (_, profile_bytes) =
        crate::measure_regular_file_bounded(&root.join("aros-targets.toml"), MAX_BYTES)?
            .ok_or_else(|| invalid("profile", "source profile document is absent"))?;
    let profile_text = std::str::from_utf8(&profile_bytes)
        .map_err(|_| invalid("profile", "source profile document is not UTF-8"))?;
    let measured_profiles = TargetProfile::parse_config(profile_text, "aros-targets.toml")?;
    if !measured_profiles
        .targets
        .iter()
        .any(|selected| selected == profile)
    {
        return Err(invalid(
            "profile",
            "caller profile differs from the measured source declaration",
        ));
    }
    if !matches!(contract.schema.as_str(), SCHEMA_V1 | SCHEMA_V2)
        || contract.profile != profile.name
    {
        return Err(invalid(
            "identity",
            "schema or selected source profile differs",
        ));
    }
    match contract.schema.as_str() {
        SCHEMA_V1 if contract.native_sdk_link_probes.is_some() => {
            return Err(invalid(
                "native_sdk_link_probes",
                "v1 contract must not declare ordinary SDK links",
            ));
        }
        SCHEMA_V2 if contract.native_sdk_link_probes.is_none() => {
            return Err(invalid(
                "native_sdk_link_probes",
                "v2 contract requires ordinary SDK links",
            ));
        }
        _ => {}
    }
    if contract.source_baseline.len() != 40
        || !contract
            .source_baseline
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid(
            "source_baseline",
            "expected a 40-character Git object ID",
        ));
    }
    if contract.roots.is_empty() || contract.roots.len() > 16 {
        return Err(invalid(
            "roots",
            "requires one to 16 explicit source endpoints",
        ));
    }
    for (index, name) in contract.roots.iter().enumerate() {
        if name.is_empty()
            || name.len() > 256
            || matches!(name.as_str(), "." | "..")
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
            || index > 0 && contract.roots[index - 1] >= *name
        {
            return Err(invalid(
                "roots",
                "endpoints must be literal, sorted and unique",
            ));
        }
    }
    // The profile declaration itself is part of the sealed source input set.
    // It names this contract; it need not contain a recursive contract digest.
    if !contract.inputs.iter().any(|input| {
        input.path == "aros-targets.toml" && input.sha256 == crate::sha256_bytes(&profile_bytes)
    }) {
        return Err(invalid(
            "inputs",
            "requires the selected source profile document",
        ));
    }
    let reject = |message: String| invalid("configuration", &message);
    validate_nofollow_inputs(root, &contract.inputs)?;
    if let Some(probes) = &contract.native_sdk_link_probes {
        validate_native_sdk_link_probes(probes, &contract.inputs)?;
    }
    native_build_contract::validate_native_invocation_configuration(
        root,
        NativeInvocationConfiguration {
            inputs: &contract.inputs,
            make_variables: &contract.make_variables,
            host_make_variables: &contract.host_make_variables,
            make_include_bindings: &contract.make_include_bindings,
            generated_make_templates: &contract.generated_make_templates,
            metamake_projection: Some(&contract.metamake_projection),
            optional_meta_dependencies: &contract.optional_meta_dependencies,
            host_file_generators: &contract.host_file_generators,
            kernel_compiler_role: contract.kernel_compiler_role.as_deref(),
        },
        &reject,
    )?;
    native_build_contract::validate_native_abi(&contract.abi, profile, &reject)
}

fn validate_native_sdk_link_probes(
    probes: &NativeSdkLinkProbes,
    inputs: &[NativeBuildInput],
) -> Result<()> {
    validate_native_sdk_link_probe("c", &probes.c, "c", inputs)?;
    validate_native_sdk_link_probe("cxx", &probes.cxx, "cpp", inputs)
}

fn validate_native_sdk_link_probe(
    language: &str,
    probe: &NativeSdkLinkProbe,
    suffix: &str,
    inputs: &[NativeBuildInput],
) -> Result<()> {
    let field = "native_sdk_link_probes";
    if probe.source.len() > MAX_NATIVE_SDK_PROBE_SOURCE_BYTES {
        return Err(invalid(field, "probe source path exceeds its byte bound"));
    }
    let source = native_build_contract::validate_source_relative_path(&probe.source)
        .map_err(|reason| invalid(field, reason))?;
    if source.extension().and_then(|extension| extension.to_str()) != Some(suffix) {
        return Err(invalid(
            field,
            &format!("{language} probe source must use the .{suffix} suffix"),
        ));
    }
    if !inputs.iter().any(|input| input.path == probe.source) {
        return Err(invalid(
            field,
            &format!("{language} probe source must be a sealed input"),
        ));
    }
    if probe.libraries.len() > MAX_NATIVE_SDK_PROBE_LIBRARIES {
        return Err(invalid(field, "probe libraries exceed the 16-item bound"));
    }
    let mut libraries = std::collections::BTreeSet::new();
    for library in &probe.libraries {
        if library.is_empty()
            || library.len() > MAX_NATIVE_SDK_PROBE_LIBRARY_BYTES
            || !library
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            || !library
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(invalid(
                field,
                "libraries must be bounded bare literal names, not flags or paths",
            ));
        }
        if !libraries.insert(library) {
            return Err(invalid(field, "probe library names must be unique"));
        }
    }
    Ok(())
}

fn validate_nofollow_inputs(root: &Path, inputs: &[NativeBuildInput]) -> Result<()> {
    if !(1..=128).contains(&inputs.len()) {
        return Err(invalid(
            "inputs",
            "requires one to 128 measured source files",
        ));
    }
    let mut identities = std::collections::BTreeSet::new();
    for input in inputs {
        let relative = native_build_contract::validate_source_relative_path(&input.path)
            .map_err(|reason| invalid("inputs", reason))?;
        let (identity, bytes) =
            crate::measure_regular_file_bounded(&root.join(relative), MAX_INPUT_BYTES)?
                .ok_or_else(|| invalid("inputs", "declared source input is absent"))?;
        if !identities.insert((identity.device(), identity.inode())) {
            return Err(invalid(
                "inputs",
                "source inputs alias the same regular file",
            ));
        }
        if crate::sha256_bytes(&bytes) != input.sha256 {
            return Err(invalid("inputs", "measured source input digest differs"));
        }
    }
    Ok(())
}

/// Bind a consumer ABI to an independently verified GNU compiler package.
///
/// # Errors
/// Rejects a different compiler family, triple, ISA, ABI or code model.
pub fn validate_native_consumer_compiler(
    contract: &NativeConsumerContract,
    compiler: &ArosCompilerIdentity,
    triple: &str,
) -> Result<()> {
    compiler
        .validate_for_target(triple)
        .map_err(|_| invalid("compiler", "verified target identity is invalid"))?;
    let ArosCompilerIdentity::Gnu { target, .. } = compiler else {
        return Err(invalid(
            "compiler",
            "consumer requires a verified GNU compiler",
        ));
    };
    if triple != contract.abi.target_triple
        || target.isa() != contract.abi.isa
        || target.abi() != contract.abi.abi
        || target.code_model() != contract.abi.code_model
    {
        return Err(invalid(
            "compiler",
            "verified triple/ISA/ABI/code model differs",
        ));
    }
    Ok(())
}

fn invalid(field: &str, message: &str) -> ArosError {
    ArosError::Configuration {
        file: format!("native consumer contract/{field}"),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "native_consumer_contract_tests.rs"]
mod tests;
