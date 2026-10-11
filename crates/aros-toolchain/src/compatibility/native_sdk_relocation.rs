//! Bounded, revalidatable copies of a native SDK input tree.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

use aros_common::{
    copy_tree_from_snapshot_nofollow, directory_entry_names_nofollow_bounded,
    measure_regular_file_digest_bounded, measure_tree_content_cas_bounded,
    normalized_toolchain_file_mode, toolchain_inventory_sha256, ArosToolchainManifestEntry,
    Sha256Digest, TreeContentCas, TreeTraversalLimits,
};

use crate::filesystem::open_directory;
use crate::ContractError;

const SDK_TREE_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_000,
    max_regular_file_bytes: 2 * 1024 * 1024 * 1024,
};
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_INVENTORY_DEPTH: usize = 128;

/// A fresh byte-for-byte copy of one canonical native SDK tree.
///
/// The inventories include every entry, including an embedded
/// `toolchain-manifest.json`; this is ordinary input to this operation.
pub(super) struct RelocatedSdk {
    pub(super) original: PathBuf,
    pub(super) relocated: PathBuf,
    pub(super) inventory_sha256: Sha256Digest,
    pub(super) inventory: Vec<ArosToolchainManifestEntry>,
    original_snapshot: TreeContentCas,
    relocated_snapshot: TreeContentCas,
    original_root: RootIdentity,
    relocated_root: RootIdentity,
}

impl RelocatedSdk {
    /// Measure and copy an SDK into a fresh, disjoint destination.
    pub(super) fn prepare(original: &Path, destination: &Path) -> Result<Self, ContractError> {
        let original = checked_real_root(original, "native SDK source")?;
        let relocated = checked_absent_root(destination, "native SDK relocation")?;
        if roots_overlap(&original, &relocated) {
            return Err(failure(
                "native SDK source and relocation roots must be distinct and non-overlapping",
            ));
        }

        let original_root = root_identity(&original)?;
        let original_snapshot = measure(&original, "native SDK source")?;
        let (inventory_sha256, source_inventory) = inventory(&original, "native SDK source")?;
        validate_relative_symlinks(&original, &source_inventory)?;
        if root_identity(&original)? != original_root {
            return Err(failure("native SDK source root changed during inspection"));
        }

        // Recheck the no-clobber condition immediately before creating the
        // leaf. create_dir itself also fails if another process wins the race.
        let parent = relocated
            .parent()
            .ok_or_else(|| failure("native SDK relocation root has no canonical parent"))?;
        open_directory(parent).map_err(|_| {
            failure("native SDK relocation parent is not a real no-follow directory")
        })?;
        ensure_absent(&relocated, "native SDK relocation")?;
        fs::create_dir(&relocated)
            .map_err(|_| failure("cannot create the fresh native SDK relocation root"))?;
        open_directory(&relocated)
            .map_err(|_| failure("fresh native SDK relocation root is not a real directory"))?;

        copy_tree_from_snapshot_nofollow(
            &original,
            &relocated,
            &original_snapshot,
            SDK_TREE_LIMITS,
        )
        .map_err(|_| {
            failure("cannot copy the bounded native SDK snapshot without following links")
        })?;

        set_root_mode(&relocated, original_root.mode)?;
        let relocated_root = root_identity(&relocated)?;
        if relocated_root.mode != original_root.mode {
            return Err(failure("native SDK relocation root mode was not preserved"));
        }

        let relocated_snapshot = measure(&relocated, "native SDK relocation")?;
        if relocated_snapshot.payload_digest_excluding(None)
            != original_snapshot.payload_digest_excluding(None)
        {
            return Err(failure(
                "native SDK relocation differs from its measured source bytes or modes",
            ));
        }
        let (relocated_inventory_sha256, relocated_inventory) =
            inventory(&relocated, "native SDK relocation")?;
        validate_relative_symlinks(&relocated, &relocated_inventory)?;
        if relocated_inventory_sha256 != inventory_sha256 || relocated_inventory != source_inventory
        {
            return Err(failure(
                "native SDK relocation inventory differs from its source",
            ));
        }
        if measure(&original, "native SDK source")? != original_snapshot
            || measure(&relocated, "native SDK relocation")? != relocated_snapshot
        {
            return Err(failure(
                "native SDK changed while its relocated inventory was checked",
            ));
        }
        if root_identity(&original)? != original_root
            || root_identity(&relocated)? != relocated_root
        {
            return Err(failure("native SDK root changed during relocation"));
        }

        Ok(Self {
            original,
            relocated,
            inventory_sha256,
            inventory: source_inventory,
            original_snapshot,
            relocated_snapshot,
            original_root,
            relocated_root,
        })
    }

    /// Recheck both exact trees against the source baseline captured at prepare.
    pub(super) fn revalidate(&self) -> Result<(), ContractError> {
        if checked_real_root(&self.original, "native SDK source")? != self.original
            || checked_real_root(&self.relocated, "native SDK relocation")? != self.relocated
        {
            return Err(failure("native SDK root is no longer canonical"));
        }
        if root_identity(&self.original)? != self.original_root
            || root_identity(&self.relocated)? != self.relocated_root
        {
            return Err(failure(
                "native SDK root identity or mode changed after relocation",
            ));
        }

        let original_snapshot = measure(&self.original, "native SDK source")?;
        let relocated_snapshot = measure(&self.relocated, "native SDK relocation")?;
        if original_snapshot != self.original_snapshot
            || relocated_snapshot != self.relocated_snapshot
            || original_snapshot.payload_digest_excluding(None)
                != relocated_snapshot.payload_digest_excluding(None)
        {
            return Err(failure(
                "native SDK source or relocation changed after preparation",
            ));
        }

        let (original_digest, original_inventory) = inventory(&self.original, "native SDK source")?;
        let (relocated_digest, relocated_inventory) =
            inventory(&self.relocated, "native SDK relocation")?;
        validate_relative_symlinks(&self.original, &original_inventory)?;
        validate_relative_symlinks(&self.relocated, &relocated_inventory)?;
        if original_digest != self.inventory_sha256
            || relocated_digest != self.inventory_sha256
            || original_inventory != self.inventory
            || relocated_inventory != self.inventory
        {
            return Err(failure("native SDK inventory changed after preparation"));
        }
        if measure(&self.original, "native SDK source")? != self.original_snapshot
            || measure(&self.relocated, "native SDK relocation")? != self.relocated_snapshot
        {
            return Err(failure(
                "native SDK changed while its revalidation inventory was checked",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootIdentity {
    device: u64,
    inode: u64,
    mode: u32,
}

fn checked_real_root(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(failure(format!("{label} root must be absolute")));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| failure(format!("{label} root cannot be canonicalized")))?;
    open_directory(&canonical)
        .map_err(|_| failure(format!("{label} root is not a real no-follow directory")))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| failure(format!("cannot inspect {label} root")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure(format!("{label} root is not a real directory")));
    }
    Ok(canonical)
}

fn checked_absent_root(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(failure(format!("{label} root must be absolute")));
    }
    let name = path
        .file_name()
        .ok_or_else(|| failure(format!("{label} root must have one final path segment")))?;
    if !matches!(
        Path::new(name).components().next(),
        Some(Component::Normal(_))
    ) {
        return Err(failure(format!(
            "{label} root has an unsafe final path segment"
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| failure(format!("{label} root has no parent directory")))?
        .canonicalize()
        .map_err(|_| failure(format!("{label} parent cannot be canonicalized")))?;
    open_directory(&parent)
        .map_err(|_| failure(format!("{label} parent is not a real no-follow directory")))?;
    let root = parent.join(name);
    ensure_absent(&root, label)?;
    Ok(root)
}

fn ensure_absent(path: &Path, label: &str) -> Result<(), ContractError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(failure(format!(
            "{label} destination already exists and cannot be adopted"
        ))),
        Err(_) => Err(failure(format!(
            "cannot safely inspect {label} destination"
        ))),
    }
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn root_identity(path: &Path) -> Result<RootIdentity, ContractError> {
    let handle = open_directory(path)
        .map_err(|_| failure("native SDK root is not a real no-follow directory"))?;
    let opened = handle
        .metadata()
        .map_err(|_| failure("cannot inspect opened native SDK root"))?;
    let named =
        fs::symlink_metadata(path).map_err(|_| failure("cannot inspect named native SDK root"))?;
    if !opened.is_dir()
        || !named.is_dir()
        || named.file_type().is_symlink()
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
    {
        return Err(failure("native SDK root changed while it was inspected"));
    }
    Ok(RootIdentity {
        device: opened.dev(),
        inode: opened.ino(),
        mode: opened.permissions().mode() & 0o7777,
    })
}

fn set_root_mode(path: &Path, mode: u32) -> Result<(), ContractError> {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .map_err(|_| failure("cannot preserve native SDK relocation root mode"))
}

fn measure(path: &Path, label: &str) -> Result<TreeContentCas, ContractError> {
    measure_tree_content_cas_bounded(path, SDK_TREE_LIMITS)
        .map_err(|_| failure(format!("cannot measure bounded {label} tree")))
}

pub(super) fn inventory(
    root: &Path,
    label: &str,
) -> Result<(Sha256Digest, Vec<ArosToolchainManifestEntry>), ContractError> {
    let mut entries = Vec::new();
    let mut budget = InventoryBudget {
        entries: 0,
        regular_file_bytes: 0,
        encoded_bytes: 3, // `[]` plus the trailing newline.
    };
    collect_inventory(root, Path::new(""), &mut entries, &mut budget, label)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let digest = toolchain_inventory_sha256(&entries)
        .map_err(|_| failure(format!("cannot hash complete {label} inventory")))?;
    let digest = Sha256Digest::parse(&digest)
        .map_err(|_| failure(format!("{label} inventory produced an invalid digest")))?;
    Ok((digest, entries))
}

/// Encode an already bounded SDK inventory while enforcing the output ceiling
/// as serde writes bytes. The trailing newline is part of the retained form.
pub(super) fn encode_inventory(
    entries: &[ArosToolchainManifestEntry],
) -> Result<Vec<u8>, ContractError> {
    if entries.len() > SDK_TREE_LIMITS.max_entries {
        return Err(failure("native SDK inventory exceeds its entry limit"));
    }
    let mut writer = BoundedInventoryWriter {
        bytes: Vec::with_capacity(4 * 1024),
        limit: MAX_INVENTORY_BYTES,
    };
    serde_json::to_writer(&mut writer, entries)
        .map_err(|_| failure("native SDK inventory exceeds its encoding limit"))?;
    writer
        .write_all(b"\n")
        .map_err(|_| failure("native SDK inventory exceeds its encoding limit"))?;
    Ok(writer.bytes)
}

struct BoundedInventoryWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl io::Write for BoundedInventoryWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("SDK inventory length overflowed"))?;
        if next > self.limit {
            return Err(io::Error::other("SDK inventory exceeds its byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct InventoryBudget {
    entries: usize,
    regular_file_bytes: u64,
    encoded_bytes: usize,
}

fn collect_inventory(
    root: &Path,
    relative: &Path,
    output: &mut Vec<ArosToolchainManifestEntry>,
    budget: &mut InventoryBudget,
    label: &str,
) -> Result<(), ContractError> {
    let depth = relative.components().count();
    if depth > MAX_INVENTORY_DEPTH {
        return Err(failure(format!(
            "{label} exceeds the {MAX_INVENTORY_DEPTH}-component inventory depth limit"
        )));
    }

    let directory = root.join(relative);
    let directory_before = fs::symlink_metadata(&directory)
        .map_err(|_| failure(format!("cannot inspect {label} inventory directory")))?;
    if !directory_before.is_dir() || directory_before.file_type().is_symlink() {
        return Err(failure(format!(
            "{label} inventory encountered a non-directory parent"
        )));
    }
    let remaining_entries = SDK_TREE_LIMITS.max_entries.saturating_sub(budget.entries);
    let names = directory_entry_names_nofollow_bounded(&directory, remaining_entries)
        .map_err(|_| failure(format!("cannot list bounded {label} inventory directory")))?;
    let mut names = names;
    names.sort();

    for name in names {
        if budget.entries >= SDK_TREE_LIMITS.max_entries {
            return Err(failure(format!(
                "{label} exceeds its inventory entry limit"
            )));
        }
        let child_relative = relative.join(&name);
        let entry_path = root.join(&child_relative);
        let metadata = fs::symlink_metadata(&entry_path)
            .map_err(|_| failure(format!("cannot inspect {label} inventory entry")))?;
        let path = portable_relative_path(&child_relative)
            .map_err(|_| failure(format!("{label} contains a non-portable inventory path")))?;
        let entry = if metadata.file_type().is_symlink() {
            let target = fs::read_link(&entry_path)
                .map_err(|_| failure(format!("cannot read {label} symbolic-link target")))?;
            let target = target
                .to_str()
                .ok_or_else(|| failure(format!("{label} has a non-UTF-8 symbolic-link target")))?;
            ArosToolchainManifestEntry {
                path,
                mode: "0777".into(),
                kind: "symlink".into(),
                sha256: None,
                size: None,
                target: Some(target.into()),
            }
        } else if metadata.is_dir() {
            ArosToolchainManifestEntry {
                path,
                mode: "0755".into(),
                kind: "directory".into(),
                sha256: None,
                size: None,
                target: None,
            }
        } else if metadata.is_file() {
            let remaining_bytes = SDK_TREE_LIMITS
                .max_regular_file_bytes
                .saturating_sub(budget.regular_file_bytes);
            let (identity, measured) =
                measure_regular_file_digest_bounded(&entry_path, remaining_bytes)
                    .map_err(|_| failure(format!("cannot hash bounded {label} inventory file")))?;
            let after = fs::symlink_metadata(&entry_path)
                .map_err(|_| failure(format!("cannot recheck {label} inventory file")))?;
            if !after.is_file()
                || after.file_type().is_symlink()
                || identity.device() != after.dev()
                || identity.inode() != after.ino()
                || measured.size != after.len()
            {
                return Err(failure(format!(
                    "{label} inventory file changed while hashing"
                )));
            }
            budget.regular_file_bytes = budget
                .regular_file_bytes
                .checked_add(measured.size)
                .ok_or_else(|| failure(format!("{label} file-byte inventory limit overflowed")))?;
            if budget.regular_file_bytes > SDK_TREE_LIMITS.max_regular_file_bytes {
                return Err(failure(format!(
                    "{label} exceeds its regular-file byte limit"
                )));
            }
            ArosToolchainManifestEntry {
                path,
                mode: format!("{:04o}", normalized_toolchain_file_mode(&after)),
                kind: "file".into(),
                sha256: Some(measured.digest.to_string()),
                size: Some(measured.size),
                target: None,
            }
        } else {
            return Err(failure(format!(
                "{label} contains an unsupported filesystem entry"
            )));
        };

        // Count each entry's exact JSON representation before retaining it, so
        // a tree full of long paths or link targets cannot build oversized
        // metadata in memory and only fail at final serialization.
        let encoded_entry = serde_json::to_vec(&entry)
            .map_err(|_| failure(format!("cannot encode {label} inventory entry")))?;
        let separator = usize::from(budget.entries != 0);
        budget.encoded_bytes = budget
            .encoded_bytes
            .checked_add(separator)
            .and_then(|size| size.checked_add(encoded_entry.len()))
            .ok_or_else(|| failure(format!("{label} inventory encoding length overflowed")))?;
        if budget.encoded_bytes > MAX_INVENTORY_BYTES {
            return Err(failure(format!(
                "{label} inventory exceeds its {MAX_INVENTORY_BYTES}-byte metadata limit"
            )));
        }
        budget.entries += 1;
        output.push(entry);
        if metadata.is_dir() {
            collect_inventory(root, &child_relative, output, budget, label)?;
        }
    }

    let directory_after = fs::symlink_metadata(&directory)
        .map_err(|_| failure(format!("cannot recheck {label} inventory directory")))?;
    if !directory_after.is_dir()
        || directory_after.file_type().is_symlink()
        || directory_before.dev() != directory_after.dev()
        || directory_before.ino() != directory_after.ino()
        || directory_before.mtime() != directory_after.mtime()
        || directory_before.mtime_nsec() != directory_after.mtime_nsec()
    {
        return Err(failure(format!("{label} inventory directory changed")));
    }
    Ok(())
}

fn portable_relative_path(path: &Path) -> Result<String, ContractError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => components.push(
                value
                    .to_str()
                    .ok_or_else(|| failure("native SDK path is not valid UTF-8"))?,
            ),
            _ => return Err(failure("native SDK path is not a portable relative path")),
        }
    }
    if components.is_empty() {
        return Err(failure("native SDK entry path is empty"));
    }
    Ok(components.join("/"))
}

fn validate_relative_symlinks(
    root: &Path,
    entries: &[ArosToolchainManifestEntry],
) -> Result<(), ContractError> {
    for entry in entries.iter().filter(|entry| entry.kind == "symlink") {
        let target = entry.target.as_deref().ok_or_else(|| {
            failure("native SDK inventory contains a symbolic link without a target")
        })?;
        let target_path = Path::new(target);
        if target.is_empty() || target_path.is_absolute() {
            return Err(failure(
                "native SDK symbolic links must have non-empty relative targets",
            ));
        }

        let parent = Path::new(&entry.path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let mut resolved = Vec::<std::ffi::OsString>::new();
        for component in parent.components().chain(target_path.components()) {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => resolved.push(name.to_os_string()),
                Component::ParentDir if resolved.pop().is_some() => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(failure(
                        "native SDK symbolic link escapes its original root",
                    ));
                }
            }
        }
        let mut target = root.to_path_buf();
        for component in resolved {
            target.push(component);
        }
        let canonical_target = target
            .canonicalize()
            .map_err(|_| failure("native SDK symbolic link is dangling or cannot be resolved"))?;
        if !canonical_target.starts_with(root) {
            return Err(failure(
                "native SDK symbolic link resolves outside its root",
            ));
        }
    }
    Ok(())
}

fn failure(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let original = parent.join("sdk");
        let destination = parent.join("relocated-sdk");
        fs::create_dir(&original).unwrap();
        (temporary, original, destination)
    }

    fn write_file(path: &Path, contents: &[u8], mode: u32) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn copies_files_relative_links_empty_directories_and_modes() {
        let (_temporary, original, destination) = fixture();
        fs::create_dir(original.join("bin")).unwrap();
        fs::create_dir(original.join("lib")).unwrap();
        fs::create_dir(original.join("empty")).unwrap();
        write_file(&original.join("lib/tool"), b"sdk bytes", 0o751);
        write_file(
            &original.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE),
            b"ordinary SDK input",
            0o640,
        );
        fs::set_permissions(original.join("empty"), fs::Permissions::from_mode(0o711)).unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o750)).unwrap();
        symlink("../lib/tool", original.join("bin/tool")).unwrap();

        let relocated = RelocatedSdk::prepare(&original, &destination).unwrap();
        relocated.revalidate().unwrap();

        assert_eq!(
            fs::read(relocated.relocated.join("lib/tool")).unwrap(),
            b"sdk bytes"
        );
        assert_eq!(
            fs::metadata(relocated.relocated.join("lib/tool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o751
        );
        assert_eq!(
            fs::metadata(relocated.relocated.join("empty"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o711
        );
        assert_eq!(
            fs::metadata(&relocated.relocated)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o750
        );
        assert_eq!(
            fs::read_link(relocated.relocated.join("bin/tool")).unwrap(),
            PathBuf::from("../lib/tool")
        );
        assert!(relocated
            .inventory
            .iter()
            .any(|entry| entry.path == aros_common::AROS_TOOLCHAIN_MANIFEST_FILE));

        let (expected_digest, expected_inventory) =
            aros_common::toolchain_tree_inventory_excluding(&relocated.original, &[]).unwrap();
        assert_eq!(relocated.inventory_sha256.as_str(), expected_digest);
        assert_eq!(relocated.inventory, expected_inventory);
        assert_eq!(
            encode_inventory(&relocated.inventory).unwrap(),
            [
                serde_json::to_vec(&relocated.inventory).unwrap(),
                b"\n".to_vec()
            ]
            .concat()
        );
    }

    #[test]
    fn revalidation_rejects_changes_to_either_tree() {
        let (_temporary, original, destination) = fixture();
        write_file(&original.join("tool"), b"before", 0o755);
        let relocated = RelocatedSdk::prepare(&original, &destination).unwrap();
        write_file(&original.join("tool"), b"changed", 0o755);
        assert!(relocated.revalidate().is_err());

        let (_temporary, original, destination) = fixture();
        write_file(&original.join("tool"), b"before", 0o755);
        let relocated = RelocatedSdk::prepare(&original, &destination).unwrap();
        write_file(&destination.join("tool"), b"changed", 0o755);
        assert!(relocated.revalidate().is_err());
    }

    #[test]
    fn rejects_absolute_escaping_and_dangling_links() {
        for target in ["/etc/passwd", "../../outside", "missing"] {
            let (_temporary, original, destination) = fixture();
            symlink(target, original.join("bad-link")).unwrap();
            assert!(
                RelocatedSdk::prepare(&original, &destination).is_err(),
                "{target}"
            );
            assert!(!destination.exists());
            assert!(original.join("bad-link").symlink_metadata().is_ok());
        }
    }

    #[test]
    fn rejects_existing_and_overlapping_destinations_without_touching_source() {
        let (_temporary, original, destination) = fixture();
        write_file(&original.join("tool"), b"preserve", 0o755);
        fs::create_dir(&destination).unwrap();
        assert!(RelocatedSdk::prepare(&original, &destination).is_err());
        assert_eq!(fs::read(original.join("tool")).unwrap(), b"preserve");
        assert!(destination.is_dir());

        let nested_destination = original.join("child");
        assert!(RelocatedSdk::prepare(&original, &nested_destination).is_err());
        assert_eq!(fs::read(original.join("tool")).unwrap(), b"preserve");
        assert!(!nested_destination.exists());
    }

    #[test]
    fn inventory_encoder_enforces_its_limit_while_writing() {
        let oversized = ArosToolchainManifestEntry {
            path: "x".repeat(MAX_INVENTORY_BYTES + 1),
            mode: "0644".into(),
            kind: "file".into(),
            sha256: Some("0".repeat(64)),
            size: Some(1),
            target: None,
        };
        assert!(encode_inventory(&[oversized]).is_err());
    }
}
