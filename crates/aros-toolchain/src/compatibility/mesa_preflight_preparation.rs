//! Private, single-use source preparation before any completion receipt exists.
//!
//! This is a live phase boundary, not a portable owner or authorization to run
//! native recipes. The unchanged completion path accepts only its exact output
//! snapshot. A future native phase must bind its stages explicitly; mutation
//! cannot silently become part of an old receipt schema.

use super::{
    complete_mesa_preflight, digest_json, environment, failure, genmodule_request_identity,
    invocation, validate_selection, CompletedMesaPreflight, MesaPreflightRequest, Stage,
};
use crate::ContractError;
use aros_common::mesa_configuration::ValidatedMesaConfiguration;
use aros_common::{measure_tree_content_cas, CancellationToken, Sha256Digest, TreeContentCas};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(super) struct ConfiguredMesaSource {
    pub(super) started: Instant,
    pub(super) source: PathBuf,
    pub(super) initial_configuration: ValidatedMesaConfiguration,
    pub(super) measured: BTreeMap<String, String>,
    pub(super) values: BTreeMap<String, String>,
    pub(super) compiler_cfg: String,
    pub(super) make: PathBuf,
    pub(super) final_path: PathBuf,
    pub(super) stages: Vec<Stage>,
}

pub(super) struct PreparedMesaPreflight {
    pub(super) configured: ConfiguredMesaSource,
    selection_sha256: Sha256Digest,
    output_tree: TreeContentCas,
    output_root_mode: u32,
    timeout: Duration,
}

impl PreparedMesaPreflight {
    pub(super) const fn output_snapshot(&self) -> &TreeContentCas {
        &self.output_tree
    }

    pub(super) fn retain(
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
        configured: ConfiguredMesaSource,
    ) -> Result<Self, ContractError> {
        validate_selection(request, cancellation)?;
        let output_root_mode = root_mode(request.output_root)?;
        let output_tree = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot retain Mesa preparation output identity"))?;
        let prepared = Self {
            configured,
            selection_sha256: selection_identity(request)?,
            output_tree,
            output_root_mode,
            timeout: request.timeout,
        };
        prepared.revalidate(request, cancellation)?;
        Ok(prepared)
    }

    pub(super) fn configured_invocation(
        &self,
    ) -> Result<&invocation::InvocationContext, ContractError> {
        self.configured
            .stages
            .get(3)
            .filter(|stage| stage.name == "configured-values" && self.configured.stages.len() == 4)
            .map(|stage| &stage.invocation_context)
            .ok_or_else(|| failure("Mesa preparation lost its configured invocation"))
    }

    pub(super) fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.revalidate_selection(request, cancellation)?;
        let current = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot remeasure Mesa preparation output identity"))?;
        if current != self.output_tree {
            return Err(failure("Mesa preparation output changed before completion"));
        }
        self.remaining()?;
        Ok(())
    }

    // Only a native phase with its own explicit output transition may use
    // this narrower check. Ordinary completion still checks the entire tree.
    pub(super) fn revalidate_selection(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.revalidate_selected_source(request, cancellation, None)
    }

    // Native Root owns a snapshot captured against this exact preparation.
    // Check its original bytes/identities once rather than capturing a second
    // whole source tree immediately before checking the held snapshot again.
    pub(super) fn revalidate_selection_with_source(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
        source: &aros_common::mesa_source::MesaSourceSnapshot,
    ) -> Result<(), ContractError> {
        self.revalidate_selected_source(request, cancellation, Some(source))
    }

    fn revalidate_selected_source(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
        source: Option<&aros_common::mesa_source::MesaSourceSnapshot>,
    ) -> Result<(), ContractError> {
        self.remaining()?;
        super::validate_selection_with_source(request, cancellation, source)?;
        self.configured_invocation()?.revalidate()?;
        if selection_identity(request)? != self.selection_sha256 {
            return Err(failure("Mesa preparation selected different inputs"));
        }
        if root_mode(request.output_root)? != self.output_root_mode {
            return Err(failure("Mesa preparation output root mode changed"));
        }
        self.remaining()?;
        Ok(())
    }

    pub(super) fn remaining(&self) -> Result<Duration, ContractError> {
        self.timeout
            .checked_sub(self.configured.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| failure("Mesa preparation deadline expired"))
    }

    pub(super) fn complete(
        self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<CompletedMesaPreflight, ContractError> {
        complete_mesa_preflight(request, cancellation, self)
    }

    pub(super) fn into_configuration(self) -> ConfiguredMesaSource {
        self.configured
    }
}

fn root_mode(path: &std::path::Path) -> Result<u32, ContractError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| failure("cannot inspect Mesa preparation output root"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure("Mesa preparation output root is not a directory"));
    }
    Ok(metadata.permissions().mode())
}

fn selection_identity(request: &MesaPreflightRequest<'_>) -> Result<Sha256Digest, ContractError> {
    // Bind selection, not just measured content: two equal trees at different
    // paths or an extended timeout are not the original live request.
    digest_json(&serde_json::json!({
        "source_root": request.source_root,
        "source_tree_sha256": request.source_tree_sha256,
        "local_source": request.local_source,
        "profile_sha256": request.profile.document_sha256(),
        "profile_name": request.profile.name(),
        "target_triple": request.profile.target_triple(),
        "configure_target": request.profile.configure_target(),
        "float_abi": request.profile.float_abi(),
        "compiler": request.drivers.compiler(),
        "compiler_tree_sha256": request.drivers.compiler_tree_sha256(),
        "clang": request.drivers.clang_path(),
        "clangxx": request.drivers.clangxx_path(),
        "environment": environment::identity(&environment::resolve(request.environment)?)?,
        "genmodule_selection": genmodule_request_identity(request)?,
        "output_root": request.output_root,
        "timeout_nanos": request.timeout.as_nanos().to_string(),
    }))
}
