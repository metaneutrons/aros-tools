//! Race-resistant, durable publication primitives shared by AROS tools.
//!
//! Mutating operations deliberately fail closed outside Unix. Rust's standard
//! Windows rename and directory APIs cannot currently express the combination
//! of no-follow traversal, compare-and-swap publication, and write-through
//! directory durability promised by this module. Read-only name and source
//! containment validation remains portable.

use crate::digest::{sha256_bytes, sha256_reader, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Write as _};
use std::path::{Component, Path, PathBuf};

/// A single filesystem component that is safe on every supported host.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PortableOutputName(String);

impl PortableOutputName {
    /// Validate one generated output component.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` for traversal, reserved, non-portable, empty, or
    /// overlong components.
    pub fn new(value: &str) -> std::io::Result<Self> {
        let invalid_character = value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        });
        if value.is_empty()
            || !value.is_ascii()
            || value == "."
            || value == ".."
            || value.ends_with(['.', ' '])
            || invalid_character
            || value.len() > 255
        {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{value}' is not a portable output name"),
            ));
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or(value)
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"))
                .is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                });
        if reserved {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{value}' is a reserved Windows device name"),
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// The validated component.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for PortableOutputName {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

/// Return a host-independent collision key for a relative generated path.
///
/// # Errors
///
/// Returns `InvalidInput` when a component is not portable or the path is not
/// a non-empty relative path.
pub fn casefold_path_key(path: &Path) -> std::io::Result<String> {
    let mut folded = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{}' is not a relative output path", path.display()),
            ));
        };
        let value = value.to_str().ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("'{}' is not valid UTF-8", path.display()),
            )
        })?;
        let portable = PortableOutputName::new(value)?;
        folded.push(portable.as_str().to_lowercase());
    }
    if folded.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "an output path must contain at least one component",
        ));
    }
    Ok(folded.join("/"))
}

/// Canonicalize an existing source file and prove it remains below `root`.
///
/// # Errors
///
/// Returns an I/O, containment, or file-type error when the canonical source
/// root and candidate do not identify a regular file below the same root.
pub fn canonical_source_file(root: &Path, candidate: &Path) -> std::io::Result<PathBuf> {
    let root = root.canonicalize().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "cannot canonicalize source root '{}': {error}",
                root.display()
            ),
        )
    })?;
    if !root.is_dir() {
        return Err(std::io::Error::new(
            ErrorKind::NotADirectory,
            format!("source root '{}' is not a directory", root.display()),
        ));
    }
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let resolved = joined.canonicalize().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("cannot canonicalize source '{}': {error}", joined.display()),
        )
    })?;
    if !resolved.starts_with(&root) {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "source '{}' escapes canonical scan root '{}'",
                resolved.display(),
                root.display()
            ),
        ));
    }
    if !resolved.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("source '{}' is not a regular file", resolved.display()),
        ));
    }
    Ok(resolved)
}

/// Stable identity used by explicit compare-and-swap replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileIdentity {
    device: u64,
    inode: u64,
}

mod tree_cas;
pub use tree_cas::TreeContentCas;
use tree_cas::{TreeContentEntry, TreeNodeSnapshot};

/// Existing-target policy for one-file publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomicFilePolicy {
    /// Publication fails if the target exists.
    NoClobber,
    /// Replace exactly the file represented by this previously measured ID and
    /// digest. Both are rechecked under the publication lock, closing the race
    /// where an in-place writer preserves the inode but changes its bytes.
    ReplaceIf {
        /// Device/inode identity measured before publication.
        identity: FileIdentity,
        /// Exact content digest measured before publication.
        sha256: Sha256Digest,
    },
}

/// Recovery work completed while opening a publication namespace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecoveryOutcome {
    /// No predecessor journal or owned staging area was present.
    #[default]
    None,
    /// A prepared predecessor was rolled back to its original file set.
    RolledBack,
    /// A committed predecessor was complete and its auxiliary files were removed.
    CompletedCleanup,
    /// An owned, interrupted tree staging area was removed before publication.
    RemovedTreeStage,
}

impl RecoveryOutcome {
    /// Whether opening the namespace changed recovery state on disk.
    #[must_use]
    pub const fn recovered(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Stable log value for machine-readable observability records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RolledBack => "rolled_back",
            Self::CompletedCleanup => "completed_cleanup",
            Self::RemovedTreeStage => "removed_tree_stage",
        }
    }
}

/// Successful publication result, including any predecessor recovery.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicationReceipt {
    recovery: RecoveryOutcome,
}

impl PublicationReceipt {
    const fn new(recovery: RecoveryOutcome) -> Self {
        Self { recovery }
    }

    /// Recovery performed before the requested publication began.
    #[must_use]
    pub const fn recovery(self) -> RecoveryOutcome {
        self.recovery
    }
}

/// Stable remediation class for a publication error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationFailureClass {
    /// The destination already exists or an explicit CAS precondition failed.
    Conflict,
    /// A path, name, symlink, file type, or containment boundary is unsafe.
    UnsafeTarget,
    /// Required durability primitives are unavailable on this platform or filesystem.
    Unsupported,
    /// Rollback or recovery could not restore or clean every owned object.
    RecoveryIncomplete,
    /// The rename completed but durable directory commit could not be proven.
    CommitStateUncertain,
    /// Another I/O failure occurred.
    Io,
}

/// Classify a publication failure without parsing its display text.
#[must_use]
pub fn publication_failure_class(error: &std::io::Error) -> PublicationFailureClass {
    if let Some(source) = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<PublicationError>())
    {
        return source.class;
    }
    match error.kind() {
        ErrorKind::AlreadyExists => PublicationFailureClass::Conflict,
        ErrorKind::InvalidInput
        | ErrorKind::InvalidFilename
        | ErrorKind::NotADirectory
        | ErrorKind::IsADirectory
        | ErrorKind::PermissionDenied => PublicationFailureClass::UnsafeTarget,
        ErrorKind::Unsupported => PublicationFailureClass::Unsupported,
        _ => PublicationFailureClass::Io,
    }
}

/// Read a regular file through a no-follow descriptor and return its identity
/// and exact bytes as one stable snapshot.
///
/// # Errors
///
/// Returns an I/O or file-type error when safe descriptor traversal or a stable
/// snapshot cannot be established; mutating-grade measurement is unsupported
/// on non-Unix hosts.
pub fn measure_regular_file(path: &Path) -> std::io::Result<Option<(FileIdentity, Vec<u8>)>> {
    #[cfg(unix)]
    {
        unix::read_regular(&absolute_path(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(unsupported_durability())
    }
}

/// Durably publish one file using no-clobber or explicit identity-CAS policy.
///
/// # Errors
///
/// Returns an I/O, collision, CAS, recovery, or unsupported-platform error.
pub fn publish_atomic_file(
    target: &Path,
    contents: &[u8],
    policy: AtomicFilePolicy,
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(target)?;
    #[cfg(unix)]
    {
        let target = absolute_path(target)?;
        match policy {
            AtomicFilePolicy::NoClobber => unix::publish_file_noclobber(&target, contents),
            AtomicFilePolicy::ReplaceIf { identity, sha256 } => {
                let journal = transaction_journal_path(&target, "file")?;
                let mut transaction = DurableFileSet::new(journal)?;
                let measured = unix::read_regular(&target)?;
                let matches = measured.as_ref().is_some_and(|(current, contents)| {
                    *current == identity && sha256_bytes(contents) == sha256
                });
                if !matches {
                    return Err(io_failure(PublicationError::conflict(format!(
                        "compare-and-swap precondition failed for '{}'",
                        target.display()
                    ))));
                }
                transaction.stage_write(&target, contents)?;
                // Re-check after staging. DurableFileSet performs a final
                // descriptor identity and digest check immediately before its
                // rename.
                let measured = unix::read_regular(&target)?;
                let matches = measured.as_ref().is_some_and(|(current, contents)| {
                    *current == identity && sha256_bytes(contents) == sha256
                });
                if !matches {
                    return Err(io_failure(PublicationError::conflict(format!(
                        "compare-and-swap target changed while staging: '{}'",
                        target.display()
                    ))));
                }
                transaction.commit()
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (target, contents, policy);
        Err(unsupported_durability())
    }
}

/// Publish an entire new flat directory with one atomic no-clobber rename.
///
/// The destination must not exist. Every member is durable before the single
/// directory rename makes the tree visible, so a crash can expose either no
/// destination or the complete tree, never a partially extracted package.
///
/// # Errors
///
/// Returns an I/O, member-name collision, destination collision, cleanup, or
/// unsupported-platform error.
pub fn publish_flat_tree_noclobber(
    destination: &Path,
    members: &[(PortableOutputName, &[u8])],
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(destination)?;
    let mut names = BTreeSet::new();
    for (name, _) in members {
        let folded = name.as_str().to_lowercase();
        if !names.insert(folded) {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!("portable member-name collision for '{}'", name.as_str()),
            ));
        }
    }
    #[cfg(unix)]
    {
        unix::publish_flat_tree_noclobber(&absolute_path(destination)?, members)
    }
    #[cfg(not(unix))]
    {
        let _ = destination;
        Err(unsupported_durability())
    }
}

/// Durably publish a caller-prepared directory beside `destination`.
///
/// Every regular file and directory is synced recursively without following
/// symlinks. The staging directory and destination must have the same parent,
/// and publication is one process-serialised `RENAME_NOREPLACE` followed by a
/// parent-directory sync. Before the rename an error leaves `staging` owned by
/// the caller; after the rename an inability to prove the parent sync is
/// reported as [`PublicationFailureClass::CommitStateUncertain`] and the
/// complete destination is deliberately left in place.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, destination-conflict, durability, or
/// unsupported-platform error.
pub fn publish_prepared_tree_noclobber(
    staging: &Path,
    destination: &Path,
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(staging)?;
    validate_target_leaf(destination)?;
    #[cfg(unix)]
    {
        unix::publish_prepared_tree_noclobber(
            &absolute_path(staging)?,
            &absolute_path(destination)?,
            unix::PreparedTreeNamePolicy::PortableGeneratedOutput,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (staging, destination);
        Err(unsupported_durability())
    }
}

/// Durably publish an already-materialized source checkout beside `destination`.
///
/// This has the same no-follow, no-clobber, identity, and durability contract as
/// [`publish_prepared_tree_noclobber`], but preserves source-controlled Unicode
/// and other non-ASCII entry names instead of treating them as newly generated
/// cross-platform output names. Unsafe path components and case-folded sibling
/// collisions remain rejected before the atomic rename.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, destination-conflict, durability, or
/// unsupported-platform error.
pub fn publish_prepared_source_tree_noclobber(
    staging: &Path,
    destination: &Path,
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(staging)?;
    validate_target_leaf(destination)?;
    #[cfg(unix)]
    {
        unix::publish_prepared_tree_noclobber(
            &absolute_path(staging)?,
            &absolute_path(destination)?,
            unix::PreparedTreeNamePolicy::PreservedSource,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (staging, destination);
        Err(unsupported_durability())
    }
}

/// Atomically exchange a caller-prepared directory with an existing directory.
///
/// Both directories must be real sibling directories. The prepared tree is
/// recursively synced first, publication is one filesystem exchange under the
/// shared publication namespace lock, and the parent is synced before success
/// is reported. On success the previous destination remains at `staging` so
/// the caller can inspect or remove it. A post-exchange durability failure is
/// classified as [`PublicationFailureClass::CommitStateUncertain`].
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, identity-race, durability, unsupported-host,
/// or unsupported-filesystem error.
pub fn exchange_prepared_tree(
    staging: &Path,
    destination: &Path,
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(staging)?;
    validate_target_leaf(destination)?;
    #[cfg(unix)]
    {
        unix::exchange_prepared_tree(&absolute_path(staging)?, &absolute_path(destination)?)
    }
    #[cfg(not(unix))]
    {
        let _ = (staging, destination);
        Err(unsupported_durability())
    }
}

/// Measure every object and every regular-file/link payload in a real tree
/// through no-follow directory descriptors.
///
/// # Errors
///
/// Returns an I/O or unsafe-tree error if any component changes while it is
/// measured or if the tree contains unsupported filesystem objects.
pub fn measure_tree_content_cas(path: &Path) -> std::io::Result<TreeContentCas> {
    validate_target_leaf(path)?;
    #[cfg(unix)]
    {
        unix::measure_tree_content_cas(&absolute_path(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(unsupported_durability())
    }
}

/// Exchange a prepared tree only if the complete destination still matches
/// the supplied snapshot immediately before the atomic exchange.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, content/identity conflict, durability,
/// unsupported-host, or unsupported-filesystem error.
pub fn exchange_prepared_tree_if_unchanged(
    staging: &Path,
    destination: &Path,
    expected_destination: &TreeContentCas,
) -> std::io::Result<PublicationReceipt> {
    validate_target_leaf(staging)?;
    validate_target_leaf(destination)?;
    #[cfg(unix)]
    {
        unix::exchange_prepared_tree_if_unchanged(
            &absolute_path(staging)?,
            &absolute_path(destination)?,
            expected_destination,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (staging, destination, expected_destination);
        Err(unsupported_durability())
    }
}

mod error;
use error::io_failure;
pub use error::{is_rollback_incomplete, PublicationError};

mod transaction;
pub use transaction::DurableFileSet;
use transaction::{DesiredState, PlannedChange};

fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    #[cfg(target_os = "macos")]
    {
        // macOS exposes these system roots as immutable compatibility symlinks
        // (`/tmp` -> `/private/tmp`, for example). Normalize that OS-owned
        // first component once so target, journal, stage, and backup paths use
        // the same spelling; user-controlled descendants remain uncanonicalized
        // and are traversed descriptor-by-descriptor with O_NOFOLLOW.
        for system_root in ["/var", "/tmp", "/etc"] {
            let root = Path::new(system_root);
            if let Ok(relative) = absolute.strip_prefix(root) {
                return root
                    .canonicalize()
                    .map(|canonical| canonical.join(relative));
            }
        }
    }
    Ok(absolute)
}

fn validate_target_leaf(path: &Path) -> std::io::Result<()> {
    let leaf = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "publication target '{}' has no UTF-8 file name",
                path.display()
            ),
        )
    })?;
    PortableOutputName::new(leaf).map(|_| ())
}

fn transaction_journal_path(target: &Path, purpose: &str) -> std::io::Result<PathBuf> {
    publication_journal_path(target, purpose)
}

/// Derive a bounded, portable sibling journal name for one transaction root.
///
/// # Errors
///
/// Returns `InvalidInput` if the target lacks a parent or cannot be represented
/// losslessly in the portable journal namespace.
pub fn publication_journal_path(target: &Path, purpose: &str) -> std::io::Result<PathBuf> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidInput, "publication target has no parent")
    })?;
    let leaf = target.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "publication target has no UTF-8 leaf for its durable journal",
        )
    })?;
    let leaf = PortableOutputName::new(leaf)?;
    PortableOutputName::new(purpose)?;
    // The parent directory is the namespace. Case-folding the portable leaf
    // ensures case aliases on APFS and other case-insensitive filesystems use
    // the same journal and advisory lock.
    let digest = sha256_bytes(leaf.as_str().to_ascii_lowercase().as_bytes()).to_string();
    Ok(parent.join(format!(
        ".aros-{purpose}-{}-transaction.json",
        &digest[..32]
    )))
}

#[cfg(not(unix))]
fn unsupported_durability() -> std::io::Error {
    std::io::Error::new(
        ErrorKind::Unsupported,
        "durable publication is unavailable on this platform: no no-follow, CAS, and write-through directory contract",
    )
}

#[cfg(unix)]
mod unix {
    use super::*;
    use rustix::fd::OwnedFd;
    use rustix::fs::{self as rfs, AtFlags, FlockOperation, Mode, OFlags, RenameFlags};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    const JOURNAL_STAGE_MAGIC: &[u8] = b"AROS-PUBLICATION-JOURNAL-v2\n";
    const TREE_STAGE_MAGIC: &[u8] = b"AROS-FLAT-TREE-STAGE-v1\n";

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum PreparedTreeNamePolicy {
        PortableGeneratedOutput,
        PreservedSource,
    }

    #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
    struct Journal {
        schema: String,
        nonce: u64,
        state: JournalState,
        operations: Vec<JournalOperation>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum JournalState {
        Prepared,
        Committed,
    }

    #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
    struct JournalOperation {
        target: PathBuf,
        stage: Option<PathBuf>,
        backup: Option<PathBuf>,
        parent_identity: FileIdentity,
        original: Option<FileIdentity>,
        original_digest: Option<Sha256Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_mode: Option<u16>,
        desired_digest: Option<Sha256Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        desired_mode: Option<u16>,
        installed: Option<FileIdentity>,
    }

    #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
    struct TreeStageMarker {
        schema: String,
        destination: String,
        parent_identity: FileIdentity,
        stage_identity: FileIdentity,
        members: BTreeMap<String, Sha256Digest>,
    }

    struct ParentHandle {
        fd: OwnedFd,
        leaf: OsString,
        path: PathBuf,
    }

    pub(super) fn lock_for_journal(journal: &Path) -> std::io::Result<std::fs::File> {
        let parent = open_parent(journal, true)?;
        let lock_name = sibling_name(&parent.leaf, "lock", 0);
        let fd = rfs::openat(
            &parent.fd,
            Path::new(&lock_name),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        rfs::flock(&fd, FlockOperation::LockExclusive)?;
        Ok(std::fs::File::from(fd))
    }

    pub(super) fn read_regular(path: &Path) -> std::io::Result<Option<(FileIdentity, Vec<u8>)>> {
        read_regular_with_mode(path)
            .map(|snapshot| snapshot.map(|(identity, bytes, _mode)| (identity, bytes)))
    }

    pub(super) fn read_regular_with_mode(
        path: &Path,
    ) -> std::io::Result<Option<(FileIdentity, Vec<u8>, u16)>> {
        let parent = match open_parent(path, false) {
            Ok(parent) => parent,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let fd = match rfs::openat(
            &parent.fd,
            Path::new(&parent.leaf),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stat = rfs::fstat(&fd)?;
        if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "publication target '{}' is not a regular file",
                    path.display()
                ),
            ));
        }
        let identity = identity_from_stat(&stat);
        let mode = permission_mode_from_stat(&stat);
        let mut file = std::fs::File::from(fd);
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)?;
        test_pause_point("read-before-final-stat");
        let final_stat = rfs::fstat(&file)?;
        if !same_regular_snapshot(&stat, &final_stat, bytes.len()) {
            return Err(std::io::Error::other(format!(
                "publication target changed or was written concurrently while reading: '{}'",
                path.display()
            )));
        }
        Ok(Some((identity, bytes, mode)))
    }

    #[allow(clippy::cast_sign_loss)]
    fn same_regular_snapshot(before: &rfs::Stat, after: &rfs::Stat, bytes: usize) -> bool {
        identity_from_stat(before) == identity_from_stat(after)
            && before.st_size >= 0
            && usize::try_from(before.st_size).ok() == Some(bytes)
            && before.st_size == after.st_size
            && before.st_mtime == after.st_mtime
            && before.st_mtime_nsec == after.st_mtime_nsec
            && before.st_ctime == after.st_ctime
            && before.st_ctime_nsec == after.st_ctime_nsec
            && before.st_mode == after.st_mode
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    const fn permission_mode_from_stat(stat: &rfs::Stat) -> u16 {
        (stat.st_mode as u32 & 0o7777) as u16
    }

    pub(super) fn recover_if_needed(journal_path: &Path) -> std::io::Result<RecoveryOutcome> {
        cleanup_journal_stage(journal_path)?;
        let Some((journal_identity, bytes)) = read_regular(journal_path)? else {
            return Ok(RecoveryOutcome::None);
        };
        let journal = parse_journal(&bytes).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "cannot parse recovery journal '{}': {error}; journal retained",
                journal_path.display()
            )))
        })?;
        validate_journal(&journal, journal_path)?;
        let (outcome, errors) = if journal.state == JournalState::Committed {
            (
                RecoveryOutcome::CompletedCleanup,
                cleanup_committed(&journal.operations),
            )
        } else {
            (
                RecoveryOutcome::RolledBack,
                rollback_operations(&journal.operations),
            )
        };
        if !errors.is_empty() {
            return Err(io_failure(PublicationError::rollback(format!(
                "recovery from '{}' is incomplete: {}; journal retained",
                journal_path.display(),
                errors.join("; ")
            ))));
        }
        remove_journal(journal_path, journal_identity).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "recovery actions for '{}' completed, but its journal could not be removed: {error}; journal retained",
                journal_path.display()
            )))
        })?;
        Ok(outcome)
    }

    pub(super) fn commit(journal_path: &Path, changes: Vec<PlannedChange>) -> std::io::Result<()> {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nonce = (u64::from(std::process::id()) << 32) ^ sequence;
        let mut operations = Vec::with_capacity(changes.len());
        let mut staged_contents = Vec::with_capacity(changes.len());

        for (index, change) in changes.into_iter().enumerate() {
            let operation = (|| {
                let parent = open_parent(&change.path, true)?;
                let backup = change.original.map(|_| {
                    parent
                        .path
                        .join(sibling_name(&parent.leaf, "backup", nonce + index as u64))
                });
                let (stage, desired_digest, desired_mode, contents) = match change.desired {
                    DesiredState::Write { contents, mode } => {
                        let stage = parent.path.join(sibling_name(
                            &parent.leaf,
                            "stage",
                            nonce + index as u64,
                        ));
                        let digest = sha256_bytes(&contents);
                        (Some(stage), Some(digest), Some(mode), Some(contents))
                    }
                    DesiredState::Remove => (None, None, None, None),
                };
                Ok::<(JournalOperation, Option<Vec<u8>>), std::io::Error>((
                    JournalOperation {
                        target: change.path,
                        stage,
                        backup,
                        parent_identity: identity_from_stat(&rfs::fstat(&parent.fd)?),
                        original: change.original,
                        original_digest: change.original_digest,
                        original_mode: change.original_mode,
                        desired_digest,
                        desired_mode,
                        installed: None,
                    },
                    contents,
                ))
            })();
            match operation {
                Ok((operation, contents)) => {
                    staged_contents.push(contents);
                    operations.push(operation);
                }
                Err(error) => return Err(error),
            }
        }

        let mut journal = Journal {
            schema: "aros-publication-journal-v3".to_owned(),
            nonce,
            state: JournalState::Prepared,
            operations,
        };
        write_journal(journal_path, &journal, nonce)?;

        for (index, contents) in staged_contents.iter().enumerate() {
            let Some(contents) = contents else {
                continue;
            };
            let stage = journal.operations[index]
                .stage
                .as_ref()
                .expect("write operations have a stage");
            let mode = journal.operations[index]
                .desired_mode
                .expect("write operations have a desired mode");
            match write_new_file_mode(stage, contents, mode) {
                Ok(identity) => {
                    journal.operations[index].installed = Some(identity);
                    test_crash_point("after-stage-before-journal-update");
                }
                Err(error) => {
                    let rollback = rollback_operations(&journal.operations);
                    return finish_failed_transaction(journal_path, error, &rollback);
                }
            }
        }
        if let Err(error) = write_journal(journal_path, &journal, nonce) {
            let rollback = rollback_operations(&journal.operations);
            return finish_failed_transaction(journal_path, error, &rollback);
        }

        test_pause_point("before-apply");
        for (index, operation) in journal.operations.iter().enumerate() {
            if let Err(error) = apply_operation(operation) {
                let rollback = rollback_operations(&journal.operations);
                return finish_failed_transaction(journal_path, error, &rollback);
            }
            if let Err(error) = test_fail_point(&format!("after-apply-{index}")) {
                let rollback = rollback_operations(&journal.operations);
                return finish_failed_transaction(journal_path, error, &rollback);
            }
        }

        journal.state = JournalState::Committed;
        if let Err(error) = write_journal(journal_path, &journal, nonce) {
            let current = read_regular(journal_path)
                .ok()
                .flatten()
                .and_then(|(_, bytes)| parse_journal(&bytes).ok());
            if current.as_ref() != Some(&journal) {
                let rollback = rollback_operations(&journal.operations);
                return finish_failed_transaction(journal_path, error, &rollback);
            }
            return Err(io_failure(PublicationError::uncertain(format!(
                "all targets were installed and the committed journal is visible, but its durable directory sync failed: {error}; outputs and journal retained for deterministic recovery"
            ))));
        }
        let cleanup = cleanup_committed(&journal.operations);
        if !cleanup.is_empty() {
            return Err(io_failure(PublicationError::rollback(format!(
                "publication committed but cleanup is incomplete: {}; committed journal retained",
                cleanup.join("; ")
            ))));
        }
        let journal_identity = read_regular(journal_path)?
            .map(|(identity, _)| identity)
            .ok_or_else(|| {
                io_failure(PublicationError::rollback(
                    "committed publication journal disappeared before cleanup",
                ))
            })?;
        remove_journal(journal_path, journal_identity).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "publication committed and auxiliaries were cleaned, but the committed journal could not be removed: {error}; journal retained"
            )))
        })?;
        Ok(())
    }

    pub(super) fn publish_file_noclobber(
        target: &Path,
        contents: &[u8],
    ) -> std::io::Result<PublicationReceipt> {
        let journal = transaction_journal_path(target, "file")?;
        let lock = lock_for_journal(&journal)?;
        let recovery = recover_if_needed(&journal)?;
        let parent = open_parent(target, true)?;
        if identity_at(&parent, &parent.leaf)?.is_some() {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "refusing to overwrite existing target '{}'",
                    target.display()
                ),
            ));
        }
        let nonce = (u64::from(std::process::id()) << 32)
            ^ TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stage_path = parent.path.join(sibling_name(&parent.leaf, "stage", nonce));
        let expected_digest = sha256_bytes(contents);
        let mut marker = Journal {
            schema: "aros-publication-journal-v3".to_owned(),
            nonce,
            state: JournalState::Prepared,
            operations: vec![JournalOperation {
                target: target.to_path_buf(),
                stage: Some(stage_path.clone()),
                backup: None,
                parent_identity: identity_from_stat(&rfs::fstat(&parent.fd)?),
                original: None,
                original_digest: None,
                original_mode: None,
                desired_digest: Some(expected_digest.clone()),
                desired_mode: Some(0o644),
                installed: None,
            }],
        };
        write_journal(&journal, &marker, nonce)?;
        let installed = match write_new_file(&stage_path, contents) {
            Ok(identity) => identity,
            Err(error) => {
                let rollback = rollback_operations(&marker.operations);
                return finish_failed_transaction(&journal, error, &rollback)
                    .map(|()| PublicationReceipt::new(recovery));
            }
        };
        marker.operations[0].installed = Some(installed);
        test_crash_point("after-stage-before-journal-update");
        if let Err(error) = write_journal(&journal, &marker, nonce) {
            let rollback = rollback_operations(&marker.operations);
            return finish_failed_transaction(&journal, error, &rollback)
                .map(|()| PublicationReceipt::new(recovery));
        }
        // For a no-clobber publication there is no original target to
        // restore. Mark cleanup-only recovery durably before the rename so a
        // crash after rename can never make a later run delete the complete
        // destination as though it were an uncommitted multi-file update.
        marker.state = JournalState::Committed;
        if let Err(error) = write_journal(&journal, &marker, nonce) {
            return finish_cleanup_only_publication(&journal, error)
                .map(|()| PublicationReceipt::new(recovery));
        }
        test_crash_point("file-after-committed-before-rename");
        let stage = open_parent(&stage_path, false)?;
        let result = rename_noclobber(&stage, &stage.leaf, &parent, &parent.leaf).and_then(|()| {
            let published = read_regular_with_mode(target)?;
            if !published.as_ref().is_some_and(|(identity, bytes, mode)| {
                *identity == installed && sha256_bytes(bytes) == expected_digest && *mode == 0o644
            }) {
                return Err(std::io::Error::other(
                    "published file identity, digest, or mode changed during no-clobber rename",
                ));
            }
            test_fail_point("file-after-rename-before-sync")?;
            rfs::fsync(&parent.fd)?;
            Ok(())
        });
        if let Err(error) = result {
            if identity_at(&parent, &parent.leaf).ok().flatten() == Some(installed) {
                drop(lock);
                return Err(io_failure(PublicationError::uncertain(format!(
                    "file rename completed but durable parent sync could not be proven: {error}; complete target retained"
                ))));
            }
            let cleanup = finish_cleanup_only_publication(&journal, error);
            drop(lock);
            return cleanup.map(|()| PublicationReceipt::new(recovery));
        }
        let cleanup = cleanup_committed(&marker.operations);
        if !cleanup.is_empty() {
            drop(lock);
            return Err(io_failure(PublicationError::rollback(format!(
                "file publication committed but cleanup is incomplete: {}; committed journal retained",
                cleanup.join("; ")
            ))));
        }
        let journal_identity = read_regular(&journal)?
            .map(|(identity, _)| identity)
            .ok_or_else(|| {
                io_failure(PublicationError::rollback(
                    "committed no-clobber journal disappeared before cleanup",
                ))
            })?;
        remove_journal(&journal, journal_identity).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "file publication committed, but its journal could not be removed: {error}; complete target retained"
            )))
        })?;
        drop(lock);
        Ok(PublicationReceipt::new(recovery))
    }

    pub(super) fn publish_flat_tree_noclobber(
        destination: &Path,
        members: &[(PortableOutputName, &[u8])],
    ) -> std::io::Result<PublicationReceipt> {
        let journal = transaction_journal_path(destination, "tree")?;
        let lock = lock_for_journal(&journal)?;
        let parent = open_parent(destination, true)?;
        let stage_name = tree_stage_name(&parent.leaf);
        let recovery = recover_flat_tree_stage(&parent, &stage_name, &parent.leaf)?;
        match rfs::statat(&parent.fd, &parent.leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "refusing to replace existing extraction destination '{}'",
                        destination.display()
                    ),
                ));
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }

        rfs::mkdirat(
            &parent.fd,
            Path::new(&stage_name),
            Mode::from_raw_mode(0o755),
        )?;
        rfs::fsync(&parent.fd)?;
        let stage_root_fd = rfs::openat(
            &parent.fd,
            Path::new(&stage_name),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let stage_identity = identity_from_stat(&rfs::fstat(&stage_root_fd)?);
        let marker = TreeStageMarker {
            schema: "aros-flat-tree-stage-v1".to_owned(),
            destination: parent.leaf.to_string_lossy().to_ascii_lowercase(),
            parent_identity: identity_from_stat(&rfs::fstat(&parent.fd)?),
            stage_identity,
            members: members
                .iter()
                .map(|(name, contents)| (name.as_str().to_owned(), sha256_bytes(contents)))
                .collect(),
        };
        let marker_path = parent.path.join(&stage_name).join("owner.json");
        let marker_bytes = encode_tree_stage_marker(&marker)?;
        if let Err(error) = write_new_file(&marker_path, &marker_bytes) {
            let cleanup = cleanup_empty_tree_root(&parent, &stage_name, stage_identity);
            return Err(combine_primary_and_cleanup(
                error,
                &cleanup.err().map_or_else(Vec::new, |e| vec![e.to_string()]),
                true,
            ));
        }
        rfs::mkdirat(
            &stage_root_fd,
            Path::new("payload"),
            Mode::from_raw_mode(0o755),
        )?;
        rfs::fsync(&stage_root_fd)?;
        let payload_fd = rfs::openat(
            &stage_root_fd,
            Path::new("payload"),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let payload_identity = identity_from_stat(&rfs::fstat(&payload_fd)?);
        let build = (|| {
            for (name, contents) in members {
                let fd = rfs::openat(
                    &payload_fd,
                    Path::new(name.as_str()),
                    OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::WRONLY
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o644),
                )?;
                let mut file = std::fs::File::from(fd);
                file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
                file.write_all(contents)?;
                file.flush()?;
                file.sync_all()?;
                let stat = rfs::fstat(&file)?;
                if identity_from_stat(&stat)
                    != identity_at_fd(&payload_fd, OsStr::new(name.as_str()))?
                        .ok_or_else(|| std::io::Error::other("tree member disappeared"))?
                {
                    return Err(std::io::Error::other(format!(
                        "tree member '{}' changed while staging",
                        name.as_str()
                    )));
                }
            }
            verify_flat_tree_members(&payload_fd, &marker.members)?;
            rfs::fsync(&payload_fd)?;
            test_crash_point("tree-before-rename");
            rfs::renameat_with(
                &stage_root_fd,
                Path::new("payload"),
                &parent.fd,
                Path::new(&parent.leaf),
                RenameFlags::NOREPLACE,
            )?;
            test_fail_point("tree-after-rename-before-sync")?;
            rfs::fsync(&parent.fd)?;
            let installed = directory_identity_at(&parent.fd, &parent.leaf)?;
            if installed != Some(payload_identity) {
                return Err(std::io::Error::other(
                    "published tree identity changed during no-clobber rename",
                ));
            }
            Ok::<(), std::io::Error>(())
        })();
        if let Err(error) = build {
            if directory_identity_at(&parent.fd, &parent.leaf)
                .ok()
                .flatten()
                == Some(payload_identity)
            {
                drop(lock);
                return Err(io_failure(PublicationError::uncertain(format!(
                    "tree rename completed but durable parent sync could not be proven: {error}; complete destination and owner marker retained"
                ))));
            }
            drop(payload_fd);
            drop(stage_root_fd);
            let cleanup = recover_flat_tree_stage(&parent, &stage_name, &parent.leaf)
                .err()
                .map_or_else(Vec::new, |cleanup| vec![cleanup.to_string()]);
            drop(lock);
            return Err(combine_primary_and_cleanup(error, &cleanup, true));
        }
        drop(payload_fd);
        drop(stage_root_fd);
        cleanup_completed_tree_root(&parent, &stage_name, &marker).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "tree publication committed, but its owner marker could not be cleaned: {error}; complete destination retained"
            )))
        })?;
        drop(lock);
        Ok(PublicationReceipt::new(recovery))
    }

    pub(super) fn publish_prepared_tree_noclobber(
        staging: &Path,
        destination: &Path,
        name_policy: PreparedTreeNamePolicy,
    ) -> std::io::Result<PublicationReceipt> {
        let stage_parent = open_parent(staging, false)?;
        let destination_parent = open_parent(destination, true)?;
        let stage_parent_identity = identity_from_stat(&rfs::fstat(&stage_parent.fd)?);
        let destination_parent_identity = identity_from_stat(&rfs::fstat(&destination_parent.fd)?);
        if stage_parent_identity != destination_parent_identity {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "prepared tree '{}' and destination '{}' must have the same parent directory",
                    staging.display(),
                    destination.display()
                ),
            ));
        }
        let journal = transaction_journal_path(destination, "prepared-tree")?;
        let lock = lock_for_journal(&journal)?;

        let stage_parent = open_parent(staging, false)?;
        let destination_parent = open_parent(destination, false)?;
        if identity_from_stat(&rfs::fstat(&stage_parent.fd)?) != stage_parent_identity
            || identity_from_stat(&rfs::fstat(&destination_parent.fd)?) != stage_parent_identity
        {
            return Err(std::io::Error::other(
                "prepared-tree parent changed while acquiring its publication lock",
            ));
        }
        match rfs::statat(
            &destination_parent.fd,
            &destination_parent.leaf,
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "refusing to replace existing prepared-tree destination '{}'",
                        destination.display()
                    ),
                ))
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
        let stage_fd = rfs::openat(
            &stage_parent.fd,
            &stage_parent.leaf,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let stage_identity = identity_from_stat(&rfs::fstat(&stage_fd)?);
        sync_prepared_tree(&stage_fd, staging, name_policy)?;
        if directory_identity_at(&stage_parent.fd, &stage_parent.leaf)? != Some(stage_identity) {
            return Err(std::io::Error::other(format!(
                "prepared tree '{}' changed while it was being synced",
                staging.display()
            )));
        }
        rfs::renameat_with(
            &stage_parent.fd,
            &stage_parent.leaf,
            &destination_parent.fd,
            &destination_parent.leaf,
            RenameFlags::NOREPLACE,
        )?;
        if let Err(error) = test_fail_point("prepared-tree-after-rename-before-sync")
            .and_then(|()| rfs::fsync(&destination_parent.fd).map_err(Into::into))
        {
            drop(lock);
            return Err(io_failure(PublicationError::uncertain(format!(
                "prepared-tree rename completed but durable parent sync could not be proven: {error}; complete destination retained"
            ))));
        }
        if directory_identity_at(&destination_parent.fd, &destination_parent.leaf)?
            != Some(stage_identity)
        {
            drop(lock);
            return Err(io_failure(PublicationError::uncertain(
                "prepared-tree destination identity changed after rename",
            )));
        }
        drop(lock);
        Ok(PublicationReceipt::default())
    }

    pub(super) fn measure_tree_content_cas(path: &Path) -> std::io::Result<TreeContentCas> {
        let parent = open_parent(path, false)?;
        let directory = rfs::openat(
            &parent.fd,
            &parent.leaf,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let root = identity_from_stat(&rfs::fstat(&directory)?);
        let entries = stable_measure_tree_content_at(&directory, path)?;
        if identity_from_stat(&rfs::fstat(&directory)?) != root
            || directory_identity_at(&parent.fd, &parent.leaf)? != Some(root)
        {
            return Err(std::io::Error::other(format!(
                "tree '{}' changed while it was measured",
                path.display()
            )));
        }
        Ok(TreeContentCas { root, entries })
    }

    pub(super) fn exchange_prepared_tree(
        staging: &Path,
        destination: &Path,
    ) -> std::io::Result<PublicationReceipt> {
        let expected = measure_tree_content_cas(destination)?;
        exchange_prepared_tree_if_unchanged(staging, destination, &expected)
    }

    pub(super) fn exchange_prepared_tree_if_unchanged(
        staging: &Path,
        destination: &Path,
        expected_destination: &TreeContentCas,
    ) -> std::io::Result<PublicationReceipt> {
        let stage_parent = open_parent(staging, false)?;
        let destination_parent = open_parent(destination, false)?;
        let parent_identity = identity_from_stat(&rfs::fstat(&stage_parent.fd)?);
        if identity_from_stat(&rfs::fstat(&destination_parent.fd)?) != parent_identity {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "prepared tree '{}' and destination '{}' must have the same parent directory",
                    staging.display(),
                    destination.display()
                ),
            ));
        }
        let journal = transaction_journal_path(destination, "prepared-tree")?;
        let lock = lock_for_journal(&journal)?;

        let stage_parent = open_parent(staging, false)?;
        let destination_parent = open_parent(destination, false)?;
        if identity_from_stat(&rfs::fstat(&stage_parent.fd)?) != parent_identity
            || identity_from_stat(&rfs::fstat(&destination_parent.fd)?) != parent_identity
        {
            return Err(std::io::Error::other(
                "prepared-tree parent changed while acquiring its publication lock",
            ));
        }
        let stage_fd = rfs::openat(
            &stage_parent.fd,
            &stage_parent.leaf,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let destination_fd = rfs::openat(
            &destination_parent.fd,
            &destination_parent.leaf,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let stage_identity = identity_from_stat(&rfs::fstat(&stage_fd)?);
        let destination_identity = identity_from_stat(&rfs::fstat(&destination_fd)?);
        if destination_identity != expected_destination.root {
            return Err(std::io::Error::other(
                "prepared-tree destination identity changed before atomic exchange",
            ));
        }
        let expected_stage = stable_measure_tree_content_at(&stage_fd, staging)?;
        test_pause_point("prepared-tree-after-stage-content-cas-before-sync");
        sync_prepared_tree(
            &stage_fd,
            staging,
            PreparedTreeNamePolicy::PortableGeneratedOutput,
        )?;
        if stable_measure_tree_content_at(&stage_fd, staging)? != expected_stage {
            return Err(std::io::Error::other(
                "prepared-tree staging content changed while it was synced",
            ));
        }
        test_pause_point("prepared-tree-before-content-cas");
        let current_destination = TreeContentCas {
            root: destination_identity,
            entries: stable_measure_tree_content_at(&destination_fd, destination)?,
        };
        if &current_destination != expected_destination {
            return Err(std::io::Error::other(
                "prepared-tree destination content changed before atomic exchange",
            ));
        }
        test_pause_point("prepared-tree-after-content-cas-before-exchange");
        if directory_identity_at(&stage_parent.fd, &stage_parent.leaf)? != Some(stage_identity)
            || directory_identity_at(&destination_parent.fd, &destination_parent.leaf)?
                != Some(destination_identity)
        {
            return Err(std::io::Error::other(
                "prepared tree or destination changed before atomic exchange",
            ));
        }
        rfs::renameat_with(
            &stage_parent.fd,
            &stage_parent.leaf,
            &destination_parent.fd,
            &destination_parent.leaf,
            RenameFlags::EXCHANGE,
        )?;
        test_pause_point("prepared-tree-after-exchange-before-content-cas");
        let installed_after_exchange = stable_measure_tree_content_at(&stage_fd, destination);
        let previous_after_exchange =
            stable_measure_tree_content_at(&destination_fd, staging).map(|entries| {
                TreeContentCas {
                    root: destination_identity,
                    entries,
                }
            });
        let installed_matches = installed_after_exchange
            .as_ref()
            .is_ok_and(|entries| entries == &expected_stage);
        let previous_matches = previous_after_exchange
            .as_ref()
            .is_ok_and(|measured| measured == expected_destination);
        if !installed_matches || !previous_matches {
            let reason = match (&installed_after_exchange, &previous_after_exchange) {
                (Err(installed), Err(previous)) => format!(
                    "installed and previous trees could not be verified after atomic exchange: {installed}; {previous}"
                ),
                (Err(error), Ok(_)) => {
                    format!("installed tree could not be verified after atomic exchange: {error}")
                }
                (Ok(_), Err(error)) => {
                    format!("previous tree could not be verified after atomic exchange: {error}")
                }
                (Ok(_), Ok(_)) => {
                    "installed or previous tree content changed across atomic exchange".to_owned()
                }
            };

            let swapped_stage_binding = directory_identity_at(&stage_parent.fd, &stage_parent.leaf);
            let swapped_destination_binding =
                directory_identity_at(&destination_parent.fd, &destination_parent.leaf);
            if !matches!(&swapped_stage_binding, Ok(Some(identity)) if *identity == destination_identity)
                || !matches!(&swapped_destination_binding, Ok(Some(identity)) if *identity == stage_identity)
            {
                drop(lock);
                return Err(io_failure(
                    PublicationError::commit_state_uncertain_with_incomplete_rollback(format!(
                        "{reason}; refusing compensating tree exchange because the swapped path bindings changed or could not be measured (staging={swapped_stage_binding:?}, destination={swapped_destination_binding:?})"
                    )),
                ));
            }

            if let Err(rollback_error) = rfs::renameat_with(
                &stage_parent.fd,
                &stage_parent.leaf,
                &destination_parent.fd,
                &destination_parent.leaf,
                RenameFlags::EXCHANGE,
            )
            .map_err(std::io::Error::from)
            .and_then(|()| test_fail_point("prepared-tree-after-compensating-exchange-before-sync"))
            .and_then(|()| rfs::fsync(&destination_parent.fd).map_err(Into::into))
            {
                drop(lock);
                return Err(io_failure(
                    PublicationError::commit_state_uncertain_with_incomplete_rollback(format!(
                        "{reason}; compensating tree exchange or its parent sync failed: {rollback_error}; commit state cannot be proven"
                    )),
                ));
            }

            let restored_destination = stable_measure_tree_content_at(&destination_fd, destination);
            let retained_staging = stable_measure_tree_content_at(&stage_fd, staging);
            let restored_destination_binding =
                directory_identity_at(&destination_parent.fd, &destination_parent.leaf);
            let restored_stage_binding =
                directory_identity_at(&stage_parent.fd, &stage_parent.leaf);
            let destination_is_restored = restored_destination
                .as_ref()
                .is_ok_and(|entries| entries == &expected_destination.entries)
                && matches!(
                    &restored_destination_binding,
                    Ok(Some(identity)) if *identity == destination_identity
                );
            let prepared_tree_is_retained =
                match (installed_after_exchange.as_ref(), retained_staging.as_ref()) {
                    (Ok(before), Ok(after)) => before == after,
                    _ => false,
                } && matches!(
                    &restored_stage_binding,
                    Ok(Some(identity)) if *identity == stage_identity
                );
            if !destination_is_restored || !prepared_tree_is_retained {
                drop(lock);
                return Err(io_failure(
                    PublicationError::commit_state_uncertain_with_incomplete_rollback(format!(
                        "{reason}; compensating tree exchange completed, but post-exchange identity/content verification failed (destination_content={restored_destination:?}, staging_content={retained_staging:?}, destination_binding={restored_destination_binding:?}, staging_binding={restored_stage_binding:?})"
                    )),
                ));
            }
            drop(lock);
            return Err(std::io::Error::other(format!(
                "{reason}; compensating tree exchange restored the original namespace"
            )));
        }
        if let Err(error) = test_fail_point("prepared-tree-after-exchange-before-sync")
            .and_then(|()| rfs::fsync(&destination_parent.fd).map_err(Into::into))
        {
            drop(lock);
            return Err(io_failure(PublicationError::uncertain(format!(
                "prepared-tree exchange completed but durable parent sync could not be proven: {error}; complete old and new trees were retained"
            ))));
        }
        let installed_identity =
            directory_identity_at(&destination_parent.fd, &destination_parent.leaf).map_err(
                |error| {
                    io_failure(PublicationError::uncertain(format!(
                "prepared-tree destination could not be measured after atomic exchange: {error}"
            )))
                },
            )?;
        let previous_identity =
            directory_identity_at(&stage_parent.fd, &stage_parent.leaf).map_err(|error| {
                io_failure(PublicationError::uncertain(format!(
                    "previous prepared-tree destination could not be measured after atomic exchange: {error}"
                )))
            })?;
        if installed_identity != Some(stage_identity)
            || previous_identity != Some(destination_identity)
        {
            drop(lock);
            return Err(io_failure(PublicationError::uncertain(
                "prepared-tree identities changed after atomic exchange",
            )));
        }
        drop(lock);
        Ok(PublicationReceipt::default())
    }

    fn apply_operation(operation: &JournalOperation) -> std::io::Result<()> {
        let parent = open_parent(&operation.target, false)?;
        if identity_from_stat(&rfs::fstat(&parent.fd)?) != operation.parent_identity {
            return Err(std::io::Error::other(format!(
                "publication parent identity changed before commit: '{}'",
                operation.target.display()
            )));
        }
        let current = read_regular_with_mode(&operation.target)?;
        if !snapshot_matches(
            current.as_ref(),
            operation.original,
            operation.original_digest.as_ref(),
            operation.original_mode,
        ) {
            return Err(std::io::Error::other(format!(
                "publication target changed before commit: '{}'",
                operation.target.display()
            )));
        }

        if let Some(backup_path) = &operation.backup {
            let backup = open_parent(backup_path, false)?;
            rename_noclobber(&parent, &parent.leaf, &backup, &backup.leaf)?;
            let moved = read_regular_with_mode(backup_path)?;
            if !snapshot_matches(
                moved.as_ref(),
                operation.original,
                operation.original_digest.as_ref(),
                operation.original_mode,
            ) {
                let _ = rename_noclobber(&backup, &backup.leaf, &parent, &parent.leaf);
                return Err(std::io::Error::other(format!(
                    "publication target identity, digest, or mode changed during backup rename: '{}'",
                    operation.target.display()
                )));
            }
            sync_parent(&operation.target)?;
            test_crash_point("after-backup");
        }
        if let Some(stage_path) = &operation.stage {
            let staged = read_regular_with_mode(stage_path)?;
            let stage_matches = snapshot_matches(
                staged.as_ref(),
                operation.installed,
                operation.desired_digest.as_ref(),
                operation.desired_mode,
            );
            if !stage_matches {
                return Err(std::io::Error::other(format!(
                    "staged file identity, digest, or mode changed before commit: '{}'",
                    stage_path.display()
                )));
            }
            let stage = open_parent(stage_path, false)?;
            rename_noclobber(&stage, &stage.leaf, &parent, &parent.leaf)?;
            let installed = read_regular_with_mode(&operation.target)?;
            let installed_matches = snapshot_matches(
                installed.as_ref(),
                operation.installed,
                operation.desired_digest.as_ref(),
                operation.desired_mode,
            );
            if !installed_matches {
                return Err(std::io::Error::other(format!(
                    "installed file identity, digest, or mode mismatch for '{}'",
                    operation.target.display()
                )));
            }
            sync_parent(&operation.target)?;
            test_crash_point("after-install");
        }
        Ok(())
    }

    fn snapshot_matches(
        snapshot: Option<&(FileIdentity, Vec<u8>, u16)>,
        expected_identity: Option<FileIdentity>,
        expected_digest: Option<&Sha256Digest>,
        expected_mode: Option<u16>,
    ) -> bool {
        match snapshot {
            Some((identity, contents, mode)) => {
                Some(*identity) == expected_identity
                    && Some(&sha256_bytes(contents)) == expected_digest
                    && expected_mode.is_none_or(|expected| expected == *mode)
            }
            None => {
                expected_identity.is_none() && expected_digest.is_none() && expected_mode.is_none()
            }
        }
    }

    fn rollback_operations(operations: &[JournalOperation]) -> Vec<String> {
        let mut errors = Vec::new();
        for operation in operations.iter().rev() {
            if let Err(error) = rollback_operation(operation) {
                errors.push(format!("{}: {error}", operation.target.display()));
            }
        }
        errors
    }

    fn rollback_operation(operation: &JournalOperation) -> std::io::Result<()> {
        let mut errors = Vec::new();
        let target_parent = open_parent(&operation.target, false).map_err(|error| {
            errors.push(format!("open target parent: {error}"));
        });

        if let Ok(parent) = &target_parent {
            match rfs::fstat(&parent.fd) {
                Ok(stat) if identity_from_stat(&stat) == operation.parent_identity => {}
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "target parent identity changed; refusing path-based rollback",
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }

        if let Ok(parent) = &target_parent {
            match read_regular_with_mode(&operation.target) {
                Ok(current)
                    if snapshot_matches(
                        current.as_ref(),
                        operation.installed,
                        operation.desired_digest.as_ref(),
                        operation.desired_mode,
                    ) =>
                {
                    match (operation.installed, operation.desired_digest.as_ref()) {
                        (Some(identity), Some(digest)) => {
                            if let Err(error) = remove_at_exact_mode(
                                parent,
                                identity,
                                digest,
                                operation.desired_mode,
                            ) {
                                errors.push(format!("remove installed target: {error}"));
                            }
                        }
                        _ => errors.push("installed target lacks journal proof".to_owned()),
                    }
                }
                Ok(current)
                    if current.is_some()
                        && !snapshot_matches(
                            current.as_ref(),
                            operation.original,
                            operation.original_digest.as_ref(),
                            operation.original_mode,
                        ) =>
                {
                    errors.push("target contains unexpected identity or bytes".to_owned());
                }
                Ok(_) => {}
                Err(error) => errors.push(format!("measure target: {error}")),
            }
        }

        if let Some(backup_path) = &operation.backup {
            match (open_parent(backup_path, false), target_parent.as_ref()) {
                (Ok(backup_parent), Ok(target_parent)) => match read_regular_with_mode(backup_path)
                {
                    Ok(backup)
                        if snapshot_matches(
                            backup.as_ref(),
                            operation.original,
                            operation.original_digest.as_ref(),
                            operation.original_mode,
                        ) =>
                    {
                        match read_regular_with_mode(&operation.target) {
                            Ok(None) => {
                                if let Err(error) = rename_noclobber(
                                    &backup_parent,
                                    &backup_parent.leaf,
                                    target_parent,
                                    &target_parent.leaf,
                                )
                                .and_then(|()| sync_parent(&operation.target))
                                .and_then(|()| verify_original_restored(operation))
                                {
                                    errors.push(format!("restore backup: {error}"));
                                }
                            }
                            Ok(current)
                                if snapshot_matches(
                                    current.as_ref(),
                                    operation.original,
                                    operation.original_digest.as_ref(),
                                    operation.original_mode,
                                ) => {}
                            Ok(_) => errors
                                .push("cannot restore backup over unexpected target".to_owned()),
                            Err(error) => {
                                errors.push(format!("measure restore target: {error}"));
                            }
                        }
                    }
                    Ok(Some(_)) => {
                        errors.push("backup identity, digest, or mode mismatch".to_owned());
                    }
                    Ok(None) => match read_regular_with_mode(&operation.target) {
                        Ok(current)
                            if snapshot_matches(
                                current.as_ref(),
                                operation.original,
                                operation.original_digest.as_ref(),
                                operation.original_mode,
                            ) => {}
                        Ok(_) if operation.original.is_some() => {
                            errors.push("required rollback backup is missing".to_owned());
                        }
                        Ok(_) => {}
                        Err(error) => {
                            errors.push(format!("measure target without backup: {error}"));
                        }
                    },
                    Err(error) => errors.push(format!("measure backup: {error}")),
                },
                (Err(error), _) => errors.push(format!("open backup parent: {error}")),
                (Ok(_), Err(())) => {}
            }
        }
        if let Some(stage) = &operation.stage {
            if let Err(error) = remove_owned_stage(operation, stage) {
                errors.push(format!("remove stage: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(std::io::Error::other(errors.join("; ")))
        }
    }

    fn verify_original_restored(operation: &JournalOperation) -> std::io::Result<()> {
        let restored = read_regular_with_mode(&operation.target)?;
        if snapshot_matches(
            restored.as_ref(),
            operation.original,
            operation.original_digest.as_ref(),
            operation.original_mode,
        ) {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "restored target identity, digest, or mode mismatch for '{}'",
                operation.target.display()
            )))
        }
    }

    fn remove_owned_stage(operation: &JournalOperation, stage: &Path) -> std::io::Result<()> {
        if let (Some(identity), Some(digest)) =
            (operation.installed, operation.desired_digest.as_ref())
        {
            remove_operation_aux_exact(operation, stage, identity, digest, operation.desired_mode)
        } else {
            remove_operation_aux_unidentified(operation, stage)
        }
    }

    fn cleanup_stages(operations: &[JournalOperation]) -> Vec<String> {
        let mut errors = Vec::new();
        for operation in operations {
            if let Some(stage) = &operation.stage {
                if let Err(error) = remove_owned_stage(operation, stage) {
                    errors.push(format!("{}: {error}", stage.display()));
                }
            }
        }
        errors
    }

    fn cleanup_committed(operations: &[JournalOperation]) -> Vec<String> {
        let mut errors = cleanup_stages(operations);
        for operation in operations {
            if let Some(backup) = &operation.backup {
                match (operation.original, operation.original_digest.as_ref()) {
                    (Some(identity), Some(digest)) => {
                        if let Err(error) = remove_operation_aux_exact(
                            operation,
                            backup,
                            identity,
                            digest,
                            operation.original_mode,
                        ) {
                            errors.push(format!("{}: {error}", backup.display()));
                        }
                    }
                    _ => errors.push(format!(
                        "{}: committed backup lacks journal identity or digest",
                        backup.display()
                    )),
                }
            }
        }
        errors
    }

    fn finish_failed_transaction(
        journal_path: &Path,
        primary: std::io::Error,
        rollback: &[String],
    ) -> std::io::Result<()> {
        if !rollback.is_empty() {
            return Err(io_failure(PublicationError::rollback(format!(
                "{primary}; rollback incomplete: {}; journal retained",
                rollback.join("; ")
            ))));
        }
        if let Err(error) = cleanup_journal_stage(journal_path) {
            return Err(io_failure(PublicationError::rollback(format!(
                "{primary}; rollback completed, but staged-journal cleanup failed: {error}; files retained"
            ))));
        }
        let Some((identity, _)) = read_regular(journal_path).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "{primary}; rollback completed, but journal identity could not be measured: {error}; journal retained"
            )))
        })? else {
            return Err(io_failure(PublicationError::rollback(format!(
                "{primary}; rollback completed, but the prepared journal disappeared unexpectedly"
            ))));
        };
        remove_journal(journal_path, identity).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "{primary}; rollback completed, but journal removal failed: {error}; journal retained"
            )))
        })?;
        Err(primary)
    }

    fn finish_cleanup_only_publication(
        journal_path: &Path,
        primary: std::io::Error,
    ) -> std::io::Result<()> {
        match recover_if_needed(journal_path) {
            Ok(_) => Err(primary),
            Err(recovery) => Err(io_failure(PublicationError::rollback(format!(
                "{primary}; cleanup-only recovery is incomplete: {recovery}; journal retained"
            )))),
        }
    }

    fn remove_journal(path: &Path, expected: FileIdentity) -> std::io::Result<()> {
        test_fail_point("journal-remove")?;
        let (_, bytes) = read_regular(path)?.ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotFound, "publication journal disappeared")
        })?;
        remove_regular_exact(path, expected, &sha256_bytes(&bytes))
    }

    mod filesystem;
    use filesystem::{
        identity_at, identity_at_fd, identity_from_stat, open_parent, remove_at_exact_mode,
        remove_operation_aux_exact, remove_operation_aux_unidentified, remove_regular_exact,
        rename_noclobber, sibling_name, write_new_file, write_new_file_mode,
    };
    mod journal;
    use journal::{cleanup_journal_stage, parse_journal, validate_journal, write_journal};

    mod tree;
    use tree::{
        cleanup_completed_tree_root, cleanup_empty_tree_root, directory_identity_at,
        encode_tree_stage_marker, recover_flat_tree_stage, stable_measure_tree_content_at,
        sync_prepared_tree, tree_stage_name, verify_flat_tree_members,
    };

    #[cfg(debug_assertions)]
    fn test_fail_point(point: &str) -> std::io::Result<()> {
        if test_point_matches("AROS_PUBLICATION_TEST_FAIL_AT", point) {
            return Err(std::io::Error::other(format!(
                "injected publication failure at {point}"
            )));
        }
        Ok(())
    }

    #[cfg(not(debug_assertions))]
    fn test_fail_point(_point: &str) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(debug_assertions)]
    fn test_pause_point(point: &str) {
        if test_point_matches("AROS_PUBLICATION_TEST_PAUSE_AT", point) {
            let millis = std::env::var("AROS_PUBLICATION_TEST_PAUSE_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(250);
            std::thread::sleep(std::time::Duration::from_millis(millis));
        }
    }

    #[cfg(not(debug_assertions))]
    fn test_pause_point(_point: &str) {}

    #[cfg(debug_assertions)]
    fn test_point_matches(variable: &str, point: &str) -> bool {
        let Some(value) = std::env::var(variable).ok() else {
            return false;
        };
        if value == point {
            return true;
        }
        value.split_once('@').is_some_and(|(configured, thread)| {
            configured == point && std::thread::current().name() == Some(thread)
        })
    }

    #[cfg(debug_assertions)]
    fn test_crash_point(point: &str) {
        if std::env::var_os("AROS_PUBLICATION_TEST_CRASH_AT")
            .as_deref()
            .is_some_and(|value| value == OsStr::new(point))
        {
            std::process::abort();
        }
    }

    #[cfg(not(debug_assertions))]
    fn test_crash_point(_point: &str) {}

    fn sync_parent(path: &Path) -> std::io::Result<()> {
        let parent = open_parent(path, false)?;
        rfs::fsync(&parent.fd)?;
        Ok(())
    }

    fn combine_primary_and_cleanup(
        primary: std::io::Error,
        cleanup: &[String],
        rollback: bool,
    ) -> std::io::Error {
        if cleanup.is_empty() {
            return primary;
        }
        let message = format!("{primary}; cleanup incomplete: {}", cleanup.join("; "));
        if rollback {
            io_failure(PublicationError::rollback(message))
        } else {
            io_failure(PublicationError::new(message))
        }
    }
}

#[cfg(test)]
mod publication_tests;
