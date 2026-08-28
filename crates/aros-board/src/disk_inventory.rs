//! Shared, fail-closed parsing primitives for removable-media inventories.
//!
//! Writing and unmounting intentionally apply different safety policies, but
//! they consume the same `lsblk`/`diskutil` schema and device-name grammar.
//! Keeping that protocol knowledge here prevents the two destructive paths
//! from drifting as host tools evolve.

use miette::Result;
use serde_json::{Map, Value};
use std::path::Path;

/// Stable absolute path used for every structured `diskutil` operation.
#[cfg(target_os = "macos")]
pub const DISKUTIL_PATH: &str = "/usr/sbin/diskutil";
/// Machine-readable output switch required by every `diskutil` inventory.
#[cfg(target_os = "macos")]
pub const DISKUTIL_PLIST_ARGUMENT: &str = "-plist";
/// Accepted value of `VirtualOrPhysical` for physical removable media.
#[cfg(any(target_os = "macos", test))]
pub const DISKUTIL_PHYSICAL_VALUE: &str = "physical";

/// Field names emitted by `lsblk --json`.
#[cfg(any(target_os = "linux", test))]
pub mod lsblk_field {
    /// Top-level array of block devices.
    pub const BLOCK_DEVICES: &str = "blockdevices";
    /// Optional nested device array.
    pub const CHILDREN: &str = "children";
    /// Kernel hot-plug observation.
    pub const HOTPLUG: &str = "hotplug";
    /// Kernel device name without `/dev`.
    pub const KERNEL_NAME: &str = "kname";
    /// Kernel major/minor device number.
    pub const MAJOR_MINOR: &str = "maj:min";
    /// Complete mount-point array.
    pub const MOUNT_POINTS: &str = "mountpoints";
    /// Name requested in absolute-path mode.
    pub const NAME: &str = "name";
    /// Parent kernel device name.
    pub const PARENT_KERNEL_NAME: &str = "pkname";
    /// Absolute device path.
    pub const PATH: &str = "path";
    /// Read-only flag.
    pub const READ_ONLY: &str = "ro";
    /// Removable-media flag.
    pub const REMOVABLE: &str = "rm";
    /// Device capacity in bytes.
    pub const SIZE: &str = "size";
    /// Host transport identifier.
    pub const TRANSPORT: &str = "tran";
    /// Device node type.
    pub const TYPE: &str = "type";
}

/// Field names emitted by `diskutil -plist`.
#[cfg(any(target_os = "macos", test))]
pub mod diskutil_field {
    /// Complete disk-identifier list.
    pub const ALL_DISKS: &str = "AllDisks";
    /// Alternate transport observation.
    pub const BUS_PROTOCOL: &str = "BusProtocol";
    /// Canonical `diskN` identifier.
    pub const DEVICE_IDENTIFIER: &str = "DeviceIdentifier";
    /// Absolute `/dev/diskN` node.
    pub const DEVICE_NODE: &str = "DeviceNode";
    /// Media-ejectability observation.
    pub const EJECTABLE: &str = "Ejectable";
    /// Internal-device observation.
    pub const INTERNAL: &str = "Internal";
    /// Human-readable media model.
    pub const MEDIA_NAME: &str = "MediaName";
    /// Current volume mount point.
    pub const MOUNT_POINT: &str = "MountPoint";
    /// Owning whole-disk identifier.
    pub const PARENT_WHOLE_DISK: &str = "ParentWholeDisk";
    /// Primary transport observation.
    pub const PROTOCOL: &str = "Protocol";
    /// Removable-media observation.
    pub const REMOVABLE: &str = "Removable";
    /// Alternate removable-media observation.
    pub const REMOVABLE_MEDIA: &str = "RemovableMedia";
    /// Persistent media or reader serial.
    pub const SERIAL_NUMBER: &str = "SerialNumber";
    /// Device capacity in bytes.
    pub const SIZE: &str = "Size";
    /// Physical-versus-virtual observation.
    pub const VIRTUAL_OR_PHYSICAL: &str = "VirtualOrPhysical";
    /// Whole-disk observation.
    pub const WHOLE_DISK: &str = "WholeDisk";
    /// Media-writability observation.
    pub const WRITABLE: &str = "Writable";
}

#[cfg(target_os = "linux")]
pub const LSBLK_PATH: &str = "/usr/bin/lsblk";
#[cfg(target_os = "linux")]
pub const LSBLK_FIELDS: &str =
    "NAME,KNAME,PKNAME,PATH,TYPE,SIZE,RM,RO,MOUNTPOINTS,TRAN,SERIAL,WWN,MODEL,VENDOR,HOTPLUG,MAJ:MIN";

/// Host inventory implementation used to establish a disk identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiskPlatform {
    /// Apple Disk Arbitration's `diskutil` inventory.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Macos,
    /// Linux's `lsblk --json` inventory.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Linux,
}

impl DiskPlatform {
    /// Stable lowercase value used in fingerprints and diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }
}

/// Accept only non-empty, trimmed, control-character-free inventory text.
pub fn safe_metadata(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

/// Require one structured inventory node to be a JSON object.
pub fn json_object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| miette::miette!("{context} must be an object."))
}

/// Read a metadata-safe string field.
pub fn json_nonempty_string<'a>(object: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| safe_metadata(value))
}

/// Read a strict boolean or numeric zero/one field.
pub fn json_bool_like(object: &Map<String, Value>, field: &str) -> Option<bool> {
    match object.get(field)? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_u64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        _ => None,
    }
}

/// Read an unsigned integer field without string coercion.
pub fn json_u64_like(object: &Map<String, Value>, field: &str) -> Option<u64> {
    object.get(field)?.as_u64()
}

#[cfg(any(target_os = "linux", test))]
/// Combine validated Linux vendor and model fields for display.
pub fn linux_model(object: &Map<String, Value>) -> Option<String> {
    let model = json_nonempty_string(object, "model")?;
    let vendor = json_nonempty_string(object, "vendor");
    let display = match vendor {
        Some(vendor) if vendor != model => format!("{vendor} {model}"),
        _ => model.to_string(),
    };
    safe_metadata(&display).then_some(display)
}

#[cfg(any(target_os = "linux", test))]
/// Derive a persistent Linux identity from serial and/or WWN.
pub fn linux_identity(object: &Map<String, Value>) -> Option<String> {
    let serial = json_nonempty_string(object, "serial");
    let wwn = json_nonempty_string(object, "wwn");
    match (serial, wwn) {
        (Some(serial), Some(wwn)) => Some(format!("serial:{serial};wwn:{wwn}")),
        (Some(serial), None) => Some(format!("serial:{serial}")),
        (None, Some(wwn)) => Some(format!("wwn:{wwn}")),
        (None, None) => None,
    }
}

/// Recognize only canonical whole SCSI/USB and MMC device paths.
pub fn is_linux_whole_device_path(path: &Path) -> bool {
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

#[cfg(target_os = "macos")]
/// Extract sorted, unique whole-disk identifiers from a `diskutil` list.
pub fn macos_whole_disk_identifiers(list: &Value) -> Result<Vec<String>> {
    let object = json_object(list, "diskutil list plist")?;
    let identifiers = object
        .get(diskutil_field::ALL_DISKS)
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
/// Validate and extract a complete descendant identifier topology.
pub fn macos_descendant_identifiers(list: &Value, whole: &str) -> Result<Vec<String>> {
    let object = json_object(list, "diskutil descendant plist")?;
    let identifiers = object
        .get(diskutil_field::ALL_DISKS)
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

/// Recognize canonical `diskN` identifiers without leading zeroes.
pub fn is_macos_whole_disk_identifier(identifier: &str) -> bool {
    identifier.strip_prefix("disk").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && (suffix.len() == 1 || !suffix.starts_with('0'))
    })
}

#[cfg(any(target_os = "macos", test))]
/// Recognize canonical descendants of one exact whole disk.
pub fn is_macos_descendant_identifier(identifier: &str, whole: &str) -> bool {
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
/// Reconcile all present `diskutil` transport observations.
pub fn macos_transport(object: &Map<String, Value>) -> Option<String> {
    let mut canonical = None;
    for field in [diskutil_field::PROTOCOL, diskutil_field::BUS_PROTOCOL] {
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

#[cfg(target_os = "linux")]
/// Construct the sole supported structured `lsblk` inventory command.
pub fn linux_inventory_command(absolute_names: bool) -> std::process::Command {
    let mut command = std::process::Command::new(LSBLK_PATH);
    command.args(["--json", "--bytes"]);
    if absolute_names {
        command.arg("--paths");
    }
    command.args(["--output", LSBLK_FIELDS]);
    command
}

#[cfg(target_os = "macos")]
/// Execute `diskutil` and convert its binary/XML plist into structured JSON.
pub fn diskutil_plist_json(arguments: &[&str]) -> Result<Value> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut diskutil = Command::new(DISKUTIL_PATH);
    diskutil.args(arguments);
    let output = crate::run_output(
        &mut diskutil,
        "diskutil structured removable-media inventory",
    )?;

    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| miette::miette!("Could not execute plutil: {error}"))?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| miette::miette!("Could not supply diskutil plist to plutil."))?;
    input
        .write_all(&output.stdout)
        .map_err(|error| miette::miette!("Could not pass diskutil plist to plutil: {error}"))?;
    drop(input);
    let converted = child
        .wait_with_output()
        .map_err(|error| miette::miette!("Could not wait for plutil: {error}"))?;
    if !converted.status.success() {
        let stderr = String::from_utf8_lossy(&converted.stderr);
        miette::bail!(
            "plutil could not convert diskutil plist output to JSON ({}): {}",
            converted.status,
            stderr.trim()
        );
    }
    serde_json::from_slice(&converted.stdout)
        .map_err(|error| miette::miette!("Could not parse converted diskutil plist: {error}"))
}
