//! Strict local board-profile schema, validation, and template generation.

use miette::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

const CURRENT_FORMAT_VERSION: u32 = 2;
const DEFAULT_SERIAL_BAUD: u32 = 115_200;
pub(crate) const NETWORK_SERVER_ADDRESS_FIELD: &str = "network.server_address";
pub(crate) const NETWORK_TARGET_ADDRESS_FIELD: &str = "network.target_address";
pub(crate) const USB_ECM_HOST_ADDRESS_FIELD: &str = "usb_ecm.host_address";
pub(crate) const USB_ECM_TARGET_ADDRESS_FIELD: &str = "usb_ecm.target_address";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardsConfig {
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    #[serde(default)]
    pub boards: BTreeMap<String, BoardConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardConfig {
    /// Firmware and artifact contract implemented by the board engine.
    pub backend: BoardBackend,
    /// Supported physical hardware model.
    pub model: BoardModel,
    /// CMake preset used by the shared build path.
    pub preset: String,
    /// Locked cross-toolchain profile for this CMake preset. A board-specific
    /// debug preset can intentionally share the audited `rpi-aarch64` release.
    pub toolchain_preset: String,
    /// The CMake target that produces a deployable board bundle.
    pub build_target: String,
    #[serde(default)]
    pub transport: Transport,
    /// Relative paths are resolved against the AROS-NG checkout.
    #[serde(default)]
    pub artifact_dir: Option<PathBuf>,
    /// Raspberry-Pi-only build inputs. They never apply to another backend.
    #[serde(default)]
    pub raspberry_pi: Option<RaspberryPiConfig>,
    /// OpenSBI/UEFI-only build inputs. They never apply to a Pi backend.
    #[serde(default)]
    pub opensbi_uefi: Option<OpenSbiUefiConfig>,
    /// Must be an absolute, pre-existing local directory when deploying.
    #[serde(default)]
    pub tftp_root: Option<PathBuf>,
    /// Relative directory below `tftp_root`; defaults to the board name.
    #[serde(default)]
    pub tftp_prefix: Option<PathBuf>,
    /// A physical serial device such as `/dev/cu.usbserial-...`.
    #[serde(default)]
    pub serial_device: Option<PathBuf>,
    #[serde(default = "default_serial_baud")]
    pub serial_baud: u32,
    /// Metadata for a future debugger integration; it never configures a
    /// debugger on the user's behalf.
    #[serde(default)]
    pub debug_transport: Option<DebugTransport>,
    /// Descriptive local policy only. The CLI intentionally does not control
    /// power equipment from this field.
    #[serde(default)]
    pub power_control: Option<String>,
    #[serde(default)]
    pub network: Option<NetworkConfig>,
    #[serde(default)]
    pub usb_ecm: Option<UsbEcmConfig>,
}

/// Implemented firmware/artifact families.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BoardBackend {
    RaspberryPi,
    OpensbiUefi,
}

impl std::fmt::Display for BoardBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RaspberryPi => formatter.write_str("raspberry-pi"),
            Self::OpensbiUefi => formatter.write_str("opensbi-uefi"),
        }
    }
}

/// Hardware models with reviewed board-engine contracts.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BoardModel {
    Rpi3,
    Rpi4,
    Rpi5,
    MilkVTitan,
}

impl BoardModel {
    /// Backend which owns this model's boot contract.
    #[must_use]
    pub const fn backend(self) -> BoardBackend {
        match self {
            Self::Rpi3 | Self::Rpi4 | Self::Rpi5 => BoardBackend::RaspberryPi,
            Self::MilkVTitan => BoardBackend::OpensbiUefi,
        }
    }

    /// Stable profile spelling used in manifests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rpi3 => "rpi3",
            Self::Rpi4 => "rpi4",
            Self::Rpi5 => "rpi5",
            Self::MilkVTitan => "milk-v-titan",
        }
    }

    /// Firmware-selected device-tree filename for a Raspberry Pi model.
    #[must_use]
    pub const fn dtb_filename(self) -> Option<&'static str> {
        match self {
            Self::Rpi3 => Some("bcm2710-rpi-3-b-plus.dtb"),
            Self::Rpi4 => Some("bcm2711-rpi-4-b.dtb"),
            Self::Rpi5 => Some("bcm2712-rpi-5-b.dtb"),
            Self::MilkVTitan => None,
        }
    }

    /// Architecture expected in legacy core KOBJs for this model.
    #[must_use]
    pub const fn core_architecture(self) -> Option<CoreArchitecture> {
        match self {
            Self::Rpi3 => Some(CoreArchitecture::Arm),
            Self::Rpi4 | Self::Rpi5 => Some(CoreArchitecture::Aarch64),
            Self::MilkVTitan => None,
        }
    }
}

impl std::fmt::Display for BoardModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// ELF machine contract for the legacy Pi core-link bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreArchitecture {
    Arm,
    Aarch64,
}

/// Inputs needed only by the Raspberry Pi artifact backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaspberryPiConfig {
    /// Pinned firmware DTB selected for this exact model.
    pub dtb_path: PathBuf,
    /// Three legacy-generated kernel/exec/task relocatable objects.
    pub core_kobj_dir: PathBuf,
}

/// Inputs needed only by the OpenSBI/UEFI artifact backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSbiUefiConfig {
    /// Three legacy-generated RISC-V kernel/exec/task relocatable objects.
    pub core_kobj_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    #[default]
    NativeTftp,
    UbootUsbEcm,
    UefiEsp,
}

impl std::fmt::Display for Transport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeTftp => formatter.write_str("native-tftp"),
            Self::UbootUsbEcm => formatter.write_str("uboot-usb-ecm"),
            Self::UefiEsp => formatter.write_str("uefi-esp"),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DebugTransport {
    Uart,
    Jtag,
    Swd,
    None,
}

impl std::fmt::Display for DebugTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uart => formatter.write_str("uart"),
            Self::Jtag => formatter.write_str("jtag"),
            Self::Swd => formatter.write_str("swd"),
            Self::None => formatter.write_str("none"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// Explicit local Ethernet interface for the native-RJ45 service path.
    /// Parsing permits its absence so non-native transports do not need a
    /// dummy value; `aros board serve` requires it for native TFTP.
    #[serde(default)]
    pub interface: Option<String>,
    pub server_address: IpAddr,
    pub target_address: IpAddr,
    /// DHCP subnet mask. If omitted, the isolated lab link defaults to /24.
    #[serde(default)]
    pub subnet_mask: Option<std::net::Ipv4Addr>,
    /// Pi-side Ethernet MAC allowed to receive the board's DHCP lease.
    /// This is required by `aros board serve` for the native-RJ45 path.
    #[serde(default)]
    pub expected_target_mac: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsbEcmConfig {
    pub host_address: IpAddr,
    pub target_address: IpAddr,
    /// DHCP subnet mask. If omitted, the isolated lab link defaults to /24.
    #[serde(default)]
    pub subnet_mask: Option<std::net::Ipv4Addr>,
    /// Stable descriptor identity for a particular USB-ECM gadget. The
    /// dynamic host interface name is deliberately not stored here.
    #[serde(default)]
    pub identity: Option<UsbEcmIdentity>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsbEcmIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub expected_target_mac: String,
}

#[derive(Debug, Clone)]
pub struct Board {
    pub name: String,
    pub config: BoardConfig,
    pub config_path: PathBuf,
}

impl Board {
    /// Resolve the board's build artifact directory against the checkout.
    #[must_use]
    pub fn artifact_dir(&self, repo_root: &Path) -> PathBuf {
        match &self.config.artifact_dir {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => repo_root.join(path),
            None => repo_root
                .join("build")
                .join(&self.config.preset)
                .join("boot")
                .join(self.config.model.as_str()),
        }
    }

    /// Resolve and validate the exact Raspberry Pi device-tree input.
    ///
    /// # Errors
    ///
    /// Returns an error when a Raspberry Pi profile has no readable, regular,
    /// flattened-device-tree input.
    pub fn raspberry_pi_dtb_path(
        &self,
        repo_root: &Path,
        override_path: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        if self.config.backend != BoardBackend::RaspberryPi {
            return Ok(None);
        }
        let pi = self.config.raspberry_pi.as_ref().ok_or_else(|| {
            miette::miette!(
                "Board '{}' uses the raspberry-pi backend but has no [boards.{}.raspberry_pi] inputs.",
                self.name,
                self.name
            )
        })?;
        let raw_path = override_path.map_or_else(|| pi.dtb_path.clone(), Path::to_path_buf);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            repo_root.join(raw_path)
        };
        let metadata = std::fs::metadata(&path).map_err(|error| {
            miette::miette!(
                "Could not access {} dtb_path '{}': {error}",
                self.config.model,
                path.display()
            )
        })?;
        if !metadata.is_file() {
            miette::bail!(
                "{} dtb_path '{}' is not a regular file.",
                self.config.model,
                path.display()
            );
        }
        let canonical_path = path.canonicalize().map_err(|error| {
            miette::miette!(
                "Could not resolve {} dtb_path '{}': {error}",
                self.config.model,
                path.display()
            )
        })?;
        validate_raspberry_pi_dtb(self.config.model, &canonical_path)?;
        Ok(Some(canonical_path))
    }

    /// Resolve and validate the Raspberry Pi legacy core-object directory.
    ///
    /// # Errors
    ///
    /// Returns an error when a Pi profile has no safe directory containing
    /// the complete expected relocatable-object set.
    pub fn raspberry_pi_core_kobj_dir(
        &self,
        repo_root: &Path,
        override_path: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        let Some(architecture) = self.config.model.core_architecture() else {
            return Ok(None);
        };
        let pi = self.config.raspberry_pi.as_ref().ok_or_else(|| {
            miette::miette!(
                "Board '{}' uses the raspberry-pi backend but has no [boards.{}.raspberry_pi] inputs.",
                self.name,
                self.name
            )
        })?;
        let raw_path = override_path.map_or_else(|| pi.core_kobj_dir.clone(), Path::to_path_buf);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            repo_root.join(raw_path)
        };
        let metadata = std::fs::metadata(&path).map_err(|error| {
            miette::miette!(
                "Could not access {} core_kobj_dir '{}': {error}",
                self.config.model,
                path.display()
            )
        })?;
        if !metadata.is_dir() {
            miette::bail!(
                "{} core_kobj_dir '{}' is not a directory.",
                self.config.model,
                path.display()
            );
        }
        for filename in ["kernel_resource.o", "exec_library.o", "task_resource.o"] {
            let object = path.join(filename);
            validate_raspberry_pi_kobj(self.config.model, architecture, &object, &path, filename)?;
        }
        let canonical_path = path.canonicalize().map_err(|error| {
            miette::miette!(
                "Could not resolve {} core_kobj_dir '{}': {error}",
                self.config.model,
                path.display()
            )
        })?;
        Ok(Some(canonical_path))
    }

    /// Resolve and validate the OpenSBI RISC-V core-object directory.
    ///
    /// # Errors
    ///
    /// Returns an error when an OpenSBI/UEFI profile has no complete set of
    /// ELF64 little-endian RISC-V relocatable core objects.
    pub fn opensbi_core_kobj_dir(
        &self,
        repo_root: &Path,
        override_path: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        if self.config.backend != BoardBackend::OpensbiUefi {
            return Ok(None);
        }
        let opensbi = self.config.opensbi_uefi.as_ref().ok_or_else(|| {
            miette::miette!(
                "Board '{}' uses opensbi-uefi but has no [boards.{}.opensbi_uefi] inputs.",
                self.name,
                self.name
            )
        })?;
        let raw_path =
            override_path.map_or_else(|| opensbi.core_kobj_dir.clone(), Path::to_path_buf);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            repo_root.join(raw_path)
        };
        let metadata = std::fs::metadata(&path).map_err(|error| {
            miette::miette!(
                "Could not access {} core_kobj_dir '{}': {error}",
                self.config.model,
                path.display()
            )
        })?;
        if !metadata.is_dir() {
            miette::bail!(
                "{} core_kobj_dir '{}' is not a directory.",
                self.config.model,
                path.display()
            );
        }
        for filename in ["kernel_resource.o", "exec_library.o", "task_resource.o"] {
            validate_relocatable_elf(
                self.config.model,
                &path.join(filename),
                &path,
                filename,
                2,
                [0xf3, 0x00],
                "ELF64 little-endian RISC-V",
            )?;
        }
        path.canonicalize().map(Some).map_err(|error| {
            miette::miette!(
                "Could not resolve {} core_kobj_dir '{}': {error}",
                self.config.model,
                path.display()
            )
        })
    }

    /// Return the configured absolute TFTP root.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile has no safe absolute TFTP root.
    pub fn tftp_root(&self) -> Result<&Path> {
        let root = self.config.tftp_root.as_deref().ok_or_else(|| {
            miette::miette!(
                "Board '{}' has no tftp_root. Add an absolute local directory to '{}'.",
                self.name,
                self.config_path.display()
            )
        })?;
        if !root.is_absolute() {
            miette::bail!(
                "Board '{}' has a relative tftp_root '{}'. Use an absolute path so deploy cannot publish somewhere unexpected.",
                self.name,
                root.display()
            );
        }
        Ok(root)
    }

    /// Resolve the board-specific deployment directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the TFTP root or prefix is missing or unsafe.
    pub fn deployment_dir(&self) -> Result<PathBuf> {
        let prefix = self.tftp_prefix()?;
        Ok(self.tftp_root()?.join(prefix))
    }

    /// Return the validated relative prefix below the TFTP root.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute or traversing prefix.
    pub fn tftp_prefix(&self) -> Result<PathBuf> {
        let prefix = self
            .config
            .tftp_prefix
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.name));
        validate_relative_path(&prefix, "tftp_prefix")?;
        Ok(prefix)
    }

    /// Return the configured absolute serial device path.
    ///
    /// # Errors
    ///
    /// Returns an error when no absolute serial device is configured.
    pub fn serial_device(&self) -> Result<&Path> {
        let device = self.config.serial_device.as_deref().ok_or_else(|| {
            miette::miette!(
                "Board '{}' has no serial_device. Add one to '{}' or pass --device.",
                self.name,
                self.config_path.display()
            )
        })?;
        if !device.is_absolute() {
            miette::bail!(
                "Board '{}' has a relative serial_device '{}'. Use an absolute device path.",
                self.name,
                device.display()
            );
        }
        Ok(device)
    }

    /// Validate the complete local board profile without touching hardware.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions, unsafe paths, incomplete
    /// identities, invalid addresses, or inconsistent build selectors.
    pub fn validate(&self) -> Result<()> {
        validate_board_name(&self.name)?;
        if self.config.model.backend() != self.config.backend {
            miette::bail!(
                "Board '{}' declares backend '{}' but model '{}' requires backend '{}'.",
                self.name,
                self.config.backend,
                self.config.model,
                self.config.model.backend()
            );
        }
        if self.config.serial_baud == 0 {
            miette::bail!("Board '{}' has serial_baud = 0.", self.name);
        }
        crate::validate_profile_name(&self.config.preset, "CMake preset")?;
        crate::validate_profile_name(&self.config.toolchain_preset, "toolchain preset")?;
        if self.config.build_target.trim().is_empty() {
            miette::bail!("Board '{}' has an empty build_target.", self.name);
        }
        if let Some(prefix) = &self.config.tftp_prefix {
            validate_relative_path(prefix, "tftp_prefix")?;
        }
        if let Some(power_control) = &self.config.power_control {
            if power_control.trim().is_empty() {
                miette::bail!("Board '{}' has an empty power_control value.", self.name);
            }
        }
        if let Some(network) = &self.config.network {
            validate_distinct_addresses(
                network.server_address,
                network.target_address,
                NETWORK_SERVER_ADDRESS_FIELD,
                NETWORK_TARGET_ADDRESS_FIELD,
            )?;
            if network
                .interface
                .as_ref()
                .is_some_and(|interface| interface.trim().is_empty())
            {
                miette::bail!("network.interface must not be empty when present.");
            }
            if let Some(mac) = &network.expected_target_mac {
                if parse_unicast_mac(mac).is_none() {
                    miette::bail!(
                        "network.expected_target_mac '{mac}' must be a six-octet unicast MAC address."
                    );
                }
            }
        }
        if let Some(usb_ecm) = &self.config.usb_ecm {
            validate_distinct_addresses(
                usb_ecm.host_address,
                usb_ecm.target_address,
                USB_ECM_HOST_ADDRESS_FIELD,
                USB_ECM_TARGET_ADDRESS_FIELD,
            )?;
            if let Some(identity) = &usb_ecm.identity {
                validate_usb_ecm_identity(identity)?;
            }
        }
        match (
            self.config.backend,
            self.config.model,
            self.config.transport,
        ) {
            (BoardBackend::RaspberryPi, _, Transport::UefiEsp) => {
                miette::bail!(
                    "Board '{}' cannot use transport 'uefi-esp' with the raspberry-pi backend.",
                    self.name
                );
            }
            (BoardBackend::RaspberryPi, model, Transport::UbootUsbEcm)
                if model != BoardModel::Rpi4 =>
            {
                miette::bail!(
                    "Board '{}' uses '{}': uboot-usb-ecm is reviewed only for model 'rpi4'.",
                    self.name,
                    self.config.model
                );
            }
            (BoardBackend::OpensbiUefi, _, transport) if transport != Transport::UefiEsp => {
                miette::bail!(
                    "Board '{}' uses the opensbi-uefi backend but transport '{}' is not a UEFI ESP deployment.",
                    self.name,
                    transport
                );
            }
            _ => {}
        }
        match self.config.backend {
            BoardBackend::RaspberryPi if self.config.raspberry_pi.is_none() => {
                miette::bail!(
                    "Board '{}' needs a [boards.{}.raspberry_pi] table with dtb_path and core_kobj_dir.",
                    self.name,
                    self.name
                );
            }
            BoardBackend::OpensbiUefi if self.config.raspberry_pi.is_some() => {
                miette::bail!(
                    "Board '{}' uses opensbi-uefi and must not declare Raspberry Pi build inputs.",
                    self.name
                );
            }
            _ => {}
        }
        match self.config.backend {
            BoardBackend::RaspberryPi if self.config.opensbi_uefi.is_some() => {
                miette::bail!(
                    "Board '{}' uses raspberry-pi and must not declare OpenSBI/UEFI build inputs.",
                    self.name
                );
            }
            BoardBackend::OpensbiUefi if self.config.opensbi_uefi.is_none() => {
                miette::bail!(
                    "Board '{}' needs a [boards.{}.opensbi_uefi] table with core_kobj_dir.",
                    self.name,
                    self.name
                );
            }
            _ => {}
        }
        Ok(())
    }
}

/// Load one named physical board from the local registry.
///
/// # Errors
///
/// Returns an error when the registry cannot be read or parsed, the board is
/// absent, or its profile fails validation.
pub fn load_board(config_override: Option<&Path>, board_name: &str) -> Result<Board> {
    validate_board_name(board_name)?;
    let config_path = config_override.map_or_else(default_config_path, Path::to_path_buf);
    let contents = std::fs::read_to_string(&config_path).map_err(|error| {
        miette::miette!(
            "Could not read board configuration '{}': {error}. Set --config or AROS_BOARDS_FILE if needed.",
            config_path.display()
        )
    })?;
    let config: BoardsConfig = toml::from_str(&contents).map_err(|error| {
        miette::miette!(
            "Could not parse board configuration '{}': {error}",
            config_path.display()
        )
    })?;
    if config.format_version != CURRENT_FORMAT_VERSION {
        miette::bail!(
            "Board configuration '{}' has format_version = {}, but this aros version supports format_version = {}.",
            config_path.display(),
            config.format_version,
            CURRENT_FORMAT_VERSION
        );
    }
    let board_config = config.boards.get(board_name).cloned().ok_or_else(|| {
        let available = if config.boards.is_empty() {
            "(none)".to_string()
        } else {
            config.boards.keys().cloned().collect::<Vec<_>>().join(", ")
        };
        miette::miette!(
            "Board '{board_name}' is not declared in '{}'. Available boards: {available}.",
            config_path.display()
        )
    })?;

    let board = Board {
        name: board_name.to_string(),
        config: board_config,
        config_path,
    };
    board.validate()?;
    Ok(board)
}

/// Prepared, intentionally incomplete local board-registry template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardTemplate {
    path: PathBuf,
    board_name: String,
    contents: String,
}

impl BoardTemplate {
    /// Destination selected for the local registry.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Local board name embedded in the template.
    #[must_use]
    pub fn board_name(&self) -> &str {
        &self.board_name
    }

    /// Complete TOML document that will be created.
    #[must_use]
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// Prepare an intentionally incomplete USB-ECM board profile without writing.
///
/// # Errors
///
/// Returns an error for an invalid board name or destination.
pub fn prepare_template(config_override: Option<&Path>, board_name: &str) -> Result<BoardTemplate> {
    validate_board_name(board_name)?;
    let path = config_override.map_or_else(default_config_path, Path::to_path_buf);
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        miette::bail!("Board configuration destination must name a file.");
    }
    Ok(BoardTemplate {
        path,
        board_name: board_name.to_string(),
        contents: board_template(board_name),
    })
}

/// Create a prepared registry without merging or replacing any existing file.
///
/// # Errors
///
/// Returns an error when the registry already exists or its parent directory,
/// atomic file creation, write, or synchronization fails.
pub fn create_template(template: &BoardTemplate) -> Result<()> {
    let path = template.path();

    if path.exists() {
        miette::bail!(
            "Refusing to overwrite existing board configuration '{}'. Add the board manually or choose a new --config file.",
            path.display()
        );
    }
    let parent = path.parent().ok_or_else(|| {
        miette::miette!(
            "Board configuration path '{}' has no parent directory.",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        miette::miette!(
            "Could not create board configuration directory '{}': {error}",
            parent.display()
        )
    })?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            miette::miette!(
                "Could not create board configuration '{}': {error}",
                path.display()
            )
        })?;
    file.write_all(template.contents().as_bytes())
        .map_err(|error| {
            miette::miette!(
                "Could not write board configuration '{}': {error}",
                path.display()
            )
        })?;
    file.sync_all().map_err(|error| {
        miette::miette!(
            "Could not persist board configuration '{}': {error}",
            path.display()
        )
    })?;
    Ok(())
}

#[must_use]
pub fn default_config_path() -> PathBuf {
    default_config_path_from(
        std::env::var_os("AROS_BOARDS_FILE"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

#[must_use]
pub fn default_config_path_from(
    configured_file: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> PathBuf {
    if let Some(path) = configured_file {
        return PathBuf::from(path);
    }
    if let Some(path) = xdg_config_home {
        return PathBuf::from(path).join("aros/boards.toml");
    }
    if let Some(path) = home {
        return PathBuf::from(path).join(".config/aros/boards.toml");
    }
    PathBuf::from(".aros/boards.toml")
}

fn board_template(board_name: &str) -> String {
    format!(
        r#"# Local AROS board profile. This file contains host-specific data;
# do not commit it to AROS-NG.
#
# First: connect the Pi's U-Boot USB-ECM gadget, run `aros board scan`, then
# replace the USB descriptor values and Pi-side gadget MAC below.

format_version = 2

[boards.{board_name}]
backend = "raspberry-pi"
model = "rpi4"
preset = "rpi4-aarch64-debug"
toolchain_preset = "rpi-aarch64"
build_target = "rpi-artifacts"
transport = "uboot-usb-ecm"
artifact_dir = "build/rpi4-aarch64-debug/boot/rpi4"
tftp_root = "/REPLACE_ME/aros-tftp"
tftp_prefix = "{board_name}/current"
serial_device = "/dev/REPLACE_ME"
serial_baud = 115200
debug_transport = "jtag"
power_control = "manual"

[boards.{board_name}.raspberry_pi]
dtb_path = "/REPLACE_ME/bcm2711-rpi-4-b.dtb"
core_kobj_dir = "/REPLACE_ME/legacy-build/bin/raspi-aarch64/gen/kobjs"

[boards.{board_name}.usb_ecm]
# Use private lab addresses that are already configured on the selected USB
# interface. `aros board serve` refuses wildcard or wrong-interface addresses.
host_address = "192.168.77.1"
target_address = "192.168.77.2"
subnet_mask = "255.255.255.0"

[boards.{board_name}.usb_ecm.identity]
# Stable USB descriptor identity from `aros board scan`.
vendor_id = 0xffff # REPLACE_ME
product_id = 0xffff # REPLACE_ME
serial = "REPLACE_ME"
# Pi/U-Boot CDC-ECM MAC, never the host interface MAC.
expected_target_mac = "02:aa:00:00:00:01"
"#
    )
}

const fn default_format_version() -> u32 {
    CURRENT_FORMAT_VERSION
}

const fn default_serial_baud() -> u32 {
    DEFAULT_SERIAL_BAUD
}

fn validate_board_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });
    if !valid {
        miette::bail!(
            "Invalid board name '{name}'. Board names may contain only ASCII letters, digits, '-', '_' and '.'."
        );
    }
    Ok(())
}

fn validate_relative_path(path: &Path, field_name: &str) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        miette::bail!("{field_name} must be a non-empty relative path.");
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            miette::bail!("{field_name} must not contain '.' or '..' path components.");
        }
    }
    Ok(())
}

fn validate_distinct_addresses(
    first: IpAddr,
    second: IpAddr,
    first_name: &str,
    second_name: &str,
) -> Result<()> {
    if first == second {
        miette::bail!("{first_name} and {second_name} must not be the same address.");
    }
    Ok(())
}

fn validate_usb_ecm_identity(identity: &UsbEcmIdentity) -> Result<()> {
    if identity.vendor_id == 0 || identity.product_id == 0 {
        miette::bail!("usb_ecm.identity vendor_id and product_id must both be non-zero.");
    }
    if identity.serial.trim().is_empty() {
        miette::bail!("usb_ecm.identity serial must not be empty.");
    }
    let octets = parse_unicast_mac(&identity.expected_target_mac).ok_or_else(|| {
        miette::miette!(
            "usb_ecm.identity expected_target_mac '{}' must be a six-octet unicast MAC address.",
            identity.expected_target_mac
        )
    })?;
    if octets == [0; 6] {
        miette::bail!("usb_ecm.identity expected_target_mac must not be all zeroes.");
    }
    Ok(())
}

/// Parse one unicast, non-zero MAC address.
#[must_use]
pub fn parse_unicast_mac(value: &str) -> Option<[u8; 6]> {
    let pieces = value.split(':').collect::<Vec<_>>();
    if pieces.len() != 6 {
        return None;
    }
    let mut octets = [0_u8; 6];
    for (index, piece) in pieces.iter().enumerate() {
        if piece.len() != 2 {
            return None;
        }
        octets[index] = u8::from_str_radix(piece, 16).ok()?;
    }
    (octets[0] & 1 == 0).then_some(octets)
}

fn validate_raspberry_pi_kobj(
    model: BoardModel,
    architecture: CoreArchitecture,
    object: &Path,
    directory: &Path,
    filename: &str,
) -> Result<()> {
    let (elf_class, machine, description) = match architecture {
        CoreArchitecture::Arm => (1, [0x28, 0x00], "ELF32 little-endian ARM"),
        CoreArchitecture::Aarch64 => (2, [0xb7, 0x00], "ELF64 little-endian AArch64"),
    };
    validate_relocatable_elf(
        model,
        object,
        directory,
        filename,
        elf_class,
        machine,
        description,
    )
}

fn validate_relocatable_elf(
    model: BoardModel,
    object: &Path,
    directory: &Path,
    filename: &str,
    elf_class: u8,
    machine: [u8; 2],
    description: &str,
) -> Result<()> {
    let metadata = std::fs::metadata(object).map_err(|error| {
        miette::miette!(
            "{} core_kobj_dir '{}' is missing '{}': {error}",
            model,
            directory.display(),
            filename
        )
    })?;
    if !metadata.is_file() {
        miette::bail!(
            "{} core KOBJ '{}' is not a regular file.",
            model,
            object.display()
        );
    }
    let mut header = [0_u8; 20];
    std::fs::File::open(object)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| {
            miette::miette!(
                "Could not read the ELF header of {} core KOBJ '{}': {error}",
                model,
                object.display()
            )
        })?;
    let is_expected_relocatable = header[0..4] == [0x7f, b'E', b'L', b'F']
        && header[4] == elf_class
        && header[5] == 1
        && header[16..18] == [1, 0]
        && header[18..20] == machine;
    if !is_expected_relocatable {
        miette::bail!(
            "{} core KOBJ '{}' is not a {} relocatable object.",
            model,
            object.display(),
            description
        );
    }
    Ok(())
}

fn validate_raspberry_pi_dtb(model: BoardModel, path: &Path) -> Result<()> {
    let expected_name = model
        .dtb_filename()
        .ok_or_else(|| miette::miette!("Model '{}' has no Raspberry Pi DTB contract.", model))?;
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name) {
        miette::bail!(
            "{} dtb_path '{}' must name the firmware-selected file '{}'.",
            model,
            path.display(),
            expected_name
        );
    }
    let mut magic = [0_u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| {
            miette::miette!(
                "Could not read the flattened-device-tree header of {} dtb_path '{}': {error}",
                model,
                path.display()
            )
        })?;
    if magic != [0xd0, 0x0d, 0xfe, 0xed] {
        miette::bail!(
            "{} dtb_path '{}' is not a flattened device tree (expected magic d00dfeed).",
            model,
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        create_template, default_config_path_from, load_board, prepare_template, Transport,
    };
    use std::ffi::OsString;

    #[test]
    fn board_config_supports_the_usb_ecm_transport() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path();
        let config = root.join("boards.toml");
        std::fs::write(
            &config,
            format!(
                "format_version = 2\n\n[boards.rpi4]\nbackend = \"raspberry-pi\"\nmodel = \"rpi4\"\npreset = \"rpi4-aarch64-debug\"\ntoolchain_preset = \"rpi-aarch64\"\nbuild_target = \"rpi-artifacts\"\ntransport = \"uboot-usb-ecm\"\nartifact_dir = \"build/rpi4-aarch64-debug/boot/rpi4\"\ntftp_root = \"{}\"\nserial_device = \"/dev/cu.usbserial-test\"\ndebug_transport = \"jtag\"\npower_control = \"manual\"\n\n[boards.rpi4.raspberry_pi]\ndtb_path = \"firmware/bcm2711-rpi-4-b.dtb\"\ncore_kobj_dir = \"legacy-kobjs\"\n\n[boards.rpi4.usb_ecm]\nhost_address = \"192.0.2.1\"\ntarget_address = \"192.0.2.2\"\n",
                root.display()
            ),
        )
        .expect("configuration");

        let board = load_board(Some(&config), "rpi4").expect("board");
        assert_eq!(board.config.transport, Transport::UbootUsbEcm);
        assert_eq!(board.config.preset, "rpi4-aarch64-debug");
        assert_eq!(board.config.serial_baud, 115_200);
        assert_eq!(
            board.deployment_dir().expect("deployment"),
            root.join("rpi4")
        );
    }

    #[test]
    fn config_path_prefers_explicit_environment_override() {
        assert_eq!(
            default_config_path_from(
                Some(OsString::from("/tmp/boards.toml")),
                Some(OsString::from("/xdg")),
                Some(OsString::from("/home/test")),
            ),
            std::path::PathBuf::from("/tmp/boards.toml")
        );
    }

    #[test]
    fn config_path_uses_xdg_then_home() {
        assert_eq!(
            default_config_path_from(
                None,
                Some(OsString::from("/xdg")),
                Some(OsString::from("/home/test")),
            ),
            std::path::PathBuf::from("/xdg/aros/boards.toml")
        );
        assert_eq!(
            default_config_path_from(None, None, Some(OsString::from("/home/test"))),
            std::path::PathBuf::from("/home/test/.config/aros/boards.toml")
        );
    }

    #[test]
    fn initialization_is_dry_until_apply_and_never_overwrites() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("nested/boards.toml");

        let template = prepare_template(Some(&path), "rpi4-usb").expect("template");
        assert!(!path.exists());

        create_template(&template).expect("created template");
        let board = load_board(Some(&path), "rpi4-usb").expect("template parses");
        assert_eq!(board.config.transport, Transport::UbootUsbEcm);
        assert!(create_template(&template).is_err());
    }

    #[test]
    fn rpi4_build_inputs_are_resolved_and_validated_from_the_board_profile() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path();
        let firmware = root.join("firmware");
        let kobjs = root.join("legacy-kobjs");
        std::fs::create_dir_all(&firmware).expect("firmware directory");
        std::fs::create_dir_all(&kobjs).expect("kobj directory");
        std::fs::write(
            firmware.join("bcm2711-rpi-4-b.dtb"),
            [0xd0, 0x0d, 0xfe, 0xed, 0, 0, 0, 0],
        )
        .expect("dtb");
        for filename in ["kernel_resource.o", "exec_library.o", "task_resource.o"] {
            std::fs::write(kobjs.join(filename), valid_aarch64_relocatable_header()).expect("kobj");
        }

        let config = root.join("boards.toml");
        std::fs::write(
            &config,
            "format_version = 2\n\n[boards.rpi4]\nbackend = \"raspberry-pi\"\nmodel = \"rpi4\"\npreset = \"rpi4-aarch64-debug\"\ntoolchain_preset = \"rpi-aarch64\"\nbuild_target = \"rpi-artifacts\"\n\n[boards.rpi4.raspberry_pi]\ndtb_path = \"firmware/bcm2711-rpi-4-b.dtb\"\ncore_kobj_dir = \"legacy-kobjs\"\n",
        )
        .expect("configuration");

        let board = load_board(Some(&config), "rpi4").expect("board");
        assert_eq!(board.config.preset, "rpi4-aarch64-debug");
        assert_eq!(board.config.toolchain_preset, "rpi-aarch64");
        assert_eq!(board.config.build_target, "rpi-artifacts");
        assert_eq!(
            board
                .raspberry_pi_dtb_path(root, None)
                .expect("dtb path")
                .expect("rpi4 dtb"),
            firmware.join("bcm2711-rpi-4-b.dtb").canonicalize().unwrap()
        );
        assert_eq!(
            board
                .raspberry_pi_core_kobj_dir(root, None)
                .expect("kobj dir")
                .expect("rpi4 kobj dir"),
            kobjs.canonicalize().unwrap()
        );
    }

    #[test]
    fn milk_v_titan_uses_only_the_riscv64_opensbi_contract() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path();
        let kobjs = root.join("legacy-kobjs");
        std::fs::create_dir_all(&kobjs).expect("kobj directory");
        for filename in ["kernel_resource.o", "exec_library.o", "task_resource.o"] {
            std::fs::write(kobjs.join(filename), valid_riscv64_relocatable_header()).expect("kobj");
        }

        let config = root.join("boards.toml");
        std::fs::write(
            &config,
            "format_version = 2\n\n[boards.titan]\nbackend = \"opensbi-uefi\"\nmodel = \"milk-v-titan\"\npreset = \"milk-v-titan-riscv64-debug\"\ntoolchain_preset = \"opensbi-riscv64\"\nbuild_target = \"opensbi-uefi-artifacts\"\ntransport = \"uefi-esp\"\n\n[boards.titan.opensbi_uefi]\ncore_kobj_dir = \"legacy-kobjs\"\n",
        )
        .expect("configuration");

        let board = load_board(Some(&config), "titan").expect("board");
        assert!(board
            .raspberry_pi_dtb_path(root, None)
            .expect("no Pi DTB")
            .is_none());
        assert_eq!(
            board
                .opensbi_core_kobj_dir(root, None)
                .expect("OpenSBI KOBJ path")
                .expect("Titan KOBJ directory"),
            kobjs.canonicalize().unwrap()
        );
    }

    #[test]
    fn model_backend_and_transport_mismatches_fail_closed() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let config = temp.path().join("boards.toml");
        std::fs::write(
            &config,
            "format_version = 2\n\n[boards.invalid]\nbackend = \"raspberry-pi\"\nmodel = \"milk-v-titan\"\npreset = \"milk-v-titan-riscv64-debug\"\ntoolchain_preset = \"opensbi-riscv64\"\nbuild_target = \"opensbi-uefi-artifacts\"\ntransport = \"native-tftp\"\n\n[boards.invalid.raspberry_pi]\ndtb_path = \"firmware/invalid.dtb\"\ncore_kobj_dir = \"legacy-kobjs\"\n",
        )
        .expect("configuration");

        let error = load_board(Some(&config), "invalid").expect_err("must reject mismatch");
        assert!(error
            .to_string()
            .contains("requires backend 'opensbi-uefi'"));
    }

    #[test]
    fn usb_ecm_identity_uses_descriptor_values_not_a_dynamic_interface_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let config = temp.path().join("boards.toml");
        std::fs::write(
            &config,
            "format_version = 2\n\n[boards.rpi4-usb]\nbackend = \"raspberry-pi\"\nmodel = \"rpi4\"\npreset = \"rpi4-aarch64-debug\"\ntoolchain_preset = \"rpi-aarch64\"\nbuild_target = \"rpi-artifacts\"\ntransport = \"uboot-usb-ecm\"\n\n[boards.rpi4-usb.raspberry_pi]\ndtb_path = \"firmware/bcm2711-rpi-4-b.dtb\"\ncore_kobj_dir = \"legacy-kobjs\"\n\n[boards.rpi4-usb.usb_ecm]\nhost_address = \"192.0.2.1\"\ntarget_address = \"192.0.2.2\"\n\n[boards.rpi4-usb.usb_ecm.identity]\nvendor_id = 0x1d6b\nproduct_id = 0x0104\nserial = \"aros-rpi4-lab-01\"\nexpected_target_mac = \"02:aa:00:00:00:01\"\n",
        )
        .expect("configuration");

        let board = load_board(Some(&config), "rpi4-usb").expect("board");
        let identity = board
            .config
            .usb_ecm
            .as_ref()
            .and_then(|usb_ecm| usb_ecm.identity.as_ref())
            .expect("USB identity");
        assert_eq!(identity.vendor_id, 0x1d6b);
        assert_eq!(identity.product_id, 0x0104);
        assert_eq!(identity.serial, "aros-rpi4-lab-01");
    }

    fn valid_aarch64_relocatable_header() -> [u8; 20] {
        let mut header = [0_u8; 20];
        header[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        header[4] = 2;
        header[5] = 1;
        header[16..18].copy_from_slice(&[1, 0]);
        header[18..20].copy_from_slice(&[0xb7, 0]);
        header
    }

    fn valid_riscv64_relocatable_header() -> [u8; 20] {
        let mut header = [0_u8; 20];
        header[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        header[4] = 2;
        header[5] = 1;
        header[16..18].copy_from_slice(&[1, 0]);
        header[18..20].copy_from_slice(&[0xf3, 0]);
        header
    }
}
