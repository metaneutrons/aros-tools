//! Literal output candidates between separately scoped selected child graphs.

use super::MatchedChildDeclaredRules;
use crate::native_owner_projection::NativeOwnerProjection;
use aros_common::Sha256Digest;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

const MAX_CHILDREN: usize = 4096;
const MAX_VISITS: usize = 262_144;
const MAX_PATH_BYTES: usize = 8192;
const MAX_INDEX_BYTES: usize = 64 * 1024 * 1024;

/// Independently selected common input context and allowed Source owner goals.
/// This data selection does not authenticate the native parent or its schedule.
pub struct DeclaredChildJoinSelection<'a> {
    pub source_root: &'a Path,
    pub source_inputs_sha256: &'a Sha256Digest,
    pub build_root: &'a Path,
    pub prepared_tree_sha256: &'a Sha256Digest,
    pub roots: &'a [String],
}

/// Two positions in separate original child traces, not a file/execution proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredChildOutputCandidate {
    pub consumer_child: usize,
    pub consumer_visit: usize,
    pub producer_child: usize,
    pub producer_visit: usize,
}

/// Closed selected child data with possible cross-child output relationships.
///
/// No graphs/variable frames are merged. A candidate means only that another
/// selected child's reached explicit rule has this exact literal output name
/// and a recipe. It does not prove the recipe writes that file, runs first or
/// succeeds. Phony/default/implicit policies, command/artifact binding, cycles
/// across children and scheduling remain separate obligations. No candidate
/// upgrades an external leaf to an admitted input or a strict recipe plan.
pub struct DeclaredChildOutputLinks {
    children: Vec<MatchedChildDeclaredRules>,
    candidates: Vec<DeclaredChildOutputCandidate>,
    unlinked_inputs: Vec<(usize, usize)>,
}

impl DeclaredChildOutputLinks {
    #[must_use]
    pub fn children(&self) -> &[MatchedChildDeclaredRules] {
        &self.children
    }
    #[must_use]
    pub fn candidates(&self) -> &[DeclaredChildOutputCandidate] {
        &self.candidates
    }
    /// Includes captured regular inputs: this layer makes no existence claims.
    #[must_use]
    pub fn unlinked_inputs(&self) -> &[(usize, usize)] {
        &self.unlinked_inputs
    }
    /// All child bindings were compared exactly; one common-root check covers
    /// the held inputs without rehashing the same roots once per object/owner.
    /// # Errors
    /// Refuses drift in any original shared input root.
    pub fn verify(&self) -> Result<(), String> {
        self.children[0].verify()
    }
}

impl NativeOwnerProjection {
    /// Relate literal reached output names within one exact Source selection.
    ///
    /// Every selected owner/goal must occur exactly once. All children must
    /// retain identical original input-root bindings (including metadata and
    /// measurement domains), while their cwd and variable frames stay separate.
    /// A caller cannot add an unselected producer to satisfy a missing input.
    /// Non-normalized/foreign output names stay unlinked, never normalized into
    /// physical identity or granted writable-path authority. No file is read or
    /// created to satisfy a missing leaf; this is a data-only candidate index.
    /// # Errors
    /// Refuses incomplete/repeated/foreign coordinates, input/context drift,
    /// ambiguous reached producers and aggregate storage/work bounds.
    pub fn link_declared_child_outputs(
        &self,
        selected: &DeclaredChildJoinSelection<'_>,
        children: Vec<MatchedChildDeclaredRules>,
    ) -> Result<DeclaredChildOutputLinks, String> {
        if children.is_empty() || children.len() > MAX_CHILDREN {
            return Err("declared child output join needs a bounded nonempty batch".into());
        }
        self.verify(selected.source_root)
            .map_err(|_| "declared child Source changed")?;
        let selection = self
            .select_source_metadata(selected.source_root, selected.roots)
            .map_err(|_| "declared child Source selection refused")?
            .into_selection();
        if !selection.missing_endpoints.is_empty() || !selection.unbound_native_endpoints.is_empty()
        {
            return Err("declared child Source selection is incomplete".into());
        }
        let first = &children[0].source;
        if first.build_root != selected.build_root
            || !first
                .snapshot
                .matches_root_selection(selected.source_root, selected.source_inputs_sha256)
            || !first
                .snapshot
                .matches_root_selection(selected.build_root, selected.prepared_tree_sha256)
        {
            return Err("declared child inputs differ from selected common context".into());
        }
        first.snapshot.verify()?;
        let mut seen = BTreeSet::new();
        let mut visits = 0usize;
        for child in &children {
            let coordinate = child.coordinates();
            let owner = coordinate.generated_owner();
            let allowed = selection
                .selected_owner_targets
                .get(owner)
                .ok_or("declared child producer owner is not selected")?;
            if self.owners.get(owner).map(String::as_str) != Some(coordinate.source_input())
                || child.source.build_root != selected.build_root
                || !first.snapshot.same_root_bindings(&child.source.snapshot)
                || child.source.snapshot.working_directory()
                    != selected.build_root.join(
                        Path::new(owner)
                            .parent()
                            .ok_or("declared child owner has no directory")?,
                    )
            {
                return Err("declared child owner or original input bindings differ".into());
            }
            for goal in coordinate.goals() {
                if !allowed.contains(goal) || !seen.insert((owner, goal.as_str())) {
                    return Err("declared child goal is unselected or repeated".into());
                }
            }
            visits = visits
                .checked_add(child.traversal.visits().len())
                .filter(|count| *count <= MAX_VISITS)
                .ok_or("declared child output join exceeds visit limit")?;
        }
        if selection
            .selected_owner_targets
            .iter()
            .any(|(owner, goals)| {
                goals
                    .iter()
                    .any(|goal| !seen.contains(&(owner.as_str(), goal.as_str())))
            })
        {
            return Err("declared child output join omits a selected owner goal".into());
        }
        let mut outputs = BTreeMap::new();
        let mut storage = 0usize;
        for (child_index, child) in children.iter().enumerate() {
            for (visit_index, visit) in child.traversal.visits().iter().enumerate() {
                if visit.instance().is_none_or(|rule| rule.recipes.is_empty()) {
                    continue;
                }
                let Some(path) = literal_output(child, visit.target(), selected.build_root) else {
                    continue;
                };
                storage = storage
                    .checked_add(path.as_os_str().len() + 128)
                    .filter(|bytes| *bytes <= MAX_INDEX_BYTES)
                    .ok_or("declared child output index exceeds storage limit")?;
                if outputs.insert(path, (child_index, visit_index)).is_some() {
                    return Err("declared child output has ambiguous reached producers".into());
                }
            }
        }
        let mut candidates = Vec::new();
        let mut unlinked_inputs = Vec::new();
        for (consumer_child, child) in children.iter().enumerate() {
            for (consumer_visit, visit) in child.traversal.unresolved_inputs() {
                let producer = literal_output(child, visit.target(), selected.build_root)
                    .and_then(|path| outputs.get(&path).copied())
                    .filter(|(producer_child, _)| *producer_child != consumer_child);
                if let Some((producer_child, producer_visit)) = producer {
                    candidates.push(DeclaredChildOutputCandidate {
                        consumer_child,
                        consumer_visit,
                        producer_child,
                        producer_visit,
                    });
                } else {
                    unlinked_inputs.push((consumer_child, consumer_visit));
                }
            }
        }
        first.snapshot.verify()?;
        self.verify(selected.source_root)
            .map_err(|_| "declared child Source changed during join")?;
        Ok(DeclaredChildOutputLinks {
            children,
            candidates,
            unlinked_inputs,
        })
    }
}

pub(super) fn literal_output(
    child: &MatchedChildDeclaredRules,
    target: &str,
    build: &Path,
) -> Option<PathBuf> {
    if target.is_empty() || target.len() > MAX_PATH_BYTES || target.chars().any(char::is_control) {
        return None;
    }
    let input = Path::new(target);
    if input
        .components()
        .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        || input.components().collect::<PathBuf>().as_os_str() != input.as_os_str()
    {
        return None;
    }
    let path = if input.is_absolute() {
        input.to_owned()
    } else {
        child.source.snapshot.working_directory().join(input)
    };
    (path.as_os_str().len() <= MAX_PATH_BYTES && path.starts_with(build) && path != build)
        .then_some(path)
}
