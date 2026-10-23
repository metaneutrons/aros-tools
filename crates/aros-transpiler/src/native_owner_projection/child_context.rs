//! Match only declared child goal/input/cwd coordinates, never execution proof.

use super::{NativeOwnerProjection, OwnerSelectionEvidence};
use aros_common::{
    make_native_invocation::NativeMakeInvocation,
    make_startup_arguments::LiteralMakeStartupArguments,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// Coordinates matched against one revalidated source-owner selection.
///
/// This does not bind parent identity, executable/argv0, environment, preload
/// inputs, phase, capture age, input bytes or success. It cannot admit a recipe
/// or satisfy an absent product. Multiple owners remain independent matches.
pub struct MatchedOwnerChildCoordinates {
    generated_owner: String,
    source_input: String,
    goals: Vec<String>,
}

impl MatchedOwnerChildCoordinates {
    #[must_use]
    pub fn generated_owner(&self) -> &str {
        &self.generated_owner
    }

    #[must_use]
    pub fn source_input(&self) -> &str {
        &self.source_input
    }

    /// Original observed goal order and repetitions, not metadata sort order.
    #[must_use]
    pub fn goals(&self) -> &[String] {
        &self.goals
    }
}

fn literal_path(path: &Path, absolute: bool) -> bool {
    path.is_absolute() == absolute
        && !path.as_os_str().is_empty()
        && path.as_os_str().len() <= 16 * 1024
        && path.components().all(|part| {
            matches!(part, Component::Normal(_)) || (absolute && part == Component::RootDir)
        })
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

fn retain_match(
    owner: &str,
    source: &str,
    goals: &[String],
    remaining_bytes: &mut usize,
) -> Result<MatchedOwnerChildCoordinates, String> {
    let cost = goals
        .iter()
        .try_fold(
            owner.len() + source.len() + std::mem::size_of::<MatchedOwnerChildCoordinates>(),
            |cost, goal| cost.checked_add(goal.len() + std::mem::size_of::<String>()),
        )
        .ok_or("child coordinate result byte bound overflow")?;
    *remaining_bytes = remaining_bytes
        .checked_sub(cost)
        .ok_or("child coordinate batch exceeds result byte bound")?;
    Ok(MatchedOwnerChildCoordinates {
        generated_owner: owner.to_owned(),
        source_input: source.to_owned(),
        goals: goals.to_owned(),
    })
}

const fn initial_result_budget() -> usize {
    16 * 1024 * 1024 - std::mem::size_of::<Vec<MatchedOwnerChildCoordinates>>()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CoordinateCoverage {
    Membership,
    ClosedDependencies,
    ObservedOwners,
}

impl NativeOwnerProjection {
    /// Match an observed child to a selected generated owner and reached goals.
    ///
    /// Revalidates this source projection and recomputes selection internally;
    /// a caller-forged public JSON report is not accepted as the authority.
    /// Exactly one explicit Make input must name the selected owner from its
    /// literal build cwd. No ambient/default Makefile or basename-only owner
    /// inference. Implicit goals and extra Makefiles refuse; actual goal order
    /// and repetitions survive. No process is started or state published.
    ///
    /// # Errors
    /// Refuses source drift, foreign/unselected coordinates, unknown/empty
    /// goals, unsupported literal startup forms and existing selection bounds.
    /// Errors never contain supplied argument, path or environment values.
    pub fn match_captured_child_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        generated_owner: &str,
        child: &NativeMakeInvocation,
    ) -> Result<MatchedOwnerChildCoordinates, String> {
        self.match_captured_children_coordinates(
            source_root,
            roots,
            build_root,
            &[(generated_owner, child)],
        )?
        .pop()
        .ok_or_else(|| "child coordinate result is unavailable".into())
    }

    /// Match a complete ordered observation batch with one metadata selection.
    ///
    /// Revalidates source inputs before and after the complete batch. This
    /// bounds4096 children and16 MiB of returned coordinate data before cloning.
    /// No partial result or per-child source-discovery copy is published.
    /// Duplicate observations retain their positions, not scheduling meaning.
    /// The same limited, non-execution scope as the scalar matcher applies.
    ///
    /// # Errors
    /// Refuses any scalar mismatch, source drift or aggregate count/byte limit.
    pub fn match_captured_children_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        children: &[(&str, &NativeMakeInvocation)],
    ) -> Result<Vec<MatchedOwnerChildCoordinates>, String> {
        self.match_children_coordinates(
            source_root,
            roots,
            build_root,
            children,
            CoordinateCoverage::Membership,
        )
    }

    /// Match exactly one occurrence of every selected owner/goal coordinate.
    ///
    /// Selection is recomputed from this source projection, never supplied by
    /// the capture. Unresolved endpoints refuse after proving optional Source
    /// template leaves; those absent leaves do not create expected child pairs.
    /// The Source-join owner separately retains that omission evidence. Missing
    /// pairs, repeated pairs within or
    /// across children, and unselected pairs refuse. Grouping and observed order
    /// are retained, not inferred from the unordered metadata relation.
    ///
    /// This is a deliberately bounded exact-once coordinate check, NOT a proof
    /// of native scheduling semantics: callers must separately establish that
    /// their selected invocation uses this dispatch model. Repeated root
    /// execution is outside this model. It also does not prove parent,
    /// executable, environment, phase, generated bytes or successful recipes.
    /// An empty, fully known virtual selection may match an empty batch.
    ///
    /// # Errors
    /// Refuses every ordinary batch mismatch, unresolved endpoint, incomplete
    /// coverage or repeated owner/goal coordinate. No partial result escapes.
    pub fn match_complete_child_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        children: &[(&str, &NativeMakeInvocation)],
    ) -> Result<Vec<MatchedOwnerChildCoordinates>, String> {
        self.match_children_coordinates(
            source_root,
            roots,
            build_root,
            children,
            CoordinateCoverage::ClosedDependencies,
        )
    }

    /// Match exactly every existing owner/goal reached by known requested roots.
    ///
    /// Native MetaMake skips absent dependency nodes but rejects absent requested
    /// roots. This data-only check preserves that distinction; it does not prove
    /// dependency completeness or grant an absent producer. Callers must retain
    /// the separate Source metadata selection, including unresolved endpoints.
    /// Use [`Self::match_complete_child_coordinates`] when closed dependencies
    /// are required. All coordinate, exact-once, input and resource checks remain.
    ///
    /// # Errors
    /// Refuses empty/unknown requested roots, missing/repeated/unselected owner
    /// coordinates, input drift or the existing count/byte bounds.
    pub fn match_observed_owner_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        children: &[(&str, &NativeMakeInvocation)],
    ) -> Result<Vec<MatchedOwnerChildCoordinates>, String> {
        self.match_children_coordinates(
            source_root,
            roots,
            build_root,
            children,
            CoordinateCoverage::ObservedOwners,
        )
    }

    /// Select required `(Source input, observed goal)` pairs from one complete
    /// child-coordinate batch, returning each pair's original child slot.
    ///
    /// The complete roster is matched and coverage-checked exactly once before
    /// the requirements are joined. Requirements are exact strings: no path or
    /// goal normalization is performed. Distinct goals in one observed child
    /// may select the same slot; a repeated requirement, absent pair, or pair
    /// found in multiple children refuses. This is a coordinate join only; it
    /// does not prove execution, scheduling, or recipe effects.
    ///
    /// # Errors
    /// Uses [`Self::match_observed_owner_coordinates`], not closed dependency
    /// validation. Unresolved dependency metadata must remain separately visible.
    /// Refuses oversized/duplicate requirements, observed-owner mismatches or
    /// any ordered requirement join error.
    pub fn select_required_child_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        children: &[(&str, &NativeMakeInvocation)],
        required: &[(String, String)],
    ) -> Result<Vec<usize>, String> {
        if required.len() > 4096 {
            return Err("required child coordinate batch exceeds count bound".into());
        }

        let mut remaining_bytes = initial_result_budget();
        for (source_input, goal) in required {
            if source_input.is_empty() || goal.is_empty() {
                return Err("required child coordinate is empty".into());
            }
            let cost = source_input
                .len()
                .checked_add(goal.len())
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>()))
                .ok_or("required child coordinate byte bound overflow")?;
            remaining_bytes = remaining_bytes
                .checked_sub(cost)
                .ok_or("required child coordinate batch exceeds byte bound")?;
        }

        let mut matches = BTreeMap::new();
        for (source_input, goal) in required {
            if matches
                .insert((source_input.as_str(), goal.as_str()), None)
                .is_some()
            {
                return Err("required child coordinate repeats".into());
            }
        }

        let coordinates =
            self.match_observed_owner_coordinates(source_root, roots, build_root, children)?;
        for (slot, coordinate) in coordinates.iter().enumerate() {
            for goal in &coordinate.goals {
                if let Some(selected) =
                    matches.get_mut(&(coordinate.source_input.as_str(), goal.as_str()))
                {
                    if selected.replace(slot).is_some() {
                        return Err("required child coordinate is ambiguous".into());
                    }
                }
            }
        }
        required
            .iter()
            .map(|(source_input, goal)| {
                matches[&(source_input.as_str(), goal.as_str())]
                    .ok_or_else(|| "required child coordinate was not observed".into())
            })
            .collect()
    }

    fn match_children_coordinates(
        &self,
        source_root: &Path,
        roots: &[String],
        build_root: &Path,
        children: &[(&str, &NativeMakeInvocation)],
        coverage: CoordinateCoverage,
    ) -> Result<Vec<MatchedOwnerChildCoordinates>, String> {
        if children.len() > 4096 {
            return Err("child coordinate batch exceeds count bound".into());
        }
        if !literal_path(build_root, true) {
            return Err("child coordinate spelling is unsupported".into());
        }
        self.verify(source_root)
            .map_err(|_| "child source projection revalidation failed")?;
        let complete = coverage != CoordinateCoverage::Membership;
        let selection = if complete {
            self.select_source_metadata(source_root, roots)
                .map_err(|_| "child source metadata selection refused")?
                .into_selection()
        } else {
            self.select(roots)
                .map_err(|_| "child source selection refused")?
        };
        if coverage == CoordinateCoverage::ClosedDependencies
            && !selection.missing_endpoints.is_empty()
        {
            return Err("child source selection has unresolved endpoints".into());
        }
        if coverage == CoordinateCoverage::ObservedOwners
            && (roots.is_empty()
                || roots
                    .iter()
                    .any(|root| selection.missing_endpoints.contains(root)))
        {
            return Err("observed owner selection requires known requested roots".into());
        }
        let mut remaining_bytes = initial_result_budget();
        let mut results = Vec::with_capacity(children.len());
        for (owner, child) in children {
            results.push(self.match_child_coordinates_inner(
                &selection,
                build_root,
                owner,
                child,
                &mut remaining_bytes,
            )?);
        }
        if complete {
            require_exact_coordinate_coverage(&selection.selected_owner_targets, &results)?;
        }
        self.verify(source_root)
            .map_err(|_| "child source projection changed during batch")?;
        Ok(results)
    }

    fn match_child_coordinates_inner(
        &self,
        selection: &OwnerSelectionEvidence,
        build_root: &Path,
        generated_owner: &str,
        child: &NativeMakeInvocation,
        remaining_bytes: &mut usize,
    ) -> Result<MatchedOwnerChildCoordinates, String> {
        let owner_path = Path::new(generated_owner);
        if !literal_path(owner_path, false) {
            return Err("child coordinate spelling is unsupported".into());
        }
        let selected = selection
            .selected_owner_targets
            .get(generated_owner)
            .ok_or("child generated owner is not selected")?;
        let parent = owner_path.parent().ok_or("child owner has no directory")?;
        if child.working_directory() != build_root.join(parent) {
            return Err("child cwd differs from selected owner directory".into());
        }
        let startup = LiteralMakeStartupArguments::admit(child.arguments())
            .map_err(|_| "child startup arguments need additional effect modeling")?;
        if startup.input_files().len() != 1 {
            return Err("child requires exactly one explicit selected Makefile".into());
        }
        let input = Path::new(&startup.input_files()[0]);
        if !literal_path(input, input.is_absolute())
            || child.working_directory().join(input) != build_root.join(owner_path)
        {
            return Err("child Makefile differs from selected owner".into());
        }
        if startup.goals().is_empty() || startup.goals().iter().any(|goal| !selected.contains(goal))
        {
            return Err("child goals are not explicit reached owner targets".into());
        }
        let source_input = self
            .owners
            .get(generated_owner)
            .ok_or("child selected owner lacks bound source input")?;
        retain_match(
            generated_owner,
            source_input,
            startup.goals(),
            remaining_bytes,
        )
    }
}

fn require_exact_coordinate_coverage(
    expected: &BTreeMap<String, BTreeSet<String>>,
    children: &[MatchedOwnerChildCoordinates],
) -> Result<(), String> {
    // Borrow retained strings: the ordinary batch byte/count limits already
    // apply, and coverage does not clone another owner/goal corpus.
    let mut seen = BTreeSet::new();
    for child in children {
        for goal in &child.goals {
            if !seen.insert((child.generated_owner.as_str(), goal.as_str())) {
                return Err("child coordinate coverage repeats an owner target".into());
            }
        }
    }
    if expected.iter().any(|(owner, goals)| {
        goals
            .iter()
            .any(|goal| !seen.contains(&(owner.as_str(), goal.as_str())))
    }) {
        return Err("child coordinate coverage omits a selected owner target".into());
    }
    // Per-child matching already rejects every unselected coordinate.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_coordinate_budget_is_exact_shared_and_charged_before_copying() {
        assert_eq!(
            initial_result_budget() + std::mem::size_of::<Vec<MatchedOwnerChildCoordinates>>(),
            16 * 1024 * 1024,
        );
        let goals = vec!["second".to_owned(), "first".to_owned(), "second".to_owned()];
        let cost = "owner".len()
            + "source".len()
            + std::mem::size_of::<MatchedOwnerChildCoordinates>()
            + goals
                .iter()
                .map(|goal| goal.len() + std::mem::size_of::<String>())
                .sum::<usize>();
        let mut exact = cost;
        let retained = retain_match("owner", "source", &goals, &mut exact).unwrap();
        assert_eq!(exact, 0);
        assert_eq!(retained.goals(), goals);
        assert!(retain_match("owner", "source", &goals, &mut exact).is_err());
        assert_eq!(exact, 0);
        let mut short = cost - 1;
        assert!(retain_match("owner", "source", &goals, &mut short).is_err());
        assert_eq!(short, cost - 1);
        let mut shared = 2 * cost;
        assert!(retain_match("owner", "source", &goals, &mut shared).is_ok());
        assert!(retain_match("owner", "source", &goals, &mut shared).is_ok());
        assert!(retain_match("owner", "source", &goals, &mut shared).is_err());
        assert_eq!(shared, 0);
    }
}
