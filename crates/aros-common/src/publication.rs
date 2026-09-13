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

impl FileIdentity {
    /// Filesystem device number measured through a no-follow descriptor.
    #[must_use]
    pub const fn device(self) -> u64 {
        self.device
    }

    /// Filesystem inode number measured through a no-follow descriptor.
    #[must_use]
    pub const fn inode(self) -> u64 {
        self.inode
    }
}

mod limits;
pub use limits::{
    probe_advisory_file_lock, AdvisoryFileLock, AdvisoryLockObservation, AdvisoryLockState,
    TreeTraversalLimits,
};

mod tree_cas;
pub use tree_cas::TreeContentCas;
use tree_cas::{TreeContentEntry, TreeNodeSnapshot};
mod payload_path;
pub use payload_path::payload_casefold_path_key;
mod tree_ops;
pub use tree_ops::{
    copy_tree_from_snapshot_nofollow, create_unique_directory_nofollow,
    directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    measure_tree_content_cas_bounded, remove_regular_file_from_snapshot_nofollow,
    remove_tree_from_snapshot_nofollow, validate_existing_directory_prefix_nofollow,
};

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
    measure_regular_file_bounded(path, u64::MAX)
}

/// Read a regular file through a no-follow descriptor under an exact byte
/// ceiling, returning its identity and contents as one stable snapshot.
///
/// This is the bounded counterpart of [`measure_regular_file`].  It is for
/// control-plane documents such as locks and receipts: an untrusted file must
/// not turn a preview or compare-and-swap precondition into an unbounded read.
///
/// # Errors
///
/// Returns an error when the file is unsafe, changes while being read, or
/// exceeds `max_bytes`.
pub fn measure_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>)>> {
    #[cfg(unix)]
    {
        unix::read_regular_bounded(&absolute_path(path)?, max_bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, max_bytes);
        Err(unsupported_durability())
    }
}

/// Open one existing regular file through a descriptor-relative no-follow
/// path walk.
///
/// The returned descriptor remains bound to the file opened during validation,
/// so a later path replacement cannot redirect a caller's read. Callers that
/// need a stable content snapshot must still compare their own measured
/// length/digest after reading: an already-open regular file can be modified
/// in place by another writer.
///
/// # Errors
///
/// Returns an I/O or file-type error when a parent component or the final leaf
/// is a symlink, when the path is not a regular file, or on non-Unix hosts
/// where this no-follow contract is unavailable.
pub fn open_regular_file_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        unix::open_regular_file_nofollow(&absolute_path(path)?)
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
        unix::test_fail_path(&target)?;
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

/// Return the persistent advisory-lock path for one publication journal.
///
/// The lock is intentionally a sibling of the journal so independent
/// publishers recover and serialize the same durable namespace. The returned
/// path is a naming convention only; acquire it through
/// [`AdvisoryFileLock::acquire`] when mutual exclusion is required.
///
/// # Errors
///
/// Returns `InvalidInput` if the journal path has no UTF-8 leaf or parent.
pub fn publication_journal_lock_path(journal: &Path) -> std::io::Result<PathBuf> {
    let parent = journal.parent().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidInput, "publication journal has no parent")
    })?;
    let leaf = journal.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "publication journal has no UTF-8 leaf for its advisory lock",
        )
    })?;
    let digest = sha256_bytes(leaf.as_bytes()).to_string();
    Ok(parent.join(format!(".aros-lock-{}-0000000000000000", &digest[..16])))
}

/// Return whether `name` has the exact portable form of a persistent
/// publication-journal advisory lock.
///
/// This recognizes the control-plane lock itself, not an arbitrary object as
/// safe for deletion. Callers must still verify its regular-file type through
/// a no-follow lookup.
#[must_use]
pub fn is_publication_journal_lock_name(name: &str) -> bool {
    const PREFIX: &str = ".aros-lock-";
    const SUFFIX: &str = "-0000000000000000";
    let Some(digest) = name
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
    else {
        return false;
    };
    digest.len() == 16
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(not(unix))]
fn unsupported_durability() -> std::io::Error {
    std::io::Error::new(
        ErrorKind::Unsupported,
        "durable publication is unavailable on this platform: no no-follow, CAS, and write-through directory contract",
    )
}

#[cfg(unix)]
mod unix;

#[cfg(test)]
mod publication_tests;
