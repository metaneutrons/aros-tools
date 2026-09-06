//! Experimental read-only legacy planning, not execution authorization.
//!
//! Validates explicit identities and selected committed metadata. It never
//! runs source-provided code, fetches objects, scans/changes caches or reserves
//! output roots. Isolated snapshots and build eligibility remain M1/M2 gates.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aros_common::{sha256_bytes, Diagnostic, DiagnosticCode, DiagnosticStage, Sha256Digest};
use serde::Serialize;

use crate::inspection::{self, Checkout};
use crate::profiles::{identifier, Profiles};
use crate::recipe::GitObjectId;
use crate::{ContractError, Recipe};

/// Explicit producer implementation choice; no automatic fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// Reserved native path: fails before inspecting or mutating inputs.
    Native,
    /// Read-only preview of the selected historical producer inputs.
    LegacyPreview,
}

/// Explicit arguments, resolved against the invocation directory only.
#[derive(Debug)]
pub struct PlanRequest {
    /// Chosen backend; callers must not infer a fallback.
    pub backend: Backend,
    /// Producer-owned profile name.
    pub preset: String,
    /// Recipe-v2 regular file.
    pub recipe: PathBuf,
    /// Exact source checkout root.
    pub source_dir: PathBuf,
    /// Exact producer checkout root.
    pub producer_dir: PathBuf,
    /// Exact recipe-selected tools/collector root, not the frontend source.
    pub tools_dir: PathBuf,
    /// Optional future work root; never reserved.
    pub work_dir: Option<PathBuf>,
    /// Optional future candidate root; never reserved.
    pub output_dir: Option<PathBuf>,
    /// Optional future source cache; never scanned or changed by this slice.
    pub cache_dir: Option<PathBuf>,
    /// Future build job budget, never an inferred default.
    pub jobs: Option<u64>,
    /// Future whole-build deadline, not the bounded inspection timeout.
    pub timeout_seconds: Option<u64>,
    /// Effective frontend offline policy. Planning is always offline.
    pub offline: bool,
}

/// A non-authorizing inspection result in the frozen plan-v1 envelope.
#[derive(Debug, Serialize)]
pub struct Plan {
    /// Versioned schema.
    pub schema: &'static str,
    /// Always `plan`.
    pub operation: &'static str,
    /// Actual inspected backend.
    pub backend: Backend,
    /// Checked recipe identity claims and explicitly unknown executor origin.
    pub identity: Identity,
    /// Canonical paths, without an ownership reservation.
    pub paths: Paths,
    /// Selected budgets and deliberately unmeasured resources.
    pub resources: Resources,
    /// Honest coarse legacy phase; no inferred internal stage names.
    pub steps: Vec<&'static str>,
    /// This slice always blocks execution pending the remaining safety gates.
    pub readiness: &'static str,
    /// Shared diagnostic records explaining every omitted gate.
    pub findings: Vec<Diagnostic>,
}

/// Recipe and frontend identity are intentionally distinct.
#[derive(Debug, Serialize, Clone)]
pub struct Identity {
    /// Verified recipe self-digest.
    pub recipe_sha256: Sha256Digest,
    /// Observed source commit, checked against its recipe tree.
    pub source_commit: GitObjectId,
    /// Observed producer commit, checked against its recipe tree.
    pub producer_commit: GitObjectId,
    /// Recipe-selected collector/tools commit, not the running frontend.
    pub tools_commit: GitObjectId,
    /// Native host from the shared host mapping.
    pub host: &'static str,
    /// Name resolved through the selected producer profile document.
    pub target_profile: String,
    /// Inspection identity, never an attestation or a guessed source commit.
    pub executor: Executor,
}

/// Unverified frontend file observation, insufficient for execution/release.
#[derive(Debug, Serialize, Clone)]
pub struct Executor {
    /// Native contract is not selected by a legacy recipe.
    pub contract_id: Option<&'static str>,
    /// Native contract digest is unavailable for this legacy preview.
    pub contract_sha256: Option<Sha256Digest>,
    /// Null only in blocked read-only plans lacking verifiable build metadata.
    pub tools_commit: Option<GitObjectId>,
    /// Measured on-disk frontend file; does not establish in-memory origin.
    pub binary_sha256: Sha256Digest,
    /// Never synthesized from a source commit or binary hash.
    pub origin_evidence_sha256: Option<Sha256Digest>,
}

/// Non-overlapping canonical root selections.
#[derive(Debug, Serialize)]
pub struct Paths {
    /// Source checkout.
    pub source: PathBuf,
    /// Producer checkout.
    pub producer: PathBuf,
    /// Collector/tools checkout.
    pub tools: PathBuf,
    /// Proposed work directory.
    pub work: Option<PathBuf>,
    /// Proposed candidate directory.
    pub output: Option<PathBuf>,
    /// Proposed cache directory.
    pub cache: Option<PathBuf>,
}

/// Observed selections, not invented build estimates.
#[derive(Debug, Serialize)]
pub struct Resources {
    /// Explicit future build jobs.
    pub jobs: Option<u64>,
    /// Explicit future build deadline.
    pub timeout_seconds: Option<u64>,
    /// Effective frontend policy; inspection itself never uses the network.
    pub offline: bool,
    /// Planned legacy policy, not an active or proven OS sandbox.
    pub network_isolation: &'static str,
    /// This slice does not measure storage capacity.
    pub free_bytes: Option<u64>,
}

/// Inspect selected committed inputs without executing any producer code.
///
/// # Errors
/// Returns structured contract/identity/preflight/prerequisite diagnostics for
/// invalid inputs. Missing build readiness is a successful *blocked inspection*,
/// never a successful build. Native selection fails before all filesystem reads.
pub fn inspect(request: &PlanRequest) -> Result<Plan, ContractError> {
    inspect_with_timeout(request, Duration::from_secs(60))
}

/// Inspect explicit inputs with a caller-selected bounded read-only budget.
///
/// The public `plan` command intentionally retains its 60-second contract.
/// The local build adapter may select a larger, still finite budget because a
/// complete AROS checkout can contain substantially more Git material than a
/// normal inspection fixture.
///
/// # Errors
///
/// Returns a typed contract error when the selected backend, resources,
/// checkout identities, recipe, or bounded inspection cannot be validated.
pub fn inspect_with_timeout(
    request: &PlanRequest,
    timeout: Duration,
) -> Result<Plan, ContractError> {
    if timeout.is_zero() {
        return Err(ContractError::preflight(
            "producer inspection timeout must be positive",
        ));
    }
    if request.backend != Backend::LegacyPreview {
        return Err(ContractError::invalid("native toolchain planning is not implemented; select --backend legacy-preview explicitly for experimental read-only inspection"));
    }
    validate_resources(request)?;
    let host = aros_common::target::native_host_key().ok_or_else(|| {
        ContractError::preflight("producer inspection is unsupported on this native host")
    })?;
    let paths = resolve_paths(request)?;
    let bytes = inspection::read(&request.recipe).map_err(|error| error.input("--recipe"))?;
    let recipe = Recipe::parse(&bytes).map_err(|error| error.input("--recipe"))?;
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        ContractError::preflight("producer inspection timeout is not representable")
    })?;
    let source = Checkout::inspect(&paths.source, recipe.source(), deadline)
        .map_err(|error| error.input("--source-dir"))?;
    let producer = Checkout::inspect(&paths.producer, recipe.producer(), deadline)
        .map_err(|error| error.input("--producer-dir"))?;
    let tools = Checkout::inspect(&paths.tools, recipe.tools(), deadline)
        .map_err(|error| error.input("--tools-dir"))?;
    let profiles = producer
        .required_file("toolchains/profiles-v1.json")
        .map_err(|error| error.input("--producer-dir"))?;
    if sha256_bytes(&profiles) != *recipe.profiles_sha256() {
        return Err(ContractError::identity(
            "selected profiles differ from the recipe digest",
        ));
    }
    Profiles::parse(&profiles)?.select(&request.preset)?;
    producer
        .source_lock(recipe.source_lock_sha256())
        .map_err(|error| error.input("--producer-dir"))?;
    // M2 owns source-lock semantics and completeness. Here only the exact
    // recipe-declared patch identities are checked, never applied.
    for patch in recipe.patches() {
        if sha256_bytes(
            &source
                .required_file(patch.path())
                .map_err(|error| error.input("--source-dir"))?,
        ) != *patch.sha256()
        {
            return Err(ContractError::identity(
                "committed source patch differs from its recipe digest",
            ));
        }
    }
    let driver_present = producer
        .file("scripts/toolchain/build-release.sh")
        .map_err(|error| error.input("--producer-dir"))?
        .is_some();
    #[cfg(unix)]
    {
        let mut budget = crate::source_audit::Budget::new(deadline);
        for (checkout, label) in [
            (&source, "--source-dir"),
            (&producer, "--producer-dir"),
            (&tools, "--tools-dir"),
        ] {
            crate::source_audit::verify(checkout, &mut budget, 0)
                .map_err(|error| error.input(label))?;
        }
    }
    let mut findings = findings(request, driver_present);
    findings.push(Diagnostic::error(
        DiagnosticCode::ProducerIdentity, DiagnosticStage::Configuration,
        "Frontend source/origin evidence is unavailable; executor.tools_commit is null, not the recipe's collector commit.",
    ).with_hint("Use this result only for inspection; verified executor identity remains required before any build can launch."));
    source.recheck()?;
    producer.recheck()?;
    tools.recheck()?;
    Ok(Plan {
        schema: "aros-toolchain-plan-v1",
        operation: "plan",
        backend: request.backend,
        identity: Identity {
            recipe_sha256: recipe.sha256().clone(),
            source_commit: recipe.source().0.clone(),
            producer_commit: recipe.producer().0.clone(),
            tools_commit: recipe.tools().0.clone(),
            host,
            target_profile: request.preset.clone(),
            executor: Executor {
                contract_id: None,
                contract_sha256: None,
                tools_commit: None,
                binary_sha256: inspection::frontend_digest()?,
                origin_evidence_sha256: None,
            },
        },
        paths,
        resources: Resources {
            jobs: request.jobs,
            timeout_seconds: request.timeout_seconds,
            offline: request.offline,
            network_isolation: "fetch-guard",
            free_bytes: None,
        },
        steps: vec!["legacy-driver"],
        readiness: "blocked",
        findings,
    })
}

pub(crate) fn validate_resources(request: &PlanRequest) -> Result<(), ContractError> {
    if request.jobs == Some(0)
        || request.timeout_seconds == Some(0)
        || request
            .jobs
            .is_some_and(|jobs| usize::try_from(jobs).is_err())
        || request.timeout_seconds.is_some_and(|seconds| {
            Instant::now()
                .checked_add(Duration::from_secs(seconds))
                .is_none()
        })
    {
        return Err(ContractError::preflight(
            "jobs and timeout must be positive and representable on this host",
        ));
    }
    if !identifier(&request.preset) {
        return Err(ContractError::invalid(
            "preset must be a safe producer profile identifier",
        ));
    }
    Ok(())
}

pub(crate) fn resolve_paths(request: &PlanRequest) -> Result<Paths, ContractError> {
    let paths = Paths {
        source: inspection::directory(&request.source_dir)
            .map_err(|error| error.input("--source-dir"))?,
        producer: inspection::directory(&request.producer_dir)
            .map_err(|error| error.input("--producer-dir"))?,
        tools: inspection::directory(&request.tools_dir)
            .map_err(|error| error.input("--tools-dir"))?,
        work: request
            .work_dir
            .as_deref()
            .map(inspection::destination)
            .transpose()?,
        output: request
            .output_dir
            .as_deref()
            .map(inspection::destination)
            .transpose()?,
        cache: request
            .cache_dir
            .as_deref()
            .map(inspection::destination)
            .transpose()?,
    };
    let roots: Vec<_> = [
        Some(&paths.source),
        Some(&paths.producer),
        Some(&paths.tools),
        paths.work.as_ref(),
        paths.output.as_ref(),
        paths.cache.as_ref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    for (index, root) in roots.iter().enumerate() {
        if roots[index + 1..]
            .iter()
            .any(|other| root.starts_with(other) || other.starts_with(root))
        {
            return Err(ContractError::preflight(
                "source, producer, tools, work, output and cache roots must not overlap or alias",
            ));
        }
    }
    Ok(paths)
}

fn findings(request: &PlanRequest, driver_present: bool) -> Vec<Diagnostic> {
    let mut findings = vec![Diagnostic::error(
        DiagnosticCode::ProducerContract, DiagnosticStage::Configuration,
        "Build execution is not implemented. Recursive raw worktree/index checks do not create isolated execution snapshots. Source capabilities, source-lock semantics, prerequisites, cache integrity, integrated ownership and cancellation remain unqualified.",
    ).with_hint("Do not execute this plan as a build authorization. Continue TCP-M1/M2; no cache was scanned, no prerequisite installed and no directory reserved.")];
    if !driver_present {
        findings.push(Diagnostic::error(DiagnosticCode::ProducerContract, DiagnosticStage::Configuration,
            "The selected producer has no committed legacy build driver.")
            .with_hint("Select a compatible reviewed producer recipe; no fallback or source switch is performed."));
    }
    if request.work_dir.is_none()
        || request.output_dir.is_none()
        || request.cache_dir.is_none()
        || request.jobs.is_none()
        || request.timeout_seconds.is_none()
    {
        findings.push(Diagnostic::warning(DiagnosticCode::ProducerPreflight, DiagnosticStage::Configuration,
            "Work/output/cache roots or explicit resource budgets are incomplete.")
            .with_hint("Select --work-dir, --output-dir, --cache-dir, --jobs and --timeout-seconds; this does not remove the other blockers."));
    }
    findings
}
