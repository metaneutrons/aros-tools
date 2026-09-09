//! Controlled native lifecycle for one local AROS toolchain candidate.
//!
//! The Rust layer owns lifecycle state, input binding, environment selection,
//! process supervision and receipts. `configure`, MetaMake and the
//! `crosstools-release` target remain unmodified source-owned programs. This
//! module has no packaging, release-publication or release-attestation
//! capability. It does durably publish a completed *local* candidate into its
//! reserved output root; that operation cannot create, modify or promote a
//! release asset.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::{
    exit_signal, measure_tree_content_cas, publication_failure_class,
    publish_prepared_tree_noclobber, run_output_with_input_and_control, sha256_bytes, sha256_file,
    CancellationToken, DiagnosticContext, PublicationFailureClass, Sha256Digest,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cargo_vendor::CargoVendorEnvironment;
use crate::executor::{BuildRequest, BuildResult, Evidence, Output, ResumePhase, ToolObservation};
use crate::metamake_fetch::SourceUseLedger;
use crate::native_declaration::NativeExecutorDeclaration;
use crate::plan::{self, Identity, PlanRequest};
use crate::preflight::{self, HostPreflight};
use crate::producer_environment::{ProducerEnvironment, ReproducibilityRoots};
use crate::python_environment::{PythonEnvironment, PythonInterpreter};
use crate::snapshot::{SourceRole, SourceSnapshot};
use crate::source_cache;
use crate::workspace::RunDirectories;
use crate::{canonical, ContractError, Recipe};

const CAPTURE_LIMIT: usize = 256 * 1024;
const CONTRACT_PATH: &str = "toolchains/producer-executor-v1.toml";
const STAGING_PREFIX: &str = ".aros-native-toolchain-stage";
const PUBLISHED_PREFIX: &str = "toolchain";

/// Execute the controlled native lifecycle for one local-only candidate.
///
/// Every phase is bounded by the single caller deadline. Failure retains all
/// owned roots and all phase logs; no phase may silently fall back to the old
/// shell producer.
pub fn run(
    request: &BuildRequest,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    if !request.offline {
        return Err(ContractError::preflight(
            "native toolchain builds require --offline and a fully prepared verified source cache",
        ));
    }
    validate_request(request)?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(request.timeout_seconds))
        .ok_or_else(|| ContractError::preflight("build timeout is not representable"))?;
    if cancellation.is_cancelled() {
        return Err(ContractError::state(
            "native toolchain build cancelled before reservation",
        ));
    }
    let plan_request = plan_request(request);
    let inspection_timeout = remaining(deadline)?.min(Duration::from_mins(15));
    let plan = plan::inspect_with_timeout(&plan_request, inspection_timeout)?;
    let recipe = Recipe::parse(
        &fs::read(&request.recipe).map_err(|_| ContractError::preflight("cannot read recipe"))?,
    )?;
    let owner = recipe.sha256().clone();
    let run_dirs = match request.resume_from {
        None => RunDirectories::reserve(&plan_request, &owner, cancellation)?,
        Some(ResumePhase::Compiler) => RunDirectories::resume(&plan_request, &owner, cancellation)?,
    };
    let result = match request.resume_from {
        None => run_owned(request, plan, &recipe, &run_dirs, deadline, cancellation),
        Some(ResumePhase::Compiler) => {
            resume_after_compiler(request, plan, &recipe, &run_dirs, deadline, cancellation)
        }
    };
    let release = run_dirs.release();
    match (result, release) {
        (Ok(result), Ok(())) => Ok(result),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(release_error)) => Err(ContractError::state(format!(
            "native lifecycle failed and owned-root release also failed: {error}; {release_error}"
        ))),
    }
}

fn run_owned(
    request: &BuildRequest,
    mut plan: plan::Plan,
    recipe: &Recipe,
    run_dirs: &RunDirectories,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    run_dirs.revalidate(cancellation)?;
    let lifecycle = LifecyclePaths::create(run_dirs)?;

    let source = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Source,
        recipe,
        remaining(deadline)?,
        cancellation,
    )?;
    let producer = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Producer,
        recipe,
        remaining(deadline)?,
        cancellation,
    )?;
    let tools = SourceSnapshot::prepare(
        run_dirs,
        SourceRole::Tools,
        recipe,
        remaining(deadline)?,
        cancellation,
    )?;
    run_dirs.revalidate(cancellation)?;
    let snapshots = SnapshotDigests::measure(source.root(), producer.root(), tools.root())?;

    let declaration = NativeExecutorDeclaration::parse(&read_regular(
        producer.root().join(CONTRACT_PATH),
        "native executor declaration",
    )?)?;
    let contract = read_regular(
        tools.root().join(declaration.contract_path()),
        "selected tools contract",
    )?;
    let lock_bytes = read_regular(
        producer.root().join(declaration.source_lock_path()),
        "selected source lock",
    )?;
    let profiles = read_regular(
        producer.root().join(declaration.profiles_path()),
        "selected profile matrix",
    )?;
    let bound = declaration.bind(recipe, &contract, &lock_bytes, &profiles, &request.preset)?;
    let host = preflight::inspect(bound.selected_profile())?;
    let cache = source_cache::verify(&request.cache_dir, bound.source_lock())?;
    let environment = ProducerEnvironment::prepare(
        &ReproducibilityRoots {
            source: source.root().to_owned(),
            producer: producer.root().to_owned(),
            tools: tools.root().to_owned(),
            work: lifecycle.work_root().to_owned(),
            source_cache: request.cache_dir.clone(),
        },
        &host.tool_directories(),
        recipe.source_date_epoch(),
        request.jobs,
    )?;
    let preflight_context = PhaseInputContext {
        recipe,
        declaration: &declaration,
        host: Some(&host),
        cache: Some(&cache),
        jobs: request.jobs,
        snapshots: &snapshots,
        environment: None,
    };
    let execution_context = PhaseInputContext {
        environment: Some(&environment),
        ..preflight_context
    };
    let preflight_input = phase_input("preflight", &preflight_context, None)?;
    let preflight_receipt = persist_receipt(
        &lifecycle,
        "preflight",
        &plan.identity,
        &preflight_input,
        lifecycle.work_root(),
        &[],
        None,
    )?;

    let interpreter = python_interpreter(&host)?;
    let python = PythonEnvironment::prepare_with_interpreter(
        bound.source_lock(),
        &request.cache_dir,
        &lifecycle.python,
        &interpreter,
    )?;
    let cargo = CargoVendorEnvironment::prepare(
        &request.cache_dir,
        &tools.root().join("Cargo.lock"),
        &lifecycle.cargo,
    )?;
    let environment_input =
        phase_input("environment", &execution_context, Some(&preflight_receipt))?;
    let environment_receipt = persist_receipt(
        &lifecycle,
        "environment",
        &plan.identity,
        &environment_input,
        lifecycle.work_root(),
        &[],
        Some(&preflight_receipt),
    )?;

    // Build beneath an owned private staging leaf.  The final `toolchain`
    // leaf remains absent until every producer phase and the collector have
    // completed; the durable publication boundary below is its only writer.
    let prefix = lifecycle.staging_prefix();
    fs::create_dir(&prefix)
        .map_err(|_| ContractError::state("cannot create fresh native candidate prefix"))?;
    let build = lifecycle.work_root().join("build");
    fs::create_dir(&build)
        .map_err(|_| ContractError::state("cannot create fresh native configure root"))?;

    let mut configure = Command::new(source.root().join("configure"));
    configure
        .current_dir(&build)
        .arg(format!(
            "--target={}",
            bound.selected_profile().configure_target()
        ))
        .arg("--with-toolchain=llvm")
        .arg(format!(
            "--with-llvm-version={}",
            bound.source_lock().version()
        ))
        .arg("--enable-toolchain-release")
        .arg(format!(
            "--with-portssources={}",
            request.cache_dir.display()
        ))
        .arg(format!(
            "--with-aros-toolchain-install={}",
            prefix.display()
        ));
    apply_child_environment(&mut configure, &environment, &python, &lifecycle);
    run_phase(
        "configure",
        &mut configure,
        &lifecycle,
        deadline,
        cancellation,
    )?;
    let configure_outputs = measured_files(&lifecycle.work_root().join("build"), "build")?;
    let configure_input = phase_input("configure", &execution_context, Some(&environment_receipt))?;
    let configure_receipt = persist_receipt(
        &lifecycle,
        "configure",
        &plan.identity,
        &configure_input,
        lifecycle.work_root(),
        &configure_outputs,
        Some(&environment_receipt),
    )?;

    let usage = SourceUseLedger::create(&lifecycle.work_root().join("verified-source-usage.log"))?;
    let bridge = fetch_bridge(request)?;
    let make = host_tool(&host, "gmake").or_else(|_| host_tool(&host, "make"))?;
    let fetch = shell_fetch_command(&bridge)?;
    let mut compiler = Command::new(make);
    compiler
        .arg("-C")
        .arg(&build)
        .arg("-j")
        .arg(request.jobs.to_string())
        .arg("crosstools-release")
        .arg("AROS_TOOLCHAIN_DEFAULT_SYSROOT=")
        .arg(format!("FETCH={fetch}"));
    apply_child_environment(&mut compiler, &environment, &python, &lifecycle);
    compiler
        .env(
            "AROS_TOOLCHAIN_FETCH_LOCK",
            producer.root().join(declaration.source_lock_path()),
        )
        .env("AROS_TOOLCHAIN_FETCH_CACHE", &request.cache_dir)
        .env("AROS_TOOLCHAIN_FETCH_LEDGER", usage.path())
        .env(
            "AROS_TOOLCHAIN_FETCH_UPSTREAM",
            source.root().join("scripts/fetch.sh"),
        );
    run_phase(
        "compiler",
        &mut compiler,
        &lifecycle,
        deadline,
        cancellation,
    )?;
    usage.verify_complete(bound.source_lock())?;
    let compiler_outputs = measured_files(&prefix, "toolchain")?;
    let compiler_input = phase_input("compiler", &execution_context, Some(&configure_receipt))?;
    let compiler_receipt = persist_receipt(
        &lifecycle,
        "compiler",
        &plan.identity,
        &compiler_input,
        &prefix,
        &compiler_outputs,
        Some(&configure_receipt),
    )?;

    let cargo_program = host_tool(&host, "cargo")?;
    let mut collector = Command::new(cargo_program);
    collector
        .arg("--config")
        .arg("net.offline=true")
        .arg("--config")
        .arg(cargo.config())
        .args([
            "build",
            "--locked",
            "--offline",
            "--release",
            "--package",
            "aros-collect",
        ])
        .arg("--jobs")
        .arg(request.jobs.to_string())
        .arg("--manifest-path")
        .arg(tools.root().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&lifecycle.rust_target);
    environment.apply_to(&mut collector);
    cargo.apply_to(&mut collector);
    collector
        .env("HOME", &lifecycle.home)
        .env("TMPDIR", &lifecycle.tmp)
        .env("RUSTFLAGS", environment.collector_rustflags())
        .env("CARGO_BUILD_JOBS", request.jobs.to_string());
    if let Some(rustup_home) = selected_rustup_home() {
        // `cargo` may be the rustup proxy. Preserve only the pre-existing
        // absolute toolchain store; Cargo's registry/configuration remains
        // the private verified vendor environment above.
        collector.env("RUSTUP_HOME", rustup_home);
    }
    if let Some(channel) = selected_rust_channel(producer.root())? {
        collector.env("RUSTUP_TOOLCHAIN", channel);
    }
    run_phase(
        "collector",
        &mut collector,
        &lifecycle,
        deadline,
        cancellation,
    )?;
    install_collector(&lifecycle.rust_target, &prefix, &request.preset)?;
    remove_producer_only_llvm_inputs(&prefix)?;
    let collector_path = prefix.join("bin/aros-collect");
    let collector_output = measure_file(&collector_path, "toolchain/bin/aros-collect")?;
    let collector_input = phase_input("collector", &execution_context, Some(&compiler_receipt))?;
    let collector_receipt = persist_receipt(
        &lifecycle,
        "collector",
        &plan.identity,
        &collector_input,
        &prefix,
        std::slice::from_ref(&collector_output),
        Some(&compiler_receipt),
    )?;
    run_dirs.revalidate(cancellation)?;
    publish_candidate(&prefix, &lifecycle.published_prefix())?;
    let published_collector = measure_file(
        &lifecycle.published_prefix().join("bin/aros-collect"),
        "toolchain/bin/aros-collect",
    )?;
    let publish_input = phase_input("publish", &execution_context, Some(&collector_receipt))?;
    let publish_receipt = persist_publish_receipt(
        &lifecycle,
        "publish",
        &plan.identity,
        &publish_input,
        &lifecycle.published_prefix(),
        std::slice::from_ref(&published_collector),
        Some(&collector_receipt),
    )?;
    run_dirs.revalidate(cancellation)?;

    plan.identity.executor.tools_commit = Some(declaration.tools_commit().clone());
    Ok(BuildResult {
        schema: "aros-toolchain-result-v1",
        operation: "build",
        identity: plan.identity,
        output_root: lifecycle.output_root().to_owned(),
        outputs: vec![published_collector],
        evidence: vec![
            receipt_evidence("preflight", preflight_receipt),
            receipt_evidence("environment", environment_receipt),
            receipt_evidence("configure", configure_receipt),
            receipt_evidence("compiler", compiler_receipt),
            receipt_evidence("collector", collector_receipt),
            receipt_evidence("publish", publish_receipt),
            Evidence {
                check: "origin",
                status: "not-run",
                report_sha256: None,
            },
        ],
        environment: host
            .tools
            .iter()
            .map(|tool| ToolObservation {
                name: tool.name,
                version: tool.version.clone(),
            })
            .collect(),
        qualification: "local-only",
        commit_state: "committed",
    })
}

/// Resume only the collector after a complete compiler receipt boundary.
///
/// A compiler failure is never a checkpoint: this path needs a completed,
/// self-verifying compiler receipt, remeasures every predecessor output, and
/// gives Cargo a new target directory. Any missing, malformed, stale or
/// tampered state is retained and rejected without launching a child process.
fn resume_after_compiler(
    request: &BuildRequest,
    mut plan: plan::Plan,
    recipe: &Recipe,
    run_dirs: &RunDirectories,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<BuildResult, ContractError> {
    run_dirs.revalidate(cancellation)?;
    let lifecycle = LifecyclePaths::open(run_dirs)?;
    let work = run_dirs
        .paths()
        .work
        .as_deref()
        .ok_or_else(|| ContractError::state("retained work root is unavailable"))?;
    let source_root = work.join("source");
    let producer_root = work.join("producer");
    let tools_root = work.join("tools");
    let snapshots = SnapshotDigests::measure(&source_root, &producer_root, &tools_root)?;

    let declaration = NativeExecutorDeclaration::parse(&read_regular(
        producer_root.join(CONTRACT_PATH),
        "retained native executor declaration",
    )?)?;
    let contract = read_regular(
        tools_root.join(declaration.contract_path()),
        "retained selected tools contract",
    )?;
    let lock_bytes = read_regular(
        producer_root.join(declaration.source_lock_path()),
        "retained selected source lock",
    )?;
    let profiles = read_regular(
        producer_root.join(declaration.profiles_path()),
        "retained selected profile matrix",
    )?;
    let bound = declaration.bind(recipe, &contract, &lock_bytes, &profiles, &request.preset)?;
    let host = preflight::inspect(bound.selected_profile())?;
    let cache = source_cache::verify(&request.cache_dir, bound.source_lock())?;
    let environment = ProducerEnvironment::prepare(
        &ReproducibilityRoots {
            source: source_root,
            producer: producer_root.clone(),
            tools: tools_root.clone(),
            work: lifecycle.work_root().to_owned(),
            source_cache: request.cache_dir.clone(),
        },
        &host.tool_directories(),
        recipe.source_date_epoch(),
        request.jobs,
    )?;
    let preflight_context = PhaseInputContext {
        recipe,
        declaration: &declaration,
        host: Some(&host),
        cache: Some(&cache),
        jobs: request.jobs,
        snapshots: &snapshots,
        environment: None,
    };
    let execution_context = PhaseInputContext {
        environment: Some(&environment),
        ..preflight_context
    };

    let preflight_input = phase_input("preflight", &preflight_context, None)?;
    let preflight_receipt = revalidate_receipt(
        &lifecycle,
        "preflight",
        &plan.identity,
        &preflight_input,
        lifecycle.work_root(),
        &[],
        None,
    )?;
    let environment_input =
        phase_input("environment", &execution_context, Some(&preflight_receipt))?;
    let environment_receipt = revalidate_receipt(
        &lifecycle,
        "environment",
        &plan.identity,
        &environment_input,
        lifecycle.work_root(),
        &[],
        Some(&preflight_receipt),
    )?;
    // Later compiler work legitimately adds files beneath the configure root.
    // Revalidate the exact durable configure outputs instead of treating those
    // later files as a mutation of the completed configure boundary.
    let configure_outputs =
        remeasure_receipt_outputs(&lifecycle, "configure", lifecycle.work_root())?;
    let configure_input = phase_input("configure", &execution_context, Some(&environment_receipt))?;
    let configure_receipt = revalidate_receipt(
        &lifecycle,
        "configure",
        &plan.identity,
        &configure_input,
        lifecycle.work_root(),
        &configure_outputs,
        Some(&environment_receipt),
    )?;
    let compiler_outputs = measured_files(&lifecycle.staging_prefix(), "toolchain")?;
    let compiler_input = phase_input("compiler", &execution_context, Some(&configure_receipt))?;
    let compiler_receipt = revalidate_receipt(
        &lifecycle,
        "compiler",
        &plan.identity,
        &compiler_input,
        &lifecycle.staging_prefix(),
        &compiler_outputs,
        Some(&configure_receipt),
    )?;
    lifecycle.require_absent_receipt("collector")?;
    run_dirs.revalidate(cancellation)?;

    let cargo = CargoVendorEnvironment::open_existing(
        &request.cache_dir,
        &tools_root.join("Cargo.lock"),
        &lifecycle.cargo,
    )?;
    let prefix = lifecycle.staging_prefix();
    require_real_directory(&prefix, "retained native candidate prefix")?;
    require_real_directory(&prefix.join("bin"), "retained compiler bin directory")?;
    for name in ["aros-collect", "collect-aros", "collect-aros32"] {
        reject_existing_path(
            &prefix.join("bin").join(name),
            "retained collector destination",
        )?;
    }
    let cargo_program = host_tool(&host, "cargo")?;
    let rust_target = lifecycle.fresh_resume_rust_target()?;
    let mut collector = Command::new(cargo_program);
    collector
        .arg("--config")
        .arg("net.offline=true")
        .arg("--config")
        .arg(cargo.config())
        .args([
            "build",
            "--locked",
            "--offline",
            "--release",
            "--package",
            "aros-collect",
        ])
        .arg("--jobs")
        .arg(request.jobs.to_string())
        .arg("--manifest-path")
        .arg(tools_root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&rust_target);
    environment.apply_to(&mut collector);
    cargo.apply_to(&mut collector);
    collector
        .env("HOME", &lifecycle.home)
        .env("TMPDIR", &lifecycle.tmp)
        .env("RUSTFLAGS", environment.collector_rustflags())
        .env("CARGO_BUILD_JOBS", request.jobs.to_string());
    if let Some(rustup_home) = selected_rustup_home() {
        collector.env("RUSTUP_HOME", rustup_home);
    }
    if let Some(channel) = selected_rust_channel(&producer_root)? {
        collector.env("RUSTUP_TOOLCHAIN", channel);
    }
    let log_name = lifecycle.next_resume_log_name("collector")?;
    run_phase_with_log(
        "collector",
        &log_name,
        &mut collector,
        &lifecycle,
        deadline,
        cancellation,
    )?;
    install_collector(&rust_target, &prefix, &request.preset)?;
    remove_producer_only_llvm_inputs(&prefix)?;
    let collector_output = measure_file(
        &prefix.join("bin/aros-collect"),
        "toolchain/bin/aros-collect",
    )?;
    let collector_input = phase_input("collector", &execution_context, Some(&compiler_receipt))?;
    let collector_receipt = persist_receipt(
        &lifecycle,
        "collector",
        &plan.identity,
        &collector_input,
        &prefix,
        std::slice::from_ref(&collector_output),
        Some(&compiler_receipt),
    )?;
    run_dirs.revalidate(cancellation)?;
    publish_candidate(&prefix, &lifecycle.published_prefix())?;
    let published_collector = measure_file(
        &lifecycle.published_prefix().join("bin/aros-collect"),
        "toolchain/bin/aros-collect",
    )?;
    let publish_input = phase_input("publish", &execution_context, Some(&collector_receipt))?;
    let publish_receipt = persist_publish_receipt(
        &lifecycle,
        "publish",
        &plan.identity,
        &publish_input,
        &lifecycle.published_prefix(),
        std::slice::from_ref(&published_collector),
        Some(&collector_receipt),
    )?;
    run_dirs.revalidate(cancellation)?;
    plan.identity.executor.tools_commit = Some(declaration.tools_commit().clone());
    Ok(BuildResult {
        schema: "aros-toolchain-result-v1",
        operation: "build",
        identity: plan.identity,
        output_root: lifecycle.output_root().to_owned(),
        outputs: vec![published_collector],
        evidence: vec![
            receipt_evidence("preflight", preflight_receipt),
            receipt_evidence("environment", environment_receipt),
            receipt_evidence("configure", configure_receipt),
            receipt_evidence("compiler", compiler_receipt),
            receipt_evidence("collector", collector_receipt),
            receipt_evidence("publish", publish_receipt),
            Evidence {
                check: "origin",
                status: "not-run",
                report_sha256: None,
            },
        ],
        environment: host
            .tools
            .iter()
            .map(|tool| ToolObservation {
                name: tool.name,
                version: tool.version.clone(),
            })
            .collect(),
        qualification: "local-only",
        commit_state: "committed",
    })
}

fn selected_rustup_home() -> Option<PathBuf> {
    std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")))
        .filter(|path| path.is_absolute() && path.is_dir())
}

fn validate_request(request: &BuildRequest) -> Result<(), ContractError> {
    if request.release_id.is_empty() || !safe_segment(&request.release_id) {
        return Err(ContractError::invalid(
            "release-id must be one safe nonempty candidate segment",
        ));
    }
    if request.fetch_bridge.is_none() {
        return Err(ContractError::preflight(
            "native toolchain build requires the frontend MetaMake fetch bridge",
        ));
    }
    Ok(())
}

fn plan_request(request: &BuildRequest) -> PlanRequest {
    PlanRequest {
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
    }
}

struct LifecyclePaths {
    root: PathBuf,
    output: PathBuf,
    logs: PathBuf,
    receipts: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    python: PathBuf,
    cargo: PathBuf,
    rust_target: PathBuf,
}

impl LifecyclePaths {
    fn create(run_dirs: &RunDirectories) -> Result<Self, ContractError> {
        let work = run_dirs
            .paths()
            .work
            .as_ref()
            .ok_or_else(|| ContractError::state("owned work root is unavailable"))?;
        let output = run_dirs
            .paths()
            .output
            .as_ref()
            .ok_or_else(|| ContractError::state("owned output root is unavailable"))?;
        let root = work.join("native-lifecycle");
        fs::create_dir(&root)
            .map_err(|_| ContractError::state("cannot create fresh native lifecycle root"))?;
        restrict(&root)?;
        let logs = root.join("logs");
        let receipts = root.join("receipts");
        let home = root.join("home");
        let tmp = root.join("tmp");
        let python = root.join("python");
        let cargo = root.join("cargo");
        let rust_target = root.join("rust-target");
        for path in [&logs, &receipts, &home, &tmp] {
            fs::create_dir(path).map_err(|_| {
                ContractError::state("cannot create private native lifecycle directory")
            })?;
            restrict(path)?;
        }
        Ok(Self {
            root,
            output: output.clone(),
            logs,
            receipts,
            home,
            tmp,
            python,
            cargo,
            rust_target,
        })
    }

    fn open(run_dirs: &RunDirectories) -> Result<Self, ContractError> {
        let work = run_dirs
            .paths()
            .work
            .as_ref()
            .ok_or_else(|| ContractError::state("owned work root is unavailable"))?;
        let output = run_dirs
            .paths()
            .output
            .as_ref()
            .ok_or_else(|| ContractError::state("owned output root is unavailable"))?;
        let root = work.join("native-lifecycle");
        let logs = root.join("logs");
        let receipts = root.join("receipts");
        let home = root.join("home");
        let tmp = root.join("tmp");
        let python = root.join("python");
        let cargo = root.join("cargo");
        let rust_target = root.join("rust-target");
        for (path, label) in [
            (&root, "retained native lifecycle root"),
            (&logs, "retained native lifecycle logs"),
            (&receipts, "retained native lifecycle receipts"),
            (&home, "retained native lifecycle home"),
            (&tmp, "retained native lifecycle temporary directory"),
            (&python, "retained native lifecycle Python environment"),
            (&cargo, "retained native lifecycle Cargo environment"),
        ] {
            require_private_directory(path, label)?;
        }
        require_real_directory(output, "retained native output root")?;
        require_real_directory(
            &output.join(STAGING_PREFIX),
            "retained native candidate staging prefix",
        )?;
        reject_existing_path(
            &output.join(PUBLISHED_PREFIX),
            "retained published native candidate",
        )?;
        Ok(Self {
            root,
            output: output.clone(),
            logs,
            receipts,
            home,
            tmp,
            python,
            cargo,
            rust_target,
        })
    }

    fn work_root(&self) -> &Path {
        &self.root
    }

    fn output_root(&self) -> &Path {
        &self.output
    }

    /// Private compiler and collector destination.  This sibling is atomically
    /// renamed to [`PUBLISHED_PREFIX`] only after all lifecycle phases have
    /// completed and their receipts are durable.
    fn staging_prefix(&self) -> PathBuf {
        self.output.join(STAGING_PREFIX)
    }

    /// Consumer-visible candidate prefix.  It is intentionally absent until
    /// [`publish_candidate`] proves the no-clobber durable commit boundary.
    fn published_prefix(&self) -> PathBuf {
        self.output.join(PUBLISHED_PREFIX)
    }

    fn require_absent_receipt(&self, phase: &str) -> Result<(), ContractError> {
        let path = self.receipts.join(format!("{phase}.json"));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(ContractError::state(format!(
                "retained {phase} receipt already exists; an explicit collector resume cannot overwrite completed or foreign state"
            ))),
            Err(_) => Err(ContractError::state("cannot inspect retained phase receipt")),
        }
    }

    fn fresh_resume_rust_target(&self) -> Result<PathBuf, ContractError> {
        for attempt in 1..=1024 {
            let candidate = self.root.join(format!("rust-target-resume-{attempt}"));
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
                Ok(_) => {}
                Err(_) => {
                    return Err(ContractError::state(
                        "cannot inspect retained collector resume target directory",
                    ))
                }
            }
        }
        Err(ContractError::state(
            "retained collector has too many prior resume target directories",
        ))
    }

    fn next_resume_log_name(&self, phase: &str) -> Result<String, ContractError> {
        for attempt in 1..=1024 {
            let name = format!("{phase}-resume-{attempt}");
            let stdout = self.logs.join(format!("{name}.stdout.log"));
            let stderr = self.logs.join(format!("{name}.stderr.log"));
            let stdout_missing = matches!(fs::symlink_metadata(&stdout), Err(error) if error.kind() == std::io::ErrorKind::NotFound);
            let stderr_missing = matches!(fs::symlink_metadata(&stderr), Err(error) if error.kind() == std::io::ErrorKind::NotFound);
            if stdout_missing && stderr_missing {
                return Ok(name);
            }
        }
        Err(ContractError::state(
            "retained collector has too many prior resume logs",
        ))
    }
}

fn restrict(path: &Path) -> Result<(), ContractError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| ContractError::state("cannot restrict native lifecycle directory"))
}

fn require_private_directory(path: &Path, label: &str) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::state(format!("{label} is unavailable")))?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(ContractError::state(format!(
            "{label} is not a private real directory"
        )));
    }
    Ok(())
}

fn read_regular(path: PathBuf, label: &str) -> Result<Vec<u8>, ContractError> {
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| ContractError::identity(format!("{label} is unavailable")))?;
    if !metadata.is_file() {
        return Err(ContractError::identity(format!(
            "{label} is not a regular file"
        )));
    }
    fs::read(path).map_err(|_| ContractError::identity(format!("cannot read {label}")))
}

fn python_interpreter(host: &HostPreflight) -> Result<PythonInterpreter, ContractError> {
    let tool = host
        .tools
        .iter()
        .find(|candidate| candidate.name == "python3")
        .ok_or_else(|| ContractError::prerequisite("native preflight omitted python3"))?;
    let version = tool.version.clone();
    if !version.starts_with("Python 3.") {
        return Err(ContractError::environment(
            "native host python3 observation is not Python 3",
        ));
    }
    Ok(PythonInterpreter {
        path: tool.path.clone(),
        version,
    })
}

fn host_tool(host: &HostPreflight, name: &str) -> Result<PathBuf, ContractError> {
    host.tools
        .iter()
        .find(|candidate| candidate.name == name)
        .map(|candidate| candidate.invocation_path.clone())
        .ok_or_else(|| ContractError::prerequisite(format!("native preflight omitted {name}")))
}

fn apply_child_environment(
    command: &mut Command,
    environment: &ProducerEnvironment,
    python: &PythonEnvironment,
    lifecycle: &LifecyclePaths,
) {
    environment.apply_to(command);
    python.apply_to(command);
    command
        .env("HOME", &lifecycle.home)
        .env("TMPDIR", &lifecycle.tmp)
        .env("AROS_OFFLINE", "1")
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "never")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_OPTIONAL_LOCKS", "0");
}

fn run_phase(
    phase: &'static str,
    command: &mut Command,
    lifecycle: &LifecyclePaths,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    run_phase_with_log(phase, phase, command, lifecycle, deadline, cancellation)
}

fn run_phase_with_log(
    phase: &'static str,
    log_name: &str,
    command: &mut Command,
    lifecycle: &LifecyclePaths,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    let output = run_output_with_input_and_control(
        command,
        &[],
        CAPTURE_LIMIT,
        remaining(deadline)?,
        cancellation,
    )
    .map_err(|error| {
        phase_error(
            phase,
            format!("cannot start or supervise native phase: {error}"),
        )
    })?;
    persist_process_output(&lifecycle.logs, log_name, &output)?;
    if output.cancelled || cancellation.is_cancelled() {
        return Err(ContractError::state(format!(
            "native {phase} phase cancelled; owned material and logs are retained"
        )));
    }
    if output.timed_out {
        return Err(phase_error(
            phase,
            format!("native {phase} phase exceeded the explicit build deadline"),
        )
        .context(DiagnosticContext {
            tool: Some(phase.into()),
            timed_out: Some(true),
            ..DiagnosticContext::default()
        }));
    }
    if !output.status.success() {
        return Err(phase_error(
            phase,
            format!(
                "native {phase} phase exited unsuccessfully (exit code {:?}); inspect retained phase logs",
                output.status.code()
            ),
        )
        .context(DiagnosticContext {
            tool: Some(phase.into()),
            exit_code: output.status.code(),
            signal: exit_signal(output.status),
            ..DiagnosticContext::default()
        }));
    }
    Ok(())
}

fn phase_error(phase: &str, message: String) -> ContractError {
    match phase {
        "configure" => ContractError::configure(message),
        "compiler" => ContractError::compiler(message),
        "collector" => ContractError::collector(message),
        _ => ContractError::state(message),
    }
}

fn persist_process_output(
    logs: &Path,
    log_name: &str,
    output: &aros_common::ProcessOutput,
) -> Result<(Sha256Digest, Sha256Digest), ContractError> {
    let stdout_path = logs.join(format!("{log_name}.stdout.log"));
    let stderr_path = logs.join(format!("{log_name}.stderr.log"));
    let stdout = create_log(&stdout_path, &output.stdout)?;
    let stderr = create_log(&stderr_path, &output.stderr)?;
    Ok((stdout, stderr))
}

fn create_log(
    path: &Path,
    stream: &aros_common::CapturedStream,
) -> Result<Sha256Digest, ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::state("cannot create native phase log"))?;
    stream
        .write_rendered(&mut file)
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::state("cannot durably persist native phase log"))?;
    sha256_file(path)
        .map(|measured| measured.digest)
        .map_err(|_| ContractError::state("cannot hash native phase log"))
}

fn selected_rust_channel(producer: &Path) -> Result<Option<String>, ContractError> {
    let path = producer.join("toolchains/rust-toolchain.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(ContractError::environment(
                "selected producer Rust toolchain file is unreadable",
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
        Some(_) => Err(ContractError::environment(
            "producer Rust toolchain channel is not a safe selection",
        )),
        None => Err(ContractError::environment(
            "producer Rust toolchain file has no channel",
        )),
    }
}

fn fetch_bridge(request: &BuildRequest) -> Result<PathBuf, ContractError> {
    let path = request
        .fetch_bridge
        .as_ref()
        .ok_or_else(|| ContractError::preflight("native fetch bridge is not selected"))?;
    let canonical = path
        .canonicalize()
        .map_err(|_| ContractError::preflight("native fetch bridge cannot be canonicalized"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|_| ContractError::preflight("native fetch bridge is unavailable"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(ContractError::preflight(
            "native fetch bridge is not an executable regular file",
        ));
    }
    Ok(canonical)
}

fn shell_fetch_command(bridge: &Path) -> Result<String, ContractError> {
    let bridge = bridge
        .to_str()
        .ok_or_else(|| ContractError::preflight("native fetch bridge path is not UTF-8"))?;
    if bridge.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(ContractError::preflight(
            "native fetch bridge path contains a control character",
        ));
    }
    Ok(format!(
        "'{}' toolchain __metamake-fetch",
        bridge.replace('\'', "'\\\"'\\\"'")
    ))
}

fn install_collector(target: &Path, prefix: &Path, profile: &str) -> Result<(), ContractError> {
    let source = target.join("release/aros-collect");
    let bin = prefix.join("bin");
    let source_metadata = fs::symlink_metadata(&source)
        .map_err(|_| ContractError::collector("native collector output is unavailable"))?;
    if !source_metadata.file_type().is_file() {
        return Err(ContractError::collector(
            "native collector output is not a regular file",
        ));
    }
    require_real_directory(prefix, "native candidate prefix")?;
    require_real_directory(&bin, "compiler bin directory")?;
    let destination = bin.join("aros-collect");
    reject_existing_path(&destination, "native collector destination")?;
    fs::copy(&source, &destination)
        .map_err(|_| ContractError::collector("cannot install exact native collector"))?;
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
        .map_err(|_| ContractError::collector("cannot set native collector mode"))?;
    let collect = bin.join("collect-aros");
    reject_existing_path(&collect, "collect-aros alias destination")?;
    symlink("aros-collect", collect)
        .map_err(|_| ContractError::collector("cannot create collect-aros alias"))?;
    if profile == "pc-x86_64" {
        let collect32 = bin.join("collect-aros32");
        reject_existing_path(&collect32, "collect-aros32 alias destination")?;
        symlink("aros-collect", collect32)
            .map_err(|_| ContractError::collector("cannot create collect-aros32 alias"))?;
    }
    Ok(())
}

fn remove_producer_only_llvm_inputs(prefix: &Path) -> Result<(), ContractError> {
    require_real_directory(prefix, "native candidate prefix")?;
    let bin = prefix.join("bin");
    require_real_directory(&bin, "compiler bin directory")?;
    let config = bin.join("llvm-config");
    match fs::symlink_metadata(&config) {
        Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(&config)
            .map_err(|_| ContractError::collector("cannot remove producer-only llvm-config"))?,
        Ok(_) => {
            return Err(ContractError::collector(
                "producer-only llvm-config is not a regular file",
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ContractError::collector(
                "cannot inspect producer-only llvm-config",
            ))
        }
    }
    let lib = prefix.join("lib");
    match fs::symlink_metadata(&lib) {
        Ok(_) => require_real_directory(&lib, "compiler lib directory")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ContractError::collector(
                "cannot inspect compiler lib directory",
            ))
        }
    }
    let cmake_root = lib.join("cmake");
    match fs::symlink_metadata(&cmake_root) {
        Ok(_) => require_real_directory(&cmake_root, "compiler CMake directory")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ContractError::collector(
                "cannot inspect compiler CMake directory",
            ))
        }
    }
    let cmake = cmake_root.join("llvm");
    match fs::symlink_metadata(&cmake) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(&cmake).map_err(|_| {
                ContractError::collector("cannot remove producer-only LLVM CMake files")
            })?;
        }
        Ok(_) => {
            return Err(ContractError::collector(
                "producer-only LLVM CMake path is not a real directory",
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ContractError::collector(
                "cannot inspect producer-only LLVM CMake path",
            ))
        }
    }
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::collector(format!("{label} is unavailable")))?;
    if !metadata.file_type().is_dir() {
        return Err(ContractError::collector(format!(
            "{label} is not a real directory"
        )));
    }
    Ok(())
}

fn reject_existing_path(path: &Path, label: &str) -> Result<(), ContractError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ContractError::collector(format!("{label} already exists"))),
        Err(_) => Err(ContractError::collector(format!("cannot inspect {label}"))),
    }
}

/// Commit one fully built native candidate into its consumer-visible prefix.
///
/// Both leaves are siblings in the already locked private output root.  The
/// shared publication primitive recursively syncs the prepared tree, performs
/// a no-clobber rename and syncs the parent directory.  A failure before the
/// rename leaves the staging tree intact; a failure after it is deliberately
/// surfaced as an uncertain commit while retaining the complete final tree.
fn publish_candidate(staging: &Path, destination: &Path) -> Result<(), ContractError> {
    publish_prepared_tree_noclobber(staging, destination)
        .map(|_| ())
        .map_err(|error| {
            let message = match publication_failure_class(&error) {
            PublicationFailureClass::Conflict => {
                "native candidate publication refused an existing final prefix; the staged candidate is retained"
            }
            PublicationFailureClass::CommitStateUncertain => {
                "native candidate publication reached an uncertain durable commit state; inspect the retained final prefix and phase evidence"
            }
            PublicationFailureClass::UnsafeTarget => {
                "native candidate publication rejected an unsafe staged or final prefix; the staged candidate is retained"
            }
            PublicationFailureClass::Unsupported => {
                "native candidate publication requires unsupported durable filesystem operations; the staged candidate is retained"
            }
            PublicationFailureClass::RecoveryIncomplete => {
                "native candidate publication recovery is incomplete; staged and final state are retained for inspection"
            }
            PublicationFailureClass::Io => {
                "native candidate publication failed before a proven durable commit; the staged candidate is retained"
            }
            };
            ContractError::state(message)
        })
}

fn measured_files(root: &Path, prefix: &str) -> Result<Vec<Output>, ContractError> {
    let mut outputs = Vec::new();
    collect_files(root, prefix, &mut outputs)?;
    outputs.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(outputs)
}

fn collect_files(root: &Path, prefix: &str, output: &mut Vec<Output>) -> Result<(), ContractError> {
    for entry in fs::read_dir(root)
        .map_err(|_| ContractError::state("cannot inspect completed native phase output"))?
    {
        let entry =
            entry.map_err(|_| ContractError::state("cannot read completed native phase output"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ContractError::state("cannot inspect completed native phase output"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| ContractError::state("native phase output name is not UTF-8"))?;
        let relative = format!("{prefix}/{name}");
        if metadata.is_dir() {
            collect_files(&path, &relative, output)?;
        } else if metadata.is_file() {
            let measured = sha256_file(&path)
                .map_err(|_| ContractError::state("cannot hash completed native phase output"))?;
            output.push(Output {
                path: relative,
                kind: "file",
                sha256: measured.digest,
                size: measured.size,
            });
        } else if !metadata.file_type().is_symlink() {
            return Err(ContractError::state(
                "native phase output contains an unsupported special object",
            ));
        }
    }
    Ok(())
}

fn measure_file(path: &Path, relative: &str) -> Result<Output, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::collector("native collector output is unavailable"))?;
    if !metadata.is_file() {
        return Err(ContractError::collector(
            "native collector output is not a regular file",
        ));
    }
    let measured = sha256_file(path)
        .map_err(|_| ContractError::collector("cannot hash native collector output"))?;
    Ok(Output {
        path: relative.into(),
        kind: "file",
        sha256: measured.digest,
        size: measured.size,
    })
}

/// Re-measure the completed regular-file outputs named by one durable receipt.
///
/// A later phase may append build-system artefacts below an earlier phase's
/// root.  That does not alter the earlier boundary.  The recorded files still
/// have to exist with their exact hashes and sizes; non-file outputs and unsafe
/// paths are rejected before touching the filesystem.
fn remeasure_receipt_outputs(
    lifecycle: &LifecyclePaths,
    phase: &str,
    root: &Path,
) -> Result<Vec<Output>, ContractError> {
    #[derive(Deserialize)]
    struct StoredOutput {
        path: String,
        kind: String,
        sha256: String,
        size: u64,
    }

    #[derive(Deserialize)]
    struct StoredReceipt {
        outputs: Vec<StoredOutput>,
    }

    let path = lifecycle.receipts.join(format!("{phase}.json"));
    let bytes = read_regular(path, "retained native phase receipt")?;
    let receipt: StoredReceipt = serde_json::from_slice(&bytes)
        .map_err(|_| ContractError::state("retained native phase receipt is not valid JSON"))?;
    let mut seen = BTreeSet::new();
    let mut remeasured = Vec::with_capacity(receipt.outputs.len());
    for output in receipt.outputs {
        if output.kind != "file"
            || !safe_relative_output_path(&output.path)
            || !seen.insert(output.path.clone())
        {
            return Err(ContractError::state(
                "retained native phase receipt names an unsafe or unsupported output",
            ));
        }
        let recorded = Sha256Digest::parse(&output.sha256).map_err(|_| {
            ContractError::state("retained native phase receipt has an invalid output digest")
        })?;
        let current = measure_file(&root.join(&output.path), &output.path)?;
        if current.sha256 != recorded || current.size != output.size {
            return Err(ContractError::state(
                "retained native phase output does not match its durable receipt",
            ));
        }
        remeasured.push(current);
    }
    Ok(remeasured)
}

fn safe_relative_output_path(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value).components().all(|component| {
            matches!(component, std::path::Component::Normal(segment) if !segment.is_empty())
        })
}

#[derive(Clone, Copy)]
struct PhaseInputContext<'a> {
    recipe: &'a Recipe,
    declaration: &'a NativeExecutorDeclaration,
    host: Option<&'a HostPreflight>,
    cache: Option<&'a source_cache::CacheObservation>,
    jobs: u64,
    snapshots: &'a SnapshotDigests,
    environment: Option<&'a ProducerEnvironment>,
}

fn phase_input(
    phase: &str,
    context: &PhaseInputContext<'_>,
    previous_receipt_sha256: Option<&Sha256Digest>,
) -> Result<Sha256Digest, ContractError> {
    let tools = context.host.map(|host| {
        host.tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "path": tool.path,
                    "invocation_path": tool.invocation_path,
                    "version": tool.version
                })
            })
            .collect::<Vec<_>>()
    });
    let payloads = context.cache.map(|cache| {
        cache
            .payloads
            .iter()
            .map(|payload| json!({"filename": payload.filename, "sha256": payload.sha256, "size": payload.size}))
            .collect::<Vec<_>>()
    });
    let value = json!({
        "schema": "aros-toolchain-phase-input-v1",
        "phase": phase,
        "recipe_sha256": context.recipe.sha256(),
        "contract_id": context.declaration.contract_id(),
        "contract_sha256": context.declaration.contract_sha256(),
        "tools_commit": context.declaration.tools_commit(),
        "host": context.host.map(|host| host.host),
        "profile": context.host.map(|host| host.profile.clone()),
        "jobs": context.jobs,
        "source_date_epoch": context.recipe.source_date_epoch(),
        "snapshots": {
            "source_sha256": context.snapshots.source,
            "producer_sha256": context.snapshots.producer,
            "tools_sha256": context.snapshots.tools,
        },
        "effective_environment": context.environment.map(|environment| json!({
            "prefix_maps": environment.prefix_maps(),
            "collector_rustflags": environment.collector_rustflags(),
            "cargo_jobs": context.jobs,
        })),
        "tools": tools,
        "payloads": payloads,
        "previous_receipt_sha256": previous_receipt_sha256,
    });
    Ok(sha256_bytes(&canonical::bytes(&value)?))
}

/// Content identities of the three retained metadata-free snapshots.
///
/// Receipt revalidation uses these as a no-follow whole-tree checksum. They
/// bind a phase to the actual material passed to source tools, not merely to
/// the original checkout identities that were used to create it.
#[derive(Debug, Clone)]
struct SnapshotDigests {
    source: Sha256Digest,
    producer: Sha256Digest,
    tools: Sha256Digest,
}

impl SnapshotDigests {
    fn measure(source: &Path, producer: &Path, tools: &Path) -> Result<Self, ContractError> {
        Ok(Self {
            source: snapshot_digest(source, "source")?,
            producer: snapshot_digest(producer, "producer")?,
            tools: snapshot_digest(tools, "tools")?,
        })
    }
}

fn snapshot_digest(root: &Path, role: &str) -> Result<Sha256Digest, ContractError> {
    measure_tree_content_cas(root)
        .map(|tree| tree.payload_digest_excluding(None))
        .map_err(|_| ContractError::state(format!("cannot measure retained {role} snapshot")))
}

#[derive(Serialize)]
struct Receipt<'a> {
    schema: &'static str,
    identity: &'a Identity,
    phase: &'static str,
    input_sha256: &'a Sha256Digest,
    output_root: &'a Path,
    outputs: &'a [Output],
    previous_receipt_sha256: Option<&'a Sha256Digest>,
    #[serde(rename = "receipt_sha256", skip_serializing_if = "Option::is_none")]
    digest: Option<Sha256Digest>,
}

fn persist_receipt(
    lifecycle: &LifecyclePaths,
    phase: &'static str,
    identity: &Identity,
    input_sha256: &Sha256Digest,
    output_root: &Path,
    outputs: &[Output],
    previous: Option<&Sha256Digest>,
) -> Result<Sha256Digest, ContractError> {
    let provisional = Receipt {
        schema: "aros-toolchain-receipt-v1",
        identity,
        phase,
        input_sha256,
        output_root,
        outputs,
        previous_receipt_sha256: previous,
        digest: None,
    };
    let value = serde_json::to_value(&provisional)
        .map_err(|_| ContractError::state("cannot encode native phase receipt"))?;
    let digest = sha256_bytes(&canonical::bytes(&value)?);
    let receipt = Receipt {
        digest: Some(digest.clone()),
        ..provisional
    };
    let value = serde_json::to_value(&receipt)
        .map_err(|_| ContractError::state("cannot encode final native phase receipt"))?;
    let bytes = canonical::bytes(&value)?;
    let path = lifecycle.receipts.join(format!("{phase}.json"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| ContractError::state("cannot create fresh native phase receipt"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::state("cannot durably persist native phase receipt"))?;
    let measured = sha256_file(&path)
        .map_err(|_| ContractError::state("cannot measure native phase receipt"))?;
    if measured.digest != sha256_bytes(&bytes) {
        return Err(ContractError::state(
            "native phase receipt changed before its durable verification",
        ));
    }
    verify_receipt(&path, &digest)?;
    Ok(digest)
}

/// Record the last receipt without hiding a candidate that has already crossed
/// the durable publication boundary.  Callers receive no success result: the
/// final prefix is retained for explicit inspection and must not be rebuilt or
/// overwritten automatically.
fn persist_publish_receipt(
    lifecycle: &LifecyclePaths,
    phase: &'static str,
    identity: &Identity,
    input_sha256: &Sha256Digest,
    output_root: &Path,
    outputs: &[Output],
    previous: Option<&Sha256Digest>,
) -> Result<Sha256Digest, ContractError> {
    persist_receipt(
        lifecycle,
        phase,
        identity,
        input_sha256,
        output_root,
        outputs,
        previous,
    )
    .map_err(|_| {
        ContractError::state(
            "native candidate was durably published but its publish receipt could not be committed; the final prefix is retained and must be inspected before any new build",
        )
    })
}

/// Re-open a durable receipt and prove its self-digest rather than merely
/// proving that its bytes still match the write buffer. This keeps retained
/// receipts independently inspectable after a phase has completed.
fn verify_receipt(path: &Path, expected: &Sha256Digest) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::state("cannot reopen native phase receipt"))?;
    if !metadata.file_type().is_file() {
        return Err(ContractError::state(
            "native phase receipt is not a regular file during verification",
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| ContractError::state("cannot read native phase receipt for verification"))?;
    let mut value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| ContractError::state("native phase receipt is not valid JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ContractError::state("native phase receipt is not a JSON object"))?;
    let recorded = object
        .remove("receipt_sha256")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| ContractError::state("native phase receipt omits its self-digest"))?;
    let recorded = Sha256Digest::parse(&recorded)
        .map_err(|_| ContractError::state("native phase receipt has an invalid self-digest"))?;
    let calculated = sha256_bytes(&canonical::bytes(&value)?);
    if &recorded != expected || calculated != recorded {
        return Err(ContractError::state(
            "native phase receipt self-digest verification failed",
        ));
    }
    Ok(())
}

/// Revalidate a retained completed boundary before a narrowly permitted
/// resume. The receipt's self-digest is necessary but insufficient: every
/// identity, predecessor, recomputed effective input and measured output must
/// also match the currently held roots.
fn revalidate_receipt(
    lifecycle: &LifecyclePaths,
    phase: &'static str,
    identity: &Identity,
    input_sha256: &Sha256Digest,
    output_root: &Path,
    outputs: &[Output],
    previous: Option<&Sha256Digest>,
) -> Result<Sha256Digest, ContractError> {
    let path = lifecycle.receipts.join(format!("{phase}.json"));
    let bytes = read_regular(path.clone(), "retained native phase receipt")?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| ContractError::state("retained native phase receipt is not valid JSON"))?;
    let recorded = value
        .get("receipt_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ContractError::state("retained native phase receipt omits its self-digest")
        })?;
    let recorded = Sha256Digest::parse(recorded).map_err(|_| {
        ContractError::state("retained native phase receipt has an invalid self-digest")
    })?;
    verify_receipt(&path, &recorded)?;
    let expected_identity = serde_json::to_value(identity)
        .map_err(|_| ContractError::state("cannot encode current native receipt identity"))?;
    let expected_outputs = serde_json::to_value(outputs)
        .map_err(|_| ContractError::state("cannot encode current native receipt outputs"))?;
    let expected_previous = previous.map_or(serde_json::Value::Null, |digest| {
        serde_json::Value::String(digest.to_string())
    });
    let checks = [
        ("schema", json!("aros-toolchain-receipt-v1")),
        ("identity", expected_identity),
        ("phase", json!(phase)),
        ("input_sha256", json!(input_sha256)),
        ("output_root", json!(output_root)),
        ("outputs", expected_outputs),
        ("previous_receipt_sha256", expected_previous),
    ];
    for (field, expected) in checks {
        if value.get(field) != Some(&expected) {
            return Err(ContractError::state(format!(
                "retained {phase} receipt does not match the current verified {field}"
            )));
        }
    }
    Ok(recorded)
}

const fn receipt_evidence(check: &'static str, digest: Sha256Digest) -> Evidence {
    Evidence {
        check,
        status: "passed",
        report_sha256: Some(digest),
    }
}

fn remaining(deadline: Instant) -> Result<Duration, ContractError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            ContractError::state("native toolchain operation exceeded its explicit deadline")
        })
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::{install_collector, remove_producer_only_llvm_inputs};

    #[test]
    fn collector_install_rejects_a_compiler_bin_symlink_before_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let target = root.join("target/release");
        let prefix = root.join("prefix");
        let outside = root.join("outside");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir(&prefix).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(target.join("aros-collect"), b"collector").unwrap();
        symlink(&outside, prefix.join("bin")).unwrap();

        assert!(install_collector(&root.join("target"), &prefix, "pc-x86_64").is_err());
        assert!(!outside.join("aros-collect").exists());
    }

    #[test]
    fn llvm_cleanup_rejects_a_symlinked_parent_before_deletion() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let prefix = root.join("prefix");
        let outside = root.join("outside");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::create_dir_all(outside.join("cmake/llvm")).unwrap();
        fs::write(outside.join("cmake/llvm/sentinel"), b"keep").unwrap();
        symlink(&outside, prefix.join("lib")).unwrap();

        assert!(remove_producer_only_llvm_inputs(&prefix).is_err());
        assert!(outside.join("cmake/llvm/sentinel").is_file());
    }
}
