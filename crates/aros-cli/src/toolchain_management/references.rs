//! Durable project-reference and external-registration control records.
//!
//! The project lock remains selection authority. These records provide only a
//! bounded, verified lifecycle index and must never be treated as a selector.

use super::{
    is_lower_sha256, normalized_absolute_utf8, stable_token, toolchain, AdvisoryFileLock,
    AtomicFilePolicy, IntoDiagnostic, MeasuredProjectReference, MeasuredRegistration, Path,
    PathBuf, ProjectReferenceReceipt, RegistrationReceipt, Result, WrapErr, MANAGEMENT_DIRECTORY,
    MAX_PROJECT_REFERENCE_BYTES, PROJECT_LOCKS_DIRECTORY, PROJECT_REFERENCES_DIRECTORY,
    PROJECT_REFERENCE_SCHEMA, REGISTRATION_RECEIPT_SCHEMA, STORE_LOCK,
};
use aros_common::{measure_regular_file_bounded, publish_atomic_file, sha256_bytes};

pub fn acquire_store_lock(store: &Path) -> Result<AdvisoryFileLock> {
    AdvisoryFileLock::acquire(&store.join(MANAGEMENT_DIRECTORY).join(STORE_LOCK))
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot acquire toolchain store lock below '{}'",
                store.display()
            )
        })
}

/// Return the deterministic OS-lock path for one canonical project root.
///
/// This guard is mutual exclusion only. It is not a project selection record.
pub fn project_lock_path(store: &Path, project: &str) -> PathBuf {
    let project_id = stable_token("aros-toolchain-project-lock-v1", &[project]);
    store
        .join(MANAGEMENT_DIRECTORY)
        .join(PROJECT_LOCKS_DIRECTORY)
        .join(format!("{project_id}.lock"))
}

/// Return the deterministic receipt path for a project's derived reference.
pub fn project_reference_path(store: &Path, project: &str) -> PathBuf {
    let project_id = stable_token("aros-toolchain-project-reference-v1", &[project]);
    store
        .join(MANAGEMENT_DIRECTORY)
        .join(PROJECT_REFERENCES_DIRECTORY)
        .join(format!("{project_id}.json"))
}

/// Read and bind one derived project reference to its authoritative lock.
///
/// `None` means that no receipt exists. Invalid, unreadable, swapped, or
/// mismatched receipts are errors; callers must treat them as lifecycle
/// blockers rather than as evidence that the referenced project is unused.
pub fn read_project_reference(
    path: &Path,
    expected_project: &str,
    expected_project_lock: &Path,
    label: &str,
) -> Result<Option<MeasuredProjectReference>> {
    let reference = read_project_reference_unbound(path, label)?;
    let Some(reference) = reference else {
        return Ok(None);
    };
    let expected_lock = normalized_absolute_utf8(expected_project_lock, "project lock")?;
    if reference.receipt.project != expected_project
        || reference.receipt.project_lock != expected_lock
    {
        return Err(miette::miette!(
            "{label} '{}' does not bind this project and its authoritative lock",
            path.display()
        ));
    }
    Ok(Some(reference))
}

/// Ensure that a project's already-published release lock has a matching
/// derived lifecycle reference.
///
/// The caller must hold both the store lock and the project's advisory lock.
/// This helper never replaces a reference: a mismatched existing receipt is
/// evidence of an incomplete or externally modified selection and must be
/// repaired through the explicit selection workflow before a build can rely
/// on it.
pub fn ensure_project_reference_for_locked_build(
    store: &Path,
    project: &str,
    project_lock: &Path,
    release_id: &str,
    lock_sha256: &str,
) -> Result<()> {
    let reference_path = project_reference_path(store, project);
    if let Some(existing) = read_project_reference(
        &reference_path,
        project,
        project_lock,
        "existing project reference",
    )? {
        if existing.receipt.release_id == release_id && existing.receipt.lock_sha256 == lock_sha256
        {
            return Ok(());
        }
        return Err(miette::miette!(
            "existing project reference '{}' does not match the locked release; run 'aros toolchain select' to repair the project lifecycle state",
            reference_path.display()
        ));
    }

    let receipt = ProjectReferenceReceipt {
        schema: PROJECT_REFERENCE_SCHEMA.to_owned(),
        project: project.to_owned(),
        project_lock: normalized_absolute_utf8(project_lock, "project lock")?,
        release_id: release_id.to_owned(),
        lock_sha256: lock_sha256.to_owned(),
    };
    publish_project_reference_receipt(&reference_path, &receipt, None).wrap_err_with(|| {
        format!(
            "cannot publish build-derived project reference '{}'; rerun 'aros toolchain select' if another selection completed concurrently",
            reference_path.display()
        )
    })?;
    Ok(())
}

/// Atomically publish one project-reference receipt and prove its readback.
///
/// `previous` is the exact receipt snapshot permitted to be replaced. Omit it
/// to require no prior receipt. The caller owns any broader transaction or
/// commit-state classification; this primitive only establishes the durable
/// reference boundary.
pub fn publish_project_reference_receipt(
    path: &Path,
    receipt: &ProjectReferenceReceipt,
    previous: Option<&MeasuredProjectReference>,
) -> Result<MeasuredProjectReference> {
    let bytes = serde_json::to_vec_pretty(receipt)
        .into_diagnostic()
        .wrap_err("cannot serialize project reference receipt")?;
    let policy = previous.map_or(AtomicFilePolicy::NoClobber, |previous| {
        AtomicFilePolicy::ReplaceIf {
            identity: previous.identity,
            sha256: sha256_bytes(&previous.bytes),
        }
    });
    publish_atomic_file(path, &bytes, policy)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot publish project reference '{}'", path.display()))?;
    let published = read_project_reference(
        path,
        &receipt.project,
        Path::new(&receipt.project_lock),
        "published project reference",
    )?
    .ok_or_else(|| {
        miette::miette!(
            "project reference disappeared after publication: '{}'",
            path.display()
        )
    })?;
    if published.bytes != bytes || published.receipt != *receipt {
        return Err(miette::miette!(
            "published project reference does not match its approved receipt"
        ));
    }
    Ok(published)
}

/// Read one project reference without a caller-supplied project binding.
///
/// This is for store-wide lifecycle scans. It validates the receipt's own
/// canonical project and lock paths, but the current lock bytes still need to
/// be measured separately before a destructive operation is allowed.
pub fn read_project_reference_unbound(
    path: &Path,
    label: &str,
) -> Result<Option<MeasuredProjectReference>> {
    let Some((identity, bytes)) = measure_regular_file_bounded(path, MAX_PROJECT_REFERENCE_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely read {label} '{}'", path.display()))?
    else {
        return Ok(None);
    };
    let receipt: ProjectReferenceReceipt = serde_json::from_slice(&bytes)
        .into_diagnostic()
        .wrap_err_with(|| format!("{label} '{}' is not valid JSON", path.display()))?;
    let project = Path::new(&receipt.project);
    let normalized_project = normalized_absolute_utf8(project, "project reference project")?;
    let expected_lock = normalized_absolute_utf8(
        &toolchain::lock_file_path(project),
        "project reference lock",
    )?;
    if receipt.schema != PROJECT_REFERENCE_SCHEMA
        || receipt.project != normalized_project
        || receipt.project_lock != expected_lock
    {
        return Err(miette::miette!(
            "{label} '{}' does not bind a canonical project and its authoritative lock",
            path.display()
        ));
    }
    if receipt.release_id.is_empty()
        || receipt.lock_sha256.len() != 64
        || !receipt
            .lock_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(miette::miette!(
            "{label} '{}' has an invalid release identity or lock digest",
            path.display()
        ));
    }
    Ok(Some(MeasuredProjectReference {
        identity,
        sha256: sha256_bytes(&bytes).to_string(),
        bytes,
        receipt,
    }))
}

/// Read and validate one non-owning external registration receipt.
///
/// A registration is never deletion authority. Lifecycle code uses this to
/// retain an owned import when an explicit registration still names it, and
/// to fail closed if a control-plane receipt is malformed.
pub fn read_registration(path: &Path, label: &str) -> Result<Option<MeasuredRegistration>> {
    let Some((identity, bytes)) = measure_regular_file_bounded(path, MAX_PROJECT_REFERENCE_BYTES)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely read {label} '{}'", path.display()))?
    else {
        return Ok(None);
    };
    let receipt: RegistrationReceipt = serde_json::from_slice(&bytes)
        .into_diagnostic()
        .wrap_err_with(|| format!("{label} '{}' is not valid JSON", path.display()))?;
    let normalized_source =
        normalized_absolute_utf8(Path::new(&receipt.source), "registration source")?;
    if receipt.schema != REGISTRATION_RECEIPT_SCHEMA
        || receipt.management != "non-owning-external"
        || !is_lower_sha256(&receipt.registration_id)
        || receipt.source != normalized_source
        || receipt.host.is_empty()
        || receipt.target_profile.is_empty()
        || receipt.target_triple.is_empty()
        || receipt.release_id_claim.is_empty()
        || !is_lower_sha256(&receipt.manifest_sha256)
        || !is_lower_sha256(&receipt.tree_sha256)
        || !is_lower_sha256(&receipt.source_snapshot_sha256)
    {
        return Err(miette::miette!(
            "{label} '{}' does not satisfy the v1 external-registration contract",
            path.display()
        ));
    }
    Ok(Some(MeasuredRegistration {
        identity,
        sha256: sha256_bytes(&bytes).to_string(),
        receipt,
    }))
}
