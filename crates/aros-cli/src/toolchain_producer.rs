//! Native producer-input commands.
//!
//! These commands deliberately cover only closed local producer stages. They
//! bind recipe bytes to committed checkouts, acquire/verify the lock-owned
//! source cache, and operate on already-built candidates; none can tag or
//! publish a toolchain release.

use aros_common::{open_regular_file_nofollow, sha256_bytes, Sha256Digest};
use aros_toolchain::compatibility::native_compatibility_host_tools;
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, EngineFreeSourceRequest,
};
use aros_toolchain::profiles::Profiles;
use aros_toolchain::qualification_evidence::{
    AttestationClaim, EvidenceCoverage, EvidencePolicy, QualificationEvidence, QualificationLane,
    ReleaseEvidence, SourceRunIdentity, QUALIFICATION_EVIDENCE_SCHEMA,
};
use aros_toolchain::recipe_builder::{self, RecipeBuildRequest};
use aros_toolchain::recovery::{
    self, FailedStage, ObservedTag, RecoveryHandoff, RecoveryOperation, RecoveryRequest,
    ReleaseHandoffState,
};
use aros_toolchain::release_index::{self, IndexRequest, IndexStage, NativeReleaseIndex};
use aros_toolchain::repackage::{self, VerifiedPackageRepackageRequest};
use aros_toolchain::source_cache;
use aros_toolchain::source_lock::SourceLock;
use aros_toolchain::{package, package_verify, Recipe};
use clap::{Args, Subcommand, ValueEnum};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use crate::observability;

mod native_compatibility;

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
    /// Acquire or verify the exact upstream ports-source closure for compatibility
    CompatibilityPorts(native_compatibility::CompatibilityPortsArgs),
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
    /// Re-evaluate recovery eligibility against one isolated complete release inventory
    ValidateRecovery(ValidateRecoveryArgs),
    /// Record complete native qualification evidence from an isolated final release
    RecordQualification(RecordQualificationArgs),
    /// Create one closed recovery request after external attestation verification
    PrepareRecovery(PrepareRecoveryArgs),
    /// Advance a complete local release inventory through one index stage
    Index(IndexArgs),
    /// Print the exact measured command roles required by native compatibility
    CompatibilityHostTools {
        #[arg(long)]
        host: String,
    },
    /// Execute all six native package-compatibility phases locally
    Compatibility(Box<native_compatibility::CompatibilityArgs>),
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

/// Inputs for one isolated recovery inventory revalidation.
#[derive(Args)]
struct ValidateRecoveryArgs {
    /// Closed recovery-request-v1 document with isolated inventory and policy claims
    #[arg(long)]
    recovery_request: PathBuf,
    /// Complete isolated 56-member source release inventory
    #[arg(long)]
    release_dir: PathBuf,
    /// Absent durable receipt for the exact revalidated recovery inventory
    #[arg(long)]
    output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for recording a complete final native qualification candidate.
///
/// The three report roots use the default names produced by the pinned GitHub
/// artifact action.  Keeping this layout explicit means the native command,
/// rather than workflow string processing, owns the release index's active or
/// historical host/profile evidence closure.
#[derive(Args)]
struct RecordQualificationArgs {
    /// Complete isolated final release inventory
    #[arg(long)]
    release_dir: PathBuf,
    /// Basename of the source-lock document in the final release inventory
    #[arg(long)]
    source_lock_filename: String,
    /// Download root containing `native-lifecycle-<host>-<profile>-{a,b}` artifacts
    #[arg(long)]
    lifecycle_reports_dir: PathBuf,
    /// Download root containing `comparison-<host>-<profile>` artifacts
    #[arg(long)]
    comparison_reports_dir: PathBuf,
    /// Download root containing `compatibility-<host>-<profile>` artifacts
    #[arg(long)]
    compatibility_reports_dir: PathBuf,
    /// Credential-free HTTPS repository that ran the producer workflow
    #[arg(long)]
    source_repository: String,
    /// Repository-relative producer workflow path
    #[arg(long)]
    source_workflow: String,
    /// Immutable GitHub Actions producer run identifier
    #[arg(long)]
    source_run_id: u64,
    /// Immutable source tag used by the producer run
    #[arg(long)]
    source_tag: String,
    /// Observed annotated source tag object identity
    #[arg(long)]
    source_tag_object: String,
    /// Observed peeled source tag commit identity
    #[arg(long)]
    source_tag_commit: String,
    /// Credential-free HTTPS repository accepted by the external attestation verifier
    #[arg(long)]
    attestation_repository: String,
    /// Repository-relative signer workflow accepted by the external verifier
    #[arg(long)]
    attestation_workflow: String,
    /// Signer identity accepted by the external verifier
    #[arg(long)]
    attestation_signer: String,
    /// Unix time at which the protected workflow recorded the evidence
    #[arg(long)]
    created_at: u64,
    /// Strict expiration time for later replay or packaging recovery
    #[arg(long)]
    expires_at: u64,
    /// Absent durable qualification-evidence-v1 output
    #[arg(long)]
    output: PathBuf,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for converting measured qualification evidence into one recovery request.
///
/// The protected workflow must cryptographically verify the attestation before
/// calling this local command.  This command records the verifier's fixed
/// policy claims and rejects a mismatch; it does not have network or forge
/// authority of its own.
#[derive(Args)]
struct PrepareRecoveryArgs {
    /// Closed qualification-evidence-v1 document from the qualified source run
    #[arg(long)]
    qualification_evidence: PathBuf,
    /// Complete isolated 56-member final release inventory from that source run
    #[arg(long)]
    release_dir: PathBuf,
    /// Re-observed annotated source tag object identity
    #[arg(long)]
    source_tag_object: String,
    /// Re-observed peeled source tag commit identity
    #[arg(long)]
    source_tag_commit: String,
    /// Fresh immutable recovery release and annotated tag name
    #[arg(long)]
    recovery_release_id: String,
    /// Re-observed annotated recovery tag object identity
    #[arg(long)]
    recovery_tag_object: String,
    /// Re-observed peeled recovery tag commit identity
    #[arg(long)]
    recovery_tag_commit: String,
    /// Credential-free HTTPS repository expected for the source workflow
    #[arg(long)]
    source_repository: String,
    /// Repository-relative source workflow expected for the qualified run
    #[arg(long)]
    source_workflow: String,
    /// Credential-free HTTPS repository accepted by the external verifier
    #[arg(long)]
    attestation_repository: String,
    /// Repository-relative signer workflow accepted by the external verifier
    #[arg(long)]
    attestation_workflow: String,
    /// Signer identity accepted by the external verifier
    #[arg(long)]
    attestation_signer: String,
    /// Unix time at the protected recovery boundary
    #[arg(long)]
    now: u64,
    /// Absent durable recovery-request-v1 output
    #[arg(long)]
    output: PathBuf,
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

/// Run one explicit native producer-input stage.
pub async fn run(args: ProducerArgs) -> miette::Result<()> {
    match args.command {
        ProducerCommand::Recipe(args) => recipe(args),
        ProducerCommand::Cache(args) => cache(args).await,
        ProducerCommand::CompatibilityPorts(args) => {
            native_compatibility::compatibility_ports(args).await
        }
        ProducerCommand::Environment(args) => environment(&args),
        ProducerCommand::Profile(args) => profile(&args),
        ProducerCommand::MaterializeEngineFreeSource(args) => {
            materialize_engine_free_source_stage(args)
        }
        ProducerCommand::Package(args) => package(args),
        ProducerCommand::VerifyPackage(args) => verify_package(args),
        ProducerCommand::Compare(args) => compare(&args),
        ProducerCommand::Repackage(args) => repackage(args),
        ProducerCommand::ValidateRecovery(args) => validate_recovery(&args),
        ProducerCommand::RecordQualification(args) => record_qualification(&args),
        ProducerCommand::PrepareRecovery(args) => prepare_recovery(&args),
        ProducerCommand::Index(args) => index(args),
        ProducerCommand::CompatibilityHostTools { host } => compatibility_host_tools(&host),
        ProducerCommand::Compatibility(args) => native_compatibility::compatibility(*args).await,
    }
}

fn compatibility_host_tools(host: &str) -> miette::Result<()> {
    for role in native_compatibility_host_tools(host).map_err(|error| native_error(&error))? {
        aros_common::outputln!("{role}");
    }
    Ok(())
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

fn validate_recovery(args: &ValidateRecoveryArgs) -> miette::Result<()> {
    let request_bytes = read_regular_input(&args.recovery_request, "recovery request")?;
    let request = RecoveryRequest::parse(&request_bytes).map_err(|error| native_error(&error))?;
    let validation = recovery::validate_recovery_inventory(&request, &args.release_dir)
        .map_err(|error| native_error(&error))?;
    let receipt = serde_json::json!({
        "schema": "aros-toolchain-recovery-validation-v1",
        "operation": "validate-recovery",
        "recovery_request_sha256": sha256_bytes(&request_bytes),
        "release_id": request.evidence.release.release_id,
        "asset_count": validation.asset_count,
        "decision": validation.decision,
    });
    let output = write_new_json(&args.output, &receipt, "recovery validation receipt")?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native recovery inventory validated: {} assets\nReceipt: {}",
            validation.asset_count,
            output.display(),
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "validate-recovery",
            "receipt": output,
            "recovery_request_sha256": sha256_bytes(&request_bytes),
            "asset_count": validation.asset_count,
        }))?,
    }
    Ok(())
}

fn record_qualification(args: &RecordQualificationArgs) -> miette::Result<()> {
    let inventory = recovery::measure_complete_release_inventory(&args.release_dir)
        .map_err(|error| native_error(&error))?;
    let index = NativeReleaseIndex::parse(&inventory.release_index_bytes)
        .map_err(|error| native_error(&error))?;
    let recipe_bytes = inventory_document(
        &args.release_dir,
        &inventory,
        "toolchain-recipe-v2.json",
        "recipe",
    )?;
    let recipe = Recipe::parse(&recipe_bytes).map_err(|error| native_error(&error))?;
    let source_lock_bytes = inventory_document(
        &args.release_dir,
        &inventory,
        &args.source_lock_filename,
        "source lock",
    )?;
    let _source_lock =
        SourceLock::parse(&source_lock_bytes).map_err(|error| native_error(&error))?;
    let profiles_bytes = inventory_document(
        &args.release_dir,
        &inventory,
        "profiles-v1.json",
        "profiles",
    )?;
    let _profiles = Profiles::parse(&profiles_bytes).map_err(|error| native_error(&error))?;

    let source_commit = parse_git_object(&index.source_commit, "release index source commit")?;
    let producer_commit =
        parse_git_object(&index.producer_commit, "release index producer commit")?;
    let tools_commit = parse_git_object(&index.tools_commit, "release index tools commit")?;
    if recipe.source().0 != &source_commit
        || recipe.producer().0 != &producer_commit
        || recipe.tools().0 != &tools_commit
        || sha256_bytes(&source_lock_bytes) != *recipe.source_lock_sha256()
        || sha256_bytes(&profiles_bytes) != *recipe.profiles_sha256()
    {
        return Err(miette::miette!(
            "native qualification evidence inputs do not match the final recipe and release index"
        ));
    }
    let checksums_sha256 = sha256_bytes(&inventory.checksums_bytes);
    let provenance_sha256 = inventory_asset_digest(
        &inventory,
        "toolchain-provenance.sigstore.json",
        "provenance bundle",
    )?;
    let source_tag_object = parse_git_object(&args.source_tag_object, "source tag object")?;
    let source_tag_commit = parse_git_object(&args.source_tag_commit, "source tag commit")?;
    if source_tag_commit != producer_commit {
        return Err(miette::miette!(
            "native qualification evidence source tag does not peel to the release-index producer commit"
        ));
    }
    let attestation = AttestationClaim {
        repository: args.attestation_repository.clone(),
        workflow: args.attestation_workflow.clone(),
        signer: args.attestation_signer.clone(),
        subject_sha256: checksums_sha256.clone(),
    };
    let evidence = QualificationEvidence {
        schema: QUALIFICATION_EVIDENCE_SCHEMA.into(),
        created_at: args.created_at,
        expires_at: args.expires_at,
        source_run: SourceRunIdentity {
            repository: args.source_repository.clone(),
            workflow: args.source_workflow.clone(),
            run_id: args.source_run_id,
            producer_commit: producer_commit.clone(),
            source_commit: source_commit.clone(),
            source_tag: args.source_tag.clone(),
            tag_object: source_tag_object,
        },
        release: ReleaseEvidence {
            release_id: index.release_id.clone(),
            base_url: index.base_url.clone(),
            release_index_sha256: sha256_bytes(&inventory.release_index_bytes),
            checksums_sha256,
            provenance_sha256,
            recipe_sha256: recipe.sha256().clone(),
            source_lock_sha256: recipe.source_lock_sha256().clone(),
            profiles_sha256: recipe.profiles_sha256().clone(),
            source_commit,
            producer_commit,
            tools_commit,
        },
        attestation,
        lanes: qualification_lanes(args, &index)?,
        coverage: EvidenceCoverage::ReleaseCandidate,
    };
    let policy = EvidencePolicy {
        source_repository: args.source_repository.clone(),
        source_workflow: args.source_workflow.clone(),
        signer_repository: args.attestation_repository.clone(),
        signer_workflow: args.attestation_workflow.clone(),
        signer: args.attestation_signer.clone(),
        now: args.created_at,
    };
    evidence
        .validate_against_index(&inventory.release_index_bytes, &policy)
        .map_err(|error| native_error(&error))?;
    let output = write_new_json(
        &args.output,
        &serde_json::to_value(&evidence)
            .map_err(|_| miette::miette!("cannot serialize native qualification evidence"))?,
        "qualification evidence",
    )?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native qualification evidence: {} lanes\nReceipt: {}\nRelease: {}",
            evidence.lanes.len(),
            output.display(),
            evidence.release.release_id,
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "record-qualification",
            "receipt": output,
            "release_id": evidence.release.release_id,
            "lanes": evidence.lanes.len(),
        }))?,
    }
    Ok(())
}

fn prepare_recovery(args: &PrepareRecoveryArgs) -> miette::Result<()> {
    let evidence = QualificationEvidence::parse(&read_regular_input(
        &args.qualification_evidence,
        "qualification evidence",
    )?)
    .map_err(|error| native_error(&error))?;
    let inventory = recovery::measure_complete_release_inventory(&args.release_dir)
        .map_err(|error| native_error(&error))?;
    let checksums_sha256 = sha256_bytes(&inventory.checksums_bytes);
    let source_tag = ObservedTag {
        name: evidence.source_run.source_tag.clone(),
        tag_object: parse_git_object(&args.source_tag_object, "source tag object")?,
        peeled_commit: parse_git_object(&args.source_tag_commit, "source tag commit")?,
    };
    let handoff = RecoveryHandoff {
        release_id: args.recovery_release_id.clone(),
        tag: ObservedTag {
            name: args.recovery_release_id.clone(),
            tag_object: parse_git_object(&args.recovery_tag_object, "recovery tag object")?,
            peeled_commit: parse_git_object(&args.recovery_tag_commit, "recovery tag commit")?,
        },
        state: ReleaseHandoffState::Absent,
    };
    let verified_attestation = AttestationClaim {
        repository: args.attestation_repository.clone(),
        workflow: args.attestation_workflow.clone(),
        signer: args.attestation_signer.clone(),
        subject_sha256: checksums_sha256,
    };
    let request = RecoveryRequest {
        operation: RecoveryOperation::PackagingRecovery,
        failed_stage: FailedStage::Packaging,
        evidence,
        release_index_bytes: inventory.release_index_bytes,
        checksums_bytes: inventory.checksums_bytes,
        assets: inventory.assets,
        verified_attestation: Some(verified_attestation),
        evidence_policy: EvidencePolicy {
            source_repository: args.source_repository.clone(),
            source_workflow: args.source_workflow.clone(),
            signer_repository: args.attestation_repository.clone(),
            signer_workflow: args.attestation_workflow.clone(),
            signer: args.attestation_signer.clone(),
            now: args.now,
        },
        source_tag,
        handoff: Some(handoff),
    };
    let decision = recovery::evaluate_recovery(&request).map_err(|error| native_error(&error))?;
    let output = write_new_json(
        &args.output,
        &serde_json::to_value(&request)
            .map_err(|_| miette::miette!("cannot serialize native recovery request"))?,
        "recovery request",
    )?;
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native recovery request prepared\nReceipt: {}\nDecision: {:?}",
            output.display(),
            decision,
        ),
        ResultFormat::Json => print_json(&serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "prepare-recovery",
            "recovery_request": output,
            "decision": decision,
        }))?,
    }
    Ok(())
}

fn inventory_document(
    release_dir: &std::path::Path,
    inventory: &recovery::MeasuredReleaseInventory,
    name: &str,
    label: &str,
) -> miette::Result<Vec<u8>> {
    let expected = inventory
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| miette::miette!("native qualification evidence is missing {label}"))?;
    let bytes = read_bounded_regular_input(&release_dir.join(name), label)?;
    if sha256_bytes(&bytes) != expected.sha256 || bytes.len() as u64 != expected.size {
        return Err(miette::miette!(
            "native qualification evidence {label} changed after final inventory measurement"
        ));
    }
    Ok(bytes)
}

fn inventory_asset_digest(
    inventory: &recovery::MeasuredReleaseInventory,
    name: &str,
    label: &str,
) -> miette::Result<Sha256Digest> {
    inventory
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.sha256.clone())
        .ok_or_else(|| miette::miette!("native qualification evidence is missing {label}"))
}

fn parse_git_object(
    value: &str,
    label: &str,
) -> miette::Result<aros_toolchain::recipe::GitObjectId> {
    aros_toolchain::recipe::GitObjectId::try_from(value.to_owned()).map_err(|_| {
        miette::miette!(
            "native qualification evidence {label} must be lowercase 40-hex Git identity"
        )
    })
}

fn qualification_lanes(
    args: &RecordQualificationArgs,
    index: &NativeReleaseIndex,
) -> miette::Result<Vec<QualificationLane>> {
    // `NativeReleaseIndex::parse` already accepts only the complete active or
    // historical v1 matrix.  The evidence must follow that selected immutable
    // inventory, rather than re-expanding it to every host the parser can read.
    let mut lanes = Vec::with_capacity(index.artifacts.len());
    for artifact in &index.artifacts {
        let host = &artifact.host;
        let profile = &artifact.target_profile;
        let lifecycle_a = args
            .lifecycle_reports_dir
            .join(format!("native-lifecycle-{host}-{profile}-a"))
            .join("publish.json");
        let lifecycle_b = args
            .lifecycle_reports_dir
            .join(format!("native-lifecycle-{host}-{profile}-b"))
            .join("publish.json");
        let comparison = args
            .comparison_reports_dir
            .join(format!("comparison-{host}-{profile}"))
            .join(format!("comparison-{host}-{profile}.json"));
        let compatibility = args
            .compatibility_reports_dir
            .join(format!("compatibility-{host}-{profile}"))
            .join("native-compatibility.receipt.json");
        lanes.push(QualificationLane {
            host: host.clone(),
            target_profile: profile.clone(),
            target_triple: artifact.target_triple.clone(),
            build_a_report_sha256: lifecycle_report_digest(&lifecycle_a)?,
            build_b_report_sha256: lifecycle_report_digest(&lifecycle_b)?,
            comparison_report_sha256: comparison_report_digest(&comparison)?,
            compatibility_report_sha256: compatibility_report_digest(&compatibility)?,
        });
    }
    Ok(lanes)
}

fn lifecycle_report_digest(path: &std::path::Path) -> miette::Result<Sha256Digest> {
    let (value, digest) = evidence_report(path, "native lifecycle publish receipt")?;
    if value.get("schema").and_then(serde_json::Value::as_str) != Some("aros-toolchain-receipt-v1")
        || value.get("phase").and_then(serde_json::Value::as_str) != Some("publish")
    {
        return Err(miette::miette!(
            "native qualification evidence lifecycle report is not a native publish receipt"
        ));
    }
    let recorded = value
        .get("receipt_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| miette::miette!("native lifecycle publish receipt has no self-digest"))?;
    let recorded = Sha256Digest::parse(recorded).map_err(|_| {
        miette::miette!("native lifecycle publish receipt has an invalid self-digest")
    })?;
    let mut material = value;
    material
        .as_object_mut()
        .ok_or_else(|| miette::miette!("native lifecycle publish receipt is not an object"))?
        .remove("receipt_sha256");
    let calculated =
        sha256_bytes(&aros_toolchain::canonical::bytes(&material).map_err(|_| {
            miette::miette!("cannot canonicalize native lifecycle publish receipt")
        })?);
    if calculated != recorded {
        return Err(miette::miette!(
            "native lifecycle publish receipt self-digest verification failed"
        ));
    }
    Ok(digest)
}

fn comparison_report_digest(path: &std::path::Path) -> miette::Result<Sha256Digest> {
    let (value, digest) = evidence_report(path, "native comparison receipt")?;
    if value.get("schema").and_then(serde_json::Value::as_u64) != Some(1)
        || value.get("operation").and_then(serde_json::Value::as_str) != Some("compare")
        || value
            .get("byte_identical")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || value
            .get("members")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|members| members.len() != 4)
    {
        return Err(miette::miette!(
            "native qualification evidence comparison report is incomplete or noncanonical"
        ));
    }
    Ok(digest)
}

fn compatibility_report_digest(path: &std::path::Path) -> miette::Result<Sha256Digest> {
    let (value, digest) = evidence_report(path, "native compatibility receipt")?;
    let ports_sources = value
        .get("ports_sources")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            miette::miette!(
                "native qualification evidence compatibility report omits its upstream source-input closure"
            )
        })?;
    if value.get("schema").and_then(serde_json::Value::as_str)
        != Some("aros-toolchain-native-compatibility-receipt-v2")
        || value.get("operation").and_then(serde_json::Value::as_str)
            != Some("native-compatibility")
        || value
            .get("phase_reports")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|reports| reports.len() != 6)
        || !compatibility_ports_source_closure(ports_sources)
    {
        return Err(miette::miette!(
            "native qualification evidence compatibility report is incomplete or noncanonical"
        ));
    }
    Ok(digest)
}

fn compatibility_ports_source_closure(sources: &[serde_json::Value]) -> bool {
    if sources.is_empty() || sources.len() > 128 {
        return false;
    }
    let mut ids = BTreeSet::new();
    let mut cache_filenames = BTreeSet::new();
    let mut relative_paths = BTreeSet::new();
    let mut fetch_markers = BTreeSet::new();
    sources.iter().all(|source| {
        let Some(record) = source.as_object() else {
            return false;
        };
        if record.len() != 6
            || !record.contains_key("id")
            || !record.contains_key("cache_filename")
            || !record.contains_key("relative_path")
            || !record.contains_key("fetch_marker")
            || !record.contains_key("sha256")
            || !record.contains_key("size")
        {
            return false;
        }
        let Some(id) = record.get("id").and_then(serde_json::Value::as_str) else {
            return false;
        };
        let Some(cache_filename) = record
            .get("cache_filename")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        let Some(relative_path) = record
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        let Some(fetch_marker) = record
            .get("fetch_marker")
            .and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        let valid_id = !id.is_empty()
            && id.len() <= 96
            && id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        let valid_filename = portable_compatibility_filename(cache_filename);
        let valid_path = !relative_path.is_empty()
            && relative_path.len() <= 512
            && !relative_path.starts_with('/')
            && !relative_path.contains('\\')
            && relative_path
                .split('/')
                .all(portable_compatibility_filename);
        let valid_marker = fetch_marker.is_empty()
            || aros_toolchain::compatibility_ports::safe_fetch_marker_path(fetch_marker);
        valid_id
            && valid_filename
            && valid_path
            && valid_marker
            && ids.insert(id)
            && cache_filenames.insert(cache_filename)
            && relative_paths.insert(relative_path)
            && (fetch_marker.is_empty() || fetch_markers.insert(fetch_marker))
            && record
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|digest| Sha256Digest::parse(digest).is_ok())
            && record
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|size| size > 0)
    })
}

fn portable_compatibility_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+._-".contains(&byte))
}

fn evidence_report(
    path: &std::path::Path,
    label: &str,
) -> miette::Result<(serde_json::Value, Sha256Digest)> {
    let bytes = read_bounded_regular_input(path, label)?;
    let digest = sha256_bytes(&bytes);
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| miette::miette!("native qualification evidence {label} is not valid JSON"))?;
    Ok((value, digest))
}

fn read_bounded_regular_input(path: &std::path::Path, label: &str) -> miette::Result<Vec<u8>> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| miette::miette!("native qualification evidence {label} is unavailable"))?;
    let before = file
        .metadata()
        .map_err(|_| miette::miette!("native qualification evidence {label} cannot be inspected"))?
        .len();
    if before == 0 || before > aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64 {
        return Err(miette::miette!(
            "native qualification evidence {label} is empty or exceeds the document limit"
        ));
    }
    let mut bytes = Vec::with_capacity(before as usize);
    std::io::Read::by_ref(&mut file)
        .take(aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| miette::miette!("native qualification evidence {label} cannot be read"))?;
    let after = file
        .metadata()
        .map_err(|_| {
            miette::miette!("native qualification evidence {label} cannot be re-inspected")
        })?
        .len();
    if bytes.len() > aros_toolchain::canonical::MAX_DOCUMENT_BYTES
        || after != before
        || bytes.len() as u64 != before
    {
        return Err(miette::miette!(
            "native qualification evidence {label} changed while it was read"
        ));
    }
    Ok(bytes)
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
#[path = "toolchain_producer_active_matrix_tests.rs"]
mod toolchain_producer_active_matrix_tests;

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;

    use super::{
        compare, compatibility_ports_source_closure, environment, CompareArgs, EnvironmentArgs,
        ResultFormat,
    };
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
        let validation = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "validate-recovery",
            "--recovery-request",
            "/evidence/recovery.json",
            "--release-dir",
            "/release/source",
            "--output",
            "/evidence/recovery-validation.json",
        ]);
        assert!(validation.is_ok());
        let qualification = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "record-qualification",
            "--release-dir",
            "/release/source",
            "--source-lock-filename",
            "llvm-11.0.0.sources.json",
            "--lifecycle-reports-dir",
            "/evidence/lifecycle",
            "--comparison-reports-dir",
            "/evidence/comparison",
            "--compatibility-reports-dir",
            "/evidence/compatibility",
            "--source-repository",
            "https://github.com/metaneutrons/aros-toolchains",
            "--source-workflow",
            ".github/workflows/toolchain-release.yml",
            "--source-run-id",
            "42",
            "--source-tag",
            "toolchain-v1-source",
            "--source-tag-object",
            "0123456789012345678901234567890123456789",
            "--source-tag-commit",
            "0123456789012345678901234567890123456789",
            "--attestation-repository",
            "https://github.com/metaneutrons/aros-toolchains",
            "--attestation-workflow",
            ".github/workflows/toolchain-release.yml",
            "--attestation-signer",
            "github-actions",
            "--created-at",
            "100",
            "--expires-at",
            "200",
            "--output",
            "/evidence/qualification.json",
        ]);
        assert!(qualification.is_ok());
        let recovery_request = Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "prepare-recovery",
            "--qualification-evidence",
            "/evidence/qualification.json",
            "--release-dir",
            "/release/source",
            "--source-tag-object",
            "0123456789012345678901234567890123456789",
            "--source-tag-commit",
            "0123456789012345678901234567890123456789",
            "--recovery-release-id",
            "toolchain-v1-recovered",
            "--recovery-tag-object",
            "1234567890123456789012345678901234567890",
            "--recovery-tag-commit",
            "0123456789012345678901234567890123456789",
            "--source-repository",
            "https://github.com/metaneutrons/aros-toolchains",
            "--source-workflow",
            ".github/workflows/toolchain-release.yml",
            "--attestation-repository",
            "https://github.com/metaneutrons/aros-toolchains",
            "--attestation-workflow",
            ".github/workflows/toolchain-release.yml",
            "--attestation-signer",
            "github-actions",
            "--now",
            "150",
            "--output",
            "/evidence/recovery.json",
        ]);
        assert!(recovery_request.is_ok());
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
        assert!(Cli::try_parse_from([
            "aros",
            "toolchain",
            "producer",
            "compatibility-host-tools",
            "--host",
            "linux-x86_64",
        ])
        .is_ok());
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

    #[test]
    fn compatibility_receipt_closure_accepts_profiled_sources_and_rejects_tampering() {
        let sources = vec![
            serde_json::json!({
                "id": "unicode-data",
                "cache_filename": "UnicodeData.txt",
                "relative_path": "UnicodeData.txt",
                "fetch_marker": "",
                "sha256": "a".repeat(64),
                "size": 1,
            }),
            serde_json::json!({
                "id": "mesa",
                "cache_filename": "mesa-20.0.8.tar.xz",
                "relative_path": "ports/mesa-20.0.8.tar.xz",
                "fetch_marker": "ports/.mesa-20.0.8-fetched",
                "sha256": "b".repeat(64),
                "size": 2,
            }),
        ];
        assert!(compatibility_ports_source_closure(&sources));

        let mut duplicate_path = sources.clone();
        duplicate_path[1]["relative_path"] = serde_json::json!("UnicodeData.txt");
        assert!(!compatibility_ports_source_closure(&duplicate_path));

        let mut unsafe_path = sources;
        unsafe_path[1]["relative_path"] = serde_json::json!("../mesa-20.0.8.tar.xz");
        assert!(!compatibility_ports_source_closure(&unsafe_path));

        let mut unsafe_marker = unsafe_path;
        unsafe_marker[1]["relative_path"] = serde_json::json!("ports/mesa-20.0.8.tar.xz");
        unsafe_marker[1]["fetch_marker"] = serde_json::json!("ports/../.mesa-20.0.8-fetched");
        assert!(!compatibility_ports_source_closure(&unsafe_marker));
    }
}
