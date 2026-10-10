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

use super::compatibility_export::{self, PublishedCompatibilityExport};
use super::{
    native_error, package_context, print_json, read_regular_input, PackageContext,
    PackageContextArgs, PackageFormatArg, ResultFormat,
};

/// Inputs for one complete native six-phase package compatibility execution.
#[derive(Args)]
pub(super) struct CompatibilityArgs {
    #[command(flatten)]
    context: PackageContextArgs,
    /// Complete verified package set to extract twice independently
    #[arg(long)]
    package_dir: PathBuf,
    /// Explicit package format; omitted keeps LLVM v1 and GNU family-v2 defaults
    #[arg(long, value_enum)]
    package_format: Option<PackageFormatArg>,
    /// Absent first relocation root for the CMake consumer
    #[arg(long)]
    first_root: PathBuf,
    /// Absent second relocation root for upstream and standalone consumers
    #[arg(long)]
    second_root: PathBuf,
    /// Engine-free source directory used for the tools-owned CMake consumer
    #[arg(long)]
    source_dir: PathBuf,
    /// Explicit source-owned preset from aros-targets.toml (required for GNU)
    #[arg(long)]
    source_preset: Option<String>,
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
    /// Explicit absolute prepared cache containing ports-lock inputs and any source-declared
    /// raw host-generator inputs, each verified by its own size/hash contract
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
    /// Absent absolute directory for inputs.json and evidence/ after complete
    /// family-v2 execution; local byte evidence only, not authenticated origin
    #[arg(long)]
    evidence_dir: Option<PathBuf>,
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
    preflight_export(&args)?;
    if let Some(format) = args.package_format {
        validate_export_format(args.evidence_dir.is_some(), format.into())?;
    }
    let context = package_context(args.context.clone())?;
    let package_format = resolve_package_format(&context, &args)?;
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
        execute_compatibility(
            context,
            args,
            host_tools,
            ports_lock,
            package_format,
            &worker_token,
        )
    });
    let result = tokio::select! {
        result = &mut worker => result.map_err(|_| miette::miette!("native compatibility worker terminated unexpectedly"))?,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                cancellation.cancel();
            }
            (&mut worker).await.map_err(|_| miette::miette!("native compatibility worker terminated unexpectedly"))?
        }
    }?;
    let report = &result.report;
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
        ResultFormat::Human => {
            aros_common::outputln!(
                "Native compatibility: {} phases, {} standalone target(s)\nReceipt: {}\nSHA-256: {}",
                phases.len(), standalone_targets.len(), report.receipt.path.display(), report.receipt.sha256,
            );
            if let Some(sdk) = &report.native_sdk {
                aros_common::outputln!(
                    "Native SDK: four application links, {} inventory entries\nSDK receipt: {}\nSDK receipt SHA-256: {}",
                    sdk.sdk_entries, sdk.receipt.display(), sdk.receipt_sha256,
                );
            }
            if let Some(export) = &result.export {
                aros_common::outputln!(
                    "Local evidence: {}\nInputs SHA-256: {}\nEvidence manifest SHA-256: {}",
                    export.directory.display(),
                    export.inputs_sha256,
                    export.manifest_sha256,
                );
            }
        }
        ResultFormat::Json => {
            let mut document = serde_json::json!({
                "schema": "aros-toolchain-producer-stage-v1",
                "operation": "compatibility",
                "phases": phases,
                "standalone_targets": standalone_targets,
                "receipt": report.receipt.path,
                "receipt_sha256": report.receipt.sha256,
            });
            if let Some(sdk) = &report.native_sdk {
                document["native_sdk"] = serde_json::json!({
                    "receipt": sdk.receipt,
                    "receipt_sha256": sdk.receipt_sha256,
                    "sdk_inventory_sha256": sdk.sdk_inventory_sha256,
                    "sdk_entries": sdk.sdk_entries,
                });
            }
            if let Some(export) = &result.export {
                document["local_evidence"] = serde_json::json!({
                    "directory": export.directory,
                    "inputs_sha256": export.inputs_sha256,
                    "manifest_sha256": export.manifest_sha256,
                });
            }
            print_json(&document)?;
        }
    }
    Ok(())
}

struct CompatibilityResult {
    report: compatibility::NativeCompatibilityReport,
    export: Option<PublishedCompatibilityExport>,
}

fn preflight_export(args: &CompatibilityArgs) -> miette::Result<()> {
    let Some(destination) = &args.evidence_dir else {
        return Ok(());
    };
    compatibility_export::preflight(
        destination,
        &[
            &args.context.recipe,
            &args.context.source_lock,
            &args.context.profiles,
            &args.context.build_environment,
            &args.package_dir,
            &args.first_root,
            &args.second_root,
            &args.source_dir,
            &args.engine_work_dir,
            &args.helpers_dir,
            &args.cmake_program,
            &args.ninja_program,
            &args.upstream_source_dir,
            &args.upstream_build_dir,
            &args.python_cache_dir,
            &args.ports_lock,
            &args.ports_cache_dir,
            &args.ports_sources_dir,
            &args.python_environment_dir,
            &args.host_tools_dir,
            &args.cmake_build_dir,
            &args.c_fixture,
            &args.cxx_fixture,
            &args.standalone_output_dir,
            &args.reports_dir,
        ],
    )?;
    for entry in &args.host_tools {
        if let Some((_, path)) = entry.split_once('=') {
            compatibility_export::preflight(destination, &[std::path::Path::new(path)])?;
        }
    }
    Ok(())
}

fn resolve_package_format(
    context: &PackageContext,
    args: &CompatibilityArgs,
) -> miette::Result<aros_toolchain::package::PackageFormat> {
    let format = args.package_format.map_or_else(
        || aros_toolchain::package::PackageFormat::default_for(context.source_lock.family()),
        Into::into,
    );
    validate_export_format(args.evidence_dir.is_some(), format)?;
    Ok(format)
}

fn validate_export_format(
    export: bool,
    format: aros_toolchain::package::PackageFormat,
) -> miette::Result<()> {
    if export && format != aros_toolchain::package::PackageFormat::CompilerFamilyV2 {
        return Err(miette::miette!(
            "compatibility --evidence-dir requires --package-format family-v2; legacy-v1 cannot export portable evidence"
        ));
    }
    Ok(())
}

fn execute_compatibility(
    context: PackageContext,
    args: CompatibilityArgs,
    host_tool_entries: Vec<CompatibilityHostTool>,
    ports_lock: CompatibilityPortsLock,
    package_format: aros_toolchain::package::PackageFormat,
    cancellation: &CancellationToken,
) -> miette::Result<CompatibilityResult> {
    let package_source_commit = context.recipe.source().0.clone();
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
    let relocation = compatibility::extract_two_roots_with_format(
        &TwoRootRelocationRequest {
            verification: verification.clone(),
            first_root: args.first_root,
            second_root: args.second_root,
        },
        package_format,
    )
    .map_err(|error| native_error(&error))?;
    let preparation = compatibility::prepare(&CompatibilityPreparationRequest {
        source_root: args.source_dir,
        work_root: args.engine_work_dir,
        helpers_root: args.helpers_dir,
    })
    .map_err(|error| native_error(&error))?;
    let python = PythonEnvironment::prepare(
        &verification.source_lock,
        &args.python_cache_dir,
        &args.python_environment_dir,
    )
    .map_err(|error| native_error(&error))?;
    let ports_sources = compatibility_ports::materialize(
        &args.ports_cache_dir,
        &ports_lock,
        &context.upstream_commit,
        verification.profile.name(),
        &args.ports_sources_dir,
    )
    .map_err(|error| native_error(&error))?;
    drop(ports_lock);
    let host_tools = compatibility::prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: args.host_tools_dir,
        tools: host_tool_entries,
    })
    .map_err(|error| native_error(&error))?;
    let request = NativeCompatibilityRequest {
        package_source_commit: Some(package_source_commit),
        source_preset: args.source_preset,
        host_generator_cache_root: Some(args.ports_cache_dir),
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
    };
    if let Some(destination) = args.evidence_dir {
        let execution = compatibility::execute_native_compatibility_with_export(
            &request,
            &context.profiles,
            cancellation,
        )
        .map_err(|error| native_error(&error))?;
        if cancellation.is_cancelled() {
            return Err(miette::miette!("compatibility cancelled before evidence publication; retained execution outputs are not exported"));
        }
        let export = compatibility_export::publish(&destination, &execution)?;
        Ok(CompatibilityResult {
            report: execution.report().clone(),
            export: Some(export),
        })
    } else {
        let report = compatibility::execute_native_compatibility_with_readback(
            &request,
            &context.profiles,
            cancellation,
        )
        .map_err(|error| native_error(&error))?;
        Ok(CompatibilityResult {
            report,
            export: None,
        })
    }
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    #[test]
    fn compatibility_export_is_explicit_optional_and_not_origin_authentication() {
        let root = crate::Cli::command();
        let command = root
            .find_subcommand("toolchain")
            .unwrap()
            .find_subcommand("producer")
            .unwrap()
            .find_subcommand("compatibility")
            .unwrap();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "evidence_dir")
            .unwrap();
        assert_eq!(argument.get_long(), Some("evidence-dir"));
        assert!(!argument.is_required_set());
        assert!(argument.get_default_values().is_empty());
        let help = argument
            .get_long_help()
            .or_else(|| argument.get_help())
            .unwrap()
            .to_string();
        assert!(help.contains("family-v2"));
        assert!(help.contains("not authenticated origin"));
    }

    #[test]
    fn compatibility_export_rejects_legacy_without_changing_ordinary_execution() {
        use aros_toolchain::package::PackageFormat::{CompilerFamilyV2, LegacyLlvmV1};
        assert!(super::validate_export_format(false, LegacyLlvmV1).is_ok());
        assert!(super::validate_export_format(false, CompilerFamilyV2).is_ok());
        assert!(super::validate_export_format(true, CompilerFamilyV2).is_ok());
        let error = super::validate_export_format(true, LegacyLlvmV1).unwrap_err();
        assert!(error
            .to_string()
            .contains("requires --package-format family-v2"));
    }

    #[test]
    fn compatibility_documents_the_explicit_source_declared_input_cache() {
        let root = crate::Cli::command();
        let command = root
            .find_subcommand("toolchain")
            .unwrap()
            .find_subcommand("producer")
            .unwrap()
            .find_subcommand("compatibility")
            .unwrap();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "ports_cache_dir")
            .unwrap();
        assert_eq!(argument.get_long(), Some("ports-cache-dir"));
        assert!(argument.is_required_set());
        assert!(argument.get_default_values().is_empty());
        let help = argument
            .get_long_help()
            .or_else(|| argument.get_help())
            .unwrap()
            .to_string();
        assert!(help.contains("source-declared"));
        assert!(help.contains("size/hash"));
        assert!(help.contains("absolute"));
    }

    #[test]
    fn compatibility_exposes_closed_package_formats_without_implicit_detection() {
        let root = crate::Cli::command();
        let command = root
            .find_subcommand("toolchain")
            .unwrap()
            .find_subcommand("producer")
            .unwrap()
            .find_subcommand("compatibility")
            .unwrap();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "package_format")
            .unwrap();
        assert_eq!(argument.get_long(), Some("package-format"));
        assert!(!argument.is_required_set());
        assert!(argument.get_default_values().is_empty());
        let values = argument
            .get_value_parser()
            .possible_values()
            .unwrap()
            .map(|value| value.get_name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(values, ["legacy-v1", "family-v2"]);
    }

    #[test]
    fn compatibility_exposes_an_explicit_source_preset_without_changing_legacy_defaults() {
        let root = crate::Cli::command();
        let command = root
            .find_subcommand("toolchain")
            .unwrap()
            .find_subcommand("producer")
            .unwrap()
            .find_subcommand("compatibility")
            .unwrap();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "source_preset")
            .unwrap();
        assert_eq!(argument.get_long(), Some("source-preset"));
        assert!(!argument.is_required_set());
        assert!(argument.get_default_values().is_empty());
    }
}
