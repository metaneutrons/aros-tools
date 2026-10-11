//! Independent local retained readback; never rerun application links.

use std::path::{Component, PathBuf};
use std::time::Instant;

use aros_common::{CancellationToken, Sha256Digest};

use super::{
    checked_directory, compiler_root, encode, error, preflight, proof_record, proof_snapshot,
    read_regular_bounded, remaining, sha256_bytes, CompletedLlvmSdkLinks, LinkRecord,
    LlvmSdkApplicationPlanRequest, LlvmSdkLinkRequest, RelocatedSdk,
};
use crate::compatibility::llvm_sdk_application_plan;
use crate::{canonical, ContractError};

/// Reconstruct a local LLVM SDK proof from independently selected inputs.
///
/// The expected receipt digest must come from the owning execution, not from
/// this retained receipt. Both SDK trees, compiler/source bindings, command
/// selection, logs, maps and ELF outputs are checked without running CMake,
/// application compilers, copying SDKs, creating outputs or repairing evidence.
/// Source audits and measured driver queries still run; filesystem validation
/// is not preemptible. No completed result is returned after the elapsed budget.
/// This is local consistency, not authenticated execution or portable evidence.
///
/// # Errors
/// Rejects unsafe roots, missing/changed receipts, mismatched independent
/// selection, incomplete evidence or changed trees. Failed evidence is retained.
pub fn readback_llvm_sdk_links(
    request: &LlvmSdkLinkRequest,
    expected_receipt_sha256: &Sha256Digest,
    cancellation: &CancellationToken,
) -> Result<CompletedLlvmSdkLinks, ContractError> {
    let started = Instant::now();
    preflight(request, cancellation)?;
    let output = retained_output(request)?;
    let request = LlvmSdkLinkRequest {
        output_root: output,
        sdk_root: checked_directory(&request.sdk_root, "retained original LLVM SDK")?,
        ..request.clone()
    };
    let before = proof_snapshot(&request.output_root)?;
    let receipt = request.output_root.join("receipt").join(super::RECEIPT);
    let bytes = read_regular_bounded(
        &receipt,
        "retained LLVM SDK receipt",
        canonical::MAX_DOCUMENT_BYTES,
    )?;
    if sha256_bytes(&bytes) != *expected_receipt_sha256 {
        return Err(error(
            "retained LLVM SDK receipt differs from its independent digest",
        ));
    }
    let parsed: LinkRecord = serde_json::from_slice(&bytes)
        .map_err(|_| error("retained LLVM SDK receipt is not closed JSON"))?;
    let relocated = RelocatedSdk::readback(
        &request.sdk_root,
        &request.output_root.join("relocated-sdk"),
    )?;
    let plan = llvm_sdk_application_plan::readback_plan(
        &LlvmSdkApplicationPlanRequest {
            source: request.source.clone(),
            drivers: request.drivers.clone(),
            original_sdk: relocated.original.clone(),
            relocated_sdk: relocated.relocated.clone(),
            output_root: request.output_root.join("links"),
        },
        cancellation,
    )?;
    let record = proof_record(&request, &plan, &relocated, true)?;
    if parsed != record || bytes != encode(&record)? {
        return Err(error(
            "retained LLVM SDK receipt differs from independent readback",
        ));
    }
    let completed = CompletedLlvmSdkLinks {
        request,
        plan,
        relocated,
        record,
        receipt_sha256: expected_receipt_sha256.clone(),
        snapshot: before,
    };
    completed.revalidate(cancellation)?;
    remaining(started, completed.request.timeout)?;
    Ok(completed)
}

fn retained_output(request: &LlvmSdkLinkRequest) -> Result<PathBuf, ContractError> {
    let output = &request.output_root;
    if !output.is_absolute()
        || output
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(error(
            "retained LLVM SDK proof root is not an absolute non-traversing leaf",
        ));
    }
    // Match execution's canonical parent spelling (e.g. macOS /var versus
    // /private/var), but never follow a symbolic-link proof leaf. Exact command
    // and receipt digests below bind the resulting selected canonical paths.
    let metadata = std::fs::symlink_metadata(output)
        .map_err(|_| error("retained LLVM SDK proof root is absent"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error(
            "retained LLVM SDK proof root is not a real directory",
        ));
    }
    let parent = checked_directory(
        output
            .parent()
            .ok_or_else(|| error("retained LLVM SDK proof parent is absent"))?,
        "retained LLVM SDK proof parent",
    )?;
    let output = parent.join(
        output
            .file_name()
            .ok_or_else(|| error("retained LLVM SDK proof leaf is absent"))?,
    );
    if checked_directory(&output, "retained LLVM SDK proof")? != output {
        return Err(error("retained LLVM SDK proof root is redirected"));
    }
    let preparation = crate::compatibility::validate_preparation(&request.preparation)?;
    let compiler = compiler_root(&request.drivers)?;
    let sdk = checked_directory(&request.sdk_root, "retained original LLVM SDK")?;
    for input in [
        &sdk,
        &compiler,
        &preparation.source,
        &preparation.engine,
        &preparation.helpers,
        request.source.original_source_root(),
    ] {
        if output.starts_with(input) || input.starts_with(&output) {
            return Err(error(
                "retained LLVM SDK proof overlaps an independent input",
            ));
        }
    }
    Ok(output)
}
