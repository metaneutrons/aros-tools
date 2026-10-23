//! Source-proven optional extension leaves, not general missing-target tolerance.

use super::{NativeOwnerProjection, OwnerSelectionEvidence};
use crate::arch_endpoint_effects::{
    collect_source_arch_endpoint_effects, ArchEndpointEffect, ArchEndpointEffectData,
    RejectedArchEndpointEffect,
};
use crate::native_owner_projection::meta_semantics::VerifiedSelectorContract;
use crate::native_owner_projection::meta_semantics::VerifiedTemplateHook;
use aros_common::native_build_contract::NativeMetaAbsence;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_WORK: usize = 4_000_000;
const MAX_PROOFS: usize = 20_000;

/// An absent leaf whose every reached incoming declaration is a verified
/// Source-template extension slot. This does not create a producer or owner.
#[derive(Debug, Serialize)]
pub struct SourceMetadataHookOmission {
    pub dependency: String,
    pub proofs: BTreeSet<VerifiedTemplateHook>,
}

/// Every reached incoming edge is separately proven by a template or an exact
/// Source-contract selector. This record retains the selector part only.
#[derive(Debug, Serialize)]
pub struct SourceMetadataSelectorOmission {
    pub dependency: String,
    pub proofs: BTreeSet<VerifiedSelectorContract>,
}

#[derive(Default)]
struct OmissionProofs {
    templates: BTreeSet<VerifiedTemplateHook>,
    selectors: BTreeSet<VerifiedSelectorContract>,
}

/// Revalidated metadata selection with explicit, Source-proven leaf omissions.
/// No process, scheduling, recipe success or product is established by this.
#[derive(Debug, Serialize)]
pub struct SourceMetadataSelection {
    selection: OwnerSelectionEvidence,
    omissions: Vec<SourceMetadataHookOmission>,
    selector_omissions: Vec<SourceMetadataSelectorOmission>,
    architecture_effects: Vec<ArchEndpointEffect>,
    architecture_rejections: Vec<RejectedArchEndpointEffect>,
}

impl SourceMetadataSelection {
    #[must_use]
    pub const fn selection(&self) -> &OwnerSelectionEvidence {
        &self.selection
    }

    #[must_use]
    pub fn omissions(&self) -> &[SourceMetadataHookOmission] {
        &self.omissions
    }

    #[must_use]
    pub fn selector_omissions(&self) -> &[SourceMetadataSelectorOmission] {
        &self.selector_omissions
    }

    /// Independently replayed selected Source effects. These establish only
    /// metadata semantics, never that a producer ran or its output exists.
    #[must_use]
    pub fn architecture_effects(&self) -> &[ArchEndpointEffect] {
        &self.architecture_effects
    }

    /// Refused selected Source effects; these records grant no authority.
    #[must_use]
    pub fn architecture_rejections(&self) -> &[RejectedArchEndpointEffect] {
        &self.architecture_rejections
    }

    pub(super) fn into_selection(self) -> OwnerSelectionEvidence {
        self.selection
    }
}

impl NativeOwnerProjection {
    /// Select original Source metadata, proving only optional generated leaves.
    ///
    /// Unknown requested roots, handwritten edges, real-owner collisions and
    /// unsupported or changed templates remain unresolved. Each occurrence of
    /// an incoming edge must independently qualify, including duplicate and
    /// cross-owner declarations. No absent endpoint becomes a known target.
    ///
    /// # Errors
    /// Refuses stale Source inputs, invalid template provenance or bounded-work
    /// excess. Other unresolved endpoints remain explicit in the selection.
    pub fn select_source_metadata(
        &self,
        source_root: &Path,
        roots: &[String],
    ) -> Result<SourceMetadataSelection, String> {
        if roots.len() > MAX_PROOFS {
            return Err("Source metadata selection exceeds root budget".into());
        }
        self.verify(source_root)?;
        let mut selection = self.select(roots)?;
        let requested: BTreeSet<_> = roots.iter().map(String::as_str).collect();
        let mut candidates = BTreeMap::<String, Option<OmissionProofs>>::new();
        let mut definitions = BTreeSet::new();
        let mut work = 0usize;
        let mut proofs = 0usize;
        let mut proof_bytes = 0usize;
        let (architecture_effects, architecture_rejections) = self
            .selected_metadata_architecture_effects(
                source_root,
                &selection,
                &requested,
                &mut work,
            )?;
        let qualified_physical_parents = architecture_effects
            .iter()
            .map(|effect| effect.endpoint.clone())
            .collect();
        for declaration in self.graph.declarations() {
            charge(&mut work, MAX_WORK)?;
            if !selection.reached_targets.contains(&declaration.target) {
                continue;
            }
            for edge in &declaration.dependencies {
                charge(&mut work, MAX_WORK)?;
                if edge.concrete.is_empty()
                    || !selection.missing_endpoints.contains(&edge.concrete)
                    || requested.contains(edge.concrete.as_str())
                {
                    continue;
                }
                let candidate = candidates
                    .entry(edge.concrete.clone())
                    .or_insert_with(|| Some(OmissionProofs::default()));
                if candidate.is_none() {
                    continue;
                }
                if let Some(proof) = self.template_variant_hook(
                    source_root,
                    declaration,
                    edge,
                    &qualified_physical_parents,
                    &mut definitions,
                )? {
                    charge(&mut proofs, MAX_PROOFS)?;
                    proof_bytes = proof_bytes
                        .checked_add(
                            std::mem::size_of::<VerifiedTemplateHook>()
                                + proof.recipe.len()
                                + proof.target.len()
                                + proof.expression.len()
                                + proof.family.len(),
                        )
                        .filter(|bytes| *bytes <= 16 * 1024 * 1024)
                        .ok_or("Source metadata extension proof exceeds byte budget")?;
                    if let Some(records) = candidate.as_mut() {
                        records.templates.insert(proof);
                    }
                } else {
                    let mut selector = None;
                    // A Source-owned selector cannot make a physical owner
                    // or mandatory declaration optional. Every matching
                    // occurrence must retain its exact original recipe,
                    // target, raw selector expression and concrete value.
                    if declaration.virtual_target
                        && !declaration.claims_make_owner
                        && !declaration.bare_marker
                    {
                        let canonical =
                            super::meta_semantics::architecture_expression(&edge.raw_expression);
                        for contract in &self.optional_meta_dependencies {
                            charge(&mut work, MAX_WORK)?;
                            if contract.absence == NativeMetaAbsence::Selector
                                && contract.target == declaration.target
                                && self.owners.get(&declaration.file) == Some(&contract.recipe)
                                && canonical.as_deref() == Some(contract.dependency.as_str())
                                && crate::graph::native_endpoint(
                                    &contract.dependency,
                                    &self.architecture_context,
                                )
                                .is_ok_and(|concrete| concrete == edge.concrete)
                            {
                                selector = Some(VerifiedSelectorContract {
                                    recipe: contract.recipe.clone(),
                                    target: contract.target.clone(),
                                    expression: contract.dependency.clone(),
                                });
                                break;
                            }
                        }
                    }
                    if let Some(proof) = selector {
                        charge(&mut proofs, MAX_PROOFS)?;
                        proof_bytes = proof_bytes
                            .checked_add(
                                std::mem::size_of::<VerifiedSelectorContract>()
                                    + proof.recipe.len()
                                    + proof.target.len()
                                    + proof.expression.len(),
                            )
                            .filter(|bytes| *bytes <= 16 * 1024 * 1024)
                            .ok_or("Source metadata selector proof exceeds byte budget")?;
                        if let Some(records) = candidate.as_mut() {
                            records.selectors.insert(proof);
                        }
                    } else {
                        *candidate = None;
                    }
                }
            }
        }
        let mut omissions = Vec::new();
        let mut selector_omissions = Vec::new();
        for (dependency, proof) in candidates {
            if let Some(proofs) =
                proof.filter(|proofs| !proofs.templates.is_empty() || !proofs.selectors.is_empty())
            {
                selection.missing_endpoints.remove(&dependency);
                if !proofs.templates.is_empty() {
                    omissions.push(SourceMetadataHookOmission {
                        dependency: dependency.clone(),
                        proofs: proofs.templates,
                    });
                }
                if !proofs.selectors.is_empty() {
                    selector_omissions.push(SourceMetadataSelectorOmission {
                        dependency,
                        proofs: proofs.selectors,
                    });
                }
            }
        }
        self.verify(source_root)?;
        Ok(SourceMetadataSelection {
            selection,
            omissions,
            selector_omissions,
            architecture_effects,
            architecture_rejections,
        })
    }

    /// Qualify only selected physical parents through the existing independent
    /// Source-effect replay. Owner presence or an endpoint spelling alone is
    /// insufficient. Other effect kinds retain their separate producer paths.
    fn selected_metadata_architecture_effects(
        &self,
        root: &Path,
        selection: &OwnerSelectionEvidence,
        requested: &BTreeSet<&str>,
        work: &mut usize,
    ) -> Result<(Vec<ArchEndpointEffect>, Vec<RejectedArchEndpointEffect>), String> {
        if !self.architecture_hook_families.contains("set-archincludes") {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut parents = BTreeSet::new();
        for declaration in self.graph.declarations() {
            charge(work, MAX_WORK)?;
            if !selection.reached_targets.contains(&declaration.target)
                || self.graph.owner_files(&declaration.target).is_none()
            {
                continue;
            }
            for edge in &declaration.dependencies {
                charge(work, MAX_WORK)?;
                if selection.missing_endpoints.contains(&edge.concrete)
                    && !requested.contains(edge.concrete.as_str())
                {
                    parents.insert(declaration.target.clone());
                }
            }
        }
        if parents.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut effects = Vec::new();
        let mut rejections = Vec::new();
        let mut rejection_bytes = 0usize;
        let mut rejection_count = 0usize;
        let mut source_bytes = 0usize;
        let mut replay_bytes = 0usize;
        let mut effect_count = 0usize;
        for (owner, targets) in &selection.selected_owner_targets {
            charge(work, MAX_WORK)?;
            if targets.is_disjoint(&parents) {
                continue;
            }
            let recipe = self
                .owners
                .get(owner)
                .ok_or("selected architecture owner is unbound")?;
            if !selection.selected_source_files.contains(recipe)
                || Path::new(recipe)
                    .extension()
                    .is_none_or(|extension| extension != "src")
            {
                continue;
            }
            let path = aros_common::canonical_source_file(root, Path::new(recipe))
                .map_err(|error| error.to_string())?;
            let (_, bytes) = aros_common::measure_regular_file_bounded(&path, 1024 * 1024)
                .map_err(|error| error.to_string())?
                .ok_or("selected architecture recipe is missing")?;
            source_bytes = source_bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= super::MAX_TOTAL_BYTES)
                .ok_or("selected architecture recipes exceed byte budget")?;
            if self.snapshots.get(recipe).map(String::as_str)
                != Some(aros_common::sha256_bytes(&bytes).as_str())
            {
                return Err("selected architecture recipe is not sealed or changed".into());
            }
            let recipe_bytes = bytes.len();
            let source = aros_common::decode_source_bytes(bytes);
            let scan = collect_source_arch_endpoint_effects(
                &source,
                Path::new(recipe),
                root,
                Some(&self.architecture_context),
            )?;
            for rejection in scan.rejected {
                charge(work, MAX_WORK)?;
                if rejection.directive == "%set_archincludes"
                    && rejection.endpoint.as_ref().is_none_or(|endpoint| {
                        targets.contains(endpoint) && parents.contains(endpoint)
                    })
                {
                    charge(&mut rejection_count, MAX_PROOFS)?;
                    let bytes = rejection.recipe.len()
                        + rejection.directive.len()
                        + rejection.endpoint.as_ref().map_or(0, String::len)
                        + rejection.reason.len();
                    rejection_bytes = rejection_bytes
                        .checked_add(bytes)
                        .filter(|total| *total <= 16 * 1024 * 1024)
                        .ok_or("selected architecture rejection exceeds byte budget")?;
                    rejections.push(rejection);
                }
            }
            for effect in scan.effects {
                charge(work, MAX_WORK)?;
                if matches!(effect.data, ArchEndpointEffectData::SetArchIncludes { .. })
                    && effect.applies_to(&self.architecture_context)
                    && targets.contains(&effect.endpoint)
                    && parents.contains(&effect.endpoint)
                {
                    charge(&mut effect_count, MAX_PROOFS)?;
                    // The independent verifier rereads/replays the original
                    // recipe per effect. Bound that amplification separately
                    // from the initial selected-recipe read budget.
                    replay_bytes = replay_bytes
                        .checked_add(recipe_bytes)
                        .filter(|total| *total <= super::MAX_TOTAL_BYTES)
                        .ok_or("selected architecture replay exceeds byte budget")?;
                    effects.push(effect);
                }
            }
        }
        let replay_work = effects
            .len()
            .checked_mul(self.graph.declarations().len())
            .ok_or("selected architecture replay exceeds work budget")?;
        *work = work
            .checked_add(replay_work)
            .filter(|total| *total <= MAX_WORK)
            .ok_or("selected architecture replay exceeds work budget")?;
        self.verify_arch_endpoint_effects(root, &effects)?;
        Ok((effects, rejections))
    }
}

fn charge(work: &mut usize, limit: usize) -> Result<(), String> {
    *work = work
        .checked_add(1)
        .filter(|work| *work <= limit)
        .ok_or("Source metadata extension proof exceeds work budget")?;
    Ok(())
}
