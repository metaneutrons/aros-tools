//! Closed native compatibility producer commands.
//!
//! This module owns the compatibility-only CLI surface: explicit host-tool
//! closure entries, the versioned upstream source-input closure, and execution of the
//! six-phase native compatibility probe. Keeping it separate prevents the
//! general producer command router from becoming the owner of consumer policy.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use aros_common::{sdk_manifest::SDK_MAX_SOURCE_DATE_EPOCH, CancellationToken};
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

#[cfg(unix)]
const SDK_MAX_FORBIDDEN_PREFIX_BYTES: usize = 4096;

/// Reusable compiler package and six-phase execution inputs.
#[derive(Args)]
pub struct NativeExecutionArgs {
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
    /// Positive explicit Make parallelism
    #[arg(long, value_parser = parse_jobs)]
    jobs: usize,
    /// Positive per-phase deadline in seconds
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
}

/// Existing native compatibility surface plus source-owned consumer selectors.
#[derive(Args)]
pub(super) struct CompatibilityArgs {
    #[command(flatten)]
    native: NativeExecutionArgs,
    /// Explicit package format; omitted keeps LLVM v1 and GNU family-v2 defaults
    #[arg(long, value_enum)]
    package_format: Option<PackageFormatArg>,
    /// Engine-free source directory used for the tools-owned CMake consumer
    #[arg(long)]
    source_dir: PathBuf,
    /// Explicit source-owned preset from aros-targets.toml (required for GNU)
    #[arg(long)]
    source_preset: Option<String>,
    /// Explicit SDK/consumer generation epoch, distinct from the compiler's;
    /// required for source-owned SDK links and recorded in the environment digest
    #[arg(long = "consumer-source-date-epoch", value_parser = clap::value_parser!(u64).range(..=SDK_MAX_SOURCE_DATE_EPOCH))]
    consumer_source_date_epoch: Option<u64>,
    /// Absent absolute directory for inputs.json and evidence/ after complete
    /// family-v2 execution; local byte evidence only, not authenticated origin
    #[arg(long)]
    evidence_dir: Option<PathBuf>,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs specific to a single end-to-end source-bound SDK build.
#[derive(Args)]
pub struct SdkBuildArgs {
    #[command(flatten)]
    native: NativeExecutionArgs,
    /// Absent regular file for the exact SDK generation record
    #[arg(long, value_name = "FILE")]
    generation_record: PathBuf,
    /// Absent directory for the source-bound SDK package set
    #[arg(long, value_name = "DIR")]
    output_dir: PathBuf,
    /// Build path prefixes forbidden from every SDK package member; repeatable
    #[arg(long = "sdk-forbidden-prefix", value_name = "DIR", required = true)]
    sdk_forbidden_prefixes: Vec<PathBuf>,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Execute the complete native compatibility contract for one package lane.
pub(super) async fn compatibility(args: CompatibilityArgs) -> miette::Result<()> {
    let format = args.format;
    preflight_consumer_epoch(
        &args.source_dir,
        args.source_preset.as_deref(),
        args.consumer_source_date_epoch,
    )?;
    preflight_export(&args)?;
    if let Some(format) = args.package_format {
        validate_export_format(args.evidence_dir.is_some(), format.into())?;
    }
    let context = package_context(args.native.context.clone())?;
    let package_format = resolve_package_format(&context, &args)?;
    let host_tools = parse_host_tools(&args.native.host_tools)?;
    let ports_lock_bytes = read_regular_input(&args.native.ports_lock, "compatibility ports lock")?;
    let ports_lock =
        CompatibilityPortsLock::parse(&ports_lock_bytes).map_err(|error| native_error(&error))?;
    let cache_request = SourceCacheRequest::from_compatibility_ports_lock(&ports_lock_bytes)
        .map_err(|error| native_error(&error))?;
    source_cache::verify_request(&args.native.ports_cache_dir, &cache_request)
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

/// Verify a source-owned SDK, run the shared native compatibility preparation,
/// then generate and package the exact source-bound SDK in this process.
#[cfg(unix)]
pub async fn build_sdk(
    source: aros_toolchain::sdk_source::VerifiedSdkSource,
    original_source_dir: PathBuf,
    identity_path: PathBuf,
    args: SdkBuildArgs,
) -> miette::Result<()> {
    preflight_sdk_build_destinations(&source, &original_source_dir, &identity_path, &args)?;
    let context = package_context(args.native.context.clone())?;
    preflight_sdk_compiler_selection(
        context.source_lock.family(),
        context.profile.name(),
        source.source_profile().toolchain_profile(),
    )?;
    let host_tools = parse_host_tools(&args.native.host_tools)?;
    let ports_lock_bytes = read_regular_input(&args.native.ports_lock, "compatibility ports lock")?;
    let ports_lock =
        CompatibilityPortsLock::parse(&ports_lock_bytes).map_err(|error| native_error(&error))?;
    let cache_request = SourceCacheRequest::from_compatibility_ports_lock(&ports_lock_bytes)
        .map_err(|error| native_error(&error))?;
    source_cache::verify_request(&args.native.ports_cache_dir, &cache_request)
        .map_err(|error| native_error(&error))?;

    let format = args.format;
    let cancellation = CancellationToken::default();
    let worker_token = cancellation.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        execute_sdk_build(source, context, args, host_tools, ports_lock, &worker_token)
    });
    let result = tokio::select! {
        result = &mut worker => result.map_err(|_| miette::miette!("SDK generation worker terminated unexpectedly"))?,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                cancellation.cancel();
            }
            (&mut worker).await.map_err(|_| miette::miette!("SDK generation worker terminated unexpectedly"))?
        }
    }?;

    match format {
        ResultFormat::Human => {
            aros_common::outputln!(
                "SDK generation: {} {}\nGeneration record: {}\nRecord SHA-256: {}\nSDK tree SHA-256: {}\nPackage: {}\nArchive SHA-256: {}\nArchive size: {} bytes\nAssurance: local-generation-binding-not-release-admission",
                result.identity.sdk_id,
                result.identity.version,
                result.record_path.display(),
                result.record_sha256,
                result.tree_sha256,
                result.package.output_dir.display(),
                result.package.archive_sha256,
                result.package.archive_size,
            );
        }
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-sdk-build-result-v1",
            "operation": "sdk.build",
            "assurance": "local-generation-binding-not-release-admission",
            "identity": result.identity,
            "generation_record": result.record_path,
            "generation_record_sha256": result.record_sha256,
            "tree_sha256": result.tree_sha256,
            "output_dir": result.package.output_dir,
            "archive": result.package.archive,
            "manifest": result.package.manifest,
            "checksum": result.package.checksum,
            "archive_sha256": result.package.archive_sha256,
            "archive_size": result.package.archive_size,
        }))?,
    }
    Ok(())
}

#[cfg(unix)]
struct SdkBuildResult {
    identity: aros_common::sdk_manifest::SdkIdentity,
    record_path: PathBuf,
    record_sha256: aros_common::Sha256Digest,
    tree_sha256: aros_common::Sha256Digest,
    package: aros_toolchain::sdk_package::SdkPackageOutput,
}

#[cfg(unix)]
fn execute_sdk_build(
    source: aros_toolchain::sdk_source::VerifiedSdkSource,
    context: PackageContext,
    args: SdkBuildArgs,
    host_tool_entries: Vec<CompatibilityHostTool>,
    ports_lock: CompatibilityPortsLock,
    cancellation: &CancellationToken,
) -> miette::Result<SdkBuildResult> {
    if cancellation.is_cancelled() {
        return Err(miette::miette!(
            "SDK generation cancelled before compatibility preparation; no execution roots were created"
        ));
    }
    source.revalidate().map_err(|error| native_error(&error))?;
    let SdkBuildArgs {
        native,
        generation_record,
        output_dir,
        sdk_forbidden_prefixes,
        format: _,
    } = args;
    let source_preset = source.source_profile().name.clone();
    let epoch = source.identity().source_date_epoch;
    let (compatibility_request, profiles) = prepare_native_request(
        context,
        native,
        host_tool_entries,
        ports_lock,
        aros_toolchain::package::PackageFormat::CompilerFamilyV2,
        NativeConsumerSelection {
            root: source.materialized_source_root().to_path_buf(),
            preset: Some(source_preset),
            epoch: Some(epoch),
        },
    )?;
    if cancellation.is_cancelled() {
        return Err(miette::miette!(
            "SDK generation cancelled during compatibility preparation; prepared roots are retained"
        ));
    }
    let completed = aros_toolchain::sdk_generation::generate(
        &aros_toolchain::sdk_generation::SdkGenerationRequest {
            source,
            compatibility: compatibility_request,
            profiles,
            record_path: generation_record,
        },
        cancellation,
    )
    .map_err(|error| native_error(&error))?;
    if cancellation.is_cancelled() {
        return Err(miette::miette!(
            "SDK generation cancelled before packaging; the completed generation record and execution roots are retained"
        ));
    }
    let package = aros_toolchain::sdk_generation::package_generated(
        &completed,
        &output_dir,
        &sdk_forbidden_prefixes,
    )
    .map_err(|error| native_error(&error))?;
    if cancellation.is_cancelled() {
        return Err(miette::miette!(
            "SDK build cancelled while packaging; the package output may already be complete and is retained"
        ));
    }
    Ok(SdkBuildResult {
        identity: completed.identity().clone(),
        record_path: completed.record_path().to_path_buf(),
        record_sha256: completed.record_sha256().clone(),
        tree_sha256: completed.tree_sha256().clone(),
        package,
    })
}

#[cfg(unix)]
fn preflight_sdk_build_destinations(
    source: &aros_toolchain::sdk_source::VerifiedSdkSource,
    original_source_dir: &Path,
    identity_path: &Path,
    args: &SdkBuildArgs,
) -> miette::Result<()> {
    let native = &args.native;
    let mut protected = vec![
        original_source_dir.to_path_buf(),
        identity_path.to_path_buf(),
        source.materialized_source_root().to_path_buf(),
        native.context.recipe.clone(),
        native.context.source_lock.clone(),
        native.context.profiles.clone(),
        native.context.build_environment.clone(),
        native.package_dir.clone(),
        native.first_root.clone(),
        native.second_root.clone(),
        native.engine_work_dir.clone(),
        native.helpers_dir.clone(),
        native.cmake_program.clone(),
        native.ninja_program.clone(),
        native.upstream_source_dir.clone(),
        native.upstream_build_dir.clone(),
        native.python_cache_dir.clone(),
        native.ports_lock.clone(),
        native.ports_cache_dir.clone(),
        native.ports_sources_dir.clone(),
        native.python_environment_dir.clone(),
        native.host_tools_dir.clone(),
        native.cmake_build_dir.clone(),
        native.c_fixture.clone(),
        native.cxx_fixture.clone(),
        native.standalone_output_dir.clone(),
        native.reports_dir.clone(),
    ];
    for entry in &native.host_tools {
        let (_, path) = entry.split_once('=').ok_or_else(|| {
            miette::miette!("native compatibility host-tool entries must use NAME=ABSOLUTE_PATH")
        })?;
        protected.push(PathBuf::from(path));
    }

    let mut record_protected = protected.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    record_protected.push(&args.output_dir);
    preflight_sdk_destination(
        "generation record",
        &args.generation_record,
        &record_protected,
        &args.sdk_forbidden_prefixes,
    )?;

    let mut package_protected = protected.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    package_protected.push(&args.generation_record);
    preflight_sdk_destination(
        "package output",
        &args.output_dir,
        &package_protected,
        &args.sdk_forbidden_prefixes,
    )
}

#[cfg(unix)]
fn preflight_sdk_destination(
    label: &str,
    destination: &Path,
    protected: &[&Path],
    scan_prefixes: &[PathBuf],
) -> miette::Result<()> {
    validate_sdk_forbidden_prefixes(scan_prefixes)?;
    compatibility_export::preflight(destination, protected).map_err(|_| {
        miette::miette!(
            "SDK {label} must be an absent absolute destination with an existing nonsymlink parent, separate from every selected input and execution root"
        )
    })
}

#[cfg(unix)]
fn validate_sdk_forbidden_prefixes(prefixes: &[PathBuf]) -> miette::Result<()> {
    for path in prefixes {
        let value = path
            .to_str()
            .ok_or_else(|| miette::miette!("forbidden SDK prefixes must be UTF-8"))?;
        if !path.is_absolute() || value.is_empty() || value.len() > SDK_MAX_FORBIDDEN_PREFIX_BYTES {
            return Err(miette::miette!(
                "forbidden SDK prefixes must be bounded nonempty absolute paths"
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn preflight_sdk_compiler_selection(
    family: aros_toolchain::source_lock::CompilerFamily,
    compiler_profile: &str,
    source_compiler_profile: &str,
) -> miette::Result<()> {
    if family != aros_toolchain::source_lock::CompilerFamily::Gnu {
        return Err(miette::miette!(
            "SDK generation currently requires a compiler-family-v2 GNU package"
        ));
    }
    if compiler_profile != source_compiler_profile {
        return Err(miette::miette!(
            "SDK source preset selects a different compiler profile"
        ));
    }
    Ok(())
}

// Reject the source-dependent missing option before extracting packages or
// preparing any owned runtime roots. This is read-only input validation, not
// source authentication; execution revalidates the bound source contract.
fn preflight_consumer_epoch(
    source_root: &std::path::Path,
    source_preset: Option<&str>,
    epoch: Option<u64>,
) -> miette::Result<()> {
    if epoch.is_some_and(|value| value > SDK_MAX_SOURCE_DATE_EPOCH) {
        return Err(miette::miette!(
            "consumer source-date epoch exceeds the SDK portable tar timestamp range"
        ));
    }
    if epoch.is_some() {
        return Ok(());
    }
    let Some(name) = source_preset else {
        return Ok(());
    };
    let bytes = read_regular_input(
        &source_root.join("aros-targets.toml"),
        "consumer target profiles",
    )?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| miette::miette!("consumer target profiles must be UTF-8"))?;
    let configuration = aros_common::TargetProfile::parse_config(text, "aros-targets.toml")
        .map_err(|error| miette::miette!("{error}"))?;
    let profile = configuration
        .targets
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| miette::miette!("compatibility source preset is not declared"))?;
    let Some(path) = &profile.native_consumer_contract else {
        return Ok(());
    };
    let loaded = aros_common::native_consumer_contract::load_bound_native_consumer_contract(
        source_root,
        std::path::Path::new(path),
        &profile,
    )
    .map_err(|error| miette::miette!("{error}"))?;
    if loaded.contract.native_sdk_link_probes.is_some() {
        return Err(miette::miette!(
            "compatibility requires --consumer-source-date-epoch for the source-owned SDK contract"
        ));
    }
    Ok(())
}

fn preflight_export(args: &CompatibilityArgs) -> miette::Result<()> {
    let Some(destination) = &args.evidence_dir else {
        return Ok(());
    };
    compatibility_export::preflight(
        destination,
        &[
            &args.native.context.recipe,
            &args.native.context.source_lock,
            &args.native.context.profiles,
            &args.native.context.build_environment,
            &args.native.package_dir,
            &args.native.first_root,
            &args.native.second_root,
            &args.source_dir,
            &args.native.engine_work_dir,
            &args.native.helpers_dir,
            &args.native.cmake_program,
            &args.native.ninja_program,
            &args.native.upstream_source_dir,
            &args.native.upstream_build_dir,
            &args.native.python_cache_dir,
            &args.native.ports_lock,
            &args.native.ports_cache_dir,
            &args.native.ports_sources_dir,
            &args.native.python_environment_dir,
            &args.native.host_tools_dir,
            &args.native.cmake_build_dir,
            &args.native.c_fixture,
            &args.native.cxx_fixture,
            &args.native.standalone_output_dir,
            &args.native.reports_dir,
        ],
    )?;
    for entry in &args.native.host_tools {
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
    let source_root = args.source_dir;
    let source_preset = args.source_preset;
    let consumer_source_date_epoch = args.consumer_source_date_epoch;
    let evidence_dir = args.evidence_dir;
    let (request, profiles) = prepare_native_request(
        context,
        args.native,
        host_tool_entries,
        ports_lock,
        package_format,
        NativeConsumerSelection {
            root: source_root,
            preset: source_preset,
            epoch: consumer_source_date_epoch,
        },
    )?;
    if let Some(destination) = evidence_dir {
        let execution = compatibility::execute_native_compatibility_with_export(
            &request,
            &profiles,
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
            &profiles,
            cancellation,
        )
        .map_err(|error| native_error(&error))?;
        Ok(CompatibilityResult {
            report,
            export: None,
        })
    }
}

struct NativeConsumerSelection {
    root: PathBuf,
    preset: Option<String>,
    epoch: Option<u64>,
}

fn prepare_native_request(
    context: PackageContext,
    args: NativeExecutionArgs,
    host_tool_entries: Vec<CompatibilityHostTool>,
    ports_lock: CompatibilityPortsLock,
    package_format: aros_toolchain::package::PackageFormat,
    consumer: NativeConsumerSelection,
) -> miette::Result<(
    NativeCompatibilityRequest,
    aros_toolchain::profiles::Profiles,
)> {
    let NativeExecutionArgs {
        context: _,
        package_dir,
        first_root,
        second_root,
        engine_work_dir,
        helpers_dir,
        cmake_program,
        ninja_program,
        upstream_source_dir,
        upstream_build_dir,
        python_cache_dir,
        ports_lock: _,
        ports_cache_dir,
        ports_sources_dir,
        python_environment_dir,
        host_tools_dir,
        host_tools: _,
        cmake_build_dir,
        c_fixture,
        cxx_fixture,
        standalone_output_dir,
        reports_dir,
        jobs,
        timeout_seconds,
    } = args;
    let package_source_commit = context.recipe.source().0.clone();
    let verification = package_verify::PackageVerificationRequest {
        package_dir,
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
            first_root,
            second_root,
        },
        package_format,
    )
    .map_err(|error| native_error(&error))?;
    let preparation = compatibility::prepare(&CompatibilityPreparationRequest {
        source_root: consumer.root,
        work_root: engine_work_dir,
        helpers_root: helpers_dir,
    })
    .map_err(|error| native_error(&error))?;
    let python = PythonEnvironment::prepare(
        &verification.source_lock,
        &python_cache_dir,
        &python_environment_dir,
    )
    .map_err(|error| native_error(&error))?;
    let ports_sources = compatibility_ports::materialize(
        &ports_cache_dir,
        &ports_lock,
        &context.upstream_commit,
        verification.profile.name(),
        &ports_sources_dir,
    )
    .map_err(|error| native_error(&error))?;
    drop(ports_lock);
    let host_tools = compatibility::prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: host_tools_dir,
        tools: host_tool_entries,
    })
    .map_err(|error| native_error(&error))?;
    let request = NativeCompatibilityRequest {
        package_source_commit: Some(package_source_commit),
        source_preset: consumer.preset,
        consumer_source_date_epoch: consumer.epoch,
        host_generator_cache_root: Some(ports_cache_dir),
        preparation,
        relocation,
        profile: verification.profile,
        cmake_program,
        ninja_program,
        cmake_build_root: cmake_build_dir,
        upstream_source_root: upstream_source_dir,
        upstream_source_commit: context.upstream_commit,
        upstream_build_root: upstream_build_dir,
        host_python: python,
        host_tools,
        ports_sources,
        host: context.host,
        make_jobs: jobs,
        standalone_fixtures: StandaloneFixtures {
            c: c_fixture,
            cxx: cxx_fixture,
        },
        standalone_output_root: standalone_output_dir,
        reports_root: reports_dir,
        timeout: Duration::from_secs(timeout_seconds),
    };
    Ok((request, context.profiles))
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
#[path = "native_compatibility_epoch_tests.rs"]
mod epoch_tests;

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    #[test]
    fn consumer_epoch_is_explicit_bounded_and_has_no_ambient_default() {
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
            .find(|argument| argument.get_id() == "consumer_source_date_epoch")
            .unwrap();
        assert_eq!(argument.get_long(), Some("consumer-source-date-epoch"));
        assert!(argument.get_default_values().is_empty());
        assert!(argument.get_env().is_none());
        let parser = clap::Command::new("epoch-test").arg(argument.clone());
        for value in ["0", "946684800", "8589934591"] {
            let matches = parser
                .clone()
                .try_get_matches_from(["epoch-test", "--consumer-source-date-epoch", value])
                .unwrap();
            assert_eq!(
                matches.get_one::<u64>("consumer_source_date_epoch"),
                Some(&value.parse::<u64>().unwrap())
            );
        }
        for value in ["-1", "8589934592", "18446744073709551616", "garbage"] {
            assert!(parser
                .clone()
                .try_get_matches_from(["epoch-test", "--consumer-source-date-epoch", value])
                .is_err());
        }
    }

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
