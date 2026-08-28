//! Typed contract for staged board SD image artifacts.
//!
//! The producer and verifier deliberately share these exact types.  Changing
//! the on-disk schema therefore requires one explicit, versioned edit instead
//! of keeping handwritten JSON and loosely typed readers in sync.

use serde::{Deserialize, Serialize};

/// Exact schema version understood by the current producer and verifier.
pub const FORMAT_VERSION: u32 = 1;
/// Type discriminator preventing another JSON manifest from being accepted.
pub const KIND: &str = "aros-board-sd-image";
/// Canonical filename of a staged image manifest.
pub const FILE_NAME: &str = "manifest.json";

/// Complete, unknown-field-denying SD image manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageManifest {
    /// Must equal [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Must equal [`KIND`].
    pub kind: String,
    /// Board identity to which the image is bound.
    pub board: ManifestBoard,
    /// Required for USB-ECM transport and forbidden otherwise.
    pub usb_ecm: Option<ManifestUsbEcmIdentity>,
    /// Deterministic partition layout written into the image.
    pub partition: ManifestPartition,
    /// Source boot-bundle manifest used to produce the image.
    pub source_manifest: ManifestSource,
    /// Raw image identity and byte size.
    pub image: ManifestImage,
    /// Smallest physical target on which the image may be written.
    pub minimum_device_bytes: u64,
    /// Complete payload inventory staged into the filesystem.
    pub payload: Vec<ManifestPayloadFile>,
}

/// Stable board fields embedded into one image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBoard {
    /// Local profile or canonical UEFI board name.
    pub name: String,
    /// Supported physical board model.
    pub model: String,
    /// Boot transport used by this image.
    pub transport: String,
}

/// Complete USB gadget identity embedded into a USB-ECM image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestUsbEcmIdentity {
    /// USB vendor ID exposed by U-Boot.
    pub vendor_id: u16,
    /// USB product ID exposed by U-Boot.
    pub product_id: u16,
    /// Unique USB serial exposed by U-Boot.
    pub serial: String,
    /// Pi-side unicast MAC accepted by DHCP.
    pub expected_target_mac: String,
}

/// Partition geometry and filesystem identity used by the image producer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPartition {
    /// Partition-table scheme, currently `mbr`.
    pub scheme: String,
    /// Filesystem type, currently `fat32`.
    pub filesystem: String,
    /// First partition sector in 512-byte logical blocks.
    pub start_lba: u64,
    /// Partition capacity in bytes.
    pub size_bytes: u64,
    /// FAT volume label.
    pub label: String,
    /// Digest of the canonical layout description.
    pub layout_sha256: String,
}

/// Identity of the source boot-bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    /// Portable relative source-manifest filename.
    pub filename: String,
    /// Lowercase SHA-256 of the exact source manifest.
    pub sha256: String,
}

/// Identity of the staged raw image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestImage {
    /// Portable relative image filename.
    pub filename: String,
    /// Lowercase SHA-256 of the exact image bytes.
    pub sha256: String,
    /// Exact image length in bytes.
    pub size_bytes: u64,
}

/// One file staged into the boot filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPayloadFile {
    /// Semantic role required by the board backend.
    pub role: String,
    /// Portable destination path inside the boot filesystem.
    pub destination: String,
    /// Lowercase SHA-256 of the staged file.
    pub sha256: String,
    /// Exact staged-file length in bytes.
    pub size_bytes: u64,
}
