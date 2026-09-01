//! CLI presentation and orchestration for physical-board workflows.

pub mod console;
pub mod doctor;
pub use aros_board::{config, deploy, scan, sd, sd_disk, sd_unmount};

use crate::build::{self, BuildOptions};
use config::{Board, Transport};
use console::ConsoleProgram;
use miette::Result;
use std::path::{Path, PathBuf};

pub fn initialize_template(
    config_override: Option<&Path>,
    board_name: &str,
    apply: bool,
) -> Result<()> {
    let template = config::prepare_template(config_override, board_name)?;
    aros_common::outputln!("🧭 AROS board profile template");
    aros_common::outputln!("  • File:  {}", template.path().display());
    aros_common::outputln!("  • Board: {}", template.board_name());
    if !apply {
        aros_common::outputln!("\n{}", template.contents());
        aros_common::outputln!(
            "Dry run: no file was created. Review the values, then rerun with `aros board init --board {board_name} --apply`."
        );
        return Ok(());
    }

    config::create_template(&template)?;
    aros_common::outputln!(
        "✅ Created '{}'. Replace every REPLACE_ME value before serving a board.",
        template.path().display()
    );
    Ok(())
}

/// Convert a selected local board profile into the immutable identity contract
/// required by an external SD boot bundle.
pub fn sd_bundle_expectation(board: &Board) -> Result<sd::BundleExpectation> {
    let manifest_board_name = if board.config.transport == Transport::UefiEsp {
        board.config.model.as_str()
    } else {
        &board.name
    };
    let expectation = sd::BundleExpectation::new(
        manifest_board_name,
        board.config.model.to_string(),
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
    aros_common::outputln!("💾 AROS board SD image plan");
    aros_common::outputln!(
        "  • Board:      {} ({})",
        board.name,
        board.config.transport
    );
    aros_common::outputln!("  • Bundle:     {}", bundle.source_dir().display());
    aros_common::outputln!(
        "  • Partition:  {} {} bytes @ LBA {}",
        bundle.partition().filesystem,
        bundle.partition().size_bytes,
        bundle.partition().start_lba
    );
    aros_common::outputln!("  • Files:      {}", bundle.files().len());
    aros_common::outputln!("  • Output:     {}", output_dir.display());
    if !apply {
        aros_common::outputln!(
            "  Dry run: the external bundle validated; no image was written. Pass --apply to create the artifact."
        );
        return Ok(());
    }

    let artifact = sd::stage_boot_bundle(&bundle, output_dir)?;
    aros_common::outputln!(
        "✅ Created verified SD artifact '{}'.",
        artifact.artifact_dir().display()
    );
    aros_common::outputln!("  • Image:      {}", artifact.image().path().display());
    aros_common::outputln!("  • SHA-256:    {}", artifact.image().sha256());
    aros_common::outputln!("  • Manifest:   {}", artifact.manifest_path().display());
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

    aros_common::outputln!("💾 AROS board safe SD-card scan");
    if let Some(artifact) = &artifact {
        aros_common::outputln!("  • Artifact:   {}", artifact.artifact_dir().display());
        aros_common::outputln!("  • Image:      {}", artifact.image_path().display());
        aros_common::outputln!("  • SHA-256:    {}", artifact.image_sha256());
    }
    if candidates.is_empty() {
        aros_common::outputln!("  No safe, unmounted removable whole-disk target was found.");
        aros_common::outputln!("  No disk was opened or changed.");
        return Ok(());
    }

    for candidate in &candidates {
        aros_common::outputln!("  • {}", candidate.summary());
        if let Some(artifact) = &artifact {
            aros_common::outputln!(
                "    Confirm token: {}",
                sd_disk::confirmation_token(artifact, candidate)
            );
        }
    }
    if artifact.is_none() {
        aros_common::outputln!(
            "  Pass --artifact <DIR> to verify an image and print its per-disk confirmation token."
        );
    }
    aros_common::outputln!("  No disk was opened or changed.");
    Ok(())
}

/// List mounted removable whole disks or explicitly unmount exactly one
/// current scan ID. Merely selecting a disk remains a non-mutating preview;
/// the platform unmount is reached only when `apply` is true.
pub fn unmount_sd_disk(selected_scan_id: Option<&str>, apply: bool, dry_run: bool) -> Result<()> {
    let candidates = sd_unmount::scan()?;

    aros_common::outputln!("💾 AROS board safe SD-card unmount");
    let Some(selected_scan_id) = selected_scan_id else {
        if candidates.is_empty() {
            aros_common::outputln!("  No mounted removable whole-disk target was found.");
        } else {
            for candidate in &candidates {
                aros_common::outputln!("  • {}", candidate.summary());
                for mount_point in candidate.mount_points() {
                    aros_common::outputln!("    Mounted at: {}", mount_point.display());
                }
            }
            aros_common::outputln!(
                "  Select one current scan ID with --device <SCAN_ID>; add --apply only when it should be unmounted."
            );
        }
        aros_common::outputln!("  No disk was opened, unmounted, or changed.");
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
                "No currently mounted removable whole disk has scan ID '{}'. Re-run `aros board sd unmount`; nothing was unmounted.",
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

    aros_common::outputln!("  • Target:     {}", candidate.summary());
    for mount_point in candidate.mount_points() {
        aros_common::outputln!("  • Mount:      {}", mount_point.display());
    }
    if !apply || dry_run {
        if dry_run {
            aros_common::outputln!(
                "  Dry run: the target was validated; nothing was unmounted or changed."
            );
        } else {
            aros_common::outputln!(
                "  Preview only: pass --apply with this --device selection to unmount it."
            );
        }
        return Ok(());
    }

    let report = sd_unmount::unmount(selected_scan_id)?;
    aros_common::outputln!("✅ Removable whole disk was unmounted.");
    aros_common::outputln!("  • Disk:       {}", report.scan_id);
    for mount_point in &report.unmounted_mount_points {
        aros_common::outputln!("  • Unmounted:  {}", mount_point.display());
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
    let plan = sd_disk::prepare_write_for_board(
        artifact_dir,
        Path::new(sd::ARTIFACT_MANIFEST),
        Path::new(sd::RAW_IMAGE_FILENAME),
        board,
        selected_scan_id,
    )?;
    let artifact = plan.artifact();
    let candidate = plan.candidate();
    let expected_token = plan.confirmation_token();

    aros_common::outputln!("💾 AROS board SD-card write plan");
    aros_common::outputln!(
        "  • Board:      {} ({})",
        board.name,
        board.config.transport
    );
    aros_common::outputln!("  • Artifact:   {}", artifact.artifact_dir().display());
    aros_common::outputln!("  • Image:      {}", artifact.image_path().display());
    aros_common::outputln!("  • SHA-256:    {}", artifact.image_sha256());
    aros_common::outputln!("  • Target:     {}", candidate.summary());
    aros_common::outputln!("  • Token:      {expected_token}");

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
            aros_common::outputln!(
                "  Preview only: pass the token above as --confirm to authorize this one write."
            );
        } else {
            aros_common::outputln!(
                "  Dry run: the token and target validated; no disk was opened or changed."
            );
        }
        return Ok(());
    }

    let confirmation = confirmation.ok_or_else(|| {
        miette::miette!(
            "internal media-safety invariant failed: an applied write has no confirmation token"
        )
    })?;
    let report =
        sd_disk::write_verified_image_for_board(artifact, board, selected_scan_id, confirmation)?;
    aros_common::outputln!("✅ Verified SD image write completed.");
    aros_common::outputln!("  • Disk:       {}", report.scan_id);
    aros_common::outputln!("  • Bytes:      {}", report.bytes_written);
    aros_common::outputln!("  • Readback:   {}", report.readback_sha256);
    Ok(())
}

/// Find USB CDC-ECM adapters without changing any network configuration.
pub fn scan() -> Result<()> {
    let adapters = scan::adapters()?;
    if adapters.is_empty() {
        aros_common::outputln!("No USB CDC-ECM adapters found.");
        aros_common::outputln!(
            "Connect and boot the board's USB-ECM profile, then run `aros board scan` again."
        );
        return Ok(());
    }
    aros_common::output!("{}", scan::format_adapters(&adapters));
    Ok(())
}

struct CliEventSink;

impl aros_board::EventSink for CliEventSink {
    fn event(
        &self,
        level: aros_common::LogLevel,
        event: &str,
        message: &str,
        context: &aros_common::DiagnosticContext,
    ) -> Result<()> {
        crate::observability::log_event(level, event, message, context)?;
        if event.starts_with("board.tftp.") || event == "board.dhcp.response_failed" {
            aros_common::outputln!(
                "  {}: {message}",
                context.tool.as_deref().unwrap_or("board")
            );
        }
        Ok(())
    }
}

pub async fn serve(board: &Board, dry_run: bool) -> Result<()> {
    let plan = aros_board::serve::resolve(board)?;
    aros_common::outputln!("🧭 AROS board service plan");
    aros_common::outputln!("  • Board:     {} ({})", plan.board_name, plan.transport);
    aros_common::outputln!("  • Interface: {}", plan.interface);
    aros_common::outputln!(
        "  • Address:   {} / {}",
        plan.server_address,
        plan.subnet_mask
    );
    aros_common::outputln!(
        "  • Board lease: {} for {}",
        plan.target_address,
        format_mac(plan.expected_target_mac)
    );
    aros_common::outputln!("  • TFTP root: {}", plan.tftp_root.display());
    if dry_run {
        aros_common::outputln!("  Dry run: no DHCP or TFTP socket was opened.");
        return Ok(());
    }

    aros_common::outputln!("\n▶ Starting restricted board service. Press Ctrl-C to stop it.");
    aros_common::outputln!(
        "  DHCP: {}:67 → {} ({})",
        plan.server_address,
        plan.target_address,
        format_mac(plan.expected_target_mac)
    );
    aros_common::outputln!(
        "  TFTP: {}:69 (root {})",
        plan.server_address,
        plan.tftp_root.display()
    );
    let result = aros_board::serve::run(&plan, &CliEventSink).await;
    aros_common::outputln!("\nStopped board service.");
    result
}

fn format_mac(mac: [u8; 6]) -> String {
    mac.iter()
        .map(|octet| format!("{octet:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn doctor(board: &Board, repo_root: &Path) -> Result<()> {
    aros_common::outputln!("🩺 Checking AROS board profile '{}'...", board.name);
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
    aros_common::outputln!(
        "🧭 Building board '{}' ({}, transport {})...",
        board.name,
        board.config.model,
        board.config.transport
    );
    if let Some(dtb_path) = board.raspberry_pi_dtb_path(repo_root, dtb_override)? {
        options.cmake_definitions.push(build::CmakeDefinition {
            key: "AROS_RPI_DTB".to_string(),
            value: dtb_path.to_string_lossy().into_owned(),
        });
    }
    if let Some(core_kobj_dir) = board.raspberry_pi_core_kobj_dir(repo_root, core_kobj_override)? {
        options.cmake_definitions.push(build::CmakeDefinition {
            key: "AROS_RPI_CORE_KOBJ_DIR".to_string(),
            value: core_kobj_dir.to_string_lossy().into_owned(),
        });
    }
    if let Some(core_kobj_dir) = board.opensbi_core_kobj_dir(repo_root, core_kobj_override)? {
        options.cmake_definitions.push(build::CmakeDefinition {
            key: "AROS_OPENSBI_CORE_KOBJ_DIR".to_string(),
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
    let mode = if apply { "APPLY" } else { "DRY RUN" };
    aros_common::outputln!("📦 AROS board deployment ({mode})");
    aros_common::outputln!("  • Board:       {}", plan.board_name);
    aros_common::outputln!("  • Source:      {}", plan.source_dir.display());
    aros_common::outputln!("  • Destination: {}", plan.destination_dir.display());
    aros_common::outputln!(
        "  • Files:       {} ({})",
        plan.files.len(),
        format_bytes(plan.total_bytes())
    );
    for file in &plan.files {
        aros_common::outputln!(
            "    - {} ({})",
            file.relative_path.display(),
            format_bytes(file.bytes)
        );
    }
    if !apply {
        aros_common::outputln!("  No files were changed. Pass --apply to publish this bundle.");
        return Ok(());
    }

    deploy::publish(&plan)?;
    aros_common::outputln!(
        "✅ Published '{}' into the local TFTP tree at {}.",
        board.name,
        plan.destination_dir.display()
    );
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * KIB;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
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
    aros_common::outputln!("🖥️  Serial terminal: {}", plan.display());
    if dry_run {
        aros_common::outputln!("  Dry run: serial terminal was not started.");
        return Ok(());
    }
    console::run(&plan)
}
