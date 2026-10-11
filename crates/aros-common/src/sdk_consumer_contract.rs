//! Source-owned, target-generic SDK consumer contract.
//!
//! This contract describes the SDK's source graph and scalar target ABI. It
//! remains separate from the legacy GNU/RISC-V native compiler ABI contract.

use crate::native_build_contract::{self, NativeBuildInput, NativeOptionalMetaDependency};
use crate::native_consumer_contract::NativeSdkLinkProbes;
use crate::native_consumer_graph::{validate_native_consumer_graph, NativeConsumerGraphView};
use crate::native_host_generator::NativeHostFileGenerator;
use crate::native_make_template::GeneratedMakeTemplateBinding;
use crate::sdk_manifest::{SdkAbiValue, SdkTarget};
use crate::{ArosError, Result, Sha256Digest, TargetProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SDK_CONSUMER_CONTRACT_SCHEMA: &str = "aros-sdk-consumer-contract-v1";
const MAX_BYTES: u64 = 1024 * 1024;

/// Complete source-owned SDK graph, generic ABI facts and ordinary link probes.
///
/// Invocation members deliberately use the same flat names and parsers as the
/// native consumer graph. The `target` member is an SDK descriptor, not a
/// `NativeBuildAbi` and does not imply that a compiler family is supported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkConsumerContract {
    pub schema: String,
    pub profile: String,
    /// Export baseline, not proof of the current Git checkout identity.
    pub source_baseline: String,
    pub roots: Vec<String>,
    pub inputs: Vec<NativeBuildInput>,
    pub target: SdkTarget,
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
    pub generated_make_templates: BTreeMap<String, GeneratedMakeTemplateBinding>,
    #[serde(default)]
    pub optional_meta_dependencies: Vec<NativeOptionalMetaDependency>,
    #[serde(default)]
    pub host_file_generators: Vec<NativeHostFileGenerator>,
    #[serde(default)]
    pub kernel_compiler_role: Option<String>,
    /// Ordinary default-driver C and C++ links against this SDK.
    pub native_sdk_link_probes: NativeSdkLinkProbes,
}

impl SdkConsumerContract {
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

/// Bounded, measured source-owned SDK contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSdkConsumerContract {
    pub path: PathBuf,
    pub sha256: Sha256Digest,
    pub contract: SdkConsumerContract,
}

/// Read and validate the source profile's exact SDK consumer contract.
///
/// The profile and every graph input are measured without following symlinks;
/// callers must retain this result and revalidate it across source execution.
///
/// # Errors
/// Rejects an undeclared path, malformed document, changed source input,
/// incomplete selector context, or target/profile disagreement.
pub fn load_bound_sdk_consumer_contract(
    root: &Path,
    relative: &Path,
    profile: &TargetProfile,
) -> Result<LoadedSdkConsumerContract> {
    let relative_text = relative
        .to_str()
        .ok_or_else(|| invalid("contract", "path is not UTF-8"))?;
    native_build_contract::validate_source_relative_path(relative_text)
        .map_err(|reason| invalid("contract", reason))?;
    if profile.sdk_consumer_contract.as_deref() != Some(relative_text) {
        return Err(invalid(
            "contract",
            "path differs from the source profile declaration",
        ));
    }
    let root = root
        .canonicalize()
        .map_err(|_| invalid("contract", "cannot resolve source root"))?;
    let path = root.join(relative);
    let (_, bytes) = crate::measure_regular_file_bounded(&path, MAX_BYTES)?
        .ok_or_else(|| invalid("contract", "source contract is absent"))?;
    let contract: SdkConsumerContract = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("contract", "invalid closed JSON document"))?;
    validate_contract(&root, &contract, profile)?;
    Ok(LoadedSdkConsumerContract {
        path,
        sha256: crate::sha256_bytes(&bytes),
        contract,
    })
}

fn validate_contract(
    root: &Path,
    contract: &SdkConsumerContract,
    profile: &TargetProfile,
) -> Result<()> {
    if contract.schema != SDK_CONSUMER_CONTRACT_SCHEMA || contract.profile != profile.name {
        return Err(invalid(
            "identity",
            "schema or selected source profile differs",
        ));
    }
    validate_sdk_target(&contract.target, profile)?;
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
            native_sdk_link_probes: Some(&contract.native_sdk_link_probes),
        },
        "SDK consumer contract",
    )
}

fn validate_sdk_target(target: &SdkTarget, profile: &TargetProfile) -> Result<()> {
    target
        .validate()
        .map_err(|reason| invalid("target", &reason))?;

    let source_cpu = profile.arch.source_cpu();
    let architecture = profile.arch.to_string();
    let triple_components = target.target_triple.split('-').collect::<Vec<_>>();
    let triple_cpu = triple_components.first().copied().unwrap_or_default();
    if triple_components.len() < 2
        || triple_components
            .iter()
            .any(|component| component.is_empty())
        || triple_components.last() != Some(&"aros")
        || ![source_cpu.as_str(), architecture.as_str()].contains(&triple_cpu)
    {
        return Err(invalid(
            "target.target_triple",
            "triple CPU must match the selected architecture and end in -aros",
        ));
    }
    require_abi_value(target, "source_cpu", &SdkAbiValue::Text(source_cpu))?;
    require_abi_value(
        target,
        "pointer_width",
        &SdkAbiValue::Unsigned(profile.arch.pointer_width() as u64),
    )?;

    if let Some(float_abi) = &profile.float_abi {
        require_abi_value(target, "float_abi", &SdkAbiValue::Text(float_abi.clone()))?;
    }

    let transpiler = profile.transpiler.as_ref().ok_or_else(|| {
        invalid(
            "profile.transpiler",
            "SDK consumer contracts require a complete source selector context",
        )
    })?;
    require_abi_value(target, "use_mmu", &SdkAbiValue::Boolean(transpiler.use_mmu))?;

    if let Some(bootstrap) = &profile.bootstrap_abi {
        require_abi_value(
            target,
            "flavour",
            &SdkAbiValue::Text(bootstrap.flavour.clone()),
        )?;
        require_abi_value(
            target,
            "platform_smp",
            &SdkAbiValue::Boolean(bootstrap.platform_smp),
        )?;
    }
    Ok(())
}

fn require_abi_value(target: &SdkTarget, key: &str, expected: &SdkAbiValue) -> Result<()> {
    if target.abi.get(key) != Some(expected) {
        return Err(invalid(
            &format!("target.abi.{key}"),
            "required source ABI fact is absent or contradicts the selected profile",
        ));
    }
    Ok(())
}

fn invalid(field: &str, message: &str) -> ArosError {
    ArosError::Configuration {
        file: format!("SDK consumer contract/{field}"),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "sdk_consumer_contract_tests.rs"]
mod tests;
