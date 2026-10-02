//! Strict local board-profile schema, validation, and template generation.

pub use aros_common::board_registry::BoardId;
use aros_common::board_registry::{built_in_board_registry, BoardContract, BoardRegistry};
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
    pub model: BoardId,
    /// CMake preset used by the shared build path.
    pub preset: String,
    /// Locked cross-toolchain profile for this CMake preset. A board-specific
    /// debug preset can intentionally share the audited `rpi-aarch64` release.
    pub toolchain_preset: String,
    /// The CMake target that produces a deployable board bundle.
    pub build_target: String,
    #[serde(default)]
    pub transport: Transport,
    /// Relative paths are resolved against the selected AROS checkout.
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

/// Resolve a portable ID against the embedded, reviewed catalog only.
///
/// # Errors
///
/// Returns an error for an invalid embedded catalog or an unknown model.
pub fn resolve_board_contract(model: &BoardId) -> Result<BoardContract> {
    let registry = built_in_board_registry().map_err(|error| miette::miette!("{error}"))?;
    registry
        .get(model.as_str())
        .cloned()
        .map_err(|error| miette::miette!("{error}"))
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
        formatter.write_str(self.as_str())
    }
}

impl Transport {
    /// Resolve an implemented transport capability, never an executable plugin.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported capability ID.
    pub fn from_id(id: &str) -> Result<Self> {
        match id {
            "native-tftp" => Ok(Self::NativeTftp),
            "uboot-usb-ecm" => Ok(Self::UbootUsbEcm),
            "uefi-esp" => Ok(Self::UefiEsp),
            _ => miette::bail!("Unsupported board transport capability '{id}'."),
        }
    }
    /// Stable transport spelling used in profiles and command-line output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeTftp => "native-tftp",
            Self::UbootUsbEcm => "uboot-usb-ecm",
            Self::UefiEsp => "uefi-esp",
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
        validate_raspberry_pi_dtb(&self.config.model, &canonical_path)?;
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
        let contract = resolve_board_contract(&self.config.model)?;
        if contract.backend() != "raspberry-pi" {
            return Ok(None);
        }
        let architecture = match contract.architecture() {
            "arm" => CoreArchitecture::Arm,
            "aarch64" => CoreArchitecture::Aarch64,
            _ => miette::bail!("Unsupported Raspberry Pi core architecture."),
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
            validate_raspberry_pi_kobj(&self.config.model, architecture, &object, &path, filename)?;
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
                &self.config.model,
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
        let contract = resolve_board_contract(&self.config.model)?;
        if contract.backend() != self.config.backend.to_string() {
            miette::bail!(
                "Board '{}' declares backend '{}' but model '{}' requires backend '{}'.",
                self.name,
                self.config.backend,
                self.config.model,
                contract.backend()
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
        contract
            .transport(self.config.transport.as_str())
            .map_err(|error| miette::miette!("Board '{}': {error}", self.name))?;
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
    model: BoardId,
    transport: Transport,
    registry_sha256: aros_common::Sha256Digest,
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

    /// Explicit hardware model encoded by the template.
    #[must_use]
    pub const fn model(&self) -> &BoardId {
        &self.model
    }

    /// Explicit boot transport encoded by the template.
    #[must_use]
    pub const fn transport(&self) -> Transport {
        self.transport
    }

    /// Exact catalog identity used to resolve these portable defaults.
    #[must_use]
    pub const fn registry_sha256(&self) -> &aros_common::Sha256Digest {
        &self.registry_sha256
    }

    /// Complete TOML document that will be created.
    #[must_use]
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// Prepare an intentionally incomplete board profile without writing.
///
/// The caller must select the hardware model. Omitting a transport selects the
/// model's conservative default; callers can select another reviewed transport
/// explicitly where the model supports it.
///
/// # Errors
///
/// Returns an error for an invalid board name or destination, an unknown model,
/// an invalid embedded catalog, or an unsupported model/transport pair.
pub fn prepare_template(
    config_override: Option<&Path>,
    board_name: &str,
    model: BoardId,
    transport: Option<Transport>,
) -> Result<BoardTemplate> {
    let registry = built_in_board_registry().map_err(|error| miette::miette!("{error}"))?;
    prepare_template_from_registry(&registry, config_override, board_name, model, transport)
}

fn prepare_template_from_registry(
    registry: &BoardRegistry,
    config_override: Option<&Path>,
    board_name: &str,
    model: BoardId,
    transport: Option<Transport>,
) -> Result<BoardTemplate> {
    validate_board_name(board_name)?;
    let path = config_override.map_or_else(default_config_path, Path::to_path_buf);
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        miette::bail!("Board configuration destination must name a file.");
    }
    let contract = registry
        .get(model.as_str())
        .map_err(|error| miette::miette!("{error}"))?;
    let transport = match transport {
        Some(transport) => transport,
        None => Transport::from_id(contract.default_transport())?,
    };
    if contract.transport(transport.as_str()).is_err() {
        miette::bail!(
            "Model '{}' has no reviewed '{}' template. Supported transports: {}.",
            model,
            transport,
            contract
                .transports()
                .map(aros_common::board_registry::BoardTransportContract::id)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(BoardTemplate {
        path,
        board_name: board_name.to_string(),
        model,
        transport,
        registry_sha256: registry.sha256().clone(),
        contents: board_template(board_name, contract, transport)?,
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

fn board_template(
    board_name: &str,
    contract: &BoardContract,
    transport: Transport,
) -> Result<String> {
    let defaults = contract
        .transport(transport.as_str())
        .map_err(|error| miette::miette!("{error}"))?;
    let debug = defaults.debug_transport().unwrap_or("none");
    let mut text = format!(
        r#"# Local AROS board profile. This file contains host-specific data;
# do not commit it to the AROS source checkout.
# Replace every REPLACE_ME value before using this profile.

format_version = 2

[boards.{board_name}]
backend = "{backend}"
model = "{model}"
preset = "{preset}"
toolchain_preset = "{toolchain_preset}"
build_target = "{build_target}"
transport = "{transport}"
artifact_dir = "{artifact_dir}"
serial_device = "/dev/REPLACE_ME"
serial_baud = 115200
debug_transport = "{debug}"
power_control = "manual"
"#,
        backend = contract.backend(),
        model = contract.id(),
        preset = defaults.preset(),
        toolchain_preset = defaults.toolchain_preset(),
        build_target = defaults.build_target(),
        artifact_dir = defaults.artifact_dir(),
    );
    if transport != Transport::UefiEsp {
        append_template(
            &mut text,
            format_args!(
                "tftp_root = \"/REPLACE_ME/aros-tftp\"\ntftp_prefix = \"{board_name}/current\"\n"
            ),
        )?;
    }
    let legacy_arch = defaults.legacy_core_architecture().unwrap_or("REPLACE_ME");
    match contract.backend() {
        "raspberry-pi" => {
            let dtb = contract.device_tree().ok_or_else(|| miette::miette!(
                "Model '{}' has no Raspberry Pi DTB contract.", contract.id()
            ))?;
            append_template(&mut text, format_args!(
                "\n[boards.{board_name}.raspberry_pi]\ndtb_path = \"/REPLACE_ME/{dtb}\"\ncore_kobj_dir = \"/REPLACE_ME/legacy-build/bin/{legacy_arch}/gen/kobjs\"\n"
            ))?;
        }
        "opensbi-uefi" => append_template(&mut text, format_args!(
            "\n# UEFI ESP only: create a verified image with `aros board sd image`.\n[boards.{board_name}.opensbi_uefi]\ncore_kobj_dir = \"/REPLACE_ME/legacy-build/bin/{legacy_arch}/gen/kobjs\"\n"
        ))?,
        _ => miette::bail!("Unsupported board backend capability."),
    }
    match transport {
        Transport::NativeTftp => append_template(
            &mut text,
            format_args!(
                r#"
[boards.{board_name}.network]
# Native-RJ45 lab example: configure the actual interface, addresses and MAC.
# `aros board serve` validates them before binding DHCP or TFTP.
interface = "REPLACE_ME"
server_address = "192.168.74.1"
target_address = "192.168.74.2"
subnet_mask = "255.255.255.0"
expected_target_mac = "02:aa:00:00:04:01"
"#
            ),
        )?,
        Transport::UbootUsbEcm => append_template(
            &mut text,
            format_args!(
                r#"
[boards.{board_name}.usb_ecm]
# Optional U-Boot USB-ECM: connect the gadget and run `aros board scan`.
# Use private lab addresses already configured on the selected USB interface.
host_address = "192.168.74.1"
target_address = "192.168.74.2"
subnet_mask = "255.255.255.0"

[boards.{board_name}.usb_ecm.identity]
# Replace the stable USB descriptor values; never store a dynamic interface name.
vendor_id = 0xffff # REPLACE_ME
product_id = 0xffff # REPLACE_ME
serial = "REPLACE_ME"
# Pi/U-Boot CDC-ECM MAC, never the host interface MAC.
expected_target_mac = "02:aa:00:00:04:02"
"#
            ),
        )?,
        Transport::UefiEsp => {}
    }
    Ok(text)
}

fn append_template(text: &mut String, arguments: std::fmt::Arguments<'_>) -> Result<()> {
    std::fmt::Write::write_fmt(text, arguments)
        .map_err(|error| miette::miette!("Could not format board template: {error}"))
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
    model: &BoardId,
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
    model: &BoardId,
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

fn validate_raspberry_pi_dtb(model: &BoardId, path: &Path) -> Result<()> {
    let contract = resolve_board_contract(model)?;
    let expected_name = contract
        .device_tree()
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
        create_template, default_config_path_from, load_board, prepare_template, BoardId, Transport,
    };
    use std::ffi::OsString;

    fn model(id: &str) -> BoardId {
        BoardId::try_from(id.to_owned()).expect("valid fixture ID")
    }

    #[test]
    fn a_new_data_descriptor_reaches_the_template_without_a_model_variant() {
        let text = include_str!("../../../profiles/boards/registry-v1.toml")
            .replace("rpi3", "new-pi-board");
        let registry = aros_common::board_registry::BoardRegistry::parse("fixture", &text)
            .expect("data-only board catalog");
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("boards.toml");
        let template = super::prepare_template_from_registry(
            &registry,
            Some(&path),
            "local-alias",
            model("new-pi-board"),
            None,
        )
        .expect("generic transport template");
        assert_eq!(template.model().as_str(), "new-pi-board");
        assert_eq!(template.registry_sha256(), registry.sha256());
        let parsed: super::BoardsConfig = toml::from_str(template.contents()).unwrap();
        let board = &parsed.boards["local-alias"];
        assert_eq!(board.model.as_str(), "new-pi-board");
        assert_eq!(board.preset, "new-pi-board-arm-debug");
        assert_eq!(board.toolchain_preset, "arm-raspi");
        assert!(!path.exists());
        // The fixture is explicitly passed only inside this unit test. It does
        // not introduce discovery or shadow the embedded production catalog.
        assert!(prepare_template(Some(&path), "local-alias", model("new-pi-board"), None).is_err());
    }

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

        let template = prepare_template(
            Some(&path),
            "rpi4-usb",
            model("rpi4"),
            Some(Transport::UbootUsbEcm),
        )
        .expect("template");
        assert!(!path.exists());
        assert_eq!(template.model(), &model("rpi4"));
        assert_eq!(template.transport(), Transport::UbootUsbEcm);

        create_template(&template).expect("created template");
        let board = load_board(Some(&path), "rpi4-usb").expect("template parses");
        assert_eq!(board.config.model, model("rpi4"));
        assert_eq!(board.config.transport, Transport::UbootUsbEcm);
        assert!(create_template(&template).is_err());
    }

    #[test]
    fn templates_cover_each_reviewed_model_and_transport_pair() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let cases = [
            ("pi3-lab", model("rpi3"), Transport::NativeTftp),
            ("pi4-lab", model("rpi4"), Transport::NativeTftp),
            ("pi4-usb", model("rpi4"), Transport::UbootUsbEcm),
            ("pi5-lab", model("rpi5"), Transport::NativeTftp),
            ("titan-lab", model("milk-v-titan"), Transport::UefiEsp),
        ];

        for (name, model, transport) in cases {
            let path = temporary.path().join(format!("{name}.toml"));
            let template = prepare_template(Some(&path), name, model.clone(), Some(transport))
                .expect("reviewed template");
            assert_eq!(template.model(), &model);
            assert_eq!(template.transport(), transport);
            create_template(&template).expect("created template");

            let board = load_board(Some(&path), name).expect("template parses and validates");
            assert_eq!(board.config.model, model);
            assert_eq!(board.config.transport, transport);
        }
    }

    #[test]
    fn templates_reject_unreviewed_model_and_transport_pairs() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let error = prepare_template(
            Some(&temporary.path().join("boards.toml")),
            "pi5-usb",
            model("rpi5"),
            Some(Transport::UbootUsbEcm),
        )
        .expect_err("Pi 5 USB-ECM has no reviewed contract");

        assert!(error
            .to_string()
            .contains("Model 'rpi5' has no reviewed 'uboot-usb-ecm' template"));
    }

    #[test]
    fn template_defaults_are_model_specific() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let cases = [
            (model("rpi3"), Transport::NativeTftp),
            (model("rpi4"), Transport::NativeTftp),
            (model("rpi5"), Transport::NativeTftp),
            (model("milk-v-titan"), Transport::UefiEsp),
        ];

        for (model, transport) in cases {
            let path = temporary.path().join(format!("{}.toml", model.as_str()));
            let template = prepare_template(Some(&path), "local", model.clone(), None)
                .expect("model default template");
            assert_eq!(template.transport(), transport);
        }
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
