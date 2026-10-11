//! Explicit, bounded selection of one source-owned SDK for an application project.
//!
//! A project selection is a canonical [`SdkReleaseLock`] written to one path
//! supplied by the caller. This module does not discover project roots, infer
//! a compiler or board, or claim that the selected SDK is compatible or
//! authenticated.

#![cfg(unix)]

use std::fs::{self, Metadata};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use aros_common::sdk_release_lock::{SdkReleaseLock, SDK_RELEASE_LOCK_MAX_BYTES};
use aros_common::{
    measure_regular_file_bounded, open_regular_file_nofollow, publish_atomic_file, sha256_bytes,
    validate_existing_directory_prefix_nofollow, AdvisoryFileLock, AtomicFilePolicy, CommitState,
    DiagnosticContext, FileIdentity, PortableOutputName, Sha256Digest,
};
use serde::Serialize;

use crate::sdk_store::{self, SdkInstallation};
use crate::ContractError;

const PROJECT_GUARD_PREFIX: &str = ".aros-sdk-project-";
const PROJECT_GUARD_SUFFIX: &str = ".lock";
const CONFIRMATION_DOMAIN: &[u8] = b"aros-sdk-project-selection-v1\0";
const PROJECT_LOCK_MAX_BYTES: u64 = SDK_RELEASE_LOCK_MAX_BYTES as u64;

/// Explicit inputs for selecting a verified SDK in one project lock file.
#[derive(Debug, Clone)]
pub struct SdkProjectRequest {
    /// Existing source-owned SDK release lock to select from.
    pub candidate_lock: PathBuf,
    /// Independently supplied SHA-256 of the exact candidate file bytes.
    pub candidate_sha256: Sha256Digest,
    /// One explicitly selected lock file owned by the application project.
    pub project_lock: PathBuf,
    /// Existing content-addressed SDK store to verify.
    pub store: PathBuf,
}

/// Read-only preview of the exact SDK lock a project selection would contain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SdkProjectPlan {
    /// The explicit project lock pathname.
    pub project_lock: PathBuf,
    /// Canonical digest of the selected SDK release lock.
    pub sdk_lock_sha256: Sha256Digest,
    /// Source-owned SDK identity read from the selected release lock.
    pub identity: aros_common::sdk_manifest::SdkIdentity,
    /// Source-tree digest recorded by the selected release lock.
    pub tree_sha256: Sha256Digest,
    /// Digest of the prior canonical project lock, if it exists.
    pub previous_lock_sha256: Option<Sha256Digest>,
    /// True when applying this plan would publish new canonical bytes.
    pub will_write: bool,
    /// Deterministic confirmation token binding paths and exact snapshots.
    pub confirmation: Sha256Digest,
}

/// Result of applying or reusing a project SDK selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SdkProjectSelection {
    /// The explicit project lock pathname.
    pub project_lock: PathBuf,
    /// Digest of the canonical SDK release lock stored at `project_lock`.
    pub lock_sha256: Sha256Digest,
    /// Source-owned SDK identity selected for this project.
    pub identity: aros_common::sdk_manifest::SdkIdentity,
    /// Source-tree digest recorded by the selected release lock.
    pub tree_sha256: Sha256Digest,
    /// Full read-only verification result for the installed SDK.
    pub installation: SdkInstallation,
    /// True when the project already held these exact canonical lock bytes.
    pub already_selected: bool,
}

/// Stable bounded snapshot of one canonical SDK lock file in a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkProjectLockSnapshot {
    /// Parsed source-owned SDK release selection.
    pub lock: SdkReleaseLock,
    /// Device and inode measured through a no-follow descriptor.
    pub file_identity: FileIdentity,
    /// Exact canonical bytes read from the project lock.
    pub raw_bytes: Vec<u8>,
    /// SHA-256 of the canonical project-lock bytes.
    pub lock_sha256: Sha256Digest,
    path: PathBuf,
}

impl SdkProjectLockSnapshot {
    /// Re-read this exact path and reject replacement or any byte change.
    ///
    /// # Errors
    /// Rejects an absent, unsafe, malformed, replaced or changed project lock.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        let current = read_project_lock(&self.path)?;
        if current.file_identity != self.file_identity || current.raw_bytes != self.raw_bytes {
            return Err(failure("SDK project lock changed after it was read"));
        }
        Ok(())
    }

    /// The exact project lock path measured by this snapshot.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    identity: FileIdentity,
    bytes: Vec<u8>,
}

struct PreparedPlan {
    public: SdkProjectPlan,
    lock: SdkReleaseLock,
    canonical_lock: Vec<u8>,
    candidate: FileSnapshot,
    previous: Option<FileSnapshot>,
    installation: SdkInstallation,
    already_selected: bool,
}

#[derive(Serialize)]
struct SnapshotBinding {
    identity: FileIdentity,
    size: usize,
    sha256: Sha256Digest,
}

#[derive(Serialize)]
struct ConfirmationClaims<'a> {
    domain: &'static str,
    candidate_path_hex: String,
    project_path_hex: String,
    store_path_hex: String,
    candidate: SnapshotBinding,
    requested_candidate_sha256: &'a Sha256Digest,
    previous: Option<SnapshotBinding>,
    sdk_identity: &'a aros_common::sdk_manifest::SdkIdentity,
    sdk_tree_sha256: &'a Sha256Digest,
}

/// Verify all selected bytes and describe the exact project lock publication.
///
/// This call is read-only. It requires an explicit normalized absolute path for
/// each input, a real existing project-lock parent, and an already installed
/// SDK that passes the store's full content verification.
///
/// # Errors
/// Rejects stale candidate bytes, unsafe paths, malformed prior locks, target
/// or ABI changes, absent or corrupt SDK installations, and unstable inputs.
pub fn preview(request: &SdkProjectRequest) -> Result<SdkProjectPlan, ContractError> {
    Ok(prepare(request)?.public)
}

/// Read one canonical project SDK lock through a bounded no-follow snapshot.
///
/// This is a read-only selector for commands that consume an already written
/// project lock. It does not verify the selected SDK store; callers that need
/// installed bytes must additionally call [`sdk_store::verify`].
///
/// # Errors
/// Rejects non-normalized paths, missing or unsafe files, hard links, invalid
/// lock documents, noncanonical JSON and unstable reads.
pub fn read_project_lock(path: &Path) -> Result<SdkProjectLockSnapshot, ContractError> {
    validate_project_lock_path(path)?;
    let snapshot = read_snapshot(path, PROJECT_LOCK_MAX_BYTES)?;
    let lock = SdkReleaseLock::from_bytes(&snapshot.bytes)
        .map_err(|_| failure("SDK project lock is malformed or invalid"))?;
    let canonical = lock
        .canonical_bytes()
        .map_err(|_| failure("SDK project lock cannot be canonically encoded"))?;
    if canonical != snapshot.bytes {
        return Err(failure("SDK project lock is not canonical JSON"));
    }
    let lock_sha256 = sha256_bytes(&canonical);
    Ok(SdkProjectLockSnapshot {
        lock,
        file_identity: snapshot.identity,
        raw_bytes: snapshot.bytes,
        lock_sha256,
        path: path.to_path_buf(),
    })
}

/// Apply a preview only while every bound path and file snapshot still matches.
///
/// Cooperative writers are serialized by a deterministic sibling guard in
/// the existing project-lock parent. An identical canonical selection is
/// returned without rewriting the project lock.
///
/// # Errors
/// Rejects stale confirmation tokens and changed inputs before publication.
/// Publication errors report their commit state. Once publication succeeds,
/// the project lock is retained even if later read-back or verification fails.
pub fn apply(
    request: &SdkProjectRequest,
    confirmation: &Sha256Digest,
) -> Result<SdkProjectSelection, ContractError> {
    let initial = prepare(request)?;
    if &initial.public.confirmation != confirmation {
        return Err(committed_state_failure(
            "SDK project confirmation token is stale",
            CommitState::RolledBack,
        ));
    }
    if initial.already_selected {
        let final_plan = prepare(request)?;
        if &final_plan.public.confirmation != confirmation {
            return Err(committed_state_failure(
                "SDK project inputs changed after preview",
                CommitState::RolledBack,
            ));
        }
        return Ok(selection_from(final_plan, true));
    }

    let guard_path = guard_path(&request.project_lock);
    if guard_path == request.project_lock || guard_path == request.candidate_lock {
        return Err(failure(
            "SDK project writer guard collides with a selected input",
        ));
    }
    let guard = AdvisoryFileLock::acquire(&guard_path)
        .map_err(|_| failure("cannot acquire the SDK project writer guard"))?;
    guard
        .revalidate()
        .map_err(|_| failure("SDK project writer guard changed"))?;

    let prepared = prepare(request)?;
    if &prepared.public.confirmation != confirmation {
        return Err(committed_state_failure(
            "SDK project inputs changed after preview",
            CommitState::RolledBack,
        ));
    }
    if prepared.already_selected {
        return Ok(selection_from(prepared, true));
    }

    revalidate_snapshots(request, &prepared)?;
    let verified = sdk_store::verify(&request.store, &prepared.lock)?;
    if verified.lock_sha256 != prepared.public.sdk_lock_sha256
        || verified.manifest.identity != prepared.lock.identity
        || verified.manifest.tree_sha256 != prepared.lock.tree_sha256
    {
        return Err(failure(
            "verified SDK identity changed before project publication",
        ));
    }
    guard
        .revalidate()
        .map_err(|_| failure("SDK project writer guard changed before publication"))?;
    revalidate_snapshots(request, &prepared)?;

    let policy = prepared
        .previous
        .as_ref()
        .map_or(AtomicFilePolicy::NoClobber, |previous| {
            AtomicFilePolicy::ReplaceIf {
                identity: previous.identity,
                sha256: sha256_bytes(&previous.bytes),
            }
        });
    publish_atomic_file(&request.project_lock, &prepared.canonical_lock, policy).map_err(
        |error| {
            let state = match aros_common::publication_failure_class(&error) {
                aros_common::PublicationFailureClass::CommitStateUncertain
                | aros_common::PublicationFailureClass::RecoveryIncomplete => {
                    CommitState::Indeterminate
                }
                _ => CommitState::RolledBack,
            };
            committed_state_failure(
                "SDK project lock publication failed; inspect its exact path before retrying",
                state,
            )
        },
    )?;

    let readback = read_snapshot(&request.project_lock, PROJECT_LOCK_MAX_BYTES).map_err(|_| {
        committed_state_failure(
            "SDK project lock was published but exact read-back failed",
            CommitState::Indeterminate,
        )
    })?;
    if readback.bytes != prepared.canonical_lock {
        return Err(committed_state_failure(
            "SDK project lock was published but read-back bytes differ",
            CommitState::Indeterminate,
        ));
    }

    guard.revalidate().map_err(|_| {
        committed_state_failure(
            "SDK project lock was published but its writer guard changed",
            CommitState::Committed,
        )
    })?;
    revalidate_candidate(request, &prepared).map_err(|_| {
        committed_state_failure(
            "SDK project lock was published but the candidate changed during application",
            CommitState::Committed,
        )
    })?;
    let installation = sdk_store::verify(&request.store, &prepared.lock).map_err(|_| {
        committed_state_failure(
            "SDK project lock was published but final SDK verification failed",
            CommitState::Committed,
        )
    })?;
    let final_readback =
        read_snapshot(&request.project_lock, PROJECT_LOCK_MAX_BYTES).map_err(|_| {
            committed_state_failure(
                "SDK project lock changed during final SDK verification",
                CommitState::Indeterminate,
            )
        })?;
    if final_readback != readback {
        return Err(committed_state_failure(
            "SDK project lock changed during final SDK verification",
            CommitState::Indeterminate,
        ));
    }

    Ok(SdkProjectSelection {
        project_lock: request.project_lock.clone(),
        lock_sha256: prepared.public.sdk_lock_sha256,
        identity: prepared.lock.identity,
        tree_sha256: prepared.lock.tree_sha256,
        installation,
        already_selected: false,
    })
}

fn prepare(request: &SdkProjectRequest) -> Result<PreparedPlan, ContractError> {
    validate_request_paths(request)?;

    let candidate = read_snapshot(&request.candidate_lock, PROJECT_LOCK_MAX_BYTES)?;
    let observed_candidate_sha256 = sha256_bytes(&candidate.bytes);
    if observed_candidate_sha256 != request.candidate_sha256 {
        return Err(failure(
            "SDK candidate lock does not match its selected SHA-256",
        ));
    }
    let lock = SdkReleaseLock::from_bytes(&candidate.bytes)
        .map_err(|_| failure("SDK candidate lock is malformed or invalid"))?;
    let canonical_lock = lock
        .canonical_bytes()
        .map_err(|_| failure("SDK candidate lock cannot be canonically encoded"))?;
    let sdk_lock_sha256 = sha256_bytes(&canonical_lock);

    let previous = read_snapshot_optional(&request.project_lock, PROJECT_LOCK_MAX_BYTES)?;
    if let Some(snapshot) = &previous {
        let parsed = SdkReleaseLock::from_bytes(&snapshot.bytes)
            .map_err(|_| failure("existing SDK project lock is malformed or invalid"))?;
        let previous_canonical = parsed
            .canonical_bytes()
            .map_err(|_| failure("existing SDK project lock cannot be canonically encoded"))?;
        if previous_canonical != snapshot.bytes {
            return Err(failure("existing SDK project lock is not canonical JSON"));
        }
        if parsed.identity.target != lock.identity.target {
            return Err(failure(
                "existing SDK project lock selects a different target or ABI",
            ));
        }
    }

    let installation = sdk_store::verify(&request.store, &lock)?;
    if installation.lock_sha256 != sdk_lock_sha256
        || installation.manifest.identity != lock.identity
        || installation.manifest.tree_sha256 != lock.tree_sha256
    {
        return Err(failure(
            "installed SDK does not match the selected SDK lock",
        ));
    }

    // Bind a stable snapshot of both lock inputs after the full SDK traversal.
    // This keeps preview read-only while rejecting edits made during its work.
    revalidate_snapshot_value(&request.candidate_lock, &candidate)?;
    if read_snapshot_optional(&request.project_lock, PROJECT_LOCK_MAX_BYTES)? != previous {
        return Err(failure("existing SDK project lock changed during preview"));
    }

    let confirmation = confirmation_token(request, &candidate, previous.as_ref(), &lock)?;
    let already_selected = previous
        .as_ref()
        .is_some_and(|snapshot| snapshot.bytes == canonical_lock);
    let previous_lock_sha256 = previous
        .as_ref()
        .map(|snapshot| sha256_bytes(&snapshot.bytes));
    let public = SdkProjectPlan {
        project_lock: request.project_lock.clone(),
        sdk_lock_sha256,
        identity: lock.identity.clone(),
        tree_sha256: lock.tree_sha256.clone(),
        previous_lock_sha256,
        will_write: !already_selected,
        confirmation,
    };

    Ok(PreparedPlan {
        public,
        lock,
        canonical_lock,
        candidate,
        previous,
        installation,
        already_selected,
    })
}

fn validate_request_paths(request: &SdkProjectRequest) -> Result<(), ContractError> {
    require_normalized_absolute_file(&request.candidate_lock, "candidate lock")?;
    validate_project_lock_path(&request.project_lock)?;
    require_normalized_absolute_directory(&request.store, "SDK store")?;
    if request.candidate_lock == request.project_lock {
        return Err(failure(
            "candidate lock and project lock must be different paths",
        ));
    }
    if request.project_lock.starts_with(&request.store) {
        return Err(failure(
            "project lock must be outside the immutable SDK store namespace",
        ));
    }
    let candidate_parent = request
        .candidate_lock
        .parent()
        .ok_or_else(|| failure("candidate lock has no parent directory"))?;
    validate_existing_directory_prefix_nofollow(candidate_parent)
        .map_err(|_| failure("candidate lock path has an unsafe directory component"))?;

    Ok(())
}

fn validate_project_lock_path(path: &Path) -> Result<(), ContractError> {
    require_normalized_absolute_file(path, "project lock")?;
    let parent = path
        .parent()
        .ok_or_else(|| failure("project lock has no parent directory"))?;
    require_existing_real_directory(parent, "project lock parent")?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| failure("project lock needs a portable UTF-8 file name"))?;
    PortableOutputName::new(leaf)
        .map_err(|_| failure("project lock needs a portable output file name"))?;
    Ok(())
}

fn require_normalized_absolute_file(path: &Path, label: &str) -> Result<(), ContractError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.file_name().is_none()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err(failure(format!(
            "{label} must be an explicit normalized absolute file path"
        )));
    }
    Ok(())
}

fn require_normalized_absolute_directory(path: &Path, label: &str) -> Result<(), ContractError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err(failure(format!(
            "{label} must be an explicit normalized absolute directory path"
        )));
    }
    Ok(())
}

fn require_existing_real_directory(path: &Path, label: &str) -> Result<(), ContractError> {
    validate_existing_directory_prefix_nofollow(path)
        .map_err(|_| failure(format!("{label} contains an unsafe path component")))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| failure(format!("{label} is absent or unreadable")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure(format!("{label} is not a real directory")));
    }
    Ok(())
}

fn read_snapshot(path: &Path, maximum: u64) -> Result<FileSnapshot, ContractError> {
    read_snapshot_optional(path, maximum)?.ok_or_else(|| failure("selected lock file is absent"))
}

fn read_snapshot_optional(
    path: &Path,
    maximum: u64,
) -> Result<Option<FileSnapshot>, ContractError> {
    let measured = measure_regular_file_bounded(path, maximum)
        .map_err(|_| failure("selected lock file is unsafe, unstable or oversized"))?;
    let Some((identity, bytes)) = measured else {
        return Ok(None);
    };

    let file = open_regular_file_nofollow(path)
        .map_err(|_| failure("selected lock file is unsafe or changed during read"))?;
    let before = file
        .metadata()
        .map_err(|_| failure("cannot inspect selected lock file"))?;
    if !before.is_file() || before.nlink() != 1 || !identity_matches_metadata(identity, &before) {
        return Err(failure(
            "selected lock file is hardlinked or changed during read",
        ));
    }
    let after_read = measure_regular_file_bounded(path, maximum)
        .map_err(|_| failure("selected lock file changed during read"))?
        .ok_or_else(|| failure("selected lock file disappeared during read"))?;
    let after = file
        .metadata()
        .map_err(|_| failure("cannot recheck selected lock file"))?;
    if after_read.0 != identity || after_read.1 != bytes || !stable_file_metadata(&before, &after) {
        return Err(failure("selected lock file changed during read"));
    }
    Ok(Some(FileSnapshot { identity, bytes }))
}

fn identity_matches_metadata(identity: FileIdentity, metadata: &Metadata) -> bool {
    identity.device() == metadata.dev() && identity.inode() == metadata.ino()
}

fn stable_file_metadata(before: &Metadata, after: &Metadata) -> bool {
    before.is_file()
        && after.is_file()
        && before.nlink() == 1
        && after.nlink() == 1
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn revalidate_snapshots(
    request: &SdkProjectRequest,
    prepared: &PreparedPlan,
) -> Result<(), ContractError> {
    revalidate_candidate(request, prepared)?;
    let previous_now = read_snapshot_optional(&request.project_lock, PROJECT_LOCK_MAX_BYTES)?;
    if previous_now != prepared.previous {
        return Err(failure(
            "existing SDK project lock changed before publication",
        ));
    }
    Ok(())
}

fn revalidate_candidate(
    request: &SdkProjectRequest,
    prepared: &PreparedPlan,
) -> Result<(), ContractError> {
    let current = read_snapshot(&request.candidate_lock, PROJECT_LOCK_MAX_BYTES)?;
    if current != prepared.candidate || sha256_bytes(&current.bytes) != request.candidate_sha256 {
        return Err(failure("SDK candidate lock changed after preview"));
    }
    Ok(())
}

fn revalidate_snapshot_value(path: &Path, expected: &FileSnapshot) -> Result<(), ContractError> {
    let current = read_snapshot(path, PROJECT_LOCK_MAX_BYTES)?;
    if &current != expected {
        return Err(failure("selected lock file changed during preview"));
    }
    Ok(())
}

fn confirmation_token(
    request: &SdkProjectRequest,
    candidate: &FileSnapshot,
    previous: Option<&FileSnapshot>,
    lock: &SdkReleaseLock,
) -> Result<Sha256Digest, ContractError> {
    let claims = ConfirmationClaims {
        domain: "aros-sdk-project-selection-v1",
        candidate_path_hex: hex(request.candidate_lock.as_os_str().as_bytes()),
        project_path_hex: hex(request.project_lock.as_os_str().as_bytes()),
        store_path_hex: hex(request.store.as_os_str().as_bytes()),
        candidate: snapshot_binding(candidate),
        requested_candidate_sha256: &request.candidate_sha256,
        previous: previous.map(snapshot_binding),
        sdk_identity: &lock.identity,
        sdk_tree_sha256: &lock.tree_sha256,
    };
    let value = serde_json::to_value(claims)
        .map_err(|_| failure("cannot encode SDK project confirmation claims"))?;
    let canonical = crate::canonical::bytes(&value)?;
    let mut token_bytes = CONFIRMATION_DOMAIN.to_vec();
    token_bytes.extend_from_slice(&canonical);
    Ok(sha256_bytes(&token_bytes))
}

fn snapshot_binding(snapshot: &FileSnapshot) -> SnapshotBinding {
    SnapshotBinding {
        identity: snapshot.identity,
        size: snapshot.bytes.len(),
        sha256: sha256_bytes(&snapshot.bytes),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[(byte >> 4) as usize]));
        encoded.push(char::from(DIGITS[(byte & 0x0f) as usize]));
    }
    encoded
}

fn guard_path(project_lock: &Path) -> PathBuf {
    let parent = project_lock
        .parent()
        .expect("validated project lock always has a parent");
    let digest = sha256_bytes(project_lock.as_os_str().as_bytes());
    parent.join(format!(
        "{PROJECT_GUARD_PREFIX}{digest}{PROJECT_GUARD_SUFFIX}"
    ))
}

fn selection_from(prepared: PreparedPlan, already_selected: bool) -> SdkProjectSelection {
    SdkProjectSelection {
        project_lock: prepared.public.project_lock,
        lock_sha256: prepared.public.sdk_lock_sha256,
        identity: prepared.lock.identity,
        tree_sha256: prepared.lock.tree_sha256,
        installation: prepared.installation,
        already_selected,
    }
}

fn failure(message: impl Into<String>) -> ContractError {
    ContractError::state(message).sdk_project_hint()
}

fn committed_state_failure(message: impl Into<String>, state: CommitState) -> ContractError {
    failure(message).context(DiagnosticContext {
        commit_state: Some(state),
        ..DiagnosticContext::default()
    })
}
