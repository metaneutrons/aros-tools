//! Opaque whole-tree compare-and-swap snapshots.

use super::{sha256_bytes, BTreeMap, FileIdentity, Sha256Digest};

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
    /// Number of filesystem objects below the measured root.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Return a stable digest of names, node kinds, regular-file bytes, and
    /// link targets. Identity and timestamps are deliberately excluded so an
    /// independently staged equivalent tree has the same digest. A top-level
    /// namespace may be omitted for self-referential receipts.
    #[must_use]
    pub fn payload_digest_excluding(&self, top_level: Option<&str>) -> Sha256Digest {
        let mut bytes = Vec::new();
        for (path, entry) in &self.entries {
            let excluded = top_level.is_some_and(|prefix| {
                path == prefix.as_bytes()
                    || path
                        .strip_prefix(prefix.as_bytes())
                        .is_some_and(|rest| rest.starts_with(b"/"))
            });
            if excluded {
                continue;
            }
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
}
