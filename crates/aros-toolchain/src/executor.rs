//! Local-only native producer execution entry point.
//!
//! The Rust lifecycle is the sole local build implementation. It accepts
//! explicit inputs, keeps process control bounded, and produces only a local
//! candidate; it has no release-attestation or publication authority.

use std::path::PathBuf;

use aros_common::CancellationToken;
use serde::Serialize;

use crate::plan::Identity;
use crate::ContractError;

/// Explicit inputs for one local native build.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    /// Producer profile name.
    pub preset: String,
    /// Self-digesting recipe-v2 JSON.
    pub recipe: PathBuf,
    /// AROS source checkout.
    pub source_dir: PathBuf,
    /// Reviewed producer checkout.
    pub producer_dir: PathBuf,
    /// Exact tools/collector checkout selected by the recipe.
    pub tools_dir: PathBuf,
    /// Fresh work root; an explicit resume must name its exact retained leaf.
    pub work_dir: PathBuf,
    /// Fresh output root; an explicit resume must name its exact retained leaf.
    pub output_dir: PathBuf,
    /// Existing prepared source cache. It is verified by the producer.
    pub cache_dir: PathBuf,
    /// Positive bounded producer parallelism.
    pub jobs: u64,
    /// Whole operation deadline.
    pub timeout_seconds: u64,
    /// Native execution requires prepared offline inputs.
    pub offline: bool,
    /// Explicit local candidate identifier; it is never a publication target.
    pub release_id: String,
    /// Exact frontend executable exposing the private MetaMake fetch bridge.
    pub fetch_bridge: Option<PathBuf>,
    /// Explicitly re-enter one verified native phase boundary.
    pub resume_from: Option<ResumePhase>,
}

/// A reviewed native lifecycle boundary eligible for explicit local resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePhase {
    /// Re-run the collector after a complete, revalidated compiler phase.
    Compiler,
}

/// Versioned result for one completed local candidate.
#[derive(Debug, Serialize)]
pub struct BuildResult {
    /// Result schema.
    pub schema: &'static str,
    /// Completed operation.
    pub operation: &'static str,
    /// Identity and executor observation.
    pub identity: Identity,
    /// Canonical owned output root.
    pub output_root: PathBuf,
    /// Measured output files.
    pub outputs: Vec<Output>,
    /// Explicit readiness and verification claims.
    pub evidence: Vec<Evidence>,
    /// Measured host tools used by the sanitized child environment.
    #[serde(skip_serializing)]
    pub environment: Vec<ToolObservation>,
    /// Local candidates never qualify as releases.
    pub qualification: &'static str,
    /// Output was durably produced, not merely planned.
    pub commit_state: &'static str,
}

/// Measured regular output file relative to the owned output root.
#[derive(Debug, Serialize)]
pub struct Output {
    /// Safe relative path.
    pub path: String,
    /// Output representation within the selected phase/result root.
    pub kind: &'static str,
    /// File SHA-256.
    pub sha256: aros_common::Sha256Digest,
    /// Measured byte length.
    pub size: u64,
}

/// One independently reviewable build claim.
#[derive(Debug, Serialize)]
pub struct Evidence {
    /// Stable check name.
    pub check: &'static str,
    /// Check result.
    pub status: &'static str,
    /// Optional digest of a persisted report. None means no attestation.
    pub report_sha256: Option<aros_common::Sha256Digest>,
}

/// A measured host-tool observation used by the native environment receipt.
#[derive(Debug, Clone)]
pub struct ToolObservation {
    /// Stable tool name.
    pub name: &'static str,
    /// Measured version text.
    pub version: String,
}

/// Execute one local native build with cancellation and bounded output.
///
/// # Errors
///
/// Returns a typed contract error when native preflight, snapshotting,
/// execution, cancellation, timeout handling, output measurement, or owned-root
/// release fails. Retained work and output material is never deleted on failure.
pub fn run(
    request: &BuildRequest,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    #[cfg(unix)]
    return crate::native_lifecycle::run(request, cancellation);
    #[cfg(not(unix))]
    {
        let _ = (request, cancellation);
        Err(ContractError::state(
            "native toolchain lifecycle requires a supported Unix host",
        ))
    }
}
