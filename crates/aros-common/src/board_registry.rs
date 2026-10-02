//! Reviewed board descriptors, independent of local aliases and host devices.
//!
//! A registry is data, never an executable plugin. Parsing validates the whole
//! document before exposing immutable descriptors and binds its exact bytes.

use crate::{sha256_bytes, ArosError, Result, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const REGISTRY_TEXT: &str = include_str!("../../../profiles/boards/registry-v1.toml");
const MAX_REGISTRY_BYTES: usize = 256 * 1024;
const MAX_BOARDS: usize = 128;
const MAX_TRANSPORTS: usize = 16;

/// A stable portable model ID, not an arbitrary local board alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BoardId(String);

impl BoardId {
    /// The validated stable spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BoardId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if !valid_id(&value) {
            return Err("board ID must be 1-64 lowercase ASCII letters, digits or hyphens, starting with a letter or digit".into());
        }
        Ok(Self(value))
    }
}

impl From<BoardId> for String {
    fn from(value: BoardId) -> Self {
        value.0
    }
}

impl std::fmt::Display for BoardId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Immutable validated defaults for one implemented transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardTransportContract {
    record: TransportDocument,
}

impl BoardTransportContract {
    /// Implemented transport capability ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.record.id
    }

    /// Source-owned build preset selected by the template.
    #[must_use]
    pub fn preset(&self) -> &str {
        &self.record.preset
    }

    /// Independently qualified compiler-profile reference.
    #[must_use]
    pub fn toolchain_preset(&self) -> &str {
        &self.record.toolchain_preset
    }

    /// Native artifact-staging target.
    #[must_use]
    pub fn build_target(&self) -> &str {
        &self.record.build_target
    }

    /// Relative build output, never a host device or absolute local path.
    #[must_use]
    pub fn artifact_dir(&self) -> &str {
        &self.record.artifact_dir
    }

    /// Optional independently validated media-profile reference.
    #[must_use]
    pub fn media_profile(&self) -> Option<&str> {
        self.record.media_profile.as_deref()
    }

    /// Default descriptive debug transport; not debugger execution authority.
    #[must_use]
    pub fn debug_transport(&self) -> Option<&str> {
        self.record.debug_transport.as_deref()
    }

    /// Existing KOBJ bridge spelling retained only for configuration migration.
    #[must_use]
    pub fn legacy_core_architecture(&self) -> Option<&str> {
        self.record.legacy_core_architecture.as_deref()
    }
}

/// Validated portable board facts, without devices, network values or commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardContract {
    id: BoardId,
    registry_sha256: Sha256Digest,
    backend: String,
    architecture: String,
    device_tree: Option<String>,
    default_transport: String,
    transports: BTreeMap<String, BoardTransportContract>,
}

impl BoardContract {
    /// Exact raw registry identity retained when this descriptor is cloned.
    #[must_use]
    pub const fn registry_sha256(&self) -> &Sha256Digest {
        &self.registry_sha256
    }

    /// Stable model identity.
    #[must_use]
    pub const fn id(&self) -> &BoardId {
        &self.id
    }

    /// Known engine implementation selected by this descriptor.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Required target architecture, not the running host architecture.
    #[must_use]
    pub fn architecture(&self) -> &str {
        &self.architecture
    }

    /// Firmware-selected DTB basename, when required by the backend.
    #[must_use]
    pub fn device_tree(&self) -> Option<&str> {
        self.device_tree.as_deref()
    }

    /// Deterministic declared conservative transport.
    #[must_use]
    pub fn default_transport(&self) -> &str {
        &self.default_transport
    }

    /// Resolve one declared transport, rejecting unknown or incompatible IDs.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when this board has no such transport.
    pub fn transport(&self, id: &str) -> Result<&BoardTransportContract> {
        self.transports
            .get(id)
            .ok_or_else(|| invalid(self.id.as_str(), &format!("unsupported transport '{id}'")))
    }

    /// Stable transport ordering, independent of TOML document order.
    #[must_use]
    pub fn transports(&self) -> impl ExactSizeIterator<Item = &BoardTransportContract> {
        self.transports.values()
    }
}

/// A completely validated catalog bound to the reviewed document bytes.
#[derive(Debug, Clone)]
pub struct BoardRegistry {
    boards: BTreeMap<BoardId, BoardContract>,
    sha256: Sha256Digest,
}

impl BoardRegistry {
    /// Parse an explicitly supplied document without executing or writing data.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for malformed, oversized, ambiguous or
    /// unsupported registry data. No partial registry is exposed.
    pub fn parse(source: &str, text: &str) -> Result<Self> {
        if text.len() > MAX_REGISTRY_BYTES {
            return Err(invalid(source, "registry exceeds 256 KiB"));
        }
        let document: RegistryDocument =
            toml::from_str(text).map_err(|error| invalid(source, &error.to_string()))?;
        if document.format_version != 1 {
            return Err(invalid(source, "unsupported registry format_version"));
        }
        if document.boards.is_empty() || document.boards.len() > MAX_BOARDS {
            return Err(invalid(source, "registry must contain 1-128 boards"));
        }
        let sha256 = sha256_bytes(text.as_bytes());
        let mut boards = BTreeMap::new();
        for record in document.boards {
            let board = validate_board(source, record, &sha256)?;
            if boards.insert(board.id.clone(), board).is_some() {
                return Err(invalid(source, "duplicate board ID"));
            }
        }
        Ok(Self { boards, sha256 })
    }

    /// Resolve a stable ID; malformed IDs and unknown models are distinct errors.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for an invalid or unregistered model ID.
    pub fn get(&self, id: &str) -> Result<&BoardContract> {
        let id = BoardId::try_from(id.to_owned())
            .map_err(|message| invalid("board selection", &message))?;
        self.boards
            .get(&id)
            .ok_or_else(|| invalid("board selection", &format!("unknown board '{id}'")))
    }

    /// Stable model ordering; it is not a hardware-qualification matrix.
    #[must_use]
    pub fn boards(&self) -> impl ExactSizeIterator<Item = &BoardContract> {
        self.boards.values()
    }

    /// Exact raw registry bytes to bind in a consumer receipt.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

/// Load only the registry embedded in this reviewed tools source revision.
///
/// # Errors
///
/// Returns a configuration error if the embedded registry violates its schema.
pub fn built_in_board_registry() -> Result<BoardRegistry> {
    BoardRegistry::parse("profiles/boards/registry-v1.toml", REGISTRY_TEXT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryDocument {
    format_version: u32,
    boards: Vec<BoardDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardDocument {
    id: BoardId,
    backend: String,
    architecture: String,
    device_tree: Option<String>,
    default_transport: String,
    transports: Vec<TransportDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransportDocument {
    id: String,
    preset: String,
    toolchain_preset: String,
    build_target: String,
    artifact_dir: String,
    media_profile: Option<String>,
    debug_transport: Option<String>,
    legacy_core_architecture: Option<String>,
}

fn validate_board(
    source: &str,
    record: BoardDocument,
    registry_sha256: &Sha256Digest,
) -> Result<BoardContract> {
    let compatible_architecture = match record.backend.as_str() {
        "raspberry-pi" => matches!(record.architecture.as_str(), "arm" | "aarch64"),
        "opensbi-uefi" => record.architecture == "riscv64",
        _ => return Err(invalid(source, "unsupported backend capability")),
    };
    if !compatible_architecture {
        return Err(invalid(source, "backend and architecture are incompatible"));
    }
    if record.backend == "raspberry-pi" {
        if !record.device_tree.as_deref().is_some_and(valid_dtb) {
            return Err(invalid(
                source,
                "Pi device_tree must be a safe DTB basename",
            ));
        }
    } else if record.device_tree.is_some() {
        return Err(invalid(
            source,
            "device_tree is unsupported by this backend",
        ));
    }
    if record.transports.is_empty() || record.transports.len() > MAX_TRANSPORTS {
        return Err(invalid(source, "board must declare 1-16 transports"));
    }
    let mut transports = BTreeMap::new();
    for transport in record.transports {
        validate_transport(source, &record.backend, &transport)?;
        if transports
            .insert(
                transport.id.clone(),
                BoardTransportContract { record: transport },
            )
            .is_some()
        {
            return Err(invalid(source, "duplicate transport ID"));
        }
    }
    if !transports.contains_key(&record.default_transport) {
        return Err(invalid(source, "default_transport is not declared"));
    }
    Ok(BoardContract {
        id: record.id,
        registry_sha256: registry_sha256.clone(),
        backend: record.backend,
        architecture: record.architecture,
        device_tree: record.device_tree,
        default_transport: record.default_transport,
        transports,
    })
}

fn validate_transport(source: &str, backend: &str, record: &TransportDocument) -> Result<()> {
    if !matches!(
        (backend, record.id.as_str()),
        ("raspberry-pi", "native-tftp" | "uboot-usb-ecm") | ("opensbi-uefi", "uefi-esp")
    ) {
        return Err(invalid(source, "unsupported backend/transport capability"));
    }
    for value in [
        &record.preset,
        &record.toolchain_preset,
        &record.build_target,
    ] {
        if !valid_reference(value) {
            return Err(invalid(source, "unsafe target or toolchain reference"));
        }
    }
    if !valid_relative_path(&record.artifact_dir) {
        return Err(invalid(source, "artifact_dir must be a safe relative path"));
    }
    if record
        .media_profile
        .as_deref()
        .is_some_and(|value| !valid_id(value))
    {
        return Err(invalid(source, "invalid media_profile reference"));
    }
    if record
        .debug_transport
        .as_deref()
        .is_some_and(|value| !matches!(value, "jtag" | "swd"))
    {
        return Err(invalid(source, "unsupported debug transport capability"));
    }
    if record
        .legacy_core_architecture
        .as_deref()
        .is_some_and(|value| !valid_reference(value))
    {
        return Err(invalid(source, "unsafe legacy core reference"));
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    value.len() <= 64
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn valid_reference(value: &str) -> bool {
    value.len() <= 128
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

fn valid_dtb(value: &str) -> bool {
    std::path::Path::new(value)
        .extension()
        .is_some_and(|extension| extension == "dtb")
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        })
        && !value.contains("..")
        && !value.starts_with('.')
}

fn valid_relative_path(value: &str) -> bool {
    value.len() <= 512 && value.split('/').all(valid_reference)
}

fn invalid(source: &str, message: &str) -> ArosError {
    ArosError::Configuration {
        file: source.to_owned(),
        message: format!("board registry: {message}"),
    }
}
