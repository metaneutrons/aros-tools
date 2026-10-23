//! Private owner of a real Mesa preflight across native CMake graph execution.
//!
//! A JSON data input cannot construct this owner. Portable result joining is a
//! separate boundary; no producer authentication or release authority is granted.

use std::path::PathBuf;
use std::time::Duration;

use aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope;
use aros_common::{CancellationToken, Sha256Digest};

use super::NativeCompatibilityRequest;
use crate::compatibility::{
    execute_mesa_preflight, CompatibilityEnvironment, CompletedMesaPreflight, HelperIdentity,
    LlvmSdkDriverBinding, MesaGenmodulePreflightSelection, MesaPreflightRequest,
    SourceMesaPreflightProfile,
};
use crate::ContractError;

pub(super) struct OwnedMesaPreflight {
    completed: CompletedMesaPreflight,
    output_root: PathBuf,
    source_profile: SourceMesaPreflightProfile,
    helper: HelperIdentity,
}

/// This policy comes from the independently verified source, never a receipt.
pub(super) fn required(request: &NativeCompatibilityRequest) -> bool {
    request.llvm_sdk_source.as_ref().is_some_and(|source| {
        source
            .source_profile()
            .transpiler
            .as_ref()
            .is_some_and(|context| context.mesa_version.as_deref() == Some("26.0.0"))
    })
}

pub(super) fn begin(
    request: &NativeCompatibilityRequest,
    reports_root: &std::path::Path,
    drivers: Option<&LlvmSdkDriverBinding>,
    environment: &CompatibilityEnvironment,
    cancellation: &CancellationToken,
) -> Result<Option<OwnedMesaPreflight>, ContractError> {
    if !required(request) {
        return Ok(None);
    }
    let drivers =
        drivers.ok_or_else(|| error("configured Mesa execution lost its LLVM drivers"))?;
    // The source SDK selector already binds platform, ABI and compiler profile.
    // Recheck that boundary here before launching any configuration process.
    super::llvm_sdk::select(request, drivers.compiler())?
        .ok_or_else(|| error("configured Mesa execution lost its verified source selection"))?;
    let source = request
        .llvm_sdk_source
        .as_ref()
        .ok_or_else(|| error("configured Mesa execution lost its source profile"))?;
    let source_profile = SourceMesaPreflightProfile::load(
        &request.preparation.source_root,
        source.source_profile(),
    )?;
    let helper = request
        .preparation
        .helpers
        .get("aros-transpiler")
        .ok_or_else(|| error("configured Mesa execution lost its measured inventory helper"))?
        .clone();
    let output_root = reports_root.join("mesa-preflight");
    let completed = execute_mesa_preflight(
        &preflight_request(
            request,
            drivers,
            environment,
            &output_root,
            &source_profile,
            &helper,
        )?,
        cancellation,
    )?;
    Ok(Some(OwnedMesaPreflight {
        completed,
        output_root,
        source_profile,
        helper,
    }))
}

impl OwnedMesaPreflight {
    pub(super) fn arguments(
        &self,
        request: &NativeCompatibilityRequest,
        drivers: &LlvmSdkDriverBinding,
        environment: &CompatibilityEnvironment,
        cancellation: &CancellationToken,
    ) -> Result<Vec<String>, ContractError> {
        self.completed.cmake_configuration_arguments(
            &preflight_request(
                request,
                drivers,
                environment,
                &self.output_root,
                &self.source_profile,
                &self.helper,
            )?,
            cancellation,
        )
    }

    pub(super) fn revalidate(
        &self,
        request: &NativeCompatibilityRequest,
        drivers: &LlvmSdkDriverBinding,
        environment: &CompatibilityEnvironment,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        if !required(request) {
            return Err(error(
                "configured Mesa source policy changed during execution",
            ));
        }
        super::llvm_sdk::select(request, drivers.compiler())?
            .ok_or_else(|| error("configured Mesa execution lost its source policy"))?;
        self.completed
            .data_evidence(
                &preflight_request(
                    request,
                    drivers,
                    environment,
                    &self.output_root,
                    &self.source_profile,
                    &self.helper,
                )?,
                cancellation,
            )
            .map(|_| ())
    }

    pub(super) const fn receipt_sha256(&self) -> &Sha256Digest {
        self.completed.receipt_sha256()
    }
}

fn preflight_request<'a>(
    request: &'a NativeCompatibilityRequest,
    drivers: &'a LlvmSdkDriverBinding,
    environment: &'a CompatibilityEnvironment,
    output_root: &'a std::path::Path,
    source_profile: &'a SourceMesaPreflightProfile,
    helper: &'a HelperIdentity,
) -> Result<MesaPreflightRequest<'a>, ContractError> {
    let current = request
        .preparation
        .helpers
        .get("aros-transpiler")
        .ok_or_else(|| error("configured Mesa execution lost its inventory helper"))?;
    if current.path != helper.path || current.sha256 != helper.sha256 {
        return Err(error("configured Mesa inventory helper selection changed"));
    }
    Ok(MesaPreflightRequest {
        source_root: &request.preparation.source_root,
        source_tree_sha256: &request.preparation.source_tree_sha256,
        local_source: None,
        profile: (&request.profile).into(),
        genmodule_selection: Some(MesaGenmodulePreflightSelection {
            source_profile,
            scope: MesaGenmoduleInventoryScope::ContractRoots,
            transpiler: &helper.path,
            transpiler_sha256: &helper.sha256,
        }),
        drivers,
        environment,
        output_root,
        timeout: request.timeout.min(Duration::from_secs(600)),
    })
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
