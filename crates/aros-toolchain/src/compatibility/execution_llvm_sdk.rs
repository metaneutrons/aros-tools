//! Source-owned LLVM SDK selection for the complete compatibility executor.
//!
//! Local execution and consistency do not authenticate a CI job or qualify a
//! release. Portable owning-job export remains a separate mandatory boundary.

use std::path::Path;
use std::time::Duration;

use aros_common::sdk_consumer_contract::{
    load_bound_sdk_consumer_contract, LoadedSdkConsumerContract, SDK_CONSUMER_CONTRACT_SCHEMA,
};
use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity};
use aros_common::{ArosCompilerIdentity, CancellationToken, Sha256Digest};

use super::NativeCompatibilityRequest;
use crate::compatibility::{
    execute_llvm_sdk_build_links, inspect_llvm_sdk_drivers, CompletedLlvmSdkBuildLinks,
    LlvmSdkDriverBinding, LlvmSdkDriverRequest, LlvmSdkGeneratedRootRequest,
};
use crate::ContractError;

/// Measured local LLVM SDK evidence from all six phases and four normal links.
///
/// These digests are execution results, not portable proof or release authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlvmSdkCompatibilityReport {
    /// Exact independently selected SDK product/source/target identity.
    pub identity: SdkIdentity,
    /// Digest of the source-derived CMake SDK binding.
    pub binding_sha256: Sha256Digest,
    /// Digest of the four-link retained receipt.
    pub receipt_sha256: Sha256Digest,
    /// Complete original/relocated SDK inventory digest.
    pub sdk_inventory_sha256: Sha256Digest,
    /// Measured SDK inventory entry count.
    pub sdk_entries: usize,
}

pub(super) fn select(
    request: &NativeCompatibilityRequest,
    compiler: &ArosCompilerIdentity,
) -> Result<Option<LoadedSdkConsumerContract>, ContractError> {
    let Some(source) = &request.llvm_sdk_source else {
        return Ok(None);
    };
    if !matches!(compiler, ArosCompilerIdentity::Llvm { .. })
        || request.relocation.first.verified.manifest.schema != 2
        || request.source_preset.is_some()
        || request.consumer_source_date_epoch != Some(source.identity().source_date_epoch)
        || source.consumer_contract_schema() != SDK_CONSUMER_CONTRACT_SCHEMA
    {
        return Err(error("LLVM SDK execution requires a family-v2 LLVM package, explicit epoch and verified generic source selection"));
    }
    source.revalidate()?;
    let identity = source.identity();
    let profile = source.source_profile();
    let context = profile
        .transpiler
        .as_ref()
        .ok_or_else(|| error("LLVM SDK source has no complete transpiler context"))?;
    if source.materialized_source_root() != request.preparation.source_root
        || identity.source.content_sha256 != request.preparation.source_tree_sha256
        || identity.target.target_triple != request.profile.target_triple()
        || profile.toolchain_profile() != request.profile.name()
        || profile.arch.source_cpu() != request.profile.cpu()
        || profile.platform != request.profile.platform()
        || profile.float_abi.as_deref().unwrap_or_default() != request.profile.float_abi()
        || context.toolchain != "llvm"
    {
        return Err(error(
            "LLVM SDK source, preparation and compiler target/profile selection differ",
        ));
    }
    let path = profile
        .sdk_consumer_contract
        .as_deref()
        .ok_or_else(|| error("LLVM SDK source does not declare its generic consumer contract"))?;
    let loaded = load_bound_sdk_consumer_contract(
        &request.preparation.source_root,
        Path::new(path),
        profile,
    )
    .map_err(|_| error("LLVM SDK consumer contract is not sealed to its source profile"))?;
    if loaded.sha256 != identity.source.contract_sha256 || loaded.contract.target != identity.target
    {
        return Err(error(
            "LLVM SDK contract differs from its selected identity",
        ));
    }
    // CMake's executable SDK contract requires these source-owned scalar
    // bootstrap facts. Never infer them from a CPU or board name in Rust.
    if !matches!(
        identity.target.abi.get("flavour"),
        Some(SdkAbiValue::Text(_))
    ) || !matches!(
        identity.target.abi.get("platform_smp"),
        Some(SdkAbiValue::Boolean(_))
    ) {
        return Err(error(
            "LLVM SDK execution lacks source-owned bootstrap ABI facts",
        ));
    }
    crate::compatibility::llvm_sdk_application::validate_target(&identity.target)?;
    Ok(Some(loaded))
}

pub(super) fn drivers(
    request: &NativeCompatibilityRequest,
    compiler: &ArosCompilerIdentity,
    cancellation: &CancellationToken,
) -> Result<Option<LlvmSdkDriverBinding>, ContractError> {
    request
        .llvm_sdk_source
        .as_ref()
        .map(|source| {
            inspect_llvm_sdk_drivers(
                &LlvmSdkDriverRequest {
                    compiler_root: request.relocation.first.root.clone(),
                    compiler: compiler.clone(),
                    target: source.identity().target.clone(),
                    timeout: request.timeout.min(Duration::from_secs(60)),
                },
                cancellation,
            )
        })
        .transpose()
}

pub(super) fn execute(
    request: &NativeCompatibilityRequest,
    drivers: Option<&LlvmSdkDriverBinding>,
    cancellation: &CancellationToken,
) -> Result<Option<CompletedLlvmSdkBuildLinks>, ContractError> {
    match (&request.llvm_sdk_source, drivers) {
        (Some(source), Some(drivers)) => execute_llvm_sdk_build_links(
            &LlvmSdkGeneratedRootRequest {
                source: source.clone(),
                preparation: request.preparation.clone(),
                cmake_build_root: request.cmake_build_root.clone(),
                timeout: request.timeout,
            },
            drivers,
            &request.reports_root.join("llvm-sdk-links"),
            cancellation,
        )
        .map(Some),
        (None, None) => Ok(None),
        _ => Err(error(
            "LLVM SDK execution lost its measured driver selection",
        )),
    }
}

pub(super) fn report(
    request: &NativeCompatibilityRequest,
    completed: &CompletedLlvmSdkBuildLinks,
) -> Result<LlvmSdkCompatibilityReport, ContractError> {
    let source = request
        .llvm_sdk_source
        .as_ref()
        .ok_or_else(|| error("LLVM SDK execution lost its independently selected source"))?;
    Ok(LlvmSdkCompatibilityReport {
        identity: source.identity().clone(),
        binding_sha256: completed.binding_sha256(),
        receipt_sha256: completed.links().receipt_sha256().clone(),
        sdk_inventory_sha256: completed.links().sdk_inventory_sha256().clone(),
        sdk_entries: completed.links().sdk_entries(),
    })
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
