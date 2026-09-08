//! Read-only producer planning, not execution authorization.
//!
//! Validates explicit identities and selected committed metadata. It never
//! runs source-provided code, fetches objects, scans/changes caches or reserves
//! output roots. Isolated snapshots and full build eligibility remain lifecycle
//! gates even for a native `ready` plan.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aros_common::{sha256_bytes, Diagnostic, DiagnosticCode, DiagnosticStage, Sha256Digest};
use serde::Serialize;

use crate::inspection::{self, Checkout};
use crate::native_declaration::NativeExecutorDeclaration;
use crate::profiles::identifier;
use crate::recipe::GitObjectId;
use crate::{ContractError, Recipe};

/// Explicit arguments, resolved against the invocation directory only.
#[derive(Debug)]
pub struct PlanRequest {
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
    /// Optional work root; planning never reserves it.
    pub work_dir: Option<PathBuf>,
    /// Optional candidate root; planning never reserves it.
    pub output_dir: Option<PathBuf>,
    /// Optional source cache; planning never scans or changes it.
    pub cache_dir: Option<PathBuf>,
    /// Build job budget, never an inferred default.
    pub jobs: Option<u64>,
    /// Whole-build deadline, not the bounded inspection timeout.
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
    /// Checked recipe identity claims and explicitly unknown executor origin.
    pub identity: Identity,
    /// Canonical paths, without an ownership reservation.
    pub paths: Paths,
    /// Selected budgets and deliberately unmeasured resources.
    pub resources: Resources,
    /// Native lifecycle phases selected by the closed executor declaration.
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
    /// Native producer contract selected by the recipe-bound declaration.
    pub contract_id: Option<&'static str>,
    /// Native contract digest bound by the declaration.
    pub contract_sha256: Option<Sha256Digest>,
    /// Exact tools commit selected by the native declaration.
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
/// invalid inputs. Missing build readiness is a successful *incomplete*
/// inspection, never a successful build.
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
/// Returns a typed contract error when resources, checkout identities, recipe,
/// native executor declaration, or bounded inspection cannot be validated.
pub fn inspect_with_timeout(
    request: &PlanRequest,
    timeout: Duration,
) -> Result<Plan, ContractError> {
    if timeout.is_zero() {
        return Err(ContractError::preflight(
            "producer inspection timeout must be positive",
        ));
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
    let declaration_bytes = producer
        .required_file("toolchains/producer-executor-v1.toml")
        .map_err(|error| error.input("--producer-dir"))?;
    let declaration = NativeExecutorDeclaration::parse(&declaration_bytes)
        .map_err(|error| error.input("--producer-dir"))?;
    let contract = tools
        .required_file(declaration.contract_path())
        .map_err(|error| error.input("--tools-dir"))?;
    let source_lock = producer
        .required_file(declaration.source_lock_path())
        .map_err(|error| error.input("--producer-dir"))?;
    let profiles = producer
        .required_file(declaration.profiles_path())
        .map_err(|error| error.input("--producer-dir"))?;
    declaration
        .bind(&recipe, &contract, &source_lock, &profiles, &request.preset)
        .map_err(|error| error.input("--producer-dir"))?;
    let executor = Executor {
        contract_id: Some("aros-toolchain-producer-v1"),
        contract_sha256: Some(declaration.contract_sha256().clone()),
        tools_commit: Some(declaration.tools_commit().clone()),
        binary_sha256: inspection::frontend_digest()?,
        origin_evidence_sha256: None,
    };
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
    let findings = findings(request);
    source.recheck()?;
    producer.recheck()?;
    tools.recheck()?;
    Ok(Plan {
        schema: "aros-toolchain-plan-v1",
        operation: "plan",
        identity: Identity {
            recipe_sha256: recipe.sha256().clone(),
            source_commit: recipe.source().0.clone(),
            producer_commit: recipe.producer().0.clone(),
            tools_commit: recipe.tools().0.clone(),
            host,
            target_profile: request.preset.clone(),
            executor,
        },
        paths,
        resources: Resources {
            jobs: request.jobs,
            timeout_seconds: request.timeout_seconds,
            offline: request.offline,
            network_isolation: "fetch-guard",
            free_bytes: None,
        },
        steps: steps(),
        readiness: readiness(request),
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

fn findings(request: &PlanRequest) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    findings.push(Diagnostic::warning(
        DiagnosticCode::ProducerIdentity,
        DiagnosticStage::Configuration,
        "Native local execution has no trusted executor-origin attestation and remains local-only.",
    ).with_hint("A protected release workflow must bind independent executor-origin evidence; a local result cannot publish."));
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
    if !request.offline {
        findings.push(Diagnostic::warning(
            DiagnosticCode::ProducerPreflight,
            DiagnosticStage::Configuration,
            "Native execution requires the explicit offline policy.",
        ).with_hint("Pass --offline after preparing the verified source cache; the lifecycle never falls back to producer-controlled network access."));
    }
    findings
}

fn steps() -> Vec<&'static str> {
    vec![
        "preflight",
        "sources",
        "environment",
        "configure",
        "compiler",
        "collector",
    ]
}

const fn readiness(request: &PlanRequest) -> &'static str {
    if request.work_dir.is_none()
        || request.output_dir.is_none()
        || request.cache_dir.is_none()
        || request.jobs.is_none()
        || request.timeout_seconds.is_none()
        || !request.offline
    {
        "incomplete"
    } else {
        "ready"
    }
}
