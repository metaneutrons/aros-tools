//! Reviewed boot-media planning, composition and artifact inspection commands.

use aros_board::sd::{stage_fat32_media_plan, verify_fat32_media_artifact, VerifiedMediaArtifact};
use aros_common::media_plan::{plan_media_image, MediaExternalFile};
use aros_common::media_profile::{built_in_media_profiles, MediaLayout};
use clap::{Args, Subcommand, ValueEnum};
use miette::Result;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_INPUT_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;

/// Plan, compose or inspect an image artifact; never write a block device.
#[derive(Subcommand)]
pub enum ImageCommand {
    /// Plan or explicitly compose a reviewed MBR/FAT32 media profile
    Build(ImageBuildArgs),
    /// Show measured facts after independently verifying the artifact
    Inspect(ImageArtifactArgs),
    /// Verify the manifest, checksums and image filesystem
    Verify(ImageArtifactArgs),
}

/// Exact paths to reviewed media contracts and measured inputs.
#[derive(Args)]
pub struct ImageBuildArgs {
    /// Exact profile ID from the reviewed built-in media registry
    #[arg(long, value_name = "ID")]
    profile: String,

    /// Root containing the relative files named in the build receipt
    #[arg(long, value_name = "DIR")]
    build_root: PathBuf,

    /// Versioned CMake or legacy-adapter build receipt
    #[arg(long, value_name = "FILE")]
    receipt: PathBuf,

    /// New output artifact directory; an existing path is refused
    #[arg(long, value_name = "DIR")]
    output: PathBuf,

    /// Reviewed external lock document as ID=FILE; repeat per profile lock
    #[arg(long, value_name = "ID=FILE", value_parser = parse_lock_binding)]
    lock: Vec<LockBinding>,

    /// Exact local locked input as LOCK_ID:FILE_ID=PATH; repeat per file
    #[arg(long, value_name = "LOCK_ID:FILE_ID=PATH", value_parser = parse_external_binding)]
    external: Vec<MediaExternalFile>,

    /// Create the artifact after validation; otherwise only show the plan
    #[arg(long, conflicts_with = "dry_run")]
    apply: bool,

    /// Explicitly request non-mutating planning (the default)
    #[arg(long, conflicts_with = "apply")]
    dry_run: bool,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: ImageOutputFormat,
}

#[derive(Clone)]
struct LockBinding {
    id: String,
    path: PathBuf,
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

/// Run one boot-media image command.
///
/// # Errors
///
/// Rejects incomplete or altered artifacts, unsupported formats and unsafe
/// paths before printing a success result.
pub fn run(command: ImageCommand) -> Result<()> {
    let (operation, args) = match command {
        ImageCommand::Build(args) => return build(&args),
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

fn build(args: &ImageBuildArgs) -> Result<()> {
    let profile = built_in_media_profiles()
        .map_err(|error| miette::miette!("Cannot load reviewed media profiles: {error}"))?
        .into_iter()
        .find(|entry| entry.profile.id == args.profile)
        .ok_or_else(|| miette::miette!("Unknown reviewed media profile '{}'.", args.profile))?;
    if !matches!(&profile.profile.layout, MediaLayout::MbrFat32 { .. }) {
        miette::bail!("The selected media profile has no implemented image backend.");
    }
    let receipt = read_regular_bounded(&args.receipt)?;
    let mut lock_bytes = Vec::with_capacity(args.lock.len());
    for binding in &args.lock {
        lock_bytes.push((binding.id.as_str(), read_regular_bounded(&binding.path)?));
    }
    let lock_documents: Vec<_> = lock_bytes
        .iter()
        .map(|(id, bytes)| (*id, bytes.as_slice()))
        .collect();
    let plan = plan_media_image(
        &profile,
        &args.build_root,
        &receipt,
        &lock_documents,
        &args.external,
    )
    .map_err(|error| miette::miette!("Cannot plan media image: {error}"))?;
    if !args.apply {
        match args.format {
            ImageOutputFormat::Human => {
                aros_common::outputln!(
                    "Ready to compose {} files for profile '{}' ({} raw image bytes).",
                    plan.files.len(),
                    plan.profile_id,
                    plan.raw_image_size_bytes.unwrap_or_default()
                );
                aros_common::outputln!("  Output: {}", args.output.display());
                aros_common::outputln!("  Use --apply to create the image; no device is written.");
            }
            ImageOutputFormat::Json => {
                let document = serde_json::json!({
                    "format_version": 1,
                    "kind": "aros-media-build-plan",
                    "applied": false,
                    "profile_id": plan.profile_id,
                    "profile_sha256": plan.profile_sha256,
                    "receipt_sha256": plan.receipt_sha256,
                    "target_preset": plan.target_preset,
                    "file_count": plan.files.len(),
                    "payload_size_bytes": plan.total_payload_bytes,
                    "raw_image_size_bytes": plan.raw_image_size_bytes,
                    "output": args.output,
                });
                print_json_value(&document)?;
            }
        }
        return Ok(());
    }
    let staged = stage_fat32_media_plan(&plan, &args.output)?;
    let verified = verify_fat32_media_artifact(&staged.artifact_dir)?;
    if verified.profile_sha256 != plan.profile_sha256
        || verified.receipt_sha256 != plan.receipt_sha256
        || verified.image_sha256.to_string() != staged.image.sha256()
    {
        miette::bail!("Published media artifact differs from its verified composition plan.");
    }
    match args.format {
        ImageOutputFormat::Human => {
            aros_common::outputln!(
                "Created verified MBR/FAT32 artifact: {}",
                staged.artifact_dir.display()
            );
            print_human("verify", &verified);
            aros_common::outputln!("No block device was written; hardware boot is unqualified.");
        }
        ImageOutputFormat::Json => {
            let document = serde_json::json!({
                "format_version": 1,
                "kind": "aros-media-build-result",
                "applied": true,
                "artifact_dir": staged.artifact_dir,
                "profile_id": verified.profile_id,
                "profile_sha256": verified.profile_sha256,
                "receipt_sha256": verified.receipt_sha256,
                "image_sha256": verified.image_sha256,
                "image_size_bytes": verified.image_size_bytes,
                "file_count": verified.file_count,
                "device_written": false,
                "boot_qualified": false,
            });
            print_json_value(&document)?;
        }
    }
    Ok(())
}

fn parse_lock_binding(raw: &str) -> std::result::Result<LockBinding, String> {
    let (id, path) = raw.split_once('=').ok_or("expected ID=FILE")?;
    if id.is_empty() || path.is_empty() {
        return Err("expected nonempty ID=FILE".into());
    }
    Ok(LockBinding {
        id: id.into(),
        path: PathBuf::from(path),
    })
}

fn parse_external_binding(raw: &str) -> std::result::Result<MediaExternalFile, String> {
    let (identity, path) = raw.split_once('=').ok_or("expected LOCK_ID:FILE_ID=PATH")?;
    let (lock_id, file_id) = identity
        .split_once(':')
        .ok_or("expected LOCK_ID:FILE_ID=PATH")?;
    if lock_id.is_empty() || file_id.is_empty() || path.is_empty() {
        return Err("expected nonempty LOCK_ID:FILE_ID=PATH".into());
    }
    Ok(MediaExternalFile {
        lock_id: lock_id.into(),
        file_id: file_id.into(),
        path: PathBuf::from(path),
    })
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = aros_common::open_regular_file_nofollow(path).map_err(|error| {
        miette::miette!("Cannot open media input '{}': {error}", path.display())
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            miette::miette!("Cannot read media input '{}': {error}", path.display())
        })?;
    if bytes.len() as u64 > MAX_INPUT_DOCUMENT_BYTES {
        miette::bail!("Media input '{}' exceeds its size limit.", path.display());
    }
    Ok(bytes)
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
    print_json_value(&document)
}

fn print_json_value(document: &serde_json::Value) -> Result<()> {
    let encoded = serde_json::to_string_pretty(document)
        .map_err(|error| miette::miette!("Could not encode image verification result: {error}"))?;
    aros_common::outputln!("{encoded}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::media_receipt::{MediaBuildFile, MediaBuildReceipt, MediaReceiptOrigin};
    use std::fs;

    fn build_args(root: &Path, output: &Path, apply: bool) -> ImageBuildArgs {
        let profile = built_in_media_profiles().unwrap().remove(0).profile;
        let files = profile
            .required_files
            .iter()
            .map(|required| {
                let path = format!("{}.bin", required.role);
                let bytes = required.role.as_bytes();
                fs::write(root.join(&path), bytes).unwrap();
                MediaBuildFile {
                    role: required.role.clone(),
                    path,
                    sha256: aros_common::sha256_bytes(bytes).to_string(),
                    size_bytes: bytes.len() as u64,
                }
            })
            .collect();
        let receipt = MediaBuildReceipt::new(
            MediaReceiptOrigin::Cmake,
            profile.target_preset,
            profile.model,
            profile.transport,
            files,
        )
        .unwrap();
        let receipt_path = root.join("receipt.json");
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        ImageBuildArgs {
            profile: profile.id,
            build_root: root.to_path_buf(),
            receipt: receipt_path,
            output: output.to_path_buf(),
            lock: Vec::new(),
            external: Vec::new(),
            apply,
            dry_run: !apply,
            format: ImageOutputFormat::Json,
        }
    }

    #[test]
    fn build_preview_is_read_only_and_apply_publishes_only_new_verified_artifacts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("inputs");
        fs::create_dir(&root).unwrap();
        let output = temporary.path().join("artifact");
        build(&build_args(&root, &output, false)).unwrap();
        assert!(!output.exists());

        build(&build_args(&root, &output, true)).unwrap();
        let verified = verify_fat32_media_artifact(&output).unwrap();
        assert_eq!(verified.profile_id, "rpi4-uboot-usb-ecm");
        assert_eq!(verified.file_count, 6);
        assert!(build(&build_args(&root, &output, true)).is_err());
    }

    #[test]
    fn rejects_changed_inputs_and_malformed_bindings() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("inputs");
        fs::create_dir(&root).unwrap();
        let output = temporary.path().join("artifact");
        let args = build_args(&root, &output, true);
        fs::write(root.join("config.bin"), b"changed").unwrap();
        assert!(build(&args).is_err());
        assert!(!output.exists());
        assert!(parse_lock_binding("=missing-id").is_err());
        assert!(parse_external_binding("lock=missing-file-id").is_err());
        assert_eq!(
            parse_external_binding("lock:file=/tmp/input")
                .unwrap()
                .file_id,
            "file"
        );
        assert_eq!(
            parse_lock_binding("firmware=/tmp/firmware=v1.toml")
                .unwrap()
                .path,
            PathBuf::from("/tmp/firmware=v1.toml")
        );
    }
}
