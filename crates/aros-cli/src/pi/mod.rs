//! Raspberry Pi build, deployment, service, console, and SD-card workflows.

pub mod config;
pub mod console;
pub mod deploy;
pub mod dhcp;
mod disk_inventory;
pub mod doctor;
mod scan;
#[cfg(target_os = "linux")]
mod scan_linux;
#[cfg(target_os = "macos")]
mod scan_macos;
pub mod sd;
pub mod sd_disk;
mod sd_manifest;
pub mod sd_unmount;
pub mod serve;
pub mod tftp;

use crate::build::{self, BuildOptions};
use config::{Board, Transport};
use console::ConsoleProgram;
use miette::Result;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// A USB CDC-ECM network function discovered on the local host.
///
/// The current BSD/Linux interface name is deliberately reported but not used
/// as the stable identity.  A later board pairing step matches the USB
/// descriptor identity and resolves the ephemeral interface name again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UsbEcmAdapter {
    pub interface: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub interface_mac: Option<String>,
    pub ipv4_addresses: Vec<Ipv4Addr>,
    pub cdc_ecm: bool,
}

/// Convert a selected local board profile into the immutable identity contract
/// required by an external SD boot bundle.
pub fn sd_bundle_expectation(board: &Board) -> Result<sd::BundleExpectation> {
    let expectation = sd::BundleExpectation::new(
        &board.name,
        &board.config.model,
        board.config.transport.to_string(),
    );
    if board.config.transport != Transport::UbootUsbEcm {
        return Ok(expectation);
    }

    let identity = board
        .config
        .usb_ecm
        .as_ref()
        .and_then(|usb_ecm| usb_ecm.identity.as_ref())
        .ok_or_else(|| {
            miette::miette!(
                "Board '{}' needs usb_ecm.identity before an SD image can be made for uboot-usb-ecm.",
                board.name
            )
        })?;
    Ok(expectation.with_usb_ecm_identity(sd::UsbEcmIdentity {
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        serial: identity.serial.clone(),
        expected_target_mac: identity.expected_target_mac.clone(),
    }))
}

/// Validate an external boot bundle and, after explicit approval, create its
/// verified MBR/FAT32 SD image artifact.  This operation writes only below the
/// caller-selected output directory; it never touches a physical disk.
pub fn create_sd_image(
    board: &Board,
    boot_bundle: &Path,
    output_dir: &Path,
    apply: bool,
) -> Result<()> {
    let expectation = sd_bundle_expectation(board)?;
    let bundle = sd::validate_boot_bundle(boot_bundle, &expectation)?;
    println!("💾 AROS Pi SD image plan");
    println!(
        "  • Board:      {} ({})",
        board.name, board.config.transport
    );
    println!("  • Bundle:     {}", bundle.source_dir().display());
    println!(
        "  • Partition:  {} {} bytes @ LBA {}",
        bundle.partition().filesystem,
        bundle.partition().size_bytes,
        bundle.partition().start_lba
    );
    println!("  • Files:      {}", bundle.files().len());
    println!("  • Output:     {}", output_dir.display());
    if !apply {
        println!(
            "  Dry run: the external bundle validated; no image was written. Pass --apply to create the artifact."
        );
        return Ok(());
    }

    let artifact = sd::stage_boot_bundle(&bundle, output_dir)?;
    println!(
        "✅ Created verified SD artifact '{}'.",
        artifact.artifact_dir().display()
    );
    println!("  • Image:      {}", artifact.image().path().display());
    println!("  • SHA-256:    {}", artifact.image().sha256());
    println!("  • Manifest:   {}", artifact.manifest_path().display());
    Ok(())
}

/// List only current whole removable disks which pass the platform safety
/// predicates. Supplying an artifact additionally derives a per-disk write
/// token, but does not open or change any disk.
pub fn scan_sd_disks(artifact_dir: Option<&Path>) -> Result<()> {
    let artifact = artifact_dir
        .map(|directory| {
            sd_disk::verify_image_artifact(
                directory,
                Path::new(sd::ARTIFACT_MANIFEST),
                Path::new(sd::RAW_IMAGE_FILENAME),
            )
        })
        .transpose()?;
    let candidates = sd_disk::scan()?;

    println!("💾 AROS Pi safe SD-card scan");
    if let Some(artifact) = &artifact {
        println!("  • Artifact:   {}", artifact.artifact_dir().display());
        println!("  • Image:      {}", artifact.image_path().display());
        println!("  • SHA-256:    {}", artifact.image_sha256());
    }
    if candidates.is_empty() {
        println!("  No safe, unmounted removable whole-disk target was found.");
        println!("  No disk was opened or changed.");
        return Ok(());
    }

    for candidate in &candidates {
        println!("  • {}", candidate.summary());
        if let Some(artifact) = &artifact {
            println!(
                "    Confirm token: {}",
                sd_disk::confirmation_token(artifact, candidate)
            );
        }
    }
    if artifact.is_none() {
        println!(
            "  Pass --artifact <DIR> to verify an image and print its per-disk confirmation token."
        );
    }
    println!("  No disk was opened or changed.");
    Ok(())
}

/// List mounted removable whole disks or explicitly unmount exactly one
/// current scan ID. Merely selecting a disk remains a non-mutating preview;
/// the platform unmount is reached only when `apply` is true.
pub fn unmount_sd_disk(selected_scan_id: Option<&str>, apply: bool, dry_run: bool) -> Result<()> {
    let candidates = sd_unmount::scan()?;

    println!("💾 AROS Pi safe SD-card unmount");
    let Some(selected_scan_id) = selected_scan_id else {
        if candidates.is_empty() {
            println!("  No mounted removable whole-disk target was found.");
        } else {
            for candidate in &candidates {
                println!("  • {}", candidate.summary());
                for mount_point in candidate.mount_points() {
                    println!("    Mounted at: {}", mount_point.display());
                }
            }
            println!(
                "  Select one current scan ID with --device <SCAN_ID>; add --apply only when it should be unmounted."
            );
        }
        println!("  No disk was opened, unmounted, or changed.");
        return Ok(());
    };

    let selected = candidates
        .iter()
        .filter(|candidate| candidate.scan_id == selected_scan_id)
        .collect::<Vec<_>>();
    let candidate = match selected.as_slice() {
        [candidate] => *candidate,
        [] => {
            miette::bail!(
                "No currently mounted removable whole disk has scan ID '{}'. Re-run `aros pi sd unmount`; nothing was unmounted.",
                selected_scan_id
            );
        }
        _ => {
            miette::bail!(
                "More than one current mounted removable whole disk has scan ID '{}'; refusing an ambiguous unmount.",
                selected_scan_id
            );
        }
    };

    println!("  • Target:     {}", candidate.summary());
    for mount_point in candidate.mount_points() {
        println!("  • Mount:      {}", mount_point.display());
    }
    if !apply || dry_run {
        if dry_run {
            println!("  Dry run: the target was validated; nothing was unmounted or changed.");
        } else {
            println!("  Preview only: pass --apply with this --device selection to unmount it.");
        }
        return Ok(());
    }

    let report = sd_unmount::unmount(selected_scan_id)?;
    println!("✅ Removable whole disk was unmounted.");
    println!("  • Disk:       {}", report.scan_id);
    for mount_point in &report.unmounted_mount_points {
        println!("  • Unmounted:  {}", mount_point.display());
    }
    Ok(())
}

/// Preview or perform the one irreversible step of copying a verified image
/// to a deliberately selected whole removable disk. The writer repeats every
/// artifact, board and disk check immediately before opening the raw device.
pub fn write_sd_image(
    board: &Board,
    artifact_dir: &Path,
    selected_scan_id: &str,
    confirmation: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let artifact = sd_disk::verify_image_artifact_for_board(
        artifact_dir,
        Path::new(sd::ARTIFACT_MANIFEST),
        Path::new(sd::RAW_IMAGE_FILENAME),
        board,
    )?;
    let candidates = sd_disk::scan()?;
    let selected = candidates
        .iter()
        .filter(|candidate| candidate.scan_id == selected_scan_id)
        .collect::<Vec<_>>();
    let candidate = match selected.as_slice() {
        [candidate] => *candidate,
        [] => {
            miette::bail!(
                "No currently safe removable whole disk has scan ID '{}'. Re-run `aros pi sd scan`; no disk was opened.",
                selected_scan_id
            );
        }
        _ => {
            miette::bail!(
                "More than one current disk has scan ID '{}'; refusing an ambiguous write.",
                selected_scan_id
            );
        }
    };
    let expected_token = sd_disk::confirmation_token(&artifact, candidate);

    println!("💾 AROS Pi SD-card write plan");
    println!(
        "  • Board:      {} ({})",
        board.name, board.config.transport
    );
    println!("  • Artifact:   {}", artifact.artifact_dir().display());
    println!("  • Image:      {}", artifact.image_path().display());
    println!("  • SHA-256:    {}", artifact.image_sha256());
    println!("  • Target:     {}", candidate.summary());
    println!("  • Token:      {expected_token}");

    if let Some(provided) = confirmation {
        if provided != expected_token {
            miette::bail!(
                "Confirmation token does not match the currently verified image and disk '{}'; no disk was opened.",
                selected_scan_id
            );
        }
    }
    if dry_run || confirmation.is_none() {
        if confirmation.is_none() {
            println!(
                "  Preview only: pass the token above as --confirm to authorize this one write."
            );
        } else {
            println!("  Dry run: the token and target validated; no disk was opened or changed.");
        }
        return Ok(());
    }

    let report = sd_disk::write_verified_image_for_board(
        &artifact,
        board,
        selected_scan_id,
        confirmation.expect("checked above"),
    )?;
    println!("✅ Verified SD image write completed.");
    println!("  • Disk:       {}", report.scan_id);
    println!("  • Bytes:      {}", report.bytes_written);
    println!("  • Readback:   {}", report.readback_sha256);
    Ok(())
}

/// Find USB CDC-ECM adapters without changing any network configuration.
pub fn scan() -> Result<()> {
    scan::print()
}

pub fn doctor(board: &Board, repo_root: &Path) -> Result<()> {
    println!("🩺 Checking AROS Pi board profile '{}'...", board.name);
    let report = doctor::inspect(board, repo_root);
    report.print();
    if report.has_failures() {
        miette::bail!(
            "Board '{}' is not ready. Fix the failed checks above; no hardware or network configuration was changed.",
            board.name
        );
    }
    Ok(())
}

pub async fn build(
    board: &Board,
    repo_root: &Path,
    mut options: BuildOptions,
    dtb_override: Option<&Path>,
    core_kobj_override: Option<&Path>,
) -> Result<()> {
    println!(
        "🧭 Building Pi board '{}' ({}, transport {})...",
        board.name, board.config.model, board.config.transport
    );
    if let Some(dtb_path) = board.rpi4_dtb_path(repo_root, dtb_override)? {
        options.cmake_definitions.push(build::CmakeDefinition {
            key: "AROS_RPI4_DTB".to_string(),
            value: dtb_path.to_string_lossy().into_owned(),
        });
    }
    if let Some(core_kobj_dir) = board.rpi4_core_kobj_dir(repo_root, core_kobj_override)? {
        options.cmake_definitions.push(build::CmakeDefinition {
            key: "AROS_RPI4_CORE_KOBJ_DIR".to_string(),
            value: core_kobj_dir.to_string_lossy().into_owned(),
        });
    }
    build::run(repo_root, &options).await
}

pub fn deploy(
    board: &Board,
    repo_root: &Path,
    artifact_override: Option<&Path>,
    apply: bool,
) -> Result<()> {
    let plan = deploy::DeploymentPlan::create(board, repo_root, artifact_override)?;
    deploy::print_plan(&plan, apply);
    if !apply {
        return Ok(());
    }

    deploy::publish(&plan)?;
    println!(
        "✅ Published '{}' into the local TFTP tree at {}.",
        board.name,
        plan.destination_dir.display()
    );
    Ok(())
}

pub fn console(
    board: &Board,
    requested_program: ConsoleProgram,
    device_override: Option<PathBuf>,
    baud_override: Option<u32>,
    dry_run: bool,
) -> Result<()> {
    let device = match device_override {
        Some(device) => device,
        None => board.serial_device()?.to_path_buf(),
    };
    let baud = baud_override.unwrap_or(board.config.serial_baud);
    let plan = console::plan(requested_program, &device, baud)?;
    println!("🖥️  Serial terminal: {}", plan.display());
    if dry_run {
        println!("  Dry run: serial terminal was not started.");
        return Ok(());
    }
    console::run(&plan)
}
