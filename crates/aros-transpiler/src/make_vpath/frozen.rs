//! Final global directory-search state, separate from target variable views.

use super::{directory_words, SearchPaths, MAX_BYTES, MAX_ENTRIES};
use crate::{
    make_rule_graph::traversal::DeclaredTrace,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
};
use aros_common::make_input_snapshot::MakeInputReader;
use std::path::{Path, PathBuf};

/// A declared external leaf and its current regular-file observation.
///
/// `None` remains unresolved, not an admitted missing/generated input. Even a
/// present file does not prove native selection, timestamp/remake behavior,
/// logical recipe spelling, complete implicit search or execution authority.
#[derive(Debug)]
pub struct DeclaredInputMembership {
    visit: usize,
    observed: Option<PathBuf>,
}

impl DeclaredInputMembership {
    #[must_use]
    pub const fn visit(&self) -> usize {
        self.visit
    }

    #[must_use]
    pub fn observed(&self) -> Option<&Path> {
        self.observed.as_deref()
    }
}

/// Read-time selective paths plus one final-global VPATH and explicit cwd.
///
/// GNU Make builds its general search list before target traversal. Target
/// assignments may change recipe-visible VPATH but do not rebuild that list.
/// No API here accepts a target-local replacement. The caller must freeze only
/// after the complete selected source read, and independently bind that read
/// and native process. This type does not authenticate either of them.
pub struct FrozenDirectorySearch {
    paths: SearchPaths,
    global_vpath: String,
    working_directory: PathBuf,
}

impl SearchPaths {
    /// Consume positioned selective entries and freeze the final global VPATH.
    ///
    /// Uses the pure global frame, never an inherited target context or guessed
    /// empty value. Filesystem/shell/eval-dependent values refuse. The existing
    /// shared interpreter work and path-value bounds apply. No roots are read.
    ///
    /// # Errors
    /// Refuses unmeasured/unsupported global expressions, invalid path data,
    /// mismatched program coordinates and exhausted bounds.
    pub fn freeze_global(
        self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<FrozenDirectorySearch, String> {
        if !paths.working_directory.is_absolute() {
            return Err("Make directory search requires absolute cwd".into());
        }
        let mut groups = program
            .evaluate_target_contexts(&[Vec::new()], &["$(strip $(VPATH))"], paths)
            .map_err(|error| error.to_string())?;
        let global_vpath = groups.remove(0).remove(0);
        // Validate now, not after a direct candidate happened to match.
        directory_words(&global_vpath)?;
        Ok(FrozenDirectorySearch {
            paths: self,
            global_vpath,
            working_directory: paths.working_directory.to_owned(),
        })
    }
}

impl FrozenDirectorySearch {
    fn check_reader(&self, reader: &MakeInputReader<'_>) -> Result<(), String> {
        if reader.working_directory() != self.working_directory {
            return Err("Make directory search reader cwd differs from frozen context".into());
        }
        Ok(())
    }

    /// The frozen data value; retaining or displaying it is the caller's policy.
    #[must_use]
    pub fn global_vpath(&self) -> &str {
        &self.global_vpath
    }

    /// Observe one literal input within the reader's verified transaction.
    ///
    /// # Errors
    /// Refuses a foreign cwd and propagates the strict captured-regular policy,
    /// unsafe/changed inputs and shared reader/search resource limits.
    pub fn find_regular_input(
        &self,
        file: &str,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<Option<PathBuf>, String> {
        self.check_reader(reader)?;
        self.paths
            .find_regular_input(file, &self.global_vpath, reader)
    }

    /// Observe the unresolved frontier without converting it into an admission.
    ///
    /// Rows retain the exact trace visit indices, not a basename lookup or a
    /// separately invented goal order. No recipe runs and no partial batch is
    /// returned. Caller must propagate errors from this transaction.
    ///
    /// # Errors
    /// Refuses unsafe/unbound candidates, foreign cwd, input changes, aggregate
    /// result limits and exhausted shared reader budgets.
    pub fn observe_declared_inputs(
        &self,
        trace: &DeclaredTrace,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<Vec<DeclaredInputMembership>, String> {
        self.check_reader(reader)?;
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        for (visit, input) in trace.unresolved_inputs() {
            if rows.len() == MAX_ENTRIES {
                return Err("Make declared input search exceeds result count limit".into());
            }
            let observed = self.find_regular_input(input.target(), reader)?;
            bytes = bytes
                .checked_add(std::mem::size_of::<DeclaredInputMembership>())
                .and_then(|bytes| {
                    bytes.checked_add(observed.as_ref().map_or(0, |path| path.as_os_str().len()))
                })
                .filter(|bytes| *bytes <= MAX_BYTES)
                .ok_or("Make declared input search exceeds result byte limit")?;
            rows.push(DeclaredInputMembership { visit, observed });
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests;
