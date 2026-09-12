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
