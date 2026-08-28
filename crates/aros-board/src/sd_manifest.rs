//! Typed contract for staged board SD image artifacts.
//!
//! The producer and verifier deliberately share these exact types.  Changing
//! the on-disk schema therefore requires one explicit, versioned edit instead
//! of keeping handwritten JSON and loosely typed readers in sync.

use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const KIND: &str = "aros-board-sd-image";
pub const FILE_NAME: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageManifest {
    pub format_version: u32,
    pub kind: String,
    pub board: ManifestBoard,
    pub usb_ecm: Option<ManifestUsbEcmIdentity>,
    pub partition: ManifestPartition,
    pub source_manifest: ManifestSource,
    pub image: ManifestImage,
    pub minimum_device_bytes: u64,
    pub payload: Vec<ManifestPayloadFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBoard {
    pub name: String,
    pub model: String,
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestUsbEcmIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub expected_target_mac: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPartition {
    pub scheme: String,
    pub filesystem: String,
    pub start_lba: u64,
    pub size_bytes: u64,
    pub label: String,
    pub layout_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    pub filename: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestImage {
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPayloadFile {
    pub role: String,
    pub destination: String,
    pub sha256: String,
    pub size_bytes: u64,
}
