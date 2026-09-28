//! Read-only boot-media artifact commands.

use aros_board::sd::{verify_fat32_media_artifact, VerifiedMediaArtifact};
use clap::{Args, Subcommand, ValueEnum};
use miette::Result;
use std::path::PathBuf;

/// Operations on a completed image artifact, never a block device.
#[derive(Subcommand)]
pub enum ImageCommand {
    /// Show measured facts after independently verifying the artifact
    Inspect(ImageArtifactArgs),
    /// Verify the manifest, checksums and image filesystem
    Verify(ImageArtifactArgs),
}

/// An existing, complete artifact directory.
#[derive(Args)]
pub struct ImageArtifactArgs {
    /// Directory containing media-image.json, SHA256SUMS and aros-media.img
    #[arg(long, value_name = "DIR")]
    artifact: PathBuf,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: ImageOutputFormat,
}

/// Presentation of successfully verified image facts.
#[derive(Clone, Copy, ValueEnum)]
pub enum ImageOutputFormat {
    /// Compact, human-readable facts.
    Human,
    /// Versioned JSON suitable for automation.
    Json,
}

/// Run one read-only image command.
///
/// # Errors
///
/// Rejects incomplete or altered artifacts, unsupported formats and unsafe
/// paths before printing a success result.
pub fn run(command: ImageCommand) -> Result<()> {
    let (operation, args) = match command {
        ImageCommand::Inspect(args) => ("inspect", args),
        ImageCommand::Verify(args) => ("verify", args),
    };
    let result = verify_fat32_media_artifact(&args.artifact)?;
    match args.format {
        ImageOutputFormat::Human => print_human(operation, &result),
        ImageOutputFormat::Json => print_json(operation, &result)?,
    }
    Ok(())
}

fn print_human(operation: &str, result: &VerifiedMediaArtifact) {
    if operation == "verify" {
        aros_common::outputln!(
            "Verified FAT32 image: SHA-256 {} ({} bytes, {} files).",
            result.image_sha256,
            result.image_size_bytes,
            result.file_count
        );
    } else {
        aros_common::outputln!("Media artifact: verified MBR/FAT32 image");
        aros_common::outputln!("  Profile:     {}", result.profile_id);
        aros_common::outputln!("  Target:      {}", result.target_preset);
        aros_common::outputln!("  Image SHA:   {}", result.image_sha256);
        aros_common::outputln!("  Image bytes: {}", result.image_size_bytes);
        aros_common::outputln!("  Files:       {}", result.file_count);
    }
}

fn print_json(operation: &str, result: &VerifiedMediaArtifact) -> Result<()> {
    let document = serde_json::json!({
        "format_version": 1,
        "kind": "aros-media-verification",
        "operation": operation,
        "verified": true,
        "format": "mbr-fat32",
        "profile_id": result.profile_id,
        "profile_sha256": result.profile_sha256,
        "receipt_sha256": result.receipt_sha256,
        "receipt_origin": result.receipt_origin,
        "target_preset": result.target_preset,
        "external_lock_sha256": result.external_lock_sha256,
        "image_sha256": result.image_sha256,
        "image_size_bytes": result.image_size_bytes,
        "file_count": result.file_count,
        "provenance_authenticated": false,
        "boot_qualified": false,
    });
    let encoded = serde_json::to_string_pretty(&document)
        .map_err(|error| miette::miette!("Could not encode image verification result: {error}"))?;
    aros_common::outputln!("{encoded}");
    Ok(())
}
