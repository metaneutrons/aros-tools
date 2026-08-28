//! Verified staging for board SD boot bundles.
//!
//! This module creates a verified staging artifact containing both a `boot/`
//! payload and a raw MBR/FAT32 image.  It never writes a raw device.  In
//! particular, it must never manufacture a plausible `u-boot.bin` or
//! `boot.scr` when the external U-Boot bundle is incomplete.
//!
//! # `boot-bundle.toml` format version 1
//!
//! ```toml
//! format_version = 1
//!
//! [board]
//! name = "rpi4-lab"
//! model = "rpi4"
//! transport = "uboot-usb-ecm"
//!
//! [usb_ecm]
//! vendor_id = 0x1d6b
//! product_id = 0x0104
//! serial = "aros-rpi4-lab-01"
//! expected_target_mac = "02:aa:00:00:00:01"
//!
//! [partition]
//! scheme = "mbr"
//! filesystem = "fat32"
//! start_lba = 2048
//! size_bytes = 67108864
//! label = "AROSBOOT"
//! layout_sha256 = "..."
//!
//! [[files]]
//! role = "config"
//! source = "config.txt"
//! destination = "config.txt"
//! sha256 = "..."
//! ```
//!
//! Every entry in `files` is copied below `boot/`.  The current Pi 4
//! `uboot-usb-ecm` profile additionally requires exactly one entry for each
//! of `config`, `firmware-start`, `firmware-fixup`, `device-tree`, `u-boot`,
//! and `boot-script`, with the firmware filenames expected by the Pi 4 boot
//! ROM.  `partition.layout_sha256` is the SHA-256 of the exact canonical
//! layout representation emitted by [`PartitionLayout::layout_sha256`].

use super::sd_manifest::{
    ImageManifest, ManifestBoard as ArtifactBoard, ManifestImage,
    ManifestPartition as ArtifactPartition, ManifestPayloadFile, ManifestSource,
    ManifestUsbEcmIdentity,
};
use crate::sha256_file_with_size as sha256_file;
use miette::Result;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Name of the manifest expected at the root of an external boot bundle.
pub const BOOT_BUNDLE_MANIFEST: &str = "boot-bundle.toml";

/// Name of the deterministic manifest emitted into a staged artifact.
pub const ARTIFACT_MANIFEST: &str = super::sd_manifest::FILE_NAME;

/// Name of the checksum file emitted into a staged artifact.
pub const ARTIFACT_CHECKSUMS: &str = "SHA256SUMS";

/// Directory below a staged artifact containing files for the FAT boot volume.
pub const BOOT_PAYLOAD_DIRECTORY: &str = "boot";

/// Stable raw-image filename inside a staged SD artifact.
pub const RAW_IMAGE_FILENAME: &str = "aros-board-boot.img";

/// Transport string used by the Pi 4 USB-C CDC-ECM bootstrap.
pub const UBOOT_USB_ECM_TRANSPORT: &str = "uboot-usb-ecm";
/// Transport string used by OpenSBI boards booting from a UEFI ESP.
pub const UEFI_ESP_TRANSPORT: &str = "uefi-esp";

const FORMAT_VERSION: u32 = 1;
const SHA256_HEX_LENGTH: usize = 64;
const SECTOR_BYTES: u64 = 512;
const MIB: u64 = 1024 * 1024;

const REQUIRED_RPI4_UBOOT_FILES: [(&str, &str); 6] = [
    ("config", "config.txt"),
    ("firmware-start", "start4.elf"),
    ("firmware-fixup", "fixup4.dat"),
    ("device-tree", "bcm2711-rpi-4-b.dtb"),
    ("u-boot", "u-boot.bin"),
    ("boot-script", "boot.scr"),
];

const REQUIRED_UEFI_ESP_FILES: [(&str, &str); 5] = [
    ("uefi-loader", "EFI/BOOT/BOOTRISCV64.EFI"),
    ("kernel-image", "EFI/AROS/Image"),
    ("bsp-package", "aros-bsp.pkg"),
    ("command-line", "aros.cmd"),
    ("startup-script", "startup.nsh"),
];

/// Stable USB-ECM identity which binds a boot bundle to one physical Pi.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbEcmIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub expected_target_mac: String,
}

/// Board values against which an external bundle is checked before staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleExpectation {
    pub board_name: String,
    pub model: String,
    pub transport: String,
    pub usb_ecm_identity: Option<UsbEcmIdentity>,
}

impl BundleExpectation {
    /// Build an expectation from the selected local board profile.
    #[must_use]
    pub fn new(
        board_name: impl Into<String>,
        model: impl Into<String>,
        transport: impl Into<String>,
    ) -> Self {
        Self {
            board_name: board_name.into(),
            model: model.into(),
            transport: transport.into(),
            usb_ecm_identity: None,
        }
    }

    /// Require a bundle to carry this exact USB-ECM descriptor identity.
    #[must_use]
    pub fn with_usb_ecm_identity(mut self, identity: UsbEcmIdentity) -> Self {
        self.usb_ecm_identity = Some(identity);
        self
    }
}

/// The disk layout declared by a boot bundle.
///
/// Version 1 represents a single FAT32 boot partition in an MBR image. The
/// declaration is validated before the module creates and readbacks the raw
/// image under an ordinary artifact directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionLayout {
    pub scheme: String,
    pub filesystem: String,
    pub start_lba: u64,
    pub size_bytes: u64,
    pub label: String,
}

impl PartitionLayout {
    /// Return the checksum that must be written to `partition.layout_sha256`.
    #[must_use]
    pub fn layout_sha256(&self) -> String {
        sha256_hex(self.canonical_representation().as_bytes())
    }

    fn canonical_representation(&self) -> String {
        format!(
            "aros-board-sd-partition-v1\nscheme={}\nfilesystem={}\nstart_lba={}\nsize_bytes={}\nlabel={}\n",
            self.scheme, self.filesystem, self.start_lba, self.size_bytes, self.label
        )
    }
}

/// One verified file from the external bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBootFile {
    pub role: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

/// A complete, verified raw MBR/FAT32 image inside a staged artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImage {
    path: PathBuf,
    sha256: String,
    size_bytes: u64,
}

impl RawImage {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// A boot bundle that has passed all content, identity and path checks.
#[derive(Debug, Clone)]
pub struct ValidatedBootBundle {
    source_dir: PathBuf,
    manifest_path: PathBuf,
    source_manifest_sha256: String,
    board_name: String,
    model: String,
    transport: String,
    usb_ecm_identity: Option<UsbEcmIdentity>,
    partition: PartitionLayout,
    files: Vec<VerifiedBootFile>,
}

impl ValidatedBootBundle {
    #[must_use]
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    #[must_use]
    pub const fn partition(&self) -> &PartitionLayout {
        &self.partition
    }

    #[must_use]
    pub fn files(&self) -> &[VerifiedBootFile] {
        &self.files
    }
}

/// Paths and checksums created by [`stage_boot_bundle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedArtifact {
    artifact_dir: PathBuf,
    image: RawImage,
    manifest_path: PathBuf,
}

impl StagedArtifact {
    #[must_use]
    pub fn artifact_dir(&self) -> &Path {
        &self.artifact_dir
    }

    #[must_use]
    pub const fn image(&self) -> &RawImage {
        &self.image
    }

    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }
}

/// Validate a complete external `boot-bundle.toml` directory without writing.
///
/// The result contains canonical source paths and immutable expected hashes.
/// [`stage_boot_bundle`] verifies those hashes again while it copies, so a
/// changed source cannot silently pass through a prior validation result.
///
/// # Errors
///
/// Returns an error when the bundle, manifest, declared files, hashes, board
/// identity, or partition constraints are invalid or cannot be read safely.
pub fn validate_boot_bundle(
    bundle_dir: &Path,
    expectation: &BundleExpectation,
) -> Result<ValidatedBootBundle> {
    validate_expectation(expectation)?;
    let source_dir = canonical_existing_directory(bundle_dir, "boot bundle directory")?;
    let manifest_path = source_dir.join(BOOT_BUNDLE_MANIFEST);
    let manifest_bytes = read_bundle_manifest(&manifest_path, &source_dir, expectation)?;
    let source_manifest_sha256 = sha256_hex(&manifest_bytes);
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| {
        miette::miette!(
            "Boot bundle manifest '{}' is not valid UTF-8: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: BootBundleManifest = toml::from_str(manifest_text).map_err(|error| {
        miette::miette!(
            "Could not parse boot bundle manifest '{}': {error}",
            manifest_path.display()
        )
    })?;

    validate_manifest_header(&manifest, expectation)?;
    let partition = validate_partition(&manifest.partition)?;
    let declared_files = validate_declared_files(&manifest.files)?;
    validate_required_profile_files(&source_dir, &manifest.board, &declared_files)?;
    let files = validate_payload_files(&source_dir, &declared_files)?;
    validate_partition_capacity(&partition, &files)?;

    Ok(ValidatedBootBundle {
        source_dir,
        manifest_path,
        source_manifest_sha256,
        board_name: manifest.board.name,
        model: manifest.board.model,
        transport: manifest.board.transport,
        usb_ecm_identity: manifest
            .usb_ecm
            .as_ref()
            .map(|identity| normalized_identity(identity, "usb_ecm"))
            .transpose()?,
        partition,
        files,
    })
}

/// Create an atomic, verified staging artifact at a previously absent path.
///
/// The temporary directory is created beside `output_dir` so the final rename
/// is atomic on the target filesystem.  No source file is modified and an
/// existing output directory is never replaced.
///
/// # Errors
///
/// Returns an error when the validated source changed, a destination path is
/// unsafe or already exists, or the staged files cannot be copied and verified.
pub fn stage_boot_bundle(
    bundle: &ValidatedBootBundle,
    output_dir: &Path,
) -> Result<StagedArtifact> {
    verify_validated_manifest(bundle)?;
    let destination = resolve_new_output_path(output_dir)?;
    let parent = destination.parent().ok_or_else(|| {
        miette::miette!(
            "Output artifact '{}' has no parent directory.",
            destination.display()
        )
    })?;
    let stage = tempfile::Builder::new()
        .prefix(".aros-board-sd-stage-")
        .tempdir_in(parent)
        .map_err(|error| {
            miette::miette!(
                "Could not create an atomic staging directory beside '{}': {error}",
                destination.display()
            )
        })?;
    let stage_path = stage.path();
    let payload_dir = stage_path.join(BOOT_PAYLOAD_DIRECTORY);
    fs::create_dir(&payload_dir).map_err(|error| {
        miette::miette!(
            "Could not create staged boot payload directory '{}': {error}",
            payload_dir.display()
        )
    })?;

    for file in &bundle.files {
        let destination_file = payload_dir.join(&file.destination);
        let destination_parent = destination_file.parent().ok_or_else(|| {
            miette::miette!(
                "Could not determine the staging parent for '{}'.",
                destination_file.display()
            )
        })?;
        fs::create_dir_all(destination_parent).map_err(|error| {
            miette::miette!(
                "Could not create staged directory '{}': {error}",
                destination_parent.display()
            )
        })?;
        let (actual_sha256, _) = copy_and_hash(&file.source, &destination_file)?;
        if actual_sha256 != file.sha256 {
            miette::bail!(
                "Boot bundle input '{}' changed after validation: expected SHA-256 {}, got {}.",
                file.source.display(),
                file.sha256,
                actual_sha256
            );
        }
    }

    let staged_image = build_raw_image(bundle, stage_path)?;
    let artifact_manifest = render_artifact_manifest(bundle, &staged_image)?;
    let manifest_path = stage_path.join(ARTIFACT_MANIFEST);
    write_new_file(&manifest_path, artifact_manifest.as_bytes())?;
    let manifest_sha256 = sha256_hex(artifact_manifest.as_bytes());

    let checksums = render_checksums(bundle, &staged_image, &manifest_sha256);
    let checksums_path = stage_path.join(ARTIFACT_CHECKSUMS);
    write_new_file(&checksums_path, checksums.as_bytes())?;

    ensure_path_absent(&destination, "output artifact")?;
    fs::rename(stage_path, &destination).map_err(|error| {
        miette::miette!(
            "Could not atomically publish staged SD artifact '{}' to '{}': {error}",
            stage_path.display(),
            destination.display()
        )
    })?;

    Ok(StagedArtifact {
        artifact_dir: destination.clone(),
        image: RawImage {
            path: destination.join(RAW_IMAGE_FILENAME),
            sha256: staged_image.sha256,
            size_bytes: staged_image.size_bytes,
        },
        manifest_path: destination.join(ARTIFACT_MANIFEST),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootBundleManifest {
    format_version: u32,
    board: ManifestBoard,
    #[serde(default)]
    usb_ecm: Option<UsbEcmIdentity>,
    partition: ManifestPartition,
    #[serde(default)]
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestBoard {
    name: String,
    model: String,
    transport: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPartition {
    scheme: String,
    filesystem: String,
    start_lba: u64,
    size_bytes: u64,
    label: String,
    layout_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    role: String,
    source: String,
    destination: String,
    sha256: String,
}

#[derive(Debug)]
struct DeclaredFile {
    role: String,
    source: PathBuf,
    destination: PathBuf,
    sha256: String,
}

fn validate_expectation(expectation: &BundleExpectation) -> Result<()> {
    validate_nonempty(&expectation.board_name, "expected board name")?;
    validate_nonempty(&expectation.model, "expected board model")?;
    validate_nonempty(&expectation.transport, "expected board transport")?;
    if let Some(identity) = &expectation.usb_ecm_identity {
        normalized_identity(identity, "expected USB-ECM identity")?;
    }
    if expectation.transport == UBOOT_USB_ECM_TRANSPORT && expectation.usb_ecm_identity.is_none() {
        miette::bail!(
            "The selected '{}' board profile needs a complete USB-ECM identity before an SD bundle can be staged.",
            UBOOT_USB_ECM_TRANSPORT
        );
    }
    Ok(())
}

fn read_bundle_manifest(
    manifest_path: &Path,
    source_dir: &Path,
    expectation: &BundleExpectation,
) -> Result<Vec<u8>> {
    let metadata = match fs::symlink_metadata(manifest_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return missing_bundle_manifest(source_dir, expectation);
        }
        Err(error) => {
            return Err(miette::miette!(
                "Could not inspect boot bundle manifest '{}': {error}",
                manifest_path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        miette::bail!(
            "Boot bundle manifest '{}' must be a regular file, not a symbolic link or special file.",
            manifest_path.display()
        );
    }
    fs::read(manifest_path).map_err(|error| {
        miette::miette!(
            "Could not read boot bundle manifest '{}': {error}",
            manifest_path.display()
        )
    })
}

fn missing_bundle_manifest<T>(source_dir: &Path, expectation: &BundleExpectation) -> Result<T> {
    let mut inputs = vec![format!("{BOOT_BUNDLE_MANIFEST} (versioned manifest)")];
    if expectation.transport == UBOOT_USB_ECM_TRANSPORT {
        inputs.extend(
            REQUIRED_RPI4_UBOOT_FILES.iter().map(|(_, destination)| {
                format!("{destination} (Pi 4 U-Boot USB-ECM boot payload)")
            }),
        );
    }
    if expectation.transport == UEFI_ESP_TRANSPORT {
        inputs.extend(
            REQUIRED_UEFI_ESP_FILES
                .iter()
                .map(|(_, destination)| format!("{destination} (OpenSBI/UEFI boot payload)")),
        );
    }
    missing_inputs(source_dir, &inputs)
}

fn validate_manifest_header(
    manifest: &BootBundleManifest,
    expectation: &BundleExpectation,
) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION {
        miette::bail!(
            "Boot bundle manifest has format_version = {}, but this aros version supports format_version = {}.",
            manifest.format_version,
            FORMAT_VERSION
        );
    }
    validate_nonempty(&manifest.board.name, "manifest board.name")?;
    validate_nonempty(&manifest.board.model, "manifest board.model")?;
    validate_nonempty(&manifest.board.transport, "manifest board.transport")?;
    if manifest.board.name != expectation.board_name {
        miette::bail!(
            "Boot bundle board.name '{}' does not match selected board '{}'.",
            manifest.board.name,
            expectation.board_name
        );
    }
    if manifest.board.model != expectation.model {
        miette::bail!(
            "Boot bundle board.model '{}' does not match selected model '{}'.",
            manifest.board.model,
            expectation.model
        );
    }
    if manifest.board.transport != expectation.transport {
        miette::bail!(
            "Boot bundle board.transport '{}' does not match selected transport '{}'.",
            manifest.board.transport,
            expectation.transport
        );
    }

    match (&expectation.usb_ecm_identity, &manifest.usb_ecm) {
        (Some(expected), Some(actual)) => {
            let expected = normalized_identity(expected, "expected USB-ECM identity")?;
            let actual = normalized_identity(actual, "manifest usb_ecm")?;
            if actual != expected {
                miette::bail!(
                    "Boot bundle USB-ECM identity does not match the selected board profile. Refusing to stage media for a different Pi."
                );
            }
        }
        (Some(_), None) => {
            miette::bail!(
                "Boot bundle is missing [usb_ecm], required to match the selected board's USB-ECM identity."
            );
        }
        (None, Some(_)) => {
            miette::bail!(
                "Boot bundle declares [usb_ecm], but the selected board profile has no matching USB-ECM identity."
            );
        }
        (None, None) => {}
    }
    Ok(())
}

fn validate_partition(partition: &ManifestPartition) -> Result<PartitionLayout> {
    let layout = PartitionLayout {
        scheme: partition.scheme.clone(),
        filesystem: partition.filesystem.clone(),
        start_lba: partition.start_lba,
        size_bytes: partition.size_bytes,
        label: partition.label.clone(),
    };
    if layout.scheme != "mbr" {
        miette::bail!(
            "Unsupported partition.scheme '{}'; boot-bundle format 1 requires 'mbr'.",
            layout.scheme
        );
    }
    if layout.filesystem != "fat32" {
        miette::bail!(
            "Unsupported partition.filesystem '{}'; boot-bundle format 1 requires 'fat32'.",
            layout.filesystem
        );
    }
    if layout.start_lba < 2048 || !layout.start_lba.is_multiple_of(2048) {
        miette::bail!(
            "partition.start_lba {} is invalid: use a 1 MiB-aligned LBA at or after 2048.",
            layout.start_lba
        );
    }
    if layout.size_bytes < 64 * MIB || !layout.size_bytes.is_multiple_of(SECTOR_BYTES) {
        miette::bail!(
            "partition.size_bytes {} is invalid: use a multiple of {} bytes of at least {} bytes.",
            layout.size_bytes,
            SECTOR_BYTES,
            64 * MIB
        );
    }
    validate_fat_label(&layout.label)?;
    let expected_checksum = normalize_sha256(&partition.layout_sha256, "partition.layout_sha256")?;
    let actual_checksum = layout.layout_sha256();
    if expected_checksum != actual_checksum {
        miette::bail!(
            "partition.layout_sha256 does not match the declared layout: expected {}, got {}.",
            expected_checksum,
            actual_checksum
        );
    }
    Ok(layout)
}

fn validate_declared_files(files: &[ManifestFile]) -> Result<Vec<DeclaredFile>> {
    if files.is_empty() {
        miette::bail!("Boot bundle manifest has no [[files]] entries.");
    }
    let mut roles = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    let mut declared = Vec::with_capacity(files.len());
    for file in files {
        validate_role(&file.role)?;
        if !roles.insert(file.role.clone()) {
            miette::bail!("Boot bundle declares duplicate file role '{}'.", file.role);
        }
        let source = validate_relative_path(&file.source, "files.source")?;
        let destination = validate_relative_path(&file.destination, "files.destination")?;
        let destination_key = portable_path(&destination).to_lowercase();
        if !destinations.insert(destination_key) {
            miette::bail!(
                "Boot bundle declares duplicate FAT destination '{}'.",
                file.destination
            );
        }
        declared.push(DeclaredFile {
            role: file.role.clone(),
            source,
            destination,
            sha256: normalize_sha256(&file.sha256, "files.sha256")?,
        });
    }
    declared.sort_by(|left, right| left.destination.cmp(&right.destination));
    Ok(declared)
}

fn validate_required_profile_files(
    source_dir: &Path,
    board: &ManifestBoard,
    files: &[DeclaredFile],
) -> Result<()> {
    let (required, profile_label) = if board.transport == UBOOT_USB_ECM_TRANSPORT {
        if board.model != "rpi4" {
            miette::bail!(
                "boot-bundle format 1 defines '{}' only for model 'rpi4'.",
                UBOOT_USB_ECM_TRANSPORT
            );
        }
        (&REQUIRED_RPI4_UBOOT_FILES[..], "Pi 4 U-Boot USB-ECM")
    } else if board.transport == UEFI_ESP_TRANSPORT {
        if board.model != "milk-v-titan" {
            miette::bail!(
                "boot-bundle format 1 defines '{}' only for model 'milk-v-titan'.",
                UEFI_ESP_TRANSPORT
            );
        }
        (&REQUIRED_UEFI_ESP_FILES[..], "OpenSBI/UEFI")
    } else {
        return Ok(());
    };
    let by_role: BTreeMap<&str, &DeclaredFile> = files
        .iter()
        .map(|file| (file.role.as_str(), file))
        .collect();
    let mut missing = Vec::new();
    for (role, destination) in required {
        match by_role.get(role) {
            Some(file) if portable_path(&file.destination) == *destination => {}
            Some(file) => {
                miette::bail!(
                    "{} role '{}' must stage as '{}', not '{}'.",
                    profile_label,
                    role,
                    destination,
                    portable_path(&file.destination)
                );
            }
            None => missing.push(format!("{destination} (required files role '{role}')")),
        }
    }
    if !missing.is_empty() {
        return missing_inputs(source_dir, &missing);
    }
    Ok(())
}

fn validate_payload_files(
    source_dir: &Path,
    files: &[DeclaredFile],
) -> Result<Vec<VerifiedBootFile>> {
    let mut sources = Vec::with_capacity(files.len());
    let mut missing = Vec::new();
    for file in files {
        let source = source_dir.join(&file.source);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(format!(
                    "{} (files role '{}')",
                    file.source.display(),
                    file.role
                ));
                continue;
            }
            Err(error) => {
                return Err(miette::miette!(
                    "Could not inspect boot bundle input '{}': {error}",
                    source.display()
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            miette::bail!(
                "Boot bundle input '{}' must be a regular file, not a symbolic link or special file.",
                source.display()
            );
        }
        ensure_no_symlink_components(source_dir, &file.source)?;
        let canonical_source = source.canonicalize().map_err(|error| {
            miette::miette!(
                "Could not resolve boot bundle input '{}': {error}",
                source.display()
            )
        })?;
        if !canonical_source.starts_with(source_dir) {
            miette::bail!(
                "Boot bundle input '{}' resolves outside bundle directory '{}'.",
                source.display(),
                source_dir.display()
            );
        }
        sources.push((file, canonical_source));
    }
    if !missing.is_empty() {
        return missing_inputs(source_dir, &missing);
    }

    let mut verified = Vec::with_capacity(files.len());
    for (file, source) in sources {
        let (actual_sha256, size_bytes) = sha256_file(&source)?;
        if actual_sha256 != file.sha256 {
            miette::bail!(
                "SHA-256 mismatch for boot bundle input '{}': expected {}, got {}.",
                source.display(),
                file.sha256,
                actual_sha256
            );
        }
        verified.push(VerifiedBootFile {
            role: file.role.clone(),
            source,
            destination: file.destination.clone(),
            sha256: file.sha256.clone(),
            size_bytes,
        });
    }
    Ok(verified)
}

fn validate_partition_capacity(
    partition: &PartitionLayout,
    files: &[VerifiedBootFile],
) -> Result<()> {
    let payload_bytes = files.iter().try_fold(0_u64, |total, file| {
        total.checked_add(file.size_bytes).ok_or_else(|| {
            miette::miette!(
                "Boot payload size overflow while accounting for '{}'.",
                file.source.display()
            )
        })
    })?;
    if payload_bytes > partition.size_bytes {
        miette::bail!(
            "Boot payload is {} bytes, larger than declared FAT32 partition size {} bytes.",
            payload_bytes,
            partition.size_bytes
        );
    }
    Ok(())
}

fn verify_validated_manifest(bundle: &ValidatedBootBundle) -> Result<()> {
    let (actual_sha256, _) = sha256_file(&bundle.manifest_path)?;
    if actual_sha256 != bundle.source_manifest_sha256 {
        miette::bail!(
            "Boot bundle manifest '{}' changed after validation: expected SHA-256 {}, got {}.",
            bundle.manifest_path.display(),
            bundle.source_manifest_sha256,
            actual_sha256
        );
    }
    Ok(())
}

fn resolve_new_output_path(output_dir: &Path) -> Result<PathBuf> {
    if output_dir.as_os_str().is_empty() {
        miette::bail!("SD artifact output path is empty.");
    }
    ensure_path_absent(output_dir, "output artifact")?;
    let output_name = output_dir.file_name().ok_or_else(|| {
        miette::miette!(
            "SD artifact output '{}' must have a final directory name.",
            output_dir.display()
        )
    })?;
    let raw_parent = output_dir.parent().unwrap_or_else(|| Path::new("."));
    let parent = canonical_existing_directory(raw_parent, "SD artifact output parent")?;
    if parent.starts_with("/dev") {
        miette::bail!(
            "Refusing SD artifact output beneath '{}': use a normal filesystem directory, never /dev.",
            parent.display()
        );
    }
    let destination = parent.join(output_name);
    ensure_path_absent(&destination, "output artifact")?;
    Ok(destination)
}

fn ensure_path_absent(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => miette::bail!(
            "Refusing to replace existing {label} '{}'. Choose a new output directory.",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(miette::miette!(
            "Could not inspect {label} '{}': {error}",
            path.display()
        )),
    }
}

fn canonical_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = fs::metadata(path).map_err(|error| {
        miette::miette!("Could not access {label} '{}': {error}", path.display())
    })?;
    if !metadata.is_dir() {
        miette::bail!("{label} '{}' is not a directory.", path.display());
    }
    path.canonicalize()
        .map_err(|error| miette::miette!("Could not resolve {label} '{}': {error}", path.display()))
}

fn validate_nonempty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value != value.trim() {
        miette::bail!("{label} must be non-empty and have no surrounding whitespace.");
    }
    Ok(())
}

fn normalized_identity(identity: &UsbEcmIdentity, label: &str) -> Result<UsbEcmIdentity> {
    if identity.vendor_id == 0 || identity.product_id == 0 {
        miette::bail!("{label} must contain non-zero USB vendor_id and product_id.");
    }
    validate_nonempty(&identity.serial, &format!("{label}.serial"))?;
    Ok(UsbEcmIdentity {
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        serial: identity.serial.clone(),
        expected_target_mac: normalize_unicast_mac(&identity.expected_target_mac, label)?,
    })
}

fn normalize_unicast_mac(mac: &str, label: &str) -> Result<String> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
        miette::bail!(
            "{label}.expected_target_mac '{mac}' is not a six-octet colon-separated MAC address."
        );
    }
    let mut bytes = [0_u8; 6];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = u8::from_str_radix(part, 16).map_err(|error| {
            miette::miette!(
                "{label}.expected_target_mac '{mac}' has an invalid octet '{part}': {error}"
            )
        })?;
    }
    if bytes.iter().all(|byte| *byte == 0) || bytes[0] & 1 != 0 {
        miette::bail!(
            "{label}.expected_target_mac '{mac}' must be a non-zero unicast MAC address."
        );
    }
    let mut normalized = String::with_capacity(17);
    for (index, byte) in bytes.iter().enumerate() {
        if index != 0 {
            normalized.push(':');
        }
        append_hex_byte(&mut normalized, *byte);
    }
    Ok(normalized)
}

fn validate_fat_label(label: &str) -> Result<()> {
    if label.is_empty()
        || label.len() > 11
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        miette::bail!(
            "partition.label '{label}' must contain 1–11 ASCII letters, digits or underscores for a FAT32 volume."
        );
    }
    Ok(())
}

fn validate_role(role: &str) -> Result<()> {
    if role.is_empty()
        || !role
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        miette::bail!(
            "files.role '{role}' must contain lowercase ASCII letters, digits or hyphens."
        );
    }
    Ok(())
}

fn validate_relative_path(raw_path: &str, label: &str) -> Result<PathBuf> {
    if raw_path.is_empty() || raw_path.contains('\\') || raw_path.chars().any(char::is_control) {
        miette::bail!("{label} '{raw_path}' is not a safe portable relative path.");
    }
    let path = Path::new(raw_path);
    let mut normal_components = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => normal_components += 1,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                miette::bail!("{label} '{raw_path}' must be a relative path without '.' or '..'.");
            }
        }
    }
    if normal_components == 0 {
        miette::bail!("{label} '{raw_path}' is not a usable relative path.");
    }
    Ok(path.to_path_buf())
}

fn normalize_sha256(value: &str, label: &str) -> Result<String> {
    aros_common::Sha256Digest::parse(value)
        .map(|digest| digest.to_string())
        .map_err(|_| {
            miette::miette!("{label} must be a {SHA256_HEX_LENGTH}-character SHA-256 hex digest.")
        })
}

fn ensure_no_symlink_components(root: &Path, relative_path: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(part) = component else {
            miette::bail!(
                "Internal error: '{}' was not validated as a normal relative path.",
                relative_path.display()
            );
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            miette::miette!(
                "Could not inspect boot bundle path '{}': {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            miette::bail!(
                "Boot bundle path '{}' contains a symbolic link. Bundle inputs must be self-contained regular files.",
                current.display()
            );
        }
    }
    Ok(())
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<(String, u64)> {
    let mut input = File::open(source).map_err(|error| {
        miette::miette!(
            "Could not open verified boot input '{}' for staging: {error}",
            source.display()
        )
    })?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            miette::miette!(
                "Could not create staged boot file '{}': {error}",
                destination.display()
            )
        })?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            miette::miette!(
                "Could not read verified boot input '{}': {error}",
                source.display()
            )
        })?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            miette::miette!(
                "Could not write staged boot file '{}': {error}",
                destination.display()
            )
        })?;
        hasher.update(&buffer[..read]);
        bytes = bytes
            .checked_add(u64::try_from(read).map_err(|error| {
                miette::miette!(
                    "Could not account for '{}' while staging: {error}",
                    source.display()
                )
            })?)
            .ok_or_else(|| miette::miette!("File '{}' is too large to stage.", source.display()))?;
    }
    output.sync_all().map_err(|error| {
        miette::miette!(
            "Could not flush staged boot file '{}': {error}",
            destination.display()
        )
    })?;
    Ok((aros_common::finish_sha256(hasher).to_string(), bytes))
}

fn write_new_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| miette::miette!("Could not create '{}': {error}", path.display()))?;
    file.write_all(contents)
        .map_err(|error| miette::miette!("Could not write '{}': {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| miette::miette!("Could not flush '{}': {error}", path.display()))
}

fn build_raw_image(bundle: &ValidatedBootBundle, artifact_dir: &Path) -> Result<RawImage> {
    let geometry = image_geometry(&bundle.partition)?;
    let image_path = artifact_dir.join(RAW_IMAGE_FILENAME);
    ensure_path_absent(&image_path, "staged raw SD image")?;

    {
        let mut image = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&image_path)
            .map_err(|error| {
                miette::miette!(
                    "Could not create staged raw SD image '{}': {error}",
                    image_path.display()
                )
            })?;
        image.set_len(geometry.image_size_bytes).map_err(|error| {
            miette::miette!(
                "Could not size staged raw SD image '{}' to {} bytes: {error}",
                image_path.display(),
                geometry.image_size_bytes
            )
        })?;
        write_mbr(&mut image, &geometry)?;
        image.sync_all().map_err(|error| {
            miette::miette!(
                "Could not flush MBR for staged raw SD image '{}': {error}",
                image_path.display()
            )
        })?;
    }

    write_fat32_payload(bundle, &image_path, &geometry)?;
    sync_image_file(&image_path)?;
    verify_raw_image(bundle, &image_path, &geometry)?;
    sync_image_file(&image_path)?;
    let (sha256, size_bytes) = sha256_file(&image_path)?;
    if size_bytes != geometry.image_size_bytes {
        miette::bail!(
            "Staged raw SD image '{}' has {} bytes, expected {} bytes.",
            image_path.display(),
            size_bytes,
            geometry.image_size_bytes
        );
    }
    Ok(RawImage {
        path: image_path,
        sha256,
        size_bytes,
    })
}

#[derive(Debug, Clone, Copy)]
struct ImageGeometry {
    partition_start_lba: u32,
    partition_sector_count: u32,
    partition_start_bytes: u64,
    partition_size_bytes: u64,
    image_size_bytes: u64,
}

fn image_geometry(partition: &PartitionLayout) -> Result<ImageGeometry> {
    let partition_start_lba = u32::try_from(partition.start_lba).map_err(|error| {
        miette::miette!(
            "partition.start_lba {} cannot be represented in a legacy MBR entry: {error}",
            partition.start_lba
        )
    })?;
    let partition_sectors = partition.size_bytes / SECTOR_BYTES;
    let partition_sector_count = u32::try_from(partition_sectors).map_err(|error| {
        miette::miette!(
            "partition.size_bytes {} is too large for a legacy MBR FAT32 entry: {error}",
            partition.size_bytes
        )
    })?;
    if partition_sector_count == 0 {
        miette::bail!("partition.size_bytes must contain at least one sector.");
    }
    let partition_start_bytes = partition
        .start_lba
        .checked_mul(SECTOR_BYTES)
        .ok_or_else(|| {
            miette::miette!(
                "partition.start_lba {} overflows the image byte offset.",
                partition.start_lba
            )
        })?;
    let image_size_bytes = partition_start_bytes
        .checked_add(partition.size_bytes)
        .ok_or_else(|| miette::miette!("SD image size overflows u64."))?;
    Ok(ImageGeometry {
        partition_start_lba,
        partition_sector_count,
        partition_start_bytes,
        partition_size_bytes: partition.size_bytes,
        image_size_bytes,
    })
}

fn write_mbr(image: &mut File, geometry: &ImageGeometry) -> Result<()> {
    let mut sector = [0_u8; SECTOR_BYTES as usize];
    let entry = &mut sector[446..462];
    entry[0] = 0x00;
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0x0c;
    entry[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]);
    entry[8..12].copy_from_slice(&geometry.partition_start_lba.to_le_bytes());
    entry[12..16].copy_from_slice(&geometry.partition_sector_count.to_le_bytes());
    sector[510..512].copy_from_slice(&[0x55, 0xaa]);
    image.seek(SeekFrom::Start(0)).map_err(|error| {
        miette::miette!("Could not seek to the MBR sector while staging an SD image: {error}")
    })?;
    image.write_all(&sector).map_err(|error| {
        miette::miette!("Could not write the MBR sector while staging an SD image: {error}")
    })
}

fn write_fat32_payload(
    bundle: &ValidatedBootBundle,
    image_path: &Path,
    geometry: &ImageGeometry,
) -> Result<()> {
    let image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image_path)
        .map_err(|error| {
            miette::miette!(
                "Could not reopen staged raw SD image '{}': {error}",
                image_path.display()
            )
        })?;
    let mut partition = PartitionStream::new(
        image,
        geometry.partition_start_bytes,
        geometry.partition_size_bytes,
    )
    .map_err(|error| {
        miette::miette!(
            "Could not bound the FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })?;
    let options = fatfs::FormatVolumeOptions::new()
        .bytes_per_sector(SECTOR_BYTES as u16)
        .total_sectors(geometry.partition_sector_count)
        .fat_type(fatfs::FatType::Fat32)
        .volume_label(fat_volume_label(&bundle.partition.label))
        .volume_id(stable_volume_id(bundle));
    fatfs::format_volume(&mut partition, options).map_err(|error| {
        miette::miette!(
            "Could not format the FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })?;
    partition.seek(SeekFrom::Start(0)).map_err(|error| {
        miette::miette!(
            "Could not rewind the FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })?;
    let filesystem =
        fatfs::FileSystem::new(partition, fatfs::FsOptions::new()).map_err(|error| {
            miette::miette!(
                "Could not mount newly formatted FAT32 boot partition in '{}': {error}",
                image_path.display()
            )
        })?;
    if filesystem.fat_type() != fatfs::FatType::Fat32 {
        miette::bail!(
            "New boot partition in '{}' is not FAT32; refusing to publish an incompatible Pi image.",
            image_path.display()
        );
    }
    {
        let root = filesystem.root_dir();
        for file in &bundle.files {
            write_file_to_fat(&root, file, image_path)?;
        }
    }
    filesystem.unmount().map_err(|error| {
        miette::miette!(
            "Could not cleanly unmount staged FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })
}

fn write_file_to_fat<T: fatfs::ReadWriteSeek>(
    root: &fatfs::Dir<'_, T>,
    file: &VerifiedBootFile,
    image_path: &Path,
) -> Result<()> {
    let destination = portable_path(&file.destination);
    create_fat_parent_directories(root, &destination, image_path)?;
    let mut output = root.create_file(&destination).map_err(|error| {
        miette::miette!(
            "Could not create '{}' in staged FAT32 image '{}': {error}",
            destination,
            image_path.display()
        )
    })?;
    output.truncate().map_err(|error| {
        miette::miette!(
            "Could not truncate '{}' in staged FAT32 image '{}': {error}",
            destination,
            image_path.display()
        )
    })?;
    let (actual_sha256, actual_size) =
        copy_source_to_writer(&file.source, &mut output, &destination)?;
    if actual_sha256 != file.sha256 || actual_size != file.size_bytes {
        miette::bail!(
            "Boot bundle input '{}' changed while creating the FAT32 image: expected SHA-256 {} ({} bytes), got {} ({} bytes).",
            file.source.display(),
            file.sha256,
            file.size_bytes,
            actual_sha256,
            actual_size
        );
    }
    output.flush().map_err(|error| {
        miette::miette!(
            "Could not flush '{}' in staged FAT32 image '{}': {error}",
            destination,
            image_path.display()
        )
    })
}

fn create_fat_parent_directories<T: fatfs::ReadWriteSeek>(
    root: &fatfs::Dir<'_, T>,
    destination: &str,
    image_path: &Path,
) -> Result<()> {
    let Some((parent, _)) = destination.rsplit_once('/') else {
        return Ok(());
    };
    let mut current = String::new();
    for component in parent.split('/') {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(component);
        root.create_dir(&current).map_err(|error| {
            miette::miette!(
                "Could not create directory '{}' in staged FAT32 image '{}': {error}",
                current,
                image_path.display()
            )
        })?;
    }
    Ok(())
}

fn copy_source_to_writer<W: Write>(
    source: &Path,
    output: &mut W,
    destination: &str,
) -> Result<(String, u64)> {
    let mut input = File::open(source).map_err(|error| {
        miette::miette!(
            "Could not open verified boot input '{}' for '{}': {error}",
            source.display(),
            destination
        )
    })?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            miette::miette!(
                "Could not read verified boot input '{}' for '{}': {error}",
                source.display(),
                destination
            )
        })?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            miette::miette!(
                "Could not write '{}' into the staged FAT32 image: {error}",
                destination
            )
        })?;
        hasher.update(&buffer[..read]);
        bytes = bytes
            .checked_add(u64::try_from(read).map_err(|error| {
                miette::miette!(
                    "Could not account for '{}' while writing '{}': {error}",
                    source.display(),
                    destination
                )
            })?)
            .ok_or_else(|| {
                miette::miette!(
                    "Boot input '{}' is too large to write into '{}'.",
                    source.display(),
                    destination
                )
            })?;
    }
    Ok((aros_common::finish_sha256(hasher).to_string(), bytes))
}

fn verify_raw_image(
    bundle: &ValidatedBootBundle,
    image_path: &Path,
    geometry: &ImageGeometry,
) -> Result<()> {
    verify_mbr(image_path, geometry)?;
    let image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image_path)
        .map_err(|error| {
            miette::miette!(
                "Could not reopen staged raw SD image '{}' for verification: {error}",
                image_path.display()
            )
        })?;
    let mut partition = PartitionStream::new(
        image,
        geometry.partition_start_bytes,
        geometry.partition_size_bytes,
    )
    .map_err(|error| {
        miette::miette!(
            "Could not bound the staged FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })?;
    partition.seek(SeekFrom::Start(0)).map_err(|error| {
        miette::miette!(
            "Could not rewind staged FAT32 image '{}' for verification: {error}",
            image_path.display()
        )
    })?;
    let filesystem =
        fatfs::FileSystem::new(partition, fatfs::FsOptions::new()).map_err(|error| {
            miette::miette!(
                "Could not mount staged FAT32 boot partition in '{}' for verification: {error}",
                image_path.display()
            )
        })?;
    if filesystem.fat_type() != fatfs::FatType::Fat32 {
        miette::bail!(
            "Staged boot partition in '{}' did not read back as FAT32.",
            image_path.display()
        );
    }
    {
        let root = filesystem.root_dir();
        for file in &bundle.files {
            let destination = portable_path(&file.destination);
            let mut input = root.open_file(&destination).map_err(|error| {
                miette::miette!(
                    "Could not read back '{}' from staged FAT32 image '{}': {error}",
                    destination,
                    image_path.display()
                )
            })?;
            let (actual_sha256, actual_size) = hash_reader(&mut input, &destination)?;
            if actual_sha256 != file.sha256 || actual_size != file.size_bytes {
                miette::bail!(
                    "Read-back verification failed for '{}' in '{}': expected SHA-256 {} ({} bytes), got {} ({} bytes).",
                    destination,
                    image_path.display(),
                    file.sha256,
                    file.size_bytes,
                    actual_sha256,
                    actual_size
                );
            }
        }
    }
    filesystem.unmount().map_err(|error| {
        miette::miette!(
            "Could not cleanly unmount verified FAT32 boot partition in '{}': {error}",
            image_path.display()
        )
    })
}

fn verify_mbr(image_path: &Path, geometry: &ImageGeometry) -> Result<()> {
    let mut image = File::open(image_path).map_err(|error| {
        miette::miette!(
            "Could not open staged raw SD image '{}' for MBR verification: {error}",
            image_path.display()
        )
    })?;
    let mut sector = [0_u8; SECTOR_BYTES as usize];
    image.read_exact(&mut sector).map_err(|error| {
        miette::miette!(
            "Could not read MBR from staged raw SD image '{}': {error}",
            image_path.display()
        )
    })?;
    if sector[510..512] != [0x55, 0xaa] {
        miette::bail!(
            "Staged raw SD image '{}' has no valid MBR signature.",
            image_path.display()
        );
    }
    let entry = &sector[446..462];
    if entry[4] != 0x0c
        || entry[8..12] != geometry.partition_start_lba.to_le_bytes()
        || entry[12..16] != geometry.partition_sector_count.to_le_bytes()
    {
        miette::bail!(
            "Staged raw SD image '{}' has an unexpected MBR FAT32 partition entry.",
            image_path.display()
        );
    }
    Ok(())
}

fn hash_reader<R: Read>(input: &mut R, label: &str) -> Result<(String, u64)> {
    let result = aros_common::sha256_reader(input).map_err(|error| {
        miette::miette!("Could not read '{label}' during SHA-256 verification: {error}")
    })?;
    Ok((result.digest.to_string(), result.size))
}

fn sync_image_file(image_path: &Path) -> Result<()> {
    let image = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image_path)
        .map_err(|error| {
            miette::miette!(
                "Could not open staged raw SD image '{}' to flush it: {error}",
                image_path.display()
            )
        })?;
    image.sync_all().map_err(|error| {
        miette::miette!(
            "Could not flush staged raw SD image '{}': {error}",
            image_path.display()
        )
    })
}

fn fat_volume_label(label: &str) -> [u8; 11] {
    let mut output = [b' '; 11];
    output[..label.len()].copy_from_slice(label.as_bytes());
    output
}

fn stable_volume_id(bundle: &ValidatedBootBundle) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(b"aros-board-sd-volume-id-v1\n");
    hasher.update(bundle.source_manifest_sha256.as_bytes());
    hasher.update(b"\n");
    hasher.update(bundle.partition.layout_sha256().as_bytes());
    let digest = hasher.finalize();
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// A seekable view limited to one partition inside a raw image file.
///
/// `fatfs` receives this wrapper rather than the whole image, so formatter or
/// filesystem mistakes cannot seek or write across the MBR or beyond the
/// declared partition boundary.
struct PartitionStream {
    file: File,
    start: u64,
    length: u64,
    position: u64,
}

impl PartitionStream {
    fn new(file: File, start: u64, length: u64) -> io::Result<Self> {
        let end = start.checked_add(length).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "partition range overflows u64")
        })?;
        if file.metadata()?.len() < end {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "partition extends beyond the image file",
            ));
        }
        Ok(Self {
            file,
            start,
            length,
            position: 0,
        })
    }

    fn absolute_position(&self) -> io::Result<u64> {
        self.start.checked_add(self.position).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "partition offset overflows u64",
            )
        })
    }

    fn bounded_buffer_length(&self, buffer_length: usize) -> io::Result<usize> {
        let buffer_length = u64::try_from(buffer_length).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer length does not fit u64",
            )
        })?;
        let available = self.length - self.position;
        usize::try_from(available.min(buffer_length)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "partition request does not fit usize",
            )
        })
    }
}

impl Read for PartitionStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = self.bounded_buffer_length(buffer.len())?;
        if length == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(self.absolute_position()?))?;
        let read = self.file.read(&mut buffer[..length])?;
        self.position = self
            .position
            .checked_add(u64::try_from(read).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "read length does not fit u64")
            })?)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "partition read overflow"))?;
        Ok(read)
    }
}

impl Write for PartitionStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let length = self.bounded_buffer_length(buffer.len())?;
        if length == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(self.absolute_position()?))?;
        let written = self.file.write(&buffer[..length])?;
        self.position = self
            .position
            .checked_add(u64::try_from(written).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "write length does not fit u64")
            })?)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "partition write overflow")
            })?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for PartitionStream {
    fn seek(&mut self, seek_from: SeekFrom) -> io::Result<u64> {
        let position = match seek_from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.length) + i128::from(offset),
        };
        if position < 0 || position > i128::from(self.length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek crosses the partition boundary",
            ));
        }
        self.position = u64::try_from(position).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position does not fit u64",
            )
        })?;
        Ok(self.position)
    }
}

fn render_artifact_manifest(bundle: &ValidatedBootBundle, image: &RawImage) -> Result<String> {
    let manifest = ImageManifest {
        format_version: super::sd_manifest::FORMAT_VERSION,
        kind: super::sd_manifest::KIND.to_string(),
        board: ArtifactBoard {
            name: bundle.board_name.clone(),
            model: bundle.model.clone(),
            transport: bundle.transport.clone(),
        },
        usb_ecm: bundle
            .usb_ecm_identity
            .as_ref()
            .map(|identity| ManifestUsbEcmIdentity {
                vendor_id: identity.vendor_id,
                product_id: identity.product_id,
                serial: identity.serial.clone(),
                expected_target_mac: identity.expected_target_mac.clone(),
            }),
        partition: ArtifactPartition {
            scheme: bundle.partition.scheme.clone(),
            filesystem: bundle.partition.filesystem.clone(),
            start_lba: bundle.partition.start_lba,
            size_bytes: bundle.partition.size_bytes,
            label: bundle.partition.label.clone(),
            layout_sha256: bundle.partition.layout_sha256(),
        },
        source_manifest: ManifestSource {
            filename: BOOT_BUNDLE_MANIFEST.to_string(),
            sha256: bundle.source_manifest_sha256.clone(),
        },
        image: ManifestImage {
            filename: RAW_IMAGE_FILENAME.to_string(),
            sha256: image.sha256.clone(),
            size_bytes: image.size_bytes,
        },
        minimum_device_bytes: image.size_bytes,
        payload: bundle
            .files
            .iter()
            .map(|file| ManifestPayloadFile {
                role: file.role.clone(),
                destination: portable_path(&file.destination),
                sha256: file.sha256.clone(),
                size_bytes: file.size_bytes,
            })
            .collect(),
    };
    let mut output = serde_json::to_string_pretty(&manifest).map_err(|error| {
        miette::miette!("Could not serialize the typed SD image manifest: {error}")
    })?;
    output.push('\n');
    Ok(output)
}

fn render_checksums(
    bundle: &ValidatedBootBundle,
    image: &RawImage,
    manifest_sha256: &str,
) -> String {
    let mut entries = BTreeMap::new();
    for file in &bundle.files {
        entries.insert(
            format!(
                "{BOOT_PAYLOAD_DIRECTORY}/{}",
                portable_path(&file.destination)
            ),
            file.sha256.as_str(),
        );
    }
    entries.insert(RAW_IMAGE_FILENAME.to_string(), image.sha256.as_str());
    entries.insert(ARTIFACT_MANIFEST.to_string(), manifest_sha256);
    let mut output = String::new();
    for (path, sha256) in entries {
        output.push_str(sha256);
        output.push_str("  ");
        output.push_str(&path);
        output.push('\n');
    }
    output
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_hex(contents: &[u8]) -> String {
    aros_common::sha256_bytes(contents).to_string()
}

fn append_hex_byte(output: &mut String, byte: u8) {
    output.push(hex_digit(byte >> 4));
    output.push(hex_digit(byte & 0x0f));
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!(),
    }
}

fn missing_inputs<T>(bundle_dir: &Path, inputs: &[String]) -> Result<T> {
    let list = inputs
        .iter()
        .map(|input| format!("  - {input}"))
        .collect::<Vec<_>>()
        .join("\n");
    Err(miette::miette!(
        "Boot bundle '{}' is incomplete. Missing required external inputs:\n{}",
        bundle_dir.display(),
        list
    ))
}

#[cfg(test)]
#[path = "sd_tests.rs"]
mod tests;
