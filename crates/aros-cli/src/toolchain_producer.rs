//! Native producer-input commands.
//!
//! These commands deliberately cover only closed local producer stages. They
//! bind recipe bytes to committed checkouts, acquire/verify the lock-owned
//! source cache, and operate on already-built candidates; none can tag or
//! publish a toolchain release.

use aros_common::{open_regular_file_nofollow, sha256_bytes, Sha256Digest};
use aros_toolchain::compatibility::native_compatibility_host_tools;
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use aros_toolchain::profiles::Profiles;
use aros_toolchain::qualification_evidence::{
    AttestationClaim, EvidenceCoverage, EvidencePolicy, QualificationEvidence, QualificationLane,
    ReleaseEvidence, SourceRunIdentity, QUALIFICATION_EVIDENCE_SCHEMA,
};
use aros_toolchain::recipe::GitObjectId;
use aros_toolchain::recipe_builder::{self, RecipeBuildRequest};
use aros_toolchain::recovery::{
    self, FailedStage, ObservedTag, RecoveryHandoff, RecoveryOperation, RecoveryRequest,
    ReleaseHandoffState,
};
use aros_toolchain::release_index::{self, IndexRequest, IndexStage, NativeReleaseIndex};
use aros_toolchain::repackage::{self, VerifiedPackageRepackageRequest};
use aros_toolchain::source_lock::SourceLock;
use aros_toolchain::{package, package_verify, Recipe};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use crate::observability;

mod compatibility_export;
mod finished_package;
mod native_compatibility;
mod release_evidence;
#[cfg(unix)]
mod release_evidence_readback;
mod release_evidence_selection;
#[cfg(unix)]
mod release_index_family;

/// Closed native producer stages exposed by `aros toolchain producer`.
#[derive(Args)]
pub struct ProducerArgs {
    #[command(subcommand)]
    command: ProducerCommand,
}

impl ProducerArgs {
    /// Return the exact public producer leaf for diagnostic context.
    ///
    /// The frontend owns the final diagnostic envelope, while this module owns
    /// the nested producer model. Keeping this mapping beside that model avoids
    /// a second parser-shaped list in the frontend.
    pub const fn diagnostic_mode(&self) -> &'static str {
        match &self.command {
            ProducerCommand::Recipe(_) => "toolchain.producer.recipe",
            ProducerCommand::Environment(_) => "toolchain.producer.environment",
            ProducerCommand::Profile(_) => "toolchain.producer.profile",
            ProducerCommand::MaterializeEngineFreeSource(_) => {
                "toolchain.producer.materialize-engine-free-source"
            }
            ProducerCommand::Package(_) => "toolchain.producer.package",
            ProducerCommand::VerifyPackage(_) => "toolchain.producer.verify-package",
            ProducerCommand::Compare(_) => "toolchain.producer.compare",
            ProducerCommand::Repackage(_) => "toolchain.producer.repackage",
            ProducerCommand::ValidateRecovery(_) => "toolchain.producer.validate-recovery",
            ProducerCommand::RecordQualification(_) => "toolchain.producer.record-qualification",
            ProducerCommand::PrepareRecovery(_) => "toolchain.producer.prepare-recovery",
            ProducerCommand::Index(_) => "toolchain.producer.index",
            ProducerCommand::CompatibilityHostTools { .. } => {
                "toolchain.producer.compatibility-host-tools"
            }
            ProducerCommand::Compatibility(_) => "toolchain.producer.compatibility",
            ProducerCommand::VerifyReleaseEvidence(_) => {
                "toolchain.producer.verify-release-evidence"
            }
        }
    }
}

/// One producer operation with no implicit backend selection.
#[derive(Subcommand)]
enum ProducerCommand {
    /// Construct one non-overwriting recipe-v2 from committed Git inputs
    Recipe(RecipeArgs),
    /// Write the deterministic build-environment receipt embedded in a package
    Environment(EnvironmentArgs),
    /// Read one recipe-bound producer profile without duplicating its selectors
    Profile(ProfileArgs),
    /// Materialize an audited source snapshot without its source-tree CMake engine
    MaterializeEngineFreeSource(MaterializeEngineFreeSourceArgs),
    /// Create one deterministic local package set from a completed candidate
    Package(PackageArgs),
    /// Read back and verify one complete deterministic local package set
    VerifyPackage(VerifyPackageArgs),
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
    /// Read back every selected family-v2 build and compatibility lane; no execution authentication
    VerifyReleaseEvidence(Box<release_evidence::EvidenceArgs>),
}

/// Machine- or human-readable local stage result.
#[derive(Clone, Copy, ValueEnum)]
enum ResultFormat {
    /// Stable concise text for a human or build log.
    Human,
    /// Structured JSON for a workflow handoff.
    Json,
}

/// Explicit archive and manifest format for local package operations.
#[derive(Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum PackageFormatArg {
    /// Historical LLVM schema-v1 metadata and v1 asset naming.
    LegacyV1,
    /// Compiler-family schema-v2 metadata and v2 asset naming.
    FamilyV2,
}

impl From<PackageFormatArg> for package::PackageFormat {
    fn from(value: PackageFormatArg) -> Self {
        match value {
            PackageFormatArg::LegacyV1 => Self::LegacyLlvmV1,
            PackageFormatArg::FamilyV2 => Self::CompilerFamilyV2,
        }
    }
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
    /// Producer-root-relative source lock (LLVM v2 or GNU v3)
    #[arg(long)]
    source_lock: PathBuf,
    /// Producer-root-relative profiles matrix (LLVM v1 or GNU v2)
    #[arg(long)]
    profiles: PathBuf,
    /// Absent recipe-v2 output file; an existing file is never replaced
    #[arg(long)]
    output: PathBuf,
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
    /// Family-selected profiles matrix bound by the selected recipe
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
#[command(group(ArgGroup::new("source_identity").args(["recipe", "source_commit"]).required(true)))]
#[command(
    after_help = "Source identity: select --recipe OR both --source-commit and --source-tree. Neither half of the explicit pair is valid alone. All identities bind a clean committed checkout; this command does not change a compiler package or its recipe."
)]
struct MaterializeEngineFreeSourceArgs {
    /// Clean exact committed AROS checkout to materialize
    #[arg(long)]
    source_dir: PathBuf,
    /// Bind to the recipe's build-source identity instead of a separate consumer
    #[arg(long, conflicts_with_all = ["source_commit", "source_tree"])]
    recipe: Option<PathBuf>,
    /// Exact consumer source commit; requires --source-tree and excludes --recipe
    #[arg(long, value_parser = parse_source_git_object, requires = "source_tree")]
    source_commit: Option<GitObjectId>,
    /// Exact consumer source tree; requires --source-commit
    #[arg(long, value_parser = parse_source_git_object, requires = "source_commit")]
    source_tree: Option<GitObjectId>,
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
    /// Family-selected source lock bound by the selected recipe
    #[arg(long)]
    source_lock: PathBuf,
    /// Family-selected profiles matrix bound by the selected recipe
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

/// Inputs for native package creation.
#[derive(Args)]
struct PackageArgs {
    #[command(flatten)]
    context: PackageContextArgs,
    /// Completed local candidate prefix to package
    #[arg(long)]
    input_dir: PathBuf,
    /// Absent final package-set directory
    #[arg(long)]
    output_dir: PathBuf,
    /// Package metadata format; omitted selects the existing compiler-family default
    #[arg(long, value_enum)]
    package_format: Option<PackageFormatArg>,
    /// Exact retained build JSON; requires its externally selected SHA-256 and original work root
    #[arg(long, requires_all = ["build_result_sha256", "build_work_dir", "package_format"])]
    build_result: Option<PathBuf>,
    /// SHA-256 of the exact build-result file, not the finished receipt's self-digest
    #[arg(long, value_parser = parse_build_result_digest, requires = "build_result")]
    build_result_sha256: Option<Sha256Digest>,
    /// Original native build work root; guarded packaging requires explicit family-v2 format
    #[arg(long, requires = "build_result")]
    build_work_dir: Option<PathBuf>,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Inputs for native package read-back verification.
#[derive(Args)]
struct VerifyPackageArgs {
    #[command(flatten)]
    context: PackageContextArgs,
    /// Complete package directory to verify without mutation
    #[arg(long)]
    input_dir: PathBuf,
    /// Package metadata format; omitted selects the existing compiler-family default
    #[arg(long, value_enum)]
    package_format: Option<PackageFormatArg>,
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
    /// Complete isolated final release inventory selected by its release index
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
    /// Complete isolated final release inventory selected by its release index
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
    /// Write the index and the format-specific pre-attestation subject list.
    PreAttestation,
    /// Verify unchanged subjects and write final checksums including provenance.
    Final,
}

/// Explicit local release inventory format; independent of release SemVer.
#[derive(Clone, Copy, ValueEnum)]
enum ReleaseFormatArg {
    /// Historical single-group LLVM release index and checksum stages.
    LegacyV1,
    /// Compiler-family release inputs, measured index and external subject list.
    FamilyV2,
}

/// Inputs for native release-index advancement without publication authority.
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
    /// Local release format (default: legacy-v1; not the public release version)
    #[arg(long, value_enum)]
    release_format: Option<ReleaseFormatArg>,
    /// V1 only: basename of the one source-lock document in the inventory
    #[arg(long, required_unless_present = "release_format", required_if_eq("release_format", "legacy-v1"), conflicts_with_all = ["lane_inputs", "subject_manifest", "subject_manifest_sha256", "forbidden_prefixes"])]
    source_lock_filename: Option<String>,
    /// V2 only: closed independent archive environment/required-path map
    #[arg(long, required_if_eq("release_format", "family-v2"))]
    lane_inputs: Option<PathBuf>,
    /// V2 only: absent pre-stage output or existing final-stage subject list, outside the release
    #[arg(long, required_if_eq("release_format", "family-v2"))]
    subject_manifest: Option<PathBuf>,
    /// V2 final only: exact subject-list SHA-256 returned by pre-attestation
    #[arg(long, required_if_eq_all = [("release_format", "family-v2"), ("stage", "final")])]
    subject_manifest_sha256: Option<String>,
    /// V2 only: absolute build root forbidden in archive payloads (repeatable)
    #[arg(long = "forbidden-prefix")]
    forbidden_prefixes: Vec<PathBuf>,
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
        ProducerCommand::Environment(args) => environment(&args),
        ProducerCommand::Profile(args) => profile(&args),
        ProducerCommand::MaterializeEngineFreeSource(args) => {
            materialize_engine_free_source_stage(args)
        }
        ProducerCommand::Package(args) => package(&args),
        ProducerCommand::VerifyPackage(args) => verify_package(args),
        ProducerCommand::Compare(args) => compare(&args),
        ProducerCommand::Repackage(args) => repackage(args),
        ProducerCommand::ValidateRecovery(args) => validate_recovery(&args),
        ProducerCommand::RecordQualification(args) => record_qualification(&args),
        ProducerCommand::PrepareRecovery(args) => prepare_recovery(&args),
        ProducerCommand::Index(args) => index(args),
        ProducerCommand::CompatibilityHostTools { host } => compatibility_host_tools(&host),
        ProducerCommand::Compatibility(args) => native_compatibility::compatibility(*args).await,
        ProducerCommand::VerifyReleaseEvidence(args) => release_evidence::run(&args),
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
    let expected_source = match (args.recipe, args.source_commit, args.source_tree) {
        (Some(path), None, None) => {
            let recipe = Recipe::parse(&read_regular_input(&path, "recipe")?)
                .map_err(|error| native_error(&error))?;
            CommittedSourceIdentity {
                commit: recipe.source().0.clone(),
                tree: recipe.source().1.clone(),
            }
        }
        (None, Some(commit), Some(tree)) => CommittedSourceIdentity { commit, tree },
        _ => {
            return Err(miette::miette!(
                "select --recipe or both --source-commit and --source-tree"
            ));
        }
    };
    let output = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: args.source_dir,
        expected_source,
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

fn parse_source_git_object(value: &str) -> Result<GitObjectId, &'static str> {
    GitObjectId::try_from(value.to_owned())
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

fn package(args: &PackageArgs) -> miette::Result<()> {
    if args.build_result.is_some()
        && !matches!(args.package_format, Some(PackageFormatArg::FamilyV2))
    {
        return Err(miette::miette!(
            "finished-build packaging requires --package-format family-v2"
        ));
    }
    let context = package_context(args.context.clone())?;
    let request = package::PackageRequest {
        candidate_root: args.input_dir.clone(),
        output_dir: args.output_dir.clone(),
        release_id: context.release_id,
        host: context.host,
        recipe: context.recipe,
        source_lock: context.source_lock,
        profile: context.profile,
        build_environment: context.build_environment,
        forbidden_prefixes: context.forbidden_prefixes,
    };
    let (output, finished_candidate) = if args.build_result.is_some() {
        finished_package::run(&request, args)?
    } else {
        let result = args.package_format.map_or_else(
            || package::package(&request),
            |format| package::package_with_format(&request, format.into()),
        );
        (result.map_err(|error| native_error(&error))?, None)
    };
    match args.format {
        ResultFormat::Human => aros_common::outputln!(
            "Native package: {}\nSHA-256: {}\nSize: {}",
            output.archive.display(),
            output.archive_sha256,
            output.archive_size
        ),
        ResultFormat::Json => {
            let mut document = serde_json::json!({
            "schema": "aros-toolchain-producer-stage-v1",
            "operation": "package",
            "package_dir": output.output_dir,
            "archive": output.archive,
            "manifest": output.manifest,
            "checksum": output.checksum,
            "sbom": output.sbom,
            "sha256": output.archive_sha256,
            "size": output.archive_size,
            });
            if let Some(proof) = finished_candidate {
                document["finished_candidate"] = proof;
            }
            print_json(&document)?;
        }
    }
    Ok(())
}

fn parse_build_result_digest(value: &str) -> Result<Sha256Digest, &'static str> {
    let digest = Sha256Digest::parse(value).map_err(|_| "expected lowercase 64-hex SHA-256")?;
    if digest.as_str() != value {
        return Err("expected lowercase 64-hex SHA-256");
    }
    Ok(digest)
}

fn verify_package(args: VerifyPackageArgs) -> miette::Result<()> {
    let context = package_context(args.context)?;
    let request = package_verify::PackageVerificationRequest {
        package_dir: args.input_dir,
        release_id: context.release_id,
        host: context.host,
        recipe: context.recipe,
        source_lock: context.source_lock,
        profile: context.profile,
        build_environment: context.build_environment,
        forbidden_prefixes: context.forbidden_prefixes,
    };
    let result = args.package_format.map_or_else(
        || package_verify::verify(&request),
        |format| package_verify::verify_with_format(&request, format.into()),
    );
    let output = result.map_err(|error| native_error(&error))?;
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
    match args.release_format.unwrap_or(ReleaseFormatArg::LegacyV1) {
        ReleaseFormatArg::LegacyV1 => index_legacy(args),
        ReleaseFormatArg::FamilyV2 => {
            #[cfg(unix)]
            {
                release_index_family::run(args)
            }
            #[cfg(not(unix))]
            {
                let _ = args;
                Err(miette::miette!(
                    "family-v2 index stages require a native Unix host"
                ))
            }
        }
    }
}

fn index_legacy(args: IndexArgs) -> miette::Result<()> {
    let stage = match args.stage {
        IndexStageArg::PreAttestation => IndexStage::PreAttestation,
        IndexStageArg::Final => IndexStage::Final,
    };
    let output = release_index::index_complete_v1(&IndexRequest {
        directory: args.directory,
        release_id: args.release_id,
        base_url: args.base_url,
        source_lock_filename: args
            .source_lock_filename
            .ok_or_else(|| miette::miette!("legacy-v1 index requires --source-lock-filename"))?,
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
    profiles: Profiles,
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
        profiles,
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
#[path = "toolchain_producer_tests.rs"]
mod tests;
