//! Fail-closed discovery and writing for Pi SD-card images.
//!
//! This module deliberately keeps three operations separate:
//!
//! 1. [`scan`] discovers only whole, removable, writable, unmounted physical
//!    disks whose persistent identity is known.
//! 2. [`verify_image_artifact`] verifies a canonical image artifact before it
//!    can be selected for writing.
//! 3. [`write_verified_image_for_board`] rescans the selected disk, verifies the image
//!    again, requires a token tied to both, and only then opens the raw whole
//!    disk.  On Linux it uses `O_EXCL | O_NOFOLLOW` and rechecks the complete
//!    mount topology after that exclusive open but before the first byte is
//!    written.  On macOS it retains a whole-disk Disk Arbitration claim from
//!    before the raw-device open until after copy, sync, and readback.  It does
//!    not unmount, repartition, or otherwise modify a disk outside the bytes
//!    occupied by the supplied image.
//!
//! The platform scanners consume structured `diskutil -plist` (macOS) and
//! `lsblk --json` (Linux) data only.  They intentionally do not try to infer
//! safety from human-oriented command output.  A disk with a missing identity,
//! ambiguous topology, a mounted descendant, or an unsupported transport is
//! simply absent from the returned candidates.
//!
//! # Image manifest contract, version 1
//!
//! `verify_image_artifact` accepts a canonical artifact directory and two
//! relative paths below it: the JSON manifest and the raw image. The manifest
//! must match the exact, unknown-field-denying [`ImageManifest`] v1 schema
//! shared with the image producer. It includes board and optional USB-ECM
//! identity, partition layout, source-manifest identity, image identity,
//! minimum device size, and the complete staged payload inventory.
//!
//! The `image.filename` must exactly equal the caller-supplied relative image
//! path.  This prevents a manifest from quietly redirecting a write command to
//! another file in the artifact directory.  Future manifest versions require
//! an explicit review instead of being accepted optimistically.

use super::config::{Board, Transport};
#[cfg(target_os = "macos")]
use super::disk_inventory::diskutil_plist_json;
#[cfg(target_os = "linux")]
use super::disk_inventory::linux_inventory_command;
#[cfg(target_os = "macos")]
use super::disk_inventory::macos_whole_disk_identifiers;
pub use super::disk_inventory::DiskPlatform;
use super::disk_inventory::{
    is_linux_whole_device_path, is_macos_whole_disk_identifier, json_bool_like,
    json_nonempty_string, json_u64_like, safe_metadata,
};
#[cfg(any(target_os = "macos", test))]
use super::disk_inventory::{
    is_macos_descendant_identifier, macos_descendant_identifiers, macos_transport,
};
#[cfg(any(target_os = "linux", test))]
use super::disk_inventory::{json_object, linux_identity, linux_model};
use super::sd_manifest::{ImageManifest, FORMAT_VERSION, KIND};
use crate::sha256_file_with_size as sha256_file;
#[cfg(target_os = "macos")]
use aros_macos_disk_claim::{WholeDiskClaim, DEFAULT_CLAIM_TIMEOUT};
use miette::Result;
#[cfg(any(target_os = "macos", test))]
use serde_json::Map;
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

const CONFIRMATION_TOKEN_PREFIX: &str = "aros-sd-write-v1:";
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const UBOOT_USB_ECM_TRANSPORT: &str = "uboot-usb-ecm";

/// The complete USB gadget identity which binds an SD artifact to one Pi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbEcmArtifactIdentity {
    /// USB vendor ID configured by the U-Boot gadget profile.
    pub vendor_id: u16,
    /// USB product ID configured by the U-Boot gadget profile.
    pub product_id: u16,
    /// Unique USB gadget serial configured by the U-Boot gadget profile.
    pub serial: String,
    /// Pi-side USB gadget MAC accepted by the restricted DHCP server.
    pub expected_target_mac: String,
}

/// Immutable board values an SD image must match before disk selection.
///
/// The value is obtained either from a local [`Board`] via
/// [`board_image_expectation`] or constructed by an integration with an
/// equally strict local board source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardImageExpectation {
    /// Local board-profile name.
    pub name: String,
    /// Board model such as `rpi4`.
    pub model: String,
    /// Declared boot transport such as `uboot-usb-ecm`.
    pub transport: String,
    /// Required for `uboot-usb-ecm`, absent for other transports.
    pub usb_ecm_identity: Option<UsbEcmArtifactIdentity>,
}

impl BoardImageExpectation {
    /// Construct a board expectation.  Call [`Self::with_usb_ecm_identity`]
    /// for an `uboot-usb-ecm` image, then pass it to
    /// [`validate_artifact_against_expectation`].
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        model: impl Into<String>,
        transport: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            model: model.into(),
            transport: transport.into(),
            usb_ecm_identity: None,
        }
    }

    /// Require an exact U-Boot USB-ECM identity as part of this expectation.
    #[must_use]
    pub fn with_usb_ecm_identity(mut self, identity: UsbEcmArtifactIdentity) -> Self {
        self.usb_ecm_identity = Some(identity);
        self
    }
}

/// One explicitly selectable, currently safe whole-disk candidate.
///
/// `scan_id` is opaque.  It is intentionally derived from the current device
/// path and persistent physical identity, so moving a card to another reader
/// or exchanging it invalidates a previously copied selection.  The caller
/// must display the returned values to the user rather than guessing a disk.
// These are independent observations from the structured platform inventory;
// collapsing them into one enum would hide a failed safety predicate.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCandidate {
    /// Opaque ID accepted by [`write_verified_image_for_board`].
    pub scan_id: String,
    /// SHA-256 fingerprint used in the confirmation token and revalidation.
    pub fingerprint: String,
    /// Structured-source platform that discovered this disk.
    pub platform: DiskPlatform,
    /// Non-raw whole-device path, for example `/dev/disk7` or `/dev/sdb`.
    pub device_path: PathBuf,
    /// Raw whole-device path which the system backend will open for writing.
    pub raw_device_path: PathBuf,
    /// Capacity reported by the operating system, in bytes.
    pub size_bytes: u64,
    /// Persistent media or reader identity; never empty for a candidate.
    pub identity: String,
    /// User-facing model string from the operating system's structured data.
    pub model: String,
    /// User-facing transport string, restricted to removable media transports.
    pub transport: String,
    /// Every candidate returned from [`scan`] is a whole disk.
    whole_disk: bool,
    /// Every candidate returned from [`scan`] is removable.
    removable: bool,
    /// Every candidate returned from [`scan`] is writable.
    writable: bool,
    /// Every candidate returned from [`scan`] and all of its descendants are
    /// unmounted.  It remains a field so a later re-scan can be checked again.
    mounted: bool,
    /// Native device-node identity captured during a real system scan.
    ///
    /// It is deliberately not exposed as a user choice.  The production
    /// writer compares it immediately before opening the raw path to narrow
    /// the scan/open race.  Test-only injected backends may leave it absent.
    raw_device_rdev: Option<u64>,
}

impl DiskCandidate {
    /// A compact line suitable for an interactive scan command.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{}  {}  {}  {} bytes  {}  {}",
            self.scan_id,
            self.device_path.display(),
            self.model,
            self.size_bytes,
            self.transport,
            self.identity
        )
    }

    fn validate_for_write(&self, minimum_device_bytes: u64) -> Result<()> {
        if self.scan_id.is_empty()
            || aros_common::Sha256Digest::parse(&self.fingerprint).is_err()
            || !safe_metadata(&self.identity)
            || !safe_metadata(&self.model)
            || !safe_metadata(&self.transport)
        {
            miette::bail!("Selected disk candidate has incomplete or malformed identity data.");
        }
        if !self.whole_disk || !self.removable || !self.writable || self.mounted {
            miette::bail!(
                "Selected disk '{}' is no longer a safe, whole, removable, writable, unmounted target.",
                self.scan_id
            );
        }
        if self.size_bytes < minimum_device_bytes {
            miette::bail!(
                "Selected disk '{}' is {} bytes, but the verified image requires at least {} bytes.",
                self.scan_id,
                self.size_bytes,
                minimum_device_bytes
            );
        }
        Ok(())
    }
}

/// A raw SD image that is still byte-for-byte tied to its checked manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedImageArtifact {
    artifact_dir: PathBuf,
    manifest_relative_path: PathBuf,
    image_relative_path: PathBuf,
    manifest_path: PathBuf,
    image_path: PathBuf,
    manifest_sha256: String,
    image_sha256: String,
    image_size_bytes: u64,
    minimum_device_bytes: u64,
    board: BoardImageExpectation,
}

impl VerifiedImageArtifact {
    /// Canonical directory containing the image and its manifest.
    #[must_use]
    pub fn artifact_dir(&self) -> &Path {
        &self.artifact_dir
    }

    /// Canonical path to the raw image checked by this value.
    #[must_use]
    pub fn image_path(&self) -> &Path {
        &self.image_path
    }

    /// SHA-256 of the raw image bytes.
    #[must_use]
    pub fn image_sha256(&self) -> &str {
        &self.image_sha256
    }
}

/// Result of a completed write and readback verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReport {
    /// Candidate ID that was selected and revalidated immediately before write.
    pub scan_id: String,
    /// Current disk fingerprint used for that revalidation.
    pub disk_fingerprint: String,
    /// Number of image bytes written and independently read back.
    pub bytes_written: u64,
    /// SHA-256 computed from the target after the write completed.
    pub readback_sha256: String,
}

/// Owns the raw target handle and every platform claim which makes that
/// handle safe to use.
///
/// The `File` is deliberately declared first: Rust drops struct fields in
/// declaration order, so every error path closes the raw descriptor before a
/// macOS Disk Arbitration claim can be released.  The normal completion path
/// performs the same ordering explicitly and surfaces a bounded unclaim
/// failure to the caller.
struct OpenedTarget {
    file: Option<File>,
    #[cfg(target_os = "macos")]
    claim: Option<WholeDiskClaim>,
    #[cfg(test)]
    _test_guard: Option<TestTargetGuard>,
}

impl OpenedTarget {
    #[cfg(any(target_os = "linux", test))]
    const fn unclaimed(file: File) -> Self {
        Self {
            file: Some(file),
            #[cfg(target_os = "macos")]
            claim: None,
            #[cfg(test)]
            _test_guard: None,
        }
    }

    #[cfg(target_os = "macos")]
    const fn claimed(file: File, claim: WholeDiskClaim) -> Self {
        Self {
            file: Some(file),
            claim: Some(claim),
            #[cfg(test)]
            _test_guard: None,
        }
    }

    #[cfg(test)]
    const fn unclaimed_with_test_guard(file: File, test_guard: TestTargetGuard) -> Self {
        Self {
            file: Some(file),
            #[cfg(target_os = "macos")]
            claim: None,
            _test_guard: Some(test_guard),
        }
    }

    fn file(&self) -> Result<&File> {
        self.file
            .as_ref()
            .ok_or_else(|| miette::miette!("The opened SD target handle was already closed."))
    }

    fn file_mut(&mut self) -> Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| miette::miette!("The opened SD target handle was already closed."))
    }

    #[cfg(target_os = "macos")]
    fn claim(&self) -> Result<&WholeDiskClaim> {
        self.claim.as_ref().ok_or_else(|| {
            miette::miette!(
                "The macOS raw SD target has no active Disk Arbitration whole-disk claim."
            )
        })
    }

    #[cfg(target_os = "macos")]
    fn finish(mut self) -> Result<()> {
        // Close the raw descriptor before releasing the Disk Arbitration
        // claim.  `drop(self)` below preserves the same ordering for all
        // earlier error returns.
        drop(self.file.take());

        if let Some(claim) = self.claim.take() {
            claim.release().map_err(|error| {
                miette::miette!(
                    "The SD image was written and read back, but the macOS Disk Arbitration claim could not be cleanly released: {error}"
                )
            })?;
        }

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn finish(mut self) {
        drop(self.file.take());
    }
}

#[cfg(test)]
struct TestTargetGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(test)]
impl Drop for TestTargetGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Enumerate only whole, removable, writable and entirely unmounted disks.
///
/// macOS uses `diskutil`'s plist format; Linux uses `lsblk` JSON.  On an
/// unsupported host this function fails instead of attempting a broad fallback
/// such as `/dev/*` globbing.
///
/// # Errors
///
/// Returns an error when the host is unsupported or its disk inventory cannot
/// be queried or parsed without weakening the safety predicates.
pub fn scan() -> Result<Vec<DiskCandidate>> {
    #[cfg(target_os = "macos")]
    {
        scan_macos()
    }
    #[cfg(target_os = "linux")]
    {
        scan_linux()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        miette::bail!(
            "Safe SD-card discovery is implemented only for macOS and Linux; refusing to enumerate raw devices on this host."
        );
    }
}

/// Verify an image artifact without touching a disk.
///
/// Both `manifest_relative_path` and `image_relative_path` must be portable,
/// non-symlink relative paths below `artifact_dir`.  The supplied `image` path
/// must exactly match `image.filename` in the manifest.
///
/// # Errors
///
/// Returns an error when paths are unsafe, files are unreadable, the manifest
/// is invalid, or the image size or digest differs from the manifest.
pub fn verify_image_artifact(
    artifact_dir: &Path,
    manifest_relative_path: &Path,
    image_relative_path: &Path,
) -> Result<VerifiedImageArtifact> {
    let artifact_dir = canonical_existing_directory(artifact_dir, "SD image artifact directory")?;
    let manifest_relative_path = safe_relative_path(manifest_relative_path, "image manifest path")?;
    let image_relative_path = safe_relative_path(image_relative_path, "image path")?;
    let manifest_path =
        verified_regular_child(&artifact_dir, &manifest_relative_path, "SD image manifest")?;
    let image_path = verified_regular_child(&artifact_dir, &image_relative_path, "SD image")?;

    let manifest_bytes = fs::read(&manifest_path).map_err(|error| {
        miette::miette!(
            "Could not read SD image manifest '{}': {error}",
            manifest_path.display()
        )
    })?;
    let manifest_sha256 = sha256_hex(&manifest_bytes);
    let manifest: ImageManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        miette::miette!(
            "Could not parse SD image manifest '{}' as the complete version 1 schema: {error}",
            manifest_path.display()
        )
    })?;
    validate_image_manifest(&manifest, &image_relative_path, &image_path)?;
    let board = board_expectation_from_manifest(&manifest)?;

    let image_sha256 = normalized_sha256(&manifest.image.sha256, "SD image manifest.image.sha256")?;
    let image_size_bytes = manifest.image.size_bytes;
    let minimum_device_bytes = manifest.minimum_device_bytes;
    if image_size_bytes == 0 || minimum_device_bytes < image_size_bytes {
        miette::bail!(
            "SD image manifest declares image size {} and minimum device size {}; both must be non-zero and the minimum must cover the image.",
            image_size_bytes,
            minimum_device_bytes
        );
    }

    let (actual_image_sha256, actual_image_size) = sha256_file(&image_path)?;
    if actual_image_size != image_size_bytes {
        miette::bail!(
            "SD image '{}' is {} bytes, but its manifest declares {} bytes.",
            image_path.display(),
            actual_image_size,
            image_size_bytes
        );
    }
    if actual_image_sha256 != image_sha256 {
        miette::bail!(
            "SHA-256 mismatch for SD image '{}': manifest declares {}, actual file is {}.",
            image_path.display(),
            image_sha256,
            actual_image_sha256
        );
    }

    Ok(VerifiedImageArtifact {
        artifact_dir,
        manifest_relative_path,
        image_relative_path,
        manifest_path,
        image_path,
        manifest_sha256,
        image_sha256,
        image_size_bytes,
        minimum_device_bytes,
        board,
    })
}

/// Convert a board profile into the immutable fields an SD image must carry.
///
/// In USB-ECM mode an incomplete profile is an error rather than a reason to
/// make a broadly reusable image.
///
/// # Errors
///
/// Returns an error when required board or USB-ECM identity fields are absent
/// or malformed.
pub fn board_image_expectation(board: &Board) -> Result<BoardImageExpectation> {
    let manifest_board_name = if board.config.transport == Transport::UefiEsp {
        board.config.model.as_str()
    } else {
        &board.name
    };
    let mut expectation = BoardImageExpectation::new(
        manifest_board_name,
        board.config.model.to_string(),
        board.config.transport.to_string(),
    );
    if board.config.transport == Transport::UbootUsbEcm {
        let identity = board
            .config
            .usb_ecm
            .as_ref()
            .and_then(|usb_ecm| usb_ecm.identity.as_ref())
            .ok_or_else(|| {
                miette::miette!(
                    "Board '{}' uses uboot-usb-ecm but has no complete usb_ecm.identity.",
                    board.name
                )
            })?;
        expectation = expectation.with_usb_ecm_identity(UsbEcmArtifactIdentity {
            vendor_id: identity.vendor_id,
            product_id: identity.product_id,
            serial: identity.serial.clone(),
            expected_target_mac: identity.expected_target_mac.clone(),
        });
    }
    validate_board_expectation(&expectation, "selected board profile")?;
    Ok(expectation)
}

/// Re-read and re-hash `artifact`, then require its board metadata to match
/// the supplied local expectation before a disk is considered.
///
/// The re-read makes a stale `VerifiedImageArtifact` insufficient after a
/// rebuild or a manifest edit.  Use [`validate_artifact_for_board`] when the
/// selected profile is already represented by [`Board`].
///
/// # Errors
///
/// Returns an error when the artifact changed, is no longer valid, or does not
/// match the supplied board expectation.
pub fn validate_artifact_against_expectation(
    artifact: &VerifiedImageArtifact,
    expectation: &BoardImageExpectation,
) -> Result<()> {
    validate_board_expectation(expectation, "local board expectation")?;
    let reverified = verify_image_artifact(
        &artifact.artifact_dir,
        &artifact.manifest_relative_path,
        &artifact.image_relative_path,
    )?;
    if reverified != *artifact {
        miette::bail!(
            "The SD image artifact changed after it was selected. Re-verify it and obtain a new disk confirmation token."
        );
    }
    validate_verified_board_match(&reverified, expectation)
}

/// Re-read and bind a verified image artifact to a selected local board
/// profile.  This is the check a `--board` CLI path should run before it shows
/// a selectable SD disk.
///
/// # Errors
///
/// Returns an error when the board expectation is incomplete, the artifact
/// changed, or its embedded identity does not match the selected board.
#[allow(dead_code)] // Public guard for non-CLI callers; CLI uses the atomic verifier/writer pair.
pub fn validate_artifact_for_board(artifact: &VerifiedImageArtifact, board: &Board) -> Result<()> {
    let expectation = board_image_expectation(board)?;
    validate_artifact_against_expectation(artifact, &expectation)
}

/// Verify image content and bind its board metadata to `board` in one
/// read-only operation.  This is suitable for a `--dry-run` write plan or for
/// showing board-scoped confirmation tokens after `sd scan`.
///
/// # Errors
///
/// Returns an error when artifact verification fails or its board identity
/// differs from the selected local board.
pub fn verify_image_artifact_for_board(
    artifact_dir: &Path,
    manifest_relative_path: &Path,
    image_relative_path: &Path,
    board: &Board,
) -> Result<VerifiedImageArtifact> {
    let expectation = board_image_expectation(board)?;
    let artifact =
        verify_image_artifact(artifact_dir, manifest_relative_path, image_relative_path)?;
    validate_verified_board_match(&artifact, &expectation)?;
    Ok(artifact)
}

/// Derive the exact token that must be presented to write `artifact` to
/// `candidate`.
///
/// The token is intentionally not a bypass flag: changing the manifest, raw
/// image, selected whole disk, capacity, or persistent disk identity changes
/// it.  A UI should show this value after `scan` and require the user to paste
/// it into a separate explicit write command.
#[must_use]
pub fn confirmation_token(artifact: &VerifiedImageArtifact, candidate: &DiskCandidate) -> String {
    let material = format!(
        "aros-sd-write-v1\\nmanifest={}\\nimage={}\\nimage_size={}\\nminimum_device={}\\ndisk={}\\n",
        artifact.manifest_sha256,
        artifact.image_sha256,
        artifact.image_size_bytes,
        artifact.minimum_device_bytes,
        candidate.fingerprint,
    );
    format!(
        "{CONFIRMATION_TOKEN_PREFIX}{}",
        sha256_hex(material.as_bytes())
    )
}

/// Reverify, bind and write an image for exactly one selected board profile.
///
/// This is the safe physical-write entry point for a CLI that accepts
/// `--board`: the final board/manifest comparison occurs after the artifact is
/// re-read and before the disk scanner or raw-device opener are called.
///
/// # Errors
///
/// Returns an error when board binding, confirmation, disk revalidation,
/// exclusive opening, writing, flushing, or post-write verification fails.
pub fn write_verified_image_for_board(
    artifact: &VerifiedImageArtifact,
    board: &Board,
    selected_scan_id: &str,
    confirmation_token_value: &str,
) -> Result<WriteReport> {
    let expectation = board_image_expectation(board)?;
    let backend = SystemDiskBackend;
    write_verified_image_with_backend_and_expectation(
        artifact,
        selected_scan_id,
        confirmation_token_value,
        &expectation,
        &backend,
    )
}

trait DiskBackend {
    fn scan(&self) -> Result<Vec<DiskCandidate>>;
    fn open_verified_target(&self, candidate: &DiskCandidate) -> Result<OpenedTarget>;
    /// Check the target again after its exclusive/raw handle has been opened,
    /// but before the writer can seek or copy any image bytes.
    fn verify_target_safe_after_open(
        &self,
        candidate: &DiskCandidate,
        target: &OpenedTarget,
    ) -> Result<()>;
}

struct SystemDiskBackend;

impl DiskBackend for SystemDiskBackend {
    fn scan(&self) -> Result<Vec<DiskCandidate>> {
        scan()
    }

    fn open_verified_target(&self, candidate: &DiskCandidate) -> Result<OpenedTarget> {
        open_system_raw_device(candidate)
    }

    fn verify_target_safe_after_open(
        &self,
        candidate: &DiskCandidate,
        target: &OpenedTarget,
    ) -> Result<()> {
        verify_system_target_safe_after_open(candidate, target)
    }
}

#[cfg(test)]
fn write_verified_image_with_backend(
    artifact: &VerifiedImageArtifact,
    selected_scan_id: &str,
    confirmation_token_value: &str,
    backend: &dyn DiskBackend,
) -> Result<WriteReport> {
    // This helper exists only in unit tests.  Production code has no raw-disk
    // writer that accepts an unbound artifact: `write_verified_image_for_board`
    // always derives a separate expectation from the selected local profile.
    let test_expectation = artifact.board.clone();
    write_verified_image_with_backend_and_expectation(
        artifact,
        selected_scan_id,
        confirmation_token_value,
        &test_expectation,
        backend,
    )
}

fn write_verified_image_with_backend_and_expectation(
    artifact: &VerifiedImageArtifact,
    selected_scan_id: &str,
    confirmation_token_value: &str,
    expectation: &BoardImageExpectation,
    backend: &dyn DiskBackend,
) -> Result<WriteReport> {
    if selected_scan_id.trim().is_empty() {
        miette::bail!("An explicit non-empty SD scan ID is required before writing.");
    }
    if !confirmation_token_value.starts_with(CONFIRMATION_TOKEN_PREFIX) {
        miette::bail!(
            "A confirmation token beginning with '{}' is required; no disk was opened.",
            CONFIRMATION_TOKEN_PREFIX
        );
    }

    // The original checked value might have been kept around while files were
    // rebuilt.  Resolve and hash it again before even looking at a disk.
    let reverified = verify_image_artifact(
        &artifact.artifact_dir,
        &artifact.manifest_relative_path,
        &artifact.image_relative_path,
    )?;
    if reverified != *artifact {
        miette::bail!(
            "The SD image artifact changed after it was selected. Re-run artifact verification and obtain a new confirmation token."
        );
    }
    validate_verified_board_match(&reverified, expectation)?;

    let current_candidates = backend.scan()?;
    let selected = current_candidates
        .iter()
        .filter(|candidate| candidate.scan_id == selected_scan_id)
        .collect::<Vec<_>>();
    let candidate = match selected.as_slice() {
        [candidate] => *candidate,
        [] => {
            miette::bail!(
                "No currently safe removable whole disk has scan ID '{}'. Re-run `aros board sd scan`; no disk was opened.",
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
    candidate.validate_for_write(reverified.minimum_device_bytes)?;

    let expected_token = confirmation_token(&reverified, candidate);
    if confirmation_token_value != expected_token {
        miette::bail!(
            "Confirmation token does not match the current verified image and disk '{}'; no disk was opened.",
            selected_scan_id
        );
    }

    // This is deliberately the first raw-device open.  Linux uses O_EXCL |
    // O_NOFOLLOW; macOS acquires a whole-disk Disk Arbitration claim first.
    // Both repeat the complete structured mount check while their platform
    // ownership and the exact raw device handle remain held.
    let mut target = backend.open_verified_target(candidate)?;
    backend.verify_target_safe_after_open(candidate, &target)?;
    target
        .file_mut()?
        .seek(SeekFrom::Start(0))
        .map_err(|error| {
            miette::miette!(
                "Could not seek target disk '{}' before image write: {error}",
                candidate.raw_device_path.display()
            )
        })?;
    let copied_sha256 = copy_image_to_target(&reverified.image_path, target.file_mut()?)?;
    if copied_sha256 != reverified.image_sha256 {
        miette::bail!(
            "The SD image changed while it was being copied. The target may contain incomplete data; do not boot it, reverify and write again."
        );
    }
    target.file()?.sync_all().map_err(|error| {
        miette::miette!(
            "Could not flush target disk '{}' after image write: {error}",
            candidate.raw_device_path.display()
        )
    })?;

    let readback_sha256 = hash_target_prefix(target.file_mut()?, reverified.image_size_bytes)?;
    if readback_sha256 != reverified.image_sha256 {
        miette::bail!(
            "Readback SHA-256 mismatch on target disk '{}': expected {}, got {}. Do not boot this media.",
            candidate.scan_id,
            reverified.image_sha256,
            readback_sha256
        );
    }

    #[cfg(target_os = "macos")]
    target.finish()?;
    #[cfg(not(target_os = "macos"))]
    target.finish();

    Ok(WriteReport {
        scan_id: candidate.scan_id.clone(),
        disk_fingerprint: candidate.fingerprint.clone(),
        bytes_written: reverified.image_size_bytes,
        readback_sha256,
    })
}

#[path = "sd_artifact_validation.rs"]
mod artifact_validation;
use artifact_validation::{
    board_expectation_from_manifest, validate_board_expectation, validate_image_manifest,
    validate_verified_board_match,
};

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

fn safe_relative_path(path: &Path, label: &str) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        miette::bail!("{label} must not be empty.");
    }
    let raw = path.to_str().ok_or_else(|| {
        miette::miette!("{label} must be valid UTF-8 and a portable relative path.")
    })?;
    if raw.contains('\\') || raw.chars().any(char::is_control) {
        miette::bail!(
            "{label} '{}' is not a portable relative path.",
            path.display()
        );
    }
    let mut components = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(part) if !part.is_empty() => components += 1,
            Component::Normal(_)
            | Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                miette::bail!(
                    "{label} '{}' must be a relative path without '.' or '..'.",
                    path.display()
                );
            }
        }
    }
    if components == 0 {
        miette::bail!("{label} '{}' is not usable.", path.display());
    }
    Ok(path.to_path_buf())
}

fn verified_regular_child(root: &Path, relative_path: &Path, label: &str) -> Result<PathBuf> {
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(part) = component else {
            miette::bail!("Internal error: unvalidated relative {label} path.");
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            miette::miette!("Could not inspect {label} '{}': {error}", current.display())
        })?;
        if metadata.file_type().is_symlink() {
            miette::bail!(
                "{label} '{}' must not contain symbolic links.",
                current.display()
            );
        }
    }
    let metadata = fs::metadata(&current).map_err(|error| {
        miette::miette!("Could not inspect {label} '{}': {error}", current.display())
    })?;
    if !metadata.is_file() {
        miette::bail!("{label} '{}' must be a regular file.", current.display());
    }
    let canonical = current.canonicalize().map_err(|error| {
        miette::miette!("Could not resolve {label} '{}': {error}", current.display())
    })?;
    if !canonical.starts_with(root) {
        miette::bail!(
            "{label} '{}' resolves outside artifact directory '{}'.",
            canonical.display(),
            root.display()
        );
    }
    Ok(canonical)
}

fn normalized_sha256(value: &str, label: &str) -> Result<String> {
    aros_common::Sha256Digest::parse(value)
        .map(|digest| digest.to_string())
        .map_err(|_| miette::miette!("{label} must be a 64-character SHA-256 hexadecimal digest."))
}

fn copy_image_to_target(image_path: &Path, target: &mut File) -> Result<String> {
    let mut image = File::open(image_path).map_err(|error| {
        miette::miette!(
            "Could not open verified SD image '{}': {error}",
            image_path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = image.read(&mut buffer).map_err(|error| {
            miette::miette!(
                "Could not read SD image '{}' while writing: {error}",
                image_path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read]).map_err(|error| {
            miette::miette!("Could not write verified SD image to target: {error}")
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok(aros_common::finish_sha256(hasher).to_string())
}

fn hash_target_prefix(target: &mut File, bytes_to_read: u64) -> Result<String> {
    target.seek(SeekFrom::Start(0)).map_err(|error| {
        miette::miette!("Could not seek target disk for readback verification: {error}")
    })?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes_to_read;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining != 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
            .map_err(|error| miette::miette!("Could not size SD image readback: {error}"))?;
        let read = target.read(&mut buffer[..requested]).map_err(|error| {
            miette::miette!("Could not read target disk during readback verification: {error}")
        })?;
        if read == 0 {
            miette::bail!("Target disk ended before the complete SD image could be read back.");
        }
        hasher.update(&buffer[..read]);
        remaining = remaining
            .checked_sub(u64::try_from(read).map_err(|error| {
                miette::miette!("Could not account for SD image readback: {error}")
            })?)
            .ok_or_else(|| miette::miette!("Internal SD image readback underflow."))?;
    }
    Ok(aros_common::finish_sha256(hasher).to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    aros_common::sha256_bytes(bytes).to_string()
}

fn make_candidate(
    platform: DiskPlatform,
    device_path: PathBuf,
    raw_device_path: PathBuf,
    size_bytes: u64,
    identity: String,
    model: String,
    transport: String,
) -> Option<DiskCandidate> {
    if size_bytes == 0
        || !safe_metadata(&identity)
        || !safe_metadata(&model)
        || !safe_metadata(&transport)
    {
        return None;
    }
    let fingerprint_material = format!(
        "aros-board-sd-disk-v1\\nplatform={}\\ndevice={}\\nidentity={}\\nsize={}\\nmodel={}\\ntransport={}\\n",
        platform.label(),
        device_path.display(),
        identity,
        size_bytes,
        model,
        transport,
    );
    let fingerprint = sha256_hex(fingerprint_material.as_bytes());
    let scan_id = format!("{}-{}", platform.label(), &fingerprint[..16]);
    Some(DiskCandidate {
        scan_id,
        fingerprint,
        platform,
        device_path,
        raw_device_path,
        size_bytes,
        identity,
        model,
        transport,
        whole_disk: true,
        removable: true,
        writable: true,
        mounted: false,
        raw_device_rdev: None,
    })
}

// ---------------------------------------------------------------------------
// Linux structured scanner (`lsblk --json`)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", test))]
fn parse_linux_inventory(value: &Value) -> Result<Vec<DiskCandidate>> {
    let root = json_object(value, "lsblk JSON output")?;
    let devices = root
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or_else(|| miette::miette!("lsblk JSON output must contain a blockdevices array."))?;

    let mut candidates = retain_unambiguous_physical_candidates(
        devices
            .iter()
            .filter_map(linux_candidate_from_node)
            .collect::<Vec<_>>(),
    );
    candidates.sort_by(|left, right| left.scan_id.cmp(&right.scan_id));
    Ok(candidates)
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn retain_unambiguous_physical_candidates(candidates: Vec<DiskCandidate>) -> Vec<DiskCandidate> {
    let mut identities = HashMap::<DiskPlatform, HashMap<String, usize>>::new();
    let mut device_paths = HashMap::new();
    let mut raw_device_paths = HashMap::new();
    for candidate in &candidates {
        *identities
            .entry(candidate.platform)
            .or_default()
            .entry(candidate.identity.clone())
            .or_insert(0) += 1;
        *device_paths
            .entry(candidate.device_path.clone())
            .or_insert(0) += 1;
        *raw_device_paths
            .entry(candidate.raw_device_path.clone())
            .or_insert(0) += 1;
    }
    candidates
        .into_iter()
        .filter(|candidate| {
            identities
                .get(&candidate.platform)
                .and_then(|platform| platform.get(&candidate.identity))
                == Some(&1)
                && device_paths.get(&candidate.device_path) == Some(&1)
                && raw_device_paths.get(&candidate.raw_device_path) == Some(&1)
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn linux_candidate_from_node(node: &Value) -> Option<DiskCandidate> {
    let object = node.as_object()?;
    if object.get("type")?.as_str()? != "disk" {
        return None;
    }
    if !json_bool_like(object, "rm")?
        || !json_bool_like(object, "hotplug")?
        || json_bool_like(object, "ro")?
    {
        return None;
    }
    let path_text = json_nonempty_string(object, "path")?;
    if json_nonempty_string(object, "name")? != path_text {
        return None;
    }
    let path = PathBuf::from(path_text);
    if !is_linux_whole_device_path(&path) {
        return None;
    }
    let root_name = path.file_name()?.to_str()?;
    if linux_kernel_name(json_nonempty_string(object, "kname")?)? != root_name
        || !matches!(object.get("pkname"), Some(Value::Null))
    {
        return None;
    }
    let transport = json_nonempty_string(object, "tran")?.to_ascii_lowercase();
    let family = linux_device_family(root_name, &transport)?;
    if !linux_tree_is_complete_and_unmounted(node, root_name, family)? {
        return None;
    }
    let size_bytes = json_u64_like(object, "size")?;
    let model = linux_model(object)?;
    let identity = linux_identity(object)?;

    make_candidate(
        DiskPlatform::Linux,
        path.clone(),
        path,
        size_bytes,
        identity,
        model,
        transport,
    )
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy)]
enum LinuxDeviceFamily {
    UsbScsi,
    Mmc,
}

#[cfg(any(target_os = "linux", test))]
fn linux_device_family(root_name: &str, transport: &str) -> Option<LinuxDeviceFamily> {
    match transport {
        "usb" if root_name.starts_with("sd") => Some(LinuxDeviceFamily::UsbScsi),
        "mmc" if root_name.starts_with("mmcblk") => Some(LinuxDeviceFamily::Mmc),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_kernel_name(value: &str) -> Option<&str> {
    let name = value.strip_prefix("/dev/").unwrap_or(value);
    (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric())).then_some(name)
}

#[cfg(any(target_os = "linux", test))]
fn linux_partition_name_is_canonical(
    root_name: &str,
    family: LinuxDeviceFamily,
    name: &str,
) -> bool {
    let Some(suffix) = name.strip_prefix(root_name) else {
        return false;
    };
    let digits = match family {
        LinuxDeviceFamily::UsbScsi => suffix,
        LinuxDeviceFamily::Mmc => {
            let Some(digits) = suffix.strip_prefix('p') else {
                return false;
            };
            digits
        }
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && !digits.starts_with('0')
}

#[cfg(any(target_os = "linux", test))]
fn linux_tree_is_complete_and_unmounted(
    node: &Value,
    root_name: &str,
    family: LinuxDeviceFamily,
) -> Option<bool> {
    let mut seen = HashSet::new();
    linux_node_is_complete_and_unmounted(node, None, root_name, family, &mut seen)
}

#[cfg(any(target_os = "linux", test))]
fn linux_node_is_complete_and_unmounted(
    node: &Value,
    expected_parent: Option<&str>,
    root_name: &str,
    family: LinuxDeviceFamily,
    seen: &mut HashSet<String>,
) -> Option<bool> {
    let object = node.as_object()?;
    let expected_type = if expected_parent.is_none() {
        "disk"
    } else {
        "part"
    };
    if json_nonempty_string(object, "type")? != expected_type {
        return Some(false);
    }

    let path = json_nonempty_string(object, "path")?;
    if json_nonempty_string(object, "name")? != path {
        return Some(false);
    }
    let kname = linux_kernel_name(json_nonempty_string(object, "kname")?)?;
    if path != format!("/dev/{kname}") || !seen.insert(kname.to_string()) {
        return Some(false);
    }
    match expected_parent {
        None => {
            if kname != root_name || !matches!(object.get("pkname"), Some(Value::Null)) {
                return Some(false);
            }
        }
        Some(parent) => {
            if parent != root_name
                || linux_kernel_name(json_nonempty_string(object, "pkname")?)? != parent
                || !linux_partition_name_is_canonical(root_name, family, kname)
            {
                return Some(false);
            }
        }
    }

    if !linux_mountpoints_are_empty(object.get("mountpoints")?) {
        return Some(false);
    }
    match object.get("children") {
        None | Some(Value::Null) => Some(true),
        Some(Value::Array(children)) => {
            // This writer intentionally understands only a whole disk with
            // direct partition children. Device-mapper, crypto, RAID, and
            // other nested stacks are not unequivocal removable-media
            // targets and therefore fail closed.
            if expected_parent.is_some() && !children.is_empty() {
                return Some(false);
            }
            for child in children {
                if !linux_node_is_complete_and_unmounted(
                    child,
                    Some(kname),
                    root_name,
                    family,
                    seen,
                )? {
                    return Some(false);
                }
            }
            Some(true)
        }
        Some(_) => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountpoints_are_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(values) => values.iter().all(|value| match value {
            Value::Null => true,
            Value::String(value) => value.trim().is_empty(),
            _ => false,
        }),
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn scan_linux() -> Result<Vec<DiskCandidate>> {
    let output = crate::run_output(
        &mut linux_inventory_command(true),
        "lsblk safe SD-card inventory",
    )?;
    let parsed: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        miette::miette!(
            "Could not parse lsblk JSON output: {error}. Refusing to enumerate raw devices."
        )
    })?;
    let candidates = parse_linux_inventory(&parsed)?;
    Ok(candidates
        .into_iter()
        .filter_map(|candidate| annotate_system_device(candidate).ok())
        .collect())
}

// ---------------------------------------------------------------------------
// macOS structured scanner (`diskutil -plist`, converted with `plutil`)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", test))]
fn macos_candidate_from_info(
    info: &Value,
    descendants_are_unmounted: bool,
) -> Option<DiskCandidate> {
    let object = info.as_object()?;
    let identifier = json_nonempty_string(object, "DeviceIdentifier")?;
    if !is_macos_whole_disk_identifier(identifier)
        || !json_bool_like(object, "WholeDisk")?
        || json_nonempty_string(object, "ParentWholeDisk")? != identifier
        || json_bool_like(object, "Internal")?
        || !json_bool_like(object, "Writable")?
        || !json_bool_like(object, "Ejectable")?
        || !macos_media_is_removable(object)
        || !macos_mountpoint_is_empty(object)
        || !descendants_are_unmounted
    {
        return None;
    }
    if !json_nonempty_string(object, "VirtualOrPhysical")?.eq_ignore_ascii_case("physical") {
        return None;
    }
    let transport = macos_transport(object)?;
    let serial = json_nonempty_string(object, "SerialNumber")?;
    let model = json_nonempty_string(object, "MediaName")?.to_string();
    let device_path = PathBuf::from(json_nonempty_string(object, "DeviceNode")?);
    let expected_device_path = PathBuf::from(format!("/dev/{identifier}"));
    if device_path != expected_device_path {
        return None;
    }
    let size_bytes = json_u64_like(object, "Size")?;
    make_candidate(
        DiskPlatform::Macos,
        device_path,
        PathBuf::from(format!("/dev/r{identifier}")),
        size_bytes,
        format!("serial:{serial}"),
        model,
        transport,
    )
}

#[cfg(any(target_os = "macos", test))]
fn macos_media_is_removable(object: &Map<String, Value>) -> bool {
    let mut saw_explicit_true = false;
    for field in ["Removable", "RemovableMedia"] {
        if object.contains_key(field) {
            match json_bool_like(object, field) {
                Some(true) => saw_explicit_true = true,
                Some(false) | None => return false,
            }
        }
    }
    saw_explicit_true
}

#[cfg(any(target_os = "macos", test))]
fn macos_mountpoint_is_empty(object: &Map<String, Value>) -> bool {
    matches!(object.get("MountPoint"), Some(Value::Null))
        || matches!(object.get("MountPoint"), Some(Value::String(value)) if value.trim().is_empty())
}

#[cfg(any(target_os = "macos", test))]
fn macos_topology_is_complete_and_unmounted(
    whole: &str,
    descendant_identifiers: &[String],
    descendant_infos: &[Value],
) -> bool {
    if descendant_identifiers.is_empty()
        || descendant_identifiers.len() != descendant_infos.len()
        || descendant_identifiers
            .iter()
            .filter(|identifier| identifier.as_str() == whole)
            .count()
            != 1
    {
        return false;
    }
    let expected = descendant_identifiers
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if expected.len() != descendant_identifiers.len()
        || !expected
            .iter()
            .all(|identifier| is_macos_descendant_identifier(identifier, whole))
    {
        return false;
    }

    let mut seen = HashSet::with_capacity(descendant_infos.len());
    for info in descendant_infos {
        let Some(object) = info.as_object() else {
            return false;
        };
        let Some(identifier) = json_nonempty_string(object, "DeviceIdentifier") else {
            return false;
        };
        let expected_path = format!("/dev/{identifier}");
        if !expected.contains(identifier)
            || !seen.insert(identifier.to_string())
            || !is_macos_descendant_identifier(identifier, whole)
            || json_bool_like(object, "WholeDisk") != Some(identifier == whole)
            || json_nonempty_string(object, "ParentWholeDisk") != Some(whole)
            || json_nonempty_string(object, "DeviceNode") != Some(expected_path.as_str())
            || !macos_mountpoint_is_empty(object)
        {
            return false;
        }
    }
    seen.len() == expected.len()
}

#[cfg(any(target_os = "macos", test))]
fn macos_candidate_from_inventory(
    root_info: &Value,
    descendant_identifiers: &[String],
    descendant_infos: &[Value],
) -> Option<DiskCandidate> {
    let whole = root_info
        .as_object()
        .and_then(|object| json_nonempty_string(object, "DeviceIdentifier"))?;
    let candidate = macos_candidate_from_info(root_info, true)?;
    if !macos_topology_is_complete_and_unmounted(whole, descendant_identifiers, descendant_infos) {
        return None;
    }
    let topology_root = descendant_infos.iter().find(|info| {
        info.as_object()
            .and_then(|object| json_nonempty_string(object, "DeviceIdentifier"))
            == Some(whole)
    })?;
    let topology_candidate = macos_candidate_from_info(topology_root, true)?;
    (topology_candidate == candidate).then_some(candidate)
}

#[cfg(target_os = "macos")]
fn scan_macos() -> Result<Vec<DiskCandidate>> {
    let list = diskutil_plist_json(&["list", "-plist"])?;
    let identifiers = macos_whole_disk_identifiers(&list)?;
    let mut candidates = Vec::new();

    for identifier in identifiers {
        let path = format!("/dev/{identifier}");
        let Ok(info) = diskutil_plist_json(&["info", "-plist", &path]) else {
            continue;
        };
        let Ok(descendants) = diskutil_plist_json(&["list", "-plist", &path]) else {
            continue;
        };
        let Ok(descendant_identifiers) = macos_descendant_identifiers(&descendants, &identifier)
        else {
            continue;
        };
        let mut descendant_infos = Vec::with_capacity(descendant_identifiers.len());
        let mut complete = true;
        for descendant in &descendant_identifiers {
            let descendant_path = format!("/dev/{descendant}");
            if let Ok(descendant_info) = diskutil_plist_json(&["info", "-plist", &descendant_path])
            {
                descendant_infos.push(descendant_info);
            } else {
                complete = false;
                break;
            }
        }
        if !complete {
            continue;
        }
        let Some(candidate) =
            macos_candidate_from_inventory(&info, &descendant_identifiers, &descendant_infos)
        else {
            continue;
        };
        candidates.push(candidate);
    }
    let mut candidates = retain_unambiguous_physical_candidates(candidates)
        .into_iter()
        .filter_map(|candidate| annotate_system_device(candidate).ok())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.scan_id.cmp(&right.scan_id));
    Ok(candidates)
}

// ---------------------------------------------------------------------------
// Raw-device opening after the final revalidation
// ---------------------------------------------------------------------------

fn annotate_system_device(mut candidate: DiskCandidate) -> Result<DiskCandidate> {
    candidate.raw_device_rdev = Some(raw_device_rdev(
        &candidate.raw_device_path,
        candidate.platform,
    )?);
    Ok(candidate)
}

fn open_system_raw_device(candidate: &DiskCandidate) -> Result<OpenedTarget> {
    #[cfg(target_os = "linux")]
    {
        open_linux_raw_device_exclusively(candidate).map(OpenedTarget::unclaimed)
    }
    #[cfg(target_os = "macos")]
    {
        open_macos_raw_device_with_claim(candidate)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = candidate;
        miette::bail!(
            "Safe physical SD-card writes are implemented only on macOS and Linux. No disk was opened."
        );
    }
}

fn verify_system_target_safe_after_open(
    candidate: &DiskCandidate,
    target: &OpenedTarget,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        verify_linux_target_safe_after_exclusive_open(candidate, target.file()?)
    }
    #[cfg(target_os = "macos")]
    {
        verify_macos_target_safe_after_claimed_open(candidate, target)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (candidate, target);
        miette::bail!(
            "Safe physical SD-card writes are unsupported on this host. No disk was written."
        );
    }
}

/// Claim one exact macOS whole disk before opening its raw character device.
///
/// Disk Arbitration ownership is the macOS equivalent of the Linux exclusive
/// block-device handle for this workflow.  The returned owner keeps the claim
/// alive until the raw descriptor has been closed after readback.
#[cfg(target_os = "macos")]
fn open_macos_raw_device_with_claim(candidate: &DiskCandidate) -> Result<OpenedTarget> {
    candidate.validate_for_write(1)?;
    if candidate.platform != DiskPlatform::Macos {
        miette::bail!(
            "Selected disk '{}' is not a macOS candidate; refusing to claim a raw target.",
            candidate.scan_id
        );
    }

    let expected_raw_path = expected_system_raw_device_path(candidate)?;
    if candidate.raw_device_path != expected_raw_path {
        miette::bail!(
            "Selected disk '{}' has an unexpected raw-device path '{}'.",
            candidate.scan_id,
            candidate.raw_device_path.display()
        );
    }
    let expected_rdev = candidate.raw_device_rdev.ok_or_else(|| {
        miette::miette!(
            "Selected disk '{}' has no native device-node identity. Re-run SD scan; no disk was opened.",
            candidate.scan_id
        )
    })?;
    if raw_device_rdev(&candidate.raw_device_path, candidate.platform)? != expected_rdev {
        miette::bail!(
            "Raw device node '{}' changed after scan. Re-run SD scan; no disk was opened.",
            candidate.raw_device_path.display()
        );
    }

    let expected_bsd_name = candidate
        .device_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            miette::miette!(
                "Selected disk '{}' has no canonical macOS whole-disk BSD name.",
                candidate.scan_id
            )
        })?;
    let claim = WholeDiskClaim::acquire(
        candidate.device_path.as_os_str(),
        DEFAULT_CLAIM_TIMEOUT,
    )
    .map_err(|error| {
        miette::miette!(
            "Could not acquire an exclusive Disk Arbitration claim for '{}': {error}. No raw disk was opened.",
            candidate.device_path.display()
        )
    })?;
    if claim.bsd_name().as_str() != expected_bsd_name
        || claim.device_path() != candidate.device_path
        || claim.raw_device_path() != candidate.raw_device_path
    {
        miette::bail!(
            "Disk Arbitration claimed a different device than selected disk '{}'; no raw disk was opened.",
            candidate.scan_id
        );
    }

    // Recheck the raw node after the asynchronous claim completed and before
    // opening it.  If any error follows, local declaration order drops `File`
    // (when present) before `claim`.
    if raw_device_rdev(claim.raw_device_path(), candidate.platform)? != expected_rdev {
        miette::bail!(
            "Raw device node '{}' changed while its Disk Arbitration claim was acquired. Re-run SD scan; no disk was opened.",
            candidate.raw_device_path.display()
        );
    }
    let raw_fd = rustix::fs::open(
        claim.raw_device_path(),
        macos_raw_open_flags(),
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| {
        miette::miette!(
            "Could not open claimed raw disk '{}' for verified writing (O_NOFOLLOW): {error}",
            candidate.raw_device_path.display()
        )
    })?;
    let file = File::from(raw_fd);
    if raw_device_rdev_from_file(&file, candidate.platform)? != expected_rdev {
        miette::bail!(
            "Raw disk '{}' changed while it was opened under Disk Arbitration claim. Refusing to write through this handle.",
            candidate.raw_device_path.display()
        );
    }

    Ok(OpenedTarget::claimed(file, claim))
}

/// Re-run the complete structured macOS safety scan while the whole-disk
/// claim and exact raw descriptor are both held.
#[cfg(target_os = "macos")]
fn verify_macos_target_safe_after_claimed_open(
    candidate: &DiskCandidate,
    target: &OpenedTarget,
) -> Result<()> {
    if candidate.platform != DiskPlatform::Macos {
        miette::bail!(
            "Selected disk '{}' is not a macOS candidate; no bytes were written.",
            candidate.scan_id
        );
    }
    let claim = target.claim()?;
    let expected_bsd_name = candidate
        .device_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            miette::miette!(
                "Selected disk '{}' has no canonical macOS whole-disk BSD name.",
                candidate.scan_id
            )
        })?;
    if claim.bsd_name().as_str() != expected_bsd_name
        || claim.device_path() != candidate.device_path
        || claim.raw_device_path() != candidate.raw_device_path
    {
        miette::bail!(
            "The active Disk Arbitration claim no longer identifies selected disk '{}'; no bytes were written.",
            candidate.scan_id
        );
    }

    let expected_rdev = candidate.raw_device_rdev.ok_or_else(|| {
        miette::miette!(
            "Selected disk '{}' has no native device-node identity. Re-run SD scan; no bytes were written.",
            candidate.scan_id
        )
    })?;
    if raw_device_rdev(claim.raw_device_path(), candidate.platform)? != expected_rdev
        || raw_device_rdev_from_file(target.file()?, candidate.platform)? != expected_rdev
    {
        miette::bail!(
            "Raw disk '{}' changed after claimed open. No bytes were written.",
            candidate.raw_device_path.display()
        );
    }

    let current_candidates = scan_macos()?;
    let current = current_candidates
        .iter()
        .filter(|observed| observed.scan_id == candidate.scan_id)
        .collect::<Vec<_>>();
    let current = match current.as_slice() {
        [observed] => *observed,
        [] => {
            miette::bail!(
                "Selected disk '{}' is no longer an entirely unmounted safe macOS target while claimed. No bytes were written.",
                candidate.scan_id
            );
        }
        _ => {
            miette::bail!(
                "Selected disk '{}' is ambiguous while claimed; no bytes were written.",
                candidate.scan_id
            );
        }
    };
    current.validate_for_write(1)?;
    if current != candidate {
        miette::bail!(
            "Selected disk '{}' changed during claimed-open revalidation. No bytes were written.",
            candidate.scan_id
        );
    }
    Ok(())
}

/// The raw macOS path is protected by the whole-disk claim; these flags still
/// reject symlink substitution and descriptor inheritance.
#[cfg(any(target_os = "macos", test))]
fn macos_raw_open_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC
}

/// Linux-only raw open that closes the scan-to-open gap as far as the kernel
/// permits: `O_NOFOLLOW` rejects path substitution and `O_EXCL` asks the block
/// layer for exclusive access before a byte can be written.
#[cfg(target_os = "linux")]
fn open_linux_raw_device_exclusively(candidate: &DiskCandidate) -> Result<File> {
    candidate.validate_for_write(1)?;
    if candidate.platform != DiskPlatform::Linux {
        miette::bail!(
            "Selected disk '{}' is not a Linux candidate; refusing to open a raw target.",
            candidate.scan_id
        );
    }
    let expected_raw_path = expected_system_raw_device_path(candidate)?;
    if candidate.raw_device_path != expected_raw_path {
        miette::bail!(
            "Selected disk '{}' has an unexpected raw-device path '{}'.",
            candidate.scan_id,
            candidate.raw_device_path.display()
        );
    }
    let expected_rdev = candidate.raw_device_rdev.ok_or_else(|| {
        miette::miette!(
            "Selected disk '{}' has no native device-node identity. Re-run SD scan; no disk was opened.",
            candidate.scan_id
        )
    })?;
    let current_rdev = raw_device_rdev(&candidate.raw_device_path, candidate.platform)?;
    if current_rdev != expected_rdev {
        miette::bail!(
            "Raw device node '{}' changed after scan. Re-run SD scan; no disk was opened.",
            candidate.raw_device_path.display()
        );
    }

    let raw_fd = rustix::fs::open(
        &candidate.raw_device_path,
        linux_raw_open_flags(),
        rustix::fs::Mode::empty(),
    )
        .map_err(|error| {
            miette::miette!(
                "Could not exclusively open selected raw disk '{}' for verified writing (O_EXCL | O_NOFOLLOW): {error}",
                candidate.raw_device_path.display()
            )
        })?;
    let target = File::from(raw_fd);
    let opened_rdev = raw_device_rdev_from_file(&target, candidate.platform)?;
    if opened_rdev != expected_rdev {
        miette::bail!(
            "Raw disk '{}' changed while it was opened. Refusing to write through this handle.",
            candidate.raw_device_path.display()
        );
    }
    Ok(target)
}

/// The flags are kept in one testable function to make the required kernel
/// exclusivity and path-hardening contract explicit.
#[cfg(any(target_os = "linux", test))]
fn linux_raw_open_flags() -> rustix::fs::OFlags {
    rustix::fs::OFlags::RDWR
        | rustix::fs::OFlags::EXCL
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::CLOEXEC
}

/// Re-read the Linux block topology after the exclusive handle is open.
///
/// A scan before `open(2)` alone is not enough: an automounter could mount a
/// child partition in the intervening window.  The kernel-held `O_EXCL` handle
/// blocks a new conflicting block-device open, while this structured `lsblk`
/// pass rejects any mount that already won the race.  This function is called
/// immediately before the first seek/write operation.
#[cfg(target_os = "linux")]
fn verify_linux_target_safe_after_exclusive_open(
    candidate: &DiskCandidate,
    target: &File,
) -> Result<()> {
    if candidate.platform != DiskPlatform::Linux {
        miette::bail!(
            "Selected disk '{}' is not a Linux candidate; no bytes were written.",
            candidate.scan_id
        );
    }
    let expected_rdev = candidate.raw_device_rdev.ok_or_else(|| {
        miette::miette!(
            "Selected disk '{}' has no native device-node identity. Re-run SD scan; no bytes were written.",
            candidate.scan_id
        )
    })?;
    if raw_device_rdev_from_file(target, candidate.platform)? != expected_rdev {
        miette::bail!(
            "Raw disk '{}' changed after exclusive open. No bytes were written.",
            candidate.raw_device_path.display()
        );
    }

    let current_candidates = scan_linux()?;
    let current = current_candidates
        .iter()
        .filter(|observed| observed.scan_id == candidate.scan_id)
        .collect::<Vec<_>>();
    let current = match current.as_slice() {
        [observed] => *observed,
        [] => {
            miette::bail!(
                "Selected disk '{}' is no longer an entirely unmounted safe Linux target after exclusive open. An automounter may have mounted it; no bytes were written.",
                candidate.scan_id
            );
        }
        _ => {
            miette::bail!(
                "Selected disk '{}' is ambiguous after exclusive open; no bytes were written.",
                candidate.scan_id
            );
        }
    };
    current.validate_for_write(1)?;
    if current.fingerprint != candidate.fingerprint
        || current.device_path != candidate.device_path
        || current.raw_device_path != candidate.raw_device_path
        || current.raw_device_rdev != Some(expected_rdev)
    {
        miette::bail!(
            "Selected disk '{}' changed during exclusive-open revalidation. No bytes were written.",
            candidate.scan_id
        );
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn expected_system_raw_device_path(candidate: &DiskCandidate) -> Result<PathBuf> {
    let device_name = candidate
        .device_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            miette::miette!(
                "Selected disk '{}' has no valid whole-device name.",
                candidate.scan_id
            )
        })?;
    match candidate.platform {
        DiskPlatform::Macos => {
            if !is_macos_whole_disk_identifier(device_name)
                || candidate.device_path != Path::new("/dev").join(device_name)
            {
                miette::bail!(
                    "Selected disk '{}' is not a macOS whole /dev/diskN device.",
                    candidate.scan_id
                );
            }
            Ok(Path::new("/dev").join(format!("r{device_name}")))
        }
        DiskPlatform::Linux => {
            if !is_linux_whole_device_path(&candidate.device_path) {
                miette::bail!(
                    "Selected disk '{}' is not a supported Linux whole removable device.",
                    candidate.scan_id
                );
            }
            Ok(candidate.device_path.clone())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
const fn raw_device_type_matches_platform(
    platform: DiskPlatform,
    is_block_device: bool,
    is_char_device: bool,
) -> bool {
    match platform {
        DiskPlatform::Linux => is_block_device && !is_char_device,
        DiskPlatform::Macos => is_char_device && !is_block_device,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn raw_device_rdev(path: &Path, platform: DiskPlatform) -> Result<u64> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = fs::symlink_metadata(path).map_err(|error| {
        miette::miette!(
            "Could not inspect raw disk node '{}': {error}",
            path.display()
        )
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink()
        || !raw_device_type_matches_platform(
            platform,
            file_type.is_block_device(),
            file_type.is_char_device(),
        )
    {
        miette::bail!(
            "Raw target '{}' is not the required direct {} device node for {}.",
            path.display(),
            match platform {
                DiskPlatform::Linux => "block",
                DiskPlatform::Macos => "character",
            },
            platform.label(),
        );
    }
    let rdev = metadata.rdev();
    if rdev == 0 {
        miette::bail!(
            "Raw target '{}' has no usable device-node identity.",
            path.display()
        );
    }
    Ok(rdev)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn raw_device_rdev(path: &Path, platform: DiskPlatform) -> Result<u64> {
    let _ = platform;
    miette::bail!(
        "Cannot verify raw target '{}' on this unsupported host.",
        path.display()
    )
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn raw_device_rdev_from_file(file: &File, platform: DiskPlatform) -> Result<u64> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = file
        .metadata()
        .map_err(|error| miette::miette!("Could not inspect opened raw disk handle: {error}"))?;
    let file_type = metadata.file_type();
    if !raw_device_type_matches_platform(
        platform,
        file_type.is_block_device(),
        file_type.is_char_device(),
    ) {
        miette::bail!(
            "Opened target is not the required {} device for {}.",
            match platform {
                DiskPlatform::Linux => "block",
                DiskPlatform::Macos => "character",
            },
            platform.label(),
        );
    }
    let rdev = metadata.rdev();
    if rdev == 0 {
        miette::bail!("Opened target has no usable device-node identity.");
    }
    Ok(rdev)
}

#[cfg(test)]
#[path = "sd_disk_tests.rs"]
mod tests;
