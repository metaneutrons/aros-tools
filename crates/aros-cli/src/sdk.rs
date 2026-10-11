//! SDK artifact stages and explicit lock-selected managed operations.

use aros_common::sdk_manifest::{SdkIdentity, SDK_MAX_METADATA_BYTES};
use aros_common::{measure_regular_file_digest_bounded, open_regular_file_nofollow, Sha256Digest};
use clap::{Args, Subcommand};
use miette::{IntoDiagnostic, Result};
use std::io::Read;
use std::path::PathBuf;

use crate::toolchain_producer::native_compatibility;

#[cfg(unix)]
#[path = "sdk_spdx_output.rs"]
mod spdx_output;

const SDK_SELECTION_HELP: &str = "Select either `--release-lock FILE --lock-sha256 SHA256` or `--project-lock FILE`. A project lock is explicit local application configuration, not publisher authentication.";

#[derive(Subcommand)]
/// Local artifact stages and management for a separately identified SDK.
pub enum SdkCommand {
    /// Bind SDK identity to an audited committed source projection without building
    VerifySource(SourceArgs),
    /// Verify a source-owned SDK, generate it with native compatibility and package it
    Build(Box<BuildArgs>),
    /// Package one source-produced SDK without modifying it or publishing a release
    Package(PackageArgs),
    /// Verify a complete local SDK package against explicit identity and digests
    VerifyPackage(VerifyArgs),
    /// Write deterministic SPDX metadata for a verified SDK into a separate stage
    Sbom(SbomArgs),
    /// Verify and extract one SDK into an absent directory; not managed installation
    Extract(ExtractArgs),
    /// Acquire and completely verify locked SDK bytes without installing them
    #[command(after_help = SDK_SELECTION_HELP)]
    Download(crate::sdk_managed::DownloadArgs),
    /// Install or fully verify reuse of one exact lock-selected SDK
    #[command(after_help = SDK_SELECTION_HELP)]
    Install(crate::sdk_managed::InstallArgs),
    /// List bounded store metadata without hashing or executing SDK payloads
    List(crate::sdk_managed::ListArgs),
    /// Preview or apply one application project's verified SDK selection
    Select(crate::sdk_managed::SelectArgs),
    /// Fully verify the SDK selected by explicit release or project metadata
    #[command(after_help = SDK_SELECTION_HELP)]
    Verify(crate::sdk_managed::InstalledArgs),
    /// Print the fully verified installed SDK path for application build tools
    #[command(after_help = SDK_SELECTION_HELP)]
    Path(crate::sdk_managed::InstalledArgs),
}

impl SdkCommand {
    /// Exact operation label for the frontend diagnostic envelope.
    pub const fn diagnostic_mode(&self) -> &'static str {
        match self {
            Self::VerifySource(_) => "sdk.verify-source",
            Self::Build(_) => "sdk.build",
            Self::Package(_) => "sdk.package",
            Self::VerifyPackage(_) => "sdk.verify-package",
            Self::Sbom(_) => "sdk.sbom",
            Self::Extract(_) => "sdk.extract",
            Self::Download(_) => "sdk.download",
            Self::Install(_) => "sdk.install",
            Self::List(_) => "sdk.list",
            Self::Select(_) => "sdk.select",
            Self::Verify(_) => "sdk.verify",
            Self::Path(_) => "sdk.path",
        }
    }
}

#[derive(Args)]
/// Read-only source/ABI verification, not SDK generation or release admission.
pub struct SourceArgs {
    /// Independently selected closed SDK product identity JSON
    #[arg(long, value_name = "FILE")]
    identity: PathBuf,
    /// Independently selected SHA-256 of the exact identity document bytes
    #[arg(long, value_name = "SHA256")]
    identity_sha256: Sha256Digest,
    /// Clean original Git checkout, including initialized committed submodules
    #[arg(long, value_name = "DIR")]
    source_dir: PathBuf,
    /// Existing engine-free source projection to compare against raw Git contents
    #[arg(long, value_name = "DIR")]
    materialized_source_dir: PathBuf,
    /// Exact source-owned profile with a sealed v2 native SDK consumer contract
    #[arg(long, value_name = "NAME")]
    source_preset: String,
}

#[derive(Args)]
/// Complete local SDK generation and packaging from a verified source binding.
pub struct BuildArgs {
    #[command(flatten)]
    source: SourceArgs,
    #[command(flatten)]
    native: native_compatibility::SdkBuildArgs,
}

#[derive(Args)]
/// Independently selected source/ABI metadata and payload identity.
pub struct SelectionArgs {
    /// Closed source-bound SDK identity JSON; no implicit host or board selection
    #[arg(long, value_name = "FILE")]
    identity: PathBuf,
    /// Independently selected SHA-256 of the exact identity document bytes
    #[arg(long, value_name = "SHA256")]
    identity_sha256: Sha256Digest,
    /// Independently selected digest of the complete normalized SDK inventory
    #[arg(long, value_name = "SHA256")]
    tree_sha256: Sha256Digest,
    /// Absolute build root that must not occur in payload bytes; repeatable
    #[arg(long = "forbidden-prefix", value_name = "DIR", required = true)]
    forbidden_prefixes: Vec<PathBuf>,
}

#[derive(Args)]
/// Explicit inputs for local deterministic SDK packaging.
pub struct PackageArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// Existing, complete source-produced Developer SDK root; read only
    #[arg(long, value_name = "DIR")]
    candidate_root: PathBuf,
    /// Absent directory for the complete local archive, manifest and checksum
    #[arg(long, value_name = "DIR")]
    output_dir: PathBuf,
}

#[derive(Args)]
/// Explicit inputs for SDK package read-back verification.
pub struct VerifyArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// Directory containing exactly the three local SDK package members
    #[arg(long, value_name = "DIR")]
    package_dir: PathBuf,
    /// Independently selected archive SHA-256; not inferred from its sidecar
    #[arg(long, value_name = "SHA256")]
    archive_sha256: Sha256Digest,
    /// Independently selected archive size in bytes
    #[arg(long, value_name = "BYTES", value_parser = parse_positive_bytes)]
    archive_size: u64,
}

#[derive(Args)]
/// Verification selectors and one fresh extraction destination.
pub struct ExtractArgs {
    #[command(flatten)]
    verification: VerifyArgs,
    /// Absent output directory under an existing real parent
    #[arg(long, value_name = "DIR")]
    output_root: PathBuf,
}

#[derive(Args)]
/// Verified package selection and one supplemental distribution metadata file.
pub struct SbomArgs {
    #[command(flatten)]
    verification: VerifyArgs,
    /// Absent absolute `<archive>.spdx.json` file outside the SDK package directory
    #[arg(long, value_name = "FILE")]
    output: PathBuf,
}

fn parse_positive_bytes(value: &str) -> std::result::Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "archive size must be a positive byte count".into())
}

fn selected_identity(args: &SelectionArgs) -> Result<SdkIdentity> {
    read_selected_identity(&args.identity, &args.identity_sha256)
}

fn read_selected_identity(path: &std::path::Path, expected: &Sha256Digest) -> Result<SdkIdentity> {
    let (_, measured) = measure_regular_file_digest_bounded(path, SDK_MAX_METADATA_BYTES as u64)
        .into_diagnostic()?;
    if &measured.digest != expected {
        miette::bail!("SDK identity document differs from its independently selected SHA-256");
    }
    let mut bytes = Vec::new();
    open_regular_file_nofollow(path)
        .into_diagnostic()?
        .take(SDK_MAX_METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .into_diagnostic()?;
    if bytes.len() > SDK_MAX_METADATA_BYTES || aros_common::sha256_bytes(&bytes) != measured.digest
    {
        miette::bail!("SDK identity document changed or exceeded its byte limit");
    }
    let identity: SdkIdentity = serde_json::from_slice(&bytes).into_diagnostic()?;
    identity.validate().map_err(|e| miette::miette!("{e}"))?;
    Ok(identity)
}

#[cfg(unix)]
fn verification(args: &VerifyArgs) -> Result<aros_toolchain::sdk_package::SdkVerificationRequest> {
    Ok(aros_toolchain::sdk_package::SdkVerificationRequest {
        package_dir: args.package_dir.clone(),
        identity: selected_identity(&args.selection)?,
        expected_tree_sha256: args.selection.tree_sha256.clone(),
        expected_archive_sha256: args.archive_sha256.clone(),
        expected_archive_size: args.archive_size,
        forbidden_prefixes: args.selection.forbidden_prefixes.clone(),
    })
}

#[cfg(unix)]
/// Execute one SDK operation without source discovery or release publication.
pub async fn run(command: SdkCommand) -> Result<()> {
    match command {
        SdkCommand::VerifySource(args) => verify_source(&args),
        SdkCommand::Build(args) => build(*args).await,
        SdkCommand::Download(args) => crate::sdk_managed::download(args).await,
        SdkCommand::Install(args) => crate::sdk_managed::install(args).await,
        SdkCommand::List(args) => crate::sdk_managed::list(args),
        SdkCommand::Select(args) => crate::sdk_managed::select(args),
        SdkCommand::Verify(args) => crate::sdk_managed::verify(args, false),
        SdkCommand::Path(args) => crate::sdk_managed::verify(args, true),
        local => run_local(local),
    }
}

#[cfg(unix)]
fn verify_source(args: &SourceArgs) -> Result<()> {
    let verified = verify_source_selection(args)?;
    read_selected_identity(&args.identity, &args.identity_sha256)?;
    aros_common::outputln!(
        "{}",
        serde_json::json!({
            "schema":"aros-sdk-source-binding-v2", "operation":"sdk.verify-source",
            "assurance":"local-source-binding-not-execution-or-release-admission",
            "identity":verified.identity(), "source_preset":verified.source_profile().name,
            "materialized_source_dir":verified.materialized_source_root(),
        })
    );
    Ok(())
}

#[cfg(unix)]
fn verify_source_selection(
    args: &SourceArgs,
) -> Result<aros_toolchain::sdk_source::VerifiedSdkSource> {
    let identity = read_selected_identity(&args.identity, &args.identity_sha256)?;
    let verified = aros_toolchain::sdk_source::verify_sdk_source(
        &aros_toolchain::sdk_source::SdkSourceVerificationRequest {
            source_root: args.source_dir.clone(),
            materialized_source_root: args.materialized_source_dir.clone(),
            source_preset: args.source_preset.clone(),
            identity,
        },
    ).map_err(|error| {
        let mut diagnostic = error.diagnostics().diagnostics[0].clone();
        diagnostic.hint = Some("select the exact clean original checkout, its engine-free materialization and sealed source SDK contract; source verification is read-only and does not repair or execute inputs".into());
        crate::observability::native_diagnostic(diagnostic)
    })?;
    Ok(verified)
}

#[cfg(unix)]
async fn build(args: BuildArgs) -> Result<()> {
    // Establish the SDK identity/source binding before the compatibility path can
    // materialize any private roots or invoke source-owned programs.
    let source = verify_source_selection(&args.source)?;
    read_selected_identity(&args.source.identity, &args.source.identity_sha256)?;
    let original_source_dir = args.source.source_dir.clone();
    let identity_path = args.source.identity.clone();
    native_compatibility::build_sdk(source, original_source_dir, identity_path, args.native).await
}

#[cfg(not(unix))]
async fn build(_args: BuildArgs) -> Result<()> {
    miette::bail!("local SDK generation requires a supported Unix host")
}

#[cfg(unix)]
fn run_local(command: SdkCommand) -> Result<()> {
    use aros_toolchain::sdk_package;
    let operation = command.diagnostic_mode();
    let result = match command {
        SdkCommand::Package(args) => {
            let identity = selected_identity(&args.selection)?;
            let output = sdk_package::package(&sdk_package::SdkPackageRequest {
                candidate_root: args.candidate_root,
                output_dir: args.output_dir,
                identity: identity.clone(),
                expected_tree_sha256: args.selection.tree_sha256.clone(),
                forbidden_prefixes: args.selection.forbidden_prefixes.clone(),
            })
            .map_err(|error| native_error(&error))?;
            selected_identity(&args.selection)?;
            serde_json::json!({"identity":identity,"output_dir":output.output_dir,
                "archive":output.archive,"manifest":output.manifest,"checksum":output.checksum,
                "archive_sha256":output.archive_sha256,"archive_size":output.archive_size,
                "tree_sha256":args.selection.tree_sha256})
        }
        SdkCommand::VerifyPackage(args) => {
            let verified =
                sdk_package::verify(&verification(&args)?).map_err(|error| native_error(&error))?;
            selected_identity(&args.selection)?;
            serde_json::json!({"identity":verified.manifest.identity,
                "archive_sha256":verified.archive_sha256,"archive_size":verified.archive_size,
                "tree_sha256":verified.manifest.tree_sha256})
        }
        SdkCommand::Extract(args) => {
            let verified = sdk_package::verify_and_extract(
                &verification(&args.verification)?,
                &args.output_root,
            )
            .map_err(|error| native_error(&error))?;
            selected_identity(&args.verification.selection)?;
            serde_json::json!({"identity":verified.manifest.identity,"output_root":args.output_root,
                "archive_sha256":verified.archive_sha256,"archive_size":verified.archive_size,
                "tree_sha256":verified.manifest.tree_sha256})
        }
        SdkCommand::Sbom(args) => write_sbom(&args)?,
        _ => {
            miette::bail!("managed SDK command reached the local package boundary");
        }
    };
    aros_common::outputln!(
        "{}",
        serde_json::json!({
            "schema":"aros-sdk-local-stage-v2", "operation":operation,
            "assurance":"local-byte-consistency-not-release-admission", "result":result,
        })
    );
    Ok(())
}

#[cfg(unix)]
fn write_sbom(args: &SbomArgs) -> Result<serde_json::Value> {
    let request = verification(&args.verification)?;
    let name = format!(
        "{}.spdx.json",
        request
            .identity
            .asset_name()
            .map_err(|error| miette::miette!("{error}"))?
    );
    if !args.output.is_absolute()
        || args.output.file_name().and_then(|n| n.to_str()) != Some(name.as_str())
    {
        miette::bail!("SDK SPDX output must be an absolute <archive>.spdx.json path");
    }
    let parent = args
        .output
        .parent()
        .ok_or_else(|| miette::miette!("SDK SPDX output has no parent"))?
        .canonicalize()
        .into_diagnostic()?;
    let package_dir = request
        .package_dir
        .canonicalize()
        .map_err(|error| miette::miette!("SDK directory is inaccessible: {error}"))?;
    if !parent.is_dir() || parent.starts_with(package_dir) {
        miette::bail!("SDK SPDX output must be outside the SDK package in an existing directory");
    }
    let destination = spdx_output::Destination::prepare(&parent, &name).into_diagnostic()?;
    let output = destination.path();
    let bytes = aros_toolchain::sdk_sbom::build_measured_sdk_sbom(&request)
        .map_err(|error| native_error(&error))?;
    selected_identity(&args.verification.selection)?;
    let digest = aros_common::sha256_bytes(&bytes);
    destination.write(&bytes).map_err(|error| {
        miette::miette!("SDK SPDX output already exists or is unavailable: {error}")
    })?;
    let (_, measured) = measure_regular_file_digest_bounded(output, SDK_MAX_METADATA_BYTES as u64)
        .into_diagnostic()?;
    if measured.digest != digest
        || measured.size != bytes.len() as u64
        || aros_toolchain::sdk_sbom::build_measured_sdk_sbom(&request)
            .map_err(|error| native_error(&error))?
            != bytes
    {
        miette::bail!("SDK package or SPDX output changed during generation; output retained");
    }
    selected_identity(&args.verification.selection)?;
    Ok(serde_json::json!({"output":output,"sha256":digest,"size":measured.size}))
}

#[cfg(unix)]
fn native_error(error: &aros_toolchain::ContractError) -> miette::Report {
    let mut diagnostic = error.diagnostics().diagnostics[0].clone();
    diagnostic.hint = Some("inspect the named SDK input and independent selection; packaging never changes the candidate or reuses an output, and an extraction root is retained if failure occurs after its creation".into());
    crate::observability::native_diagnostic(diagnostic)
}

#[cfg(not(unix))]
/// Refuse unsupported local SDK execution rather than approximating Unix policy.
pub async fn run(_command: SdkCommand) -> Result<()> {
    miette::bail!("local SDK package stages require a supported Unix host")
}

#[cfg(test)]
mod build_tests {
    use clap::CommandFactory as _;

    #[test]
    fn sdk_build_reuses_source_binding_without_redundant_generation_selectors() {
        let root = crate::Cli::command();
        let command = root
            .find_subcommand("sdk")
            .unwrap()
            .find_subcommand("build")
            .unwrap();
        let ids = command
            .get_arguments()
            .map(|argument| argument.get_id().as_str())
            .collect::<Vec<_>>();

        for required in [
            "identity",
            "identity_sha256",
            "source_dir",
            "materialized_source_dir",
            "source_preset",
            "preset",
            "generation_record",
            "output_dir",
            "sdk_forbidden_prefixes",
            "forbidden_prefixes",
        ] {
            assert!(ids.contains(&required), "missing --{required}");
        }
        for prohibited in [
            "candidate_root",
            "tree_sha256",
            "consumer_source_date_epoch",
        ] {
            assert!(!ids.contains(&prohibited), "unexpected --{prohibited}");
        }
    }
}
