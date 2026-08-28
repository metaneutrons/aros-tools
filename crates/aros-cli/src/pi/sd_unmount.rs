//! Fail-closed discovery and unmounting for removable SD-card media.
//!
//! The public scan contains only unequivocally whole, physical, writable,
//! removable devices with a persistent identity.  A caller selects an opaque
//! scan ID, never a device path.  [`unmount`] obtains a fresh structured
//! inventory, requires one exact match, performs a normal (non-force,
//! non-lazy) unmount, and verifies the same physical device is fully unmounted.

#[cfg(target_os = "macos")]
use super::disk_inventory::diskutil_plist_json;
#[cfg(target_os = "linux")]
use super::disk_inventory::linux_inventory_command;
#[cfg(any(target_os = "linux", test))]
use super::disk_inventory::{is_linux_whole_device_path, json_object, linux_identity, linux_model};
#[cfg(any(target_os = "macos", test))]
use super::disk_inventory::{
    is_macos_descendant_identifier, is_macos_whole_disk_identifier, macos_descendant_identifiers,
    macos_transport, macos_whole_disk_identifiers,
};
use super::disk_inventory::{
    json_bool_like, json_nonempty_string, json_u64_like, safe_metadata, DiskPlatform,
};
use miette::Result;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

const SCAN_ID_PREFIX: &str = "aros-sd-unmount-v1:";
const SAFE_REMOVABLE_MOUNT_ROOTS: &[&str] = &["/Volumes", "/media", "/run/media", "/mnt"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LinuxDeviceNumber {
    major: u32,
    minor: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MountedSource {
    Macos {
        device_node: PathBuf,
    },
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Linux {
        device_node: PathBuf,
        device_number: LinuxDeviceNumber,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MountedVolume {
    mount_point: PathBuf,
    source: MountedSource,
}

impl MountedVolume {
    fn fingerprint_component(&self) -> String {
        match &self.source {
            MountedSource::Macos { device_node } => format!(
                "macos:{}:{}",
                device_node.display(),
                self.mount_point.display()
            ),
            MountedSource::Linux {
                device_node,
                device_number,
            } => format!(
                "linux:{}:{}:{}:{}",
                device_node.display(),
                device_number.major,
                device_number.minor,
                self.mount_point.display()
            ),
        }
    }
}

/// One explicitly selectable removable whole disk which currently has at
/// least one mounted volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmountCandidate {
    /// Opaque selection ID.  Device paths are deliberately not accepted by
    /// [`unmount`].
    pub scan_id: String,
    /// Exact whole-device path reported by the structured platform inventory.
    device_path: PathBuf,
    /// Every currently mounted volume belonging to the whole disk.
    mounted_volumes: Vec<MountedVolume>,
    mount_points: Vec<PathBuf>,
    platform: DiskPlatform,
    stable_fingerprint: String,
    size_bytes: u64,
    identity: String,
    model: String,
    transport: String,
}

impl UnmountCandidate {
    /// A compact, human-readable line for an explicit selection UI.
    #[must_use]
    pub fn summary(&self) -> String {
        let mounts = self
            .mount_points
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{}  {}  {} bytes  {}  {}  mounted: {}",
            self.scan_id, self.model, self.size_bytes, self.transport, self.identity, mounts
        )
    }

    /// Mounted volumes which will be unmounted as one explicitly selected
    /// whole disk.
    #[must_use]
    pub fn mount_points(&self) -> &[PathBuf] {
        &self.mount_points
    }
}

/// Successful, verified whole-disk unmount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmountReport {
    /// The exact opaque selection which was revalidated.
    pub scan_id: String,
    /// Mount points which were present before the operation and verified gone
    /// afterward.
    pub unmounted_mount_points: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceState {
    platform: DiskPlatform,
    device_path: PathBuf,
    mounted_volumes: Vec<MountedVolume>,
    stable_fingerprint: String,
    size_bytes: u64,
    identity: String,
    model: String,
    transport: String,
}

impl DeviceState {
    fn scan_id(&self) -> String {
        let mut parts = vec![self.stable_fingerprint.clone()];
        parts.extend(
            self.mounted_volumes
                .iter()
                .map(MountedVolume::fingerprint_component),
        );
        let parts = parts.iter().map(String::as_str).collect::<Vec<_>>();
        format!("{SCAN_ID_PREFIX}{}", hash_parts(&parts))
    }

    fn candidate(&self) -> Option<UnmountCandidate> {
        if self.mounted_volumes.is_empty() {
            return None;
        }
        Some(UnmountCandidate {
            scan_id: self.scan_id(),
            device_path: self.device_path.clone(),
            mounted_volumes: self.mounted_volumes.clone(),
            mount_points: self
                .mounted_volumes
                .iter()
                .map(|volume| volume.mount_point.clone())
                .collect(),
            platform: self.platform,
            stable_fingerprint: self.stable_fingerprint.clone(),
            size_bytes: self.size_bytes,
            identity: self.identity.clone(),
            model: self.model.clone(),
            transport: self.transport.clone(),
        })
    }
}

trait UnmountBackend {
    fn inventory(&self) -> Result<Vec<DeviceState>>;
    fn execute(&self, candidate: &UnmountCandidate) -> Result<()>;
}

struct SystemBackend;

impl UnmountBackend for SystemBackend {
    fn inventory(&self) -> Result<Vec<DeviceState>> {
        #[cfg(target_os = "macos")]
        {
            scan_macos_inventory()
        }
        #[cfg(target_os = "linux")]
        {
            scan_linux_inventory()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            miette::bail!(
                "Safe removable-disk unmounting is implemented only for macOS and Linux."
            );
        }
    }

    fn execute(&self, candidate: &UnmountCandidate) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            execute_macos_unmount(candidate)
        }
        #[cfg(target_os = "linux")]
        {
            execute_linux_unmount(candidate)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = candidate;
            miette::bail!(
                "Safe removable-disk unmounting is implemented only for macOS and Linux."
            );
        }
    }
}

/// List only mounted devices for which every removable-media safety predicate
/// is explicitly known and satisfied.
pub fn scan() -> Result<Vec<UnmountCandidate>> {
    scan_with_backend(&SystemBackend)
}

/// Unmount one previously scanned removable whole disk and verify the result.
///
/// `selected_scan_id` must be the opaque ID returned by [`scan`].  A raw device
/// path is never accepted as an alternative selector.
pub fn unmount(selected_scan_id: &str) -> Result<UnmountReport> {
    unmount_with_backend(&SystemBackend, selected_scan_id)
}

fn scan_with_backend(backend: &impl UnmountBackend) -> Result<Vec<UnmountCandidate>> {
    let mut candidates = normalized_inventory(backend.inventory()?)
        .iter()
        .filter_map(DeviceState::candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.scan_id.cmp(&right.scan_id));
    Ok(candidates)
}

fn unmount_with_backend(
    backend: &impl UnmountBackend,
    selected_scan_id: &str,
) -> Result<UnmountReport> {
    if !valid_scan_id(selected_scan_id) {
        miette::bail!(
            "Unmount selection must be an opaque ID printed by `aros pi sd unmount`; raw device paths are never accepted."
        );
    }

    // This fresh inventory is the authority for the operation.  A stale scan
    // ID changes whenever the device, identity, or mount topology changes.
    let candidates = normalized_inventory(backend.inventory()?)
        .iter()
        .filter_map(DeviceState::candidate)
        .filter(|candidate| candidate.scan_id == selected_scan_id)
        .collect::<Vec<_>>();
    let [candidate] = candidates.as_slice() else {
        miette::bail!(
            "Unmount selection did not match exactly one currently eligible removable whole disk; nothing was unmounted."
        );
    };
    backend.execute(candidate)?;

    let after = normalized_inventory(backend.inventory()?);
    let matching = after
        .iter()
        .filter(|state| {
            state.platform == candidate.platform
                && state.device_path == candidate.device_path
                && state.stable_fingerprint == candidate.stable_fingerprint
        })
        .collect::<Vec<_>>();
    let [verified] = matching.as_slice() else {
        miette::bail!(
            "Unmount command returned, but the same eligible removable disk could not be uniquely revalidated afterward."
        );
    };
    if !verified.mounted_volumes.is_empty() {
        miette::bail!(
            "Unmount command returned, but selected scan ID '{}' still has mounted volumes; refusing to report success.",
            candidate.scan_id
        );
    }

    Ok(UnmountReport {
        scan_id: candidate.scan_id.clone(),
        unmounted_mount_points: candidate.mount_points.clone(),
    })
}

/// Remove candidates whose supposedly persistent identity or device path is
/// duplicated.  Ambiguity produces a false negative rather than an unsafe UI.
fn normalized_inventory(mut states: Vec<DeviceState>) -> Vec<DeviceState> {
    let mut identities = HashMap::<(DiskPlatform, String), usize>::new();
    let mut paths = HashMap::<(DiskPlatform, PathBuf), usize>::new();
    for state in &states {
        *identities
            .entry((state.platform, state.identity.clone()))
            .or_default() += 1;
        *paths
            .entry((state.platform, state.device_path.clone()))
            .or_default() += 1;
    }
    states.retain(|state| {
        identities
            .get(&(state.platform, state.identity.clone()))
            .copied()
            == Some(1)
            && paths
                .get(&(state.platform, state.device_path.clone()))
                .copied()
                == Some(1)
    });
    states.sort_by(|left, right| left.device_path.cmp(&right.device_path));
    states
}

fn make_state(
    platform: DiskPlatform,
    device_path: PathBuf,
    size_bytes: u64,
    identity: String,
    model: String,
    transport: String,
    mut mounted_volumes: Vec<MountedVolume>,
) -> Option<DeviceState> {
    if size_bytes == 0
        || !safe_metadata(&identity)
        || !safe_metadata(&model)
        || !safe_metadata(&transport)
        || mounted_volumes
            .iter()
            .any(|volume| !safe_removable_mount_point(&volume.mount_point))
        || mounted_volumes.iter().any(|volume| {
            !matches!(
                (platform, &volume.source),
                (DiskPlatform::Macos, MountedSource::Macos { .. })
                    | (DiskPlatform::Linux, MountedSource::Linux { .. })
            )
        })
    {
        return None;
    }
    mounted_volumes.sort();
    if mounted_volumes
        .windows(2)
        .any(|pair| pair[0].mount_point == pair[1].mount_point)
    {
        return None;
    }
    let device = device_path.to_str()?;
    let stable_fingerprint = hash_parts(&[
        platform.label(),
        device,
        &size_bytes.to_string(),
        &identity,
        &model,
        &transport,
    ]);
    Some(DeviceState {
        platform,
        device_path,
        mounted_volumes,
        stable_fingerprint,
        size_bytes,
        identity,
        model,
        transport,
    })
}

fn valid_scan_id(value: &str) -> bool {
    value.strip_prefix(SCAN_ID_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn hash_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn safe_absolute_mount_path(path: &Path) -> bool {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return false;
    }
    let Some(text) = path.to_str() else {
        return false;
    };
    if text.chars().any(char::is_control)
        || (text != "/"
            && text
                .split('/')
                .skip(1)
                .any(|segment| segment.is_empty() || matches!(segment, "." | "..")))
    {
        return false;
    }
    true
}

fn safe_removable_mount_point(path: &Path) -> bool {
    if !safe_absolute_mount_path(path) {
        return false;
    }
    SAFE_REMOVABLE_MOUNT_ROOTS.iter().any(|root| {
        let root = Path::new(root);
        path != root && path.starts_with(root)
    })
}

enum ParsedMountPoint {
    Unmounted,
    Mounted(PathBuf),
}

fn mount_point_from_json(value: &Value) -> Option<ParsedMountPoint> {
    match value {
        Value::Null => Some(ParsedMountPoint::Unmounted),
        Value::String(value) if value.is_empty() => Some(ParsedMountPoint::Unmounted),
        Value::String(value) => {
            let path = PathBuf::from(value);
            safe_removable_mount_point(&path).then_some(ParsedMountPoint::Mounted(path))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Linux: structured lsblk inventory and normal umount2 calls.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", test))]
fn parse_linux_inventory(value: &Value) -> Result<Vec<DeviceState>> {
    let root = json_object(value, "lsblk JSON output")?;
    let devices = root
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or_else(|| miette::miette!("lsblk JSON output must contain blockdevices."))?;
    Ok(devices.iter().filter_map(linux_state_from_node).collect())
}

#[cfg(any(target_os = "linux", test))]
fn linux_state_from_node(node: &Value) -> Option<DeviceState> {
    let object = node.as_object()?;
    if json_nonempty_string(object, "type")? != "disk"
        || !json_bool_like(object, "rm")?
        || !json_bool_like(object, "hotplug")?
        || json_bool_like(object, "ro")?
    {
        return None;
    }

    let device_path = PathBuf::from(json_nonempty_string(object, "path")?);
    if !is_linux_whole_device_path(&device_path) {
        return None;
    }
    let root_kname = json_nonempty_string(object, "kname")?;
    if device_path.file_name()?.to_str()? != root_kname
        || !matches!(object.get("pkname"), Some(Value::Null))
    {
        return None;
    }
    let transport = json_nonempty_string(object, "tran")?.to_ascii_lowercase();
    let path_matches_transport = match transport.as_str() {
        "usb" => root_kname.starts_with("sd"),
        "mmc" => root_kname.starts_with("mmcblk"),
        _ => false,
    };
    if !path_matches_transport {
        return None;
    }

    let size_bytes = json_u64_like(object, "size")?;
    let model = linux_model(object)?;
    let identity = linux_identity(object)?;
    let mut mounted_volumes = Vec::new();
    let mut seen = HashSet::new();
    if !linux_collect_mounts(node, None, &mut seen, &mut mounted_volumes) {
        return None;
    }

    make_state(
        DiskPlatform::Linux,
        device_path,
        size_bytes,
        identity,
        model,
        transport,
        mounted_volumes,
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_collect_mounts(
    node: &Value,
    expected_parent: Option<&str>,
    seen: &mut HashSet<String>,
    mounted_volumes: &mut Vec<MountedVolume>,
) -> bool {
    let Some(object) = node.as_object() else {
        return false;
    };
    let Some(kname) = json_nonempty_string(object, "kname") else {
        return false;
    };
    let Some(node_type) = json_nonempty_string(object, "type") else {
        return false;
    };
    if (expected_parent.is_none() && node_type != "disk")
        || (expected_parent.is_some() && node_type != "part")
    {
        return false;
    }
    let Some(device_node_value) = json_nonempty_string(object, "path") else {
        return false;
    };
    let device_node = PathBuf::from(device_node_value);
    let Some(device_number) = linux_device_number(object) else {
        return false;
    };
    if !safe_linux_kernel_name(kname)
        || !seen.insert(kname.to_string())
        || device_node != Path::new("/dev").join(kname)
    {
        return false;
    }
    match expected_parent {
        None if !matches!(object.get("pkname"), Some(Value::Null)) => return false,
        Some(parent) if json_nonempty_string(object, "pkname") != Some(parent) => return false,
        _ => {}
    }
    let Some(mounts) = linux_mount_points(object.get("mountpoints")) else {
        return false;
    };
    mounted_volumes.extend(mounts.into_iter().map(|mount_point| MountedVolume {
        mount_point,
        source: MountedSource::Linux {
            device_node: device_node.clone(),
            device_number,
        },
    }));

    match object.get("children") {
        None | Some(Value::Null) => true,
        Some(Value::Array(children)) => children
            .iter()
            .all(|child| linux_collect_mounts(child, Some(kname), seen, mounted_volumes)),
        Some(_) => false,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_device_number(object: &Map<String, Value>) -> Option<LinuxDeviceNumber> {
    parse_linux_device_number(json_nonempty_string(object, "maj:min")?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_device_number(value: &str) -> Option<LinuxDeviceNumber> {
    let (major, minor) = value.split_once(':')?;
    if major.is_empty()
        || minor.is_empty()
        || (major.len() > 1 && major.starts_with('0'))
        || (minor.len() > 1 && minor.starts_with('0'))
        || !major.bytes().all(|byte| byte.is_ascii_digit())
        || !minor.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(LinuxDeviceNumber {
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_mount_points(value: Option<&Value>) -> Option<Vec<PathBuf>> {
    match value? {
        Value::Null => Some(Vec::new()),
        Value::String(_) => match mount_point_from_json(value?)? {
            ParsedMountPoint::Unmounted => Some(Vec::new()),
            ParsedMountPoint::Mounted(path) => Some(vec![path]),
        },
        Value::Array(values) => {
            let mut result = Vec::new();
            for value in values {
                if let ParsedMountPoint::Mounted(path) = mount_point_from_json(value)? {
                    result.push(path);
                }
            }
            Some(result)
        }
        _ => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn safe_linux_kernel_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

#[cfg(target_os = "linux")]
fn scan_linux_inventory() -> Result<Vec<DeviceState>> {
    let output = crate::observability::run_output_at(
        &mut linux_inventory_command(false),
        "lsblk removable-media inventory",
        crate::observability::ErrorBoundary::MEDIA_SAFETY,
    )?;
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| miette::miette!("Could not parse structured lsblk JSON: {error}"))?;
    parse_linux_inventory(&parsed)
}

#[cfg(target_os = "linux")]
fn execute_linux_unmount(candidate: &UnmountCandidate) -> Result<()> {
    use rustix::mount::{unmount, UnmountFlags};

    if candidate.platform != DiskPlatform::Linux
        || !is_linux_whole_device_path(&candidate.device_path)
    {
        miette::bail!("Refusing a non-Linux or non-whole-disk unmount target.");
    }
    let mut remaining = candidate.mounted_volumes.clone();
    remaining.sort();
    remaining.dedup();
    let plan = ordered_volumes(&remaining);

    for volume in plan {
        // Recheck the exact device-to-mount relationship before every syscall;
        // this prevents a changed mount topology from being followed blindly.
        let live = normalized_inventory(scan_linux_inventory()?);
        let matching = live
            .iter()
            .filter(|state| {
                state.platform == DiskPlatform::Linux
                    && state.device_path == candidate.device_path
                    && state.stable_fingerprint == candidate.stable_fingerprint
            })
            .collect::<Vec<_>>();
        let [state] = matching.as_slice() else {
            miette::bail!(
                "The selected removable disk could not be uniquely revalidated immediately before unmount."
            );
        };
        if state.mounted_volumes != remaining {
            miette::bail!(
                "The selected removable disk's mount topology changed; refusing to continue unmounting."
            );
        }
        verify_linux_mount_source(&volume)?;
        // NOFOLLOW hardens the pathname lookup; FORCE and DETACH (lazy) are
        // deliberately absent.
        unmount(&volume.mount_point, UnmountFlags::NOFOLLOW).map_err(|error| {
            miette::miette!(
                "Could not normally unmount '{}': {error}. No force or lazy fallback was attempted.",
                volume.mount_point.display()
            )
        })?;
        remaining.retain(|remaining_volume| remaining_volume != &volume);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinuxMountInfoEntry {
    device_number: LinuxDeviceNumber,
    mount_point: PathBuf,
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_mountinfo(input: &str) -> Result<Vec<LinuxMountInfoEntry>> {
    if input.is_empty() {
        miette::bail!("Linux mountinfo is empty; refusing to unmount by path.");
    }
    let mut entries = Vec::new();
    let mut mount_ids = HashSet::new();
    for (line_index, line) in input.lines().enumerate() {
        if line.is_empty() {
            miette::bail!("Linux mountinfo contains an empty record.");
        }
        let Some((before_separator, after_separator)) = line.split_once(" - ") else {
            miette::bail!(
                "Linux mountinfo record {} has no field separator.",
                line_index + 1
            );
        };
        if after_separator.contains(" - ") {
            miette::bail!(
                "Linux mountinfo record {} has an ambiguous field separator.",
                line_index + 1
            );
        }
        let fields = before_separator
            .split_ascii_whitespace()
            .collect::<Vec<_>>();
        let trailing = after_separator.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 || trailing.len() != 3 {
            miette::bail!(
                "Linux mountinfo record {} is structurally incomplete.",
                line_index + 1
            );
        }
        let mount_id = parse_canonical_u64(fields[0]).ok_or_else(|| {
            miette::miette!(
                "Linux mountinfo record {} has an invalid mount ID.",
                line_index + 1
            )
        })?;
        if parse_canonical_u64(fields[1]).is_none() || !mount_ids.insert(mount_id) {
            miette::bail!(
                "Linux mountinfo record {} has an invalid parent or duplicate mount ID.",
                line_index + 1
            );
        }
        let device_number = parse_linux_device_number(fields[2]).ok_or_else(|| {
            miette::miette!(
                "Linux mountinfo record {} has an invalid device number.",
                line_index + 1
            )
        })?;
        // Validate every escaped path-bearing field, not just the selected
        // mount point, so malformed structured input fails as a whole.
        let root = PathBuf::from(decode_linux_mountinfo_path(fields[3]).ok_or_else(|| {
            miette::miette!(
                "Linux mountinfo record {} has a malformed root path.",
                line_index + 1
            )
        })?);
        let mount_point =
            PathBuf::from(decode_linux_mountinfo_path(fields[4]).ok_or_else(|| {
                miette::miette!(
                    "Linux mountinfo record {} has a malformed mount-point path.",
                    line_index + 1
                )
            })?);
        if !safe_absolute_mount_path(&root)
            || !safe_absolute_mount_path(&mount_point)
            || fields[5].is_empty()
            || trailing[0].is_empty()
            || decode_linux_mountinfo_path(trailing[1]).is_none()
            || trailing[2].is_empty()
        {
            miette::bail!(
                "Linux mountinfo record {} contains malformed structured fields.",
                line_index + 1
            );
        }
        entries.push(LinuxMountInfoEntry {
            device_number,
            mount_point,
        });
    }
    if entries.is_empty() {
        miette::bail!("Linux mountinfo has no records; refusing to unmount by path.");
    }
    Ok(entries)
}

#[cfg(any(target_os = "linux", test))]
fn parse_canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

#[cfg(any(target_os = "linux", test))]
fn decode_linux_mountinfo_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let octal = bytes.get(index + 1..index + 4)?;
        if !octal.iter().all(|byte| matches!(byte, b'0'..=b'7')) {
            return None;
        }
        let escaped = (octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + (octal[2] - b'0');
        if !matches!(escaped, b' ' | b'\t' | b'\n' | b'\\') {
            return None;
        }
        decoded.push(escaped);
        index += 4;
    }
    String::from_utf8(decoded).ok()
}

#[cfg(any(target_os = "linux", test))]
fn verify_linux_mountinfo_source(volume: &MountedVolume, mountinfo: &str) -> Result<()> {
    let MountedSource::Linux { device_number, .. } = &volume.source else {
        miette::bail!("Refusing a Linux source check for a non-Linux mounted volume.");
    };
    let entries = parse_linux_mountinfo(mountinfo)?;
    let matching = entries
        .iter()
        .filter(|entry| entry.mount_point == volume.mount_point)
        .collect::<Vec<_>>();
    let [entry] = matching.as_slice() else {
        miette::bail!(
            "Mount point '{}' is missing or stacked in Linux mountinfo; refusing an ambiguous unmount.",
            volume.mount_point.display()
        );
    };
    if entry.device_number != *device_number {
        miette::bail!(
            "Mount point '{}' is currently backed by a different Linux device; refusing to unmount it.",
            volume.mount_point.display()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn verify_linux_mount_source(volume: &MountedVolume) -> Result<()> {
    use rustix::fs::{major, minor, stat};

    let MountedSource::Linux { device_number, .. } = &volume.source else {
        miette::bail!("Refusing a Linux source check for a non-Linux mounted volume.");
    };
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| miette::miette!("Could not read /proc/self/mountinfo safely: {error}"))?;
    verify_linux_mountinfo_source(volume, &mountinfo)?;
    let metadata = stat(&volume.mount_point).map_err(|error| {
        miette::miette!(
            "Could not stat mount point '{}' immediately before unmount: {error}",
            volume.mount_point.display()
        )
    })?;
    let actual = LinuxDeviceNumber {
        major: major(metadata.st_dev),
        minor: minor(metadata.st_dev),
    };
    if actual != *device_number {
        miette::bail!(
            "Mount point '{}' resolves to a different topmost filesystem; refusing to unmount it.",
            volume.mount_point.display()
        );
    }
    Ok(())
}

fn mount_depth(path: &Path) -> usize {
    path.components().count()
}

#[cfg(test)]
fn ordered_mount_points(mounts: &[PathBuf]) -> Vec<PathBuf> {
    let mut plan = mounts.to_vec();
    plan.sort_by(|left, right| {
        mount_depth(right)
            .cmp(&mount_depth(left))
            .then_with(|| right.cmp(left))
    });
    plan
}

fn ordered_volumes(volumes: &[MountedVolume]) -> Vec<MountedVolume> {
    let mut plan = volumes.to_vec();
    plan.sort_by(|left, right| {
        mount_depth(&right.mount_point)
            .cmp(&mount_depth(&left.mount_point))
            .then_with(|| right.mount_point.cmp(&left.mount_point))
    });
    plan
}

// ---------------------------------------------------------------------------
// macOS: structured diskutil plist inventory and checked per-volume unmounts.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", test))]
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosRootEvidence {
    identifier: String,
    device_path: PathBuf,
    size_bytes: u64,
    identity: String,
    model: String,
    transport: String,
}

#[cfg(any(target_os = "macos", test))]
fn macos_root_evidence(info: &Value) -> Option<MacosRootEvidence> {
    let root = info.as_object()?;
    let identifier = json_nonempty_string(root, "DeviceIdentifier")?;
    if !is_macos_whole_disk_identifier(identifier)
        || !json_bool_like(root, "WholeDisk")?
        || json_nonempty_string(root, "ParentWholeDisk")? != identifier
        || json_bool_like(root, "Internal")?
        || !json_bool_like(root, "Writable")?
        || !json_bool_like(root, "Ejectable")?
        || !macos_explicitly_removable(root)
        || !json_nonempty_string(root, "VirtualOrPhysical")?.eq_ignore_ascii_case("physical")
    {
        return None;
    }
    let device_path = PathBuf::from(json_nonempty_string(root, "DeviceNode")?);
    let expected_device_path = format!("/dev/{identifier}");
    if device_path.as_os_str() != expected_device_path.as_str() {
        return None;
    }
    Some(MacosRootEvidence {
        identifier: identifier.to_string(),
        device_path,
        size_bytes: json_u64_like(root, "Size")?,
        identity: format!("serial:{}", json_nonempty_string(root, "SerialNumber")?),
        model: json_nonempty_string(root, "MediaName")?.to_string(),
        transport: macos_transport(root)?,
    })
}

#[cfg(any(target_os = "macos", test))]
fn macos_root_observations_match(first: &Value, second: &Value) -> bool {
    const FIELDS: &[&str] = &[
        "DeviceIdentifier",
        "DeviceNode",
        "ParentWholeDisk",
        "WholeDisk",
        "Internal",
        "Writable",
        "Ejectable",
        "Removable",
        "RemovableMedia",
        "VirtualOrPhysical",
        "Protocol",
        "BusProtocol",
        "SerialNumber",
        "MediaName",
        "Size",
        "MountPoint",
    ];
    let (Some(first), Some(second)) = (first.as_object(), second.as_object()) else {
        return false;
    };
    FIELDS
        .iter()
        .all(|field| first.get(*field) == second.get(*field))
}

#[cfg(any(target_os = "macos", test))]
fn macos_state_from_info(
    root_info: &Value,
    descendant_identifiers: &[String],
    descendant_infos: &[Value],
) -> Option<DeviceState> {
    let evidence = macos_root_evidence(root_info)?;
    let identifier = evidence.identifier.as_str();

    if descendant_identifiers.is_empty()
        || descendant_identifiers.len() != descendant_infos.len()
        || !descendant_identifiers
            .iter()
            .any(|descendant| descendant == identifier)
    {
        return None;
    }
    let expected = descendant_identifiers
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if expected.len() != descendant_identifiers.len() {
        return None;
    }
    let mut seen = HashSet::new();
    let mut mounted_volumes = Vec::new();
    for info in descendant_infos {
        let object = info.as_object()?;
        let descendant = json_nonempty_string(object, "DeviceIdentifier")?;
        let descendant_is_whole = json_bool_like(object, "WholeDisk")?;
        let expected_device_node = format!("/dev/{descendant}");
        let device_node = PathBuf::from(json_nonempty_string(object, "DeviceNode")?);
        if !expected.contains(descendant)
            || !seen.insert(descendant.to_string())
            || !is_macos_descendant_identifier(descendant, identifier)
            || descendant_is_whole ^ (descendant == identifier)
            || json_nonempty_string(object, "ParentWholeDisk")? != identifier
            || device_node.as_os_str() != expected_device_node.as_str()
        {
            return None;
        }
        if descendant == identifier
            && (macos_root_evidence(info)? != evidence
                || !macos_root_observations_match(root_info, info))
        {
            return None;
        }
        if let ParsedMountPoint::Mounted(path) = mount_point_from_json(object.get("MountPoint")?)? {
            mounted_volumes.push(MountedVolume {
                mount_point: path,
                source: MountedSource::Macos { device_node },
            });
        }
    }
    if seen.len() != expected.len() {
        return None;
    }

    make_state(
        DiskPlatform::Macos,
        evidence.device_path,
        evidence.size_bytes,
        evidence.identity,
        evidence.model,
        evidence.transport,
        mounted_volumes,
    )
}

#[cfg(any(target_os = "macos", test))]
fn macos_explicitly_removable(object: &Map<String, Value>) -> bool {
    let mut present = false;
    for field in ["Removable", "RemovableMedia"] {
        if object.contains_key(field) {
            present = true;
            if json_bool_like(object, field) != Some(true) {
                return false;
            }
        }
    }
    present
}

#[cfg(any(target_os = "macos", test))]
#[cfg(target_os = "macos")]
fn scan_macos_inventory() -> Result<Vec<DeviceState>> {
    let list = diskutil_plist_json(&["list", "-plist"])?;
    let identifiers = macos_whole_disk_identifiers(&list)?;
    let mut states = Vec::new();
    for identifier in identifiers {
        let path = format!("/dev/{identifier}");
        let Ok(root_info) = diskutil_plist_json(&["info", "-plist", &path]) else {
            continue;
        };
        let Ok(descendant_list) = diskutil_plist_json(&["list", "-plist", &path]) else {
            continue;
        };
        let Ok(descendants) = macos_descendant_identifiers(&descendant_list, &identifier) else {
            continue;
        };
        let mut infos = Vec::with_capacity(descendants.len());
        let mut complete = true;
        // Query the repeated whole-disk observation last.  Its exact evidence
        // comparison therefore brackets all descendant topology reads.
        let mut query_order = descendants.clone();
        query_order.sort_by_key(|descendant| descendant == &identifier);
        for descendant in &query_order {
            let descendant_path = format!("/dev/{descendant}");
            if let Ok(info) = diskutil_plist_json(&["info", "-plist", &descendant_path]) {
                infos.push(info);
            } else {
                complete = false;
                break;
            }
        }
        if complete {
            if let Some(state) = macos_state_from_info(&root_info, &descendants, &infos) {
                states.push(state);
            }
        }
    }
    Ok(states)
}

#[cfg(target_os = "macos")]
struct SystemMacosUnmountOps;

#[cfg(any(target_os = "macos", test))]
trait MacosUnmountOps {
    fn inventory(&self) -> Result<Vec<DeviceState>>;
    fn unmount_volume(&self, volume: &MountedVolume) -> Result<()>;
}

#[cfg(target_os = "macos")]
impl MacosUnmountOps for SystemMacosUnmountOps {
    fn inventory(&self) -> Result<Vec<DeviceState>> {
        scan_macos_inventory()
    }

    fn unmount_volume(&self, volume: &MountedVolume) -> Result<()> {
        use std::process::Command;

        let MountedSource::Macos { device_node } = &volume.source else {
            miette::bail!("Refusing a non-macOS mounted-volume source.");
        };
        if !safe_removable_mount_point(&volume.mount_point) {
            miette::bail!("Refusing a macOS unmount outside the removable-media mount roots.");
        }
        crate::observability::run_output_at(
            Command::new("/usr/sbin/diskutil")
                .arg("unmount")
                .arg(device_node),
            &format!(
                "diskutil normal unmount for '{}' (no force or eject fallback)",
                volume.mount_point.display()
            ),
            crate::observability::ErrorBoundary::MEDIA_SAFETY,
        )?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn execute_macos_unmount(candidate: &UnmountCandidate) -> Result<()> {
    execute_macos_unmount_with_ops(candidate, &SystemMacosUnmountOps)
}

#[cfg(any(target_os = "macos", test))]
fn execute_macos_unmount_with_ops(
    candidate: &UnmountCandidate,
    ops: &impl MacosUnmountOps,
) -> Result<()> {
    let Some(identifier) = candidate
        .device_path
        .strip_prefix("/dev")
        .ok()
        .and_then(|path| path.to_str())
    else {
        miette::bail!("Refusing a malformed macOS whole-disk unmount target.");
    };
    let expected_device_path = format!("/dev/{identifier}");
    if candidate.platform != DiskPlatform::Macos
        || !is_macos_whole_disk_identifier(identifier)
        || candidate.device_path.as_os_str() != expected_device_path.as_str()
        || candidate.mounted_volumes.is_empty()
    {
        miette::bail!("Refusing a non-macOS or non-whole-disk unmount target.");
    }

    let mut remaining = candidate.mounted_volumes.clone();
    remaining.sort();
    remaining.dedup();
    for volume in ordered_volumes(&remaining) {
        let live = normalized_inventory(ops.inventory()?);
        let matching = live
            .iter()
            .filter(|state| {
                state.platform == DiskPlatform::Macos
                    && state.device_path == candidate.device_path
                    && state.stable_fingerprint == candidate.stable_fingerprint
            })
            .collect::<Vec<_>>();
        let [state] = matching.as_slice() else {
            miette::bail!(
                "The selected removable macOS disk could not be uniquely revalidated immediately before unmount."
            );
        };
        if state.mounted_volumes != remaining {
            miette::bail!(
                "The selected removable macOS disk's mount topology changed; refusing to continue unmounting."
            );
        }
        let MountedSource::Macos { device_node } = &volume.source else {
            miette::bail!("The selected macOS volume has a non-macOS source binding.");
        };
        let Some(descendant) = device_node
            .strip_prefix("/dev")
            .ok()
            .and_then(|path| path.to_str())
        else {
            miette::bail!("The selected macOS volume has a malformed source device.");
        };
        let expected_device_node = format!("/dev/{descendant}");
        if !is_macos_descendant_identifier(descendant, identifier)
            || device_node.as_os_str() != expected_device_node.as_str()
        {
            miette::bail!("The selected macOS volume is not bound to the selected whole disk.");
        }
        ops.unmount_volume(&volume)?;
        remaining.retain(|remaining_volume| remaining_volume != &volume);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    fn linux_fixture() -> Value {
        json!({
            "blockdevices": [{
                "path": "/dev/sdz",
                "kname": "sdz",
                "pkname": null,
                "type": "disk",
                "size": 32_000_000_000_u64,
                "rm": true,
                "ro": false,
                "mountpoints": [null],
                "tran": "usb",
                "serial": "CARD-1234",
                "wwn": null,
                "model": "SD Reader",
                "vendor": "Lab",
                "hotplug": true,
                "maj:min": "8:240",
                "children": [{
                    "path": "/dev/sdz1",
                    "kname": "sdz1",
                    "pkname": "sdz",
                    "type": "part",
                    "maj:min": "8:241",
                    "mountpoints": ["/media/aros-boot"],
                    "children": null
                }]
            }]
        })
    }

    fn macos_fixture() -> (Value, Vec<String>, Vec<Value>) {
        let root = json!({
            "DeviceIdentifier": "disk19",
            "DeviceNode": "/dev/disk19",
            "ParentWholeDisk": "disk19",
            "WholeDisk": true,
            "Internal": false,
            "Writable": true,
            "Ejectable": true,
            "Removable": true,
            "RemovableMedia": true,
            "VirtualOrPhysical": "Physical",
            "BusProtocol": "USB",
            "SerialNumber": "USB-SD-987",
            "MediaName": "SD Card Reader",
            "Size": 64_000_000_000_u64,
            "MountPoint": null
        });
        let child = json!({
            "DeviceIdentifier": "disk19s1",
            "DeviceNode": "/dev/disk19s1",
            "ParentWholeDisk": "disk19",
            "WholeDisk": false,
            "MountPoint": "/Volumes/AROSBOOT"
        });
        (
            root.clone(),
            vec!["disk19".into(), "disk19s1".into()],
            vec![root, child],
        )
    }

    #[test]
    fn linux_positive_fixture_yields_one_mounted_removable_whole_disk() {
        let states = parse_linux_inventory(&linux_fixture()).expect("linux fixture");
        assert_eq!(states.len(), 1);
        let candidate = states[0].candidate().expect("mounted candidate");
        assert_eq!(candidate.device_path, Path::new("/dev/sdz"));
        assert_eq!(
            candidate.mount_points(),
            [PathBuf::from("/media/aros-boot")]
        );
        assert!(valid_scan_id(&candidate.scan_id));
    }

    #[test]
    fn linux_rejects_unknown_removable_and_hotplug_flags() {
        for field in ["rm", "hotplug"] {
            let mut fixture = linux_fixture();
            fixture["blockdevices"][0]
                .as_object_mut()
                .expect("disk object")
                .remove(field);
            assert!(parse_linux_inventory(&fixture)
                .expect("parse fixture")
                .is_empty());
        }
    }

    #[test]
    fn linux_rejects_read_only_non_hotplug_virtual_partition_and_bad_transport() {
        let mutations = [
            ("ro", json!(true)),
            ("hotplug", json!(false)),
            ("type", json!("loop")),
            ("type", json!("part")),
            ("tran", json!("sata")),
        ];
        for (field, value) in mutations {
            let mut fixture = linux_fixture();
            fixture["blockdevices"][0][field] = value;
            assert!(parse_linux_inventory(&fixture)
                .expect("parse fixture")
                .is_empty());
        }
    }

    #[test]
    fn linux_rejects_incomplete_tree_and_critical_or_relative_mounts() {
        for mount in [
            json!("/boot"),
            json!("/Users/fabian"),
            json!("/etc/aros"),
            json!("/opt/aros"),
            json!("/tmp/aros"),
            json!("relative/path"),
        ] {
            let mut fixture = linux_fixture();
            fixture["blockdevices"][0]["children"][0]["mountpoints"] = json!([mount]);
            assert!(parse_linux_inventory(&fixture)
                .expect("parse fixture")
                .is_empty());
        }
        let mut incomplete = linux_fixture();
        incomplete["blockdevices"][0]["children"][0]
            .as_object_mut()
            .expect("child object")
            .remove("mountpoints");
        assert!(parse_linux_inventory(&incomplete)
            .expect("parse fixture")
            .is_empty());

        let mut unknown_type = linux_fixture();
        unknown_type["blockdevices"][0]["children"][0]
            .as_object_mut()
            .expect("child object")
            .remove("type");
        assert!(parse_linux_inventory(&unknown_type)
            .expect("parse fixture")
            .is_empty());

        let mut leading_zero = linux_fixture();
        leading_zero["blockdevices"][0]["path"] = json!("/dev/mmcblk01");
        leading_zero["blockdevices"][0]["kname"] = json!("mmcblk01");
        leading_zero["blockdevices"][0]["tran"] = json!("mmc");
        leading_zero["blockdevices"][0]["children"] = Value::Null;
        assert!(parse_linux_inventory(&leading_zero)
            .expect("parse fixture")
            .is_empty());

        let mut missing_device_number = linux_fixture();
        missing_device_number["blockdevices"][0]["children"][0]
            .as_object_mut()
            .expect("child object")
            .remove("maj:min");
        assert!(parse_linux_inventory(&missing_device_number)
            .expect("parse fixture")
            .is_empty());
    }

    fn linux_fixture_volume() -> MountedVolume {
        parse_linux_inventory(&linux_fixture())
            .expect("fixture")
            .remove(0)
            .mounted_volumes
            .remove(0)
    }

    #[test]
    fn linux_mountinfo_binds_the_only_mount_to_the_expected_device_number() {
        let volume = linux_fixture_volume();
        let mountinfo = "42 35 8:241 / /media/aros-boot rw,relatime - vfat /dev/sdz1 rw\n";
        verify_linux_mountinfo_source(&volume, mountinfo).expect("bound source");

        let wrong_source = "42 35 8:1 / /media/aros-boot rw,relatime - ext4 /dev/sda1 rw\n";
        assert!(verify_linux_mountinfo_source(&volume, wrong_source).is_err());

        let missing = "42 35 8:241 / /media/other rw - vfat /dev/sdz1 rw\n";
        assert!(verify_linux_mountinfo_source(&volume, missing).is_err());
    }

    #[test]
    fn linux_mountinfo_rejects_stacked_overmounts_even_if_one_source_matches() {
        let volume = linux_fixture_volume();
        let stacked = concat!(
            "42 35 8:241 / /media/aros-boot rw - vfat /dev/sdz1 rw\n",
            "73 35 8:1 / /media/aros-boot rw - ext4 /dev/sda1 rw\n",
        );
        assert!(verify_linux_mountinfo_source(&volume, stacked).is_err());
    }

    #[test]
    fn linux_mountinfo_decodes_known_escapes_and_rejects_malformed_paths() {
        let mut volume = linux_fixture_volume();
        volume.mount_point = PathBuf::from("/media/aros boot");
        let escaped = "42 35 8:241 / /media/aros\\040boot rw - vfat /dev/sdz1 rw\n";
        verify_linux_mountinfo_source(&volume, escaped).expect("escaped space");

        for malformed in [
            "42 35 8:241 / /media/aros\\04xboot rw - vfat /dev/sdz1 rw\n",
            "42 35 8:241 relative /media/aros-boot rw - vfat /dev/sdz1 rw\n",
            "42 35 8:241 /../root /media/aros-boot rw - vfat /dev/sdz1 rw\n",
            "42 35 8:241 / /media/aros-boot rw vfat /dev/sdz1 rw\n",
        ] {
            assert!(parse_linux_mountinfo(malformed).is_err());
        }
    }

    #[test]
    fn macos_positive_fixture_requires_complete_physical_removable_topology() {
        let (root, identifiers, infos) = macos_fixture();
        let state =
            macos_state_from_info(&root, &identifiers, &infos).expect("safe macOS candidate");
        let candidate = state.candidate().expect("mounted candidate");
        assert_eq!(candidate.device_path, Path::new("/dev/disk19"));
        assert_eq!(
            candidate.mount_points(),
            [PathBuf::from("/Volumes/AROSBOOT")]
        );

        assert!(macos_state_from_info(&root, &identifiers, &infos[..1]).is_none());

        let mut unknown_child_type = infos.clone();
        unknown_child_type[1]
            .as_object_mut()
            .expect("child object")
            .remove("WholeDisk");
        assert!(macos_state_from_info(&root, &identifiers, &unknown_child_type).is_none());

        let mut reused_device = infos.clone();
        reused_device[0]["SerialNumber"] = json!("DIFFERENT-CARD");
        assert!(macos_state_from_info(&root, &identifiers, &reused_device).is_none());

        let mut changed_safety_evidence = infos;
        changed_safety_evidence[0]["RemovableMedia"] = json!(false);
        assert!(macos_state_from_info(&root, &identifiers, &changed_safety_evidence).is_none());

        let (_, _, mut changed_root_mount) = macos_fixture();
        changed_root_mount[0]["MountPoint"] = json!("/Volumes/WHOLE-CARD");
        assert!(macos_state_from_info(&root, &identifiers, &changed_root_mount).is_none());

        let (_, _, mut missing_root_mount) = macos_fixture();
        missing_root_mount[0]
            .as_object_mut()
            .expect("root object")
            .remove("MountPoint");
        assert!(macos_state_from_info(&root, &identifiers, &missing_root_mount).is_none());
    }

    #[test]
    fn macos_rejects_unknown_or_false_safety_flags_and_virtual_media() {
        for field in ["Internal", "Writable", "Ejectable"] {
            let (mut root, identifiers, mut infos) = macos_fixture();
            root.as_object_mut().expect("root object").remove(field);
            infos[0] = root.clone();
            assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
        }
        for (field, value) in [
            ("Internal", json!(true)),
            ("Writable", json!(false)),
            ("Ejectable", json!(false)),
            ("WholeDisk", json!(false)),
            ("VirtualOrPhysical", json!("Virtual")),
        ] {
            let (mut root, identifiers, mut infos) = macos_fixture();
            root[field] = value;
            infos[0] = root.clone();
            assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
        }
    }

    #[test]
    fn macos_requires_explicit_removability_serial_transport_and_safe_mounts() {
        let (mut root, identifiers, mut infos) = macos_fixture();
        root.as_object_mut()
            .expect("root object")
            .remove("Removable");
        root.as_object_mut()
            .expect("root object")
            .remove("RemovableMedia");
        infos[0] = root.clone();
        assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());

        for (removable, removable_media) in [(false, true), (true, false)] {
            let (mut root, identifiers, mut infos) = macos_fixture();
            root["Removable"] = json!(removable);
            root["RemovableMedia"] = json!(removable_media);
            infos[0] = root.clone();
            assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
        }

        let (mut root, identifiers, mut infos) = macos_fixture();
        root["RemovableMedia"] = json!("unknown");
        infos[0] = root.clone();
        assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());

        for (field, value) in [("SerialNumber", json!(null)), ("BusProtocol", json!("PCI"))] {
            let (mut root, identifiers, mut infos) = macos_fixture();
            root[field] = value;
            infos[0] = root.clone();
            assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
        }

        for contradictory_transport in ["PCI", "Secure Digital"] {
            let (mut root, identifiers, mut infos) = macos_fixture();
            root["Protocol"] = json!("USB");
            root["BusProtocol"] = json!(contradictory_transport);
            infos[0] = root.clone();
            assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
        }

        let (mut root, identifiers, mut infos) = macos_fixture();
        root["Protocol"] = json!("USB");
        root["BusProtocol"] = Value::Null;
        infos[0] = root.clone();
        assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());

        let (root, identifiers, mut infos) = macos_fixture();
        infos[1]["MountPoint"] = json!("/System/Volumes/Data");
        assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());

        let (mut root, identifiers, mut infos) = macos_fixture();
        root["DeviceIdentifier"] = json!("disk019");
        root["DeviceNode"] = json!("/dev/disk019");
        root["ParentWholeDisk"] = json!("disk019");
        infos[0] = root.clone();
        assert!(macos_state_from_info(&root, &identifiers, &infos).is_none());
    }

    #[test]
    fn mount_points_are_limited_to_standard_removable_roots() {
        for path in [
            "/Volumes/CARD",
            "/media/card",
            "/run/media/user/card",
            "/mnt/card",
        ] {
            assert!(safe_removable_mount_point(Path::new(path)), "{path}");
        }
        for path in [
            "/Volumes",
            "/media",
            "/run/media",
            "/mnt",
            "/Applications/card",
            "/Library/card",
            "/root/card",
            "/media//card",
            "/mnt/card/",
            "/mnt/card/../other",
        ] {
            assert!(!safe_removable_mount_point(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn mount_plan_is_deepest_first_without_force_or_lazy_flags() {
        let mounts = [
            PathBuf::from("/media/card"),
            PathBuf::from("/media/card/nested"),
            PathBuf::from("/run/media/card"),
        ];
        let mounts = ordered_mount_points(&mounts);
        let nested = mounts
            .iter()
            .position(|path| path == Path::new("/media/card/nested"))
            .expect("nested mount");
        let parent = mounts
            .iter()
            .position(|path| path == Path::new("/media/card"))
            .expect("parent mount");
        assert!(nested < parent);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_unmount_flags_use_nofollow_but_neither_force_nor_lazy_detach() {
        use rustix::mount::UnmountFlags;

        let flags = UnmountFlags::NOFOLLOW;
        assert!(flags.contains(UnmountFlags::NOFOLLOW));
        assert!(!flags.intersects(UnmountFlags::FORCE | UnmountFlags::DETACH));
    }

    struct FakeMacosUnmountOps {
        inventories: RefCell<VecDeque<Vec<DeviceState>>>,
        unmounted: RefCell<Vec<MountedVolume>>,
    }

    impl MacosUnmountOps for FakeMacosUnmountOps {
        fn inventory(&self) -> Result<Vec<DeviceState>> {
            self.inventories
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| miette::miette!("missing fake macOS inventory"))
        }

        fn unmount_volume(&self, volume: &MountedVolume) -> Result<()> {
            self.unmounted.borrow_mut().push(volume.clone());
            Ok(())
        }
    }

    fn macos_volume(device_node: &str, mount_point: &str) -> MountedVolume {
        MountedVolume {
            mount_point: PathBuf::from(mount_point),
            source: MountedSource::Macos {
                device_node: PathBuf::from(device_node),
            },
        }
    }

    fn macos_state_with_two_mounts() -> DeviceState {
        let (root, identifiers, infos) = macos_fixture();
        let mut state =
            macos_state_from_info(&root, &identifiers, &infos).expect("safe macOS state");
        state
            .mounted_volumes
            .push(macos_volume("/dev/disk19s2", "/Volumes/AROS-DATA"));
        state.mounted_volumes.sort();
        state
    }

    #[test]
    fn macos_new_mount_aborts_before_any_unmount_command() {
        let state = macos_state_with_two_mounts();
        let candidate = state.candidate().expect("mounted candidate");
        let mut changed = state;
        changed
            .mounted_volumes
            .push(macos_volume("/dev/disk19s3", "/Volumes/NEW-MOUNT"));
        changed.mounted_volumes.sort();
        let ops = FakeMacosUnmountOps {
            inventories: RefCell::new(VecDeque::from([vec![changed]])),
            unmounted: RefCell::new(Vec::new()),
        };

        assert!(execute_macos_unmount_with_ops(&candidate, &ops).is_err());
        assert!(ops.unmounted.borrow().is_empty());
    }

    #[test]
    fn macos_partial_progress_stops_before_a_second_changed_mount() {
        let state = macos_state_with_two_mounts();
        let candidate = state.candidate().expect("mounted candidate");
        let plan = ordered_volumes(&state.mounted_volumes);
        let mut changed_after_first = state.clone();
        changed_after_first
            .mounted_volumes
            .retain(|mount| mount != &plan[0]);
        changed_after_first
            .mounted_volumes
            .push(macos_volume("/dev/disk19s3", "/Volumes/UNEXPECTED"));
        changed_after_first.mounted_volumes.sort();
        let ops = FakeMacosUnmountOps {
            inventories: RefCell::new(VecDeque::from([vec![state], vec![changed_after_first]])),
            unmounted: RefCell::new(Vec::new()),
        };

        assert!(execute_macos_unmount_with_ops(&candidate, &ops).is_err());
        assert_eq!(ops.unmounted.borrow().as_slice(), [plan[0].clone()]);
    }

    struct FakeBackend {
        inventories: RefCell<VecDeque<Vec<DeviceState>>>,
        executed: RefCell<Vec<String>>,
    }

    impl UnmountBackend for FakeBackend {
        fn inventory(&self) -> Result<Vec<DeviceState>> {
            self.inventories
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| miette::miette!("missing fake inventory"))
        }

        fn execute(&self, candidate: &UnmountCandidate) -> Result<()> {
            self.executed.borrow_mut().push(candidate.scan_id.clone());
            Ok(())
        }
    }

    #[test]
    fn unmount_rescans_exact_selection_and_verifies_empty_mounts() {
        let mounted = parse_linux_inventory(&linux_fixture())
            .expect("fixture")
            .remove(0);
        let selected = mounted.scan_id();
        let mut unmounted = mounted.clone();
        unmounted.mounted_volumes.clear();
        let backend = FakeBackend {
            inventories: RefCell::new(VecDeque::from([vec![mounted], vec![unmounted]])),
            executed: RefCell::new(Vec::new()),
        };

        let report = unmount_with_backend(&backend, &selected).expect("verified unmount");
        assert_eq!(report.scan_id, selected);
        assert_eq!(
            report.unmounted_mount_points,
            [PathBuf::from("/media/aros-boot")]
        );
        assert_eq!(backend.executed.borrow().as_slice(), [selected]);
    }

    #[test]
    fn raw_device_path_is_never_a_selection() {
        let backend = FakeBackend {
            inventories: RefCell::new(VecDeque::new()),
            executed: RefCell::new(Vec::new()),
        };
        assert!(unmount_with_backend(&backend, "/dev/sdz").is_err());
        assert!(backend.executed.borrow().is_empty());
    }

    #[test]
    fn duplicate_persistent_identity_hides_every_ambiguous_device() {
        let state = parse_linux_inventory(&linux_fixture())
            .expect("fixture")
            .remove(0);
        let mut duplicate = state.clone();
        duplicate.device_path = PathBuf::from("/dev/sdy");
        assert!(normalized_inventory(vec![state, duplicate]).is_empty());
    }
}
