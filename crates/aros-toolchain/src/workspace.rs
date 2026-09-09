//! Fresh, advisory-locked work/output directories, not build authorization.
//!
//! This lower-level M1 primitive is not called by read-only planning. It does
//! not verify recipes, executor origin or source snapshots, launch commands,
//! publish candidates or resume work. Callers must preflight the intended
//! operation; snapshot preparation repeats its own source preflight before
//! staging. Builds need all additional readiness gates. Callers must revalidate
//! before and after every mutation boundary.
//! Existing directories (even empty ones) are never adopted. Failed or dropped
//! reservations retain their directories; no user tree is a cleanup target.

use std::path::Path;

use aros_common::{CancellationToken, Sha256Digest};

use crate::plan::{self, Paths, PlanRequest};
use crate::ContractError;

#[cfg(unix)]
mod directory_lock;
#[cfg(unix)]
mod unix;

/// Held directory locks and identities for one fresh local operation.
///
/// The owning process must retain this guard for the whole operation. Locks
/// are advisory and close-on-exec; a same-user hostile process is not sandboxed.
/// Call `release` to report unlock failures after all child activity has ended;
/// Drop performs fallback unlocking and logs failures, never deletes data.
/// This record is neither a phase receipt nor permission to execute a plan.
#[derive(Debug)]
pub struct RunDirectories {
    paths: Paths,
    #[cfg(unix)]
    work: unix::OwnedDirectory,
    #[cfg(unix)]
    output: unix::OwnedDirectory,
}

impl RunDirectories {
    #[cfg(unix)]
    pub(crate) const fn work_file(&self) -> &std::fs::File {
        self.work.file()
    }

    /// Reserve only the final work/output leaves under existing parents.
    ///
    /// `owner` binds these directories to a caller-measured operation identity;
    /// it is not origin evidence and cannot authorize reuse. All original root
    /// selections are resolved afresh, not accepted from a serialized plan.
    /// No cache/source directory is created or modified. On a partial failure,
    /// any newly created directories are retained and the failure is explicit.
    ///
    /// # Errors
    /// Returns AX0101/AX0202 for invalid selections before mutation, AX0801 for
    /// cancellation, existing/unsafe roots, locking or identity failures.
    pub fn reserve(
        request: &PlanRequest,
        owner: &Sha256Digest,
        cancellation: &CancellationToken,
    ) -> Result<Self, ContractError> {
        plan::validate_resources(request)?;
        if request.work_dir.is_none()
            || request.output_dir.is_none()
            || request.cache_dir.is_none()
            || request.jobs.is_none()
            || request.timeout_seconds.is_none()
        {
            return Err(ContractError::preflight(
                "reservation requires explicit work/output/cache roots and resource budgets",
            ));
        }
        check_cancellation(cancellation)?;
        let paths = plan::resolve_paths(request)?;
        #[cfg(not(unix))]
        {
            let _ = (paths, owner);
            Err(ContractError::state(
                "work ownership requires a supported Unix host",
            ))
        }
        #[cfg(unix)]
        {
            let work = paths
                .work
                .as_deref()
                .ok_or_else(|| ContractError::state("missing work root"))?;
            let output = paths
                .output
                .as_deref()
                .ok_or_else(|| ContractError::state("missing output root"))?;
            // Resolve and reject both occupied leaves before the first mkdir.
            let work_parent = unix::Parent::inspect(work)?;
            let output_parent = unix::Parent::inspect(output)?;
            check_cancellation(cancellation)?;
            let work = work_parent.reserve(owner, "work")?;
            check_cancellation(cancellation)?;
            let output = output_parent.reserve(owner, "output")?;
            let result = Self {
                paths,
                work,
                output,
            };
            result.revalidate(cancellation)?;
            Ok(result)
        }
    }

    /// Reacquire exactly the two retained roots of an explicitly requested
    /// local resume.
    ///
    /// This is intentionally separate from [`Self::reserve`]: callers must
    /// opt in, supply the same owner identity, and perform their own receipt
    /// and phase-output revalidation before executing anything. It never
    /// adopts an arbitrary directory, removes retained state, or relaxes the
    /// fresh-root rule for ordinary and release builds.
    ///
    /// # Errors
    ///
    /// Returns AX0202/AX0801 when selections are invalid, cancellation is
    /// requested, either retained root is not private and lockable, or its
    /// exact ownership marker does not match the requested operation.
    pub fn resume(
        request: &PlanRequest,
        owner: &Sha256Digest,
        cancellation: &CancellationToken,
    ) -> Result<Self, ContractError> {
        plan::validate_resources(request)?;
        if request.work_dir.is_none()
            || request.output_dir.is_none()
            || request.cache_dir.is_none()
            || request.jobs.is_none()
            || request.timeout_seconds.is_none()
        {
            return Err(ContractError::preflight(
                "resume requires explicit work/output/cache roots and resource budgets",
            ));
        }
        check_cancellation(cancellation)?;
        let paths = plan::resolve_paths(request)?;
        #[cfg(not(unix))]
        {
            let _ = (paths, owner);
            Err(ContractError::state(
                "work ownership requires a supported Unix host",
            ))
        }
        #[cfg(unix)]
        {
            let work = paths
                .work
                .as_deref()
                .ok_or_else(|| ContractError::state("missing work root"))?;
            let output = paths
                .output
                .as_deref()
                .ok_or_else(|| ContractError::state("missing output root"))?;
            let work = unix::Parent::resume(work, owner, "work")?;
            check_cancellation(cancellation)?;
            let output = unix::Parent::resume(output, owner, "output")?;
            let result = Self {
                paths,
                work,
                output,
            };
            result.revalidate(cancellation)?;
            Ok(result)
        }
    }

    /// Recheck the original namespace, root/marker identities, bytes and locks.
    ///
    /// # Errors
    /// Returns AX0801 when cancelled or any reservation has changed. This is a
    /// boundary check, not OS isolation between checks or a content-tree audit.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        check_cancellation(cancellation)?;
        #[cfg(unix)]
        {
            self.work.revalidate()?;
            self.output.revalidate()?;
            Ok(())
        }
        #[cfg(not(unix))]
        Err(ContractError::state(
            "work ownership requires a supported Unix host",
        ))
    }

    /// Explicitly release both locks without modifying any directory contents.
    ///
    /// Call only after all work and child-process cleanup has completed. This
    /// consumes ownership even on failure; neither a cancelled operation nor a
    /// changed namespace may skip releasing the original held descriptors.
    ///
    /// # Errors
    /// Returns AX0801 for any unlock failure. Both roots are attempted; Drop
    /// makes a final fallback attempt and closes descriptors. A fallback must
    /// not turn the returned failure into a successful operation.
    pub fn release(mut self) -> Result<(), ContractError> {
        #[cfg(unix)]
        {
            let mut failures = Vec::new();
            if let Err(error) = self.work.release() {
                failures.push(format!("work directory unlock failed: {error}"));
            }
            if let Err(error) = self.output.release() {
                failures.push(format!("output directory unlock failed: {error}"));
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(ContractError::state(failures.join("; ")))
            }
        }
        #[cfg(not(unix))]
        Err(ContractError::state(
            "work ownership requires a supported Unix host",
        ))
    }

    /// Resolved selections. Observing these paths does not transfer ownership.
    #[must_use]
    pub const fn paths(&self) -> &Paths {
        &self.paths
    }
}

fn check_cancellation(token: &CancellationToken) -> Result<(), ContractError> {
    if token.is_cancelled() {
        Err(ContractError::state(
            "operation cancelled; no further work may start",
        ))
    } else {
        Ok(())
    }
}

fn state_io(operation: &str, path: &Path, error: &std::io::Error) -> ContractError {
    ContractError::state(format!("{operation} '{}': {error}", path.display()))
}
