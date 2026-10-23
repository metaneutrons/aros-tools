//! Opaque whole-tree compare-and-swap snapshots.

use super::{sha256_bytes, BTreeMap, FileIdentity, Sha256Digest};
use std::ops::Bound::{Excluded, Included, Unbounded};

#[path = "tree_cas_fresh.rs"]
mod fresh_outputs;

/// Filesystem object kind recorded in a measured tree snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeContentEntryKind {
    /// A regular file whose bytes were measured.
    RegularFile,
    /// A directory recorded in the tree.
    Directory,
    /// A symbolic link whose target bytes were measured.
    SymbolicLink,
}

/// Descriptor-measured snapshot used to compare-and-swap a complete tree.
///
/// The representation is intentionally opaque: callers can retain it as a
/// publication precondition and derive a content-only digest, but cannot
/// fabricate identities or omit filesystem objects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeContentCas {
    pub(super) root: FileIdentity,
    pub(super) entries: BTreeMap<Vec<u8>, TreeContentEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TreeContentEntry {
    pub(super) snapshot: TreeNodeSnapshot,
    pub(super) content: Option<Sha256Digest>,
    pub(super) symbolic_link_target: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TreeNodeSnapshot {
    pub(super) identity: FileIdentity,
    pub(super) kind: u8,
    pub(super) mode: u32,
    pub(super) size: i64,
    pub(super) mtime: i64,
    pub(super) mtime_nsec: i64,
    pub(super) ctime: i64,
    pub(super) ctime_nsec: i64,
}

impl TreeContentCas {
    /// Maximum raw target bytes retained for one symbolic link.
    ///
    /// This is a memory bound for stored link metadata, independent of regular-
    /// file byte limits and tree digest formats.
    pub(crate) const MAX_RETAINED_SYMBOLIC_LINK_TARGET_BYTES: usize = 4_095;

    /// Maximum aggregate raw target bytes retained across symbolic links.
    ///
    /// This is a memory bound for stored link metadata, independent of regular-
    /// file byte limits and tree digest formats.
    pub(crate) const MAX_RETAINED_SYMBOLIC_LINK_TARGET_BYTES_TOTAL: usize = 16 * 1024 * 1024;

    /// Return the recorded kind for one exact relative-name key.
    ///
    /// This reads only the already-measured snapshot; it performs no filesystem
    /// I/O and does not canonicalize or otherwise normalize `relative`. Unknown
    /// names return `None`.
    #[must_use]
    pub fn entry_kind(&self, relative: &str) -> Option<TreeContentEntryKind> {
        self.entries
            .get(relative.as_bytes())
            .and_then(|entry| match entry.snapshot.kind {
                1 => Some(TreeContentEntryKind::RegularFile),
                2 => Some(TreeContentEntryKind::Directory),
                3 => Some(TreeContentEntryKind::SymbolicLink),
                _ => None,
            })
    }

    /// Return every measured relative name and its kind in raw-byte key order.
    ///
    /// Names are returned as bytes so non-UTF-8 filesystem names are retained.
    /// The iterator exposes the complete stored measurement, including any
    /// exclusions applied when that snapshot was created. It performs no
    /// filesystem I/O, filtering, or path normalization.
    pub fn entry_kinds(&self) -> impl Iterator<Item = (&[u8], TreeContentEntryKind)> + '_ {
        self.entries.iter().map(|(path, entry)| {
            let kind = match entry.snapshot.kind {
                1 => TreeContentEntryKind::RegularFile,
                2 => TreeContentEntryKind::Directory,
                3 => TreeContentEntryKind::SymbolicLink,
                _ => unreachable!("measured tree snapshots only contain known entry kinds"),
            };
            (path.as_slice(), kind)
        })
    }

    /// Return exact immediate child names and kinds from a captured directory.
    ///
    /// Names are raw bytes because a measured Unix tree can contain names that
    /// are not UTF-8. The result is sorted by the captured byte keys. The root
    /// directory is represented by the empty relative name; other names must
    /// match a recorded directory exactly. This reads no filesystem state and
    /// does not expose any way to construct or alter the snapshot.
    pub(crate) fn directory_entry_kinds(
        &self,
        relative: &str,
    ) -> Option<Vec<(Vec<u8>, TreeContentEntryKind)>> {
        if !relative.is_empty()
            && self.entry_kind(relative) != Some(TreeContentEntryKind::Directory)
        {
            return None;
        }

        let prefix = if relative.is_empty() {
            Vec::new()
        } else {
            let mut prefix = relative.as_bytes().to_vec();
            prefix.push(b'/');
            prefix
        };
        let mut lower = Included(prefix.clone());
        let mut result = Vec::new();
        while let Some((path, entry)) = self.entries.range((lower.clone(), Unbounded)).next() {
            let Some(child) = path.strip_prefix(prefix.as_slice()) else {
                break;
            };
            if child.is_empty() {
                return None;
            }
            if let Some(separator) = child.iter().position(|byte| *byte == b'/') {
                // The directory entry itself sorts before its descendants.
                // Jump only once a descendant is reached: siblings ending in
                // '-' or '.' must not be skipped along with the '/' subtree.
                let mut after_subtree = path[..prefix.len() + separator].to_vec();
                after_subtree.push(b'/' + 1);
                lower = Included(after_subtree);
                continue;
            }
            let kind = match entry.snapshot.kind {
                1 => TreeContentEntryKind::RegularFile,
                2 => TreeContentEntryKind::Directory,
                3 => TreeContentEntryKind::SymbolicLink,
                _ => return None,
            };
            result.push((child.to_vec(), kind));
            lower = Excluded(path.clone());
        }
        Some(result)
    }

    /// Return the measured digest for one exact regular-file name.
    ///
    /// This reads only the already-measured snapshot and performs no filesystem
    /// I/O. The relative-name key is matched byte-for-byte without
    /// canonicalization; missing names, directories, and symbolic links return
    /// `None`.
    #[must_use]
    pub fn regular_file_digest(&self, relative: &str) -> Option<&Sha256Digest> {
        self.entries
            .get(relative.as_bytes())
            .filter(|entry| entry.snapshot.kind == 1)
            .and_then(|entry| entry.content.as_ref())
    }

    /// Return retained raw target bytes for one exact symbolic-link name.
    ///
    /// This reads only the already-measured snapshot and performs no filesystem
    /// I/O. Relative names are matched byte-for-byte without canonicalization;
    /// missing names, directories, and regular files return `None`. The bytes
    /// are independent of the entry's content digest domain.
    #[must_use]
    pub fn symbolic_link_target(&self, relative: &str) -> Option<&[u8]> {
        self.entries
            .get(relative.as_bytes())
            .filter(|entry| entry.snapshot.kind == 3)
            .and_then(|entry| entry.symbolic_link_target.as_deref())
    }

    /// Match one exact regular-file name, measured identity, and content digest.
    ///
    /// This reads only the already-measured snapshot and performs no filesystem
    /// I/O. Relative names are matched byte-for-byte without canonicalization.
    /// Directories, symbolic links, unknown names, and any identity or digest
    /// mismatch return `false`. Permission mode is not part of this comparison.
    #[must_use]
    pub fn matches_regular_file_identity(
        &self,
        relative: &str,
        identity: FileIdentity,
        content: &Sha256Digest,
    ) -> bool {
        self.entries.get(relative.as_bytes()).is_some_and(|entry| {
            entry.snapshot.kind == 1
                && entry.snapshot.identity == identity
                && entry.content.as_ref() == Some(content)
        })
    }

    /// Match one exact directory in this descriptor-measured snapshot.
    /// This does not inspect current filesystem state or authenticate its origin.
    #[must_use]
    pub fn matches_directory(&self, relative: &str, mode: u32) -> bool {
        self.matches_entry(relative, 2, mode, None)
    }

    /// Match one exact regular-file name, permission mode and content digest.
    /// Missing entries, directories and symbolic links never match.
    #[must_use]
    pub fn matches_regular_file(&self, relative: &str, mode: u32, content: &Sha256Digest) -> bool {
        self.matches_entry(relative, 1, mode, Some(content))
    }

    /// Match one exact symbolic-link name, permission mode and target-byte digest.
    /// This does not establish that the target resolves inside the tree.
    #[must_use]
    pub fn matches_symbolic_link(&self, relative: &str, mode: u32, target: &Sha256Digest) -> bool {
        self.matches_entry(relative, 3, mode, Some(target))
    }

    fn matches_entry(
        &self,
        relative: &str,
        kind: u8,
        mode: u32,
        content: Option<&Sha256Digest>,
    ) -> bool {
        self.entries.get(relative.as_bytes()).is_some_and(|entry| {
            entry.snapshot.kind == kind
                && entry.snapshot.mode & 0o7777 == mode
                && entry.content.as_ref() == content
        })
    }

    /// Whether this snapshot contains any symbolic links.
    #[must_use]
    pub fn has_symlinks(&self) -> bool {
        self.entries.values().any(|entry| entry.snapshot.kind == 3)
    }

    /// Compare portable tree content while permitting byte/size changes in one
    /// exact regular file. Names, node kinds and modes remain immutable, even
    /// for that file. Missing, link, directory or renamed exceptions fail.
    /// This is for explicitly mutable producer metadata, not input validation.
    #[must_use]
    pub fn matches_content_except_regular_file(&self, other: &Self, relative: &str) -> bool {
        let key = relative.as_bytes();
        let (Some(left), Some(right)) = (self.entries.get(key), other.entries.get(key)) else {
            return false;
        };
        if left.snapshot.kind != 1
            || right.snapshot.kind != 1
            || left.snapshot.mode != right.snapshot.mode
            || self.entries.len() != other.entries.len()
        {
            return false;
        }
        self.entries.iter().all(|(path, before)| {
            other.entries.get(path).is_some_and(|after| {
                before.snapshot.kind == after.snapshot.kind
                    && before.snapshot.mode == after.snapshot.mode
                    && before.symbolic_link_target == after.symbolic_link_target
                    && (path.as_slice() == key || before.content == after.content)
            })
        })
    }

    /// Number of filesystem objects below the measured root.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Total bytes in regular files represented by this snapshot.
    ///
    /// Returns `None` only if a platform supplied an invalid negative size or
    /// the exact sum does not fit in `u64`. Callers can treat that condition as
    /// a safety blocker rather than guessing a destructive operation's scope.
    #[must_use]
    pub fn regular_file_bytes(&self) -> Option<u64> {
        self.entries
            .values()
            .filter(|entry| entry.snapshot.kind == 1)
            .try_fold(0_u64, |total, entry| {
                let size = u64::try_from(entry.snapshot.size).ok()?;
                total.checked_add(size)
            })
    }

    /// Return a stable digest of names, node kinds, regular-file bytes, and
    /// link targets. Identity and timestamps are deliberately excluded so an
    /// independently staged equivalent tree has the same digest. A top-level
    /// namespace may be omitted for self-referential receipts.
    #[must_use]
    pub fn payload_digest_excluding(&self, top_level: Option<&str>) -> Sha256Digest {
        payload_digest(self.entries.iter().filter_map(|(path, entry)| {
            let excluded = top_level.is_some_and(|prefix| {
                path == prefix.as_bytes()
                    || path
                        .strip_prefix(prefix.as_bytes())
                        .is_some_and(|rest| rest.starts_with(b"/"))
            });
            if excluded {
                None
            } else {
                Some((path.as_slice(), entry))
            }
        }))
    }

    /// Return the payload digest for an exact directory in this measured tree.
    ///
    /// The selected directory itself is omitted and descendant names are
    /// rebased to that directory, matching a fresh measurement rooted there.
    /// An empty name selects this snapshot's root. Non-empty names must match a
    /// recorded real directory exactly; this performs no filesystem I/O and no
    /// path normalization. Files, symbolic links, missing names and alternate
    /// spellings return `None`.
    #[must_use]
    pub fn subtree_payload_digest(&self, directory: &str) -> Option<Sha256Digest> {
        if directory.is_empty() {
            return Some(self.payload_digest_excluding(None));
        }
        if self.entry_kind(directory) != Some(TreeContentEntryKind::Directory) {
            return None;
        }

        let mut prefix = directory.as_bytes().to_vec();
        prefix.push(b'/');
        let entries = self
            .entries
            .range(prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(prefix.as_slice()))
            .map(|(path, entry)| (&path[prefix.len()..], entry));
        Some(payload_digest(entries))
    }

    /// Return a digest binding this exact measured filesystem snapshot.
    ///
    /// Unlike [`Self::payload_digest_excluding`], this includes root and child
    /// device/inode identities and timestamps.  It is suitable for a
    /// short-lived compare-and-apply token, not for portable content identity.
    #[must_use]
    pub fn snapshot_digest(&self) -> Sha256Digest {
        let mut bytes = Vec::new();
        append_identity(&mut bytes, self.root);
        for (path, entry) in &self.entries {
            bytes.extend_from_slice(&(path.len() as u64).to_be_bytes());
            bytes.extend_from_slice(path);
            append_identity(&mut bytes, entry.snapshot.identity);
            bytes.push(entry.snapshot.kind);
            bytes.extend_from_slice(&entry.snapshot.mode.to_be_bytes());
            bytes.extend_from_slice(&entry.snapshot.size.to_be_bytes());
            bytes.extend_from_slice(&entry.snapshot.mtime.to_be_bytes());
            bytes.extend_from_slice(&entry.snapshot.mtime_nsec.to_be_bytes());
            bytes.extend_from_slice(&entry.snapshot.ctime.to_be_bytes());
            bytes.extend_from_slice(&entry.snapshot.ctime_nsec.to_be_bytes());
            if let Some(content) = &entry.content {
                bytes.extend_from_slice(content.to_string().as_bytes());
            }
        }
        sha256_bytes(&bytes)
    }
}

fn append_identity(bytes: &mut Vec<u8>, identity: FileIdentity) {
    bytes.extend_from_slice(&identity.device.to_be_bytes());
    bytes.extend_from_slice(&identity.inode.to_be_bytes());
}

fn payload_digest<'a>(
    entries: impl IntoIterator<Item = (&'a [u8], &'a TreeContentEntry)>,
) -> Sha256Digest {
    let mut bytes = Vec::new();
    for (path, entry) in entries {
        bytes.extend_from_slice(&(path.len() as u64).to_be_bytes());
        bytes.extend_from_slice(path);
        bytes.push(entry.snapshot.kind);
        bytes.extend_from_slice(&entry.snapshot.mode.to_be_bytes());
        if let Some(digest) = &entry.content {
            bytes.extend_from_slice(digest.to_string().as_bytes());
        }
    }
    sha256_bytes(&bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::publication::{measure_regular_file, measure_tree_content_cas};
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn directory_index_skips_subtrees_without_losing_adjacent_siblings() {
        use TreeContentEntryKind::{Directory, RegularFile};
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("a")).unwrap();
        std::fs::create_dir(root.join("a/deep")).unwrap();
        for index in 0..128 {
            std::fs::write(root.join(format!("a/deep/{index}")), "data").unwrap();
        }
        for name in ["a-", "a.", "a0", "b"] {
            std::fs::write(root.join(name), "sibling").unwrap();
        }
        let snapshot = measure_tree_content_cas(root).unwrap();
        assert_eq!(
            snapshot.directory_entry_kinds(""),
            Some(vec![
                (b"a".to_vec(), Directory),
                (b"a-".to_vec(), RegularFile),
                (b"a.".to_vec(), RegularFile),
                (b"a0".to_vec(), RegularFile),
                (b"b".to_vec(), RegularFile)
            ])
        );
        assert_eq!(
            snapshot.directory_entry_kinds("a"),
            Some(vec![(b"deep".to_vec(), Directory)])
        );
        assert_eq!(snapshot.directory_entry_kinds("a/deep").unwrap().len(), 128);
        assert!(snapshot.directory_entry_kinds("a-").is_none());
        assert!(snapshot.directory_entry_kinds("missing").is_none());
    }

    #[test]
    fn entry_kind_inventory_is_complete_and_sorted_by_raw_name_bytes() {
        use TreeContentEntryKind::{Directory, RegularFile, SymbolicLink};

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("dir/child"), b"child").unwrap();
        std::fs::write(root.join("file"), b"file").unwrap();
        symlink("file", root.join("link")).unwrap();
        let snapshot = measure_tree_content_cas(root).unwrap();
        let entries: Vec<_> = snapshot
            .entry_kinds()
            .map(|(path, kind)| (path.to_vec(), kind))
            .collect();

        assert_eq!(
            entries,
            vec![
                (b"dir".to_vec(), Directory),
                (b"dir/child".to_vec(), RegularFile),
                (b"file".to_vec(), RegularFile),
                (b"link".to_vec(), SymbolicLink),
            ]
        );
    }

    // APFS rejects non-UTF-8 names at creation. Exercise the physical raw-name
    // case on Linux rather than accepting a failed filesystem write as proof.
    #[cfg(target_os = "linux")]
    #[test]
    fn entry_kind_inventory_preserves_non_utf8_filesystem_names() {
        let temporary = tempfile::tempdir().unwrap();
        let name = vec![b'r', 0xff];
        std::fs::write(
            temporary
                .path()
                .join(std::ffi::OsString::from_vec(name.clone())),
            b"raw",
        )
        .unwrap();
        let snapshot = measure_tree_content_cas(temporary.path()).unwrap();
        assert_eq!(
            snapshot
                .entry_kinds()
                .map(|(path, kind)| (path.to_vec(), kind))
                .collect::<Vec<_>>(),
            vec![(name, TreeContentEntryKind::RegularFile)]
        );
    }

    #[test]
    fn read_only_metadata_uses_exact_keys_and_regular_file_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("file"), b"snapshot bytes").unwrap();
        std::fs::write(root.join("other"), b"other bytes").unwrap();
        symlink("file", root.join("link")).unwrap();

        let snapshot = measure_tree_content_cas(root).unwrap();
        let file_digest = sha256_bytes(b"snapshot bytes");
        let (file_identity, _) = measure_regular_file(&root.join("file")).unwrap().unwrap();
        let (other_identity, _) = measure_regular_file(&root.join("other")).unwrap().unwrap();

        assert_eq!(
            snapshot.entry_kind("file"),
            Some(TreeContentEntryKind::RegularFile)
        );
        assert_eq!(
            snapshot.entry_kind("dir"),
            Some(TreeContentEntryKind::Directory)
        );
        assert_eq!(
            snapshot.entry_kind("link"),
            Some(TreeContentEntryKind::SymbolicLink)
        );
        assert_eq!(snapshot.entry_kind("missing"), None);
        assert_eq!(snapshot.entry_kind("./file"), None);
        assert_eq!(snapshot.entry_kind("dir/../file"), None);
        assert_eq!(snapshot.entry_kind("FILE"), None);

        assert_eq!(snapshot.regular_file_digest("file"), Some(&file_digest));
        assert_eq!(snapshot.regular_file_digest("dir"), None);
        assert_eq!(snapshot.regular_file_digest("link"), None);
        assert_eq!(snapshot.regular_file_digest("missing"), None);
        assert_eq!(snapshot.regular_file_digest("./file"), None);
        assert_eq!(
            snapshot.symbolic_link_target("link"),
            Some(b"file".as_slice())
        );
        assert_eq!(snapshot.symbolic_link_target("file"), None);
        assert_eq!(snapshot.symbolic_link_target("dir"), None);
        assert_eq!(snapshot.symbolic_link_target("./link"), None);

        assert!(snapshot.matches_regular_file_identity("file", file_identity, &file_digest));
        assert!(!snapshot.matches_regular_file_identity("file", other_identity, &file_digest));
        assert!(!snapshot.matches_regular_file_identity(
            "file",
            file_identity,
            &sha256_bytes(b"different bytes")
        ));
        assert!(!snapshot.matches_regular_file_identity("dir", file_identity, &file_digest));
        assert!(!snapshot.matches_regular_file_identity("link", file_identity, &file_digest));
        assert!(!snapshot.matches_regular_file_identity("./file", file_identity, &file_digest));
    }

    #[test]
    fn subtree_and_complete_payload_keep_the_existing_wire_encoding() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("d");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("a"), b"x").unwrap();
        std::fs::set_permissions(directory.join("a"), std::fs::Permissions::from_mode(0o640))
            .unwrap();
        // Existing wire format: u64 BE name length, raw name, file kind 1,
        // u32 BE Unix mode 0100640, then the ASCII SHA-256 of the file bytes.
        // Fixed bytes keep a shared-serializer regression from passing merely
        // because both the subtree and fresh-tree paths changed together.
        let wire = [
            b"\0\0\0\0\0\0\0\x01a\x01\0\0\x81\xa0".as_slice(),
            b"2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        ]
        .concat();
        let expected = sha256_bytes(&wire);
        let parent = measure_tree_content_cas(temporary.path()).unwrap();
        let child = measure_tree_content_cas(&directory).unwrap();
        assert_eq!(parent.subtree_payload_digest("d"), Some(expected.clone()));
        assert_eq!(child.payload_digest_excluding(None), expected);
    }

    #[test]
    fn subtree_payload_digest_matches_fresh_measurement_and_excludes_prefix_siblings() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let configure = root.join("configure");
        std::fs::create_dir_all(configure.join("nested")).unwrap();
        std::fs::create_dir(configure.join("empty")).unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        std::fs::write(configure.join("Makefile"), b"all: nested/tool\n").unwrap();
        std::fs::write(configure.join("nested/tool"), b"tool bytes").unwrap();
        std::fs::set_permissions(
            configure.join("Makefile"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        std::fs::set_permissions(
            configure.join("nested"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        std::fs::set_permissions(
            configure.join("empty"),
            std::fs::Permissions::from_mode(0o711),
        )
        .unwrap();
        symlink("../outside", configure.join("outside-link")).unwrap();
        std::fs::write(root.join("configure-old"), b"sibling").unwrap();
        std::fs::write(root.join("configure2"), b"another sibling").unwrap();

        let parent = measure_tree_content_cas(root).unwrap();
        let child = measure_tree_content_cas(&configure).unwrap();

        assert_eq!(
            parent.subtree_payload_digest("configure"),
            Some(child.payload_digest_excluding(None))
        );
        assert_eq!(
            parent.entry_kind("configure"),
            Some(TreeContentEntryKind::Directory)
        );
        assert_eq!(
            parent.subtree_payload_digest(""),
            Some(parent.payload_digest_excluding(None))
        );
    }

    #[test]
    fn subtree_payload_digest_requires_an_exact_recorded_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("dir/file"), b"file").unwrap();
        std::fs::write(root.join("file"), b"file").unwrap();
        symlink("dir", root.join("link")).unwrap();
        let snapshot = measure_tree_content_cas(root).unwrap();

        for name in [
            "missing",
            "file",
            "dir/file",
            "link",
            "./dir",
            "dir/",
            "dir/../dir",
        ] {
            assert!(
                snapshot.subtree_payload_digest(name).is_none(),
                "{name:?} must not select a subtree"
            );
        }
        assert!(snapshot.subtree_payload_digest("dir").is_some());
    }

    #[test]
    fn subtree_payload_digest_reads_only_the_immutable_captured_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("configure")).unwrap();
        std::fs::write(root.join("configure/input"), b"original").unwrap();
        let parent = measure_tree_content_cas(root).unwrap();
        let selected = parent.subtree_payload_digest("configure").unwrap();
        let parent_digest = parent.payload_digest_excluding(None);
        let parent_snapshot_digest = parent.snapshot_digest();

        std::fs::write(root.join("configure/input"), b"changed").unwrap();
        let current = measure_tree_content_cas(&root.join("configure")).unwrap();

        assert_eq!(
            parent.subtree_payload_digest("configure"),
            Some(selected.clone())
        );
        assert_ne!(current.payload_digest_excluding(None), selected);
        assert_eq!(parent.payload_digest_excluding(None), parent_digest);
        assert_eq!(parent.snapshot_digest(), parent_snapshot_digest);
    }
}
