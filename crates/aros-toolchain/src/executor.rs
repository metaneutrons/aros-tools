//! Local-only producer execution dispatch.
//!
//! Native execution is a controlled M3 lifecycle. The retained M1 adapter is
//! selected only through explicit `legacy-preview` and remains a historical
//! diagnostic path. Both use explicit inputs and bounded process control; a
//! successful local run is never a release attestation or publication.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::{
    exit_signal, run_output_with_input_and_control, sha256_file, CancellationToken,
    DiagnosticContext, Sha256Digest,
};
use serde::Serialize;

use crate::plan::{self, Backend, Identity, PlanRequest};
use crate::snapshot::{LegacySourceView, SourceRole, SourceSnapshot};
use crate::workspace::RunDirectories;
use crate::{ContractError, Recipe};

const CAPTURE_LIMIT: usize = 256 * 1024;
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Explicit inputs for one local native or legacy-preview build.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    /// Backend must be selected explicitly; no fallback exists.
    pub backend: Backend,
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
    /// Existing prepared source cache.  It is verified by the producer.
    pub cache_dir: PathBuf,
    /// Positive bounded producer parallelism.
    pub jobs: u64,
    /// Whole operation deadline.
    pub timeout_seconds: u64,
    /// Both execution backends require prepared offline inputs.
    pub offline: bool,
    /// Explicit local candidate identifier; it is never a publication target.
    pub release_id: String,
    /// Exact frontend executable exposing the private MetaMake fetch bridge.
    ///
    /// Native execution requires this value. The CLI supplies its own binary;
    /// the legacy preview never reads it.
    pub fetch_bridge: Option<PathBuf>,
    /// Explicitly re-enter one verified native phase boundary.
    ///
    /// This is deliberately narrow: a failed compiler tree is never reused.
    /// Additional boundaries require their own verified recovery contract.
    pub resume_from: Option<ResumePhase>,
}

/// A reviewed native lifecycle boundary eligible for explicit local resume.
///
/// Resumption is not an incremental compiler cache. It currently permits only
/// re-running the collector after a complete, revalidated compiler phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePhase {
    /// Re-run the collector in a fresh Cargo target directory after compiler
    /// outputs and all predecessor receipts have been revalidated.
    Compiler,
}

struct PreparedInputs {
    recipe: Recipe,
    tools: Vec<ToolObservation>,
    rust_toolchain: Option<String>,
}

/// Versioned result for one completed local candidate.
#[derive(Debug, Serialize)]
pub struct BuildResult {
    /// Result schema.
    pub schema: &'static str,
    /// Completed operation.
    pub operation: &'static str,
    /// Explicit backend that completed the candidate.
    pub backend: Backend,
    /// Identity and executor observation.
    pub identity: Identity,
    /// Canonical owned output root.
    pub output_root: PathBuf,
    /// Measured output files.
    pub outputs: Vec<Output>,
    /// Explicit readiness and verification claims.
    pub evidence: Vec<Evidence>,
    /// Measured host tools used by the sanitized child environment.
    ///
    /// This is retained for the in-process caller and receipt construction;
    /// it is not an undeclared field of `aros-toolchain-result-v1`.
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
    pub sha256: Sha256Digest,
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
    /// Optional digest of a persisted report.  None means no attestation.
    pub report_sha256: Option<Sha256Digest>,
}

/// Execute one local legacy-preview build with cancellation and bounded output.
///
/// # Errors
///
/// Returns a typed contract error when preflight, snapshotting, execution,
/// cancellation, timeout handling, output measurement, or owned-root release
/// fails. Retained work and output material is never deleted on a failed run.
pub fn run(
    request: &BuildRequest,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    match request.backend {
        Backend::Native => {
            #[cfg(unix)]
            return crate::native_lifecycle::run(request, cancellation);
            #[cfg(not(unix))]
            return Err(ContractError::state(
                "native toolchain lifecycle requires a supported Unix host",
            ));
        }
        Backend::LegacyPreview => run_legacy(request, cancellation),
    }
}

fn run_legacy(
    request: &BuildRequest,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    if request.backend != Backend::LegacyPreview {
        return Err(ContractError::invalid(
            "legacy execution requires --backend legacy-preview explicitly",
        ));
    }
    if request.resume_from.is_some() {
        return Err(ContractError::invalid(
            "legacy-preview has no resumable phase boundary; select native --resume-from compiler explicitly",
        ));
    }
    if !request.offline {
        return Err(ContractError::preflight(
            "the legacy preview requires a prepared offline cache; pass --offline explicitly",
        ));
    }
    if request.release_id.is_empty() || !safe_segment(&request.release_id) {
        return Err(ContractError::invalid(
            "release-id must be one safe nonempty candidate segment",
        ));
    }
    let plan_request = PlanRequest {
        backend: request.backend,
        preset: request.preset.clone(),
        recipe: request.recipe.clone(),
        source_dir: request.source_dir.clone(),
        producer_dir: request.producer_dir.clone(),
        tools_dir: request.tools_dir.clone(),
        work_dir: Some(request.work_dir.clone()),
        output_dir: Some(request.output_dir.clone()),
        cache_dir: Some(request.cache_dir.clone()),
        jobs: Some(request.jobs),
        timeout_seconds: Some(request.timeout_seconds),
        offline: true,
    };
    let inspection_timeout = Duration::from_secs(request.timeout_seconds.min(900));
    let plan = plan::inspect_with_timeout(&plan_request, inspection_timeout)?;
    let recipe = Recipe::parse(
        &fs::read(&request.recipe).map_err(|_| ContractError::preflight("cannot read recipe"))?,
    )?;
    ensure_cache(&request.cache_dir)?;
    ensure_driver(&request.producer_dir)?;
    let rust_toolchain = selected_rust_toolchain(&request.producer_dir)?;
    let tools = check_prerequisites(cancellation, rust_toolchain.as_deref())?;
    if cancellation.is_cancelled() {
        return Err(ContractError::state(
            "toolchain build cancelled before reservation",
        ));
    }

    let operation_deadline = Instant::now()
        .checked_add(Duration::from_secs(request.timeout_seconds))
        .ok_or_else(|| ContractError::preflight("build timeout is not representable"))?;
    let owner = recipe.sha256().clone();
    let run_dirs = RunDirectories::reserve(&plan_request, &owner, cancellation)?;
    let inputs = PreparedInputs {
        recipe,
        tools,
        rust_toolchain,
    };
    run_owned(
        request,
        plan,
        &inputs,
        run_dirs,
        operation_deadline,
        cancellation,
    )
}

fn run_owned(
    request: &BuildRequest,
    mut plan: plan::Plan,
    inputs: &PreparedInputs,
    run_dirs: RunDirectories,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    let result = run_owned_inner(
        request,
        &mut plan,
        inputs,
        &run_dirs,
        deadline,
        cancellation,
    );
    let release = run_dirs.release();
    match (result, release) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(release_error)) => Err(ContractError::state(format!(
            "legacy build failed and owned-root release also failed: {error}; {release_error}"
        ))),
    }
}

fn run_owned_inner(
    request: &BuildRequest,
    plan: &mut plan::Plan,
    inputs: &PreparedInputs,
    run_dirs: &RunDirectories,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    run_dirs.revalidate(cancellation)?;
    let timeout = remaining(deadline)?;

    // Every producer input receives a fresh metadata-free snapshot and then a
    // separate shallow Git view.  No original checkout is passed to the driver.
    let source = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Source,
        &inputs.recipe,
        timeout,
        cancellation,
    )?;
    let source = LegacySourceView::prepare(source, remaining(deadline)?, cancellation)?;
    let producer = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Producer,
        &inputs.recipe,
        remaining(deadline)?,
        cancellation,
    )?;
    let producer = LegacySourceView::prepare(producer, remaining(deadline)?, cancellation)?;
    let tools_view = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Tools,
        &inputs.recipe,
        remaining(deadline)?,
        cancellation,
    )?;
    let tools_view = LegacySourceView::prepare(tools_view, remaining(deadline)?, cancellation)?;

    let source_lock = select_source_lock(producer.root(), inputs.recipe.source_lock_sha256())?;
    let profiles = producer.root().join("toolchains/profiles-v1.json");
    let driver = producer.root().join("scripts/toolchain/build-release.sh");
    let driver_work = run_dirs
        .paths()
        .work
        .as_ref()
        .ok_or_else(|| ContractError::state("owned work root is unavailable"))?
        .join("driver");
    fs::create_dir(&driver_work)
        .map_err(|_| ContractError::state("cannot create the private driver work root"))?;
    let home = driver_work.join("home");
    let tmp = driver_work.join("tmp");
    fs::create_dir(&home)
        .map_err(|_| ContractError::state("cannot create the private driver HOME"))?;
    fs::create_dir(&tmp)
        .map_err(|_| ContractError::state("cannot create the private driver TMPDIR"))?;
    let recipe_copy = driver_work.join("recipe.json");
    fs::copy(&request.recipe, &recipe_copy)
        .map_err(|_| ContractError::state("cannot seal the recipe into the private driver root"))?;
    let source_root = source.root().to_owned();
    let producer_root = producer.root().to_owned();
    let tools_root = tools_view.root().to_owned();
    let work_root = driver_work;
    let output_root = run_dirs
        .paths()
        .output
        .as_ref()
        .ok_or_else(|| ContractError::state("owned output root is unavailable"))?
        .to_owned();
    let host = plan.identity.host.to_owned();
    let jobs = request.jobs.to_string();
    let mut command = Command::new(&driver);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .env("AROS_OFFLINE", "1")
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "never")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(&producer_root)
        .args([
            "--producer-root",
            producer_root
                .to_str()
                .ok_or_else(|| ContractError::preflight("producer view path is not UTF-8"))?,
            "--aros-source-root",
            source_root
                .to_str()
                .ok_or_else(|| ContractError::preflight("source view path is not UTF-8"))?,
            "--aros-tools-root",
            tools_root
                .to_str()
                .ok_or_else(|| ContractError::preflight("tools view path is not UTF-8"))?,
            "--work-dir",
            work_root
                .to_str()
                .ok_or_else(|| ContractError::preflight("driver work path is not UTF-8"))?,
            "--source-cache",
            request
                .cache_dir
                .to_str()
                .ok_or_else(|| ContractError::preflight("cache path is not UTF-8"))?,
            "--recipe",
            recipe_copy
                .to_str()
                .ok_or_else(|| ContractError::preflight("recipe path is not UTF-8"))?,
            "--lock",
            source_lock
                .to_str()
                .ok_or_else(|| ContractError::preflight("source-lock path is not UTF-8"))?,
            "--profiles",
            profiles
                .to_str()
                .ok_or_else(|| ContractError::preflight("profiles path is not UTF-8"))?,
            "--profile",
            &request.preset,
            "--host",
            &host,
            "--release-id",
            &request.release_id,
            "--output-dir",
            output_root
                .to_str()
                .ok_or_else(|| ContractError::preflight("output path is not UTF-8"))?,
            "--jobs",
            &jobs,
        ]);
    if let Some(rust_toolchain) = inputs.rust_toolchain.as_deref() {
        command.env("RUSTUP_TOOLCHAIN", rust_toolchain);
    }
    let output = run_output_with_input_and_control(
        &mut command,
        &[],
        CAPTURE_LIMIT,
        remaining(deadline)?,
        cancellation,
    )
    .map_err(|error| process_error(&driver, &error, cancellation))?;
    let (stdout_report, stderr_report) = persist_process_output(&work_root, &output)?;
    if output.cancelled || cancellation.is_cancelled() {
        return Err(ContractError::state(
            "legacy producer cancelled; owned material and outputs are retained",
        ));
    }
    if output.timed_out {
        return Err(ContractError::state("legacy producer exceeded the explicit build deadline; owned material and outputs are retained").context(DiagnosticContext {
            tool: Some("build-release.sh".into()),
            timed_out: Some(true),
            timeout_ms: Some(request.timeout_seconds.saturating_mul(1000)),
            ..DiagnosticContext::default()
        }));
    }
    if !output.status.success() {
        return Err(ContractError::state(format!(
            "legacy producer exited unsuccessfully (exit code {:?}); inspect retained build material",
            output.status.code()
        )).context(DiagnosticContext {
            tool: Some("build-release.sh".into()),
            exit_code: output.status.code(),
            signal: exit_signal(output.status),
            ..DiagnosticContext::default()
        }));
    }
    run_dirs.revalidate(cancellation)?;
    let outputs = collect_outputs(&output_root)?;
    if outputs.is_empty() {
        return Err(ContractError::state(
            "legacy producer returned success without regular output files",
        ));
    }
    // The local executor observation is explicit and does not become release
    // provenance.  The executable is measured, and source identity is checked,
    // but no signed/trusted origin attestation is invented here.
    plan.identity.executor.tools_commit = Some(plan.identity.tools_commit.clone());
    Ok(BuildResult {
        schema: "aros-toolchain-result-v1",
        operation: "build",
        backend: request.backend,
        identity: plan.identity.clone(),
        output_root,
        outputs,
        evidence: vec![
            Evidence {
                check: "prerequisites",
                status: "passed",
                report_sha256: None,
            },
            Evidence {
                check: "cache",
                status: "passed",
                report_sha256: None,
            },
            Evidence {
                check: "environment",
                status: "passed",
                report_sha256: Some(stdout_report),
            },
            Evidence {
                check: "integrity",
                status: "passed",
                report_sha256: Some(stderr_report),
            },
            Evidence {
                check: "origin",
                status: "not-run",
                report_sha256: None,
            },
        ],
        environment: inputs.tools.clone(),
        qualification: "local-only",
        commit_state: "committed",
    })
}

fn persist_process_output(
    work_root: &Path,
    output: &aros_common::ProcessOutput,
) -> Result<(Sha256Digest, Sha256Digest), ContractError> {
    let stdout_path = work_root.join("producer-stdout.log");
    let stderr_path = work_root.join("producer-stderr.log");
    let mut stdout = fs::File::create(&stdout_path)
        .map_err(|_| ContractError::state("cannot persist producer stdout report"))?;
    output
        .stdout
        .write_rendered(&mut stdout)
        .map_err(|_| ContractError::state("cannot write producer stdout report"))?;
    stdout
        .flush()
        .map_err(|_| ContractError::state("cannot flush producer stdout report"))?;
    let mut stderr = fs::File::create(&stderr_path)
        .map_err(|_| ContractError::state("cannot persist producer stderr report"))?;
    output
        .stderr
        .write_rendered(&mut stderr)
        .map_err(|_| ContractError::state("cannot write producer stderr report"))?;
    stderr
        .flush()
        .map_err(|_| ContractError::state("cannot flush producer stderr report"))?;
    let stdout_digest = sha256_file(&stdout_path)
        .map_err(|_| ContractError::state("cannot hash producer stdout report"))?
        .digest;
    let stderr_digest = sha256_file(&stderr_path)
        .map_err(|_| ContractError::state("cannot hash producer stderr report"))?
        .digest;
    Ok((stdout_digest, stderr_digest))
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolObservation {
    /// Executable name.
    pub name: &'static str,
    /// First version line returned by the executable.
    pub version: String,
}

fn check_prerequisites(
    cancellation: &CancellationToken,
    rust_toolchain: Option<&str>,
) -> Result<Vec<ToolObservation>, ContractError> {
    let mut tools = Vec::new();
    for name in ["git", "python3", "cmake", "rustc", "cargo"] {
        tools.push(check_tool(name, cancellation, rust_toolchain)?);
    }
    let make = if check_tool("gmake", cancellation, None).is_ok() {
        "gmake"
    } else {
        "make"
    };
    tools.push(check_tool(make, cancellation, None)?);
    Ok(tools)
}

fn check_tool(
    name: &'static str,
    cancellation: &CancellationToken,
    rust_toolchain: Option<&str>,
) -> Result<ToolObservation, ContractError> {
    let mut command = Command::new(name);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .arg("--version");
    if matches!(name, "rustc" | "cargo") {
        if let Some(rust_toolchain) = rust_toolchain {
            command.env("RUSTUP_TOOLCHAIN", rust_toolchain);
        }
    }
    let output =
        run_output_with_input_and_control(&mut command, &[], 16 * 1024, TOOL_TIMEOUT, cancellation)
            .map_err(|error| {
                if cancellation.is_cancelled() || error.kind() == std::io::ErrorKind::Interrupted {
                    ContractError::state("toolchain prerequisite probe cancelled")
                } else {
                    ContractError::prerequisite(format!(
                        "required host tool '{name}' could not be started"
                    ))
                }
            })?;
    if output.cancelled || cancellation.is_cancelled() {
        return Err(ContractError::state(
            "toolchain prerequisite probe cancelled",
        ));
    }
    if output.timed_out || !output.status.success() {
        return Err(ContractError::prerequisite(format!(
            "required host tool '{name}' did not provide a usable version"
        )));
    }
    let version = output
        .stdout
        .exact_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .unwrap_or("unavailable")
        .to_owned();
    Ok(ToolObservation { name, version })
}

fn selected_rust_toolchain(root: &Path) -> Result<Option<String>, ContractError> {
    let path = root.join("toolchains/rust-toolchain.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(ContractError::prerequisite(
                "producer Rust toolchain file is unreadable",
            ))
        }
    };
    let channel = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("channel = "))
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
        .map(str::to_owned);
    match channel {
        Some(channel) if safe_segment(&channel) => Ok(Some(channel)),
        Some(_) => Err(ContractError::identity(
            "producer Rust toolchain channel is not a safe selection",
        )),
        None => Err(ContractError::identity(
            "producer Rust toolchain file has no channel",
        )),
    }
}

fn ensure_cache(path: &Path) -> Result<(), ContractError> {
    if !path.is_dir() {
        return Err(ContractError::preflight(
            "source cache must be an existing directory prepared before the offline build",
        ));
    }
    Ok(())
}

fn ensure_driver(path: &Path) -> Result<(), ContractError> {
    let driver = path.join("scripts/toolchain/build-release.sh");
    let metadata = fs::metadata(&driver).map_err(|_| {
        ContractError::prerequisite("selected producer has no readable build-release.sh")
    })?;
    if !metadata.is_file() {
        return Err(ContractError::prerequisite(
            "selected producer build driver is not a regular file",
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(ContractError::prerequisite(
            "selected producer build driver is not executable",
        ));
    }
    Ok(())
}

fn select_source_lock(root: &Path, expected: &Sha256Digest) -> Result<PathBuf, ContractError> {
    let directory = root.join("toolchains");
    let mut matches = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|_| ContractError::prerequisite("producer toolchains directory is unreadable"))?
    {
        let entry = entry
            .map_err(|_| ContractError::prerequisite("producer toolchains entry is unreadable"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json")
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.ends_with(".sources.json"))
            && sha256_file(&path).is_ok_and(|result| result.digest == *expected)
        {
            matches.push(path);
        }
    }
    if matches.len() != 1 {
        return Err(ContractError::identity(
            "producer source lock selection is not unique by recipe digest",
        ));
    }
    Ok(matches.remove(0))
}

fn collect_outputs(root: &Path) -> Result<Vec<Output>, ContractError> {
    let mut files = Vec::new();
    collect_outputs_inner(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_outputs_inner(
    root: &Path,
    current: &Path,
    output: &mut Vec<Output>,
) -> Result<(), ContractError> {
    for entry in fs::read_dir(current)
        .map_err(|_| ContractError::state("cannot enumerate retained producer output"))?
    {
        let entry = entry
            .map_err(|_| ContractError::state("cannot read retained producer output entry"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ContractError::state("cannot inspect retained producer output entry"))?;
        if metadata.is_dir() {
            collect_outputs_inner(root, &path, output)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| ContractError::state("producer output escaped its owned root"))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| ContractError::state("producer output path is not UTF-8"))?;
            if relative.is_empty()
                || relative
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(ContractError::state("producer output path is unsafe"));
            }
            if relative == ".aros-toolchain-owner-v1.json" {
                continue;
            }
            let measured = sha256_file(&path)
                .map_err(|_| ContractError::state("cannot hash retained producer output"))?;
            output.push(Output {
                path: relative.replace(std::path::MAIN_SEPARATOR, "/"),
                kind: "file",
                sha256: measured.digest,
                size: measured.size,
            });
        } else {
            return Err(ContractError::state(
                "producer output contains a non-regular file",
            ));
        }
    }
    Ok(())
}

fn remaining(deadline: Instant) -> Result<Duration, ContractError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            ContractError::state("legacy producer operation exceeded its explicit deadline")
        })
}

fn process_error(
    path: &Path,
    error: &std::io::Error,
    cancellation: &CancellationToken,
) -> ContractError {
    if cancellation.is_cancelled() || error.kind() == std::io::ErrorKind::Interrupted {
        ContractError::state("legacy producer cancellation or child cleanup failed")
    } else {
        ContractError::prerequisite(format!(
            "could not start or supervise producer driver '{}': {}",
            path.display(),
            error.kind()
        ))
    }
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
