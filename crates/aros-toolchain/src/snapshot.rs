//! Metadata-free, no-clobber source material under held work ownership.
//!
//! This library primitive is not called by planning and does not authorize a
//! build. It copies raw Git objects, never working files, filters or hardlinks.
//! Each selected root is prepared independently; a future executor must hold
//! and revalidate all three snapshots plus its separate readiness evidence.

use std::path::Path;
use std::time::{Duration, Instant};

use aros_common::CancellationToken;

use crate::{recipe::GitObjectId, workspace::RunDirectories, ContractError, Recipe};

#[cfg(unix)]
mod links;
#[cfg(unix)]
mod unix;

/// Exactly one recipe input; arbitrary destination paths are not accepted.
#[derive(Debug, Clone, Copy)]
pub enum SourceRole {
    /// AROS source and recursively selected modules.
    Source,
    /// Producer rules and metadata.
    Producer,
    /// Recipe-selected collector/tools, not frontend origin evidence.
    Tools,
}

impl SourceRole {
    fn selection<'a>(
        self,
        run: &'a RunDirectories,
        recipe: &'a Recipe,
    ) -> (&'a Path, (&'a GitObjectId, &'a GitObjectId)) {
        match self {
            Self::Source => (&run.paths().source, recipe.source()),
            Self::Producer => (&run.paths().producer, recipe.producer()),
            Self::Tools => (&run.paths().tools, recipe.tools()),
        }
    }

    const fn leaf(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Producer => "producer",
            Self::Tools => "tools",
        }
    }
}

/// An in-process material inventory borrowing the owning run guard.
///
/// Not serializable, resumable, an origin attestation or a same-user sandbox.
/// Drop retains all files. The borrow prevents releasing ownership while this
/// value is live; callers must still revalidate at every use boundary. Paths
/// copied out of this value do not carry any verification guarantee.
pub struct SourceSnapshot<'run> {
    run: &'run RunDirectories,
    #[cfg(unix)]
    material: unix::Material,
}

impl<'run> SourceSnapshot<'run> {
    /// Inspect one complete input, then copy and re-inspect committed material.
    ///
    /// Requires a fresh reservation and a positive explicit operation timeout.
    /// Git children share that deadline (at most ten seconds each); filesystem
    /// loops check cancellation/deadline between bounded entries/chunks. Kernel
    /// filesystem I/O/fsync itself is not preemptible. Symlinks must resolve
    /// wholly inside this input without link-expansion cycles or missing targets.
    /// No `.git` entries survive, including recursively initialized gitlinks.
    ///
    /// Success provides only source material, not recipe/lock semantics, trusted
    /// Git-object origin, executor identity, cache readiness or build permission.
    /// A failure retains any private `.<role>-pending` or complete `<role>` tree;
    /// neither is adopted by a retry, and other inputs/output/cache are untouched.
    ///
    /// # Errors
    /// Returns shared AX diagnostics for invalid identities/material, unsafe
    /// links, timeout/cancellation, destination conflicts or durability failure.
    pub fn prepare(
        run: &'run RunDirectories,
        role: SourceRole,
        recipe: &Recipe,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<Self, ContractError> {
        let deadline = deadline(timeout)?;
        run.revalidate(cancellation)?;
        #[cfg(unix)]
        {
            let material = unix::prepare(run, role, recipe, deadline, cancellation)
                .map_err(ContractError::retained_material)?;
            Ok(Self { run, material })
        }
        #[cfg(not(unix))]
        {
            let _ = (role, recipe, deadline);
            Err(ContractError::state(
                "source snapshots require a supported Unix host",
            ))
        }
    }

    /// Re-read every path, raw byte, executable bit and link target, without Git.
    ///
    /// Original checkout edits cannot change this material. This checks the
    /// held work/output ownership plus the snapshot root identity and exact
    /// inventory. It does not prevent same-user edits between checks.
    ///
    /// # Errors
    /// Returns AX diagnostics on changed material/ownership or exhausted budgets.
    pub fn revalidate(
        &self,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        let deadline = deadline(timeout)?;
        self.run.revalidate(cancellation)?;
        #[cfg(unix)]
        self.material
            .revalidate(deadline, cancellation)
            .map_err(ContractError::retained_material)?;
        self.run.revalidate(cancellation)
    }

    /// The complete retained snapshot; observing a path is not a use guard.
    #[cfg(unix)]
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.material.root
    }
}

fn deadline(timeout: Duration) -> Result<Instant, ContractError> {
    if timeout.is_zero() {
        return Err(ContractError::state(
            "source operation timeout must be positive",
        ));
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| ContractError::state("source operation timeout is not representable"))
}
