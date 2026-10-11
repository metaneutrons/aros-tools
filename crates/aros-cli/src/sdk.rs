//! SDK artifact stages and explicit lock-selected managed operations.

use aros_common::sdk_manifest::{SdkIdentity, SDK_MAX_METADATA_BYTES};
use aros_common::{measure_regular_file_digest_bounded, open_regular_file_nofollow, Sha256Digest};
use clap::{Args, Subcommand};
use miette::{IntoDiagnostic, Result};
use std::io::Read;
use std::path::PathBuf;

#[derive(Subcommand)]
/// Local artifact stages and management for a separately identified SDK.
pub enum SdkCommand {
    /// Package one source-produced SDK without modifying it or publishing a release
    Package(PackageArgs),
    /// Verify a complete local SDK package against explicit identity and digests
    VerifyPackage(VerifyArgs),
    /// Verify and extract one SDK into an absent directory; not managed installation
    Extract(ExtractArgs),
    /// Acquire and completely verify locked SDK bytes without installing them
    Download(crate::sdk_managed::DownloadArgs),
    /// Install or fully verify reuse of one exact lock-selected SDK
    Install(crate::sdk_managed::InstallArgs),
    /// List bounded store metadata without hashing or executing SDK payloads
    List(crate::sdk_managed::ListArgs),
    /// Preview or apply one application project's verified SDK selection
    Select(crate::sdk_managed::SelectArgs),
    /// Fully verify the SDK selected by explicit release or project metadata
    #[command(
        after_help = "Select either `--release-lock FILE --lock-sha256 SHA256` or `--project-lock FILE`. A project lock is explicit local application configuration, not publisher authentication."
    )]
    Verify(crate::sdk_managed::InstalledArgs),
    /// Print the fully verified installed SDK path for application build tools
    #[command(
        after_help = "Select either `--release-lock FILE --lock-sha256 SHA256` or `--project-lock FILE`. A project lock is explicit local application configuration, not publisher authentication."
    )]
    Path(crate::sdk_managed::InstalledArgs),
}

impl SdkCommand {
    /// Exact operation label for the frontend diagnostic envelope.
    pub const fn diagnostic_mode(&self) -> &'static str {
        match self {
            Self::Package(_) => "sdk.package",
            Self::VerifyPackage(_) => "sdk.verify-package",
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

fn parse_positive_bytes(value: &str) -> std::result::Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "archive size must be a positive byte count".into())
}

fn selected_identity(args: &SelectionArgs) -> Result<SdkIdentity> {
    let (_, measured) =
        measure_regular_file_digest_bounded(&args.identity, SDK_MAX_METADATA_BYTES as u64)
            .into_diagnostic()?;
    if measured.digest != args.identity_sha256 {
        miette::bail!("SDK identity document differs from its independently selected SHA-256");
    }
    let mut bytes = Vec::new();
    open_regular_file_nofollow(&args.identity)
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
