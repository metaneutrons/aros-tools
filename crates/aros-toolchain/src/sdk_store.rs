//! Content-addressed, host-independent SDK installation and observation.
//!
//! A selected lock authorizes expected bytes, not publisher identity or a
//! global active SDK. No operation here executes SDK payloads, removes an
//! installation, repairs corruption or selects a project lock.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};

use aros_common::sdk_manifest::SdkManifest;
use aros_common::sdk_release_lock::{SdkReleaseLock, SDK_RELEASE_LOCK_MAX_BYTES};
use aros_common::{
    directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    measure_regular_file_bounded, open_regular_file_nofollow,
    publish_prepared_source_tree_noclobber, validate_existing_directory_prefix_nofollow,
    AdvisoryFileLock, PortableOutputName, Sha256Digest,
};
use serde::Serialize;

use crate::sdk_package::{self, SdkLockedArchiveRequest};
use crate::ContractError;

const STORE_VERSION: &str = "v2";
const STORE_LOCK: &str = ".sdk-store.lock";
const ENVELOPE_LOCK: &str = "lock.json";
const COMPLETE: &str = ".complete";
const COMPLETE_BYTES: &[u8] = b"complete\n";
const PAYLOAD: &str = "sdk";
const MAX_INVENTORY_ENTRIES: usize = 100_000;

/// Explicit selected archive and destination store, independent of the host.
#[derive(Debug, Clone)]
pub struct SdkStoreRequest {
    pub store: PathBuf,
    pub lock: SdkReleaseLock,
    pub archive: PathBuf,
}

/// A completely byte-verified installation, not a compatibility attestation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SdkInstallation {
    pub envelope: PathBuf,
    pub payload: PathBuf,
    pub lock_sha256: Sha256Digest,
    pub manifest: SdkManifest,
    pub already_installed: bool,
}

/// Metadata-only inventory observation. Payload integrity is never inferred.
#[derive(Debug, Clone, Serialize)]
pub struct SdkStoreEntry {
    pub envelope: PathBuf,
    pub payload: PathBuf,
    pub lock: SdkReleaseLock,
    pub lock_sha256: Sha256Digest,
    pub integrity: String,
}

/// An unsafe or incomplete entry retained as an observation, never adopted.
#[derive(Debug, Clone, Serialize)]
pub struct SdkStoreFinding {
    pub path: PathBuf,
    pub reason: String,
}

/// Read-only bounded store metadata, including explicit coverage limits.
#[derive(Debug, Clone, Serialize)]
pub struct SdkStoreInventory {
    pub schema: String,
    pub store: PathBuf,
    pub store_state: String,
    pub entries: Vec<SdkStoreEntry>,
    pub findings: Vec<SdkStoreFinding>,
    pub truncated: bool,
}

/// Derive the complete envelope path from one independent SDK lock.
///
/// # Errors
/// Rejects invalid locks, nonabsolute/ambiguous store paths or a volume root.
pub fn envelope_path(store: &Path, lock: &SdkReleaseLock) -> Result<PathBuf, ContractError> {
    require_store_path(store)?;
    lock.validate().map_err(failure)?;
    Ok(store
        .join(STORE_VERSION)
        .join(&lock.identity.sdk_id)
        .join(&lock.identity.version)
        .join(lock.digest().map_err(failure)?.as_str()))
}

/// Install one verified archive in a fresh complete content-addressed envelope.
///
/// A pre-existing envelope is fully verified, never overwritten or repaired.
/// The operation does not execute payload tools or select a project SDK.
///
/// # Errors
/// Rejects invalid input/archive bytes, unsafe or changed paths, corrupt
/// existing installations, concurrent writers or unsuccessful publication.
pub fn install(request: &SdkStoreRequest) -> Result<SdkInstallation, ContractError> {
    let destination = envelope_path(&request.store, &request.lock)?;
    let archive_request = SdkLockedArchiveRequest {
        archive: request.archive.clone(),
        lock: request.lock.clone(),
    };
    // No store namespace is reserved for an invalid selected archive.
    sdk_package::verify_locked_archive(&archive_request)?;
    ensure_directory_nofollow(&request.store)
        .map_err(|_| failure("SDK store is not a safe real directory"))?;
    let guard = AdvisoryFileLock::acquire(&request.store.join(STORE_LOCK))
        .map_err(|_| failure("cannot acquire the SDK store writer lock"))?;
    guard
        .revalidate()
        .map_err(|_| failure("SDK store lock changed"))?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let existing = verify_envelope(&destination, &request.lock, true)?;
            guard
                .revalidate()
                .map_err(|_| failure("SDK store lock changed"))?;
            return Ok(existing);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(failure("cannot inspect SDK installation destination")),
    }
    let parent = destination
        .parent()
        .ok_or_else(|| failure("SDK envelope has no parent"))?;
    ensure_directory_nofollow(parent).map_err(|_| failure("SDK installation parent is unsafe"))?;
    let stage = tempfile::Builder::new()
        .prefix(".aros-sdk-install-")
        .tempdir_in(parent)
        .map_err(|_| failure("cannot reserve private SDK installation stage"))?;
    sdk_package::extract_locked_archive(&archive_request, &stage.path().join(PAYLOAD))?;
    write_new(
        &stage.path().join(ENVELOPE_LOCK),
        &request.lock.canonical_bytes().map_err(failure)?,
    )?;
    write_new(&stage.path().join(COMPLETE), COMPLETE_BYTES)?;
    verify_envelope(stage.path(), &request.lock, false)?;
    guard
        .revalidate()
        .map_err(|_| failure("SDK store lock changed before publication"))?;
    // The source-name policy preserves valid Unicode SDK payload paths. The
    // envelope's generated control names remain fixed and ASCII.
    publish_prepared_source_tree_noclobber(stage.path(), &destination).map_err(|error| {
        let commit_status = match aros_common::publication_failure_class(&error) {
            aros_common::PublicationFailureClass::CommitStateUncertain
            | aros_common::PublicationFailureClass::RecoveryIncomplete => {
                aros_common::CommitState::Indeterminate
            }
            _ => aros_common::CommitState::RolledBack,
        };
        publication_failure(
            "SDK publication failed; inspect the exact destination before retrying",
            commit_status,
        )
    })?;
    let installed = verify_envelope(&destination, &request.lock, false).map_err(|_| {
        publication_failure(
            "SDK envelope was published but final verification failed",
            aros_common::CommitState::Indeterminate,
        )
    })?;
    guard.revalidate().map_err(|_| {
        publication_failure(
            "SDK store lock changed after publication",
            aros_common::CommitState::Indeterminate,
        )
    })?;
    Ok(installed)
}

/// Fully verify the selected installed SDK without creating or repairing state.
///
/// # Errors
/// Rejects absent or unsafe stores, missing/corrupt control records and any
/// payload byte, inventory or selected-identity mismatch.
pub fn verify(store: &Path, lock: &SdkReleaseLock) -> Result<SdkInstallation, ContractError> {
    let destination = envelope_path(store, lock)?;
    require_existing_directory(store)?;
    // Only an already-installed store has this guard. Refuse absence before
    // calling the lock API so verification cannot create control-plane state.
    read_control(&store.join(STORE_LOCK), 0)?;
    let guard = AdvisoryFileLock::acquire_shared(&store.join(STORE_LOCK))
        .map_err(|_| failure("cannot acquire SDK store read lock"))?;
    let result = verify_envelope(&destination, lock, true)?;
    guard
        .revalidate()
        .map_err(|_| failure("SDK store lock changed during verification"))?;
    Ok(result)
}

/// List metadata in a fixed SDK namespace; never hash or execute the payload.
///
/// Absence is an empty observation, not an instruction to create a store.
/// Malformed entries and truncation remain explicit findings.
///
/// # Errors
/// Rejects an invalid store path or a zero/excessive observation budget.
pub fn inventory(store: &Path, max_entries: usize) -> Result<SdkStoreInventory, ContractError> {
    require_store_path(store)?;
    if !(1..=MAX_INVENTORY_ENTRIES).contains(&max_entries) {
        return Err(failure(
            "SDK inventory budget must be between one and 100000 entries",
        ));
    }
    let mut result = SdkStoreInventory {
        schema: "aros-sdk-store-inventory-v2".into(),
        store: store.into(),
        store_state: "present".into(),
        entries: Vec::new(),
        findings: Vec::new(),
        truncated: false,
    };
    match fs::symlink_metadata(store) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            result.store_state = "absent".into();
            return Ok(result);
        }
        _ => {}
    }
    if require_existing_directory(store).is_err() {
        result.store_state = "unsafe".into();
        finding(&mut result, store, "store is not a safe real directory");
        return Ok(result);
    }
    let mut budget = max_entries;
    let root_names = observation_names(store, &mut budget, &mut result);
    for name in root_names {
        if name == std::ffi::OsStr::new(STORE_LOCK) {
            if read_control(&store.join(&name), 0).is_err() {
                finding(&mut result, &store.join(&name), "unsafe store lock");
            }
        } else if name == std::ffi::OsStr::new(STORE_VERSION) {
            observe_level(&store.join(&name), 0, &mut budget, &mut result);
        } else {
            finding(
                &mut result,
                &store.join(&name),
                "unknown SDK store namespace entry",
            );
        }
    }
    Ok(result)
}

fn observe_level(path: &Path, depth: usize, budget: &mut usize, result: &mut SdkStoreInventory) {
    if result.truncated {
        return;
    }
    if require_existing_directory(path).is_err() {
        finding(
            result,
            path,
            "namespace component is not a safe real directory",
        );
        return;
    }
    if depth == 3 {
        match inspect_envelope_metadata(path) {
            Ok(entry) => {
                let expected = envelope_path(&result.store, &entry.lock);
                if expected.as_ref().is_ok_and(|expected| expected == path) {
                    result.entries.push(entry);
                } else {
                    finding(
                        result,
                        path,
                        "SDK envelope path disagrees with its lock identity",
                    );
                }
            }
            Err(error) => finding(result, path, &error.to_string()),
        }
        return;
    }
    let names = observation_names(path, budget, result);
    // Durable no-clobber publication leaves an empty persistent lock beside
    // each envelope. Recognize only locks derived from actual digest leaves,
    // not arbitrary files that happen to resemble a publication lock.
    let publication_locks: BTreeSet<_> = if depth == 2 {
        names
            .iter()
            .filter_map(|name| {
                Sha256Digest::parse(name.to_str()?).ok()?;
                let journal =
                    aros_common::publication_journal_path(&path.join(name), "prepared-tree")
                        .ok()?;
                aros_common::publication_journal_lock_path(&journal).ok()
            })
            .collect()
    } else {
        BTreeSet::new()
    };
    for name in names {
        let child = path.join(&name);
        if publication_locks.contains(&child) {
            if read_control(&child, 0).is_err() {
                finding(result, &child, "unsafe SDK publication lock");
            }
            continue;
        }
        if name
            .to_str()
            .is_none_or(|name| PortableOutputName::new(name).is_err())
        {
            finding(result, &child, "unsafe SDK namespace component");
            continue;
        }
        observe_level(&child, depth + 1, budget, result);
        if result.truncated {
            break;
        }
    }
}

fn observation_names(
    path: &Path,
    budget: &mut usize,
    result: &mut SdkStoreInventory,
) -> Vec<std::ffi::OsString> {
    if *budget == 0 {
        result.truncated = true;
        finding(result, path, "SDK inventory traversal budget exhausted");
        return Vec::new();
    }
    match directory_entry_names_nofollow_bounded(path, *budget) {
        Ok(names) => {
            *budget -= names.len();
            names
        }
        Err(error) => {
            // A bounded enumeration may fail without returning any prefix.
            // Never silently call that complete coverage.
            result.truncated = true;
            finding(
                result,
                path,
                &format!("SDK inventory enumeration incomplete: {error}"),
            );
            Vec::new()
        }
    }
}

fn inspect_envelope_metadata(envelope: &Path) -> Result<SdkStoreEntry, ContractError> {
    require_existing_directory(envelope)?;
    let before = directory_identity(envelope)?;
    let names = directory_entry_names_nofollow_bounded(envelope, 3)
        .map_err(|_| failure("SDK envelope does not have the exact control/payload set"))?;
    let expected: BTreeSet<_> = [COMPLETE, ENVELOPE_LOCK, PAYLOAD]
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect();
    if names.into_iter().collect::<BTreeSet<_>>() != expected {
        return Err(failure("SDK envelope member set is not closed"));
    }
    let bytes = read_control(
        &envelope.join(ENVELOPE_LOCK),
        SDK_RELEASE_LOCK_MAX_BYTES as u64,
    )?;
    let lock = SdkReleaseLock::from_bytes(&bytes).map_err(failure)?;
    if lock.canonical_bytes().map_err(failure)? != bytes {
        return Err(failure("stored SDK lock is not canonical"));
    }
    if read_control(&envelope.join(COMPLETE), COMPLETE_BYTES.len() as u64)? != COMPLETE_BYTES {
        return Err(failure("SDK completion marker is invalid"));
    }
    let payload = envelope.join(PAYLOAD);
    require_existing_directory(&payload)?;
    let payload_before = directory_identity(&payload)?;
    // Metadata inventory checks this one small file, not payload contents.
    let manifest_bytes = read_control(
        &payload.join(aros_common::sdk_manifest::SDK_MANIFEST_FILE),
        lock.manifest.size,
    )?;
    if manifest_bytes.len() as u64 != lock.manifest.size
        || aros_common::sha256_bytes(&manifest_bytes) != lock.manifest.sha256
    {
        return Err(failure(
            "installed SDK manifest differs from its stored lock",
        ));
    }
    let manifest = SdkManifest::from_bytes(&manifest_bytes).map_err(failure)?;
    let canonical_manifest = sdk_package::bounded_pretty_manifest(&manifest, false)?;
    if canonical_manifest != manifest_bytes {
        return Err(failure("installed SDK manifest is not canonical"));
    }
    if manifest.identity != lock.identity {
        return Err(failure(
            "installed SDK manifest identity disagrees with its stored lock",
        ));
    }
    if manifest.tree_sha256 != lock.tree_sha256 {
        return Err(failure(
            "installed SDK manifest tree digest disagrees with its stored lock",
        ));
    }
    if read_control(
        &envelope.join(ENVELOPE_LOCK),
        SDK_RELEASE_LOCK_MAX_BYTES as u64,
    )? != bytes
        || read_control(&envelope.join(COMPLETE), COMPLETE_BYTES.len() as u64)? != COMPLETE_BYTES
        || read_control(
            &payload.join(aros_common::sdk_manifest::SDK_MANIFEST_FILE),
            lock.manifest.size,
        )? != manifest_bytes
        || directory_identity(envelope)? != before
        || directory_identity(&payload)? != payload_before
    {
        return Err(failure(
            "SDK control records changed during metadata observation",
        ));
    }
    Ok(SdkStoreEntry {
        envelope: envelope.into(),
        payload,
        lock_sha256: lock.digest().map_err(failure)?,
        lock,
        integrity: "not-checked".into(),
    })
}

fn verify_envelope(
    envelope: &Path,
    lock: &SdkReleaseLock,
    already_installed: bool,
) -> Result<SdkInstallation, ContractError> {
    let envelope_identity = directory_identity(envelope)?;
    let payload_identity = directory_identity(&envelope.join(PAYLOAD))?;
    let before = inspect_envelope_metadata(envelope)?;
    if &before.lock != lock {
        return Err(failure(
            "installed SDK lock differs from the independent selection",
        ));
    }
    let manifest = sdk_package::verify_installed_sdk(&before.payload, lock)?;
    let after = inspect_envelope_metadata(envelope)?;
    if before.lock != after.lock
        || before.lock_sha256 != after.lock_sha256
        || directory_identity(envelope)? != envelope_identity
        || directory_identity(&before.payload)? != payload_identity
    {
        return Err(failure(
            "SDK envelope changed during full integrity verification",
        ));
    }
    Ok(SdkInstallation {
        envelope: envelope.into(),
        payload: before.payload,
        lock_sha256: before.lock_sha256,
        manifest,
        already_installed,
    })
}

fn require_store_path(path: &Path) -> Result<(), ContractError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
    {
        return Err(failure(
            "SDK store must be an explicit normalized absolute non-root directory",
        ));
    }
    Ok(())
}

fn require_existing_directory(path: &Path) -> Result<(), ContractError> {
    validate_existing_directory_prefix_nofollow(path)
        .map_err(|_| failure("SDK directory path is unsafe"))?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| failure("SDK directory is absent or unreadable"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure("SDK directory is not real"));
    }
    Ok(())
}

fn directory_identity(path: &Path) -> Result<(u64, u64), ContractError> {
    require_existing_directory(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| failure("cannot inspect SDK directory"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure("SDK directory is not real"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

fn read_control(path: &Path, limit: u64) -> Result<Vec<u8>, ContractError> {
    let file =
        open_regular_file_nofollow(path).map_err(|_| failure("SDK control file is unsafe"))?;
    let before = file
        .metadata()
        .map_err(|_| failure("cannot inspect SDK control file"))?;
    if before.nlink() != 1 {
        return Err(failure("SDK control file is hardlinked"));
    }
    let (identity, bytes) = measure_regular_file_bounded(path, limit)
        .map_err(|_| failure("SDK control file changed or exceeded its byte limit"))?
        .ok_or_else(|| failure("SDK control file is absent"))?;
    let after = file
        .metadata()
        .map_err(|_| failure("cannot recheck SDK control file"))?;
    if (identity.device(), identity.inode()) != (before.dev(), before.ino())
        || before.nlink() != after.nlink()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(failure("SDK control file changed during read-back"));
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc_o_nofollow())
        .open(path)
        .map_err(|_| failure("cannot create new SDK control file"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| failure("cannot durably write SDK control file"))
}

const fn libc_o_nofollow() -> i32 {
    rustix::fs::OFlags::NOFOLLOW.bits().cast_signed()
}

fn finding(result: &mut SdkStoreInventory, path: &Path, reason: &str) {
    result.findings.push(SdkStoreFinding {
        path: path.into(),
        reason: reason.into(),
    });
}

fn failure(message: impl Into<String>) -> ContractError {
    ContractError::state(message).sdk_store_hint()
}

fn publication_failure(message: &str, state: aros_common::CommitState) -> ContractError {
    failure(message).context(aros_common::DiagnosticContext {
        commit_state: Some(state),
        ..aros_common::DiagnosticContext::default()
    })
}
