//! Closed native compatibility producer commands.
//!
//! This module owns the compatibility-only CLI surface: explicit host-tool
//! closure entries, the versioned upstream source-input closure, and execution of the
//! six-phase native compatibility probe. Keeping it separate prevents the
//! general producer command router from becoming the owner of consumer policy.

use std::path::PathBuf;
use std::time::Duration;

use aros_common::CancellationToken;
use aros_toolchain::compatibility::{
    self, CompatibilityHostTool, CompatibilityPreparationRequest, HostToolClosureRequest,
    NativeCompatibilityRequest, StandaloneFixtures, TwoRootRelocationRequest,
};
use aros_toolchain::compatibility_ports::{self, CompatibilityPortsLock};
use aros_toolchain::package_verify;
use aros_toolchain::python_environment::PythonEnvironment;
use aros_toolchain::source_cache;
use aros_toolchain::source_cache_request::SourceCacheRequest;
use clap::Args;

use super::{
    native_error, package_context, print_json, read_regular_input, PackageContext,
    PackageContextArgs, ResultFormat,
};

/// Inputs for one complete native six-phase package compatibility execution.
#[derive(Args)]
pub(super) struct CompatibilityArgs {
    #[command(flatten)]
    context: PackageContextArgs,
    /// Complete verified package set to extract twice independently
    #[arg(long)]
    package_dir: PathBuf,
    /// Absent first relocation root for the CMake consumer
    #[arg(long)]
    first_root: PathBuf,
    /// Absent second relocation root for upstream and standalone consumers
    #[arg(long)]
    second_root: PathBuf,
    /// Engine-free source directory used for the tools-owned CMake consumer
    #[arg(long)]
    source_dir: PathBuf,
    /// Existing work root where the embedded engine receives one fresh leaf
    #[arg(long)]
    engine_work_dir: PathBuf,
    /// Fresh Cargo release directory containing the exact required helpers
    #[arg(long)]
    helpers_dir: PathBuf,
    /// Explicit absolute CMake executable
    #[arg(long)]
    cmake_program: PathBuf,
    /// Explicit absolute Ninja executable
    #[arg(long)]
    ninja_program: PathBuf,
    /// Pristine upstream source tree containing the source-owned configure script
    #[arg(long)]
    upstream_source_dir: PathBuf,
    /// Absent upstream build directory
    #[arg(long)]
    upstream_build_dir: PathBuf,
    /// Prepared source cache containing the lock-owned host Python packages
    #[arg(long)]
    python_cache_dir: PathBuf,
    /// Compatibility-ports-v2 lock selecting the exact upstream source inputs
    #[arg(long)]
    ports_lock: PathBuf,
    /// Prepared direct cache containing the ports-lock-owned source inputs
    #[arg(long)]
    ports_cache_dir: PathBuf,
    /// Absent private directory materialized for upstream --with-portssources
    #[arg(long)]
    ports_sources_dir: PathBuf,
    /// Absent private host Python environment directory
    #[arg(long)]
    python_environment_dir: PathBuf,
    /// Absent private host-command closure directory for CMake host tools and
    /// upstream configure/Make
    #[arg(long)]
    host_tools_dir: PathBuf,
    /// Exact host command closure entry as NAME=ABSOLUTE_PATH; repeatable
    #[arg(long = "host-tool", value_name = "NAME=PATH")]
    host_tools: Vec<String>,
    /// Absent CMake consumer build directory
    #[arg(long)]
    cmake_build_dir: PathBuf,
    /// C standalone fixture source
    #[arg(long)]
    c_fixture: PathBuf,
    /// C++ standalone fixture source
    #[arg(long)]
    cxx_fixture: PathBuf,
    /// Absent standalone-output directory
    #[arg(long)]
    standalone_output_dir: PathBuf,
    /// Absent durable phase-report directory
    #[arg(long)]
    reports_dir: PathBuf,
    /// Positive explicit Make parallelism
    #[arg(long, value_parser = parse_jobs)]
    jobs: usize,
    /// Positive per-phase deadline in seconds
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Execute the complete native compatibility contract for one package lane.
pub(super) async fn compatibility(args: CompatibilityArgs) -> miette::Result<()> {
    let format = args.format;
    let context = package_context(args.context.clone())?;
    let host_tools = parse_host_tools(&args.host_tools)?;
    let ports_lock_bytes = read_regular_input(&args.ports_lock, "compatibility ports lock")?;
    let ports_lock =
        CompatibilityPortsLock::parse(&ports_lock_bytes).map_err(|error| native_error(&error))?;
    let cache_request = SourceCacheRequest::from_compatibility_ports_lock(&ports_lock_bytes)
        .map_err(|error| native_error(&error))?;
    source_cache::verify_request(&args.ports_cache_dir, &cache_request)
        .map_err(|error| native_error(&error))?;
    let cancellation = CancellationToken::default();
    let worker_token = cancellation.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        execute_compatibility(context, args, host_tools, ports_lock, &worker_token)
    });
    let report = tokio::select! {
        result = &mut worker => result.map_err(|_| miette::miette!("native compatibility worker terminated unexpectedly"))?,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                cancellation.cancel();
            }
            (&mut worker).await.map_err(|_| miette::miette!("native compatibility worker terminated unexpectedly"))?
        }
    }
    .map_err(|error| native_error(&error))?;
    let phases = report
        .probes
        .reports
        .keys()
        .map(|phase| format!("{phase:?}"))
        .collect::<Vec<_>>();
    let standalone_targets = report
        .standalone
        .targets
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    match format {
        ResultFormat::Human => aros_common::outputln!(
            "Native compatibility: {} phases, {} standalone target(s)\nReceipt: {}\nSHA-256: {}",
            phases.len(),
            standalone_targets.len(),
            report.receipt.path.display(),
            report.receipt.sha256,
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "compatibility",
            "phases": phases,
            "standalone_targets": standalone_targets,
            "receipt": report.receipt.path,
            "receipt_sha256": report.receipt.sha256,
        }))?,
    }
    Ok(())
}

fn execute_compatibility(
    context: PackageContext,
    args: CompatibilityArgs,
    host_tool_entries: Vec<CompatibilityHostTool>,
    ports_lock: CompatibilityPortsLock,
    cancellation: &CancellationToken,
) -> Result<compatibility::NativeCompatibilityReport, aros_toolchain::ContractError> {
    let verification = package_verify::PackageVerificationRequest {
        package_dir: args.package_dir,
        release_id: context.release_id,
        host: context.host.clone(),
        recipe: context.recipe,
        source_lock: context.source_lock,
        profile: context.profile,
        build_environment: context.build_environment,
        forbidden_prefixes: context.forbidden_prefixes,
    };
    let relocation = compatibility::extract_two_roots(&TwoRootRelocationRequest {
        verification: verification.clone(),
        first_root: args.first_root,
        second_root: args.second_root,
    })?;
    let preparation = compatibility::prepare(&CompatibilityPreparationRequest {
        source_root: args.source_dir,
        work_root: args.engine_work_dir,
        helpers_root: args.helpers_dir,
    })?;
    let python = PythonEnvironment::prepare(
        &verification.source_lock,
        &args.python_cache_dir,
        &args.python_environment_dir,
    )?;
    let ports_sources = compatibility_ports::materialize(
        &args.ports_cache_dir,
        &ports_lock,
        &context.upstream_commit,
        verification.profile.name(),
        &args.ports_sources_dir,
    )?;
    drop(ports_lock);
    let host_tools = compatibility::prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: args.host_tools_dir,
        tools: host_tool_entries,
    })?;
    compatibility::execute_native_compatibility(
        &NativeCompatibilityRequest {
            preparation,
            relocation,
            profile: verification.profile,
            cmake_program: args.cmake_program,
            ninja_program: args.ninja_program,
            cmake_build_root: args.cmake_build_dir,
            upstream_source_root: args.upstream_source_dir,
            upstream_source_commit: context.upstream_commit,
            upstream_build_root: args.upstream_build_dir,
            host_python: python,
            host_tools,
            ports_sources,
            host: context.host,
            make_jobs: args.jobs,
            standalone_fixtures: StandaloneFixtures {
                c: args.c_fixture,
                cxx: args.cxx_fixture,
            },
            standalone_output_root: args.standalone_output_dir,
            reports_root: args.reports_dir,
            timeout: Duration::from_secs(args.timeout_seconds),
        },
        cancellation,
    )
}

fn parse_host_tools(entries: &[String]) -> miette::Result<Vec<CompatibilityHostTool>> {
    if entries.is_empty() {
        return Err(miette::miette!(
            "native compatibility requires at least one explicit --host-tool NAME=PATH entry"
        ));
    }
    entries
        .iter()
        .map(|entry| {
            let (name, program) = entry.split_once('=').ok_or_else(|| {
                miette::miette!(
                    "native compatibility host-tool entries must use NAME=ABSOLUTE_PATH"
                )
            })?;
            if name.is_empty() || program.is_empty() {
                return Err(miette::miette!(
                    "native compatibility host-tool entries must use nonempty NAME=ABSOLUTE_PATH"
                ));
            }
            Ok(CompatibilityHostTool {
                name: name.to_owned(),
                program: PathBuf::from(program),
            })
        })
        .collect()
}

fn parse_jobs(value: &str) -> Result<usize, String> {
    let jobs = value
        .parse::<usize>()
        .map_err(|_| "expected an integer from 1 through 64".to_owned())?;
    if !(1..=64).contains(&jobs) {
        return Err("expected an integer from 1 through 64".to_owned());
    }
    Ok(jobs)
}
