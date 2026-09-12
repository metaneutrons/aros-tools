//! OS-held build leases for managed local toolchain imports.
//!
//! A project lock prevents a selection change during a build. An imported
//! envelope additionally receives its own lock and durable lease receipt, so
//! lifecycle cleanup can distinguish an active build from a stale record
//! without trusting a PID, clock, or expiry value.

use crate::toolchain::{self, ResolvedToolchain, ToolchainSource};
use crate::toolchain_management::{
    acquire_store_lock, managed_import_envelope_for_payload, management_store,
    normalized_absolute_utf8, project_lock_path, publication_error, stable_token,
};
use aros_common::{
    measure_regular_file_bounded, publish_atomic_file, sha256_bytes, AdvisoryFileLock,
    AtomicFilePolicy, CommitState, DiagnosticContext, LogLevel,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LEASE_SCHEMA: &str = "aros-toolchain-build-lease-v1";
const LEASES_DIRECTORY: &str = "leases/v1";
const LEASE_LOCKS_DIRECTORY: &str = "lease-locks/v1";
const ENVELOPE_LOCKS_DIRECTORY: &str = "envelope-locks/v1";
const MAX_LEASE_RECEIPT_BYTES: u64 = 1024 * 1024;
static LEASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A durable record whose authority exists only while its corresponding
/// advisory lock remains held.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BuildLeaseReceipt {
    schema: String,
    lease_id: String,
    project: String,
    project_lock: String,
    project_lock_sha256: Option<String>,
    toolchain_root: String,
    envelope: String,
    managed_id: String,
    release_id_claim: String,
}

/// Build-lifetime guards for the current project and, when applicable, its
/// owned local-import envelope.
///
/// The store lock is released before compilation. The project guard prevents
/// a concurrent project-lock selection while the build is using its resolved
/// input; the envelope and lease locks prevent managed cleanup from treating
/// an active imported payload as stale.
pub struct BuildLease {
    _project: AdvisoryFileLock,
    _envelope: Option<AdvisoryFileLock>,
    _lease: Option<AdvisoryFileLock>,
}

/// Acquire all lifecycle guards needed after a toolchain has been resolved.
///
/// The resolver may download a released asset, so it intentionally runs before
/// this function. Once the project guard is held, a locked release is reread
/// and must still name the resolved release. A changed selection therefore
/// fails before CMake runs rather than silently mixing selection and build
/// state.
pub fn acquire_for_build(repo_root: &Path, resolved: &ResolvedToolchain) -> Result<BuildLease> {
    let store = management_store(None)?;
    acquire_for_build_in_store(repo_root, resolved, &store)
}

fn acquire_for_build_in_store(
    repo_root: &Path,
    resolved: &ResolvedToolchain,
    store: &Path,
) -> Result<BuildLease> {
    let store_guard = acquire_store_lock(store)?;
    let project = normalized_absolute_utf8(repo_root, "project root")?;
    let project_guard_path = project_lock_path(store, &project);
    let project_guard = AdvisoryFileLock::acquire(&project_guard_path)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot acquire build project lock below '{}'",
                project_guard_path.display()
            )
        })?;
    revalidate(&store_guard, &project_guard, None, None)?;

    if resolved.source == ToolchainSource::LockedRelease {
        let lock = toolchain::load_lock(repo_root)?;
        if resolved.release_id.as_deref() != Some(lock.release_id.as_str()) {
            return Err(miette::miette!(
                "project toolchain lock changed while resolving the build input; rerun the build"
            ));
        }
    }

    let imported = match resolved.source {
        ToolchainSource::LocalManifest => {
            managed_import_envelope_for_payload(store, &resolved.paths.root)?
        }
        ToolchainSource::LockedRelease | ToolchainSource::LegacyLocal => None,
    };
    let Some(imported) = imported else {
        drop(store_guard);
        return Ok(BuildLease {
            _project: project_guard,
            _envelope: None,
            _lease: None,
        });
    };

    let envelope = normalized_absolute_utf8(&imported.envelope, "managed import envelope")?;
    let envelope_guard_path = envelope_lock_path(store, &envelope);
    let envelope_guard = AdvisoryFileLock::acquire(&envelope_guard_path)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot acquire managed-import envelope lock below '{}'",
                envelope_guard_path.display()
            )
        })?;
    let project_lock = toolchain::lock_file_path(repo_root);
    let project_lock_sha256 = measured_project_lock_sha256(&project_lock)?;
    let (lease, lease_guard, lease_path) = prepare_lease(LeasePreparation {
        store,
        project: &project,
        project_lock: &project_lock,
        project_lock_sha256,
        resolved,
        envelope: &envelope,
        managed_id: &imported.managed_id,
        release_id_claim: &imported.release_id_claim,
    })?;
    revalidate(
        &store_guard,
        &project_guard,
        Some(&envelope_guard),
        Some(&lease_guard),
    )?;
    if let Err(error) = publish_atomic_file(&lease_path, &lease.1, AtomicFilePolicy::NoClobber) {
        return publication_error(
            error,
            "managed toolchain build-lease publication failed",
            "build-lease publication may have crossed its durable boundary; inspect the exact lease before retrying",
            "managed toolchain build-lease did not cross its durable publication boundary",
        );
    }
    verify_lease_receipt(&lease_path, &lease.1, &lease.0)?;
    revalidate(
        &store_guard,
        &project_guard,
        Some(&envelope_guard),
        Some(&lease_guard),
    )?;
    let context = DiagnosticContext {
        target: Some(resolved.target_triple.clone()),
        output: Some(envelope),
        commit_state: Some(CommitState::Committed),
        ..DiagnosticContext::default()
    };
    crate::observability::log_event(
        LogLevel::Info,
        "toolchain.build_lease.acquired",
        "acquired an OS-held lease for a managed local toolchain import",
        &context,
    )
    .or_else(|error| {
        crate::observability::commit_state(
            Err(error),
            CommitState::Committed,
            "managed build lease was published, but its acquisition event could not be persisted",
        )
    })?;
    drop(store_guard);
    Ok(BuildLease {
        _project: project_guard,
        _envelope: Some(envelope_guard),
        _lease: Some(lease_guard),
    })
}

#[cfg(test)]
pub fn acquire_for_build_in_test_store(
    repo_root: &Path,
    resolved: &ResolvedToolchain,
    store: &Path,
) -> Result<BuildLease> {
    acquire_for_build_in_store(repo_root, resolved, store)
}

fn measured_project_lock_sha256(path: &Path) -> Result<Option<String>> {
    let measured = measure_regular_file_bounded(path, MAX_LEASE_RECEIPT_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely read project lock '{}'", path.display()))?;
    Ok(measured.map(|(_, bytes)| sha256_bytes(&bytes).to_string()))
}

struct LeasePreparation<'a> {
    store: &'a Path,
    project: &'a str,
    project_lock: &'a Path,
    project_lock_sha256: Option<String>,
    resolved: &'a ResolvedToolchain,
    envelope: &'a str,
    managed_id: &'a str,
    release_id_claim: &'a str,
}

fn prepare_lease(
    input: LeasePreparation<'_>,
) -> Result<((BuildLeaseReceipt, Vec<u8>), AdvisoryFileLock, PathBuf)> {
    for _ in 0..16 {
        let lease_id = next_lease_id(input.project, input.envelope, input.managed_id);
        let lease_path = lease_record_path(input.store, &lease_id);
        let lease_guard_path = lease_lock_path(input.store, &lease_id);
        let lease_guard = AdvisoryFileLock::acquire(&lease_guard_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot acquire build-lease lock below '{}'",
                    lease_guard_path.display()
                )
            })?;
        if measure_regular_file_bounded(&lease_path, MAX_LEASE_RECEIPT_BYTES)
            .into_diagnostic()
            .wrap_err_with(|| format!("cannot inspect build lease '{}'", lease_path.display()))?
            .is_some()
        {
            continue;
        }
        let receipt = BuildLeaseReceipt {
            schema: LEASE_SCHEMA.to_owned(),
            lease_id,
            project: input.project.to_owned(),
            project_lock: normalized_absolute_utf8(input.project_lock, "project lock")?,
            project_lock_sha256: input.project_lock_sha256,
            toolchain_root: normalized_absolute_utf8(
                &input.resolved.paths.root,
                "resolved toolchain root",
            )?,
            envelope: input.envelope.to_owned(),
            managed_id: input.managed_id.to_owned(),
            release_id_claim: input.release_id_claim.to_owned(),
        };
        let bytes = serde_json::to_vec_pretty(&receipt)
            .into_diagnostic()
            .wrap_err("cannot serialize managed toolchain build lease")?;
        return Ok(((receipt, bytes), lease_guard, lease_path));
    }
    Err(miette::miette!(
        "could not allocate a unique build-lease identity after 16 attempts"
    ))
}

fn next_lease_id(project: &str, envelope: &str, managed_id: &str) -> String {
    let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed).to_string();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    stable_token(
        "aros-toolchain-build-lease-v1",
        &[
            project,
            envelope,
            managed_id,
            &std::process::id().to_string(),
            &sequence,
            &nanos,
        ],
    )
}

fn envelope_lock_path(store: &Path, envelope: &str) -> PathBuf {
    let id = stable_token("aros-toolchain-envelope-lock-v1", &[envelope]);
    store
        .join(crate::toolchain_management::MANAGEMENT_DIRECTORY)
        .join(ENVELOPE_LOCKS_DIRECTORY)
        .join(format!("{id}.lock"))
}

fn lease_record_path(store: &Path, lease_id: &str) -> PathBuf {
    store
        .join(crate::toolchain_management::MANAGEMENT_DIRECTORY)
        .join(LEASES_DIRECTORY)
        .join(format!("{lease_id}.json"))
}

fn lease_lock_path(store: &Path, lease_id: &str) -> PathBuf {
    store
        .join(crate::toolchain_management::MANAGEMENT_DIRECTORY)
        .join(LEASE_LOCKS_DIRECTORY)
        .join(format!("{lease_id}.lock"))
}

fn revalidate(
    store: &AdvisoryFileLock,
    project: &AdvisoryFileLock,
    envelope: Option<&AdvisoryFileLock>,
    lease: Option<&AdvisoryFileLock>,
) -> Result<()> {
    for (label, lock) in [("toolchain store", store), ("project", project)] {
        lock.revalidate()
            .into_diagnostic()
            .wrap_err_with(|| format!("{label} lifecycle lock could not be revalidated"))?;
    }
    for (label, lock) in [
        ("managed-import envelope", envelope),
        ("build lease", lease),
    ] {
        if let Some(lock) = lock {
            lock.revalidate()
                .into_diagnostic()
                .wrap_err_with(|| format!("{label} lifecycle lock could not be revalidated"))?;
        }
    }
    Ok(())
}

fn verify_lease_receipt(
    path: &Path,
    expected_bytes: &[u8],
    expected: &BuildLeaseReceipt,
) -> Result<()> {
    let Some((_, actual)) = measure_regular_file_bounded(path, MAX_LEASE_RECEIPT_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot reread build lease '{}'", path.display()))?
    else {
        return Err(miette::miette!(
            "build lease '{}' disappeared after publication",
            path.display()
        ));
    };
    if actual != expected_bytes {
        return Err(miette::miette!(
            "build lease '{}' changed during readback",
            path.display()
        ));
    }
    let receipt: BuildLeaseReceipt = serde_json::from_slice(&actual)
        .into_diagnostic()
        .wrap_err_with(|| format!("build lease '{}' is not valid JSON", path.display()))?;
    if &receipt != expected {
        return Err(miette::miette!(
            "build lease '{}' does not match its approved identity",
            path.display()
        ));
    }
    Ok(())
}
