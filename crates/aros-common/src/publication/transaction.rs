//! Journalled multi-file publication transaction.

#[cfg(unix)]
use super::unix;
#[cfg(not(unix))]
use super::unsupported_durability;
use super::{
    absolute_path, casefold_path_key, sha256_bytes, FileIdentity, PublicationReceipt,
    RecoveryOutcome, Sha256Digest,
};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub(super) enum DesiredState {
    Write { contents: Vec<u8>, mode: u16 },
    Remove,
}

#[derive(Clone, Debug)]
pub(super) struct PlannedChange {
    pub(super) path: PathBuf,
    pub(super) original: Option<FileIdentity>,
    pub(super) original_digest: Option<Sha256Digest>,
    pub(super) original_mode: Option<u16>,
    pub(super) desired: DesiredState,
}

/// A journalled, lock-protected set of file mutations.
///
/// Existing files are renamed to sibling backups, retaining inode metadata,
/// hard-link relationships, ACLs, xattrs, ownership, and timestamps. A durable
/// journal records the complete intent before the first rename. File identities
/// let recovery distinguish unapplied, partially applied, and applied changes
/// without rewriting an increasingly large journal after every operation. An
/// interrupted transaction is rolled back before a later transaction can
/// acquire the same journal lock. This is writer-atomic and crash-recoverable,
/// but it is not a live snapshot transaction for uncoordinated readers: while
/// commit is applying several file renames, readers can observe an intermediate
/// set. Build-graph consumers must run after the generating action or acquire
/// the same namespace lock.
#[derive(Debug)]
pub struct DurableFileSet {
    journal: PathBuf,
    scope: PathBuf,
    recovery: RecoveryOutcome,
    changes: BTreeMap<PathBuf, PlannedChange>,
    commit_marker: Option<PlannedChange>,
    collision_keys: BTreeMap<String, PathBuf>,
    #[cfg(unix)]
    lock: std::fs::File,
}

impl DurableFileSet {
    /// Open a transaction namespace and recover any interrupted predecessor.
    ///
    /// # Errors
    ///
    /// Returns an I/O, lock, journal-recovery, or unsupported-platform error.
    pub fn new(journal: impl Into<PathBuf>) -> std::io::Result<Self> {
        let journal = absolute_path(&journal.into())?;
        let scope = journal
            .parent()
            .ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "publication journal has no containment scope",
                )
            })?
            .to_path_buf();
        #[cfg(unix)]
        {
            let lock = unix::lock_for_journal(&journal)?;
            let recovery = unix::recover_if_needed(&journal)?;
            Ok(Self {
                journal,
                scope,
                recovery,
                changes: BTreeMap::new(),
                commit_marker: None,
                collision_keys: BTreeMap::new(),
                lock,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = journal;
            Err(unsupported_durability())
        }
    }

    /// Recovery performed while this transaction acquired its namespace.
    #[must_use]
    pub const fn recovery_outcome(&self) -> RecoveryOutcome {
        self.recovery
    }

    /// Stage a content update, returning false when the existing bytes match.
    ///
    /// # Errors
    ///
    /// Returns an I/O, target-type, collision, or conflicting-operation error.
    pub fn stage_write(&mut self, path: &Path, contents: &[u8]) -> std::io::Result<bool> {
        self.stage_write_inner(path, contents, 0o644, false)
    }

    /// Stage a content update with one exact portable output mode.
    ///
    /// Only `0o644` and `0o755` are accepted. Unlike [`Self::stage_write`],
    /// this stages a replacement when the bytes already match but the mode
    /// does not. The mode is covered by the same journal, compare-and-swap,
    /// readback, rollback, and crash-recovery contract as the contents.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` for any other mode, or an I/O, target-type,
    /// collision, or conflicting-operation error.
    pub fn stage_write_mode(
        &mut self,
        path: &Path,
        contents: &[u8],
        mode: u16,
    ) -> std::io::Result<bool> {
        validate_portable_mode(mode)?;
        self.stage_write_inner(path, contents, mode, true)
    }

    /// Stage creation of one absent file with an exact portable output mode.
    ///
    /// Unlike [`Self::stage_write_mode`], this is a strict no-clobber
    /// operation. An existing regular file is rejected while staging, and the
    /// recorded absent snapshot is checked again immediately before commit.
    /// A concurrent creator therefore causes the complete transaction to roll
    /// back instead of being replaced.
    ///
    /// # Errors
    ///
    /// Returns `AlreadyExists` when the target already exists, `InvalidInput`
    /// for a non-portable mode, or an I/O, target-type, collision, or
    /// conflicting-operation error.
    pub fn stage_create_mode(
        &mut self,
        path: &Path,
        contents: &[u8],
        mode: u16,
    ) -> std::io::Result<()> {
        validate_portable_mode(mode)?;
        let path = absolute_path(path)?;
        self.register_target(&path)?;
        #[cfg(unix)]
        {
            if unix::read_regular_with_mode(&path)?.is_some() {
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!("publication target already exists: '{}'", path.display()),
                ));
            }
            self.insert_change(
                path.clone(),
                PlannedChange {
                    path,
                    original: None,
                    original_digest: None,
                    original_mode: None,
                    desired: DesiredState::Write {
                        contents: contents.to_vec(),
                        mode,
                    },
                },
            )?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = contents;
            Err(unsupported_durability())
        }
    }

    fn stage_write_inner(
        &mut self,
        path: &Path,
        contents: &[u8],
        mode: u16,
        enforce_mode: bool,
    ) -> std::io::Result<bool> {
        let path = absolute_path(path)?;
        self.register_target(&path)?;
        #[cfg(unix)]
        {
            let current = unix::read_regular_with_mode(&path)?;
            if current.as_ref().is_some_and(|(_, bytes, current_mode)| {
                bytes == contents && (!enforce_mode || *current_mode == mode)
            }) {
                return Ok(false);
            }
            let (original, original_digest, original_mode) = current
                .map_or((None, None, None), |(identity, bytes, mode)| {
                    (Some(identity), Some(sha256_bytes(&bytes)), Some(mode))
                });
            self.insert_change(
                path.clone(),
                PlannedChange {
                    path,
                    original,
                    original_digest,
                    original_mode,
                    desired: DesiredState::Write {
                        contents: contents.to_vec(),
                        mode,
                    },
                },
            )?;
            Ok(true)
        }
        #[cfg(not(unix))]
        {
            let _ = (contents, mode, enforce_mode);
            Err(unsupported_durability())
        }
    }

    /// Stage removal of a regular file, returning false when it is absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O, target-type, collision, or conflicting-operation error.
    pub fn stage_remove(&mut self, path: &Path) -> std::io::Result<bool> {
        let path = absolute_path(path)?;
        self.register_target(&path)?;
        #[cfg(unix)]
        {
            let current = unix::read_regular_with_mode(&path)?;
            let Some((original, bytes, original_mode)) = current else {
                return Ok(false);
            };
            self.insert_change(
                path.clone(),
                PlannedChange {
                    path,
                    original: Some(original),
                    original_digest: Some(sha256_bytes(&bytes)),
                    original_mode: Some(original_mode),
                    desired: DesiredState::Remove,
                },
            )?;
            Ok(true)
        }
        #[cfg(not(unix))]
        {
            Err(unsupported_durability())
        }
    }

    /// Stage the file which marks the complete set as committed.
    ///
    /// The marker is always replaced, even when its bytes are unchanged, and is
    /// applied after every ordinary write and removal in the same durable
    /// journal. Exactly one marker is allowed per transaction. This lets a
    /// consumer which treats one generated file as the generation boundary
    /// retain that ordering without reimplementing locking, CAS, rollback, or
    /// crash recovery around [`DurableFileSet`].
    ///
    /// # Errors
    ///
    /// Returns an I/O, target-type, collision, duplicate-marker, or
    /// conflicting-operation error.
    pub fn stage_commit_marker(&mut self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = absolute_path(path)?;
        if self.commit_marker.is_some() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "a durable file-set transaction accepts exactly one commit marker",
            ));
        }
        if self.changes.contains_key(&path) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "commit marker '{}' already has an ordinary mutation",
                    path.display()
                ),
            ));
        }
        self.register_target(&path)?;
        #[cfg(unix)]
        {
            let current = unix::read_regular_with_mode(&path)?;
            let (original, original_digest, original_mode) = current
                .map_or((None, None, None), |(identity, bytes, mode)| {
                    (Some(identity), Some(sha256_bytes(&bytes)), Some(mode))
                });
            self.commit_marker = Some(PlannedChange {
                path,
                original,
                original_digest,
                original_mode,
                desired: DesiredState::Write {
                    contents: contents.to_vec(),
                    mode: 0o644,
                },
            });
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = contents;
            Err(unsupported_durability())
        }
    }

    fn insert_change(&mut self, path: PathBuf, change: PlannedChange) -> std::io::Result<()> {
        if self
            .commit_marker
            .as_ref()
            .is_some_and(|marker| marker.path == path)
        {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "ordinary mutation '{}' conflicts with the commit marker",
                    path.display()
                ),
            ));
        }
        if let Some(existing) = self.changes.get(&path) {
            let same = match (&existing.desired, &change.desired) {
                (DesiredState::Remove, DesiredState::Remove) => true,
                (
                    DesiredState::Write {
                        contents: left_contents,
                        mode: left_mode,
                    },
                    DesiredState::Write {
                        contents: right_contents,
                        mode: right_mode,
                    },
                ) => left_contents == right_contents && left_mode == right_mode,
                _ => false,
            };
            if same {
                return Ok(());
            }
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("conflicting mutations for '{}'", path.display()),
            ));
        }
        self.changes.insert(path, change);
        Ok(())
    }

    fn register_target(&mut self, path: &Path) -> std::io::Result<()> {
        let relative = path.strip_prefix(&self.scope).map_err(|_| {
            std::io::Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "publication target '{}' escapes transaction scope '{}'",
                    path.display(),
                    self.scope.display()
                ),
            )
        })?;
        // The user-selected scope may itself contain host-specific names. Every
        // generated component below it must nevertheless be portable, and the
        // same relative key supplies the global casefold collision barrier.
        let key = casefold_path_key(relative)?;
        if let Some(existing) = self.collision_keys.get(&key) {
            if existing != path {
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "portable output collision: '{}' and '{}'",
                        existing.display(),
                        path.display()
                    ),
                ));
            }
        } else {
            self.collision_keys.insert(key, path.to_path_buf());
        }
        Ok(())
    }

    /// Commit all staged mutations or roll them all back.
    ///
    /// # Errors
    ///
    /// Returns a publication error. [`super::is_rollback_incomplete`]
    /// distinguishes a primary failure from an incomplete best-effort
    /// rollback/recovery.
    pub fn commit(self) -> std::io::Result<PublicationReceipt> {
        if self.changes.is_empty() && self.commit_marker.is_none() {
            return Ok(PublicationReceipt::new(self.recovery));
        }
        #[cfg(unix)]
        {
            let _lock = self.lock;
            let changes = ordered_changes(self.changes, self.commit_marker);
            unix::commit(&self.journal, changes)?;
            Ok(PublicationReceipt::new(self.recovery))
        }
        #[cfg(not(unix))]
        {
            Err(unsupported_durability())
        }
    }
}

fn validate_portable_mode(mode: u16) -> std::io::Result<()> {
    if matches!(mode, 0o644 | 0o755) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("publication mode {mode:#o} is not portable; expected 0o644 or 0o755"),
        ))
    }
}

fn ordered_changes(
    changes: BTreeMap<PathBuf, PlannedChange>,
    commit_marker: Option<PlannedChange>,
) -> Vec<PlannedChange> {
    let mut ordered: Vec<_> = changes.into_values().collect();
    ordered.extend(commit_marker);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_marker_is_always_last() {
        let ordinary = PlannedChange {
            path: PathBuf::from("z-sidecar"),
            original: None,
            original_digest: None,
            original_mode: None,
            desired: DesiredState::Remove,
        };
        let marker = PlannedChange {
            path: PathBuf::from("a-graph"),
            original: None,
            original_digest: None,
            original_mode: None,
            desired: DesiredState::Write {
                contents: b"graph".to_vec(),
                mode: 0o644,
            },
        };
        let ordered = ordered_changes(
            BTreeMap::from([(ordinary.path.clone(), ordinary)]),
            Some(marker),
        );

        assert_eq!(ordered[0].path, Path::new("z-sidecar"));
        assert_eq!(ordered[1].path, Path::new("a-graph"));
    }
}
