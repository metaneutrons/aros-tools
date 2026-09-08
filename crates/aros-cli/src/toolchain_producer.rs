//! Native producer-input commands.
//!
//! These commands deliberately cover only closed local producer stages. They
//! bind recipe bytes to committed checkouts, acquire/verify the lock-owned
//! source cache, and operate on already-built candidates; none can tag or
//! publish a toolchain release.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use aros_common::{sha256_bytes, CancellationToken};
use aros_toolchain::compatibility::{
    self, CompatibilityHostTool, CompatibilityPreparationRequest, HostToolClosureRequest,
    NativeCompatibilityRequest, StandaloneFixtures, TwoRootRelocationRequest,
};
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, EngineFreeSourceRequest,
};
use aros_toolchain::profiles::Profiles;
use aros_toolchain::python_environment::PythonEnvironment;
use aros_toolchain::recipe_builder::{self, RecipeBuildRequest};
use aros_toolchain::recovery::RecoveryRequest;
use aros_toolchain::release_index::{self, IndexRequest, IndexStage};
use aros_toolchain::repackage::{self, VerifiedPackageRepackageRequest};
use aros_toolchain::source_cache;
use aros_toolchain::source_lock::SourceLock;
use aros_toolchain::{package, package_verify, Recipe};
use clap::{Args, Subcommand, ValueEnum};

use crate::observability;

/// Closed native producer stages exposed by `aros toolchain producer`.
#[derive(Args)]
pub struct ProducerArgs {
    #[command(subcommand)]
    command: ProducerCommand,
}

/// One producer operation with no implicit backend selection.
#[derive(Subcommand)]
enum ProducerCommand {
    /// Construct one non-overwriting recipe-v2 from committed Git inputs
    Recipe(RecipeArgs),
    /// Acquire or verify the exact source-cache closure selected by a lock
    Cache(CacheArgs),
    /// Write the deterministic build-environment receipt embedded in a package
    Environment(EnvironmentArgs),
    /// Read one recipe-bound producer profile without duplicating its selectors
    Profile(ProfileArgs),
    /// Materialize an audited source snapshot without its source-tree CMake engine
    MaterializeEngineFreeSource(MaterializeEngineFreeSourceArgs),
    /// Create one deterministic local package set from a completed candidate
    Package(PackageArgs),
    /// Read back and verify one complete deterministic local package set
    VerifyPackage(PackageArgs),
    /// Compare two complete local package sets byte-for-byte
    Compare(CompareArgs),
    /// Repackage one evidence-bound retained package into two fresh package sets
    Repackage(RepackageArgs),
    /// Advance a complete local release inventory through one index stage
    Index(IndexArgs),
    /// Execute all six native package-compatibility phases locally
    Compatibility(Box<CompatibilityArgs>),
}

/// Machine- or human-readable local stage result.
#[derive(Clone, Copy, ValueEnum)]
enum ResultFormat {
    /// Stable concise text for a human or build log.
    Human,
    /// Structured JSON for a workflow handoff.
    Json,
}

/// Inputs for native closed recipe construction.
#[derive(Args)]
struct RecipeArgs {
    /// Exact AROS source checkout containing all lock-declared patches
    #[arg(long)]
    source_dir: PathBuf,
    /// Exact toolchain-producer checkout containing the lock and profiles
    #[arg(long)]
    producer_dir: PathBuf,
    /// Exact aros-tools checkout selected as the producer executor
    #[arg(long)]
    tools_dir: PathBuf,
    /// Producer-root-relative source-lock-v2 document
    #[arg(long)]
    source_lock: PathBuf,
    /// Producer-root-relative profiles-v1 document
    #[arg(long)]
    profiles: PathBuf,
    /// Absent recipe-v2 output file; an existing file is never replaced
    #[arg(long)]
    output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for native source-cache acquisition or verification.
#[derive(Args)]
struct CacheArgs {
    /// Source-lock-v2 document selecting the complete archive closure
    #[arg(long)]
    source_lock: PathBuf,
    /// Existing local cache root; only lock-selected direct children are used
    #[arg(long)]
    cache_dir: PathBuf,
    /// Refuse transport and report a typed error for every cache miss
    #[arg(long, env = "AROS_OFFLINE")]
    offline: bool,
    /// Verify the selected closure without inserting a missing cache object
    #[arg(long)]
    verify_only: bool,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for one deterministic build-environment receipt.
#[derive(Args)]
struct EnvironmentArgs {
    /// Closed v1 build-host selector recorded in the package manifest
    #[arg(long)]
    host: String,
    /// Absent receipt destination; an existing path is never replaced
    #[arg(long)]
    output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for one recipe-bound profile selection.
#[derive(Args)]
struct ProfileArgs {
    /// Self-digesting recipe-v2 JSON document
    #[arg(long)]
    recipe: PathBuf,
    /// Profiles-v1 document bound by the selected recipe
    #[arg(long)]
    profiles: PathBuf,
    /// Exact profile from the recipe-bound profiles matrix
    #[arg(long)]
    preset: String,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for one engine-free compatibility source snapshot.
#[derive(Args)]
struct MaterializeEngineFreeSourceArgs {
    /// Clean exact AROS checkout selected by the producer recipe
    #[arg(long)]
    source_dir: PathBuf,
    /// Self-digesting recipe-v2 binding the selected source checkout
    #[arg(long)]
    recipe: PathBuf,
    /// Absent output directory for the engine-free compatibility snapshot
    #[arg(long)]
    output_dir: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Recipe-bound inputs shared by package creation and read-back verification.
#[derive(Args, Clone)]
struct PackageContextArgs {
    /// Self-digesting recipe-v2 JSON document
    #[arg(long)]
    recipe: PathBuf,
    /// Source-lock-v2 document bound by the selected recipe
    #[arg(long)]
    source_lock: PathBuf,
    /// Profiles-v1 document bound by the selected recipe
    #[arg(long)]
    profiles: PathBuf,
    /// Exact profile from the recipe-bound profiles matrix
    #[arg(long)]
    preset: String,
    /// Immutable local candidate/release identifier recorded in the manifest
    #[arg(long)]
    release_id: String,
    /// Closed v1 build-host selector
    #[arg(long)]
    host: String,
    /// JSON object recording the measured native build environment
    #[arg(long)]
    build_environment: PathBuf,
    /// Absolute build root forbidden from package regular-file contents; repeatable
    #[arg(long = "forbidden-prefix")]
    forbidden_prefixes: Vec<PathBuf>,
}

/// Inputs for native package creation or read-back verification.
#[derive(Args)]
struct PackageArgs {
    #[command(flatten)]
    context: PackageContextArgs,
    /// Completed candidate prefix for `package`, or complete package directory for `verify-package`
    #[arg(long)]
    input_dir: PathBuf,
    /// Absent final package-set directory; required only by `package`
    #[arg(long)]
    output_dir: Option<PathBuf>,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for a local byte-identical package-set comparison.
#[derive(Args)]
struct CompareArgs {
    /// Complete first local package set
    #[arg(long)]
    left: PathBuf,
    /// Complete independent second local package set
    #[arg(long)]
    right: PathBuf,
    /// Absent durable receipt recording the exact compared package members
    #[arg(long)]
    output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for one evidence-bound, two-output local packaging recovery.
#[derive(Args)]
struct RepackageArgs {
    /// Closed recovery-request-v1 document with isolated inventory and policy claims
    #[arg(long)]
    recovery_request: PathBuf,
    /// Complete four-member retained source package set
    #[arg(long)]
    source_package_dir: PathBuf,
    /// Immutable release identity embedded in the retained source package
    #[arg(long)]
    source_release_id: String,
    /// Self-digesting recipe-v2 JSON document
    #[arg(long)]
    recipe: PathBuf,
    /// Source-lock-v2 document bound by the selected recipe
    #[arg(long)]
    source_lock: PathBuf,
    /// Profiles-v1 document bound by the selected recipe
    #[arg(long)]
    profiles: PathBuf,
    /// Exact profile from the recipe-bound profiles matrix
    #[arg(long)]
    preset: String,
    /// Closed v1 host selector for the retained source package
    #[arg(long)]
    host: String,
    /// JSON object recording the retained package's measured build environment
    #[arg(long)]
    build_environment: PathBuf,
    /// Absolute build root forbidden from package regular-file contents; repeatable
    #[arg(long = "forbidden-prefix")]
    forbidden_prefixes: Vec<PathBuf>,
    /// Absent first owned extraction directory
    #[arg(long)]
    first_extraction_dir: PathBuf,
    /// Absent second owned extraction directory
    #[arg(long)]
    second_extraction_dir: PathBuf,
    /// Absent first recovered package-set directory
    #[arg(long)]
    first_output_dir: PathBuf,
    /// Absent second recovered package-set directory
    #[arg(long)]
    second_output_dir: PathBuf,
    /// Absent canonical receipt proving recovered package-set byte identity
    #[arg(long)]
    comparison_output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Index operation stage selected explicitly by the protected workflow.
#[derive(Clone, Copy, ValueEnum)]
enum IndexStageArg {
    /// Write the closed index and pre-attestation checksums.
    PreAttestation,
    /// Verify pre-attestation material and bind final checksums to provenance.
    Final,
}

/// Inputs for native v1 release-index advancement.
#[derive(Args)]
struct IndexArgs {
    /// Complete local release inventory directory
    #[arg(long)]
    directory: PathBuf,
    /// Immutable release identifier shared by every selected manifest
    #[arg(long)]
    release_id: String,
    /// Credential-free HTTPS root written into the index
    #[arg(long)]
    base_url: String,
    /// Basename of the one selected source-lock document in the inventory
    #[arg(long)]
    source_lock_filename: String,
    /// Explicit closed inventory stage
    #[arg(long, value_enum)]
    stage: IndexStageArg,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for one complete native six-phase package compatibility execution.
#[derive(Args)]
struct CompatibilityArgs {
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
    /// Absent private host Python environment directory
    #[arg(long)]
    python_environment_dir: PathBuf,
    /// Absent private host-command closure directory
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
    #[arg(long, value_parser = parse_compatibility_jobs)]
    jobs: usize,
    /// Positive per-phase deadline in seconds
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Run one explicit native producer-input stage.
pub async fn run(args: ProducerArgs) -> miette::Result<()> {
    match args.command {
        ProducerCommand::Recipe(args) => recipe(args),
        ProducerCommand::Cache(args) => cache(args).await,
        ProducerCommand::Environment(args) => environment(&args),
        ProducerCommand::Profile(args) => profile(&args),
        ProducerCommand::MaterializeEngineFreeSource(args) => {
            materialize_engine_free_source_stage(args)
        }
        ProducerCommand::Package(args) => package(args),
        ProducerCommand::VerifyPackage(args) => verify_package(args),
        ProducerCommand::Compare(args) => compare(&args),
        ProducerCommand::Repackage(args) => repackage(args),
        ProducerCommand::Index(args) => index(args),
        ProducerCommand::Compatibility(args) => compatibility(*args).await,
    }
}

fn profile(args: &ProfileArgs) -> miette::Result<()> {
    let recipe = Recipe::parse(&read_regular_input(&args.recipe, "recipe")?)
        .map_err(|error| native_error(&error))?;
    let profiles_bytes = read_regular_input(&args.profiles, "profiles")?;
    if sha256_bytes(&profiles_bytes) != *recipe.profiles_sha256() {
        return Err(miette::miette!(
            "native producer profiles differ from the selected recipe digest"
        ));
    }
    let profiles = Profiles::parse(&profiles_bytes).map_err(|error| native_error(&error))?;
    let selected = profiles
        .select(&args.preset)
        .map_err(|error| native_error(&error))?;
    let document = serde_json::json!({
        "schema": "aros-toolchain-producer-stage-v1",
        "operation": "profile",
        "preset": selected.name(),
        "upstream_commit": profiles.upstream_commit(),
        "configure_target": selected.configure_target(),
        "upstream_output_target": selected.upstream_output_target(),
        "target_triple": selected.target_triple(),
        "cpu": selected.cpu(),
        "platform": selected.platform(),
        "float_abi": selected.float_abi(),
        "capabilities": selected.capabilities(),
    });
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native profile: {}\nUpstream commit: {}\nTarget: {}",
            selected.name(),
            profiles.upstream_commit().as_str(),
            selected.target_triple(),
        ),
        ResultFormat::Json => print_json(&document)?,
    }
    Ok(())
}

fn materialize_engine_free_source_stage(
    args: MaterializeEngineFreeSourceArgs,
) -> miette::Result<()> {
    let output = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: args.source_dir,
        recipe: Recipe::parse(&read_regular_input(&args.recipe, "recipe")?)
            .map_err(|error| native_error(&error))?,
        output_root: args.output_dir,
    })
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native engine-free compatibility source: {}\nSource commit: {}\nSHA-256: {}",
            output.root.display(),
            output.source_commit,
            output.source_tree_sha256
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "materialize-engine-free-source",
            "root": output.root,
            "source_commit": output.source_commit,
            "source_tree": output.source_tree,
            "source_tree_sha256": output.source_tree_sha256,
        }))?,
    }
    Ok(())
}

fn recipe(args: RecipeArgs) -> miette::Result<()> {
    let output = recipe_builder::build(&RecipeBuildRequest {
        source_root: args.source_dir,
        producer_root: args.producer_dir,
        tools_root: args.tools_dir,
        source_lock: args.source_lock,
        profiles: args.profiles,
        output: args.output,
    })
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => {
            aros_common::outputln!(
                "Native recipe: {}\nSHA-256: {}",
                output.path.display(),
                output.sha256
            );
        }
        ResultFormat::Json => {
            let document = serde_json::json!({
                "schema": "aros-toolchain-producer-stage-v1",
                "operation": "recipe",
                "path": output.path,
                "sha256": output.sha256,
            });
            print_json(&document)?;
        }
    }
    Ok(())
}

async fn cache(args: CacheArgs) -> miette::Result<()> {
    let lock = SourceLock::parse(&read_regular_input(&args.source_lock, "source lock")?)
        .map_err(|error| native_error(&error))?;
    let observation = if args.verify_only {
        source_cache::verify(&args.cache_dir, &lock)
    } else {
        source_cache::acquire(&args.cache_dir, &lock, args.offline).await
    }
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => {
            let operation = if args.verify_only {
                "verified"
            } else {
                "acquired"
            };
            let mut text = format!(
                "Native source cache {operation}: {} payload(s)",
                observation.payloads.len()
            );
            for payload in observation.payloads {
                let _ = write!(
                    text,
                    "\n  {} {} ({})",
                    payload.sha256, payload.size, payload.filename
                );
            }
            aros_common::outputln!("{text}");
        }
        ResultFormat::Json => {
            let payloads = observation
                .payloads
                .into_iter()
                .map(|payload| {
                    serde_json::json!({
                        "filename": payload.filename,
                        "sha256": payload.sha256,
                        "size": payload.size,
                    })
                })
                .collect::<Vec<_>>();
            let document = serde_json::json!({
                "schema": "aros-toolchain-producer-stage-v1",
                "operation": if args.verify_only { "cache-verify" } else { "cache-acquire" },
                "payloads": payloads,
            });
            print_json(&document)?;
        }
    }
    Ok(())
}

fn environment(args: &EnvironmentArgs) -> miette::Result<()> {
    if !release_index::V1_HOSTS.contains(&args.host.as_str()) {
        return Err(miette::miette!(
            "native producer build environment requires one closed v1 host selector"
        ));
    }
    let receipt = serde_json::json!({
        "schema": "aros-toolchain-build-environment-v1",
        "host": args.host,
    });
    let output = write_new_json(&args.output, &receipt, "build-environment receipt")?;
    match args.format {
        ResultFormat::Human => {
            aros_common::outputln!("Native build-environment receipt: {}", output.display());
        }
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "environment",
            "path": output,
            "receipt": receipt,
        }))?,
    }
    Ok(())
}

fn package(args: PackageArgs) -> miette::Result<()> {
    let output_dir = args.output_dir.ok_or_else(|| {
        miette::miette!("native producer package requires an absent --output-dir")
    })?;
    let context = package_context(args.context.clone())?;
    let output = package::package(&package::PackageRequest {
        candidate_root: args.input_dir,
        output_dir,
        release_id: context.release_id,
        host: context.host,
        recipe: context.recipe,
        source_lock: context.source_lock,
        profile: context.profile,
        build_environment: context.build_environment,
        forbidden_prefixes: context.forbidden_prefixes,
    })
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native package: {}\nSHA-256: {}\nSize: {}",
            output.archive.display(),
            output.archive_sha256,
            output.archive_size
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "package",
            "package_dir": output.output_dir,
            "archive": output.archive,
            "manifest": output.manifest,
            "checksum": output.checksum,
            "sbom": output.sbom,
            "sha256": output.archive_sha256,
            "size": output.archive_size,
        }))?,
    }
    Ok(())
}

fn verify_package(args: PackageArgs) -> miette::Result<()> {
    if args.output_dir.is_some() {
        return Err(miette::miette!(
            "native producer verify-package does not accept --output-dir"
        ));
    }
    let context = package_context(args.context)?;
    let output = package_verify::verify(&package_verify::PackageVerificationRequest {
        package_dir: args.input_dir,
        release_id: context.release_id,
        host: context.host,
        recipe: context.recipe,
        source_lock: context.source_lock,
        profile: context.profile,
        build_environment: context.build_environment,
        forbidden_prefixes: context.forbidden_prefixes,
    })
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native package verified\nSHA-256: {}\nSize: {}",
            output.archive_sha256,
            output.archive_size
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "verify-package",
            "sha256": output.archive_sha256,
            "size": output.archive_size,
            "manifest": output.manifest,
        }))?,
    }
    Ok(())
}

fn compare(args: &CompareArgs) -> miette::Result<()> {
    let comparison = release_index::compare_package_sets(&args.left, &args.right)
        .map_err(|error| native_error(&error))?;
    let receipt = release_index::write_package_comparison_report(&args.output, &comparison)
        .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native package comparison: byte-identical\nReceipt: {}\nReceipt SHA-256: {}",
            receipt.path.display(),
            receipt.sha256
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "compare",
            "byte_identical": true,
            "receipt": receipt.path,
            "receipt_sha256": receipt.sha256,
            "package_set_sha256": comparison.package_set_sha256,
            "members": comparison.members,
        }))?,
    }
    Ok(())
}

fn repackage(args: RepackageArgs) -> miette::Result<()> {
    let recovery = RecoveryRequest::parse(&read_regular_input(
        &args.recovery_request,
        "recovery request",
    )?)
    .map_err(|error| native_error(&error))?;
    let context = package_context(PackageContextArgs {
        recipe: args.recipe,
        source_lock: args.source_lock,
        profiles: args.profiles,
        preset: args.preset,
        release_id: args.source_release_id,
        host: args.host,
        build_environment: args.build_environment,
        forbidden_prefixes: args.forbidden_prefixes,
    })?;
    let output = repackage::repackage_verified_package(&VerifiedPackageRepackageRequest {
        recovery,
        source: package_verify::PackageVerificationRequest {
            package_dir: args.source_package_dir,
            release_id: context.release_id,
            host: context.host,
            recipe: context.recipe,
            source_lock: context.source_lock,
            profile: context.profile,
            build_environment: context.build_environment,
            forbidden_prefixes: context.forbidden_prefixes,
        },
        first_extraction_root: args.first_extraction_dir,
        second_extraction_root: args.second_extraction_dir,
        first_output_dir: args.first_output_dir,
        second_output_dir: args.second_output_dir,
    })
    .map_err(|error| native_error(&error))?;
    let comparison = release_index::compare_package_sets(
        &output.repackaged.first.output_dir,
        &output.repackaged.second.output_dir,
    )
    .map_err(|error| native_error(&error))?;
    let receipt =
        release_index::write_package_comparison_report(&args.comparison_output, &comparison)
            .map_err(|error| native_error(&error))?;
    let recovery_release_id = match &output.repackaged.decision {
        aros_toolchain::recovery::RecoveryDecision::Repackage { release_id, .. } => release_id,
        aros_toolchain::recovery::RecoveryDecision::ReplayCompatibility => {
            return Err(miette::miette!(
                "native repackage completed without packaging-recovery authority"
            ));
        }
    };
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native repackage: {}\nFirst package: {}\nSecond package: {}\nComparison receipt: {}\nReceipt SHA-256: {}",
            recovery_release_id,
            output.repackaged.first.output_dir.display(),
            output.repackaged.second.output_dir.display(),
            receipt.path.display(),
            receipt.sha256,
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "repackage",
            "source_archive_sha256": output.source.archive_sha256,
            "source_archive_size": output.source.archive_size,
            "release_id": recovery_release_id,
            "first_package_dir": output.repackaged.first.output_dir,
            "second_package_dir": output.repackaged.second.output_dir,
            "comparison_receipt": receipt.path,
            "comparison_receipt_sha256": receipt.sha256,
            "package_set_sha256": comparison.package_set_sha256,
        }))?,
    }
    Ok(())
}

fn index(args: IndexArgs) -> miette::Result<()> {
    let stage = match args.stage {
        IndexStageArg::PreAttestation => IndexStage::PreAttestation,
        IndexStageArg::Final => IndexStage::Final,
    };
    let output = release_index::index_complete_v1(&IndexRequest {
        directory: args.directory,
        release_id: args.release_id,
        base_url: args.base_url,
        source_lock_filename: args.source_lock_filename,
        stage,
    })
    .map_err(|error| native_error(&error))?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native release index: {}\nChecksums: {}\nSHA-256: {}",
            output.index_path.display(),
            output.checksums_path.display(),
            output.checksums_sha256
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "index",
            "release_id": output.index.release_id,
            "index": output.index_path,
            "checksums": output.checksums_path,
            "checksums_sha256": output.checksums_sha256,
        }))?,
    }
    Ok(())
}

async fn compatibility(args: CompatibilityArgs) -> miette::Result<()> {
    let format = args.format;
    let context = package_context(args.context.clone())?;
    let host_tools = parse_host_tools(&args.host_tools)?;
    let cancellation = CancellationToken::default();
    let worker_token = cancellation.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        execute_compatibility(context, args, host_tools, &worker_token)
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
    cancellation: &CancellationToken,
) -> Result<compatibility::NativeCompatibilityReport, aros_toolchain::ContractError> {
    let verification = package_verify::PackageVerificationRequest {
        package_dir: args.package_dir,
        release_id: context.release_id,
        host: context.host,
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

fn parse_compatibility_jobs(value: &str) -> Result<usize, String> {
    let jobs = value
        .parse::<usize>()
        .map_err(|_| "expected an integer from 1 through 64".to_owned())?;
    if !(1..=64).contains(&jobs) {
        return Err("expected an integer from 1 through 64".to_owned());
    }
    Ok(jobs)
}

struct PackageContext {
    recipe: Recipe,
    source_lock: SourceLock,
    profile: aros_toolchain::profiles::Profile,
    upstream_commit: aros_toolchain::recipe::GitObjectId,
    release_id: String,
    host: String,
    build_environment: serde_json::Map<String, serde_json::Value>,
    forbidden_prefixes: Vec<PathBuf>,
}

fn package_context(args: PackageContextArgs) -> miette::Result<PackageContext> {
    let recipe = Recipe::parse(&read_regular_input(&args.recipe, "recipe")?)
        .map_err(|error| native_error(&error))?;
    let source_lock_bytes = read_regular_input(&args.source_lock, "source lock")?;
    if sha256_bytes(&source_lock_bytes) != *recipe.source_lock_sha256() {
        return Err(miette::miette!(
            "native producer source lock differs from the selected recipe digest"
        ));
    }
    let source_lock =
        SourceLock::parse(&source_lock_bytes).map_err(|error| native_error(&error))?;
    source_lock
        .verify_recipe_patches(&recipe)
        .map_err(|error| native_error(&error))?;
    let profiles_bytes = read_regular_input(&args.profiles, "profiles")?;
    if sha256_bytes(&profiles_bytes) != *recipe.profiles_sha256() {
        return Err(miette::miette!(
            "native producer profiles differ from the selected recipe digest"
        ));
    }
    let profiles = Profiles::parse(&profiles_bytes).map_err(|error| native_error(&error))?;
    let profile = profiles
        .select(&args.preset)
        .map_err(|error| native_error(&error))?
        .clone();
    let environment = serde_json::from_slice::<serde_json::Value>(&read_regular_input(
        &args.build_environment,
        "build-environment receipt",
    )?)
    .map_err(|_| miette::miette!("native producer build-environment receipt is not JSON"))?;
    let build_environment = environment.as_object().cloned().ok_or_else(|| {
        miette::miette!("native producer build-environment receipt must be a JSON object")
    })?;
    if build_environment
        .get("schema")
        .and_then(serde_json::Value::as_str)
        != Some("aros-toolchain-build-environment-v1")
        || build_environment
            .get("host")
            .and_then(serde_json::Value::as_str)
            != Some(args.host.as_str())
    {
        return Err(miette::miette!(
            "native producer build-environment receipt is not bound to the selected host"
        ));
    }
    Ok(PackageContext {
        recipe,
        source_lock,
        profile,
        upstream_commit: profiles.upstream_commit().clone(),
        release_id: args.release_id,
        host: args.host,
        build_environment,
        forbidden_prefixes: args.forbidden_prefixes,
    })
}

fn read_regular_input(path: &std::path::Path, label: &str) -> miette::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| miette::miette!("native producer {label} is unavailable"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "native producer {label} must be a regular non-symlink file"
        ));
    }
    fs::read(path).map_err(|_| miette::miette!("native producer {label} cannot be read"))
}

fn write_new_json(
    path: &std::path::Path,
    document: &serde_json::Value,
    label: &str,
) -> miette::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(miette::miette!(
            "native producer {label} output must be an absolute path"
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| miette::miette!("native producer {label} output has no parent"))?;
    let parent = parent
        .canonicalize()
        .map_err(|_| miette::miette!("native producer {label} parent is unavailable"))?;
    let parent_metadata = fs::symlink_metadata(&parent)
        .map_err(|_| miette::miette!("native producer receipt parent is unavailable"))?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "native producer {label} parent must be a real directory"
        ));
    }
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| miette::miette!("native producer {label} output has no file name"))?,
    );
    let mut encoded = serde_json::to_vec_pretty(document)
        .map_err(|_| miette::miette!("cannot serialize native producer {label}"))?;
    encoded.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)
        .map_err(|_| {
            miette::miette!("native producer {label} output already exists or is unavailable")
        })?;
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|_| miette::miette!("cannot durably write native producer {label}"))?;
    Ok(output)
}

fn native_error(error: &aros_toolchain::ContractError) -> miette::Report {
    observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
}

fn print_json(document: &serde_json::Value) -> miette::Result<()> {
    let encoded = serde_json::to_string_pretty(document)
        .map_err(|_| miette::miette!("cannot serialize native producer stage result"))?;
    aros_common::outputln!("{encoded}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;

    use super::{compare, environment, CompareArgs, EnvironmentArgs, ResultFormat};
    use crate::Cli;

    #[test]
    fn producer_recipe_and_cache_stages_are_publicly_parseable() {
        let recipe = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "recipe",
            "--source-dir",
            "/source",
            "--producer-dir",
            "/producer",
            "--tools-dir",
            "/tools",
            "--source-lock",
            "/producer/toolchains/lock.sources.json",
            "--profiles",
            "/producer/toolchains/profiles.json",
            "--output",
            "/output/recipe.json",
        ]);
        assert!(recipe.is_ok());
        let cache = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "cache",
            "--source-lock",
            "/producer/toolchains/lock.sources.json",
            "--cache-dir",
            "/cache",
            "--verify-only",
        ]);
        assert!(cache.is_ok());
        let profile = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "profile",
            "--recipe",
            "/producer/recipe.json",
            "--profiles",
            "/producer/toolchains/profiles.json",
            "--preset",
            "pc-x86_64",
            "--format",
            "json",
        ]);
        assert!(profile.is_ok());
        let package = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "package",
            "--recipe",
            "/producer/recipe.json",
            "--source-lock",
            "/producer/toolchains/lock.sources.json",
            "--profiles",
            "/producer/toolchains/profiles.json",
            "--preset",
            "pc-x86_64",
            "--release-id",
            "candidate-1",
            "--host",
            "linux-x86_64",
            "--build-environment",
            "/evidence/environment.json",
            "--input-dir",
            "/candidate/toolchain",
            "--output-dir",
            "/packages/first",
        ]);
        assert!(package.is_ok());
        let compare = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "compare",
            "--left",
            "/packages/left",
            "--right",
            "/packages/right",
            "--output",
            "/evidence/comparison.json",
        ]);
        assert!(compare.is_ok());
        let repackage = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "repackage",
            "--recovery-request",
            "/evidence/recovery.json",
            "--source-package-dir",
            "/packages/source",
            "--source-release-id",
            "toolchain-v1-source",
            "--recipe",
            "/producer/recipe.json",
            "--source-lock",
            "/producer/toolchains/lock.sources.json",
            "--profiles",
            "/producer/toolchains/profiles.json",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--build-environment",
            "/evidence/environment.json",
            "--first-extraction-dir",
            "/work/extracted-a",
            "--second-extraction-dir",
            "/work/extracted-b",
            "--first-output-dir",
            "/packages/recovered-a",
            "--second-output-dir",
            "/packages/recovered-b",
            "--comparison-output",
            "/evidence/recovery-comparison.json",
        ]);
        assert!(repackage.is_ok());
        let source = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "materialize-engine-free-source",
            "--source-dir",
            "/source",
            "--recipe",
            "/producer/recipe.json",
            "--output-dir",
            "/output/engine-free",
        ]);
        assert!(source.is_ok());
        let index = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "index",
            "--directory",
            "/release",
            "--release-id",
            "candidate-1",
            "--base-url",
            "https://aros-toolchains.metaneutrons.cc/releases/candidate-1",
            "--source-lock-filename",
            "lock.sources.json",
            "--stage",
            "pre-attestation",
        ]);
        assert!(index.is_ok());
    }

    #[test]
    fn environment_receipt_is_closed_and_non_overwriting() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("environment.json");
        environment(&EnvironmentArgs {
            host: "linux-x86_64".into(),
            output: output.clone(),
            format: ResultFormat::Human,
        })
        .unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            document,
            serde_json::json!({
                "schema": "aros-toolchain-build-environment-v1",
                "host": "linux-x86_64",
            })
        );
        assert!(environment(&EnvironmentArgs {
            host: "linux-x86_64".into(),
            output,
            format: ResultFormat::Human,
        })
        .is_err());
    }

    #[test]
    fn comparison_receipt_is_closed_and_non_overwriting() {
        let temporary = tempfile::tempdir().unwrap();
        let left = temporary.path().join("left");
        let right = temporary.path().join("right");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        for directory in [&left, &right] {
            fs::write(directory.join("archive.tar.xz"), b"archive").unwrap();
            fs::write(directory.join("archive.tar.xz.manifest.json"), b"manifest").unwrap();
            fs::write(directory.join("archive.tar.xz.sha256"), b"checksum").unwrap();
            fs::write(directory.join("archive.tar.xz.spdx.json"), b"sbom").unwrap();
        }
        let output = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("comparison.json");
        let args = CompareArgs {
            left,
            right,
            output: output.clone(),
            format: ResultFormat::Human,
        };
        compare(&args).unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(document["schema"], 1);
        assert_eq!(document["operation"], "compare");
        assert_eq!(document["members"].as_array().unwrap().len(), 4);
        assert!(compare(&args).is_err());
    }
}
