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
//! relative paths below it: the JSON manifest and the raw image.  The manifest
//! must contain at least:
//!
//! ```json
//! {
//!   "format_version": 1,
//!   "kind": "aros-pi-sd-image",
//!   "image": {
//!     "filename": "aros-rpi4-usb.img",
//!     "sha256": "<64 lowercase-or-uppercase hexadecimal characters>",
//!     "size_bytes": 67108864
//!   },
//!   "minimum_device_bytes": 67108864
//! }
//! ```
//!
//! The `image.filename` must exactly equal the caller-supplied relative image
//! path.  This prevents a manifest from quietly redirecting a write command to
//! another file in the artifact directory.  Future manifest versions require
//! an explicit review instead of being accepted optimistically.

use super::config::{Board, Transport};
#[cfg(target_os = "macos")]
use aros_macos_disk_claim::{WholeDiskClaim, DEFAULT_CLAIM_TIMEOUT};
use miette::Result;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

const IMAGE_MANIFEST_FORMAT_VERSION: u64 = 1;
const IMAGE_MANIFEST_KIND: &str = "aros-pi-sd-image";
const CONFIRMATION_TOKEN_PREFIX: &str = "aros-sd-write-v1:";
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const UBOOT_USB_ECM_TRANSPORT: &str = "uboot-usb-ecm";

/// A platform on which this module can safely enumerate whole disks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiskPlatform {
    /// Apple Disk Arbitration's `diskutil` view, queried as plist data.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Macos,
    /// Linux block-device topology reported by `lsblk --json`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Linux,
}

impl DiskPlatform {
    const fn label(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }
}

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

/// Immutable board values an SD image must match before a physical disk can be
/// selected.  The value is obtained either from a local [`Board`] via
/// [`board_image_expectation`] or constructed by a test/integration that has
/// an equally strict local board source.
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
            || !is_sha256(&self.fingerprint)
            || !safe_metadata_component(&self.identity)
            || !safe_metadata_component(&self.model)
            || !safe_metadata_component(&self.transport)
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

    fn finish(mut self) -> Result<()> {
        // Close the raw descriptor before releasing the Disk Arbitration
        // claim.  `drop(self)` below preserves the same ordering for all
        // earlier error returns.
        drop(self.file.take());

        #[cfg(target_os = "macos")]
        if let Some(claim) = self.claim.take() {
            claim.release().map_err(|error| {
                miette::miette!(
                    "The SD image was written and read back, but the macOS Disk Arbitration claim could not be cleanly released: {error}"
                )
            })?;
        }

        Ok(())
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
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        miette::miette!(
            "Could not parse SD image manifest '{}' as JSON: {error}",
            manifest_path.display()
        )
    })?;
    let manifest = json_object(&manifest, "SD image manifest")?;
    validate_image_manifest(manifest, &image_relative_path, &image_path)?;
    let board = board_expectation_from_manifest(manifest)?;

    let image = json_object_field(manifest, "image", "SD image manifest")?;
    let image_sha256 = normalized_sha256(
        json_string_field(image, "sha256", "SD image manifest.image")?,
        "SD image manifest.image.sha256",
    )?;
    let image_size_bytes = json_u64_field(image, "size_bytes", "SD image manifest.image")?;
    let minimum_device_bytes =
        json_u64_field(manifest, "minimum_device_bytes", "SD image manifest")?;
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

/// Convert the selected local board profile into the immutable fields an SD
/// image manifest must carry.  In USB-ECM mode an incomplete profile is an
/// error rather than a reason to make a broadly reusable image.
pub fn board_image_expectation(board: &Board) -> Result<BoardImageExpectation> {
    let mut expectation = BoardImageExpectation::new(
        &board.name,
        &board.config.model,
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
#[allow(dead_code)] // Public guard for non-CLI callers; CLI uses the atomic verifier/writer pair.
pub fn validate_artifact_for_board(artifact: &VerifiedImageArtifact, board: &Board) -> Result<()> {
    let expectation = board_image_expectation(board)?;
    validate_artifact_against_expectation(artifact, &expectation)
}

/// Verify image content and bind its board metadata to `board` in one
/// read-only operation.  This is suitable for a `--dry-run` write plan or for
/// showing board-scoped confirmation tokens after `sd scan`.
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

    target.finish()?;

    Ok(WriteReport {
        scan_id: candidate.scan_id.clone(),
        disk_fingerprint: candidate.fingerprint.clone(),
        bytes_written: reverified.image_size_bytes,
        readback_sha256,
    })
}

fn validate_image_manifest(
    manifest: &Map<String, Value>,
    requested_image_relative_path: &Path,
    requested_image_path: &Path,
) -> Result<()> {
    let format_version = json_u64_field(manifest, "format_version", "SD image manifest")?;
    if format_version != IMAGE_MANIFEST_FORMAT_VERSION {
        miette::bail!(
            "SD image manifest has format_version {}, but this aros version supports image format {}.",
            format_version,
            IMAGE_MANIFEST_FORMAT_VERSION
        );
    }
    let kind = json_string_field(manifest, "kind", "SD image manifest")?;
    if kind != IMAGE_MANIFEST_KIND {
        miette::bail!(
            "SD image manifest kind '{}' is not '{}'. A staging manifest is not a raw image manifest.",
            kind,
            IMAGE_MANIFEST_KIND
        );
    }
    let image = json_object_field(manifest, "image", "SD image manifest")?;
    let declared_filename = json_string_field(image, "filename", "SD image manifest.image")?;
    let declared_relative_path =
        safe_relative_path(Path::new(declared_filename), "manifest image.filename")?;
    if declared_relative_path != requested_image_relative_path {
        miette::bail!(
            "SD image manifest declares image '{}', but the explicitly selected image is '{}'.",
            declared_relative_path.display(),
            requested_image_relative_path.display()
        );
    }
    if !requested_image_path.is_file() {
        miette::bail!(
            "Selected SD image '{}' is not a regular file.",
            requested_image_path.display()
        );
    }
    normalized_sha256(
        json_string_field(image, "sha256", "SD image manifest.image")?,
        "SD image manifest.image.sha256",
    )?;
    Ok(())
}

fn board_expectation_from_manifest(manifest: &Map<String, Value>) -> Result<BoardImageExpectation> {
    let board = json_object_field(manifest, "board", "SD image manifest")?;
    let name = json_string_field(board, "name", "SD image manifest.board")?.to_string();
    let model = json_string_field(board, "model", "SD image manifest.board")?.to_string();
    let transport = json_string_field(board, "transport", "SD image manifest.board")?.to_string();
    let usb_ecm_identity = match manifest.get("usb_ecm") {
        None | Some(Value::Null) => None,
        Some(value) => Some(usb_ecm_identity_from_manifest(value)?),
    };
    let expectation = BoardImageExpectation {
        name,
        model,
        transport,
        usb_ecm_identity,
    };
    validate_board_expectation(&expectation, "SD image manifest")?;
    normalized_board_expectation(&expectation, "SD image manifest")
}

fn usb_ecm_identity_from_manifest(value: &Value) -> Result<UsbEcmArtifactIdentity> {
    let identity = json_object(value, "SD image manifest.usb_ecm")?;
    let vendor_id = u16::try_from(json_u64_field(
        identity,
        "vendor_id",
        "SD image manifest.usb_ecm",
    )?)
    .map_err(|_| {
        miette::miette!("SD image manifest.usb_ecm.vendor_id must fit an unsigned 16-bit USB ID.")
    })?;
    let product_id = u16::try_from(json_u64_field(
        identity,
        "product_id",
        "SD image manifest.usb_ecm",
    )?)
    .map_err(|_| {
        miette::miette!("SD image manifest.usb_ecm.product_id must fit an unsigned 16-bit USB ID.")
    })?;
    Ok(UsbEcmArtifactIdentity {
        vendor_id,
        product_id,
        serial: json_string_field(identity, "serial", "SD image manifest.usb_ecm")?.to_string(),
        expected_target_mac: json_string_field(
            identity,
            "expected_target_mac",
            "SD image manifest.usb_ecm",
        )?
        .to_string(),
    })
}

fn validate_board_expectation(expectation: &BoardImageExpectation, label: &str) -> Result<()> {
    for (field, value) in [
        ("name", expectation.name.as_str()),
        ("model", expectation.model.as_str()),
        ("transport", expectation.transport.as_str()),
    ] {
        if !safe_metadata_component(value) {
            miette::bail!(
                "{label}.{field} must be non-empty and contain no surrounding whitespace or control characters."
            );
        }
    }

    match (&expectation.transport[..], &expectation.usb_ecm_identity) {
        (UBOOT_USB_ECM_TRANSPORT, Some(identity)) => {
            normalized_usb_ecm_identity(identity, &format!("{label}.usb_ecm"))?;
        }
        (UBOOT_USB_ECM_TRANSPORT, None) => {
            miette::bail!(
                "{label} uses '{UBOOT_USB_ECM_TRANSPORT}' but lacks a complete USB-ECM identity."
            );
        }
        (_, Some(_)) => {
            miette::bail!(
                "{label} declares usb_ecm identity but transport is '{}', not '{UBOOT_USB_ECM_TRANSPORT}'.",
                expectation.transport
            );
        }
        (_, None) => {}
    }
    Ok(())
}

fn normalized_board_expectation(
    expectation: &BoardImageExpectation,
    label: &str,
) -> Result<BoardImageExpectation> {
    validate_board_expectation(expectation, label)?;
    Ok(BoardImageExpectation {
        name: expectation.name.clone(),
        model: expectation.model.clone(),
        transport: expectation.transport.clone(),
        usb_ecm_identity: expectation
            .usb_ecm_identity
            .as_ref()
            .map(|identity| normalized_usb_ecm_identity(identity, &format!("{label}.usb_ecm")))
            .transpose()?,
    })
}

fn normalized_usb_ecm_identity(
    identity: &UsbEcmArtifactIdentity,
    label: &str,
) -> Result<UsbEcmArtifactIdentity> {
    if identity.vendor_id == 0 || identity.product_id == 0 {
        miette::bail!("{label} must have non-zero USB vendor_id and product_id.");
    }
    if !safe_metadata_component(&identity.serial) {
        miette::bail!(
            "{label}.serial must be non-empty and contain no surrounding whitespace or control characters."
        );
    }
    Ok(UsbEcmArtifactIdentity {
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        serial: identity.serial.clone(),
        expected_target_mac: normalize_unicast_mac(&identity.expected_target_mac, label)?,
    })
}

fn normalize_unicast_mac(value: &str, label: &str) -> Result<String> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
        miette::bail!(
            "{label}.expected_target_mac '{}' must be a six-octet colon-separated MAC address.",
            value
        );
    }
    let mut bytes = [0_u8; 6];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = u8::from_str_radix(part, 16).map_err(|error| {
            miette::miette!(
                "{label}.expected_target_mac '{}' has invalid octet '{}': {error}",
                value,
                part
            )
        })?;
    }
    if bytes == [0; 6] || bytes[0] & 1 != 0 {
        miette::bail!(
            "{label}.expected_target_mac '{}' must be a non-zero unicast MAC address.",
            value
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

fn append_hex_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

fn validate_verified_board_match(
    artifact: &VerifiedImageArtifact,
    expectation: &BoardImageExpectation,
) -> Result<()> {
    let expected = normalized_board_expectation(expectation, "local board expectation")?;
    let actual = &artifact.board;
    if actual.name != expected.name {
        miette::bail!(
            "SD image manifest board.name '{}' does not match selected board '{}'; no disk was opened.",
            actual.name,
            expected.name
        );
    }
    if actual.model != expected.model {
        miette::bail!(
            "SD image manifest board.model '{}' does not match selected board model '{}'; no disk was opened.",
            actual.model,
            expected.model
        );
    }
    if actual.transport != expected.transport {
        miette::bail!(
            "SD image manifest board.transport '{}' does not match selected board transport '{}'; no disk was opened.",
            actual.transport,
            expected.transport
        );
    }
    if actual.usb_ecm_identity != expected.usb_ecm_identity {
        miette::bail!(
            "SD image manifest USB-ECM identity does not match selected board '{}'; no disk was opened.",
            expected.name
        );
    }
    Ok(())
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

fn json_object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| miette::miette!("{label} must be a JSON object."))
}

fn json_object_field<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<&'a Map<String, Value>> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| miette::miette!("{label}.{field} must be a JSON object."))
}

fn json_string_field<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| miette::miette!("{label}.{field} must be a JSON string."))
}

fn json_u64_field(object: &Map<String, Value>, field: &str, label: &str) -> Result<u64> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| miette::miette!("{label}.{field} must be a non-negative JSON integer."))
}

fn normalized_sha256(value: &str, label: &str) -> Result<String> {
    if !is_sha256(value) {
        miette::bail!("{label} must be a 64-character SHA-256 hexadecimal digest.");
    }
    Ok(value.to_ascii_lowercase())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(|error| {
        miette::miette!(
            "Could not open '{}' for SHA-256 verification: {error}",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            miette::miette!(
                "Could not read '{}' for SHA-256 verification: {error}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read).map_err(|error| {
                miette::miette!(
                    "Could not account for '{}' while hashing: {error}",
                    path.display()
                )
            })?)
            .ok_or_else(|| miette::miette!("File '{}' is too large to hash.", path.display()))?;
    }
    Ok((hex_digest(hasher.finalize()), size))
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
    Ok(hex_digest(hasher.finalize()))
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
    Ok(hex_digest(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = digest.as_ref();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn safe_metadata_component(value: &str) -> bool {
    !value.trim().is_empty() && value == value.trim() && !value.chars().any(char::is_control)
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
        || !safe_metadata_component(&identity)
        || !safe_metadata_component(&model)
        || !safe_metadata_component(&transport)
    {
        return None;
    }
    let fingerprint_material = format!(
        "aros-pi-sd-disk-v1\\nplatform={}\\ndevice={}\\nidentity={}\\nsize={}\\nmodel={}\\ntransport={}\\n",
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

fn json_nonempty_string<'a>(object: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| safe_metadata_component(value))
}

fn json_bool_like(object: &Map<String, Value>, field: &str) -> Option<bool> {
    let value = object.get(field)?;
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_u64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        _ => None,
    }
}

fn json_u64_like(object: &Map<String, Value>, field: &str) -> Option<u64> {
    object.get(field)?.as_u64()
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

#[cfg(any(target_os = "linux", test))]
fn linux_model(object: &Map<String, Value>) -> Option<String> {
    let model = json_nonempty_string(object, "model")?;
    let vendor = json_nonempty_string(object, "vendor");
    let display = match vendor {
        Some(vendor) if vendor != model => format!("{vendor} {model}"),
        _ => model.to_string(),
    };
    safe_metadata_component(&display).then_some(display)
}

#[cfg(any(target_os = "linux", test))]
fn linux_identity(object: &Map<String, Value>) -> Option<String> {
    if let Some(serial) = json_nonempty_string(object, "serial") {
        return Some(format!("serial:{serial}"));
    }
    json_nonempty_string(object, "wwn").map(|wwn| format!("wwn:{wwn}"))
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn is_linux_whole_device_path(path: &Path) -> bool {
    let Some(raw) = path.to_str() else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if path.parent() != Some(Path::new("/dev"))
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || raw != format!("/dev/{name}")
    {
        return false;
    }
    let sd_name = name.strip_prefix("sd").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_lowercase())
    });
    let mmc_name = name.strip_prefix("mmcblk").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && (suffix.len() == 1 || !suffix.starts_with('0'))
    });
    sd_name || mmc_name
}

#[cfg(target_os = "linux")]
fn scan_linux() -> Result<Vec<DiskCandidate>> {
    use std::process::Command;

    let output = Command::new("/usr/bin/lsblk")
        .args([
            "--json",
            "--bytes",
            "--paths",
            "--output",
            "NAME,KNAME,PKNAME,PATH,TYPE,SIZE,RM,RO,MOUNTPOINTS,TRAN,SERIAL,WWN,MODEL,VENDOR,HOTPLUG",
        ])
        .output()
        .map_err(|error| {
            miette::miette!(
                "Could not execute /usr/bin/lsblk for safe SD-card discovery: {error}. Refusing to enumerate raw devices."
            )
        })?;
    if !output.status.success() {
        miette::bail!(
            "lsblk did not provide structured block-device data (exit status {}). Refusing to enumerate raw devices.",
            output.status
        );
    }
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
fn macos_whole_disk_identifiers(list: &Value) -> Result<Vec<String>> {
    let object = json_object(list, "diskutil list plist")?;
    let identifiers = object
        .get("AllDisks")
        .and_then(Value::as_array)
        .ok_or_else(|| miette::miette!("diskutil list plist must contain an AllDisks array."))?;
    let mut result = identifiers
        .iter()
        .filter_map(Value::as_str)
        .filter(|identifier| is_macos_whole_disk_identifier(identifier))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    Ok(result)
}

#[cfg(any(target_os = "macos", test))]
fn macos_descendant_identifiers(list: &Value, whole: &str) -> Result<Vec<String>> {
    let object = json_object(list, "diskutil descendant plist")?;
    let identifiers = object
        .get("AllDisks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            miette::miette!("diskutil descendant plist must contain an AllDisks array.")
        })?;
    let mut result = Vec::with_capacity(identifiers.len());
    for value in identifiers {
        let identifier = value
            .as_str()
            .ok_or_else(|| miette::miette!("diskutil descendant identifier must be a string."))?;
        if !is_macos_descendant_identifier(identifier, whole) {
            miette::bail!(
                "diskutil returned descendant '{}' outside selected whole disk '{}'.",
                identifier,
                whole
            );
        }
        result.push(identifier.to_string());
    }
    let original_len = result.len();
    result.sort();
    result.dedup();
    if result.len() != original_len
        || result
            .iter()
            .filter(|identifier| identifier.as_str() == whole)
            .count()
            != 1
    {
        miette::bail!(
            "diskutil returned a duplicate, rootless, or otherwise incomplete descendant topology."
        );
    }
    Ok(result)
}

fn is_macos_whole_disk_identifier(identifier: &str) -> bool {
    identifier.strip_prefix("disk").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && (suffix.len() == 1 || !suffix.starts_with('0'))
    })
}

#[cfg(any(target_os = "macos", test))]
fn is_macos_descendant_identifier(identifier: &str, whole: &str) -> bool {
    if identifier == whole {
        return is_macos_whole_disk_identifier(whole);
    }
    if !is_macos_whole_disk_identifier(whole) {
        return false;
    }
    let Some(mut suffix) = identifier.strip_prefix(whole) else {
        return false;
    };
    let mut segments = 0;
    while let Some(rest) = suffix.strip_prefix('s') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        let number = &rest[..digits];
        if number.len() > 1 && number.starts_with('0') {
            return false;
        }
        suffix = &rest[digits..];
        segments += 1;
    }
    segments > 0 && suffix.is_empty()
}

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
fn macos_transport(object: &Map<String, Value>) -> Option<String> {
    let mut canonical = None;
    for field in ["Protocol", "BusProtocol"] {
        let Some(_) = object.get(field) else {
            continue;
        };
        let value = json_nonempty_string(object, field)?.to_ascii_lowercase();
        let normalized = match value.as_str() {
            "usb" => "usb",
            "sd" | "secure digital" => "sd",
            _ => return None,
        };
        if canonical.is_some_and(|current| current != normalized) {
            return None;
        }
        canonical = Some(normalized);
    }
    canonical.map(ToOwned::to_owned)
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

#[cfg(target_os = "macos")]
fn diskutil_plist_json(arguments: &[&str]) -> Result<Value> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let output = Command::new("/usr/sbin/diskutil")
        .args(arguments)
        .output()
        .map_err(|error| miette::miette!("Could not execute diskutil: {error}"))?;
    if !output.status.success() {
        miette::bail!(
            "diskutil did not provide plist discovery data (exit status {}).",
            output.status
        );
    }

    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            miette::miette!("Could not execute plutil to decode diskutil plist: {error}")
        })?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| miette::miette!("Could not supply diskutil plist bytes to plutil."))?;
    input.write_all(&output.stdout).map_err(|error| {
        miette::miette!("Could not pass diskutil plist bytes to plutil: {error}")
    })?;
    drop(input);
    let converted = child.wait_with_output().map_err(|error| {
        miette::miette!("Could not wait for plutil while decoding diskutil output: {error}")
    })?;
    if !converted.status.success() {
        miette::bail!("plutil could not convert diskutil plist output to JSON.");
    }
    serde_json::from_slice(&converted.stdout)
        .map_err(|error| miette::miette!("Could not parse converted diskutil plist JSON: {error}"))
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
mod tests {
    use super::{
        confirmation_token, macos_candidate_from_info, make_candidate, parse_linux_inventory,
        validate_artifact_against_expectation, validate_artifact_for_board, verify_image_artifact,
        write_verified_image_with_backend, write_verified_image_with_backend_and_expectation,
        BoardImageExpectation, DiskBackend, DiskCandidate, DiskPlatform, OpenedTarget,
        TestTargetGuard, UsbEcmArtifactIdentity,
    };
    use miette::Result;
    use serde_json::{json, Value};
    use std::fs::{self, OpenOptions};
    use std::path::{Path, PathBuf};

    #[derive(Clone)]
    struct FakeBackend {
        candidates: Vec<DiskCandidate>,
        target: PathBuf,
        after_open_safe: bool,
    }

    impl DiskBackend for FakeBackend {
        fn scan(&self) -> Result<Vec<DiskCandidate>> {
            Ok(self.candidates.clone())
        }

        fn open_verified_target(&self, _candidate: &DiskCandidate) -> Result<OpenedTarget> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.target)
                .map_err(|error| miette::miette!("Could not open injected test target: {error}"))?;
            Ok(OpenedTarget::unclaimed(file))
        }

        fn verify_target_safe_after_open(
            &self,
            _candidate: &DiskCandidate,
            _target: &OpenedTarget,
        ) -> Result<()> {
            if self.after_open_safe {
                Ok(())
            } else {
                miette::bail!("Injected post-open mount revalidation failure.");
            }
        }
    }

    struct GuardedFakeBackend {
        candidate: DiskCandidate,
        target: PathBuf,
        guard_drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        after_open_safe: bool,
    }

    impl DiskBackend for GuardedFakeBackend {
        fn scan(&self) -> Result<Vec<DiskCandidate>> {
            Ok(vec![self.candidate.clone()])
        }

        fn open_verified_target(&self, _candidate: &DiskCandidate) -> Result<OpenedTarget> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.target)
                .map_err(|error| {
                    miette::miette!("Could not open injected guarded test target: {error}")
                })?;
            Ok(OpenedTarget::unclaimed_with_test_guard(
                file,
                TestTargetGuard(std::sync::Arc::clone(&self.guard_drops)),
            ))
        }

        fn verify_target_safe_after_open(
            &self,
            _candidate: &DiskCandidate,
            target: &OpenedTarget,
        ) -> Result<()> {
            assert!(target.file().is_ok(), "raw target must still be open");
            assert_eq!(
                self.guard_drops.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "claim-like ownership must still be retained during post-open verification"
            );
            if self.after_open_safe {
                Ok(())
            } else {
                miette::bail!("Injected post-open guarded revalidation failure.");
            }
        }
    }

    fn write_image_artifact(artifact_dir: &Path, image_name: &str, image: &[u8]) {
        fs::create_dir_all(artifact_dir).expect("artifact directory");
        fs::write(artifact_dir.join(image_name), image).expect("image");
        let manifest = json!({
            "format_version": 1,
            "kind": "aros-pi-sd-image",
            "board": {
                "name": "rpi4-lab",
                "model": "rpi4",
                "transport": "uboot-usb-ecm",
            },
            "usb_ecm": {
                "vendor_id": 0x1d6b,
                "product_id": 0x0104,
                "serial": "aros-rpi4-lab-01",
                "expected_target_mac": "02:aa:00:00:00:01",
            },
            "image": {
                "filename": image_name,
                "sha256": super::sha256_hex(image),
                "size_bytes": image.len(),
            },
            "minimum_device_bytes": 4096,
        });
        fs::write(
            artifact_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
        )
        .expect("manifest");
    }

    fn matching_board_expectation() -> BoardImageExpectation {
        BoardImageExpectation::new("rpi4-lab", "rpi4", "uboot-usb-ecm").with_usb_ecm_identity(
            UsbEcmArtifactIdentity {
                vendor_id: 0x1d6b,
                product_id: 0x0104,
                serial: "aros-rpi4-lab-01".to_string(),
                // The comparison intentionally accepts the normal equivalent
                // configured spelling while retaining one canonical value.
                expected_target_mac: "02:AA:00:00:00:01".to_string(),
            },
        )
    }

    fn fake_candidate() -> DiskCandidate {
        make_candidate(
            DiskPlatform::Linux,
            PathBuf::from("/dev/sdb"),
            PathBuf::from("/dev/sdb"),
            8192,
            "serial:test-card-01".to_string(),
            "Test USB Reader".to_string(),
            "usb".to_string(),
        )
        .expect("test candidate")
    }

    fn safe_linux_inventory_node() -> Value {
        json!({
            "name": "/dev/sdb",
            "kname": "/dev/sdb",
            "pkname": null,
            "path": "/dev/sdb",
            "type": "disk",
            "size": 33_554_432_u64,
            "rm": true,
            "hotplug": true,
            "ro": false,
            "mountpoints": [null],
            "tran": "usb",
            "serial": "CARD-STRICT-01",
            "wwn": null,
            "model": "SD Reader",
            "vendor": "ACME",
            "children": [{
                "name": "/dev/sdb1",
                "kname": "/dev/sdb1",
                "pkname": "/dev/sdb",
                "path": "/dev/sdb1",
                "type": "part",
                "size": 33_552_384_u64,
                "rm": true,
                "hotplug": true,
                "ro": false,
                "mountpoints": [null]
            }]
        })
    }

    fn safe_mmc_inventory_node() -> Value {
        json!({
            "name": "/dev/mmcblk0",
            "kname": "mmcblk0",
            "pkname": null,
            "path": "/dev/mmcblk0",
            "type": "disk",
            "size": 33_554_432_u64,
            "rm": true,
            "hotplug": true,
            "ro": false,
            "mountpoints": [null],
            "tran": "mmc",
            "serial": "CARD-MMC-01",
            "wwn": null,
            "model": "SD Card",
            "vendor": "MMC",
            "children": [{
                "name": "/dev/mmcblk0p1",
                "kname": "mmcblk0p1",
                "pkname": "mmcblk0",
                "path": "/dev/mmcblk0p1",
                "type": "part",
                "size": 33_552_384_u64,
                "rm": true,
                "hotplug": true,
                "ro": false,
                "mountpoints": [null]
            }]
        })
    }

    fn renamed_linux_usb_inventory_node(mut value: Value, root_name: &str) -> Value {
        let root_path = format!("/dev/{root_name}");
        let partition_path = format!("{root_path}1");
        let object = value.as_object_mut().expect("root fixture object");
        for field in ["name", "kname", "path"] {
            object.insert(field.to_string(), json!(root_path));
        }
        let child = object["children"][0]
            .as_object_mut()
            .expect("child fixture object");
        for field in ["name", "kname", "path"] {
            child.insert(field.to_string(), json!(partition_path));
        }
        child.insert("pkname".to_string(), json!(root_path));
        value
    }

    fn renamed_macos_disk_info(mut value: Value, identifier: &str) -> Value {
        let object = value.as_object_mut().expect("macOS fixture object");
        object.insert("DeviceIdentifier".to_string(), json!(identifier));
        object.insert(
            "DeviceNode".to_string(),
            json!(format!("/dev/{identifier}")),
        );
        object.insert("ParentWholeDisk".to_string(), json!(identifier));
        value
    }

    fn safe_macos_disk_info() -> Value {
        json!({
            "DeviceIdentifier": "disk7",
            "DeviceNode": "/dev/disk7",
            "ParentWholeDisk": "disk7",
            "WholeDisk": true,
            "Internal": false,
            "Writable": true,
            "Ejectable": true,
            "Removable": true,
            "RemovableMedia": true,
            "MountPoint": null,
            "VirtualOrPhysical": "Physical",
            "Protocol": "USB",
            "SerialNumber": "CARD-STRICT-01",
            "MediaName": "USB SD Reader",
            "Size": 33_554_432_u64,
        })
    }

    fn safe_macos_topology() -> (Vec<String>, Vec<Value>) {
        (
            vec!["disk7".to_string(), "disk7s1".to_string()],
            vec![
                safe_macos_disk_info(),
                json!({
                    "DeviceIdentifier": "disk7s1",
                    "DeviceNode": "/dev/disk7s1",
                    "ParentWholeDisk": "disk7",
                    "WholeDisk": false,
                    "MountPoint": null,
                }),
            ],
        )
    }

    fn without_json_field(mut value: Value, field: &str) -> Value {
        value.as_object_mut().expect("fixture object").remove(field);
        value
    }

    fn with_json_field(mut value: Value, field: &str, replacement: Value) -> Value {
        value
            .as_object_mut()
            .expect("fixture object")
            .insert(field.to_string(), replacement);
        value
    }

    fn without_linux_child_field(mut value: Value, field: &str) -> Value {
        value["children"][0]
            .as_object_mut()
            .expect("child fixture object")
            .remove(field);
        value
    }

    fn with_linux_child_field(mut value: Value, field: &str, replacement: Value) -> Value {
        value["children"][0]
            .as_object_mut()
            .expect("child fixture object")
            .insert(field.to_string(), replacement);
        value
    }

    #[test]
    fn linux_structured_scan_rejects_mounted_and_nonremovable_nodes() {
        let inventory: Value = json!({
            "blockdevices": [
                {
                    "name": "/dev/sdb", "kname": "/dev/sdb", "pkname": null,
                    "path": "/dev/sdb",
                    "type": "disk", "size": 33_554_432_u64, "rm": true, "ro": false,
                    "mountpoints": [null], "tran": "usb", "serial": "CARD-01",
                    "wwn": null, "model": "SD Reader", "vendor": "ACME", "hotplug": true,
                    "children": [{
                        "name": "/dev/sdb1", "kname": "/dev/sdb1", "pkname": "/dev/sdb",
                        "path": "/dev/sdb1",
                        "type": "part", "size": 33_552_384_u64, "rm": true, "ro": false,
                        "mountpoints": [null]
                    }]
                },
                {
                    "name": "/dev/sdc", "kname": "/dev/sdc", "pkname": null,
                    "path": "/dev/sdc",
                    "type": "disk", "size": 33_554_432_u64, "rm": true, "ro": false,
                    "mountpoints": [null], "tran": "usb", "serial": "CARD-02",
                    "wwn": null, "model": "SD Reader", "vendor": "ACME", "hotplug": true,
                    "children": [{
                        "name": "/dev/sdc1", "kname": "/dev/sdc1", "pkname": "/dev/sdc",
                        "path": "/dev/sdc1",
                        "type": "part", "size": 33_552_384_u64, "rm": true, "ro": false,
                        "mountpoints": ["/media/card"]
                    }]
                },
                {
                    "name": "/dev/sda", "kname": "/dev/sda", "pkname": null,
                    "path": "/dev/sda",
                    "type": "disk", "size": 33_554_432_u64, "rm": false, "ro": false,
                    "mountpoints": [null], "tran": "usb", "serial": "SYSTEM-01",
                    "wwn": null, "model": "System", "vendor": "ACME", "hotplug": true
                },
                {
                    "name": "/dev/loop0", "kname": "/dev/loop0", "pkname": null,
                    "path": "/dev/loop0",
                    "type": "loop", "size": 33_554_432_u64, "rm": true, "ro": false,
                    "mountpoints": [null], "tran": "usb", "serial": "LOOP-01",
                    "wwn": null, "model": "Loop", "vendor": "ACME", "hotplug": true
                }
            ]
        });

        let candidates = parse_linux_inventory(&inventory).expect("parse inventory");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].device_path, Path::new("/dev/sdb"));
        assert_eq!(candidates[0].identity, "serial:CARD-01");
        assert!(!candidates[0].mounted);
    }

    #[test]
    fn linux_candidate_requires_every_explicit_removable_safety_signal() {
        let safe = safe_linux_inventory_node();
        let candidate = super::linux_candidate_from_node(&safe).expect("strict safe candidate");
        assert_eq!(candidate.device_path, Path::new("/dev/sdb"));
        let mmc_candidate = super::linux_candidate_from_node(&safe_mmc_inventory_node())
            .expect("strict native MMC candidate");
        assert_eq!(mmc_candidate.device_path, Path::new("/dev/mmcblk0"));
        assert_eq!(mmc_candidate.transport, "mmc");

        let bare_kernel_names = with_linux_child_field(
            with_linux_child_field(
                with_json_field(safe.clone(), "kname", json!("sdb")),
                "kname",
                json!("sdb1"),
            ),
            "pkname",
            json!("sdb"),
        );
        super::linux_candidate_from_node(&bare_kernel_names)
            .expect("lsblk bare kernel-name spelling is accepted and cross-checked");

        let mut rejected = [
            "type",
            "rm",
            "hotplug",
            "ro",
            "mountpoints",
            "tran",
            "name",
            "kname",
            "pkname",
            "path",
            "size",
            "model",
        ]
        .into_iter()
        .map(|field| {
            (
                format!("missing root {field}"),
                without_json_field(safe.clone(), field),
            )
        })
        .collect::<Vec<_>>();
        rejected.extend([
            (
                "non-disk type".to_string(),
                with_json_field(safe.clone(), "type", json!("part")),
            ),
            (
                "rm false".to_string(),
                with_json_field(safe.clone(), "rm", json!(false)),
            ),
            (
                "rm unknown".to_string(),
                with_json_field(safe.clone(), "rm", json!("yes")),
            ),
            (
                "hotplug false".to_string(),
                with_json_field(safe.clone(), "hotplug", json!(false)),
            ),
            (
                "hotplug unknown".to_string(),
                with_json_field(safe.clone(), "hotplug", json!("yes")),
            ),
            (
                "read-only".to_string(),
                with_json_field(safe.clone(), "ro", json!(true)),
            ),
            (
                "unknown read-only state".to_string(),
                with_json_field(safe.clone(), "ro", json!("unknown")),
            ),
            (
                "unsupported transport".to_string(),
                with_json_field(safe.clone(), "tran", json!("sata")),
            ),
            (
                "MMC transport on a SCSI-style path".to_string(),
                with_json_field(safe.clone(), "tran", json!("mmc")),
            ),
            (
                "USB transport on an MMC-style path".to_string(),
                with_json_field(safe_mmc_inventory_node(), "tran", json!("usb")),
            ),
            (
                "whole root has a parent".to_string(),
                with_json_field(safe.clone(), "pkname", json!("sda")),
            ),
            (
                "unknown root parent".to_string(),
                with_json_field(safe.clone(), "pkname", json!(7)),
            ),
            (
                "mounted root".to_string(),
                with_json_field(safe.clone(), "mountpoints", json!(["/media/card"])),
            ),
        ]);

        let mut inconsistent_name = safe.clone();
        inconsistent_name
            .as_object_mut()
            .expect("fixture object")
            .insert("kname".to_string(), json!("/dev/sdc"));
        rejected.push(("inconsistent lsblk names".to_string(), inconsistent_name));

        let mut partition_path = safe.clone();
        for field in ["name", "kname", "path"] {
            partition_path
                .as_object_mut()
                .expect("fixture object")
                .insert(field.to_string(), json!("/dev/sdb1"));
        }
        rejected.push(("partition path".to_string(), partition_path));

        let mut noncanonical_mmc = with_json_field(safe.clone(), "tran", json!("mmc"));
        for field in ["name", "kname", "path"] {
            noncanonical_mmc
                .as_object_mut()
                .expect("fixture object")
                .insert(field.to_string(), json!("/dev/mmcblk01"));
        }
        rejected.push(("non-canonical mmc whole path".to_string(), noncanonical_mmc));

        let mut missing_identity = safe.clone();
        let identity_object = missing_identity.as_object_mut().expect("fixture object");
        identity_object.insert("serial".to_string(), Value::Null);
        identity_object.insert("wwn".to_string(), Value::Null);
        rejected.push(("missing serial and WWN".to_string(), missing_identity));

        for field in ["type", "name", "kname", "pkname", "path", "mountpoints"] {
            rejected.push((
                format!("missing child {field}"),
                without_linux_child_field(safe.clone(), field),
            ));
        }
        rejected.extend([
            (
                "unknown child type".to_string(),
                with_linux_child_field(safe.clone(), "type", json!(7)),
            ),
            (
                "unsupported child type".to_string(),
                with_linux_child_field(safe.clone(), "type", json!("crypt")),
            ),
            (
                "foreign child path".to_string(),
                with_linux_child_field(safe.clone(), "path", json!("/dev/sdc1")),
            ),
            (
                "foreign child name".to_string(),
                with_linux_child_field(safe.clone(), "name", json!("/dev/sdc1")),
            ),
            (
                "foreign child kernel name".to_string(),
                with_linux_child_field(safe.clone(), "kname", json!("/dev/sdc1")),
            ),
            (
                "wrong child parent".to_string(),
                with_linux_child_field(safe.clone(), "pkname", json!("/dev/sdc")),
            ),
            (
                "unknown child parent".to_string(),
                with_linux_child_field(safe.clone(), "pkname", json!(7)),
            ),
            (
                "mounted child".to_string(),
                with_linux_child_field(safe.clone(), "mountpoints", json!(["/media/card"])),
            ),
            (
                "unknown child mount state".to_string(),
                with_linux_child_field(safe.clone(), "mountpoints", json!(7)),
            ),
            (
                "nested child topology".to_string(),
                with_linux_child_field(
                    safe.clone(),
                    "children",
                    json!([{
                        "name": "/dev/sdb2",
                        "kname": "/dev/sdb2",
                        "pkname": "/dev/sdb1",
                        "path": "/dev/sdb2",
                        "type": "part",
                        "mountpoints": [null]
                    }]),
                ),
            ),
        ]);

        let mut noncanonical_partition = safe.clone();
        for field in ["name", "kname", "path"] {
            noncanonical_partition["children"][0]
                .as_object_mut()
                .expect("child fixture object")
                .insert(field.to_string(), json!("/dev/sdb01"));
        }
        rejected.push((
            "non-canonical child partition".to_string(),
            noncanonical_partition,
        ));

        let mut duplicate_child = safe.clone();
        let repeated = duplicate_child["children"][0].clone();
        duplicate_child["children"]
            .as_array_mut()
            .expect("children fixture array")
            .push(repeated);
        rejected.push(("duplicate child".to_string(), duplicate_child));

        let invalid_children = with_json_field(safe.clone(), "children", json!("unknown"));
        rejected.push(("invalid child topology".to_string(), invalid_children));

        for (label, node) in rejected {
            assert!(
                super::linux_candidate_from_node(&node).is_none(),
                "unsafe Linux fixture was accepted: {label}"
            );
        }

        let wwn_only = with_json_field(
            with_json_field(safe, "serial", Value::Null),
            "wwn",
            json!("0x5000000000000001"),
        );
        assert_eq!(
            super::linux_candidate_from_node(&wwn_only)
                .expect("WWN is an accepted persistent identity")
                .identity,
            "wwn:0x5000000000000001"
        );
    }

    #[test]
    fn linux_inventory_hides_every_candidate_with_ambiguous_identity_or_path() {
        let first = safe_linux_inventory_node();
        let second = renamed_linux_usb_inventory_node(first.clone(), "sdc");
        let unique = with_json_field(
            renamed_linux_usb_inventory_node(first.clone(), "sdd"),
            "serial",
            json!("CARD-UNIQUE-03"),
        );
        let candidates = parse_linux_inventory(&json!({
            "blockdevices": [first, second, unique]
        }))
        .expect("ambiguous identity inventory");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].device_path, Path::new("/dev/sdd"));

        let same_path_different_identity =
            with_json_field(first.clone(), "serial", json!("CARD-DIFFERENT-02"));
        assert!(
            parse_linux_inventory(&json!({
                "blockdevices": [first, same_path_different_identity]
            }))
            .expect("ambiguous device path inventory")
            .is_empty(),
            "all candidates sharing a device/raw path must be hidden"
        );
    }

    #[test]
    fn physical_candidate_filter_is_platform_scoped_and_rejects_macos_collisions() {
        let first = macos_candidate_from_info(&safe_macos_disk_info(), true)
            .expect("first macOS candidate");
        let second_info = renamed_macos_disk_info(safe_macos_disk_info(), "disk8");
        let second = macos_candidate_from_info(&second_info, true).expect("second macOS candidate");
        assert!(
            super::retain_unambiguous_physical_candidates(vec![first.clone(), second]).is_empty(),
            "the same persistent identity on two macOS disks must hide both"
        );

        let different_identity_same_path = macos_candidate_from_info(
            &with_json_field(
                safe_macos_disk_info(),
                "SerialNumber",
                json!("CARD-DIFFERENT-02"),
            ),
            true,
        )
        .expect("same path with a different identity");
        assert!(
            super::retain_unambiguous_physical_candidates(vec![
                first.clone(),
                different_identity_same_path,
            ])
            .is_empty(),
            "the same device path must hide every colliding candidate"
        );

        let raw_path_collision = make_candidate(
            DiskPlatform::Macos,
            PathBuf::from("/dev/disk8"),
            first.raw_device_path.clone(),
            first.size_bytes,
            "serial:CARD-RAW-COLLISION".to_string(),
            first.model.clone(),
            first.transport.clone(),
        )
        .expect("synthetic raw-path collision");
        assert!(
            super::retain_unambiguous_physical_candidates(vec![first.clone(), raw_path_collision,])
                .is_empty(),
            "the same raw path must hide every colliding candidate"
        );

        let linux_same_identity = make_candidate(
            DiskPlatform::Linux,
            PathBuf::from("/dev/sdz"),
            PathBuf::from("/dev/sdz"),
            first.size_bytes,
            first.identity.clone(),
            "USB SD Reader".to_string(),
            "usb".to_string(),
        )
        .expect("cross-platform fixture");
        assert_eq!(
            super::retain_unambiguous_physical_candidates(vec![first, linux_same_identity]).len(),
            2,
            "identity uniqueness is scoped by platform"
        );
    }

    #[test]
    fn macos_structured_scan_requires_physical_unmounted_removable_whole_disk() {
        let safe_info = json!({
            "DeviceIdentifier": "disk7",
            "DeviceNode": "/dev/disk7",
            "ParentWholeDisk": "disk7",
            "WholeDisk": true,
            "Internal": false,
            "Writable": true,
            "Ejectable": true,
            "Removable": true,
            "RemovableMedia": true,
            "MountPoint": "",
            "VirtualOrPhysical": "Physical",
            "Protocol": "USB",
            "SerialNumber": "CARD-01",
            "MediaName": "USB SD Reader",
            "Size": 33_554_432_u64,
        });
        let candidate = macos_candidate_from_info(&safe_info, true).expect("safe candidate");
        assert_eq!(candidate.device_path, Path::new("/dev/disk7"));
        assert_eq!(candidate.raw_device_path, Path::new("/dev/rdisk7"));

        let mounted_info = json!({
            "DeviceIdentifier": "disk7",
            "DeviceNode": "/dev/disk7",
            "ParentWholeDisk": "disk7",
            "WholeDisk": true,
            "Internal": false,
            "Writable": true,
            "Ejectable": true,
            "Removable": true,
            "RemovableMedia": true,
            "MountPoint": "",
            "VirtualOrPhysical": "Physical",
            "Protocol": "USB",
            "SerialNumber": "CARD-01",
            "MediaName": "USB SD Reader",
            "Size": 33_554_432_u64,
        });
        assert!(macos_candidate_from_info(&mounted_info, false).is_none());
    }

    #[test]
    fn macos_candidate_rejects_missing_or_unknown_removability_evidence() {
        let safe = safe_macos_disk_info();
        macos_candidate_from_info(&safe, true).expect("strict safe macOS candidate");

        let only_removable = without_json_field(safe.clone(), "RemovableMedia");
        macos_candidate_from_info(&only_removable, true)
            .expect("explicit Removable=true is sufficient");
        let only_removable_media = without_json_field(safe.clone(), "Removable");
        macos_candidate_from_info(&only_removable_media, true)
            .expect("explicit RemovableMedia=true is sufficient");
        let only_bus_protocol = with_json_field(
            without_json_field(safe.clone(), "Protocol"),
            "BusProtocol",
            json!("USB"),
        );
        macos_candidate_from_info(&only_bus_protocol, true)
            .expect("one explicit supported transport field is sufficient");
        let consistent_usb = with_json_field(safe.clone(), "BusProtocol", json!("USB"));
        macos_candidate_from_info(&consistent_usb, true)
            .expect("matching transport fields are accepted");
        let consistent_sd = with_json_field(
            with_json_field(safe.clone(), "Protocol", json!("Secure Digital")),
            "BusProtocol",
            json!("SD"),
        );
        assert_eq!(
            macos_candidate_from_info(&consistent_sd, true)
                .expect("equivalent SD transport spellings are accepted")
                .transport,
            "sd"
        );

        let mut rejected = Vec::new();
        for field in [
            "DeviceIdentifier",
            "DeviceNode",
            "ParentWholeDisk",
            "WholeDisk",
            "Internal",
            "Writable",
            "Ejectable",
            "MountPoint",
            "VirtualOrPhysical",
            "Protocol",
            "SerialNumber",
            "MediaName",
            "Size",
        ] {
            rejected.push((
                format!("missing {field}"),
                without_json_field(safe.clone(), field),
            ));
        }
        for field in ["WholeDisk", "Internal", "Writable", "Ejectable"] {
            rejected.push((
                format!("unknown {field}"),
                with_json_field(safe.clone(), field, json!("unknown")),
            ));
        }

        rejected.extend([
            (
                "not whole".to_string(),
                with_json_field(safe.clone(), "WholeDisk", json!(false)),
            ),
            (
                "wrong parent".to_string(),
                with_json_field(safe.clone(), "ParentWholeDisk", json!("disk8")),
            ),
            (
                "internal".to_string(),
                with_json_field(safe.clone(), "Internal", json!(true)),
            ),
            (
                "not writable".to_string(),
                with_json_field(safe.clone(), "Writable", json!(false)),
            ),
            (
                "not ejectable".to_string(),
                with_json_field(safe.clone(), "Ejectable", json!(false)),
            ),
            (
                "virtual".to_string(),
                with_json_field(safe.clone(), "VirtualOrPhysical", json!("Virtual")),
            ),
            (
                "unsupported protocol".to_string(),
                with_json_field(safe.clone(), "Protocol", json!("SATA")),
            ),
            (
                "mounted root".to_string(),
                with_json_field(safe.clone(), "MountPoint", json!("/Volumes/CARD")),
            ),
            (
                "wrong device node".to_string(),
                with_json_field(safe.clone(), "DeviceNode", json!("/dev/disk8")),
            ),
            (
                "empty serial".to_string(),
                with_json_field(safe.clone(), "SerialNumber", json!("")),
            ),
            (
                "zero size".to_string(),
                with_json_field(safe.clone(), "Size", json!(0)),
            ),
        ]);

        let neither_removable = without_json_field(
            without_json_field(safe.clone(), "Removable"),
            "RemovableMedia",
        );
        rejected.push(("missing removable evidence".to_string(), neither_removable));

        let false_removable = with_json_field(
            with_json_field(safe.clone(), "Removable", json!(false)),
            "RemovableMedia",
            json!(false),
        );
        rejected.push(("explicitly non-removable".to_string(), false_removable));

        let conflicting_removable = with_json_field(
            with_json_field(safe.clone(), "Removable", json!(false)),
            "RemovableMedia",
            json!(true),
        );
        rejected.push((
            "conflicting removable fields".to_string(),
            conflicting_removable,
        ));
        let conflicting_removable_reverse = with_json_field(
            with_json_field(safe.clone(), "Removable", json!(true)),
            "RemovableMedia",
            json!(false),
        );
        rejected.push((
            "reverse conflicting removable fields".to_string(),
            conflicting_removable_reverse,
        ));
        let unknown_with_true_removable = with_json_field(
            with_json_field(safe.clone(), "Removable", json!("unknown")),
            "RemovableMedia",
            json!(true),
        );
        rejected.push((
            "unknown removable field beside true".to_string(),
            unknown_with_true_removable,
        ));

        let unsupported_bus = with_json_field(safe.clone(), "BusProtocol", json!("PCI"));
        rejected.push((
            "unsupported secondary transport".to_string(),
            unsupported_bus,
        ));
        let conflicting_transport = with_json_field(safe.clone(), "BusProtocol", json!("SD"));
        rejected.push((
            "conflicting supported transport fields".to_string(),
            conflicting_transport,
        ));
        let unknown_bus = with_json_field(safe.clone(), "BusProtocol", json!(1));
        rejected.push(("unknown secondary transport".to_string(), unknown_bus));

        let unknown_removable = without_json_field(
            with_json_field(safe, "Removable", json!("unknown")),
            "RemovableMedia",
        );
        rejected.push(("unknown removable state".to_string(), unknown_removable));

        let noncanonical_identifier = with_json_field(
            with_json_field(
                with_json_field(safe_macos_disk_info(), "DeviceIdentifier", json!("disk07")),
                "DeviceNode",
                json!("/dev/disk07"),
            ),
            "ParentWholeDisk",
            json!("disk07"),
        );
        rejected.push((
            "non-canonical whole-disk identifier".to_string(),
            noncanonical_identifier,
        ));

        for (label, info) in rejected {
            assert!(
                macos_candidate_from_info(&info, true).is_none(),
                "unsafe macOS fixture was accepted: {label}"
            );
        }

        assert!(
            macos_candidate_from_info(&safe_macos_disk_info(), false).is_none(),
            "incomplete or mounted descendant topology must reject the whole disk"
        );
    }

    #[test]
    fn macos_descendant_topology_must_be_complete_unique_and_consistent() {
        let root = safe_macos_disk_info();
        let (identifiers, infos) = safe_macos_topology();
        super::macos_candidate_from_inventory(&root, &identifiers, &infos)
            .expect("complete consistent topology");

        let parsed = super::macos_descendant_identifiers(
            &json!({ "AllDisks": ["disk7s1", "disk7"] }),
            "disk7",
        )
        .expect("valid descendant list");
        assert_eq!(parsed, identifiers);

        for (label, list) in [
            ("rootless", json!({ "AllDisks": ["disk7s1"] })),
            (
                "duplicate",
                json!({ "AllDisks": ["disk7", "disk7s1", "disk7s1"] }),
            ),
            ("foreign", json!({ "AllDisks": ["disk7", "disk8s1"] })),
            ("unparseable", json!({ "AllDisks": ["disk7", "disk7foo"] })),
            (
                "non-canonical",
                json!({ "AllDisks": ["disk7", "disk7s01"] }),
            ),
            ("non-string", json!({ "AllDisks": ["disk7", 7] })),
        ] {
            assert!(
                super::macos_descendant_identifiers(&list, "disk7").is_err(),
                "unsafe descendant list was accepted: {label}"
            );
        }

        let mut rejected = vec![
            (
                "missing descendant info".to_string(),
                identifiers.clone(),
                vec![infos[0].clone()],
            ),
            (
                "duplicate descendant info".to_string(),
                identifiers.clone(),
                vec![infos[1].clone(), infos[1].clone()],
            ),
            (
                "rootless identifier set".to_string(),
                vec!["disk7s1".to_string()],
                vec![infos[1].clone()],
            ),
            (
                "duplicate identifier set".to_string(),
                vec!["disk7".to_string(), "disk7".to_string()],
                vec![infos[0].clone(), infos[0].clone()],
            ),
            (
                "foreign identifier set".to_string(),
                vec!["disk7".to_string(), "disk8s1".to_string()],
                infos.clone(),
            ),
        ];

        for (label, field, replacement) in [
            ("wrong parent", "ParentWholeDisk", json!("disk8")),
            ("wrong device node", "DeviceNode", json!("/dev/disk8s1")),
            ("child claims whole-disk status", "WholeDisk", json!(true)),
            ("unknown child whole-disk status", "WholeDisk", json!(7)),
            ("mounted descendant", "MountPoint", json!("/Volumes/CARD")),
            ("unknown mount state", "MountPoint", json!(7)),
            ("foreign info", "DeviceIdentifier", json!("disk8s1")),
        ] {
            let mut changed_infos = infos.clone();
            changed_infos[1] = with_json_field(changed_infos[1].clone(), field, replacement);
            rejected.push((label.to_string(), identifiers.clone(), changed_infos));
        }
        let mut missing_mount = infos.clone();
        missing_mount[1] = without_json_field(missing_mount[1].clone(), "MountPoint");
        rejected.push((
            "missing descendant mount state".to_string(),
            identifiers.clone(),
            missing_mount,
        ));
        let mut missing_whole_disk = infos.clone();
        missing_whole_disk[1] = without_json_field(missing_whole_disk[1].clone(), "WholeDisk");
        rejected.push((
            "missing child whole-disk status".to_string(),
            identifiers.clone(),
            missing_whole_disk,
        ));

        let mut false_root_whole_disk = infos.clone();
        false_root_whole_disk[0] =
            with_json_field(false_root_whole_disk[0].clone(), "WholeDisk", json!(false));
        rejected.push((
            "root denies whole-disk status".to_string(),
            identifiers.clone(),
            false_root_whole_disk,
        ));

        let mut changed_root_infos = infos;
        changed_root_infos[0] = with_json_field(
            changed_root_infos[0].clone(),
            "SerialNumber",
            json!("DIFFERENT-CARD"),
        );
        rejected.push((
            "root changed between inventory reads".to_string(),
            identifiers,
            changed_root_infos,
        ));

        for (label, candidate_identifiers, candidate_infos) in rejected {
            assert!(
                super::macos_candidate_from_inventory(
                    &root,
                    &candidate_identifiers,
                    &candidate_infos,
                )
                .is_none(),
                "unsafe macOS topology was accepted: {label}"
            );
        }
    }

    #[test]
    fn artifact_board_validation_requires_the_exact_usb_ecm_identity() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");

        validate_artifact_against_expectation(&artifact, &matching_board_expectation())
            .expect("matching board expectation");

        let mismatched = BoardImageExpectation::new("rpi4-lab", "rpi4", "uboot-usb-ecm")
            .with_usb_ecm_identity(UsbEcmArtifactIdentity {
                vendor_id: 0x1d6b,
                product_id: 0x0104,
                serial: "a-different-pi".to_string(),
                expected_target_mac: "02:aa:00:00:00:01".to_string(),
            });
        let error = validate_artifact_against_expectation(&artifact, &mismatched)
            .expect_err("identity mismatch must fail");
        assert!(error.to_string().contains("USB-ECM identity"));
    }

    #[test]
    fn local_board_profile_is_converted_and_bound_to_the_artifact() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let boards_path = temporary.path().join("boards.toml");
        fs::write(
            &boards_path,
            r#"
format_version = 1

[boards.rpi4-lab]
model = "rpi4"
transport = "uboot-usb-ecm"

[boards.rpi4-lab.usb_ecm]
host_address = "192.168.101.1"
target_address = "192.168.101.2"

[boards.rpi4-lab.usb_ecm.identity]
vendor_id = 0x1d6b
product_id = 0x0104
serial = "aros-rpi4-lab-01"
expected_target_mac = "02:AA:00:00:00:01"
"#,
        )
        .expect("board profile");
        let board = super::super::config::load_board(Some(&boards_path), "rpi4-lab")
            .expect("local board profile");

        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");

        validate_artifact_for_board(&artifact, &board).expect("matching local board profile");
    }

    #[test]
    fn production_writer_requires_a_matching_local_board_before_any_disk_scan() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let boards_path = temporary.path().join("boards.toml");
        fs::write(
            &boards_path,
            r#"
format_version = 1

[boards.rpi4-lab]
model = "rpi4"
transport = "uboot-usb-ecm"

[boards.rpi4-lab.usb_ecm]
host_address = "192.168.101.1"
target_address = "192.168.101.2"

[boards.rpi4-lab.usb_ecm.identity]
vendor_id = 0x1d6b
product_id = 0x0104
serial = "aros-rpi4-lab-01"
expected_target_mac = "02:AA:00:00:00:01"
"#,
        )
        .expect("board profile");
        let mut wrong_board = super::super::config::load_board(Some(&boards_path), "rpi4-lab")
            .expect("local board profile");
        wrong_board.name = "rpi5-lab".to_string();

        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");

        let error = super::write_verified_image_for_board(
            &artifact,
            &wrong_board,
            "a-deliberate-nonempty-selection",
            "aros-sd-write-v1:test-token",
        )
        .expect_err("production writer must reject a mismatched board before scanning");
        assert!(error.to_string().contains("board.name"));
    }

    #[test]
    fn board_mismatch_stops_before_the_injected_disk_backend_is_scanned() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");
        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, b"still intact").expect("fake target");
        let candidate = fake_candidate();
        let token = confirmation_token(&artifact, &candidate);
        let backend = FakeBackend {
            candidates: vec![candidate.clone()],
            target: target.clone(),
            after_open_safe: true,
        };
        let wrong_board = BoardImageExpectation::new("rpi5-lab", "rpi5", "uboot-usb-ecm")
            .with_usb_ecm_identity(UsbEcmArtifactIdentity {
                vendor_id: 0x1d6b,
                product_id: 0x0104,
                serial: "aros-rpi4-lab-01".to_string(),
                expected_target_mac: "02:aa:00:00:00:01".to_string(),
            });

        let error = write_verified_image_with_backend_and_expectation(
            &artifact,
            &candidate.scan_id,
            &token,
            &wrong_board,
            &backend,
        )
        .expect_err("wrong board must fail before a disk is opened");
        assert!(error.to_string().contains("board.name"));
        assert_eq!(fs::read_to_string(&target).expect("target"), "still intact");
    }

    #[test]
    fn uboot_usb_ecm_image_without_identity_is_not_a_verified_artifact() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let manifest_path = artifact_dir.join("manifest.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
                .expect("manifest JSON");
        manifest
            .as_object_mut()
            .expect("manifest object")
            .insert("usb_ecm".to_string(), Value::Null);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("updated manifest"),
        )
        .expect("updated manifest");

        let error = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect_err("uboot USB-ECM image requires identity");
        assert!(error
            .to_string()
            .contains("lacks a complete USB-ECM identity"));
    }

    #[test]
    fn verified_artifact_and_fake_target_receive_a_checked_write_and_readback() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        let image = b"AROS SD image payload";
        write_image_artifact(&artifact_dir, "aros-rpi4-usb.img", image);
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("aros-rpi4-usb.img"),
        )
        .expect("verified artifact");

        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, vec![0_u8; 8192]).expect("fake target");
        let candidate = fake_candidate();
        let token = confirmation_token(&artifact, &candidate);
        let backend = FakeBackend {
            candidates: vec![candidate.clone()],
            target: target.clone(),
            after_open_safe: true,
        };

        let report =
            write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
                .expect("fake write");
        assert_eq!(
            report.bytes_written,
            u64::try_from(image.len()).expect("image length")
        );
        assert_eq!(report.readback_sha256, super::sha256_hex(image));
        assert_eq!(
            &fs::read(&target).expect("target bytes")[..image.len()],
            image
        );
    }

    #[test]
    fn opened_target_retains_its_guard_through_write_sync_and_readback() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        let image = b"AROS guarded SD image payload";
        write_image_artifact(&artifact_dir, "image.img", image);
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");

        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, vec![0_u8; 8192]).expect("fake target");
        let candidate = fake_candidate();
        let token = confirmation_token(&artifact, &candidate);
        let guard_drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend = GuardedFakeBackend {
            candidate: candidate.clone(),
            target: target.clone(),
            guard_drops: std::sync::Arc::clone(&guard_drops),
            after_open_safe: true,
        };

        let report =
            write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
                .expect("guarded fake write");
        assert_eq!(report.readback_sha256, super::sha256_hex(image));
        assert_eq!(
            &fs::read(&target).expect("target bytes")[..image.len()],
            image
        );
        assert_eq!(
            guard_drops.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "claim-like ownership must be released exactly once after readback"
        );
    }

    #[test]
    fn post_open_failure_closes_target_and_releases_guard_without_writing() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");

        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, b"leave this target alone").expect("fake target");
        let candidate = fake_candidate();
        let token = confirmation_token(&artifact, &candidate);
        let guard_drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend = GuardedFakeBackend {
            candidate: candidate.clone(),
            target: target.clone(),
            guard_drops: std::sync::Arc::clone(&guard_drops),
            after_open_safe: false,
        };

        let error =
            write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
                .expect_err("guarded post-open check must fail");
        assert!(error.to_string().contains("guarded revalidation"));
        assert_eq!(
            fs::read_to_string(&target).expect("untouched target"),
            "leave this target alone"
        );
        assert_eq!(
            guard_drops.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "claim-like ownership must be released exactly once on failure"
        );
    }

    #[test]
    fn post_open_mount_revalidation_failure_happens_before_the_first_write() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");
        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, b"leave this target alone").expect("fake target");
        let candidate = fake_candidate();
        let token = confirmation_token(&artifact, &candidate);
        let backend = FakeBackend {
            candidates: vec![candidate.clone()],
            target: target.clone(),
            after_open_safe: false,
        };

        let error =
            write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
                .expect_err("post-open mount check must stop before writes");
        assert!(error.to_string().contains("post-open mount revalidation"));
        assert_eq!(
            fs::read_to_string(&target).expect("untouched target"),
            "leave this target alone"
        );
    }

    #[test]
    fn linux_raw_open_contract_uses_exclusive_nofollow_readwrite_flags() {
        let flags = super::linux_raw_open_flags();
        assert!(flags.contains(rustix::fs::OFlags::RDWR));
        assert!(flags.contains(rustix::fs::OFlags::EXCL));
        assert!(flags.contains(rustix::fs::OFlags::NOFOLLOW));
        assert!(flags.contains(rustix::fs::OFlags::CLOEXEC));
    }

    #[test]
    fn macos_raw_open_contract_uses_nofollow_readwrite_flags() {
        let flags = super::macos_raw_open_flags();
        assert!(flags.contains(rustix::fs::OFlags::RDWR));
        assert!(flags.contains(rustix::fs::OFlags::NOFOLLOW));
        assert!(flags.contains(rustix::fs::OFlags::CLOEXEC));
        assert!(!flags.contains(rustix::fs::OFlags::EXCL));
    }

    #[test]
    fn raw_device_node_type_must_match_the_selected_platform_exactly() {
        assert!(super::raw_device_type_matches_platform(
            DiskPlatform::Linux,
            true,
            false
        ));
        assert!(super::raw_device_type_matches_platform(
            DiskPlatform::Macos,
            false,
            true
        ));

        for platform in [DiskPlatform::Linux, DiskPlatform::Macos] {
            assert!(!super::raw_device_type_matches_platform(
                platform, false, false
            ));
            assert!(!super::raw_device_type_matches_platform(
                platform, true, true
            ));
        }
        assert!(!super::raw_device_type_matches_platform(
            DiskPlatform::Linux,
            false,
            true
        ));
        assert!(!super::raw_device_type_matches_platform(
            DiskPlatform::Macos,
            true,
            false
        ));
    }

    #[test]
    fn mismatched_token_never_opens_the_injected_target() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");
        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, b"leave this target alone").expect("fake target");
        let candidate = fake_candidate();
        let backend = FakeBackend {
            candidates: vec![candidate.clone()],
            target: target.clone(),
            after_open_safe: true,
        };

        let error = write_verified_image_with_backend(
            &artifact,
            &candidate.scan_id,
            "aros-sd-write-v1:not-a-token",
            &backend,
        )
        .expect_err("token must fail");
        assert!(error.to_string().contains("Confirmation token"));
        assert_eq!(
            fs::read_to_string(&target).expect("untouched target"),
            "leave this target alone"
        );
    }

    #[test]
    fn a_mounted_candidate_is_rejected_before_the_injected_target_is_opened() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let artifact_dir = temporary.path().join("artifact");
        write_image_artifact(&artifact_dir, "image.img", b"payload");
        let artifact = verify_image_artifact(
            &artifact_dir,
            Path::new("manifest.json"),
            Path::new("image.img"),
        )
        .expect("verified artifact");
        let target = temporary.path().join("fake-whole-disk");
        fs::write(&target, b"still intact").expect("fake target");
        let mut candidate = fake_candidate();
        candidate.mounted = true;
        let token = confirmation_token(&artifact, &candidate);
        let backend = FakeBackend {
            candidates: vec![candidate.clone()],
            target: target.clone(),
            after_open_safe: true,
        };

        let error =
            write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
                .expect_err("mounted target must fail");
        assert!(error.to_string().contains("safe, whole, removable"));
        assert_eq!(fs::read_to_string(&target).expect("target"), "still intact");
    }
}
