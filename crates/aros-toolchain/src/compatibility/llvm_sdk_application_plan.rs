//! Source-bound ordinary LLVM application commands, not executed-link evidence.

use std::fs;
use std::path::{Path, PathBuf};

use aros_common::sdk_consumer_contract::{
    load_bound_sdk_consumer_contract, LoadedSdkConsumerContract, SDK_CONSUMER_CONTRACT_SCHEMA,
};
use aros_common::{
    measure_tree_content_cas_bounded, CancellationToken, TreeContentCas, TreeTraversalLimits,
};

use super::{checked_directory, llvm_sdk_application, CompatibilityCommand, LlvmSdkDriverBinding};
use crate::sdk_source::VerifiedSdkSource;
use crate::{filesystem::open_directory, ContractError};

const SDK_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_000,
    max_regular_file_bytes: 2 * 1024 * 1024 * 1024,
};

/// Independent, already measured inputs to four normal LLVM application links.
#[derive(Debug, Clone)]
pub struct LlvmSdkApplicationPlanRequest {
    /// Clean committed source, selected SDK profile and sealed C/C++ probes.
    pub source: VerifiedSdkSource,
    /// Both queried drivers and the unchanged complete selected compiler tree.
    pub drivers: LlvmSdkDriverBinding,
    /// Complete original SDK supplied by its owning generation executor.
    pub original_sdk: PathBuf,
    /// Independently relocated, byte-identical SDK, not an alias of the original.
    pub relocated_sdk: PathBuf,
    /// Absent direct-child proof directory under an existing real parent.
    pub output_root: PathBuf,
}

/// Opaque, revalidatable source-selected commands for later supervised execution.
///
/// No source build, application compiler, copy or filesystem write happens here.
/// Equal SDK trees do not prove that either tree is a complete generated SDK.
/// The owner still validates generation, bounded traces/maps, retained outputs,
/// byte-identical relocated links and all portable evidence before admission.
#[derive(Debug, Clone)]
pub struct LlvmSdkApplicationPlan {
    request: LlvmSdkApplicationPlanRequest,
    original_snapshot: TreeContentCas,
    relocated_snapshot: TreeContentCas,
    commands: Vec<CompatibilityCommand>,
}

impl LlvmSdkApplicationPlan {
    /// Recheck only immutable inputs after the execution owner reserved output.
    pub(super) fn revalidate_inputs(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        bound_contract(&self.request, cancellation)?;
        if checked_directory(&self.request.original_sdk, "original LLVM SDK")?
            != self.request.original_sdk
            || checked_directory(&self.request.relocated_sdk, "relocated LLVM SDK")?
                != self.request.relocated_sdk
            || snapshot(&self.request.original_sdk)? != self.original_snapshot
            || snapshot(&self.request.relocated_sdk)? != self.relocated_snapshot
        {
            return Err(error("LLVM SDK application plan inputs changed"));
        }
        Ok(())
    }

    /// C original/relocated followed by C++ original/relocated normal links.
    #[must_use]
    pub fn commands(&self) -> &[CompatibilityCommand] {
        &self.commands
    }

    /// Reaudit input identities and the still-absent destination before execution.
    ///
    /// # Errors
    /// Rejects changed source, compiler, SDK trees, root selection or commands.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        let current = plan_llvm_sdk_application_links(&self.request, cancellation)?;
        if current.original_snapshot != self.original_snapshot
            || current.relocated_snapshot != self.relocated_snapshot
            || current.commands.len() != self.commands.len()
            || current
                .commands
                .iter()
                .zip(&self.commands)
                .any(|(a, b)| a.program != b.program || a.arguments != b.arguments)
        {
            return Err(error("LLVM SDK application plan inputs changed"));
        }
        Ok(())
    }

    /// Verify only source-declared ELF structure/marking of one supplied output.
    ///
    /// This does not prove which command produced the bytes or that an ordinary
    /// application linked. Invocation, input closure and C++ initialization
    /// qualification remain the execution owner's separate responsibilities.
    ///
    /// # Errors
    /// Rejects malformed ELF, wrong class/machine/flags/AROS marking or collector.
    pub fn verify_application_elf(&self, bytes: &[u8]) -> Result<(), ContractError> {
        llvm_sdk_application::verify_application_elf(bytes, &self.request.source.identity().target)
    }
}

/// Plan four normal application links from sealed source probes and measured drivers.
///
/// No generic build guard is relaxed. The source-owned target must explicitly
/// declare ELF machine, pointer width and flag mask/value; no board or CPU table
/// supplies missing facts. Package authenticity and SDK generation are external
/// gates. This boundary makes no execution, ABI-completeness or release claim.
///
/// # Errors
/// Rejects selection disagreement, missing ABI facts, stale inputs, unequal or
/// aliased SDKs and occupied/overlapping outputs before any filesystem write.
pub fn plan_llvm_sdk_application_links(
    request: &LlvmSdkApplicationPlanRequest,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkApplicationPlan, ContractError> {
    plan_selected(request, cancellation, false)
}

/// Reconstruct commands for an existing proof without adopting its commands.
pub(super) fn readback_plan(
    request: &LlvmSdkApplicationPlanRequest,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkApplicationPlan, ContractError> {
    plan_selected(request, cancellation, true)
}

fn plan_selected(
    request: &LlvmSdkApplicationPlanRequest,
    cancellation: &CancellationToken,
    retained: bool,
) -> Result<LlvmSdkApplicationPlan, ContractError> {
    let loaded = bound_contract(request, cancellation)?;
    let source_root = request.source.materialized_source_root();
    let target = &request.source.identity().target;
    let original = checked_directory(&request.original_sdk, "original LLVM SDK")?;
    let relocated = checked_directory(&request.relocated_sdk, "relocated LLVM SDK")?;
    if overlap(&original, &relocated) {
        return Err(error(
            "LLVM SDK application roots must be distinct and non-overlapping",
        ));
    }
    let output = selected_output(&request.output_root, retained)?;
    for input in [
        source_root,
        request.source.original_source_root(),
        request
            .drivers
            .clang_path()
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| error("LLVM SDK compiler root is absent"))?,
        original.as_path(),
        relocated.as_path(),
    ] {
        if overlap(&output, input) {
            return Err(error("LLVM SDK application output overlaps an input"));
        }
    }
    let original_snapshot = snapshot(&original)?;
    let relocated_snapshot = snapshot(&relocated)?;
    if original_snapshot.payload_digest_excluding(None)
        != relocated_snapshot.payload_digest_excluding(None)
    {
        return Err(error("LLVM SDK application trees differ before linking"));
    }
    let mut commands = Vec::with_capacity(4);
    for (language, program, probe) in [
        (
            "c",
            request.drivers.clang_path(),
            &loaded.contract.native_sdk_link_probes.c,
        ),
        (
            "cxx",
            request.drivers.clangxx_path(),
            &loaded.contract.native_sdk_link_probes.cxx,
        ),
    ] {
        for (label, sdk) in [("original", &original), ("relocated", &relocated)] {
            commands.push(llvm_sdk_application::application_command(
                program,
                &source_root.join(&probe.source),
                probe,
                target,
                sdk,
                &output,
                (language, label),
            )?);
        }
    }
    if cancellation.is_cancelled() {
        return Err(error("LLVM SDK application planning was cancelled"));
    }
    request.source.revalidate()?;
    request.drivers.revalidate(cancellation)?;
    if snapshot(&original)? != original_snapshot || snapshot(&relocated)? != relocated_snapshot {
        return Err(error("LLVM SDK application trees changed during planning"));
    }
    selected_output(&output, retained)?;
    Ok(LlvmSdkApplicationPlan {
        request: LlvmSdkApplicationPlanRequest {
            source: request.source.clone(),
            drivers: request.drivers.clone(),
            original_sdk: original,
            relocated_sdk: relocated,
            output_root: output,
        },
        original_snapshot,
        relocated_snapshot,
        commands,
    })
}

fn selected_output(path: &Path, retained: bool) -> Result<PathBuf, ContractError> {
    if !retained {
        return output_root(path);
    }
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || checked_directory(path, "retained LLVM SDK application output")? != path
    {
        return Err(error(
            "retained LLVM SDK application output is not canonical",
        ));
    }
    open_directory(path)
        .map_err(|_| error("retained LLVM SDK application output is not a real directory"))?;
    Ok(path.to_owned())
}

fn bound_contract(
    request: &LlvmSdkApplicationPlanRequest,
    cancellation: &CancellationToken,
) -> Result<LoadedSdkConsumerContract, ContractError> {
    if cancellation.is_cancelled() {
        return Err(error("LLVM SDK application planning was cancelled"));
    }
    if request.source.consumer_contract_schema() != SDK_CONSUMER_CONTRACT_SCHEMA {
        return Err(error(
            "LLVM SDK application planning requires the generic source contract",
        ));
    }
    let target = &request.source.identity().target;
    if request.drivers.target() != target {
        return Err(error(
            "LLVM SDK drivers differ from the complete source-selected target",
        ));
    }
    llvm_sdk_application::validate_target(target)?;
    request.source.revalidate()?;
    request.drivers.revalidate(cancellation)?;
    let source_root = request.source.materialized_source_root();
    let profile = request.source.source_profile();
    let relative = profile
        .sdk_consumer_contract
        .as_deref()
        .ok_or_else(|| error("LLVM SDK source profile has no generic contract"))?;
    let loaded = load_bound_sdk_consumer_contract(source_root, Path::new(relative), profile)
        .map_err(|_| error("LLVM SDK application source contract is not sealed"))?;
    if loaded.sha256 != request.source.identity().source.contract_sha256
        || loaded.contract.target != *target
    {
        return Err(error(
            "LLVM SDK application source contract differs from its identity",
        ));
    }
    Ok(loaded)
}

fn output_root(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(error(
            "LLVM SDK application output must be an absolute non-traversing leaf",
        ));
    }
    let parent = checked_directory(
        path.parent()
            .ok_or_else(|| error("LLVM SDK output parent is absent"))?,
        "LLVM SDK output parent",
    )?;
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| error("LLVM SDK output leaf is absent"))?,
    );
    open_directory(&parent).map_err(|_| error("LLVM SDK output parent is not a real directory"))?;
    match fs::symlink_metadata(&output) {
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(output),
        _ => Err(error(
            "LLVM SDK application output already exists or cannot be inspected",
        )),
    }
}

fn snapshot(root: &Path) -> Result<TreeContentCas, ContractError> {
    measure_tree_content_cas_bounded(root, SDK_LIMITS)
        .map_err(|_| error("LLVM SDK application tree is unsafe or exceeds its bounds"))
}

fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}
