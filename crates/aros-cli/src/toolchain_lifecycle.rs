//! OS-held build leases for managed local toolchain imports.
//!
//! A released project lock prevents a selection change during a build. An
//! imported envelope additionally receives its own lock and durable lease
//! receipt, so lifecycle cleanup can distinguish an active build from a stale
//! record without trusting a PID, clock, or expiry value. Explicit local
//! prefixes have neither selection state nor cleanup authority.

use crate::toolchain::{self, ResolvedToolchain, ToolchainSource};
use crate::toolchain_management::{
    acquire_store_lock, ensure_project_reference_for_locked_build, is_lower_sha256,
    managed_import_envelope_for_payload, managed_import_limits, management_store,
    normalized_absolute_utf8, project_lock_path, project_reference_path, publication_error,
    read_project_reference_unbound, read_registration, safe_segment, stable_token,
    MANAGED_IMPORTS_DIRECTORY, MANAGED_PAYLOAD_DIRECTORY, MANAGEMENT_DIRECTORY,
    PROJECT_LOCKS_DIRECTORY, PROJECT_REFERENCES_DIRECTORY, REGISTRATIONS_DIRECTORY,
};
use aros_common::{
    directory_entry_names_nofollow_bounded, is_publication_journal_lock_name,
    measure_regular_file_bounded, measure_tree_content_cas_bounded, probe_advisory_file_lock,
    publication_journal_lock_path, publication_journal_path, publish_atomic_file,
    remove_tree_from_snapshot_nofollow, sha256_bytes, AdvisoryFileLock, AdvisoryLockState,
    ArosToolchainLock, AtomicFilePolicy, CommitState, DiagnosticContext, FileIdentity, LogLevel,
    TreeContentCas,
};
use clap::Args;
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LEASE_SCHEMA: &str = "aros-toolchain-build-lease-v1";
const LEASES_DIRECTORY: &str = "leases/v1";
const LEASE_LOCKS_DIRECTORY: &str = "lease-locks/v1";
const ENVELOPE_LOCKS_DIRECTORY: &str = "envelope-locks/v1";
const REMOVALS_DIRECTORY: &str = "removals/v1";
const MAX_LEASE_RECEIPT_BYTES: u64 = 1024 * 1024;
const MAX_CONTROL_ENTRIES: usize = 10_000;
const MAX_RELEASE_LOCK_BYTES: u64 = 4 * 1024 * 1024;
const LIFECYCLE_SCHEMA: &str = "aros-toolchain-lifecycle-v1";
const REMOVAL_SCHEMA: &str = "aros-toolchain-removal-v1";
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
    lease_lock: String,
    lease_lock_identity: FileIdentity,
    toolchain_root: String,
    envelope: String,
    managed_id: String,
    release_id_claim: String,
}

/// Build-lifetime guards for a released project selection and, when
/// applicable, its owned local-import envelope.
///
/// The store lock is released before compilation. The project guard prevents
/// a concurrent project-lock selection while the build is using its resolved
/// input; the envelope and lease locks prevent managed cleanup from treating
/// an active imported payload as stale.
pub struct BuildLease {
    _project: Option<AdvisoryFileLock>,
    _envelope: Option<AdvisoryFileLock>,
    _lease: Option<AdvisoryFileLock>,
}

/// Acquire all lifecycle guards needed after a toolchain has been resolved.
///
/// The resolver may download a released asset, so it intentionally runs before
/// this function. For a locked release, the project guard is held, the release
/// is reread and a matching derived reference is ensured before compilation.
/// A changed or partially recorded selection therefore fails before CMake runs
/// rather than silently mixing selection and build state.
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
    let project_guard = if resolved.source == ToolchainSource::LockedRelease {
        let project_guard_path = project_lock_path(store, &project);
        let project_guard = AdvisoryFileLock::acquire(&project_guard_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot acquire build project lock below '{}'",
                    project_guard_path.display()
                )
            })?;
        revalidate(&store_guard, Some(&project_guard), None, None)?;
        let lock = toolchain::load_lock(repo_root)?;
        if resolved.release_id.as_deref() != Some(lock.release_id.as_str()) {
            return Err(miette::miette!(
                "project toolchain lock changed while resolving the build input; rerun the build"
            ));
        }
        let project_lock = toolchain::lock_file_path(repo_root);
        let lock_sha256 = measured_project_lock_sha256(&project_lock)?.ok_or_else(|| {
            miette::miette!(
                "project toolchain lock disappeared while acquiring the build lifecycle guard"
            )
        })?;
        ensure_project_reference_for_locked_build(
            store,
            &project,
            &project_lock,
            &lock.release_id,
            &lock_sha256,
        )?;
        revalidate(&store_guard, Some(&project_guard), None, None)?;
        Some(project_guard)
    } else {
        None
    };

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
        project_guard.as_ref(),
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
        project_guard.as_ref(),
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
        let lease_lock_identity = lease_guard.identity().into_diagnostic().wrap_err_with(|| {
            format!(
                "cannot bind build-lease lock '{}',",
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
            lease_lock: normalized_absolute_utf8(&lease_guard_path, "build lease lock")?,
            lease_lock_identity,
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
    project: Option<&AdvisoryFileLock>,
    envelope: Option<&AdvisoryFileLock>,
    lease: Option<&AdvisoryFileLock>,
) -> Result<()> {
    store
        .revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lifecycle lock could not be revalidated")?;
    if let Some(project) = project {
        project
            .revalidate()
            .into_diagnostic()
            .wrap_err("project lifecycle lock could not be revalidated")?;
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

/// Arguments for one exact owned-import removal. Without `--apply`, this is a
/// non-persistent, byte-scoped preview.
#[derive(Args)]
pub struct RemoveArgs {
    /// Exact managed-import identity reported by `aros toolchain inventory`
    #[arg(long, value_name = "SHA256")]
    managed_id: String,

    /// Explicit absolute toolchain-store root; defaults to AROS_CROSS_TOOLCHAINS_DIR or AROS_HOME
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Apply only the exact preview identified by this token
    #[arg(long, value_name = "TOKEN")]
    apply: Option<String>,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: crate::toolchain_management::ResultFormat,
}

/// Arguments for conservative reclamation of currently unprotected owned
/// imports. Without `--apply`, this is a non-persistent preview.
#[derive(Args)]
pub struct GcArgs {
    /// Explicit absolute toolchain-store root; defaults to AROS_CROSS_TOOLCHAINS_DIR or AROS_HOME
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Apply only the exact preview identified by this token
    #[arg(long, value_name = "TOKEN")]
    apply: Option<String>,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: crate::toolchain_management::ResultFormat,
}

#[derive(Clone)]
enum LifecycleOperation {
    Remove(String),
    Gc,
}

impl LifecycleOperation {
    const fn name(&self) -> &'static str {
        match self {
            Self::Remove(_) => "remove",
            Self::Gc => "gc",
        }
    }

    fn selected<'a>(&self, plan: &'a LifecyclePlan) -> Vec<&'a ManagedCandidate> {
        match self {
            Self::Remove(managed_id) => plan
                .candidates
                .iter()
                .filter(|candidate| candidate.managed_id == *managed_id)
                .collect(),
            Self::Gc => plan.candidates.iter().collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct LifecycleResult {
    schema: &'static str,
    operation: &'static str,
    state: &'static str,
    store: String,
    candidates: Vec<LifecycleCandidateResult>,
    apply_token: Option<String>,
    note: &'static str,
}

#[derive(Debug, Serialize)]
struct LifecycleCandidateResult {
    managed_id: String,
    release_id_claim: String,
    envelope: String,
    snapshot_sha256: String,
    entries: usize,
    regular_file_bytes: u64,
    status: &'static str,
    blockers: Vec<String>,
}

#[derive(Clone)]
struct ManagedCandidate {
    managed_id: String,
    release_id_claim: String,
    envelope: PathBuf,
    envelope_text: String,
    payload_text: String,
    snapshot: TreeContentCas,
    snapshot_sha256: String,
    entries: usize,
    regular_file_bytes: u64,
    blockers: Vec<String>,
}

#[derive(Clone)]
struct ProjectReferenceState {
    path: PathBuf,
    identity: FileIdentity,
    sha256: String,
    project: String,
    project_lock: PathBuf,
    project_lock_identity: FileIdentity,
    project_lock_sha256: String,
    release_id: String,
}

#[derive(Clone)]
struct RegistrationState {
    path: PathBuf,
    identity: FileIdentity,
    sha256: String,
    source: String,
}

#[derive(Clone)]
struct LeaseState {
    path: PathBuf,
    identity: FileIdentity,
    sha256: String,
    lease_id: String,
    lock_path: PathBuf,
    lock_identity: FileIdentity,
    project: String,
    envelope: String,
    state: AdvisoryLockState,
}

struct LifecyclePlan {
    store: PathBuf,
    candidates: Vec<ManagedCandidate>,
    project_references: Vec<ProjectReferenceState>,
    registrations: Vec<RegistrationState>,
    leases: Vec<LeaseState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RemovalJournal {
    schema: String,
    journal_id: String,
    operation: String,
    managed_id: String,
    envelope: String,
    release_id_claim: String,
    snapshot_sha256: String,
    entries: usize,
    regular_file_bytes: u64,
}

struct LifecycleScope {
    store: AdvisoryFileLock,
    projects: Vec<AdvisoryFileLock>,
}

/// Preview or remove one exact managed import.
pub fn remove(args: RemoveArgs) -> Result<()> {
    if !is_lower_sha256(&args.managed_id) {
        return Err(miette::miette!(
            "--managed-id must be one lowercase SHA-256 identity"
        ));
    }
    let store = management_store(args.store)?;
    let operation = LifecycleOperation::Remove(args.managed_id);
    run_lifecycle(&store, &operation, args.apply, args.format)
}

/// Preview or reclaim every currently unprotected managed import.
pub fn gc(args: GcArgs) -> Result<()> {
    let store = management_store(args.store)?;
    run_lifecycle(&store, &LifecycleOperation::Gc, args.apply, args.format)
}

fn run_lifecycle(
    store: &Path,
    operation: &LifecycleOperation,
    provided_token: Option<String>,
    format: crate::toolchain_management::ResultFormat,
) -> Result<()> {
    let plan = inspect_lifecycle(store, operation)?;
    let token = lifecycle_apply_token(&plan, operation);
    let result = if let Some(provided) = provided_token {
        if provided != token {
            return Err(miette::miette!(
                "{} apply token does not match the current managed-store snapshot; rerun the preview",
                operation.name()
            ));
        }
        apply_lifecycle(store, operation, &provided)?
    } else {
        lifecycle_result(
            &plan,
            operation,
            "preview",
            Some(token),
            &BTreeSet::new(),
            "validated ownership, project references, registrations, leases, and exact byte scope; no managed envelope was removed",
        )
    };
    print_lifecycle_result(&result, format);
    Ok(())
}

fn apply_lifecycle(
    store: &Path,
    operation: &LifecycleOperation,
    provided_token: &str,
) -> Result<LifecycleResult> {
    let scope = LifecycleScope::acquire(store)?;
    let plan = inspect_lifecycle(store, operation)?;
    if lifecycle_apply_token(&plan, operation) != provided_token {
        return Err(miette::miette!(
            "{} inputs changed after preview; no managed envelope was removed",
            operation.name()
        ));
    }
    scope.revalidate()?;

    let selected = operation.selected(&plan);
    if matches!(operation, LifecycleOperation::Remove(_)) && selected.is_empty() {
        return Err(miette::miette!(
            "managed import requested for removal no longer exists"
        ));
    }
    if matches!(operation, LifecycleOperation::Remove(_)) && selected.len() != 1 {
        return Err(miette::miette!(
            "managed import identity is ambiguous; no managed envelope was removed"
        ));
    }

    let mut removed = BTreeSet::new();
    for candidate in selected {
        if !candidate.blockers.is_empty() {
            if matches!(operation, LifecycleOperation::Remove(_)) {
                return Err(miette::miette!(
                    "managed import '{}' is not removable: {}",
                    candidate.managed_id,
                    candidate.blockers.join("; ")
                ));
            }
            continue;
        }
        let envelope_guard_path = envelope_lock_path(store, &candidate.envelope_text);
        let envelope_guard = AdvisoryFileLock::acquire(&envelope_guard_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot acquire managed-import envelope lock below '{}'",
                    envelope_guard_path.display()
                )
            })?;
        scope.revalidate()?;
        validate_controls_unchanged(store, &plan, &scope)?;
        envelope_guard
            .revalidate()
            .into_diagnostic()
            .wrap_err("managed-import envelope lock could not be revalidated")?;
        remove_candidate(store, candidate)?;
        removed.insert(candidate.managed_id.clone());
        drop(envelope_guard);
    }
    let note = if removed.is_empty() {
        "no eligible managed imports were found; retained candidates remain protected by project references, external registrations, or live leases"
    } else {
        "removed only owned import envelopes whose preview token, control-plane receipts, OS-held locks, and exact filesystem snapshots remained valid"
    };
    Ok(lifecycle_result(
        &plan,
        operation,
        "committed",
        None,
        &removed,
        note,
    ))
}

impl LifecycleScope {
    fn acquire(store: &Path) -> Result<Self> {
        let store_guard = acquire_store_lock(store)?;
        let before = inspect_project_references(store)?;
        let mut lock_paths = before
            .iter()
            .map(|reference| {
                (
                    reference.project.clone(),
                    project_lock_path(store, &reference.project),
                )
            })
            .collect::<Vec<_>>();
        lock_paths.sort_by(|left, right| left.0.cmp(&right.0));
        let mut projects = Vec::with_capacity(lock_paths.len());
        for (_, path) in lock_paths {
            projects.push(
                AdvisoryFileLock::acquire(&path)
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!(
                            "cannot acquire lifecycle project lock below '{}'",
                            path.display()
                        )
                    })?,
            );
        }
        let after = inspect_project_references(store)?;
        if !same_project_references(&before, &after) {
            return Err(miette::miette!(
                "project references changed while lifecycle locks were acquired; no managed envelope was removed"
            ));
        }
        store_guard
            .revalidate()
            .into_diagnostic()
            .wrap_err("toolchain store lifecycle lock could not be revalidated")?;
        for project in &projects {
            project
                .revalidate()
                .into_diagnostic()
                .wrap_err("project lifecycle lock could not be revalidated")?;
        }
        Ok(Self {
            store: store_guard,
            projects,
        })
    }

    fn revalidate(&self) -> Result<()> {
        self.store
            .revalidate()
            .into_diagnostic()
            .wrap_err("toolchain store lifecycle lock could not be revalidated")?;
        for project in &self.projects {
            project
                .revalidate()
                .into_diagnostic()
                .wrap_err("project lifecycle lock could not be revalidated")?;
        }
        Ok(())
    }
}

fn inspect_lifecycle(store: &Path, operation: &LifecycleOperation) -> Result<LifecyclePlan> {
    inspect_removal_journals(store)?;
    let project_references = inspect_project_references(store)?;
    let registrations = inspect_registrations(store)?;
    let leases = inspect_leases(store)?;
    inspect_project_guards(store, &project_references, &leases)?;
    let mut candidates = inspect_owned_imports(store)?;
    for candidate in &mut candidates {
        for reference in &project_references {
            if reference.release_id == candidate.release_id_claim {
                candidate.blockers.push(format!(
                    "project '{}' still declares release_id '{}'",
                    reference.project, candidate.release_id_claim
                ));
            }
        }
        for registration in &registrations {
            if registration.source == candidate.payload_text {
                candidate.blockers.push(format!(
                    "external registration '{}' still names this payload",
                    registration.path.display()
                ));
            }
        }
        for lease in &leases {
            if lease.envelope == candidate.envelope_text && lease.state == AdvisoryLockState::Held {
                candidate.blockers.push(format!(
                    "active OS-held build lease '{}' protects this envelope",
                    lease.lease_id
                ));
            }
        }
    }
    candidates.sort_by(|left, right| left.envelope_text.cmp(&right.envelope_text));
    if let LifecycleOperation::Remove(managed_id) = operation {
        let count = candidates
            .iter()
            .filter(|candidate| candidate.managed_id == *managed_id)
            .count();
        if count == 0 {
            return Err(miette::miette!(
                "managed import '{}' was not found below the owned v1 import namespace",
                managed_id
            ));
        }
        if count != 1 {
            return Err(miette::miette!(
                "managed import '{}' is ambiguous across fixed-layout envelopes",
                managed_id
            ));
        }
    }
    Ok(LifecyclePlan {
        store: store.to_path_buf(),
        candidates,
        project_references,
        registrations,
        leases,
    })
}

fn inspect_owned_imports(store: &Path) -> Result<Vec<ManagedCandidate>> {
    let imports = store.join(MANAGED_IMPORTS_DIRECTORY);
    let Some(hosts) = list_directory_names(&imports, "managed import root")? else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    for host in hosts {
        if !safe_segment(&host) {
            return Err(miette::miette!(
                "managed import root '{}' has an unsafe host entry '{}'; cleanup is blocked",
                imports.display(),
                host
            ));
        }
        let host_path = required_real_directory(&imports, &host, "managed import host")?;
        let Some(profiles) = list_directory_names(&host_path, "managed import host")? else {
            return Err(miette::miette!(
                "managed import host '{}' disappeared during inspection",
                host_path.display()
            ));
        };
        if profiles.is_empty() {
            return Err(miette::miette!(
                "managed import host '{}' has no profile directories; cleanup is blocked",
                host_path.display()
            ));
        }
        for profile in profiles {
            if !safe_segment(&profile) {
                return Err(miette::miette!(
                    "managed import host '{}' has an unsafe profile entry '{}'; cleanup is blocked",
                    host_path.display(),
                    profile
                ));
            }
            let profile_path =
                required_real_directory(&host_path, &profile, "managed import target profile")?;
            let Some(identities) = list_directory_names(&profile_path, "managed import profile")?
            else {
                return Err(miette::miette!(
                    "managed import profile '{}' disappeared during inspection",
                    profile_path.display()
                ));
            };
            let managed_ids = identities
                .iter()
                .filter(|identity| is_lower_sha256(identity))
                .cloned()
                .collect::<Vec<_>>();
            let expected_locks = managed_ids
                .iter()
                .map(|managed_id| {
                    let envelope = profile_path.join(managed_id);
                    let journal = publication_journal_path(&envelope, "prepared-tree")
                        .into_diagnostic()
                        .wrap_err("cannot derive managed import publication journal")?;
                    let lock = publication_journal_lock_path(&journal)
                        .into_diagnostic()
                        .wrap_err("cannot derive managed import publication lock")?;
                    Ok(lock)
                })
                .collect::<Result<BTreeSet<_>>>()?;
            for name in &identities {
                if is_lower_sha256(name) {
                    continue;
                }
                let path = profile_path.join(name);
                if !expected_locks.contains(&path) && !is_publication_journal_lock_name(name) {
                    return Err(miette::miette!(
                        "managed import profile '{}' has an unknown entry '{}'; cleanup is blocked",
                        profile_path.display(),
                        name
                    ));
                }
                let metadata = fs::symlink_metadata(&path)
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!(
                            "cannot inspect managed import publication lock '{}'",
                            path.display()
                        )
                    })?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(miette::miette!(
                        "managed import publication lock '{}' is not a regular file; cleanup is blocked",
                        path.display()
                    ));
                }
            }
            for managed_id in managed_ids {
                let envelope =
                    required_real_directory(&profile_path, &managed_id, "managed import envelope")?;
                let payload = envelope.join(MANAGED_PAYLOAD_DIRECTORY);
                let imported =
                    managed_import_envelope_for_payload(store, &payload)?.ok_or_else(|| {
                        miette::miette!(
                            "managed import payload '{}' escaped its owned namespace",
                            payload.display()
                        )
                    })?;
                if imported.envelope != envelope || imported.managed_id != managed_id {
                    return Err(miette::miette!(
                        "managed import envelope '{}' changed while its ownership receipt was verified",
                        envelope.display()
                    ));
                }
                let snapshot = measure_tree_content_cas_bounded(&envelope, managed_import_limits())
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!(
                            "cannot take a bounded no-follow snapshot of managed envelope '{}'",
                            envelope.display()
                        )
                    })?;
                let regular_file_bytes = snapshot.regular_file_bytes().ok_or_else(|| {
                    miette::miette!(
                        "managed envelope '{}' has an unreportable regular-file byte scope",
                        envelope.display()
                    )
                })?;
                candidates.push(ManagedCandidate {
                    managed_id,
                    release_id_claim: imported.release_id_claim,
                    envelope_text: normalized_absolute_utf8(&envelope, "managed import envelope")?,
                    payload_text: normalized_absolute_utf8(&payload, "managed import payload")?,
                    envelope,
                    snapshot_sha256: snapshot.snapshot_digest().to_string(),
                    entries: snapshot.entry_count(),
                    regular_file_bytes,
                    snapshot,
                    blockers: Vec::new(),
                });
            }
        }
    }
    Ok(candidates)
}

fn inspect_project_references(store: &Path) -> Result<Vec<ProjectReferenceState>> {
    let root = store
        .join(MANAGEMENT_DIRECTORY)
        .join(PROJECT_REFERENCES_DIRECTORY);
    let Some(entries) = list_control_record_names(&root, "project reference directory")? else {
        return Ok(Vec::new());
    };
    let mut references = Vec::with_capacity(entries.len());
    for name in entries {
        let Some(identity) = name.strip_suffix(".json") else {
            return Err(miette::miette!(
                "project reference directory '{}' contains an unknown entry '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        };
        if !is_lower_sha256(identity) {
            return Err(miette::miette!(
                "project reference directory '{}' contains an invalid identity '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        }
        let path = root.join(&name);
        let reference =
            read_project_reference_unbound(&path, "project reference")?.ok_or_else(|| {
                miette::miette!(
                    "project reference '{}' disappeared during inspection; cleanup is blocked",
                    path.display()
                )
            })?;
        let expected = project_reference_path(store, &reference.receipt.project);
        if expected != path {
            return Err(miette::miette!(
                "project reference '{}' does not use its deterministic project identity; cleanup is blocked",
                path.display()
            ));
        }
        let project_lock = PathBuf::from(&reference.receipt.project_lock);
        let (project_lock_identity, project_lock_sha256, lock) =
            read_authoritative_project_lock(&project_lock)?;
        if project_lock_sha256 != reference.receipt.lock_sha256
            || lock.release_id != reference.receipt.release_id
        {
            return Err(miette::miette!(
                "project reference '{}' no longer matches its authoritative project lock; cleanup is blocked",
                path.display()
            ));
        }
        references.push(ProjectReferenceState {
            path,
            identity: reference.identity,
            sha256: reference.sha256,
            project: reference.receipt.project,
            project_lock,
            project_lock_identity,
            project_lock_sha256,
            release_id: lock.release_id,
        });
    }
    references.sort_by(|left, right| left.project.cmp(&right.project));
    Ok(references)
}

fn inspect_registrations(store: &Path) -> Result<Vec<RegistrationState>> {
    let root = store
        .join(MANAGEMENT_DIRECTORY)
        .join(REGISTRATIONS_DIRECTORY);
    let Some(entries) = list_control_record_names(&root, "external registration directory")? else {
        return Ok(Vec::new());
    };
    let mut registrations = Vec::with_capacity(entries.len());
    for name in entries {
        let Some(registration_id) = name.strip_suffix(".json") else {
            return Err(miette::miette!(
                "external registration directory '{}' contains an unknown entry '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        };
        if !is_lower_sha256(registration_id) {
            return Err(miette::miette!(
                "external registration directory '{}' contains an invalid identity '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        }
        let path = root.join(&name);
        let registration = read_registration(&path, "external registration")?.ok_or_else(|| {
            miette::miette!(
                "external registration '{}' disappeared during inspection; cleanup is blocked",
                path.display()
            )
        })?;
        if registration.receipt.registration_id != registration_id {
            return Err(miette::miette!(
                "external registration '{}' does not match its deterministic identity; cleanup is blocked",
                path.display()
            ));
        }
        registrations.push(RegistrationState {
            path,
            identity: registration.identity,
            sha256: registration.sha256,
            source: registration.receipt.source,
        });
    }
    registrations.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(registrations)
}

fn inspect_leases(store: &Path) -> Result<Vec<LeaseState>> {
    let root = store.join(MANAGEMENT_DIRECTORY).join(LEASES_DIRECTORY);
    let Some(entries) = list_control_record_names(&root, "build lease directory")? else {
        return Ok(Vec::new());
    };
    let mut leases = Vec::with_capacity(entries.len());
    for name in entries {
        let Some(lease_id) = name.strip_suffix(".json") else {
            return Err(miette::miette!(
                "build lease directory '{}' contains an unknown entry '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        };
        if !is_lower_sha256(lease_id) {
            return Err(miette::miette!(
                "build lease directory '{}' contains an invalid identity '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        }
        let path = root.join(&name);
        let Some((identity, bytes)) = measure_regular_file_bounded(&path, MAX_LEASE_RECEIPT_BYTES)
            .into_diagnostic()
            .wrap_err_with(|| format!("cannot safely read build lease '{}'", path.display()))?
        else {
            return Err(miette::miette!(
                "build lease '{}' disappeared during inspection; cleanup is blocked",
                path.display()
            ));
        };
        let receipt: BuildLeaseReceipt = serde_json::from_slice(&bytes)
            .into_diagnostic()
            .wrap_err_with(|| format!("build lease '{}' is not valid JSON", path.display()))?;
        validate_lease_receipt(store, &path, lease_id, &receipt)?;
        let lock_path = lease_lock_path(store, lease_id);
        let observation = probe_advisory_file_lock(&lock_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot safely probe build-lease lock for receipt '{}'",
                    path.display()
                )
            })?;
        if observation.identity != Some(receipt.lease_lock_identity) {
            return Err(miette::miette!(
                "build lease '{}' no longer binds its recorded lock inode; cleanup is blocked",
                path.display()
            ));
        }
        leases.push(LeaseState {
            path,
            identity,
            sha256: sha256_bytes(&bytes).to_string(),
            lease_id: lease_id.to_owned(),
            lock_path,
            lock_identity: receipt.lease_lock_identity,
            project: receipt.project,
            envelope: receipt.envelope,
            state: observation.state,
        });
    }
    leases.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(leases)
}

/// Inspect every durable project-lock pathname, including paths which no
/// longer have a derived project receipt. A lock left by a selection that
/// committed its authoritative project lock before reference publication is
/// not evidence of non-use; it blocks cleanup rather than disappearing from
/// the index. The only permitted receipt-less guard is one bound to a valid
/// managed-build lease, whose lifecycle contract identifies its project.
fn inspect_project_guards(
    store: &Path,
    references: &[ProjectReferenceState],
    leases: &[LeaseState],
) -> Result<()> {
    let root = store
        .join(MANAGEMENT_DIRECTORY)
        .join(PROJECT_LOCKS_DIRECTORY);
    let Some(entries) = list_directory_names(&root, "project lifecycle lock directory")? else {
        return Ok(());
    };
    let reference_guard_paths = references
        .iter()
        .map(|reference| project_lock_path(store, &reference.project))
        .collect::<BTreeSet<_>>();
    let lease_guard_paths = leases
        .iter()
        .map(|lease| project_lock_path(store, &lease.project))
        .collect::<BTreeSet<_>>();
    for name in entries {
        let Some(identity) = name.strip_suffix(".lock") else {
            return Err(miette::miette!(
                "project lifecycle lock directory '{}' contains an unknown entry '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        };
        if !is_lower_sha256(identity) {
            return Err(miette::miette!(
                "project lifecycle lock directory '{}' contains an invalid identity '{}'; cleanup is blocked",
                root.display(),
                identity
            ));
        }
        let path = root.join(&name);
        let observation = probe_advisory_file_lock(&path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot safely inspect project lifecycle lock '{}'",
                    path.display()
                )
            })?;
        observation.identity.ok_or_else(|| {
            miette::miette!(
                "project lifecycle lock '{}' disappeared during inspection; cleanup is blocked",
                path.display()
            )
        })?;
        if !reference_guard_paths.contains(&path) && !lease_guard_paths.contains(&path) {
            return Err(miette::miette!(
                "project lifecycle lock '{}' has no valid project reference or managed-build lease; cleanup is blocked",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_lease_receipt(
    store: &Path,
    path: &Path,
    expected_lease_id: &str,
    receipt: &BuildLeaseReceipt,
) -> Result<()> {
    let project = normalized_absolute_utf8(Path::new(&receipt.project), "build lease project")?;
    let expected_project_lock = normalized_absolute_utf8(
        &toolchain::lock_file_path(Path::new(&project)),
        "build lease project lock",
    )?;
    let project_lock =
        normalized_absolute_utf8(Path::new(&receipt.project_lock), "build lease project lock")?;
    let expected_lease_lock = normalized_absolute_utf8(
        &lease_lock_path(store, expected_lease_id),
        "build lease lock",
    )?;
    let lease_lock = normalized_absolute_utf8(Path::new(&receipt.lease_lock), "build lease lock")?;
    let envelope = normalized_absolute_utf8(Path::new(&receipt.envelope), "build lease envelope")?;
    let toolchain_root = normalized_absolute_utf8(
        Path::new(&receipt.toolchain_root),
        "build lease toolchain root",
    )?;
    let expected_envelope = managed_envelope_path(store, &envelope, &receipt.managed_id)?;
    let expected_root = normalized_absolute_utf8(
        &expected_envelope.join(MANAGED_PAYLOAD_DIRECTORY),
        "build lease toolchain root",
    )?;
    if receipt.schema != LEASE_SCHEMA
        || receipt.lease_id != expected_lease_id
        || receipt.project != project
        || receipt.project_lock != project_lock
        || receipt.project_lock != expected_project_lock
        || receipt.lease_lock != lease_lock
        || receipt.lease_lock != expected_lease_lock
        || receipt
            .project_lock_sha256
            .as_deref()
            .is_some_and(|digest| !is_lower_sha256(digest))
        || receipt.envelope != envelope
        || receipt.toolchain_root != toolchain_root
        || receipt.toolchain_root != expected_root
        || !is_lower_sha256(&receipt.managed_id)
        || receipt.release_id_claim.is_empty()
    {
        return Err(miette::miette!(
            "build lease '{}' violates the v1 lifecycle contract; cleanup is blocked",
            path.display()
        ));
    }
    Ok(())
}

fn inspect_removal_journals(store: &Path) -> Result<()> {
    let root = store.join(MANAGEMENT_DIRECTORY).join(REMOVALS_DIRECTORY);
    let Some(entries) = list_control_record_names(&root, "managed removal journal directory")?
    else {
        return Ok(());
    };
    for name in entries {
        let Some(journal_id) = name.strip_suffix(".json") else {
            return Err(miette::miette!(
                "managed removal journal directory '{}' contains an unknown entry '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        };
        if !is_lower_sha256(journal_id) {
            return Err(miette::miette!(
                "managed removal journal directory '{}' contains an invalid identity '{}'; cleanup is blocked",
                root.display(),
                name
            ));
        }
        let path = root.join(&name);
        let Some((_, bytes)) = measure_regular_file_bounded(&path, MAX_LEASE_RECEIPT_BYTES)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot safely read managed removal journal '{}'",
                    path.display()
                )
            })?
        else {
            return Err(miette::miette!(
                "managed removal journal '{}' disappeared during inspection; cleanup is blocked",
                path.display()
            ));
        };
        let journal: RemovalJournal = serde_json::from_slice(&bytes)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "managed removal journal '{}' is not valid JSON",
                    path.display()
                )
            })?;
        let envelope = managed_envelope_path(store, &journal.envelope, &journal.managed_id)?;
        let expected_path = removal_journal_path(
            store,
            &journal.managed_id,
            &journal.envelope,
            &journal.snapshot_sha256,
        );
        if journal.schema != REMOVAL_SCHEMA
            || journal.journal_id != journal_id
            || journal.operation != "remove"
            || journal.envelope != normalized_absolute_utf8(&envelope, "journal envelope")?
            || journal.release_id_claim.is_empty()
            || !is_lower_sha256(&journal.snapshot_sha256)
            || path != expected_path
        {
            return Err(miette::miette!(
                "managed removal journal '{}' violates the v1 lifecycle contract; cleanup is blocked",
                path.display()
            ));
        }
        match fs::symlink_metadata(&envelope) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(miette::miette!(
                    "managed removal journal '{}' has a still-present envelope '{}'; preserve it for recovery before attempting more cleanup",
                    path.display(),
                    envelope.display()
                ));
            }
            Err(error) => {
                return Err(error).into_diagnostic().wrap_err_with(|| {
                    format!(
                        "cannot determine recovery state for managed removal journal '{}'",
                        path.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn read_authoritative_project_lock(
    path: &Path,
) -> Result<(FileIdentity, String, ArosToolchainLock)> {
    let Some((identity, bytes)) = measure_regular_file_bounded(path, MAX_RELEASE_LOCK_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely read project lock '{}'", path.display()))?
    else {
        return Err(miette::miette!(
            "project lock '{}' is missing; cleanup is blocked",
            path.display()
        ));
    };
    let contents = std::str::from_utf8(&bytes)
        .into_diagnostic()
        .wrap_err_with(|| format!("project lock '{}' is not UTF-8", path.display()))?;
    let lock: ArosToolchainLock = toml::from_str(contents)
        .into_diagnostic()
        .wrap_err_with(|| format!("project lock '{}' is not valid TOML", path.display()))?;
    lock.validate()
        .map_err(|message| miette::miette!("{message}"))
        .wrap_err_with(|| format!("project lock '{}' violates the v1 contract", path.display()))?;
    Ok((identity, sha256_bytes(&bytes).to_string(), lock))
}

fn list_directory_names(path: &Path, label: &str) -> Result<Option<Vec<String>>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("cannot inspect {label} '{}'", path.display()));
        }
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(miette::miette!(
                "{label} '{}' is not a real directory; cleanup is blocked",
                path.display()
            ));
        }
        Ok(_) => {}
    }
    let mut names = directory_entry_names_nofollow_bounded(path, MAX_CONTROL_ENTRIES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely list {label} '{}'", path.display()))?
        .into_iter()
        .map(|name| {
            name.into_string().map_err(|_| {
                miette::miette!(
                    "{label} '{}' contains a non-UTF-8 entry name; cleanup is blocked",
                    path.display()
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    Ok(Some(names))
}

fn list_control_record_names(path: &Path, label: &str) -> Result<Option<Vec<String>>> {
    let Some(names) = list_directory_names(path, label)? else {
        return Ok(None);
    };
    let mut records = Vec::with_capacity(names.len());
    for name in names {
        if !is_publication_journal_lock_name(&name) {
            records.push(name);
            continue;
        }
        let lock_path = path.join(&name);
        let metadata = fs::symlink_metadata(&lock_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot inspect shared publication lock '{}'",
                    lock_path.display()
                )
            })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(miette::miette!(
                "shared publication lock '{}' is not a regular file; cleanup is blocked",
                lock_path.display()
            ));
        }
    }
    Ok(Some(records))
}

fn required_real_directory(parent: &Path, name: &str, label: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    let metadata = fs::symlink_metadata(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot inspect {label} '{}'", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "{label} '{}' is not a real directory; cleanup is blocked",
            path.display()
        ));
    }
    Ok(path)
}

fn managed_envelope_path(store: &Path, envelope_text: &str, managed_id: &str) -> Result<PathBuf> {
    if !is_lower_sha256(managed_id) {
        return Err(miette::miette!(
            "managed import identity is not a lowercase SHA-256 value"
        ));
    }
    let envelope = PathBuf::from(envelope_text);
    let normalized = normalized_absolute_utf8(&envelope, "managed import envelope")?;
    if normalized != envelope_text {
        return Err(miette::miette!("managed import envelope is not canonical"));
    }
    let root = store.join(MANAGED_IMPORTS_DIRECTORY);
    let relative = envelope.strip_prefix(&root).map_err(|_| {
        miette::miette!("managed import envelope lies outside the owned v1 import namespace")
    })?;
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(component) => component
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| miette::miette!("managed import envelope has non-UTF-8 components")),
            _ => Err(miette::miette!(
                "managed import envelope has unsafe path components"
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    let [host, profile, observed_id] = components.as_slice() else {
        return Err(miette::miette!(
            "managed import envelope does not use the v1 host/profile/identity layout"
        ));
    };
    if !safe_segment(host)
        || !safe_segment(profile)
        || !is_lower_sha256(observed_id)
        || observed_id != managed_id
    {
        return Err(miette::miette!(
            "managed import envelope has an invalid v1 identity"
        ));
    }
    Ok(root.join(host).join(profile).join(observed_id))
}

fn validate_controls_unchanged(
    store: &Path,
    plan: &LifecyclePlan,
    scope: &LifecycleScope,
) -> Result<()> {
    scope.revalidate()?;
    inspect_removal_journals(store)?;
    let current_references = inspect_project_references(store)?;
    let current_registrations = inspect_registrations(store)?;
    let current_leases = inspect_leases(store)?;
    inspect_project_guards(store, &current_references, &current_leases)?;
    if !same_project_references(&plan.project_references, &current_references)
        || !same_registrations(&plan.registrations, &current_registrations)
        || !same_leases(&plan.leases, &current_leases)
    {
        return Err(miette::miette!(
            "toolchain lifecycle control-plane state changed after preview; no further managed envelope was removed"
        ));
    }
    Ok(())
}

fn remove_candidate(store: &Path, candidate: &ManagedCandidate) -> Result<()> {
    let current = measure_tree_content_cas_bounded(&candidate.envelope, managed_import_limits())
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot remeasure managed envelope '{}' before removal",
                candidate.envelope.display()
            )
        })?;
    if current != candidate.snapshot {
        return Err(miette::miette!(
            "managed envelope '{}' changed after preview; it was not removed",
            candidate.envelope.display()
        ));
    }
    let journal = RemovalJournal {
        schema: REMOVAL_SCHEMA.to_owned(),
        journal_id: removal_journal_id(
            &candidate.managed_id,
            &candidate.envelope_text,
            &candidate.snapshot_sha256,
        ),
        operation: "remove".to_owned(),
        managed_id: candidate.managed_id.clone(),
        envelope: candidate.envelope_text.clone(),
        release_id_claim: candidate.release_id_claim.clone(),
        snapshot_sha256: candidate.snapshot_sha256.clone(),
        entries: candidate.entries,
        regular_file_bytes: candidate.regular_file_bytes,
    };
    let journal_path = removal_journal_path(
        store,
        &candidate.managed_id,
        &candidate.envelope_text,
        &candidate.snapshot_sha256,
    );
    let journal_bytes = serde_json::to_vec_pretty(&journal)
        .into_diagnostic()
        .wrap_err("cannot serialize managed removal journal")?;
    if let Err(error) =
        publish_atomic_file(&journal_path, &journal_bytes, AtomicFilePolicy::NoClobber)
    {
        return publication_error(
            error,
            "managed removal journal publication failed",
            "managed removal journal publication may have crossed its durable boundary; inspect that exact journal before retrying",
            "managed removal journal did not cross its durable publication boundary",
        );
    }
    let Some((_, published_bytes)) =
        measure_regular_file_bounded(&journal_path, MAX_LEASE_RECEIPT_BYTES)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "cannot reread managed removal journal '{}'",
                    journal_path.display()
                )
            })?
    else {
        return crate::observability::commit_state(
            Err(miette::miette!(
                "managed removal journal '{}' disappeared after publication",
                journal_path.display()
            )),
            CommitState::Indeterminate,
            "the removal journal was published, but its readback could not be proven; preserve the envelope and inspect recovery state",
        );
    };
    if published_bytes != journal_bytes {
        return crate::observability::commit_state(
            Err(miette::miette!(
                "managed removal journal '{}' changed during readback",
                journal_path.display()
            )),
            CommitState::Indeterminate,
            "the removal journal was published, but its readback changed; preserve the envelope and inspect recovery state",
        );
    }
    if let Err(error) = remove_tree_from_snapshot_nofollow(
        &candidate.envelope,
        &candidate.snapshot,
        managed_import_limits(),
    ) {
        return crate::observability::commit_state(
            Err(error)
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "managed envelope '{}' could not be removed from its approved snapshot",
                        candidate.envelope.display()
                    )
                }),
            CommitState::Indeterminate,
            "the removal journal is durable; the envelope may be unchanged or partially removed, so preserve it for recovery and do not retry blindly",
        );
    }
    match fs::symlink_metadata(&candidate.envelope) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => crate::observability::commit_state(
            Err(miette::miette!(
                "managed envelope '{}' still exists after snapshot-bound removal",
                candidate.envelope.display()
            )),
            CommitState::Indeterminate,
            "the removal journal is durable, but final absence could not be proven; preserve the envelope for recovery",
        ),
        Err(error) => crate::observability::commit_state(
            Err(error)
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "cannot verify final absence of managed envelope '{}'",
                        candidate.envelope.display()
                    )
                }),
            CommitState::Indeterminate,
            "the removal journal is durable, but final absence could not be proven; preserve the envelope for recovery",
        ),
    }
}

fn removal_journal_id(managed_id: &str, envelope: &str, snapshot_sha256: &str) -> String {
    stable_token(
        "aros-toolchain-removal-journal-v1",
        &[managed_id, envelope, snapshot_sha256],
    )
}

fn removal_journal_path(
    store: &Path,
    managed_id: &str,
    envelope: &str,
    snapshot_sha256: &str,
) -> PathBuf {
    store
        .join(MANAGEMENT_DIRECTORY)
        .join(REMOVALS_DIRECTORY)
        .join(format!(
            "{}.json",
            removal_journal_id(managed_id, envelope, snapshot_sha256)
        ))
}

fn same_project_references(
    expected: &[ProjectReferenceState],
    actual: &[ProjectReferenceState],
) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(left, right)| {
            left.path == right.path
                && left.identity == right.identity
                && left.sha256 == right.sha256
                && left.project == right.project
                && left.project_lock == right.project_lock
                && left.project_lock_identity == right.project_lock_identity
                && left.project_lock_sha256 == right.project_lock_sha256
                && left.release_id == right.release_id
        })
}

fn same_registrations(expected: &[RegistrationState], actual: &[RegistrationState]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(left, right)| {
            left.path == right.path
                && left.identity == right.identity
                && left.sha256 == right.sha256
                && left.source == right.source
        })
}

fn same_leases(expected: &[LeaseState], actual: &[LeaseState]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(left, right)| {
            left.path == right.path
                && left.identity == right.identity
                && left.sha256 == right.sha256
                && left.lease_id == right.lease_id
                && left.lock_path == right.lock_path
                && left.lock_identity == right.lock_identity
                && left.project == right.project
                && left.envelope == right.envelope
                && left.state == right.state
        })
}

fn lifecycle_apply_token(plan: &LifecyclePlan, operation: &LifecycleOperation) -> String {
    let mut fields = vec![
        operation.name().to_owned(),
        plan.store.display().to_string(),
    ];
    if let LifecycleOperation::Remove(managed_id) = operation {
        fields.push(managed_id.clone());
    }
    for candidate in &plan.candidates {
        fields.extend([
            candidate.managed_id.clone(),
            candidate.envelope_text.clone(),
            candidate.snapshot_sha256.clone(),
            candidate.entries.to_string(),
            candidate.regular_file_bytes.to_string(),
            candidate.blockers.join("\n"),
        ]);
    }
    for reference in &plan.project_references {
        fields.extend([
            reference.path.display().to_string(),
            reference.identity.device().to_string(),
            reference.identity.inode().to_string(),
            reference.sha256.clone(),
            reference.project_lock_identity.device().to_string(),
            reference.project_lock_identity.inode().to_string(),
            reference.project_lock_sha256.clone(),
            reference.release_id.clone(),
        ]);
    }
    for registration in &plan.registrations {
        fields.extend([
            registration.path.display().to_string(),
            registration.identity.device().to_string(),
            registration.identity.inode().to_string(),
            registration.sha256.clone(),
            registration.source.clone(),
        ]);
    }
    for lease in &plan.leases {
        fields.extend([
            lease.path.display().to_string(),
            lease.identity.device().to_string(),
            lease.identity.inode().to_string(),
            lease.sha256.clone(),
            lease.lock_path.display().to_string(),
            lease.lock_identity.device().to_string(),
            lease.lock_identity.inode().to_string(),
            lease.project.clone(),
            lease.envelope.clone(),
            format!("{:?}", lease.state),
        ]);
    }
    let field_refs = fields.iter().map(String::as_str).collect::<Vec<_>>();
    stable_token("aros-toolchain-lifecycle-apply-v1", &field_refs)
}

fn lifecycle_result(
    plan: &LifecyclePlan,
    operation: &LifecycleOperation,
    state: &'static str,
    apply_token: Option<String>,
    removed: &BTreeSet<String>,
    note: &'static str,
) -> LifecycleResult {
    LifecycleResult {
        schema: LIFECYCLE_SCHEMA,
        operation: operation.name(),
        state,
        store: plan.store.display().to_string(),
        candidates: operation
            .selected(plan)
            .into_iter()
            .map(|candidate| LifecycleCandidateResult {
                managed_id: candidate.managed_id.clone(),
                release_id_claim: candidate.release_id_claim.clone(),
                envelope: candidate.envelope_text.clone(),
                snapshot_sha256: candidate.snapshot_sha256.clone(),
                entries: candidate.entries,
                regular_file_bytes: candidate.regular_file_bytes,
                status: if removed.contains(&candidate.managed_id) {
                    "removed"
                } else if candidate.blockers.is_empty() {
                    "eligible"
                } else {
                    "retained"
                },
                blockers: candidate.blockers.clone(),
            })
            .collect(),
        apply_token,
        note,
    }
}

fn print_lifecycle_result(
    result: &LifecycleResult,
    format: crate::toolchain_management::ResultFormat,
) {
    match format {
        crate::toolchain_management::ResultFormat::Human => {
            aros_common::outputln!("Toolchain lifecycle {}: {}", result.operation, result.state);
            aros_common::outputln!("  Store: {}", result.store);
            for candidate in &result.candidates {
                aros_common::outputln!(
                    "  {} [{}] {} entries, {} bytes",
                    candidate.managed_id,
                    candidate.status,
                    candidate.entries,
                    candidate.regular_file_bytes
                );
                aros_common::outputln!("    Envelope: {}", candidate.envelope);
                for blocker in &candidate.blockers {
                    aros_common::outputln!("    Retained: {blocker}");
                }
            }
            aros_common::outputln!("  {}", result.note);
            if let Some(token) = &result.apply_token {
                aros_common::outputln!("  Apply token: {token}");
                aros_common::outputln!(
                    "  No managed envelope changed. Re-run with --apply {token} to commit this exact preview."
                );
            }
        }
        crate::toolchain_management::ResultFormat::Json => {
            let document = serde_json::to_string_pretty(result)
                .expect("lifecycle result serialization is infallible");
            aros_common::outputln!("{document}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_lifecycle, inspect_lifecycle, lease_lock_path, lifecycle_apply_token,
        managed_import_limits, project_lock_path, project_reference_path, removal_journal_id,
        removal_journal_path, BuildLeaseReceipt, LifecycleOperation, RemovalJournal, LEASE_SCHEMA,
        REMOVAL_SCHEMA,
    };
    use crate::toolchain::{ResolvedToolchain, ToolchainPaths, ToolchainSource};
    use aros_common::{
        sha256_bytes, toolchain_tree_inventory, AdvisoryFileLock, ArosToolchainArtifact,
        ArosToolchainLock, ArosToolchainManifest, AROS_TOOLCHAIN_MANIFEST_FILE,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    const MANAGED_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn owned_import(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let store = root.join("store");
        let envelope = store
            .join("imports/v1/linux-x86_64/pc-x86_64")
            .join(MANAGED_ID);
        let payload = envelope.join("toolchain");
        fs::create_dir_all(payload.join("bin")).unwrap();
        fs::write(payload.join("bin/aros-collect"), b"fixture collector").unwrap();
        let (tree_sha256, files) = toolchain_tree_inventory(&payload).unwrap();
        let manifest = ArosToolchainManifest {
            schema: 1,
            release_id: "local-fixture".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: tree_sha256.clone(),
            llvm_version: Some("11.0.0".into()),
            recipe_sha256: "b".repeat(64),
            source_lock_sha256: "c".repeat(64),
            profiles_sha256: "d".repeat(64),
            source_commit: "e".repeat(40),
            producer_commit: "f".repeat(40),
            tools_commit: "0".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["collect-aros".into()],
            build_environment: serde_json::Map::new(),
            files,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(payload.join(AROS_TOOLCHAIN_MANIFEST_FILE), &manifest_bytes).unwrap();
        let ownership = serde_json::json!({
            "schema": "aros-toolchain-ownership-v1",
            "management": "owned-import",
            "managed_id": MANAGED_ID,
            "host": "linux-x86_64",
            "target_profile": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "release_id_claim": "local-fixture",
            "manifest_sha256": sha256_bytes(&manifest_bytes).to_string(),
            "tree_sha256": tree_sha256,
            "source_snapshot_sha256": "1".repeat(64),
        });
        fs::write(
            envelope.join("ownership.json"),
            serde_json::to_vec_pretty(&ownership).unwrap(),
        )
        .unwrap();
        fs::write(envelope.join(".complete"), b"complete\n").unwrap();
        (store, envelope, payload)
    }

    fn resolved_locked(root: &Path, release_id: &str) -> ResolvedToolchain {
        let root = root.to_path_buf();
        ResolvedToolchain {
            paths: ToolchainPaths {
                clang: root.join("bin/clang"),
                clangxx: root.join("bin/clang++"),
                lld: root.join("bin/ld.lld"),
                llvm_ar: root.join("bin/llvm-ar"),
                aros_collect: root.join("bin/aros-collect"),
                collect_aros: root.join("bin/collect-aros"),
                collect_aros32: root.join("bin/collect-aros32"),
                root,
            },
            target_triple: "x86_64-unknown-aros".to_owned(),
            release_id: Some(release_id.to_owned()),
            source: ToolchainSource::LockedRelease,
        }
    }

    #[test]
    fn lifecycle_preview_is_read_only_and_reports_exact_owned_scope() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());

        let plan = inspect_lifecycle(&store, &LifecycleOperation::Gc).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert!(plan.candidates[0].blockers.is_empty());
        assert!(plan.candidates[0].regular_file_bytes > 0);
        assert!(!store.join(".aros-management").exists());
        assert!(envelope.is_dir());
    }

    #[test]
    fn lifecycle_apply_removes_only_the_snapshot_bound_owned_envelope() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let operation = LifecycleOperation::Remove(MANAGED_ID.to_owned());
        let preview = inspect_lifecycle(&store, &operation).unwrap();
        let candidate = preview.candidates.first().unwrap();
        let token = lifecycle_apply_token(&preview, &operation);
        let journal = removal_journal_path(
            &store,
            &candidate.managed_id,
            &candidate.envelope_text,
            &candidate.snapshot_sha256,
        );

        let result = apply_lifecycle(&store, &operation, &token).unwrap();
        assert_eq!(result.state, "committed");
        assert_eq!(result.candidates[0].status, "removed");
        assert!(!envelope.exists());
        assert!(journal.is_file());
        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc)
            .unwrap()
            .candidates
            .is_empty());
    }

    #[test]
    fn active_os_held_lease_retains_an_owned_import() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, payload) = owned_import(temporary.path());
        let project = temporary.path().join("project");
        fs::create_dir(&project).unwrap();
        let lease_id = "2".repeat(64);
        let lease_guard = AdvisoryFileLock::acquire(&lease_lock_path(&store, &lease_id)).unwrap();
        let receipt = BuildLeaseReceipt {
            schema: LEASE_SCHEMA.to_owned(),
            lease_id: lease_id.clone(),
            project: project.display().to_string(),
            project_lock: project
                .join("aros-toolchains.lock.toml")
                .display()
                .to_string(),
            project_lock_sha256: None,
            lease_lock: lease_lock_path(&store, &lease_id).display().to_string(),
            lease_lock_identity: lease_guard.identity().unwrap(),
            toolchain_root: payload.display().to_string(),
            envelope: envelope.display().to_string(),
            managed_id: MANAGED_ID.to_owned(),
            release_id_claim: "local-fixture".to_owned(),
        };
        let lease_path = store
            .join(".aros-management/v1/leases/v1")
            .join(format!("{lease_id}.json"));
        fs::create_dir_all(lease_path.parent().unwrap()).unwrap();
        fs::write(&lease_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

        let plan = inspect_lifecycle(&store, &LifecycleOperation::Gc).unwrap();
        assert_eq!(plan.candidates.len(), 1);
        assert!(plan.candidates[0]
            .blockers
            .iter()
            .any(|blocker| blocker.contains("active OS-held build lease")));
        let lock_path = lease_lock_path(&store, &lease_id);
        let displaced = lock_path.with_file_name("displaced-lease.lock");
        fs::rename(&lock_path, &displaced).unwrap();
        fs::write(&lock_path, b"replacement").unwrap();
        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_err());
        drop(lease_guard);
    }

    #[test]
    fn external_registration_retains_an_owned_import() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, _, payload) = owned_import(temporary.path());
        let registration_id = "3".repeat(64);
        let registration = serde_json::json!({
            "schema": "aros-toolchain-registration-v1",
            "management": "non-owning-external",
            "registration_id": registration_id,
            "source": payload.display().to_string(),
            "host": "linux-x86_64",
            "target_profile": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "release_id_claim": "local-fixture",
            "manifest_sha256": "4".repeat(64),
            "tree_sha256": "5".repeat(64),
            "source_snapshot_sha256": "6".repeat(64),
        });
        let receipt = store
            .join(".aros-management/v1/registrations")
            .join(format!("{registration_id}.json"));
        fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        fs::write(&receipt, serde_json::to_vec_pretty(&registration).unwrap()).unwrap();

        let plan = inspect_lifecycle(&store, &LifecycleOperation::Gc).unwrap();
        assert!(plan.candidates[0]
            .blockers
            .iter()
            .any(|blocker| blocker.contains("external registration")));
    }

    #[test]
    fn changed_owned_tree_invalidates_the_preview_before_removal() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let operation = LifecycleOperation::Remove(MANAGED_ID.to_owned());
        let preview = inspect_lifecycle(&store, &operation).unwrap();
        let token = lifecycle_apply_token(&preview, &operation);
        fs::write(envelope.join("unapproved"), b"changed").unwrap();

        assert!(apply_lifecycle(&store, &operation, &token).is_err());
        assert!(envelope.is_dir());
        assert!(!store.join(".aros-management/v1/removals/v1").exists());
    }

    #[test]
    fn scope_limit_remains_shared_with_import_validation() {
        assert!(managed_import_limits().max_entries >= 1);
    }

    #[test]
    fn unknown_import_namespace_entry_blocks_cleanup_instead_of_being_skipped() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        fs::write(envelope.parent().unwrap().join("unowned-sibling"), b"stop").unwrap();

        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_err());
        assert!(envelope.is_dir());
    }

    #[test]
    fn unresolved_removal_journal_preserves_the_envelope_and_blocks_retry() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let plan = inspect_lifecycle(&store, &LifecycleOperation::Gc).unwrap();
        let candidate = plan.candidates.first().unwrap();
        let journal_id = removal_journal_id(
            &candidate.managed_id,
            &candidate.envelope_text,
            &candidate.snapshot_sha256,
        );
        let journal = RemovalJournal {
            schema: REMOVAL_SCHEMA.to_owned(),
            journal_id,
            operation: "remove".to_owned(),
            managed_id: candidate.managed_id.clone(),
            envelope: candidate.envelope_text.clone(),
            release_id_claim: candidate.release_id_claim.clone(),
            snapshot_sha256: candidate.snapshot_sha256.clone(),
            entries: candidate.entries,
            regular_file_bytes: candidate.regular_file_bytes,
        };
        let path = removal_journal_path(
            &store,
            &candidate.managed_id,
            &candidate.envelope_text,
            &candidate.snapshot_sha256,
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();

        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_err());
        assert!(envelope.is_dir());
        assert!(path.is_file());
    }

    #[test]
    fn malformed_project_reference_blocks_all_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let reference = store
            .join(".aros-management/v1/projects/v1")
            .join(format!("{}.json", "7".repeat(64)));
        fs::create_dir_all(reference.parent().unwrap()).unwrap();
        fs::write(&reference, b"{}").unwrap();

        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_err());
        assert!(envelope.is_dir());
    }

    #[test]
    fn referenced_project_guard_can_be_created_during_apply_without_staling_preview() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let project = temporary.path().join("project");
        fs::create_dir(&project).unwrap();
        let lock = ArosToolchainLock {
            schema: 1,
            release_id: "local-fixture".to_owned(),
            base_url: Some("https://example.invalid/releases/local-fixture".to_owned()),
            artifacts: vec![ArosToolchainArtifact {
                host: "linux-x86_64".to_owned(),
                target_profile: "pc-x86_64".to_owned(),
                target_triple: "x86_64-unknown-aros".to_owned(),
                asset: "fixture.tar.xz".to_owned(),
                sha256: "a".repeat(64),
                tree_sha256: "b".repeat(64),
                llvm_version: Some("11.0.0".to_owned()),
                size: Some(1),
                enabled: true,
                disabled_reason: None,
                strip_components: 1,
                required_paths: vec!["bin/clang".to_owned()],
            }],
        };
        let lock_path = project.join("aros-toolchains.lock.toml");
        let lock_bytes = toml::to_string(&lock).unwrap().into_bytes();
        fs::write(&lock_path, &lock_bytes).unwrap();
        let project_text = project.display().to_string();
        let receipt = serde_json::json!({
            "schema": "aros-toolchain-project-reference-v1",
            "project": project_text,
            "project_lock": lock_path.display().to_string(),
            "release_id": "local-fixture",
            "lock_sha256": sha256_bytes(&lock_bytes).to_string(),
        });
        let reference = project_reference_path(&store, &project.display().to_string());
        fs::create_dir_all(reference.parent().unwrap()).unwrap();
        fs::write(reference, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

        let operation = LifecycleOperation::Remove(MANAGED_ID.to_owned());
        let preview = inspect_lifecycle(&store, &operation).unwrap();
        let token = lifecycle_apply_token(&preview, &operation);
        let error = apply_lifecycle(&store, &operation, &token).unwrap_err();
        assert!(error.to_string().contains("is not removable"));
        assert!(envelope.is_dir());
    }

    #[test]
    fn orphaned_project_guard_blocks_cleanup_after_partial_selection() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, envelope, _) = owned_import(temporary.path());
        let project = temporary.path().join("project");
        fs::create_dir(&project).unwrap();
        let project = project.display().to_string();
        let guard = AdvisoryFileLock::acquire(&project_lock_path(&store, &project)).unwrap();
        drop(guard);

        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_err());
        assert!(envelope.is_dir());
    }

    #[test]
    fn locked_build_derives_a_persistent_project_reference_instead_of_an_orphan_guard() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, _, _) = owned_import(temporary.path());
        let project = temporary.path().join("project");
        let toolchain_root = temporary.path().join("released-toolchain");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&toolchain_root).unwrap();
        let lock = ArosToolchainLock {
            schema: 1,
            release_id: "locked-fixture".to_owned(),
            base_url: Some("https://example.invalid/releases/locked-fixture".to_owned()),
            artifacts: Vec::new(),
        };
        fs::write(
            project.join("aros-toolchains.lock.toml"),
            toml::to_string(&lock).unwrap(),
        )
        .unwrap();

        let lease = super::acquire_for_build_in_store(
            &project,
            &resolved_locked(&toolchain_root, &lock.release_id),
            &store,
        )
        .unwrap();
        drop(lease);

        let project_text = project.display().to_string();
        let reference = project_reference_path(&store, &project_text);
        assert!(reference.is_file());
        assert!(inspect_lifecycle(&store, &LifecycleOperation::Gc).is_ok());
    }
}
