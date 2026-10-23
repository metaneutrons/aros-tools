//! Source-verified reuse of one architecture object lane through a handwritten
//! virtual MetaMake edge. The selector can be pure virtual or compose with an
//! already-proven physical selected-tag object lane. The proof aliases a source
//! group; it never creates a Make producer or changes an architecture effect.

use super::NativeOwnerProjection;
use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
use crate::genmf_projection::ExpandedLineProvenance;
use crate::metamake_owner_graph::TargetDeclarationProvenance;
use crate::parser::TargetContext;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const MAX_ALIASES: usize = 128;
const MAX_EFFECT_CANDIDATES: usize = 500_000;
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
type AliasCandidate<'a> = (&'a ArchEndpointEffect, String, String, String);

/// An opaque proof that a source-owned architecture object group is selected
/// through one exact, handwritten virtual `#MM-` edge.
///
/// It covers either a
/// pure virtual selector or a selector composed with its proven physical
/// selected-tag object lane.
///
/// Instances can only be constructed by
/// [`NativeOwnerProjection::verified_arch_lane_aliases`]. The contained
/// effect remains the original source lane; `selected_tag` is only the CMake
/// source-selection route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArchLaneAlias {
    effect: ArchEndpointEffect,
    physical_owner: Option<VerifiedPhysicalOwner>,
    mainmmake: String,
    source_tag: String,
    selected_tag: String,
    consumer: String,
    dependency: String,
}

/// A selected-tag producer that already physically owns the alias consumer.
/// This is included only after the native projection binds its unique owner
/// file to the exact source-replayed object effect and its paired linklib lane.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedPhysicalOwner {
    owner: String,
    effect: ArchEndpointEffect,
    pair: ArchEndpointEffect,
}

impl VerifiedArchLaneAlias {
    /// Whether this proof belongs to the exact source effect selected by the
    /// native binder. This does not expose or mutate the proof's route.
    #[must_use]
    pub fn matches_effect(&self, effect: &ArchEndpointEffect) -> bool {
        &self.effect == effect
    }

    /// Original endpoint whose source-backed effect must remain selected.
    #[must_use]
    pub fn original_endpoint(&self) -> &str {
        &self.effect.endpoint
    }

    pub(crate) const fn original_effect(&self) -> &ArchEndpointEffect {
        &self.effect
    }

    pub(crate) fn route(&self) -> (&str, &str, &str, &str, &str) {
        (
            &self.mainmmake,
            &self.source_tag,
            &self.selected_tag,
            &self.consumer,
            &self.dependency,
        )
    }

    pub(crate) fn physical_owner(
        &self,
    ) -> Option<(&ArchEndpointEffect, &ArchEndpointEffect, &str)> {
        self.physical_owner
            .as_ref()
            .map(|owner| (&owner.effect, &owner.pair, owner.owner.as_str()))
    }

    pub(crate) fn verify_physical_pair(
        &self,
        effects: &[ArchEndpointEffect],
    ) -> Result<(), String> {
        let Some(owner) = &self.physical_owner else {
            return Err("architecture lane composition has no physical owner proof".into());
        };
        if object_effect_pair(effects, &owner.effect)? != &owner.pair {
            return Err("architecture lane composition physical linklib effect changed".into());
        }
        Ok(())
    }
}

impl NativeOwnerProjection {
    /// Independently prove architecture object groups pulled in through an
    /// exact source-written virtual edge for a selected primary target tag.
    /// The selector may be pure virtual, or may already have a physical owner
    /// if that owner is exactly bound to its source-backed selected-tag object
    /// effect, paired effect, and source group.
    ///
    /// CPU32 is intentionally not a selector here: a source lane may reuse
    /// another ABI's implementation, but only the selected CPU/platform/family
    /// spelling on the edge's consumer endpoint determines CMake selection.
    /// Macro/template-generated edges are not accepted by this proof.
    ///
    /// # Errors
    /// Rejects changed projection inputs, ambiguous source edges, generated or
    /// nonliteral edge provenance, and source effects that do not replay and
    /// retain their existing producer proof.
    pub fn verified_arch_lane_aliases(
        &self,
        root: &Path,
        effects: &[ArchEndpointEffect],
        context: &TargetContext,
    ) -> Result<Vec<VerifiedArchLaneAlias>, String> {
        self.verify(root)?;
        if context.cpu != self.architecture_context.cpu
            || context.platform != self.architecture_context.platform
            || context.family != self.architecture_context.family
            || context.variant != self.architecture_context.variant
            || context.toolchain != self.architecture_context.toolchain
            || context.cpu32 != self.architecture_context.cpu32
        {
            return Err(
                "architecture lane alias selector differs from the sealed source context".into(),
            );
        }
        if effects.len() > MAX_EFFECT_CANDIDATES {
            return Err("architecture lane alias effect input limit exceeded".into());
        }
        let selected_tags = selected_primary_tags(context);
        if selected_tags.is_empty() {
            return Ok(Vec::new());
        }

        // Index only actual source-effect lanes that could be pulled into a
        // selected primary tag. This keeps unrelated architecture macros and
        // projection metadata outside the proof's dependency closure.
        let mut declarations_by_target: BTreeMap<&str, Vec<&TargetDeclarationProvenance>> =
            BTreeMap::new();
        for declaration in self.graph.declarations() {
            declarations_by_target
                .entry(&declaration.target)
                .or_default()
                .push(declaration);
        }
        let mut candidates: BTreeMap<(String, String), Vec<AliasCandidate<'_>>> = BTreeMap::new();
        let mut candidate_effect_count = 0usize;
        for effect in effects {
            let ArchEndpointEffectData::ArchModuleObjects {
                mainmmake,
                tag,
                directory,
                ..
            } = &effect.data
            else {
                continue;
            };
            if !valid_identity(mainmmake) || !valid_identity(tag) {
                continue;
            }
            // Named ISA sublanes (e.g. an AVX lane inside another CPU's
            // directory) already use the ordinary lane-attachment proof.
            // This contract only reuses the owning source CPU namespace;
            // no CPU enum or board-name allowlist is involved.
            let source_namespace = Path::new(directory)
                .strip_prefix("arch")
                .ok()
                .and_then(|path| path.components().next())
                .and_then(|component| component.as_os_str().to_str());
            if !source_namespace.is_some_and(|namespace| namespace.starts_with(&format!("{tag}-")))
            {
                continue;
            }
            for selected_tag in &selected_tags {
                if selected_tag == tag {
                    continue;
                }
                let consumer = format!("{mainmmake}-{selected_tag}");
                let dependency = format!("{mainmmake}-{tag}");
                if !declarations_by_target
                    .get(consumer.as_str())
                    .is_some_and(|declarations| {
                        declarations.iter().any(|declaration| {
                            declaration
                                .dependencies
                                .iter()
                                .any(|edge| edge.concrete == dependency)
                        })
                    })
                {
                    continue;
                }
                candidates.entry((consumer, dependency)).or_default().push((
                    effect,
                    mainmmake.clone(),
                    tag.clone(),
                    selected_tag.clone(),
                ));
                candidate_effect_count += 1;
                if candidates.len() > MAX_ALIASES || candidate_effect_count > MAX_ALIASES {
                    return Err("architecture lane alias candidate limit exceeded".into());
                }
            }
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let mut aliases = Vec::new();
        let mut seen_effects = BTreeSet::new();
        let mut proof_effects = BTreeMap::new();
        for ((consumer, dependency), source_effects) in candidates {
            let matching_edges: Vec<_> = declarations_by_target
                .get(consumer.as_str())
                .into_iter()
                .flatten()
                .filter(|declaration| {
                    declaration
                        .dependencies
                        .iter()
                        .any(|edge| edge.concrete == dependency)
                })
                .copied()
                .collect();
            if matching_edges.is_empty() {
                // This is the lazy boundary: no adjacent source-owned edge,
                // no alias proof and no extra template-closure requirement.
                continue;
            }
            let [declaration] = matching_edges.as_slice() else {
                return Err(format!(
                    "architecture lane alias edge is duplicated or ambiguous: {consumer} -> {dependency}"
                ));
            };
            let Some((_, mainmmake, source_tag, selected_tag)) = source_effects.first() else {
                return Err("architecture lane alias has no source-effect candidate".into());
            };
            let mainmmake = mainmmake.clone();
            let source_tag = source_tag.clone();
            let selected_tag = selected_tag.clone();
            if !selected_tags.contains(&selected_tag)
                || !source_effects.iter().all(|(effect, candidate_main, candidate_source, candidate_selected)| {
                    candidate_main == &mainmmake
                        && candidate_source == &source_tag
                        && candidate_selected == &selected_tag
                        &&
                    matches!(
                        &effect.data,
                        ArchEndpointEffectData::ArchModuleObjects { mainmmake: owner, tag, .. }
                            if owner == &mainmmake && tag == &source_tag
                    )
                })
            {
                return Err("architecture lane alias endpoints do not share a selected module lane".into());
            }
            self.verify_handwritten_virtual_edge(root, declaration, &consumer, &dependency)?;
            let physical_owner =
                self.verified_physical_owner(&consumer, &mainmmake, &selected_tag, effects)?;

            for (effect, _, _, _) in source_effects {
                let identity = (
                    effect.recipe.clone(),
                    effect.line,
                    effect.endpoint.clone(),
                    consumer.clone(),
                );
                if !seen_effects.insert(identity) {
                    return Err(format!(
                        "duplicate architecture lane alias effect: {}:{} {}",
                        effect.recipe, effect.line, effect.endpoint
                    ));
                }
                proof_effects.insert(
                    (effect.recipe.clone(), effect.line, effect.endpoint.clone()),
                    effect.clone(),
                );
                let pair = object_effect_pair(effects, effect)?;
                proof_effects.insert(
                    (pair.recipe.clone(), pair.line, pair.endpoint.clone()),
                    pair.clone(),
                );
                if let Some(owner) = &physical_owner {
                    proof_effects.insert(
                        (
                            owner.effect.recipe.clone(),
                            owner.effect.line,
                            owner.effect.endpoint.clone(),
                        ),
                        owner.effect.clone(),
                    );
                    proof_effects.insert(
                        (
                            owner.pair.recipe.clone(),
                            owner.pair.line,
                            owner.pair.endpoint.clone(),
                        ),
                        owner.pair.clone(),
                    );
                }
                aliases.push(VerifiedArchLaneAlias {
                    effect: effect.clone(),
                    physical_owner: physical_owner.clone(),
                    mainmmake: mainmmake.clone(),
                    source_tag: source_tag.clone(),
                    selected_tag: selected_tag.clone(),
                    consumer: consumer.clone(),
                    dependency: dependency.clone(),
                });
                if aliases.len() > MAX_ALIASES {
                    return Err("verified architecture lane alias limit exceeded".into());
                }
            }
        }
        if !proof_effects.is_empty() {
            self.verify_arch_endpoint_effects(
                root,
                &proof_effects.into_values().collect::<Vec<_>>(),
            )?;
        }
        Ok(aliases)
    }

    fn verified_physical_owner(
        &self,
        consumer: &str,
        mainmmake: &str,
        selected_tag: &str,
        effects: &[ArchEndpointEffect],
    ) -> Result<Option<VerifiedPhysicalOwner>, String> {
        let Some(owners) = self.graph.owner_files(consumer) else {
            return Ok(None);
        };
        if owners.is_empty() {
            return Ok(None);
        }
        if owners.len() != 1 {
            return Err(format!(
                "architecture lane alias selector has multiple physical owners: {consumer}"
            ));
        }
        let owner = owners.first().expect("one physical owner");
        let matching_effects = effects
            .iter()
            .filter(|effect| {
                effect.endpoint == consumer
                    && effect.recipe.strip_suffix(".src") == Some(owner.as_str())
                    && matches!(
                        &effect.data,
                        ArchEndpointEffectData::ArchModuleObjects {
                            mainmmake: effect_main,
                            tag: effect_tag,
                            ..
                        } if effect_main == mainmmake && effect_tag == selected_tag
                    )
            })
            .collect::<Vec<_>>();
        let [effect] = matching_effects.as_slice() else {
            return Err(format!(
                "architecture lane alias selector physical owner lacks one exact selected-tag object effect: {consumer}"
            ));
        };
        let pair = object_effect_pair(effects, effect)?;
        Ok(Some(VerifiedPhysicalOwner {
            owner: (*owner).clone(),
            effect: (*effect).clone(),
            pair: pair.clone(),
        }))
    }

    fn verify_handwritten_virtual_edge(
        &self,
        root: &Path,
        declaration: &TargetDeclarationProvenance,
        expected_consumer: &str,
        expected_dependency: &str,
    ) -> Result<String, String> {
        if !declaration.virtual_target
            || declaration.bare_marker
            || declaration.claims_make_owner
            || declaration.raw_target != expected_consumer
            || declaration.target != expected_consumer
            || declaration.dependencies.len() != 1
            || declaration.dependencies[0].raw_expression != expected_dependency
            || declaration.dependencies[0].concrete != expected_dependency
        {
            return Err(format!(
                "architecture lane alias is not one exact virtual source edge: {expected_consumer} -> {expected_dependency}"
            ));
        }
        let origin = self.expansion_origin(declaration).ok_or_else(|| {
            "architecture lane alias has no captured source-line provenance".to_owned()
        })?;
        let recipe = self.bound_recipe(&declaration.file)?;
        let recipe = validate_recipe(&recipe)?;
        if !is_direct_source_origin(root, recipe, origin, declaration) {
            return Err(format!(
                "architecture lane alias must be a handwritten source edge: {expected_consumer} -> {expected_dependency}"
            ));
        }
        let digest = self
            .snapshots
            .get(recipe)
            .ok_or("architecture lane alias source is not sealed")?;
        let path = aros_common::canonical_source_file(root, Path::new(recipe))
            .map_err(|error| error.to_string())?;
        let (_, bytes) = aros_common::measure_regular_file_bounded(&path, MAX_RECIPE_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or("architecture lane alias source is missing")?;
        if aros_common::sha256_bytes(&bytes).as_str() != digest {
            return Err(format!("architecture lane alias source changed: {recipe}"));
        }
        let source = String::from_utf8(bytes)
            .map_err(|_| format!("architecture lane alias source is not UTF-8: {recipe}"))?;
        let line = crate::metamake_owner_graph::source_declaration_at(
            &source,
            origin
                .source_line
                .ok_or("architecture lane alias has no physical source line")?,
            recipe,
        )?;
        let rest = line
            .strip_prefix("#MM-")
            .ok_or("architecture lane alias source is not a #MM- declaration")?;
        if rest.contains(['\\', '#', '"', '\'', '$'])
            || !line.is_ascii()
            || line.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err("architecture lane alias uses unsupported Make syntax".into());
        }
        if rest.starts_with('-') {
            return Err("malformed architecture lane alias marker".into());
        }
        let rest = rest.trim_start();
        let (targets, dependencies) = rest
            .split_once(':')
            .ok_or("architecture lane alias has no explicit dependency")?;
        if dependencies.contains(':') {
            return Err("architecture lane alias has multiple dependency separators".into());
        }
        let targets = targets.split_whitespace().collect::<Vec<_>>();
        let dependencies = dependencies.split_whitespace().collect::<Vec<_>>();
        if targets.as_slice() != [expected_consumer]
            || dependencies.as_slice() != [expected_dependency]
        {
            return Err("architecture lane alias source tokens do not match its graph edge".into());
        }
        Ok(recipe.to_owned())
    }
}

fn object_effect_pair<'a>(
    effects: &'a [ArchEndpointEffect],
    effect: &ArchEndpointEffect,
) -> Result<&'a ArchEndpointEffect, String> {
    let ArchEndpointEffectData::ArchModuleObjects {
        mainmmake,
        tag,
        module_sources,
        linklib_sources,
        ..
    } = &effect.data
    else {
        return Err("architecture lane alias requires an object-source effect".into());
    };
    let pairs: Vec<_> = effects
        .iter()
        .filter(|candidate| {
            candidate.recipe == effect.recipe
                && candidate.line == effect.line
                && match &candidate.data {
                    ArchEndpointEffectData::EmptyLinklibAggregate {
                        mainmmake: pair_main,
                        tag: pair_tag,
                        module_sources: pair_sources,
                        compiler,
                    } => {
                        linklib_sources.is_empty()
                            && pair_main == mainmmake
                            && pair_tag == tag
                            && pair_sources == module_sources
                            && (compiler == "target" || compiler == "kernel")
                    }
                    ArchEndpointEffectData::ArchClientLinklibSources {
                        mainmmake: pair_main,
                        tag: pair_tag,
                        module_sources: pair_sources,
                        linklib_sources: pair_linklib_sources,
                        compiler,
                        ..
                    } => {
                        pair_main == mainmmake
                            && pair_tag == tag
                            && pair_sources == module_sources
                            && pair_linklib_sources == linklib_sources
                            && (compiler == "target" || compiler == "kernel")
                    }
                    _ => false,
                }
        })
        .collect();
    let [pair] = pairs.as_slice() else {
        return Err(format!(
            "architecture lane alias lacks its unique linklib-lane proof: {}:{} {}",
            effect.recipe, effect.line, effect.endpoint
        ));
    };
    Ok(pair)
}

fn is_direct_source_origin(
    root: &Path,
    recipe: &str,
    origin: &ExpandedLineProvenance,
    declaration: &TargetDeclarationProvenance,
) -> bool {
    if !origin.macro_stack.is_empty()
        || origin.template_path.is_some()
        || origin.template_definition_line.is_some()
        || origin.output_line != declaration.expanded_line
        || origin.source_line.is_none_or(|line| line == 0)
    {
        return false;
    }
    let lexical = root.join(recipe);
    origin.source_path.as_ref().is_some_and(|path| {
        path == &lexical
            || fs::canonicalize(&lexical)
                .is_ok_and(|canonical| canonical.as_path() == path.as_path())
    })
}

fn selected_primary_tags(context: &TargetContext) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    for value in [context.cpu.as_deref(), context.platform.as_deref()]
        .into_iter()
        .flatten()
    {
        if valid_identity(value) {
            tags.insert(value.to_owned());
        }
    }
    if let (Some(cpu), Some(platform)) = (&context.cpu, &context.platform) {
        let combined = format!("{platform}-{cpu}");
        if valid_identity(&combined) {
            tags.insert(combined);
        }
    }
    if let Some(family) = context
        .family
        .as_deref()
        .filter(|value| valid_identity(value))
    {
        tags.insert(family.to_owned());
    }
    tags
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

fn validate_recipe(recipe: &str) -> Result<&str, String> {
    if recipe.is_empty()
        || recipe.contains(['\\', '\0'])
        || Path::new(recipe).is_absolute()
        || (recipe != "mmakefile.src" && !recipe.ends_with("/mmakefile.src"))
        || recipe
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!(
            "unsafe architecture lane alias source path: {recipe:?}"
        ));
    }
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::{NativeOwnerProjection, VerifiedArchLaneAlias};
    use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
    use crate::arch_sources::collect_arch_sources;
    use crate::genmf_projection::{expand_bytes, Limits as GenmfLimits};
    use crate::graph::DependencyGraph;
    use crate::metamake_owner_graph::{Limits as MetaMakeLimits, MetaMakeOwnerGraph};
    use crate::parser::TargetContext;
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::fs;
    use std::path::Path;

    const OBJECTS_RECIPE: &str = "arch/i386-all/debug/mmakefile.src";
    const SELECTOR_RECIPE: &str = "arch/x86_64-all/debug/mmakefile.src";
    const EXTRA_OWNER_RECIPE: &str = "arch/x86_64-all/other/mmakefile.src";
    const TEMPLATE: &str = "config/make.tmpl";
    const OWNER_OBJECTS: &str = "arch/i386-all/debug/mmakefile";
    const OWNER_SELECTOR: &str = "arch/x86_64-all/debug/mmakefile";
    const EXTRA_OWNER: &str = "arch/x86_64-all/other/mmakefile";
    const EFFECT_SOURCE: &str = "%build_archspecific mainmmake=kernel-debug maindir=rom/debug modname=debug arch=i386 files=debug_platform linklibfiles=debug_client\n";
    const SELECTED_EFFECT_SOURCE: &str = "%build_archspecific mainmmake=kernel-debug maindir=rom/debug modname=debug arch=x86_64 files=primary_platform linklibfiles=primary_client\n";
    const EXTRA_OWNER_SOURCE: &str = "#MM\nkernel-debug-x86_64\n";
    const SELECTOR_EDGE: &str = "#MM- kernel-debug-x86_64 : kernel-debug-i386\n";
    const GEN_ARCHSPECIFICRULES: &str = concat!(
        "%define gen_archspecificrules mainmmake=/A target= subtarget=\n",
        "#MM- %(mainmmake)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(CPU)%(target)%(subtarget)\n",
        "\n",
        "#MM- %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)\n",
        "\n",
        "#MM- %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget)\n",
        "\n",
        "#MM- %(mainmmake)-$(ARCH)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)\n",
        "\n",
        "#MM- %(mainmmake)-$(FAMILY)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(ARCH)%(target)%(subtarget)\n",
        "\n",
        "#MM- %(mainmmake)-$(CPU)%(target)%(subtarget) : \\\n",
        "#MM             %(mainmmake)-$(FAMILY)%(target)%(subtarget)\n",
        "%end\n",
    );
    const BUILD_MODULE: &str = concat!(
        "%define build_module mainmmake=/A\n",
        "%gen_archspecificrules mainmmake=%(mainmmake)\n",
        "%gen_archspecificrules mainmmake=%(mainmmake) target=-set-archincludes\n",
        "%gen_archspecificrules mainmmake=%(mainmmake) target=-linklib\n",
        "%end\n",
    );

    struct Fixture {
        root: tempfile::TempDir,
        projection: NativeOwnerProjection,
        context: TargetContext,
        effects: Vec<ArchEndpointEffect>,
        source_group: crate::arch_sources::ArchSourceDecl,
        physical_source_group: Option<crate::arch_sources::ArchSourceDecl>,
    }

    fn context(cpu: &str, cpu32: Option<&str>) -> TargetContext {
        TargetContext {
            cpu: Some(cpu.into()),
            platform: Some("pc".into()),
            cpu32: cpu32.map(str::to_owned),
            ..TargetContext::default()
        }
    }

    fn fixture(selector: &str, generated_alias: bool) -> Fixture {
        fixture_with_physical_owner(selector, generated_alias, false)
    }

    fn fixture_with_physical_owner(
        selector: &str,
        generated_alias: bool,
        physical_owner: bool,
    ) -> Fixture {
        fixture_with_owner_configuration(selector, generated_alias, physical_owner, false)
    }

    fn fixture_with_duplicate_owner(selector: &str, generated_alias: bool) -> Fixture {
        fixture_with_owner_configuration(selector, generated_alias, true, true)
    }

    fn fixture_with_owner_configuration(
        selector: &str,
        generated_alias: bool,
        physical_owner: bool,
        duplicate_owner: bool,
    ) -> Fixture {
        let temp_root = std::env::temp_dir().canonicalize().unwrap();
        let root = tempfile::tempdir_in(temp_root).unwrap();
        let objects_dir = root.path().join("arch/i386-all/debug");
        let selector_dir = root.path().join("arch/x86_64-all/debug");
        fs::create_dir_all(&objects_dir).unwrap();
        fs::create_dir_all(&selector_dir).unwrap();
        if duplicate_owner {
            fs::create_dir_all(root.path().join("arch/x86_64-all/other")).unwrap();
        }
        fs::create_dir_all(root.path().join("config")).unwrap();

        let context = context("x86_64", Some("i386"));
        let mut template = format!(
            "{GEN_ARCHSPECIFICRULES}{BUILD_MODULE}{}",
            include_str!("../tests/fixtures/architecture_objects.tmpl")
        );
        let mut selector_source = if generated_alias {
            template.push_str("\n%define fixture_lane_alias\n");
            template.push_str(selector);
            template.push_str("%end\n");
            "%fixture_lane_alias\n".to_owned()
        } else {
            selector.to_owned()
        };
        if physical_owner {
            selector_source.push_str(SELECTED_EFFECT_SOURCE);
        }
        let object_path = root.path().join(OBJECTS_RECIPE);
        let selector_path = root.path().join(SELECTOR_RECIPE);
        let template_path = root.path().join(TEMPLATE);
        fs::write(&object_path, EFFECT_SOURCE).unwrap();
        fs::write(&selector_path, selector_source.as_bytes()).unwrap();
        fs::write(&template_path, template.as_bytes()).unwrap();
        if duplicate_owner {
            fs::write(root.path().join(EXTRA_OWNER_RECIPE), EXTRA_OWNER_SOURCE).unwrap();
        }

        let mut owners = BTreeMap::new();
        owners.insert(OWNER_OBJECTS.to_owned(), OBJECTS_RECIPE.to_owned());
        owners.insert(OWNER_SELECTOR.to_owned(), SELECTOR_RECIPE.to_owned());
        if duplicate_owner {
            owners.insert(EXTRA_OWNER.to_owned(), EXTRA_OWNER_RECIPE.to_owned());
        }
        let mut discovered_inputs = BTreeSet::new();
        discovered_inputs.insert(OBJECTS_RECIPE.to_owned());
        discovered_inputs.insert(SELECTOR_RECIPE.to_owned());
        if duplicate_owner {
            discovered_inputs.insert(EXTRA_OWNER_RECIPE.to_owned());
        }
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            OBJECTS_RECIPE.to_owned(),
            aros_common::sha256_bytes(EFFECT_SOURCE.as_bytes()).to_string(),
        );
        snapshots.insert(
            SELECTOR_RECIPE.to_owned(),
            aros_common::sha256_bytes(selector_source.as_bytes()).to_string(),
        );
        snapshots.insert(
            TEMPLATE.to_owned(),
            aros_common::sha256_bytes(template.as_bytes()).to_string(),
        );
        if duplicate_owner {
            snapshots.insert(
                EXTRA_OWNER_RECIPE.to_owned(),
                aros_common::sha256_bytes(EXTRA_OWNER_SOURCE.as_bytes()).to_string(),
            );
        }

        let mut expanded_files = BTreeMap::new();
        let mut expansion_origins = BTreeMap::new();
        let mut owner_sources = vec![
            (OWNER_OBJECTS, OBJECTS_RECIPE, EFFECT_SOURCE),
            (OWNER_SELECTOR, SELECTOR_RECIPE, selector_source.as_str()),
        ];
        if duplicate_owner {
            owner_sources.push((EXTRA_OWNER, EXTRA_OWNER_RECIPE, EXTRA_OWNER_SOURCE));
        }
        for (owner, recipe, source) in owner_sources {
            let expanded = expand_bytes(
                source.as_bytes(),
                &root.path().join(recipe),
                &template_path,
                GenmfLimits::default(),
            )
            .unwrap();
            for origin in expanded.provenance {
                let fragment = &expanded.text[origin.output_start_byte..origin.output_end_byte];
                if fragment.starts_with("#MM") {
                    assert!(expansion_origins
                        .insert((owner.to_owned(), origin.output_line), origin)
                        .is_none());
                }
            }
            expanded_files.insert(owner.to_owned(), expanded.text);
        }
        let graph = MetaMakeOwnerGraph::parse_expanded_files(
            &expanded_files,
            &BTreeMap::from([
                ("ARCH".into(), "esp32p4".into()),
                ("CPU".into(), "riscv".into()),
                ("FAMILY".into(), String::new()),
                ("AROS_TARGET_VARIANT".into(), String::new()),
            ]),
            MetaMakeLimits {
                preserve_empty_endpoints: true,
                ..MetaMakeLimits::default()
            },
        )
        .unwrap();
        let projection = NativeOwnerProjection {
            architecture_context: context.clone(),
            optional_meta_dependencies: Vec::new(),
            metamake_globals: BTreeMap::new(),
            runtime_configuration: None,
            graph,
            expansion_origins,
            expanded_program_sha256: BTreeMap::new(),
            architecture_hook_families: BTreeSet::new(),
            mesa26_original_producers: Vec::new(),
            owners,
            discovery_original_inputs: discovered_inputs
                .iter()
                .map(|name| (name.clone(), name.clone()))
                .collect(),
            discovered_inputs,
            ignored_directories: BTreeSet::new(),
            excluded_paths: BTreeSet::new(),
            snapshots,
            sealed_configuration_inputs: BTreeMap::new(),
            policy_sha256: "test-lane-alias".into(),
            expanded_bytes: expanded_files.values().map(String::len).sum(),
        };

        let scan = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
            EFFECT_SOURCE,
            Path::new(OBJECTS_RECIPE),
            root.path(),
            Some(&context),
        )
        .unwrap();
        assert!(scan.rejected.is_empty(), "{:?}", scan.rejected);
        let mut effects = scan.effects;
        let physical_source_group = if physical_owner {
            let selected_scan = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
                &selector_source,
                Path::new(SELECTOR_RECIPE),
                root.path(),
                Some(&context),
            )
            .unwrap();
            assert!(
                selected_scan.rejected.is_empty(),
                "{:?}",
                selected_scan.rejected
            );
            effects.extend(selected_scan.effects);
            let (mut groups, skipped) = collect_arch_sources(
                &selector_source,
                Path::new("arch/x86_64-all/debug"),
                Some(&context),
            );
            assert!(skipped.is_empty(), "{skipped:?}");
            assert_eq!(groups.len(), 1);
            Some(groups.remove(0))
        } else {
            None
        };
        let (mut source_groups, skipped) = collect_arch_sources(
            EFFECT_SOURCE,
            Path::new("arch/i386-all/debug"),
            Some(&context),
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(source_groups.len(), 1);
        Fixture {
            root,
            projection,
            context,
            effects,
            source_group: source_groups.remove(0),
            physical_source_group,
        }
    }

    fn selected_object_effect(effects: &[ArchEndpointEffect]) -> &ArchEndpointEffect {
        effects
            .iter()
            .find(|effect| {
                matches!(
                    &effect.data,
                    ArchEndpointEffectData::ArchModuleObjects { .. }
                )
            })
            .unwrap()
    }

    fn aliases(fixture: &Fixture) -> Result<Vec<VerifiedArchLaneAlias>, String> {
        fixture.projection.verified_arch_lane_aliases(
            fixture.root.path(),
            &fixture.effects,
            &fixture.context,
        )
    }

    fn physical_graph(fixture: &Fixture) -> DependencyGraph {
        let mut graph = DependencyGraph::default();
        graph.arch_endpoint_effects = fixture.effects.clone();
        let mut groups = vec![fixture.source_group.clone()];
        if let Some(group) = &fixture.physical_source_group {
            groups.push(group.clone());
        }
        graph.arch_sources.insert("kernel-debug".into(), groups);
        graph.meta_targets.insert(
            "kernel-debug-x86_64".into(),
            HashSet::from(["kernel-debug-i386".into()]),
        );
        graph
    }

    #[test]
    fn handwritten_x86_64_edge_retags_original_i386_group_without_replacing_effect() {
        let fixture = fixture(SELECTOR_EDGE, false);
        let aliases = aliases(&fixture).unwrap();
        assert_eq!(aliases.len(), 1);
        let original = selected_object_effect(&fixture.effects);
        assert!(aliases[0].matches_effect(original));
        assert_eq!(aliases[0].original_endpoint(), "kernel-debug-i386");

        let mut graph = DependencyGraph::default();
        graph.arch_endpoint_effects.push(original.clone());
        graph
            .arch_sources
            .insert("kernel-debug".into(), vec![fixture.source_group.clone()]);
        graph.meta_targets.insert(
            "kernel-debug-x86_64".into(),
            HashSet::from(["kernel-debug-i386".into()]),
        );
        let original_effects = graph.arch_endpoint_effects.clone();
        let notes = graph.apply_arch_lane_aliases(&aliases).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(graph.arch_endpoint_effects, original_effects);
        let groups = &graph.arch_sources["kernel-debug"];
        assert!(groups.iter().any(|group| group.tag == "i386"));
        let retagged_groups = groups
            .iter()
            .filter(|group| group.tag == "x86_64")
            .collect::<Vec<_>>();
        let [retagged] = retagged_groups.as_slice() else {
            panic!("one x86_64 CMake source group")
        };
        assert_eq!(retagged.files, fixture.source_group.files);
        assert!(retagged.linklib_files.is_empty());
    }

    #[test]
    fn handwritten_edge_composes_with_an_exact_selected_tag_physical_owner() {
        let fixture = fixture_with_physical_owner(SELECTOR_EDGE, false, true);
        let aliases = aliases(&fixture).unwrap();
        assert_eq!(aliases.len(), 1);
        assert!(aliases[0].physical_owner().is_some());

        let original_effects = fixture.effects.clone();
        let original_groups = vec![
            fixture.source_group.clone(),
            fixture.physical_source_group.clone().unwrap(),
        ];
        let mut graph = physical_graph(&fixture);
        let notes = graph.apply_arch_lane_aliases(&aliases).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(graph.arch_endpoint_effects, original_effects);
        let groups = &graph.arch_sources["kernel-debug"];
        assert_eq!(
            serde_json::to_value(&groups[..2]).unwrap(),
            serde_json::to_value(&original_groups).unwrap()
        );
        let retagged = groups
            .iter()
            .filter(|group| group.tag == "x86_64" && group.files == fixture.source_group.files)
            .collect::<Vec<_>>();
        assert_eq!(retagged.len(), 1);
        assert!(retagged[0].linklib_files.is_empty());
    }

    #[test]
    fn physical_composition_requires_the_unique_selected_effect_pair_group_and_route() {
        let fixture = fixture_with_physical_owner(SELECTOR_EDGE, false, true);
        let aliases = aliases(&fixture).unwrap();

        let mut no_route = physical_graph(&fixture);
        no_route.meta_targets.clear();
        assert!(no_route.apply_arch_lane_aliases(&aliases).is_err());

        let mut no_selected_effect = physical_graph(&fixture);
        no_selected_effect.arch_endpoint_effects.retain(|effect| {
            !(effect.endpoint == "kernel-debug-x86_64"
                && matches!(
                    &effect.data,
                    ArchEndpointEffectData::ArchModuleObjects { .. }
                ))
        });
        assert!(no_selected_effect
            .apply_arch_lane_aliases(&aliases)
            .is_err());

        let mut no_selected_pair = physical_graph(&fixture);
        let selected_effect = no_selected_pair
            .arch_endpoint_effects
            .iter()
            .find(|effect| {
                effect.endpoint == "kernel-debug-x86_64"
                    && matches!(
                        &effect.data,
                        ArchEndpointEffectData::ArchModuleObjects { .. }
                    )
            })
            .unwrap()
            .clone();
        no_selected_pair.arch_endpoint_effects.retain(|candidate| {
            !(candidate.recipe == selected_effect.recipe
                && candidate.line == selected_effect.line
                && candidate.endpoint != selected_effect.endpoint)
        });
        assert!(no_selected_pair.apply_arch_lane_aliases(&aliases).is_err());

        let mut changed_pair = physical_graph(&fixture);
        let (_, pair, _) = aliases[0].physical_owner().unwrap();
        let changed = changed_pair
            .arch_endpoint_effects
            .iter_mut()
            .find(|candidate| *candidate == pair)
            .unwrap();
        if let ArchEndpointEffectData::ArchClientLinklibSources { compiler, .. } = &mut changed.data
        {
            *compiler = "host".into();
        } else {
            panic!("fixture must exercise a nonempty client linklib lane");
        }
        let before = serde_json::to_value(&changed_pair.arch_sources).unwrap();
        assert!(changed_pair.apply_arch_lane_aliases(&aliases).is_err());
        assert_eq!(
            serde_json::to_value(&changed_pair.arch_sources).unwrap(),
            before
        );

        let mut duplicate_selected_effect = physical_graph(&fixture);
        let selected_effect = duplicate_selected_effect
            .arch_endpoint_effects
            .iter()
            .find(|effect| {
                effect.endpoint == "kernel-debug-x86_64"
                    && matches!(
                        &effect.data,
                        ArchEndpointEffectData::ArchModuleObjects { .. }
                    )
            })
            .unwrap()
            .clone();
        duplicate_selected_effect
            .arch_endpoint_effects
            .push(selected_effect);
        assert!(duplicate_selected_effect
            .apply_arch_lane_aliases(&aliases)
            .is_err());

        let mut mismatched_group = physical_graph(&fixture);
        mismatched_group
            .arch_sources
            .get_mut("kernel-debug")
            .unwrap()[1]
            .files[0] = "unrelated_source".into();
        assert!(mismatched_group.apply_arch_lane_aliases(&aliases).is_err());
    }

    #[test]
    fn physical_composition_rejects_missing_duplicate_or_stale_owner_effects() {
        let fixture = fixture_with_physical_owner(SELECTOR_EDGE, false, true);
        let generic_effects = fixture
            .effects
            .iter()
            .filter(|effect| effect.recipe == OBJECTS_RECIPE)
            .cloned()
            .collect::<Vec<_>>();
        assert!(fixture
            .projection
            .verified_arch_lane_aliases(fixture.root.path(), &generic_effects, &fixture.context,)
            .unwrap_err()
            .contains("physical owner lacks one exact selected-tag object effect"));

        let multiple_owners = fixture_with_duplicate_owner(SELECTOR_EDGE, false);
        assert!(aliases(&multiple_owners)
            .unwrap_err()
            .contains("multiple physical owners"));

        let selected = fixture
            .effects
            .iter()
            .find(|effect| {
                effect.endpoint == "kernel-debug-x86_64"
                    && matches!(
                        &effect.data,
                        ArchEndpointEffectData::ArchModuleObjects { .. }
                    )
            })
            .unwrap()
            .clone();
        let mut duplicate = fixture.effects.clone();
        duplicate.push(selected);
        assert!(fixture
            .projection
            .verified_arch_lane_aliases(fixture.root.path(), &duplicate, &fixture.context,)
            .unwrap_err()
            .contains("physical owner lacks one exact selected-tag object effect"));

        let mut stale = fixture.effects.clone();
        for effect in &mut stale {
            if effect.endpoint == "kernel-debug-x86_64" {
                if let ArchEndpointEffectData::ArchModuleObjects { module_sources, .. } =
                    &mut effect.data
                {
                    module_sources[0] = "changed_source".into();
                }
            }
        }
        assert!(fixture
            .projection
            .verified_arch_lane_aliases(fixture.root.path(), &stale, &fixture.context)
            .is_err());
    }

    #[test]
    fn changed_or_missing_edge_and_missing_original_effect_do_not_alias() {
        let changed = fixture("#MM- kernel-debug-x86_64 : kernel-debug-other\n", false);
        assert!(aliases(&changed).unwrap().is_empty());
        let missing = fixture("", false);
        assert!(aliases(&missing).unwrap().is_empty());
        assert!(missing
            .projection
            .verified_arch_lane_aliases(missing.root.path(), &[], &missing.context,)
            .unwrap()
            .is_empty());
        let absent_effect = fixture(SELECTOR_EDGE, false);
        assert!(absent_effect
            .projection
            .verified_arch_lane_aliases(absent_effect.root.path(), &[], &absent_effect.context,)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn generated_or_physical_selector_edges_are_rejected() {
        let generated = fixture(SELECTOR_EDGE, true);
        assert!(aliases(&generated)
            .unwrap_err()
            .contains("handwritten source edge"));
        let generated_with_physical_owner = fixture_with_physical_owner(SELECTOR_EDGE, true, true);
        assert!(aliases(&generated_with_physical_owner)
            .unwrap_err()
            .contains("handwritten source edge"));
        let physical = fixture("#MM kernel-debug-x86_64 : kernel-debug-i386\n", false);
        assert!(aliases(&physical)
            .unwrap_err()
            .contains("one exact virtual source edge"));
    }

    #[test]
    fn cpu32_never_selects_a_reused_primary_abi_lane() {
        let fixture = fixture(SELECTOR_EDGE, false);
        let mut other_primary = context("arm", Some("i386"));
        other_primary.platform = Some("native".into());
        assert!(fixture
            .projection
            .verified_arch_lane_aliases(fixture.root.path(), &fixture.effects, &other_primary,)
            .unwrap_err()
            .contains("sealed source context"));
    }

    #[test]
    fn partially_overlapping_alias_plans_fail_before_graph_mutation() {
        let fixture = fixture(SELECTOR_EDGE, false);
        let first = aliases(&fixture).unwrap().remove(0);
        // Application-only fixture for two already-proven routes. Production
        // callers cannot construct or alter these opaque proofs.
        let mut second = first.clone();
        second.source_tag = "other".into();
        second.dependency = "kernel-debug-other".into();
        second.effect.endpoint = second.dependency.clone();
        let mut other_group = fixture.source_group.clone();
        other_group.tag = "other".into();
        other_group.files.push("second_object".into());
        if let ArchEndpointEffectData::ArchModuleObjects {
            tag,
            module_sources,
            ..
        } = &mut second.effect.data
        {
            *tag = "other".into();
            module_sources.clone_from(&other_group.files);
        }
        let mut graph = DependencyGraph::default();
        graph.arch_endpoint_effects = vec![first.effect.clone(), second.effect.clone()];
        graph.arch_sources.insert(
            "kernel-debug".into(),
            vec![fixture.source_group, other_group],
        );
        graph.meta_targets.insert(
            "kernel-debug-x86_64".into(),
            HashSet::from([first.dependency.clone(), second.dependency.clone()]),
        );
        let before = serde_json::to_value(&graph.arch_sources).unwrap();
        let error = graph.apply_arch_lane_aliases(&[first, second]).unwrap_err();
        assert!(error.contains("overlap in the planned"), "{error}");
        assert_eq!(serde_json::to_value(&graph.arch_sources).unwrap(), before);
    }

    #[test]
    fn graph_route_and_existing_destination_collisions_are_fail_closed() {
        let fixture = fixture(SELECTOR_EDGE, false);
        let aliases = aliases(&fixture).unwrap();
        let original = selected_object_effect(&fixture.effects).clone();
        let mut graph = DependencyGraph::default();
        graph.arch_endpoint_effects.push(original);
        graph
            .arch_sources
            .insert("kernel-debug".into(), vec![fixture.source_group.clone()]);
        assert!(graph.apply_arch_lane_aliases(&aliases).is_err());

        graph.meta_targets.insert(
            "kernel-debug-x86_64".into(),
            HashSet::from(["kernel-debug-i386".into()]),
        );
        let mut unverified_provider = DependencyGraph::default();
        unverified_provider
            .arch_endpoint_effects
            .push(selected_object_effect(&fixture.effects).clone());
        unverified_provider
            .arch_sources
            .insert("kernel-debug".into(), vec![fixture.source_group.clone()]);
        unverified_provider.meta_targets.insert(
            "kernel-debug-x86_64".into(),
            HashSet::from(["kernel-debug-i386".into()]),
        );
        unverified_provider
            .make_meta_providers
            .insert("kernel-debug-x86_64".into());
        assert!(unverified_provider
            .apply_arch_lane_aliases(&aliases)
            .is_err());
        let mut collision = fixture.source_group;
        collision.tag = "x86_64".into();
        graph
            .arch_sources
            .get_mut("kernel-debug")
            .unwrap()
            .push(collision);
        let before = graph.arch_sources["kernel-debug"].len();
        assert!(graph.apply_arch_lane_aliases(&aliases).is_err());
        assert_eq!(graph.arch_sources["kernel-debug"].len(), before);
    }
}
