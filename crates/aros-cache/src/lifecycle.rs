//! Exact-object retention and preview/apply removal for AROS-owned caches.
//!
//! This module deliberately has no root-wide deletion API. A caller must
//! supply a family, an existing absolute cache root and a portable relative
//! object path selected by that family's verifier. The module measures that
//! exact object, binds its snapshot to a short-lived preview token and removes
//! it only after re-measuring under the object's lifecycle lock.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aros_common::{
    directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    is_publication_journal_lock_name, measure_regular_file_bounded,
    measure_tree_content_cas_bounded, publish_atomic_file,
    remove_regular_file_from_snapshot_nofollow, remove_tree_from_snapshot_nofollow, sha256_bytes,
    validate_private_directory_nofollow, AdvisoryFileLock, AtomicFilePolicy, FileIdentity,
    PortableOutputName, Sha256Digest, TreeContentCas, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{resolve_explicit_root, CacheFamily, RootResolutionError};

const LIFECYCLE_SCHEMA: &str = "aros-cache-lifecycle-v1";
const RETENTION_SCHEMA: &str = "aros-cache-retention-v1";
const TOKEN_SCHEMA: &str = "aros-cache-remove-token-v1";
const RETENTION_RELEASE_TOKEN_SCHEMA: &str = "aros-cache-release-token-v1";
const CONTROL_DIRECTORY: &str = ".aros-cache-lifecycle";
const CONTROL_VERSION: &str = "v1";
const RETENTION_DIRECTORY: &str = "retention";
const LOCK_DIRECTORY: &str = "locks";
const RETENTION_SUFFIX: &str = ".json";
const MAX_RETENTION_RECEIPTS: usize = 1024;
const MAX_RETENTION_OBJECTS: usize = 256;
const MAX_RETENTION_RECEIPT_BYTES: u64 = 64 * 1024;
const PREVIEW_LIFETIME_SECONDS: u64 = 5 * 60;

/// The kind and explicit measurement budget of one exact cache object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheObjectKind {
    /// One cache entry stored as a regular file.
    RegularFile {
        /// Maximum content bytes that a lifecycle operation may read.
        max_bytes: u64,
    },
    /// One immutable cache generation stored as a complete directory tree.
    Tree {
        /// Maximum non-root filesystem entries measured or removed.
        max_entries: usize,
        /// Maximum regular-file payload bytes measured or removed.
        max_regular_file_bytes: u64,
    },
}

impl CacheObjectKind {
    const fn label(self) -> &'static str {
        match self {
            Self::RegularFile { .. } => "regular_file",
            Self::Tree { .. } => "tree",
        }
    }

    fn tree_limits(self) -> Result<TreeTraversalLimits, CacheLifecycleError> {
        match self {
            Self::Tree {
                max_entries,
                max_regular_file_bytes,
            } => TreeTraversalLimits::new(max_entries, max_regular_file_bytes)
                .map_err(|error| CacheLifecycleError::invalid(error.to_string())),
            Self::RegularFile { .. } => Err(CacheLifecycleError::invalid(
                "a regular-file cache object does not have tree traversal limits",
            )),
        }
    }

    const fn token_policy(self) -> TokenObjectPolicy {
        match self {
            Self::RegularFile { max_bytes } => TokenObjectPolicy::RegularFile { max_bytes },
            Self::Tree {
                max_entries,
                max_regular_file_bytes,
            } => TokenObjectPolicy::Tree {
                max_entries,
                max_regular_file_bytes,
            },
        }
    }
}

/// One caller-selected immutable object below an explicit cache root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheObjectRequest {
    /// Resource family that owns the object semantics.
    pub family: CacheFamily,
    /// Existing absolute root selected by the family command.
    pub cache_root: PathBuf,
    /// Portable relative path of the exact selected object below `cache_root`.
    pub relative_path: PathBuf,
    /// Object representation and bounded measurement policy.
    pub kind: CacheObjectKind,
}

/// User-provided name of a persisted retention reference to release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRetentionRelease {
    /// Resource family holding the reference.
    pub family: CacheFamily,
    /// Exact cache root that owns the reference namespace.
    pub cache_root: PathBuf,
    /// Existing portable retention-reference name.
    pub name: String,
}

/// Non-mutating, short-lived plan to release one named retention reference.
///
/// Releasing a reference never removes cached bytes directly, but it can make
/// them eligible for a later destructive operation. It therefore has the same
/// snapshot-bound preview/apply boundary as object removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRetentionReleasePreview {
    /// Result document schema.
    pub schema: &'static str,
    /// Planned operation.
    pub operation: &'static str,
    /// Exact family selected by the caller.
    pub family: CacheFamily,
    /// Absolute cache root bound into the token.
    pub cache_root: PathBuf,
    /// Named retention reference selected by the caller.
    pub name: String,
    /// Portable path of the receipt that would be removed.
    pub relative_path: String,
    /// Exact descriptor- and content-measured receipt proof.
    pub receipt: CacheObjectProof,
    /// Number of cache objects that the receipt currently protects.
    pub retained_object_count: usize,
    /// Unix timestamp after which this preview is rejected.
    pub expires_unix_seconds: u64,
    /// Exact token required by [`apply_retention_release`].
    pub apply_token: String,
    /// Explicit recovery boundary for this single-reference primitive.
    pub recovery: &'static str,
}

/// An OS-held lifecycle lease for one selected cache object.
///
/// The lease is intentionally non-cloneable. Retain it for the complete read
/// or write operation; dropping it releases the advisory lock.
#[derive(Debug)]
pub struct CacheObjectLease {
    cache_root: PathBuf,
    relative_path: String,
    lock_path: PathBuf,
    lock: AdvisoryFileLock,
}

/// An OS-held, all-or-nothing lifecycle lease set for one closed cache
/// selection.
///
/// Every contained lease belongs to the same cache root and family. The
/// requests are canonicalized into portable-path order before acquisition, so
/// independently constructed closures cannot deadlock by taking locks in a
/// different order. Dropping the set releases every lease.
#[derive(Debug)]
pub struct CacheObjectLeases {
    leases: Vec<CacheObjectLease>,
}

impl CacheObjectLeases {
    /// Number of exact objects protected by this set.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.leases.len()
    }

    /// Whether the set protects no objects.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    /// Reassert every no-follow lock in deterministic object order.
    ///
    /// # Errors
    ///
    /// Returns an error when any held lock can no longer be proven.
    pub fn revalidate(&self) -> Result<(), CacheLifecycleError> {
        for lease in &self.leases {
            lease.revalidate()?;
        }
        Ok(())
    }
}

impl CacheObjectLease {
    /// Absolute cache root bound to this lease.
    #[must_use]
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Portable cache-object path bound to this lease.
    #[must_use]
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Reassert that the lock file has not been substituted while held.
    ///
    /// # Errors
    ///
    /// Returns an error when the held no-follow lock can no longer be proven.
    pub fn revalidate(&self) -> Result<(), CacheLifecycleError> {
        self.lock.revalidate().map_err(|error| {
            CacheLifecycleError::io("revalidate lifecycle lease", &self.lock_path, error)
        })
    }
}

/// Content proof for the object selected during retention or preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheObjectProof {
    /// Exact snapshot of a regular cache entry.
    RegularFile {
        /// Descriptor-measured inode binding used by the preview token.
        identity: FileIdentity,
        /// SHA-256 of the complete entry bytes.
        sha256: Sha256Digest,
        /// Exact entry size in bytes.
        size: u64,
    },
    /// Complete descriptor-measured tree snapshot.
    Tree {
        /// Stable content digest excluding device, inode and timestamps.
        payload_sha256: Sha256Digest,
        /// Exact mutable filesystem snapshot digest for preview/apply CAS.
        snapshot_sha256: Sha256Digest,
        /// Number of non-root entries in the generation tree.
        entry_count: usize,
        /// Total regular-file bytes in the generation tree.
        regular_file_bytes: u64,
    },
}

/// A named retention receipt created for one exact cache selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRetentionRecord {
    /// Versioned receipt schema.
    pub schema: &'static str,
    /// Named user retention reference.
    pub name: String,
    /// Family that owns the retained object.
    pub family: CacheFamily,
    /// Absolute root path bound into the receipt.
    pub cache_root: PathBuf,
    /// Every exact selected object retained by this named reference, sorted by
    /// portable relative path.
    pub objects: Vec<CacheRetainedObject>,
    /// Unix timestamp when the receipt was written.
    pub created_unix_seconds: u64,
}

/// One object bound into a named retention reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRetainedObject {
    /// Portable object path relative to the receipt's cache root.
    pub relative_path: String,
    /// Object representation.
    pub object_kind: &'static str,
    /// Exact content proof retained by the reference.
    pub object: CacheObjectProof,
}

/// A retention record that blocks a removal preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRemovalBlocker {
    /// Receipt name that blocks removal.
    pub name: String,
    /// Whether the receipt still proves the current selected object.
    pub matches_current_object: bool,
    /// Stable reason suitable for JSON consumers and human diagnostics.
    pub reason: &'static str,
}

/// Non-mutating, short-lived removal plan for one exact object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRemovalPreview {
    /// Result document schema.
    pub schema: &'static str,
    /// Planned operation.
    pub operation: &'static str,
    /// Exact family selected by the caller.
    pub family: CacheFamily,
    /// Absolute cache root bound into the token.
    pub cache_root: PathBuf,
    /// Portable relative path of the only object this preview may remove.
    pub relative_path: String,
    /// Object representation.
    pub object_kind: &'static str,
    /// Descriptor- and content-measured object proof.
    pub object: CacheObjectProof,
    /// Named retention records that make removal ineligible.
    pub blockers: Vec<CacheRemovalBlocker>,
    /// Whether apply is eligible if all token bindings still match.
    pub eligible: bool,
    /// Unix timestamp after which this preview is rejected.
    pub expires_unix_seconds: u64,
    /// Exact token required by [`apply_removal`].
    pub apply_token: String,
    /// Explicit recovery boundary for this single-object primitive.
    pub recovery: &'static str,
}

/// Result of a successful destructive apply or named-reference release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRemovalResult {
    /// Result document schema.
    pub schema: &'static str,
    /// Completed operation.
    pub operation: &'static str,
    /// Exact family whose state changed.
    pub family: CacheFamily,
    /// Root under which the mutation was performed.
    pub cache_root: PathBuf,
    /// Relative object or receipt path changed by the operation.
    pub relative_path: String,
    /// Deterministic operation result.
    pub outcome: &'static str,
}

/// A fail-closed lifecycle error.
#[derive(Debug, Error)]
pub enum CacheLifecycleError {
    /// The caller supplied an invalid cache root.
    #[error("cache lifecycle root is invalid: {0}")]
    Root(#[from] RootResolutionError),
    /// The caller supplied an unsafe family/path/limit combination.
    #[error("invalid cache lifecycle request: {0}")]
    Invalid(String),
    /// A selected object did not exist when it was measured.
    #[error("selected cache object '{0}' does not exist")]
    Missing(PathBuf),
    /// Receipt state is absent, malformed or has an unexpected binding.
    #[error("cache lifecycle control state is invalid: {0}")]
    Control(String),
    /// An apply token is malformed, expired or no longer matches the plan.
    #[error("cache removal preview token is invalid: {0}")]
    Token(String),
    /// A named retention reference prevents deletion.
    #[error("cache object is retained by: {0}")]
    Retained(String),
    /// A family-specific verifier rejected the exact object while its
    /// lifecycle lock was held.
    #[error("cache lifecycle object validation failed: {0}")]
    Validation(String),
    /// One descriptor-relative filesystem operation failed.
    #[error("cache lifecycle {action} failed for '{}': {source}", path.display())]
    Io {
        /// Short operation label without untrusted data.
        action: &'static str,
        /// Path supplied for diagnostic context.
        path: PathBuf,
        /// Underlying fail-closed filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The system clock could not produce an expiry boundary.
    #[error("cannot establish a cache-preview expiry timestamp: {0}")]
    Clock(String),
}

impl CacheLifecycleError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Convert a bounded family-specific validation failure into the shared
    /// lifecycle error channel.
    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// Create one named retention reference for the exact current object.
///
/// The caller's root must be an existing private no-follow directory. The
/// receipt is published no-clobber, so a name can never replace or weaken an
/// existing reference.
///
/// # Errors
///
/// Returns an error for an unsafe root/path, missing or changed object,
/// invalid name, occupied name, or failed durable receipt publication.
pub fn keep(
    request: &CacheObjectRequest,
    name: &str,
) -> Result<CacheRetentionRecord, CacheLifecycleError> {
    keep_many(std::slice::from_ref(request), name)
}

/// Create one named retention reference after validating an exact object
/// under its exclusive lifecycle lock.
///
/// The supplied verifier must only inspect the object selected by `request`.
/// It runs while the same no-follow lock used to measure and retain the object
/// is held, so a cooperating writer cannot replace bytes between the
/// family-specific integrity check and durable retention publication.
///
/// # Errors
///
/// Returns the same errors as [`keep`], or [`CacheLifecycleError::Validation`]
/// when the family verifier rejects the locked object. A failed verifier never
/// creates a retention receipt.
pub fn keep_validated<F>(
    request: &CacheObjectRequest,
    name: &str,
    validate: F,
) -> Result<CacheRetentionRecord, CacheLifecycleError>
where
    F: FnOnce() -> Result<(), CacheLifecycleError>,
{
    keep_many_validated(std::slice::from_ref(request), name, validate)
}

/// Acquire a shared no-follow lease for one exact active cache read.
///
/// A shared lease coexists with other readers but rejects any concurrent
/// lifecycle removal or writer lease for the same cache object.
///
/// # Errors
///
/// Returns an error for unsafe roots or paths, or when a writer/removal lease
/// is already active.
pub fn acquire_read_lease(
    request: &CacheObjectRequest,
) -> Result<CacheObjectLease, CacheLifecycleError> {
    acquire_lease(request, LeaseMode::Read)
}

/// Acquire an exclusive no-follow lease for one cache-object writer.
///
/// An exclusive lease excludes active readers and lifecycle removals for the
/// same cache object. It does not measure or mutate the selected payload.
///
/// # Errors
///
/// Returns an error for unsafe roots or paths, or when any lease is active.
pub fn acquire_write_lease(
    request: &CacheObjectRequest,
) -> Result<CacheObjectLease, CacheLifecycleError> {
    acquire_lease(request, LeaseMode::Write)
}

/// Acquire shared no-follow leases for one exact, closed cache selection.
///
/// The complete selection must use one exact root and family and may not
/// repeat an object. Requests are acquired in portable-path order. If any lock
/// is unavailable, all previously acquired leases are dropped before this
/// function returns an error.
///
/// # Errors
///
/// Returns an error for an empty, mixed or duplicate selection, unsafe root or
/// path, or an active writer/removal lease for any selected object.
pub fn acquire_read_leases(
    requests: &[CacheObjectRequest],
) -> Result<CacheObjectLeases, CacheLifecycleError> {
    acquire_leases(requests, LeaseMode::Read)
}

/// Acquire exclusive no-follow leases for one exact, closed cache selection.
///
/// The selection and failure semantics are identical to
/// [`acquire_read_leases`], but every selected object excludes readers,
/// lifecycle removal and other writers until this returned set is dropped.
///
/// # Errors
///
/// Returns an error for an empty, mixed or duplicate selection, unsafe root or
/// path, or an active lease for any selected object.
pub fn acquire_write_leases(
    requests: &[CacheObjectRequest],
) -> Result<CacheObjectLeases, CacheLifecycleError> {
    acquire_leases(requests, LeaseMode::Write)
}

/// Create one named retention reference for an exact closed selection.
///
/// All requests must select the same root and family. Every selected object is
/// measured under its deterministically ordered lifecycle lock before one
/// no-clobber receipt records the complete selection. A failed measurement or
/// publication therefore never creates a partial retention reference.
///
/// # Errors
///
/// Returns an error for an empty, mixed-root, mixed-family or duplicate
/// selection, unsafe objects or roots, lock contention, or failed receipt
/// publication.
pub fn keep_many(
    requests: &[CacheObjectRequest],
    name: &str,
) -> Result<CacheRetentionRecord, CacheLifecycleError> {
    keep_many_validated(requests, name, || Ok(()))
}

/// Create one named retention reference for a closed selection after a
/// family-specific verifier has accepted it while every selected object lock
/// is held.
///
/// This is the multi-object counterpart to [`keep_validated`]. The verifier
/// runs after all deterministically ordered exclusive locks are acquired and
/// before any object is measured or the no-clobber receipt is published.
///
/// # Errors
///
/// Returns the same errors as [`keep_many`], or
/// [`CacheLifecycleError::Validation`] when the family verifier rejects the
/// selection. A rejected selection never produces a partial receipt.
pub fn keep_many_validated<F>(
    requests: &[CacheObjectRequest],
    name: &str,
    validate: F,
) -> Result<CacheRetentionRecord, CacheLifecycleError>
where
    F: FnOnce() -> Result<(), CacheLifecycleError>,
{
    let name = retention_name(name)?;
    let (family, root, relative_paths) = common_selection(requests)?;
    validate_private_directory_nofollow(&root)
        .map_err(|error| CacheLifecycleError::io("validate private root", &root, error))?;
    let locks = acquire_object_locks(&root, family, &relative_paths)?;
    validate()?;
    revalidate_object_locks(&locks)?;
    let mut selected = requests
        .iter()
        .map(SelectedObject::measure)
        .collect::<Result<Vec<_>, _>>()?;
    selected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    revalidate_object_locks(&locks)?;
    let directory = retention_directory(&root, family);
    ensure_directory_nofollow(&directory).map_err(|error| {
        CacheLifecycleError::io("create retention directory", &directory, error)
    })?;
    validate_private_directory_nofollow(&directory).map_err(|error| {
        CacheLifecycleError::io("validate retention directory", &directory, error)
    })?;

    let record = CacheRetentionRecord {
        schema: RETENTION_SCHEMA,
        name: name.clone(),
        family,
        cache_root: root,
        objects: selected
            .into_iter()
            .map(|object| CacheRetainedObject {
                relative_path: object.relative_path,
                object_kind: selected_kind(&object.snapshot),
                object: object.proof,
            })
            .collect(),
        created_unix_seconds: current_unix_seconds()?,
    };
    let bytes = serde_json::to_vec(&record).map_err(|error| {
        CacheLifecycleError::Control(format!("cannot encode retention receipt: {error}"))
    })?;
    let path = retention_receipt_path(&directory, &name);
    publish_atomic_file(&path, &bytes, AtomicFilePolicy::NoClobber)
        .map_err(|error| CacheLifecycleError::io("publish retention receipt", &path, error))?;
    revalidate_object_locks(&locks)?;
    Ok(record)
}

/// Preview release of exactly one named retention receipt.
///
/// Releasing a receipt does not remove cache bytes, but it removes the
/// reference that prevents a later cache-object removal. This command is
/// therefore deliberately non-mutating and emits the short-lived token needed
/// by [`apply_retention_release`].
///
/// # Errors
///
/// Returns an error when the selected root or receipt is unsafe, absent,
/// malformed, bound to a different root/family/name, or cannot be measured
/// inside the bounded receipt policy.
pub fn preview_retention_release(
    request: &CacheRetentionRelease,
) -> Result<CacheRetentionReleasePreview, CacheLifecycleError> {
    let now = current_unix_seconds()?;
    preview_retention_release_until(request, now.saturating_add(PREVIEW_LIFETIME_SECONDS))
}

/// Apply one previously previewed named-retention release.
///
/// The token is bound to the exact receipt contents, its no-follow descriptor
/// identity, selected cache root, family and reference name. The receipt is
/// measured and validated again while an exclusive lifecycle lock is held, so
/// a substituted, changed or expired preview can never release a reference.
///
/// # Errors
///
/// Returns an error for a malformed, expired or stale token; unsafe root or
/// receipt state; lock contention; or failed descriptor-relative removal.
pub fn apply_retention_release(
    request: &CacheRetentionRelease,
    apply_token: &str,
) -> Result<CacheRemovalResult, CacheLifecycleError> {
    apply_retention_release_at(request, apply_token, current_unix_seconds()?)
}

fn apply_retention_release_at(
    request: &CacheRetentionRelease,
    apply_token: &str,
    now: u64,
) -> Result<CacheRemovalResult, CacheLifecycleError> {
    let expires = parse_token_expiry_for(apply_token, RETENTION_RELEASE_TOKEN_SCHEMA)?;
    if now > expires {
        return Err(CacheLifecycleError::Token(
            "the release preview expired; run the preview command again".to_owned(),
        ));
    }
    if expires.saturating_sub(now) > PREVIEW_LIFETIME_SECONDS {
        return Err(CacheLifecycleError::Token(
            "the release preview expiry is outside the permitted lifetime".to_owned(),
        ));
    }

    let (root, _name, relative_path) = retention_receipt_location(request)?;
    let (lock_path, lock) = acquire_object_lock(&root, request.family, &relative_path)?;
    let prepared = prepare_retention_release(request, expires)?;
    if prepared.preview.apply_token != apply_token {
        return Err(CacheLifecycleError::Token(
            "the selected retention receipt, root, family or reference name changed; run the preview command again"
                .to_owned(),
        ));
    }
    lock.revalidate()
        .map_err(|error| CacheLifecycleError::io("revalidate lifecycle lock", &lock_path, error))?;
    prepared.receipt.remove()?;
    Ok(CacheRemovalResult {
        schema: LIFECYCLE_SCHEMA,
        operation: "release",
        family: request.family,
        cache_root: root,
        relative_path,
        outcome: "retention_reference_released",
    })
}

/// Measure one object and return a short-lived preview token for its removal.
///
/// This operation creates no lock, receipt or directory. It fails closed if
/// the existing control namespace is malformed rather than interpreting an
/// unverified record as disposable.
///
/// # Errors
///
/// Returns an error when the object/control state is missing, unsafe, corrupt
/// or cannot be measured inside the caller-supplied resource limits.
pub fn preview_removal(
    request: &CacheObjectRequest,
) -> Result<CacheRemovalPreview, CacheLifecycleError> {
    let now = current_unix_seconds()?;
    preview_removal_until(request, now.saturating_add(PREVIEW_LIFETIME_SECONDS))
}

/// Apply a previewed exact-object removal.
///
/// The method creates an object-scoped lock only after token syntax and expiry
/// validation. Under that lock it re-measures the object, rereads every
/// retention reference and rebuilds the same token. Any change means no cache
/// object is removed.
///
/// # Errors
///
/// Returns an error for stale/tampered/expired tokens, retention blockers,
/// contention, measurement races or descriptor-relative cleanup failures.
pub fn apply_removal(
    request: &CacheObjectRequest,
    apply_token: &str,
) -> Result<CacheRemovalResult, CacheLifecycleError> {
    apply_removal_at(request, apply_token, current_unix_seconds()?)
}

fn apply_removal_at(
    request: &CacheObjectRequest,
    apply_token: &str,
    now: u64,
) -> Result<CacheRemovalResult, CacheLifecycleError> {
    let expires = parse_token_expiry(apply_token)?;
    if now > expires {
        return Err(CacheLifecycleError::Token(
            "the preview expired; run the preview command again".to_owned(),
        ));
    }
    if expires.saturating_sub(now) > PREVIEW_LIFETIME_SECONDS {
        return Err(CacheLifecycleError::Token(
            "the preview expiry is outside the permitted lifetime".to_owned(),
        ));
    }

    let root = selected_root(&request.cache_root)?;
    validate_private_directory_nofollow(&root)
        .map_err(|error| CacheLifecycleError::io("validate private root", &root, error))?;
    let relative_path = relative_path(&request.relative_path)?;
    let (lock_path, lock) = acquire_object_lock(&root, request.family, &relative_path)?;

    let preview = preview_removal_until(request, expires)?;
    if preview.apply_token != apply_token {
        return Err(CacheLifecycleError::Token(
            "the selected object, its retention state or its policy changed; run the preview command again"
                .to_owned(),
        ));
    }
    if !preview.eligible {
        return Err(CacheLifecycleError::Retained(
            preview
                .blockers
                .iter()
                .map(|blocker| blocker.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let selected = SelectedObject::measure(request)?;
    lock.revalidate()
        .map_err(|error| CacheLifecycleError::io("revalidate lifecycle lock", &lock_path, error))?;
    let root = selected.root.clone();
    let relative_path = selected.relative_path.clone();
    selected.remove()?;
    Ok(CacheRemovalResult {
        schema: LIFECYCLE_SCHEMA,
        operation: "remove",
        family: request.family,
        cache_root: root,
        relative_path,
        outcome: "object_removed",
    })
}

#[derive(Debug, Clone, Serialize)]
struct TokenBinding<'a> {
    schema: &'static str,
    operation: &'static str,
    family: CacheFamily,
    cache_root: &'a Path,
    relative_path: &'a str,
    object_kind: &'static str,
    policy: TokenObjectPolicy,
    object: &'a CacheObjectProof,
    blockers: &'a [CacheRemovalBlocker],
    expires_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RetentionReleaseTokenBinding<'a> {
    schema: &'static str,
    operation: &'static str,
    family: CacheFamily,
    cache_root: &'a Path,
    name: &'a str,
    relative_path: &'a str,
    receipt: &'a CacheObjectProof,
    retained_object_count: usize,
    expires_unix_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TokenObjectPolicy {
    RegularFile {
        max_bytes: u64,
    },
    Tree {
        max_entries: usize,
        max_regular_file_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRetentionRecord {
    schema: String,
    name: String,
    family: CacheFamily,
    cache_root: PathBuf,
    objects: Vec<StoredRetainedObject>,
    created_unix_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRetainedObject {
    relative_path: String,
    object_kind: String,
    object: CacheObjectProof,
}

#[derive(Debug)]
struct SelectedObject {
    root: PathBuf,
    relative_path: String,
    path: PathBuf,
    proof: CacheObjectProof,
    snapshot: ObjectSnapshot,
}

#[derive(Debug)]
struct PreparedRetentionRelease {
    preview: CacheRetentionReleasePreview,
    receipt: SelectedObject,
}

#[derive(Debug)]
enum ObjectSnapshot {
    RegularFile {
        identity: FileIdentity,
        sha256: Sha256Digest,
        size: u64,
        max_bytes: u64,
    },
    Tree {
        snapshot: TreeContentCas,
        limits: TreeTraversalLimits,
    },
}

impl SelectedObject {
    fn measure(request: &CacheObjectRequest) -> Result<Self, CacheLifecycleError> {
        let root = selected_root(&request.cache_root)?;
        let relative_path = relative_path(&request.relative_path)?;
        let path = root.join(&relative_path);
        let (proof, snapshot) = match request.kind {
            CacheObjectKind::RegularFile { max_bytes } => {
                if max_bytes == 0 {
                    return Err(CacheLifecycleError::invalid(
                        "regular-file cache object byte limit must be greater than zero",
                    ));
                }
                let (identity, bytes) = measure_regular_file_bounded(&path, max_bytes)
                    .map_err(|error| CacheLifecycleError::io("measure cache file", &path, error))?
                    .ok_or_else(|| CacheLifecycleError::Missing(path.clone()))?;
                let size = u64::try_from(bytes.len())
                    .map_err(|error| CacheLifecycleError::invalid(error.to_string()))?;
                let sha256 = sha256_bytes(&bytes);
                (
                    CacheObjectProof::RegularFile {
                        identity,
                        sha256: sha256.clone(),
                        size,
                    },
                    ObjectSnapshot::RegularFile {
                        identity,
                        sha256,
                        size,
                        max_bytes,
                    },
                )
            }
            kind @ CacheObjectKind::Tree { .. } => {
                let limits = kind.tree_limits()?;
                let snapshot = measure_tree_content_cas_bounded(&path, limits)
                    .map_err(|error| CacheLifecycleError::io("measure cache tree", &path, error))?;
                let regular_file_bytes = snapshot.regular_file_bytes().ok_or_else(|| {
                    CacheLifecycleError::Control(
                        "selected tree reported an invalid regular-file byte total".to_owned(),
                    )
                })?;
                (
                    CacheObjectProof::Tree {
                        payload_sha256: snapshot.payload_digest_excluding(None),
                        snapshot_sha256: snapshot.snapshot_digest(),
                        entry_count: snapshot.entry_count(),
                        regular_file_bytes,
                    },
                    ObjectSnapshot::Tree { snapshot, limits },
                )
            }
        };
        Ok(Self {
            root,
            relative_path,
            path,
            proof,
            snapshot,
        })
    }

    fn remove(self) -> Result<(), CacheLifecycleError> {
        match self.snapshot {
            ObjectSnapshot::RegularFile {
                identity,
                sha256,
                size,
                max_bytes,
            } => remove_regular_file_from_snapshot_nofollow(
                &self.path, identity, &sha256, size, max_bytes,
            )
            .map_err(|error| CacheLifecycleError::io("remove cache file", &self.path, error)),
            ObjectSnapshot::Tree { snapshot, limits } => {
                remove_tree_from_snapshot_nofollow(&self.path, &snapshot, limits).map_err(|error| {
                    CacheLifecycleError::io("remove cache tree", &self.path, error)
                })
            }
        }
    }
}

fn preview_retention_release_until(
    request: &CacheRetentionRelease,
    expires_unix_seconds: u64,
) -> Result<CacheRetentionReleasePreview, CacheLifecycleError> {
    Ok(prepare_retention_release(request, expires_unix_seconds)?.preview)
}

fn prepare_retention_release(
    request: &CacheRetentionRelease,
    expires_unix_seconds: u64,
) -> Result<PreparedRetentionRelease, CacheLifecycleError> {
    let (root, name, relative_path) = retention_receipt_location(request)?;
    let path = root.join(&relative_path);
    let (identity, bytes) = measure_regular_file_bounded(&path, MAX_RETENTION_RECEIPT_BYTES)
        .map_err(|error| CacheLifecycleError::io("read retention receipt", &path, error))?
        .ok_or_else(|| CacheLifecycleError::Missing(path.clone()))?;
    let record = decode_retention_record(&bytes, &path)?;
    validate_retention_binding(&record, request.family, &root, &name)?;
    let size = u64::try_from(bytes.len())
        .map_err(|error| CacheLifecycleError::invalid(error.to_string()))?;
    let sha256 = sha256_bytes(&bytes);
    let proof = CacheObjectProof::RegularFile {
        identity,
        sha256: sha256.clone(),
        size,
    };
    let receipt = SelectedObject {
        root: root.clone(),
        relative_path: relative_path.clone(),
        path,
        proof: proof.clone(),
        snapshot: ObjectSnapshot::RegularFile {
            identity,
            sha256,
            size,
            max_bytes: MAX_RETENTION_RECEIPT_BYTES,
        },
    };
    let apply_token = retention_release_token(
        request.family,
        &root,
        &name,
        &relative_path,
        &proof,
        record.objects.len(),
        expires_unix_seconds,
    )?;
    Ok(PreparedRetentionRelease {
        preview: CacheRetentionReleasePreview {
            schema: LIFECYCLE_SCHEMA,
            operation: "release_preview",
            family: request.family,
            cache_root: root,
            name,
            relative_path,
            receipt: proof,
            retained_object_count: record.objects.len(),
            expires_unix_seconds,
            apply_token,
            recovery: "releasing this receipt removes no cache bytes, but can make its retained objects eligible for a later token-confirmed removal",
        },
        receipt,
    })
}

fn retention_receipt_location(
    request: &CacheRetentionRelease,
) -> Result<(PathBuf, String, String), CacheLifecycleError> {
    let root = selected_root(&request.cache_root)?;
    validate_private_directory_nofollow(&root)
        .map_err(|error| CacheLifecycleError::io("validate private root", &root, error))?;
    let name = retention_name(&request.name)?;
    let relative_path = relative_path(
        &PathBuf::from(CONTROL_DIRECTORY)
            .join(CONTROL_VERSION)
            .join(RETENTION_DIRECTORY)
            .join(request.family.as_str())
            .join(format!("{name}{RETENTION_SUFFIX}")),
    )?;
    Ok((root, name, relative_path))
}

fn preview_removal_until(
    request: &CacheObjectRequest,
    expires_unix_seconds: u64,
) -> Result<CacheRemovalPreview, CacheLifecycleError> {
    let selected = SelectedObject::measure(request)?;
    validate_private_directory_nofollow(&selected.root)
        .map_err(|error| CacheLifecycleError::io("validate private root", &selected.root, error))?;
    let blockers = read_retention_blockers(&selected, request.family)?;
    let eligible = blockers.is_empty();
    let token = removal_token(request, &selected, &blockers, expires_unix_seconds)?;
    Ok(CacheRemovalPreview {
        schema: LIFECYCLE_SCHEMA,
        operation: "remove_preview",
        family: request.family,
        cache_root: selected.root,
        relative_path: selected.relative_path,
        object_kind: request.kind.label(),
        object: selected.proof,
        blockers,
        eligible,
        expires_unix_seconds,
        apply_token: token,
        recovery: "single-object removal is descriptor-relative; no root-wide or recursive parent deletion is performed",
    })
}

fn read_retention_blockers(
    selected: &SelectedObject,
    family: CacheFamily,
) -> Result<Vec<CacheRemovalBlocker>, CacheLifecycleError> {
    let directory = retention_directory(&selected.root, family);
    let records = read_retention_records(&directory, family, &selected.root)?;
    let mut blockers = records
        .into_iter()
        .filter_map(|record| {
            let object = record
                .objects
                .iter()
                .find(|object| object.relative_path == selected.relative_path)?;
            let matches_current_object = object.object_kind == selected_kind(&selected.snapshot)
                && object.object == selected.proof;
            Some(CacheRemovalBlocker {
                name: record.name,
                matches_current_object,
                reason: if matches_current_object {
                    "named_retention_reference"
                } else {
                    "named_retention_reference_has_stale_object_proof"
                },
            })
        })
        .collect::<Vec<_>>();
    blockers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(blockers)
}

const fn selected_kind(snapshot: &ObjectSnapshot) -> &'static str {
    match snapshot {
        ObjectSnapshot::RegularFile { .. } => "regular_file",
        ObjectSnapshot::Tree { .. } => "tree",
    }
}

fn selected_root(root: &Path) -> Result<PathBuf, CacheLifecycleError> {
    Ok(resolve_explicit_root(root.to_owned())?.path)
}

fn relative_path(path: &Path) -> Result<String, CacheLifecycleError> {
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(CacheLifecycleError::invalid(format!(
                "cache object path '{}' must be non-empty, relative and traversal-free",
                path.display()
            )));
        };
        let value = value.to_str().ok_or_else(|| {
            CacheLifecycleError::invalid(format!(
                "cache object path '{}' contains non-UTF-8 components",
                path.display()
            ))
        })?;
        PortableOutputName::new(value)
            .map_err(|error| CacheLifecycleError::invalid(error.to_string()))?;
        components.push(value);
    }
    if components.is_empty() {
        return Err(CacheLifecycleError::invalid(
            "cache object path must contain at least one portable component",
        ));
    }
    Ok(components.join("/"))
}

fn retention_name(name: &str) -> Result<String, CacheLifecycleError> {
    PortableOutputName::new(name)
        .map_err(|error| CacheLifecycleError::invalid(error.to_string()))?;
    if name.ends_with(RETENTION_SUFFIX) {
        return Err(CacheLifecycleError::invalid(
            "retention reference names must not end in '.json'",
        ));
    }
    Ok(name.to_owned())
}

fn control_root(root: &Path) -> PathBuf {
    root.join(CONTROL_DIRECTORY).join(CONTROL_VERSION)
}

fn retention_directory(root: &Path, family: CacheFamily) -> PathBuf {
    control_root(root)
        .join(RETENTION_DIRECTORY)
        .join(family.as_str())
}

fn retention_receipt_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}{RETENTION_SUFFIX}"))
}

fn object_lock_path(root: &Path, family: CacheFamily, relative_path: &str) -> PathBuf {
    let binding = format!("{}\n{}\n{}", root.display(), family.as_str(), relative_path);
    let digest = sha256_bytes(binding.as_bytes());
    control_root(root)
        .join(LOCK_DIRECTORY)
        .join(family.as_str())
        .join(format!("{}.lock", &digest.as_str()[..32]))
}

fn common_selection(
    requests: &[CacheObjectRequest],
) -> Result<(CacheFamily, PathBuf, Vec<String>), CacheLifecycleError> {
    let first = requests.first().ok_or_else(|| {
        CacheLifecycleError::invalid("a retention reference must select at least one cache object")
    })?;
    if requests.len() > MAX_RETENTION_OBJECTS {
        return Err(CacheLifecycleError::invalid(format!(
            "a retention reference cannot select more than {MAX_RETENTION_OBJECTS} cache objects"
        )));
    }
    let family = first.family;
    let root = selected_root(&first.cache_root)?;
    let mut relative_paths = requests
        .iter()
        .map(|request| {
            if request.family != family || selected_root(&request.cache_root)? != root {
                return Err(CacheLifecycleError::invalid(
                    "a retention reference must use one exact cache root and family",
                ));
            }
            relative_path(&request.relative_path)
        })
        .collect::<Result<Vec<_>, _>>()?;
    relative_paths.sort();
    if relative_paths.windows(2).any(|paths| paths[0] == paths[1]) {
        return Err(CacheLifecycleError::invalid(
            "a retention reference cannot select the same cache object more than once",
        ));
    }
    Ok((family, root, relative_paths))
}

#[derive(Debug)]
struct HeldObjectLock {
    path: PathBuf,
    lock: AdvisoryFileLock,
}

#[derive(Clone, Copy)]
enum LeaseMode {
    Read,
    Write,
}

fn acquire_object_locks(
    root: &Path,
    family: CacheFamily,
    relative_paths: &[String],
) -> Result<Vec<HeldObjectLock>, CacheLifecycleError> {
    relative_paths
        .iter()
        .map(|relative_path| {
            let (path, lock) = acquire_object_lock(root, family, relative_path)?;
            Ok(HeldObjectLock { path, lock })
        })
        .collect()
}

fn acquire_lease(
    request: &CacheObjectRequest,
    mode: LeaseMode,
) -> Result<CacheObjectLease, CacheLifecycleError> {
    let root = selected_root(&request.cache_root)?;
    validate_private_directory_nofollow(&root)
        .map_err(|error| CacheLifecycleError::io("validate private root", &root, error))?;
    let relative_path = relative_path(&request.relative_path)?;
    let lock_path = object_lock_path(&root, request.family, &relative_path);
    let lock_parent = lock_path.parent().ok_or_else(|| {
        CacheLifecycleError::invalid("object lock path does not have a parent directory")
    })?;
    ensure_directory_nofollow(lock_parent).map_err(|error| {
        CacheLifecycleError::io("create lifecycle lock directory", lock_parent, error)
    })?;
    validate_private_directory_nofollow(lock_parent).map_err(|error| {
        CacheLifecycleError::io("validate lifecycle lock directory", lock_parent, error)
    })?;
    let lock = match mode {
        LeaseMode::Read => AdvisoryFileLock::acquire_shared(&lock_path),
        LeaseMode::Write => AdvisoryFileLock::acquire(&lock_path),
    }
    .map_err(|error| CacheLifecycleError::io("acquire lifecycle lease", &lock_path, error))?;
    Ok(CacheObjectLease {
        cache_root: root,
        relative_path,
        lock_path,
        lock,
    })
}

fn acquire_leases(
    requests: &[CacheObjectRequest],
    mode: LeaseMode,
) -> Result<CacheObjectLeases, CacheLifecycleError> {
    let requests = normalized_lease_requests(requests)?;
    let mut leases = Vec::with_capacity(requests.len());
    for request in &requests {
        // `leases` is local: any failure drops every earlier lock before the
        // error crosses this boundary, so callers never observe a partial set.
        leases.push(acquire_lease(request, mode)?);
    }
    Ok(CacheObjectLeases { leases })
}

fn normalized_lease_requests(
    requests: &[CacheObjectRequest],
) -> Result<Vec<CacheObjectRequest>, CacheLifecycleError> {
    let first = requests.first().ok_or_else(|| {
        CacheLifecycleError::invalid("a lifecycle lease set must select at least one cache object")
    })?;
    if requests.len() > MAX_RETENTION_OBJECTS {
        return Err(CacheLifecycleError::invalid(format!(
            "a lifecycle lease set cannot select more than {MAX_RETENTION_OBJECTS} cache objects"
        )));
    }
    let family = first.family;
    let root = selected_root(&first.cache_root)?;
    let mut normalized = requests
        .iter()
        .map(|request| {
            if request.family != family || selected_root(&request.cache_root)? != root {
                return Err(CacheLifecycleError::invalid(
                    "a lifecycle lease set must use one exact cache root and family",
                ));
            }
            Ok(CacheObjectRequest {
                family,
                cache_root: root.clone(),
                relative_path: PathBuf::from(relative_path(&request.relative_path)?),
                kind: request.kind,
            })
        })
        .collect::<Result<Vec<_>, CacheLifecycleError>>()?;
    normalized.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if normalized
        .windows(2)
        .any(|objects| objects[0].relative_path == objects[1].relative_path)
    {
        return Err(CacheLifecycleError::invalid(
            "a lifecycle lease set cannot select the same cache object more than once",
        ));
    }
    Ok(normalized)
}

fn revalidate_object_locks(locks: &[HeldObjectLock]) -> Result<(), CacheLifecycleError> {
    for lock in locks {
        lock.lock.revalidate().map_err(|error| {
            CacheLifecycleError::io("revalidate lifecycle lock", &lock.path, error)
        })?;
    }
    Ok(())
}

fn acquire_object_lock(
    root: &Path,
    family: CacheFamily,
    relative_path: &str,
) -> Result<(PathBuf, AdvisoryFileLock), CacheLifecycleError> {
    let lock_path = object_lock_path(root, family, relative_path);
    let lock_parent = lock_path.parent().ok_or_else(|| {
        CacheLifecycleError::invalid("object lock path does not have a parent directory")
    })?;
    ensure_directory_nofollow(lock_parent).map_err(|error| {
        CacheLifecycleError::io("create lifecycle lock directory", lock_parent, error)
    })?;
    validate_private_directory_nofollow(lock_parent).map_err(|error| {
        CacheLifecycleError::io("validate lifecycle lock directory", lock_parent, error)
    })?;
    let lock = AdvisoryFileLock::acquire(&lock_path)
        .map_err(|error| CacheLifecycleError::io("acquire lifecycle lock", &lock_path, error))?;
    Ok((lock_path, lock))
}

fn read_retention_records(
    directory: &Path,
    family: CacheFamily,
    root: &Path,
) -> Result<Vec<StoredRetentionRecord>, CacheLifecycleError> {
    let entries = match directory_entry_names_nofollow_bounded(directory, MAX_RETENTION_RECEIPTS) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CacheLifecycleError::io(
                "list retention receipts",
                directory,
                error,
            ))
        }
    };
    let mut records = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.to_str().is_some_and(is_publication_journal_lock_name) {
            continue;
        }
        let name = retention_name_from_entry(&entry, directory)?;
        let path = retention_receipt_path(directory, &name);
        let (_, bytes) = measure_regular_file_bounded(&path, MAX_RETENTION_RECEIPT_BYTES)
            .map_err(|error| CacheLifecycleError::io("read retention receipt", &path, error))?
            .ok_or_else(|| {
                CacheLifecycleError::Control(format!(
                    "retention receipt '{}' disappeared during the bounded listing",
                    path.display()
                ))
            })?;
        let record = decode_retention_record(&bytes, &path)?;
        validate_retention_binding(&record, family, root, &name)?;
        records.push(record);
    }
    Ok(records)
}

fn retention_name_from_entry(
    entry: &OsString,
    directory: &Path,
) -> Result<String, CacheLifecycleError> {
    let entry = entry.to_str().ok_or_else(|| {
        CacheLifecycleError::Control(format!(
            "retention directory '{}' contains a non-UTF-8 entry",
            directory.display()
        ))
    })?;
    let name = entry.strip_suffix(RETENTION_SUFFIX).ok_or_else(|| {
        CacheLifecycleError::Control(format!(
            "retention directory '{}' contains unsupported control entry '{entry}'",
            directory.display()
        ))
    })?;
    retention_name(name)
}

fn decode_retention_record(
    bytes: &[u8],
    path: &Path,
) -> Result<StoredRetentionRecord, CacheLifecycleError> {
    let record = serde_json::from_slice::<StoredRetentionRecord>(bytes).map_err(|error| {
        CacheLifecycleError::Control(format!(
            "retention receipt '{}' is not a valid strict JSON document: {error}",
            path.display()
        ))
    })?;
    if record.schema != RETENTION_SCHEMA {
        return Err(CacheLifecycleError::Control(format!(
            "retention receipt '{}' has unsupported schema '{}'",
            path.display(),
            record.schema
        )));
    }
    Ok(record)
}

fn validate_retention_binding(
    record: &StoredRetentionRecord,
    family: CacheFamily,
    root: &Path,
    name: &str,
) -> Result<(), CacheLifecycleError> {
    if record.family != family || record.cache_root != root || record.name != name {
        return Err(CacheLifecycleError::Control(format!(
            "retention receipt '{name}' is not bound to its selected root, family and portable object"
        )));
    }
    if record.objects.is_empty() {
        return Err(CacheLifecycleError::Control(format!(
            "retention receipt '{name}' does not retain any object"
        )));
    }
    let mut prior = None;
    for object in &record.objects {
        let normalized = relative_path(Path::new(&object.relative_path))?;
        if object.relative_path != normalized
            || prior
                .as_deref()
                .is_some_and(|prior| prior >= normalized.as_str())
        {
            return Err(CacheLifecycleError::Control(format!(
                "retention receipt '{name}' does not contain strictly sorted unique portable object paths"
            )));
        }
        let proof_kind = match object.object {
            CacheObjectProof::RegularFile { .. } => "regular_file",
            CacheObjectProof::Tree { .. } => "tree",
        };
        if object.object_kind != proof_kind {
            return Err(CacheLifecycleError::Control(format!(
                "retention receipt '{name}' declares object kind '{}' but contains '{proof_kind}' proof",
                object.object_kind
            )));
        }
        prior = Some(normalized);
    }
    Ok(())
}

fn removal_token(
    request: &CacheObjectRequest,
    selected: &SelectedObject,
    blockers: &[CacheRemovalBlocker],
    expires_unix_seconds: u64,
) -> Result<String, CacheLifecycleError> {
    let binding = TokenBinding {
        schema: TOKEN_SCHEMA,
        operation: "remove",
        family: request.family,
        cache_root: &selected.root,
        relative_path: &selected.relative_path,
        object_kind: request.kind.label(),
        policy: request.kind.token_policy(),
        object: &selected.proof,
        blockers,
        expires_unix_seconds,
    };
    let bytes = serde_json::to_vec(&binding).map_err(|error| {
        CacheLifecycleError::Control(format!("cannot encode preview token binding: {error}"))
    })?;
    Ok(format!(
        "{TOKEN_SCHEMA}:{expires_unix_seconds}:{}",
        sha256_bytes(&bytes)
    ))
}

fn retention_release_token(
    family: CacheFamily,
    cache_root: &Path,
    name: &str,
    relative_path: &str,
    receipt: &CacheObjectProof,
    retained_object_count: usize,
    expires_unix_seconds: u64,
) -> Result<String, CacheLifecycleError> {
    let binding = RetentionReleaseTokenBinding {
        schema: RETENTION_RELEASE_TOKEN_SCHEMA,
        operation: "release",
        family,
        cache_root,
        name,
        relative_path,
        receipt,
        retained_object_count,
        expires_unix_seconds,
    };
    let bytes = serde_json::to_vec(&binding).map_err(|error| {
        CacheLifecycleError::Control(format!(
            "cannot encode retention-release token binding: {error}"
        ))
    })?;
    Ok(format!(
        "{RETENTION_RELEASE_TOKEN_SCHEMA}:{expires_unix_seconds}:{}",
        sha256_bytes(&bytes)
    ))
}

fn parse_token_expiry(token: &str) -> Result<u64, CacheLifecycleError> {
    parse_token_expiry_for(token, TOKEN_SCHEMA)
}

fn parse_token_expiry_for(token: &str, schema: &str) -> Result<u64, CacheLifecycleError> {
    let mut parts = token.split(':');
    let token_schema = parts.next();
    let expiry = parts.next();
    let digest = parts.next();
    if token_schema != Some(schema) || parts.next().is_some() {
        return Err(CacheLifecycleError::Token(
            "expected a versioned token emitted by the corresponding preview command".to_owned(),
        ));
    }
    let expiry = expiry
        .ok_or_else(|| CacheLifecycleError::Token("token has no expiry value".to_owned()))?
        .parse::<u64>()
        .map_err(|_| {
            CacheLifecycleError::Token("token expiry is not an unsigned timestamp".to_owned())
        })?;
    Sha256Digest::parse(
        digest
            .ok_or_else(|| CacheLifecycleError::Token("token has no binding digest".to_owned()))?,
    )
    .map_err(|_| CacheLifecycleError::Token("token binding digest is malformed".to_owned()))?;
    Ok(expiry)
}

fn current_unix_seconds() -> Result<u64, CacheLifecycleError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| CacheLifecycleError::Clock(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_read_lease, acquire_read_leases, acquire_write_lease, acquire_write_leases,
        apply_retention_release_at, keep, keep_many, keep_validated, preview_removal_until,
        preview_retention_release_until, CacheLifecycleError, CacheObjectKind, CacheObjectRequest,
        CacheRetentionRelease, PREVIEW_LIFETIME_SECONDS,
    };
    use crate::CacheFamily;
    use std::path::PathBuf;

    fn file_request(root: &std::path::Path, relative: &str) -> CacheObjectRequest {
        CacheObjectRequest {
            family: CacheFamily::Archives,
            cache_root: root.to_path_buf(),
            relative_path: PathBuf::from(relative),
            kind: CacheObjectKind::RegularFile { max_bytes: 1024 },
        }
    }

    fn tree_request(root: &std::path::Path, relative: &str) -> CacheObjectRequest {
        CacheObjectRequest {
            family: CacheFamily::Cargo,
            cache_root: root.to_path_buf(),
            relative_path: PathBuf::from(relative),
            kind: CacheObjectKind::Tree {
                max_entries: 16,
                max_regular_file_bytes: 1024,
            },
        }
    }

    #[test]
    #[cfg(unix)]
    fn preview_apply_removes_only_the_current_exact_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("archive");
        std::fs::write(&entry, b"approved").unwrap();
        let request = file_request(&root, "archive");
        let preview = preview_removal_until(&request, 4_000).unwrap();

        let result = apply_removal_with_now(&request, &preview.apply_token, 3_900).unwrap();

        assert_eq!(result.outcome, "object_removed");
        assert!(!entry.exists());
    }

    #[test]
    #[cfg(unix)]
    fn preview_apply_removes_only_the_selected_tree_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        let target = root.join("cargo/v1/generation");
        let sibling = root.join("cargo/v1/other-generation");
        std::fs::create_dir_all(target.join("vendor")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(target.join("vendor/package"), b"approved").unwrap();
        std::fs::write(target.join("receipt.json"), b"receipt").unwrap();
        std::fs::write(sibling.join("receipt.json"), b"must-survive").unwrap();
        let request = tree_request(&root, "cargo/v1/generation");
        let preview = preview_removal_until(&request, 4_000).unwrap();

        apply_removal_with_now(&request, &preview.apply_token, 3_900).unwrap();

        assert!(!target.exists());
        assert_eq!(
            std::fs::read(sibling.join("receipt.json")).unwrap(),
            b"must-survive"
        );
    }

    #[test]
    #[cfg(unix)]
    fn retention_blocks_removal_until_the_final_named_reference_is_released() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("archive");
        std::fs::write(&entry, b"approved").unwrap();
        let request = file_request(&root, "archive");
        keep(&request, "release-candidate").unwrap();
        let preview = preview_removal_until(&request, 4_000).unwrap();
        assert!(!preview.eligible);
        assert_eq!(preview.blockers.len(), 1);
        assert!(matches!(
            apply_removal_with_now(&request, &preview.apply_token, 3_900),
            Err(CacheLifecycleError::Retained(_))
        ));

        let release = CacheRetentionRelease {
            family: CacheFamily::Archives,
            cache_root: root,
            name: "release-candidate".to_owned(),
        };
        let preview = preview_retention_release_until(&release, 4_000).unwrap();
        apply_retention_release_at(&release, &preview.apply_token, 3_900).unwrap();
        let preview = preview_removal_until(&request, 4_000).unwrap();
        apply_removal_with_now(&request, &preview.apply_token, 3_900).unwrap();
        assert!(!entry.exists());
    }

    #[test]
    #[cfg(unix)]
    fn retention_release_requires_an_unchanged_preview_token() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("archive"), b"approved").unwrap();
        let request = file_request(&root, "archive");
        keep(&request, "release-candidate").unwrap();
        let release = CacheRetentionRelease {
            family: CacheFamily::Archives,
            cache_root: root.clone(),
            name: "release-candidate".to_owned(),
        };
        let preview = preview_retention_release_until(&release, 4_000).unwrap();
        assert!(
            !preview_removal_until(&request, 4_000).unwrap().eligible,
            "a retention-release preview must not weaken the live reference"
        );
        let receipt =
            root.join(".aros-cache-lifecycle/v1/retention/archives/release-candidate.json");
        let bytes = std::fs::read(&receipt).unwrap();
        std::fs::remove_file(&receipt).unwrap();
        std::fs::write(&receipt, bytes).unwrap();

        assert!(matches!(
            apply_retention_release_at(&release, &preview.apply_token, 3_900),
            Err(CacheLifecycleError::Token(_))
        ));
        assert!(receipt.is_file());
        assert!(!preview_removal_until(&request, 4_000).unwrap().eligible);
    }

    #[test]
    #[cfg(unix)]
    fn validated_keep_holds_the_object_lock_and_never_retains_a_rejected_object() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("archive"), b"approved").unwrap();
        let request = file_request(&root, "archive");
        let mut validation_ran = false;

        let record = keep_validated(&request, "release-candidate", || {
            validation_ran = true;
            assert!(matches!(
                acquire_write_lease(&request),
                Err(CacheLifecycleError::Io {
                    action: "acquire lifecycle lease",
                    ..
                })
            ));
            Ok(())
        })
        .unwrap();

        assert!(validation_ran);
        assert_eq!(record.name, "release-candidate");
        assert!(matches!(
            keep_validated(&request, "rejected", || Err(CacheLifecycleError::validation("mismatch"))),
            Err(CacheLifecycleError::Validation(message)) if message == "mismatch"
        ));
        assert!(!root
            .join(".aros-cache-lifecycle/v1/retention/archives/rejected.json")
            .exists());
    }

    #[test]
    #[cfg(unix)]
    fn keep_many_creates_one_sorted_atomic_reference_for_the_complete_selection() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha"), b"alpha").unwrap();
        std::fs::write(root.join("beta"), b"beta").unwrap();
        let alpha = file_request(&root, "alpha");
        let beta = file_request(&root, "beta");

        let record = keep_many(&[beta.clone(), alpha.clone()], "release-candidate").unwrap();

        assert_eq!(
            record
                .objects
                .iter()
                .map(|object| object.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        let receipt_directory = root.join(".aros-cache-lifecycle/v1/retention/archives");
        assert_eq!(
            std::fs::read_dir(&receipt_directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
                .count(),
            1
        );
        for request in [&alpha, &beta] {
            let preview = preview_removal_until(request, 4_000).unwrap();
            assert!(!preview.eligible);
            assert_eq!(preview.blockers[0].name, "release-candidate");
        }

        let release = CacheRetentionRelease {
            family: CacheFamily::Archives,
            cache_root: root,
            name: "release-candidate".to_owned(),
        };
        let preview = preview_retention_release_until(&release, 4_000).unwrap();
        apply_retention_release_at(&release, &preview.apply_token, 3_900).unwrap();
        assert!(preview_removal_until(&alpha, 4_000).unwrap().eligible);
        assert!(preview_removal_until(&beta, 4_000).unwrap().eligible);
    }

    #[test]
    #[cfg(unix)]
    fn keep_many_rejects_ambiguous_or_mixed_selections_before_creating_a_receipt() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        let other_root = temporary.path().join("other-cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&other_root).unwrap();
        std::fs::write(root.join("archive"), b"approved").unwrap();
        std::fs::write(other_root.join("archive"), b"approved").unwrap();
        let archive = file_request(&root, "archive");
        let same_archive = file_request(&root, "archive");
        let other_archive = file_request(&other_root, "archive");
        let cargo = CacheObjectRequest {
            family: CacheFamily::Cargo,
            ..archive.clone()
        };

        for selection in [
            vec![archive.clone(), same_archive],
            vec![archive.clone(), other_archive],
            vec![archive, cargo],
        ] {
            assert!(matches!(
                keep_many(&selection, "must-not-exist"),
                Err(CacheLifecycleError::Invalid(_))
            ));
        }
        assert!(!root
            .join(".aros-cache-lifecycle/v1/retention/archives/must-not-exist.json")
            .exists());
    }

    #[test]
    #[cfg(unix)]
    fn stale_or_expired_token_never_removes_the_object() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("archive");
        std::fs::write(&entry, b"approved").unwrap();
        let request = file_request(&root, "archive");
        let preview = preview_removal_until(&request, 4_000).unwrap();
        std::fs::write(&entry, b"changed").unwrap();

        assert!(matches!(
            apply_removal_with_now(&request, &preview.apply_token, 3_900),
            Err(CacheLifecycleError::Token(_))
        ));
        assert_eq!(std::fs::read(&entry).unwrap(), b"changed");

        let expired = preview_removal_until(&request, 4_000).unwrap();
        assert!(matches!(
            apply_removal_with_now(
                &request,
                &expired.apply_token,
                4_000 + PREVIEW_LIFETIME_SECONDS + 1,
            ),
            Err(CacheLifecycleError::Token(_))
        ));
        assert_eq!(std::fs::read(&entry).unwrap(), b"changed");
    }

    #[test]
    #[cfg(unix)]
    fn policy_or_root_safety_change_invalidates_removal_before_mutation() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("archive");
        std::fs::write(&entry, b"approved").unwrap();
        let request = file_request(&root, "archive");
        let preview = preview_removal_until(&request, 4_000).unwrap();
        let widened_policy = CacheObjectRequest {
            kind: CacheObjectKind::RegularFile { max_bytes: 2048 },
            ..request.clone()
        };
        assert!(matches!(
            apply_removal_with_now(&widened_policy, &preview.apply_token, 3_900),
            Err(CacheLifecycleError::Token(_))
        ));
        assert_eq!(std::fs::read(&entry).unwrap(), b"approved");

        let mode = std::fs::metadata(&root).unwrap().mode() & 0o7777;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode | 0o020)).unwrap();
        assert!(matches!(
            preview_removal_until(&request, 4_000),
            Err(CacheLifecycleError::Io { .. })
        ));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(std::fs::read(&entry).unwrap(), b"approved");
    }

    #[test]
    #[cfg(unix)]
    fn keep_uses_the_same_object_lock_as_remove() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("archive"), b"approved").unwrap();
        let request = file_request(&root, "archive");
        let relative = super::relative_path(&request.relative_path).unwrap();
        let (lock_path, lock) =
            super::acquire_object_lock(&root, request.family, &relative).unwrap();

        assert!(matches!(
            keep(&request, "blocked-while-removing"),
            Err(CacheLifecycleError::Io {
                action: "acquire lifecycle lock",
                ..
            })
        ));
        assert!(!root
            .join(".aros-cache-lifecycle/v1/retention/archives/blocked-while-removing.json")
            .exists());
        lock.revalidate().unwrap();
        drop(lock);
        assert!(keep(&request, "blocked-while-removing").is_ok());
        assert!(lock_path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn active_reader_leases_exclude_lifecycle_writers_until_every_reader_releases() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("archive"), b"approved").unwrap();
        let request = file_request(&root, "archive");

        let first = acquire_read_lease(&request).unwrap();
        let second = acquire_read_lease(&request).unwrap();
        first.revalidate().unwrap();
        second.revalidate().unwrap();
        assert!(matches!(
            acquire_write_lease(&request),
            Err(CacheLifecycleError::Io {
                action: "acquire lifecycle lease",
                ..
            })
        ));
        drop(first);
        assert!(acquire_write_lease(&request).is_err());
        drop(second);

        let writer = acquire_write_lease(&request).unwrap();
        writer.revalidate().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn closure_leases_are_ordered_all_or_nothing_and_protect_every_selected_object() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("alpha"), b"approved").unwrap();
        std::fs::write(root.join("beta"), b"approved").unwrap();
        let alpha = file_request(&root, "alpha");
        let beta = file_request(&root, "beta");

        let blocked = acquire_write_lease(&beta).unwrap();
        assert!(acquire_read_leases(&[beta.clone(), alpha.clone()]).is_err());
        // A failed multi-object acquisition must not strand the first,
        // alphabetically acquired lease.
        assert!(acquire_write_lease(&alpha).is_ok());
        drop(blocked);

        let readers = acquire_read_leases(&[beta.clone(), alpha.clone()]).unwrap();
        assert_eq!(readers.len(), 2);
        assert!(!readers.is_empty());
        readers.revalidate().unwrap();
        assert!(acquire_write_leases(&[alpha.clone(), beta.clone()]).is_err());
        drop(readers);
        assert!(acquire_write_leases(&[alpha, beta]).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn traversal_and_unregistered_control_entries_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).unwrap();
        let request = file_request(&root, "../outside");
        assert!(matches!(
            preview_removal_until(&request, 4_000),
            Err(CacheLifecycleError::Invalid(_))
        ));

        let entry = root.join("archive");
        std::fs::write(&entry, b"approved").unwrap();
        let request = file_request(&root, "archive");
        let controls = root.join(".aros-cache-lifecycle/v1/retention/archives");
        std::fs::create_dir_all(&controls).unwrap();
        std::fs::write(controls.join("foreign"), b"not a receipt").unwrap();
        assert!(matches!(
            preview_removal_until(&request, 4_000),
            Err(CacheLifecycleError::Control(_))
        ));
    }

    fn apply_removal_with_now(
        request: &CacheObjectRequest,
        token: &str,
        now: u64,
    ) -> Result<super::CacheRemovalResult, CacheLifecycleError> {
        super::apply_removal_at(request, token, now)
    }
}
