//! Explicit local-only GNU toolchain description.
//!
//! This contract binds a local prefix to measured bytes and a checked GNU
//! executable layout. It carries no release, producer, source-commit, or ABI
//! qualification claim. Compiler behavior must be established by the caller's
//! separate, source-bound probes.

use crate::toolchain_inventory::toolchain_tree_inventory_excluding;
use crate::toolchain_layout::{ToolchainToolLayout, TOOLCHAIN_TOOLS_FILE};
use crate::toolchain_manifest::{
    ArosCompilerIdentity, ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use crate::{
    toolchain_inventory_sha256, validate_existing_directory_prefix_nofollow, Sha256Digest,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Fixed descriptor filename inside a local GNU compiler prefix.
pub const LOCAL_TOOLCHAIN_DESCRIPTOR_FILE: &str = "toolchain-local.json";

const LOCAL_TOOLCHAIN_SCHEMA: &str = "aros-local-toolchain-v1";
const LOCAL_TOOLCHAIN_QUALIFICATION: &str = "local-byte-verified";
const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INVENTORY_ENTRIES: usize = 100_000;

/// Measured byte inventory and declared layout for an explicitly local GNU
/// prefix. This is not a release manifest and does not qualify compiler ABI
/// behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalToolchainDescriptor {
    /// Closed descriptor schema identifier.
    pub schema: String,
    /// Fixed, deliberately limited qualification label.
    pub qualification: String,
    /// Supported host on which this local prefix is used.
    pub host: String,
    /// Source profile that selected this compiler.
    pub target_profile: String,
    /// Compiler target triple.
    pub target_triple: String,
    /// Declared GNU compiler and source-derived target identity.
    pub compiler: ArosCompilerIdentity,
    /// Digest of the measured prefix tree, excluding this descriptor only.
    pub tree_sha256: String,
    /// Digest of the exact `toolchain-tools.json` bytes.
    pub tool_layout_sha256: String,
    /// Canonical, sorted inventory of the prefix excluding this descriptor.
    pub files: Vec<ArosToolchainManifestEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalToolchainDescriptorRecord {
    schema: String,
    qualification: String,
    host: String,
    target_profile: String,
    target_triple: String,
    compiler: ArosCompilerIdentity,
    tree_sha256: String,
    tool_layout_sha256: String,
    files: Vec<ArosToolchainManifestEntry>,
}

impl LocalToolchainDescriptor {
    /// Read a bounded descriptor through a no-follow regular-file snapshot.
    ///
    /// Loading parses metadata only. Call [`Self::verify`] before using the
    /// prefix as a selected compiler input.
    ///
    /// # Errors
    /// Rejects an unsafe root, coexisting release manifest, unreadable or
    /// oversized descriptor, and any invalid descriptor field.
    pub fn load(root: &Path) -> Result<Self, String> {
        let root = validate_local_root(root)?;
        reject_release_manifest(&root)?;
        let path = root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
        let (_, bytes) = crate::measure_regular_file_bounded(&path, MAX_DESCRIPTOR_BYTES)
            .map_err(|error| format!("cannot read local toolchain descriptor: {error}"))?
            .ok_or_else(|| "local toolchain descriptor is missing".to_string())?;
        Self::parse(&bytes)
    }

    /// Parse the closed, size-bounded local descriptor schema.
    ///
    /// # Errors
    /// Rejects oversized, malformed, unknown-field, or internally inconsistent
    /// descriptor bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_DESCRIPTOR_BYTES {
            return Err("local toolchain descriptor exceeds 16 MiB".into());
        }
        let record: LocalToolchainDescriptorRecord = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid local toolchain descriptor: {error}"))?;
        let descriptor = Self {
            schema: record.schema,
            qualification: record.qualification,
            host: record.host,
            target_profile: record.target_profile,
            target_triple: record.target_triple,
            compiler: record.compiler,
            tree_sha256: record.tree_sha256,
            tool_layout_sha256: record.tool_layout_sha256,
            files: record.files,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Measure a local prefix and construct its descriptor without publishing
    /// or modifying any file.
    ///
    /// The caller supplies the measured compiler identity. This function does
    /// not execute a compiler and does not claim that the compiler obeys the
    /// declared ABI.
    ///
    /// # Errors
    /// Rejects an unsafe prefix, release-manifest ambiguity, unsupported host
    /// or identity, invalid GNU layout, oversized inventory, or escaping
    /// symlink.
    pub fn capture(
        root: &Path,
        host: &str,
        target_profile: &str,
        target_triple: &str,
        compiler: ArosCompilerIdentity,
    ) -> Result<Self, String> {
        let root = validate_local_root(root)?;
        reject_release_manifest(&root)?;
        validate_identity(host, target_profile, target_triple, &compiler)?;
        validate_optional_local_descriptor(&root)?;

        let layout = ToolchainToolLayout::load(&root)?;
        if !layout.has_objdump_role() {
            return Err("local GNU toolchain requires a v3 objdump role".into());
        }
        layout.validate_binding(&compiler, target_triple)?;
        layout.resolve_tools(&root)?;

        let (tree_sha256, files) =
            toolchain_tree_inventory_excluding(&root, &[LOCAL_TOOLCHAIN_DESCRIPTOR_FILE])
                .map_err(|error| format!("cannot inventory local GNU toolchain: {error}"))?;
        if files.len() > MAX_INVENTORY_ENTRIES {
            return Err("local toolchain inventory exceeds 100000 entries".into());
        }
        let descriptor = Self {
            schema: LOCAL_TOOLCHAIN_SCHEMA.into(),
            qualification: LOCAL_TOOLCHAIN_QUALIFICATION.into(),
            host: host.into(),
            target_profile: target_profile.into(),
            target_triple: target_triple.into(),
            compiler,
            tree_sha256,
            tool_layout_sha256: layout.sha256().to_string(),
            files,
        };
        descriptor.validate()?;
        descriptor.verify_inventory_symlinks(&root)?;
        let serialized_size = serde_json::to_vec(&descriptor)
            .map_err(|error| format!("cannot serialize local toolchain descriptor: {error}"))?
            .len();
        if u64::try_from(serialized_size).unwrap_or(u64::MAX) > MAX_DESCRIPTOR_BYTES {
            return Err("local toolchain descriptor exceeds 16 MiB".into());
        }
        Ok(descriptor)
    }

    /// Remeasure the prefix and return its verified v3 executable layout.
    ///
    /// A coexisting release manifest is rejected even if malformed: the
    /// caller must resolve a release through the release contract instead of
    /// silently treating that directory as a local-only prefix.
    ///
    /// # Errors
    /// Rejects an unsafe root, release-manifest ambiguity, changed descriptor
    /// bytes or payload inventory, invalid GNU layout, or escaping symlink.
    pub fn verify(&self, root: &Path) -> Result<ToolchainToolLayout, String> {
        let root = validate_local_root(root)?;
        reject_release_manifest(&root)?;
        self.validate()?;
        validate_optional_local_descriptor(&root)?;
        if let Some(on_disk) = read_optional_descriptor(&root)? {
            if on_disk != *self {
                return Err(
                    "on-disk local toolchain descriptor differs from the selected descriptor"
                        .into(),
                );
            }
        }

        let (tree_sha256, files) =
            toolchain_tree_inventory_excluding(&root, &[LOCAL_TOOLCHAIN_DESCRIPTOR_FILE])
                .map_err(|error| format!("cannot inventory local GNU toolchain: {error}"))?;
        if tree_sha256 != self.tree_sha256 || files != self.files {
            return Err("local toolchain payload differs from its descriptor".into());
        }

        let layout = ToolchainToolLayout::load(&root)?;
        if layout.sha256().as_str() != self.tool_layout_sha256 {
            return Err("GNU executable layout bytes differ from the local descriptor".into());
        }
        layout.validate_binding(&self.compiler, &self.target_triple)?;
        if !layout.has_objdump_role() {
            return Err("local GNU toolchain requires a v3 objdump role".into());
        }
        layout.resolve_tools(&root)?;
        self.verify_inventory_symlinks(&root)?;
        Ok(layout)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != LOCAL_TOOLCHAIN_SCHEMA {
            return Err("unsupported local toolchain descriptor schema".into());
        }
        if self.qualification != LOCAL_TOOLCHAIN_QUALIFICATION {
            return Err("unsupported local toolchain qualification".into());
        }
        validate_identity(
            &self.host,
            &self.target_profile,
            &self.target_triple,
            &self.compiler,
        )?;
        validate_sha256("tree_sha256", &self.tree_sha256)?;
        validate_sha256("tool_layout_sha256", &self.tool_layout_sha256)?;
        if self.files.is_empty() {
            return Err("local toolchain inventory must not be empty".into());
        }
        if self.files.len() > MAX_INVENTORY_ENTRIES {
            return Err("local toolchain inventory exceeds 100000 entries".into());
        }

        let mut previous: Option<&str> = None;
        for entry in &self.files {
            validate_inventory_path(&entry.path)?;
            if matches!(
                entry.path.as_str(),
                LOCAL_TOOLCHAIN_DESCRIPTOR_FILE | AROS_TOOLCHAIN_MANIFEST_FILE
            ) {
                return Err(format!(
                    "local toolchain inventory must not contain '{}'",
                    entry.path
                ));
            }
            if previous.is_some_and(|prior| prior >= entry.path.as_str()) {
                return Err("local toolchain inventory must be strictly path-sorted".into());
            }
            previous = Some(&entry.path);
            validate_inventory_entry(entry)?;
        }
        let layout_entry = self
            .files
            .iter()
            .find(|entry| entry.path == TOOLCHAIN_TOOLS_FILE)
            .ok_or_else(|| "local inventory omits toolchain-tools.json".to_string())?;
        if layout_entry.kind != "file"
            || layout_entry.sha256.as_deref() != Some(self.tool_layout_sha256.as_str())
        {
            return Err("local inventory does not bind the declared GNU layout bytes".into());
        }
        let measured_tree = toolchain_inventory_sha256(&self.files)
            .map_err(|error| format!("cannot hash local toolchain inventory: {error}"))?;
        if measured_tree != self.tree_sha256 {
            return Err("local toolchain tree digest does not match its inventory".into());
        }
        Ok(())
    }

    fn verify_inventory_symlinks(&self, root: &Path) -> Result<(), String> {
        let canonical_root = root
            .canonicalize()
            .map_err(|error| format!("cannot resolve local toolchain root: {error}"))?;
        for entry in self.files.iter().filter(|entry| entry.kind == "symlink") {
            let path = root.join(&entry.path);
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "cannot inspect local toolchain symlink '{}': {error}",
                    entry.path
                )
            })?;
            if !metadata.file_type().is_symlink() {
                return Err(format!(
                    "local toolchain inventory entry '{}' is no longer a symlink",
                    entry.path
                ));
            }
            let target = fs::read_link(&path).map_err(|error| {
                format!(
                    "cannot read local toolchain symlink '{}': {error}",
                    entry.path
                )
            })?;
            if target.to_str() != entry.target.as_deref() {
                return Err(format!(
                    "local toolchain symlink '{}' differs from its inventory",
                    entry.path
                ));
            }
            let resolved = path.canonicalize().map_err(|error| {
                format!(
                    "local toolchain symlink '{}' is dangling: {error}",
                    entry.path
                )
            })?;
            if !resolved.starts_with(&canonical_root) {
                return Err(format!(
                    "local toolchain symlink '{}' resolves outside its prefix",
                    entry.path
                ));
            }
        }
        Ok(())
    }
}

fn validate_identity(
    host: &str,
    target_profile: &str,
    target_triple: &str,
    compiler: &ArosCompilerIdentity,
) -> Result<(), String> {
    if !matches!(host, "linux-x86_64" | "linux-aarch64" | "macos-aarch64") {
        return Err("local toolchain host is unsupported".into());
    }
    if target_profile.len() > 128 || !crate::media_profile::valid_target_preset(target_profile) {
        return Err("local toolchain target_profile is not a portable target token".into());
    }
    if !matches!(compiler, ArosCompilerIdentity::Gnu { .. }) {
        return Err("local toolchain descriptor requires a GNU compiler identity".into());
    }
    compiler
        .validate_for_target(target_triple)
        .map_err(|error| format!("invalid local GNU compiler target binding: {error}"))
}

fn validate_sha256(field: &str, value: &str) -> Result<(), String> {
    let parsed =
        Sha256Digest::parse(value).map_err(|_| format!("{field} is not a SHA-256 digest"))?;
    if parsed.as_str() != value {
        return Err(format!("{field} must use lowercase hexadecimal"));
    }
    Ok(())
}

fn validate_inventory_path(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('\\') || value.contains(':') {
        return Err(format!("local inventory path '{value}' is unsafe"));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
        })
    {
        return Err(format!("local inventory path '{value}' is unsafe"));
    }
    Ok(())
}

fn validate_inventory_entry(entry: &ArosToolchainManifestEntry) -> Result<(), String> {
    match entry.kind.as_str() {
        "directory"
            if entry.mode == "0755"
                && entry.sha256.is_none()
                && entry.size.is_none()
                && entry.target.is_none() =>
        {
            Ok(())
        }
        "file"
            if matches!(entry.mode.as_str(), "0644" | "0755")
                && entry.size.is_some()
                && entry.target.is_none() =>
        {
            validate_sha256(
                "inventory sha256",
                entry
                    .sha256
                    .as_deref()
                    .ok_or_else(|| "file inventory entry must contain sha256".to_string())?,
            )
        }
        "symlink" if entry.mode == "0777" && entry.sha256.is_none() && entry.size.is_none() => {
            let target = entry
                .target
                .as_deref()
                .filter(|target| !target.is_empty())
                .ok_or_else(|| {
                    "symlink inventory entry must contain a nonempty target".to_string()
                })?;
            validate_symlink_target(&entry.path, target)
        }
        _ => Err(format!(
            "invalid type, mode, or fields for local inventory entry '{}'",
            entry.path
        )),
    }
}

fn validate_symlink_target(path: &str, target: &str) -> Result<(), String> {
    if target.contains('\\')
        || target.contains(':')
        || target.chars().any(char::is_control)
        || Path::new(target).is_absolute()
    {
        return Err(format!("local symlink '{path}' has an unsafe target"));
    }
    let mut depth = Path::new(path).parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
    });
    for component in Path::new(target).components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("local symlink '{path}' escapes its prefix"));
            }
        }
    }
    Ok(())
}

fn validate_local_root(root: &Path) -> Result<PathBuf, String> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(root)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err("local toolchain root must be an absolute normalized path".into());
            }
        }
    }
    validate_existing_directory_prefix_nofollow(&normalized)
        .map_err(|error| format!("invalid local toolchain root: {error}"))?;
    let metadata = fs::symlink_metadata(&normalized)
        .map_err(|error| format!("cannot inspect local toolchain root: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("local toolchain root is not a real directory".into());
    }
    normalized
        .canonicalize()
        .map_err(|error| format!("cannot resolve local toolchain root: {error}"))
}

fn reject_release_manifest(root: &Path) -> Result<(), String> {
    match fs::symlink_metadata(root.join(AROS_TOOLCHAIN_MANIFEST_FILE)) {
        Ok(_) => Err("local-only toolchain route rejects a coexisting release manifest".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect release manifest path: {error}")),
    }
}

fn validate_optional_local_descriptor(root: &Path) -> Result<(), String> {
    let path = root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("local toolchain descriptor path is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect local toolchain descriptor path: {error}"
        )),
    }
}

fn read_optional_descriptor(root: &Path) -> Result<Option<LocalToolchainDescriptor>, String> {
    let path = root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot inspect local toolchain descriptor path: {error}"
        )),
        Ok(_) => crate::measure_regular_file_bounded(&path, MAX_DESCRIPTOR_BYTES)
            .map_err(|error| format!("cannot read local toolchain descriptor: {error}"))?
            .map(|(_, bytes)| LocalToolchainDescriptor::parse(&bytes))
            .transpose(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::elf::riscv::TargetContract;
    use serde_json::{json, Value};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use tempfile::TempDir;

    const TRIPLE: &str = "riscv-aros";

    fn compiler() -> ArosCompilerIdentity {
        ArosCompilerIdentity::Gnu {
            gcc_version: "16.2.0".into(),
            binutils_version: "2.47".into(),
            target: TargetContract::parse(
                br#"{"schema":"aros-riscv-target-v1","isa":"rv32imafc_zicsr_zifencei_zaamo_zalrsc","abi":"ilp32f","code_model":"medany","architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0","unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0}"#,
            )
            .unwrap(),
        }
    }

    fn executable(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn layout_bytes(identity: &ArosCompilerIdentity, schema: &str) -> Vec<u8> {
        let mut tools = json!({
            "c": "bin/gcc",
            "cxx": "bin/g++",
            "assembler": "bin/as",
            "linker": "bin/ld",
            "archive": "bin/ar",
            "ranlib": "bin/ranlib",
            "strip": "bin/strip",
            "collector": "bin/collect-aros",
            "nm": "bin/nm",
            "objcopy": "bin/objcopy"
        });
        if schema == "aros-toolchain-tools-v3" {
            tools["objdump"] = json!("bin/objdump");
        }
        serde_json::to_vec(&json!({
            "schema": schema,
            "compiler": identity,
            "target_triple": TRIPLE,
            "tools": tools
        }))
        .unwrap()
    }

    fn fixture() -> (TempDir, ArosCompilerIdentity) {
        let root = tempfile::tempdir().unwrap();
        let identity = compiler();
        for path in [
            "bin/gcc",
            "bin/g++",
            "bin/as",
            "bin/ld",
            "bin/ar",
            "bin/ranlib",
            "bin/strip",
            "bin/collect-aros",
            "bin/nm",
            "bin/objcopy",
            "bin/objdump",
        ] {
            executable(root.path(), path);
        }
        fs::write(
            root.path().join(TOOLCHAIN_TOOLS_FILE),
            layout_bytes(&identity, "aros-toolchain-tools-v3"),
        )
        .unwrap();
        (root, identity)
    }

    fn capture(root: &Path, identity: ArosCompilerIdentity) -> LocalToolchainDescriptor {
        LocalToolchainDescriptor::capture(root, "macos-aarch64", "esp32p4-d1001", TRIPLE, identity)
            .unwrap()
    }

    fn descriptor_json(descriptor: &LocalToolchainDescriptor) -> Value {
        serde_json::to_value(descriptor).unwrap()
    }

    #[test]
    fn captures_loads_and_verifies_a_local_only_prefix() {
        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity);
        assert!(!descriptor_json(&descriptor)
            .as_object()
            .unwrap()
            .contains_key("release_id"));
        assert!(descriptor.verify(root.path()).is_ok());

        let bytes = serde_json::to_vec(&descriptor).unwrap();
        fs::write(root.path().join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE), &bytes).unwrap();
        let loaded = LocalToolchainDescriptor::load(root.path()).unwrap();
        assert_eq!(loaded, descriptor);
        assert!(loaded.verify(root.path()).unwrap().has_objdump_role());
    }

    #[test]
    fn parser_rejects_release_provenance_fields_and_duplicate_keys() {
        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity);
        for (field, value) in [
            ("release_id", json!("invented-release")),
            ("source_commit", json!("0".repeat(40))),
            ("producer_commit", json!("1".repeat(40))),
        ] {
            let mut value_document = descriptor_json(&descriptor);
            value_document[field] = value;
            assert!(
                LocalToolchainDescriptor::parse(&serde_json::to_vec(&value_document).unwrap())
                    .is_err()
            );
        }

        let bytes = serde_json::to_vec(&descriptor).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let duplicated = text.replacen(
            "\"schema\":\"aros-local-toolchain-v1\"",
            "\"schema\":\"aros-local-toolchain-v1\",\"schema\":\"aros-local-toolchain-v1\"",
            1,
        );
        assert!(LocalToolchainDescriptor::parse(duplicated.as_bytes()).is_err());
    }

    #[test]
    fn capture_rejects_missing_wrong_or_non_v3_layouts() {
        let (root, identity) = fixture();
        let layout_path = root.path().join(TOOLCHAIN_TOOLS_FILE);
        fs::remove_file(&layout_path).unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity.clone(),
        )
        .is_err());

        fs::write(
            &layout_path,
            layout_bytes(&identity, "aros-toolchain-tools-v2"),
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity.clone(),
        )
        .is_err());

        let mut missing_objdump: Value =
            serde_json::from_slice(&layout_bytes(&identity, "aros-toolchain-tools-v3")).unwrap();
        missing_objdump["tools"]
            .as_object_mut()
            .unwrap()
            .remove("objdump");
        fs::write(&layout_path, serde_json::to_vec(&missing_objdump).unwrap()).unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity.clone(),
        )
        .is_err());

        let v3 = layout_bytes(&identity, "aros-toolchain-tools-v3");
        let wrong_triple = String::from_utf8(v3)
            .unwrap()
            .replace(TRIPLE, "riscv64-aros");
        fs::write(&layout_path, wrong_triple).unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity,
        )
        .is_err());
    }

    #[test]
    fn verification_rejects_a_changed_tree_or_layout() {
        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity);
        fs::write(root.path().join("bin/gcc"), b"changed compiler\n").unwrap();
        assert!(descriptor.verify(root.path()).is_err());

        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity.clone());
        fs::write(
            root.path().join(TOOLCHAIN_TOOLS_FILE),
            layout_bytes(&identity, "aros-toolchain-tools-v2"),
        )
        .unwrap();
        assert!(descriptor.verify(root.path()).is_err());
    }

    #[test]
    fn parser_and_loader_bound_descriptor_size_and_reject_descriptor_symlinks() {
        let oversized = vec![b' '; usize::try_from(MAX_DESCRIPTOR_BYTES).unwrap() + 1];
        assert!(LocalToolchainDescriptor::parse(&oversized).is_err());

        let (oversized_root, _) = fixture();
        fs::write(
            oversized_root.path().join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
            &oversized,
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::load(oversized_root.path()).is_err());

        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity);
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), serde_json::to_vec(&descriptor).unwrap()).unwrap();
        symlink(
            outside.path(),
            root.path().join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::load(root.path()).is_err());
        assert!(descriptor.verify(root.path()).is_err());
    }

    #[test]
    fn root_and_inventory_symlinks_must_remain_inside_the_prefix() {
        let (root, identity) = fixture();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("outside.txt"), b"outside\n").unwrap();
        symlink(
            external.path().join("outside.txt"),
            root.path().join("external-link"),
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity,
        )
        .is_err());

        let parent = tempfile::tempdir().unwrap();
        let linked_root = parent.path().join("prefix-link");
        symlink(root.path(), &linked_root).unwrap();
        assert!(validate_local_root(&linked_root).is_err());
    }

    #[test]
    fn a_coexisting_release_manifest_never_downgrades_to_local_only() {
        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity.clone());
        fs::write(
            root.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
            b"invalid release json",
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::load(root.path()).is_err());
        assert!(descriptor.verify(root.path()).is_err());
        fs::remove_file(root.path().join(AROS_TOOLCHAIN_MANIFEST_FILE)).unwrap();

        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(
            outside.path(),
            root.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
        )
        .unwrap();
        assert!(LocalToolchainDescriptor::capture(
            root.path(),
            "macos-aarch64",
            "esp32p4-d1001",
            TRIPLE,
            identity,
        )
        .is_err());
    }

    #[test]
    fn parse_rejects_unsorted_duplicate_and_unsafe_inventory_paths() {
        let (root, identity) = fixture();
        let descriptor = capture(root.path(), identity);
        let original_files = descriptor_json(&descriptor)["files"]
            .as_array()
            .unwrap()
            .clone();

        let mut duplicated = original_files.clone();
        duplicated.push(duplicated[0].clone());
        duplicated.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        let mut value = descriptor_json(&descriptor);
        value["files"] = json!(duplicated);
        assert!(LocalToolchainDescriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut unsorted = original_files;
        let last_index = unsorted.len() - 1;
        unsorted.swap(0, last_index);
        let mut value = descriptor_json(&descriptor);
        value["files"] = json!(unsorted);
        assert!(LocalToolchainDescriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = descriptor_json(&descriptor);
        value["files"][0]["path"] = json!("../escape");
        assert!(LocalToolchainDescriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = descriptor_json(&descriptor);
        value["files"][0]["type"] = json!("device");
        assert!(LocalToolchainDescriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = descriptor_json(&descriptor);
        let gcc_index = value["files"]
            .as_array()
            .unwrap()
            .iter()
            .position(|entry| entry["path"] == "bin/gcc")
            .unwrap();
        value["files"][gcc_index]["sha256"] = json!("invalid");
        assert!(LocalToolchainDescriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
