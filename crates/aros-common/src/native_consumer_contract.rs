//! Source-owned native compiler-consumer graphs, separate from boot/media builds.
//!
//! A consumer declares real MetaMake roots and measured invocation inputs. It
//! cannot manufacture board, core, package or media facts, omit a failed
//! selected producer, or qualify a compiler merely by parsing successfully.

use crate::native_build_contract::{
    self, NativeBuildAbi, NativeBuildInput, NativeOptionalMetaDependency,
};
use crate::native_consumer_graph::{validate_native_consumer_graph, NativeConsumerGraphView};
use crate::{ArosCompilerIdentity, ArosError, Result, Sha256Digest, TargetProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SCHEMA_V1: &str = "aros-native-consumer-contract-v1";
const SCHEMA_V2: &str = "aros-native-consumer-contract-v2";
const MAX_BYTES: u64 = 1024 * 1024;

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
    validate_native_consumer_graph(
        root,
        profile,
        NativeConsumerGraphView {
            contract_profile: &contract.profile,
            source_baseline: &contract.source_baseline,
            roots: &contract.roots,
            inputs: &contract.inputs,
            metamake_projection: &contract.metamake_projection,
            make_variables: &contract.make_variables,
            host_make_variables: &contract.host_make_variables,
            make_include_bindings: &contract.make_include_bindings,
            generated_make_templates: &contract.generated_make_templates,
            optional_meta_dependencies: &contract.optional_meta_dependencies,
            host_file_generators: &contract.host_file_generators,
            kernel_compiler_role: contract.kernel_compiler_role.as_deref(),
            native_sdk_link_probes: contract.native_sdk_link_probes.as_ref(),
        },
        "native consumer contract",
    )?;
    let reject = |message: String| invalid("configuration", &message);
    native_build_contract::validate_native_abi(&contract.abi, profile, &reject)
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
