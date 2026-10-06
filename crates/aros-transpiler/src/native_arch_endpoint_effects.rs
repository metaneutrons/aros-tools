//! Verifies source-derived architecture effects against the sealed native
//! MetaMake projection. This is provenance validation, not producer authority.

use super::NativeOwnerProjection;
use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
use crate::genmf_projection::{ExpandedLineProvenance, MacroExpansionFrame, SourceLineLocation};
use crate::metamake_owner_graph::TargetDeclarationProvenance;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// Reviewed revisions of each macro block. build_archspecific since upstream
// 684b78f85c also registers its -quick and -linklib targets with %buildid,
// which only selects classic Make's per-target variable namespace.
const BUILD_ARCHSPECIFIC_SHA256: &[&str] = &[
    "ece0c04dcb5628546286f0c10600d5438bca0efc787d888bd12d664f411e8bca",
    "f3f12399d993765cff710f8f37caba7ffb0eb240f02ca1c93a6d619a208c7db8",
];
const SET_ARCHINCLUDES_SHA256: &[&str] =
    &["a74165bbdcf00f2bc38c7db885a52e036967471cfaa47e4abd3f917358f0f596"];
const GET_ARCHINCLUDES_SHA256: &[&str] =
    &["344288e6364a32e5ff3a7da878693e31aff2e6e5170a62a3baf481c6f1b6000d"];
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
const MAX_TEMPLATE_BYTES: u64 = 8 * 1024 * 1024;

impl NativeOwnerProjection {
    pub(super) fn verify_get_archincludes_semantics(&self, root: &Path) -> Result<(), String> {
        // The owner index retains #MM output provenance. get_archincludes
        // emits assignments, not #MM targets, so its frame is not indexed.
        // Find its unique definition in the very same sealed templates as
        // the source-owned flag producers, never an ambient config path.
        let templates = self
            .expansion_origins
            .values()
            .flat_map(|origin| &origin.macro_stack)
            .filter(|frame| frame.name == "set_archincludes")
            .map(|frame| (frame.template_path.clone(), frame.clone()))
            .collect::<BTreeMap<_, _>>();
        if templates.is_empty() {
            return Err("get_archincludes has no sealed flag-producer template proof".into());
        }
        for (path, mut frame) in templates {
            let (_, bytes) = aros_common::measure_regular_file_bounded(&path, MAX_TEMPLATE_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or("architecture template is missing")?;
            let template =
                std::str::from_utf8(&bytes).map_err(|_| "architecture template is not UTF-8")?;
            let definitions = template
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.split_whitespace()
                        .take(2)
                        .eq(["%define", "get_archincludes"])
                })
                .map(|(line, _)| line + 1)
                .collect::<Vec<_>>();
            let [line] = definitions.as_slice() else {
                return Err("get_archincludes has no unique sealed template definition".into());
            };
            frame.name = "get_archincludes".into();
            frame.definition_line = *line;
            verify_sealed_macro_definition(self, root, &frame, "get_archincludes")?;
        }
        Ok(())
    }
    /// Verify architecture-effect candidates against this projection's source
    /// owners, emitted MetaMake declarations, expansion frames, and sealed
    /// template definitions. No graph or producer is created here.
    ///
    /// # Errors
    /// Refuses stale snapshots, unbound recipes, handwritten/virtual owners,
    /// mismatched expansion arguments, and unsupported template semantics.
    pub fn verify_arch_endpoint_effects(
        &self,
        root: &Path,
        effects: &[ArchEndpointEffect],
    ) -> Result<(), String> {
        self.verify(root)?;

        let mut seen = BTreeSet::new();
        let mut definitions = BTreeSet::new();
        for effect in effects {
            let identity = (effect.recipe.clone(), effect.line, effect.endpoint.clone());
            if !seen.insert(identity) {
                return Err(format!(
                    "duplicate architecture effect candidate: {}:{} {}",
                    effect.recipe, effect.line, effect.endpoint
                ));
            }
            self.verify_one_arch_endpoint_effect(root, effect, &mut definitions)?;
        }

        // The object-lane candidate is derived only beside the empty
        // linklib-object candidate from the same exact macro invocation.
        for effect in effects {
            if let ArchEndpointEffectData::ArchModuleObjects {
                mainmmake,
                tag,
                module_sources,
                ..
            } = &effect.data
            {
                let paired = effects.iter().filter(|candidate| {
                    candidate.recipe == effect.recipe
                        && candidate.line == effect.line
                        && matches!(
                            &candidate.data,
                            ArchEndpointEffectData::EmptyLinklibAggregate {
                                mainmmake: pair_main,
                                tag: pair_tag,
                                module_sources: pair_sources,
                                compiler: pair_compiler,
                                ..
                            } if pair_main == mainmmake
                                && pair_tag == tag
                                && pair_sources == module_sources
                                && (pair_compiler == "target"
                                    || pair_compiler == "kernel"
                                        && self
                                            .architecture_context
                                            .native_kernel_sources_in_target_role)
                        )
                });
                if paired.count() != 1 {
                    return Err(format!(
                        "architecture object effect lacks its unique empty-linklib proof: {}:{} {}",
                        effect.recipe, effect.line, effect.endpoint
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_one_arch_endpoint_effect(
        &self,
        root: &Path,
        effect: &ArchEndpointEffect,
        verified_definitions: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Result<(), String> {
        let recipe = validate_recipe(&effect.recipe)?;
        let owner = recipe
            .strip_suffix(".src")
            .ok_or_else(|| format!("architecture effect recipe is not a .src owner: {recipe}"))?;
        let bound = self.bound_recipe(owner)?;
        if bound != recipe {
            return Err(format!(
                "architecture effect recipe {recipe} is not the unique bound owner of {owner}"
            ));
        }
        let snapshot_digest = self
            .snapshots
            .get(recipe)
            .ok_or_else(|| format!("architecture effect recipe is not sealed: {recipe}"))?;
        let source_path = aros_common::canonical_source_file(root, Path::new(recipe))
            .map_err(|error| error.to_string())?;
        let (_, source_bytes) =
            aros_common::measure_regular_file_bounded(&source_path, MAX_RECIPE_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("architecture effect source is missing: {recipe}"))?;
        if aros_common::sha256_bytes(&source_bytes).as_str() != snapshot_digest {
            return Err(format!("architecture effect source changed: {recipe}"));
        }
        let source = String::from_utf8(source_bytes)
            .map_err(|_| format!("architecture effect source is not UTF-8: {recipe}"))?;

        // Re-derive from the exact captured recipe with a positional local
        // scope. This prevents a caller from substituting effect data that was
        // not produced by the source invocation at this line.
        let configuration =
            crate::local_make_includes::inline_native_make_configuration_with_templates(
                &source,
                root,
                Path::new(recipe),
                crate::local_make_includes::LocalMakeIncludeLimits::default(),
                &self.architecture_context.make_include_bindings,
                &self.architecture_context.generated_make_templates,
            );
        if configuration.issues.is_empty() {
            for fragment in &configuration.fragments {
                let path = fragment
                    .path
                    .to_str()
                    .ok_or("non-UTF-8 architecture configuration input")?;
                let digest = self
                    .sealed_configuration_inputs
                    .get(path)
                    .or_else(|| self.snapshots.get(path))
                    .ok_or_else(|| {
                        format!("architecture configuration input is not sealed: {path}")
                    })?;
                let file = aros_common::canonical_source_file(root, &fragment.path)
                    .map_err(|error| error.to_string())?;
                let (_, bytes) = aros_common::measure_regular_file_bounded(&file, MAX_RECIPE_BYTES)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| {
                        format!("architecture configuration input is missing: {path}")
                    })?;
                if aros_common::sha256_bytes(&bytes).as_str() != digest {
                    return Err(format!("architecture configuration input changed: {path}"));
                }
            }
        }
        let replay = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
            &source,
            Path::new(recipe),
            root,
            Some(&self.architecture_context),
        )?;
        let replays = replay
            .effects
            .iter()
            .filter(|candidate| {
                candidate.recipe == effect.recipe
                    && candidate.line == effect.line
                    && candidate.endpoint == effect.endpoint
            })
            .collect::<Vec<_>>();
        if replays.len() != 1 || replays[0] != effect {
            return Err(format!(
                "architecture effect does not exactly replay from sealed source: {}:{} {}",
                effect.recipe, effect.line, effect.endpoint
            ));
        }

        let (macro_name, expected_endpoint, expected_dependencies) =
            expected_effect_identity(effect)?;
        if effect.endpoint != expected_endpoint || effect.dependencies != expected_dependencies {
            return Err(format!(
                "architecture effect endpoint/dependencies disagree with its resolved arguments: {}:{}",
                effect.recipe, effect.line
            ));
        }

        let physical_owners = self.graph.owner_files(&effect.endpoint).ok_or_else(|| {
            format!(
                "architecture endpoint has no physical owner: {}",
                effect.endpoint
            )
        })?;
        if physical_owners.len() != 1 || !physical_owners.contains(owner) {
            return Err(format!(
                "architecture endpoint has duplicate or mismatched physical owners: {}",
                effect.endpoint
            ));
        }

        let declarations = self
            .graph
            .declarations()
            .iter()
            .filter(|declaration| declaration.target == effect.endpoint)
            .collect::<Vec<_>>();
        let expected_shapes = match &effect.data {
            ArchEndpointEffectData::SetArchIncludes { .. } => (0, 1),
            ArchEndpointEffectData::ArchModuleObjects { .. }
            | ArchEndpointEffectData::EmptyLinklibAggregate { .. } => (1, 1),
        };
        let mut dependency_declarations = 0usize;
        let mut owner_declarations = 0usize;
        for declaration in declarations {
            if declaration.virtual_target {
                // Any captured source owner can add a virtual dependency to
                // this endpoint. Such edges remain in the graph but are never
                // treated as providers or counted as the physical producer.
                if declaration.bare_marker
                    || declaration.claims_make_owner
                    || declaration.dependencies.is_empty()
                {
                    return Err(format!(
                        "architecture endpoint has a non-dependency virtual collision: {}",
                        effect.endpoint
                    ));
                }
                let origin = self.expansion_origin(declaration).ok_or_else(|| {
                    format!(
                        "architecture endpoint additional dependency lacks source provenance: {}",
                        effect.endpoint
                    )
                })?;
                self.verify_supplemental_virtual_dependency(root, declaration, origin)?;
                continue;
            }
            if declaration.file != owner {
                return Err(format!(
                    "architecture endpoint has a foreign physical declaration: {}",
                    effect.endpoint
                ));
            }
            if declaration.raw_target != effect.endpoint || !declaration.claims_make_owner {
                return Err(format!(
                    "architecture endpoint has a handwritten physical collision: {}",
                    effect.endpoint
                ));
            }
            let origin = self.expansion_origin(declaration).ok_or_else(|| {
                format!(
                    "architecture endpoint declaration lacks expansion provenance: {}",
                    effect.endpoint
                )
            })?;
            // A source-written #MM dependency augments the already-qualified
            // physical owner in this same recipe. It is not another producer:
            // the exact macro owner/dependency shapes are still required below.
            if origin.macro_stack.is_empty()
                && !declaration.bare_marker
                && !declaration.dependencies.is_empty()
            {
                verify_direct_source_dependency(
                    root,
                    source_path.as_path(),
                    recipe,
                    declaration,
                    origin,
                    &source,
                )?;
                continue;
            }
            self.verify_source_macro_origin(
                root,
                source_path.as_path(),
                recipe,
                effect.line,
                effect,
                declaration,
                origin,
                &source,
                macro_name,
                verified_definitions,
            )?;

            if declaration.bare_marker {
                if !declaration.dependencies.is_empty() {
                    return Err(format!(
                        "architecture owner declaration unexpectedly has dependencies: {}",
                        effect.endpoint
                    ));
                }
                owner_declarations += 1;
            } else if declaration.dependencies.len() == 1
                && expected_dependencies.first().is_some_and(|expected| {
                    declaration.dependencies[0].raw_expression == *expected
                        && declaration.dependencies[0].concrete == *expected
                })
            {
                dependency_declarations += 1;
            } else {
                return Err(format!(
                    "architecture endpoint has an unexpected declaration shape: {}",
                    effect.endpoint
                ));
            }
        }
        if (dependency_declarations, owner_declarations) != expected_shapes {
            return Err(format!(
                "architecture endpoint declaration count is not exact: {}",
                effect.endpoint
            ));
        }
        Ok(())
    }

    fn verify_supplemental_virtual_dependency(
        &self,
        root: &Path,
        declaration: &TargetDeclarationProvenance,
        origin: &ExpandedLineProvenance,
    ) -> Result<(), String> {
        if !declaration.virtual_target
            || declaration.bare_marker
            || declaration.claims_make_owner
            || declaration.dependencies.is_empty()
            || origin.output_line != declaration.expanded_line
        {
            return Err(format!(
                "architecture endpoint supplemental declaration is not a pure virtual edge: {}",
                declaration.target
            ));
        }
        let source_recipe = self.bound_recipe(&declaration.file)?;
        let source_recipe = validate_bound_source_path(&source_recipe)?;
        let source_path = aros_common::canonical_source_file(root, Path::new(source_recipe))
            .map_err(|error| error.to_string())?;
        let snapshot_digest = self
            .snapshots
            .get(source_recipe)
            .ok_or_else(|| format!("virtual edge source is not sealed: {source_recipe}"))?;
        let (_, source_bytes) =
            aros_common::measure_regular_file_bounded(&source_path, MAX_RECIPE_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("virtual edge source is missing: {source_recipe}"))?;
        if aros_common::sha256_bytes(&source_bytes).as_str() != snapshot_digest {
            return Err(format!("virtual edge source changed: {source_recipe}"));
        }
        let source = String::from_utf8(source_bytes)
            .map_err(|_| format!("virtual edge source is not UTF-8: {source_recipe}"))?;

        if origin.macro_stack.is_empty() {
            return verify_direct_source_dependency(
                root,
                &source_path,
                source_recipe,
                declaration,
                origin,
                &source,
            );
        }

        let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
        let top_level = origin
            .top_level_source_invocation
            .as_ref()
            .ok_or_else(|| "virtual template edge has no source invocation".to_owned())?;
        verify_callsite(root, &source_path, source_recipe, top_level.line, top_level)?;
        let call_line = source
            .lines()
            .nth(top_level.line - 1)
            .ok_or_else(|| "virtual edge source invocation line is missing".to_owned())?;
        let first = &origin.macro_stack[0];
        let expected_call = format!("%{}", first.name);
        let trimmed_call = call_line.trim_start();
        if !trimmed_call.starts_with(&expected_call)
            || trimmed_call[expected_call.len()..]
                .chars()
                .next()
                .is_some_and(|character| !character.is_whitespace())
        {
            return Err("virtual edge source line does not invoke its first macro frame".into());
        }

        for (index, frame) in origin.macro_stack.iter().enumerate() {
            if frame.name.is_empty() || frame.definition_line == 0 {
                return Err("virtual edge has an incomplete macro frame".into());
            }
            verify_sealed_template_frame(self, &canonical_root, frame)?;
            let invocation = frame.invocation.as_ref().ok_or_else(|| {
                "virtual edge macro frame has no invocation provenance".to_owned()
            })?;
            if invocation.line == 0 {
                return Err("virtual edge macro invocation has an invalid line".into());
            }
            let expected_path = if index == 0 {
                &top_level.path
            } else {
                &origin.macro_stack[index - 1].template_path
            };
            if invocation.path != *expected_path
                || (index == 0 && invocation.line != top_level.line)
            {
                return Err("virtual edge macro frame chain is not source-bound".into());
            }
        }
        let last = origin
            .macro_stack
            .last()
            .ok_or_else(|| "virtual edge has an empty macro stack".to_owned())?;
        if origin.template_path.as_ref() != Some(&last.template_path)
            || origin.template_definition_line != Some(last.definition_line)
            || origin.source_path.as_ref() != Some(&last.template_path)
            || origin.source_line.is_none_or(|line| line == 0)
        {
            return Err("virtual edge template body provenance is inconsistent".into());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_source_macro_origin(
        &self,
        root: &Path,
        canonical_recipe: &Path,
        recipe: &str,
        source_line: usize,
        effect: &ArchEndpointEffect,
        declaration: &TargetDeclarationProvenance,
        origin: &ExpandedLineProvenance,
        source: &str,
        expected_macro: &str,
        verified_definitions: &mut BTreeSet<(PathBuf, usize, String)>,
    ) -> Result<(), String> {
        if origin.output_line != declaration.expanded_line
            || origin.template_definition_line.is_none()
        {
            return Err("architecture declaration has incomplete template provenance".into());
        }
        let matching_frames = origin
            .macro_stack
            .iter()
            .filter(|frame| frame.name == expected_macro)
            .count();
        let Some(frame) = origin.macro_stack.last() else {
            return Err("architecture declaration has an empty expansion stack".into());
        };
        if frame.name != expected_macro || matching_frames != 1 {
            return Err(format!(
                "architecture declaration was not emitted by the exact {expected_macro} frame"
            ));
        }
        if origin.template_path.as_ref() != Some(&frame.template_path)
            || origin.template_definition_line != Some(frame.definition_line)
        {
            return Err("architecture declaration template origin differs from its frame".into());
        }
        verify_invocation_arguments(source, source_line, expected_macro, frame)?;
        verify_frame_arguments(
            frame,
            effect,
            expected_macro,
            self.architecture_context
                .native_kernel_sources_in_target_role,
        )?;

        let callsite = origin
            .top_level_source_invocation
            .as_ref()
            .ok_or_else(|| "architecture declaration lacks a source callsite".to_owned())?;
        verify_callsite(root, canonical_recipe, recipe, source_line, callsite)?;
        let frame_callsite = frame
            .invocation
            .as_ref()
            .ok_or_else(|| "architecture macro frame lacks its invocation callsite".to_owned())?;
        verify_callsite(root, canonical_recipe, recipe, source_line, frame_callsite)?;

        let definition_key = (
            frame.template_path.clone(),
            frame.definition_line,
            expected_macro.to_owned(),
        );
        if verified_definitions.insert(definition_key) {
            verify_sealed_macro_definition(self, root, frame, expected_macro)?;
        }
        Ok(())
    }
}

/// Cross-check the GenMF frame's explicit arguments against the exact source
/// invocation. The effect parser independently interprets that same source
/// call, while this check prevents a provenance frame from being paired with
/// different effective arguments.
fn verify_invocation_arguments(
    source: &str,
    source_line: usize,
    macro_name: &str,
    frame: &MacroExpansionFrame,
) -> Result<(), String> {
    let directive = format!("%{macro_name}");
    let matches = crate::includes::directive_bodies_at(source, &directive)
        .into_iter()
        .filter(|(line, _)| *line + 1 == source_line)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "source does not contain one exact {directive} invocation at line {source_line}"
        ));
    }
    let explicit = parse_invocation_arguments(&matches[0].1, &directive)?;
    let mut expected = frame.arguments.clone();
    for (name, value) in explicit {
        let slot = expected
            .get_mut(&name)
            .ok_or_else(|| format!("{directive} frame does not declare source argument {name}="))?;
        *slot = Some(value);
    }
    if expected != frame.arguments {
        return Err(format!(
            "{directive} expansion arguments differ from source line {source_line}"
        ));
    }
    Ok(())
}

fn parse_invocation_arguments(
    body: &str,
    directive: &str,
) -> Result<BTreeMap<String, String>, String> {
    let tail = body
        .strip_prefix(directive)
        .ok_or_else(|| format!("malformed {directive} invocation"))?;
    if tail
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return Err(format!("malformed {directive} invocation boundary"));
    }
    let mut rest = tail;
    let mut arguments = BTreeMap::new();
    loop {
        rest = rest.trim_start_matches(char::is_whitespace);
        if rest.is_empty() {
            break;
        }
        let bytes = rest.as_bytes();
        if !bytes.first().is_some_and(u8::is_ascii_alphanumeric) {
            return Err(format!("malformed {directive} source argument"));
        }
        let mut key_end = 1;
        while bytes
            .get(key_end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            key_end += 1;
        }
        if key_end == 0 || bytes.get(key_end) != Some(&b'=') {
            return Err(format!("malformed {directive} source argument"));
        }
        let key = &rest[..key_end];
        let value_start = key_end + 1;
        let (value, consumed) = match bytes.get(value_start) {
            None => (String::new(), value_start),
            Some(byte) if byte.is_ascii_whitespace() => (String::new(), value_start),
            Some(b'"') => {
                let after_open = &rest[value_start + 1..];
                let close = after_open
                    .find('"')
                    .ok_or_else(|| format!("unterminated quoted {directive} argument {key}="))?;
                (after_open[..close].to_owned(), value_start + close + 2)
            }
            Some(_) => {
                let mut end = value_start;
                while end < rest.len() {
                    let character = rest[end..]
                        .chars()
                        .next()
                        .ok_or_else(|| format!("malformed {directive} argument"))?;
                    if character.is_whitespace() || character == '"' {
                        break;
                    }
                    end += character.len_utf8();
                }
                if end == value_start {
                    return Err(format!("malformed {directive} argument {key}="));
                }
                (rest[value_start..end].to_owned(), end)
            }
        };
        // GenMF treats an empty/omitted value after an explicit key as an
        // explicit empty string, but a key without '=' is rejected above.
        if arguments.insert(key.to_owned(), value).is_some() {
            return Err(format!("duplicate {directive} argument {key}="));
        }
        rest = &rest[consumed..];
        if !rest.is_empty() && !rest.chars().next().is_some_and(char::is_whitespace) {
            return Err(format!("malformed {directive} argument boundary"));
        }
    }
    Ok(arguments)
}

fn validate_bound_source_path(path: &str) -> Result<&str, String> {
    if path.is_empty()
        || path.contains(['\\', '\0'])
        || Path::new(path).is_absolute()
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("unsafe bound virtual-edge source path: {path:?}"));
    }
    Ok(path)
}

fn verify_direct_source_dependency(
    root: &Path,
    canonical_recipe: &Path,
    recipe: &str,
    declaration: &TargetDeclarationProvenance,
    origin: &ExpandedLineProvenance,
    source: &str,
) -> Result<(), String> {
    let lexical_recipe = root.join(recipe);
    let direct_source = origin
        .source_path
        .as_ref()
        .is_some_and(|path| path == canonical_recipe || path == &lexical_recipe);
    if origin.macro_stack.is_empty()
        && origin.template_path.is_none()
        && origin.template_definition_line.is_none()
        && direct_source
    {
        // Direct source dependencies are preserved, never substituted for
        // the independently verified physical macro producer.
    } else {
        return Err(format!(
            "architecture endpoint extra dependency is not direct source metadata: {}",
            declaration.target
        ));
    }
    let source_line = origin
        .source_line
        .ok_or_else(|| "architecture metadata dependency lacks a source line".to_owned())?;
    if source_line == 0 || origin.output_line != declaration.expanded_line {
        return Err("architecture metadata dependency has invalid line provenance".into());
    }
    let line = crate::metamake_owner_graph::source_declaration_at(source, source_line, recipe)?;
    if !line.is_ascii() {
        return Err("non-ASCII architecture metadata is unsupported".into());
    }
    let trimmed = line.trim_start();
    let marker = if declaration.virtual_target {
        "#MM-"
    } else {
        "#MM"
    };
    let rest = trimmed.strip_prefix(marker).ok_or_else(|| {
        "architecture metadata dependency marker disagrees with its source".to_owned()
    })?;
    if rest.starts_with('-') {
        return Err("malformed virtual architecture metadata marker".into());
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Err("bare #MM- metadata cannot supplement an architecture endpoint".into());
    }
    let (targets, dependencies) = rest.split_once(':').ok_or_else(|| {
        "architecture metadata dependency has no explicit dependency list".to_owned()
    })?;
    if targets.contains(['#', '\\', '"', '\'']) || dependencies.contains(['#', '\\', '"', '\'']) {
        return Err("quoted, escaped, or commented architecture metadata is unsupported".into());
    }
    let targets = targets.split_whitespace().collect::<Vec<_>>();
    let dependencies = dependencies.split_whitespace().collect::<Vec<_>>();
    if targets.len() != 1
        || targets[0] != declaration.raw_target
        || dependencies.len() != declaration.dependencies.len()
        || dependencies
            .iter()
            .zip(&declaration.dependencies)
            .any(|(raw, dependency)| *raw != dependency.raw_expression)
    {
        return Err(format!(
            "architecture metadata dependency source tokens disagree: {}",
            declaration.target
        ));
    }
    if targets
        .iter()
        .chain(&dependencies)
        .any(|token| !token.is_ascii() || token.bytes().any(|byte| byte.is_ascii_control()))
    {
        return Err("unsafe architecture metadata token".into());
    }
    Ok(())
}

fn verify_sealed_template_frame(
    projection: &NativeOwnerProjection,
    canonical_root: &Path,
    frame: &MacroExpansionFrame,
) -> Result<(), String> {
    let canonical_template = fs::canonicalize(&frame.template_path)
        .map_err(|error| format!("cannot resolve virtual-edge template: {error}"))?;
    let relative = canonical_template
        .strip_prefix(canonical_root)
        .map_err(|_| "virtual-edge template is outside source root")?
        .to_str()
        .ok_or("non-UTF-8 virtual-edge template path")?;
    let digest = projection
        .snapshots
        .get(relative)
        .ok_or("virtual-edge template is not sealed in this projection")?;
    let (_, bytes) =
        aros_common::measure_regular_file_bounded(&canonical_template, MAX_TEMPLATE_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or("virtual-edge template is missing")?;
    if aros_common::sha256_bytes(&bytes).as_str() != digest {
        return Err("virtual-edge template changed since projection load".into());
    }
    let template = std::str::from_utf8(&bytes).map_err(|_| "virtual-edge template is not UTF-8")?;
    let lines = template.lines().collect::<Vec<_>>();
    let definitions = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.split_whitespace()
                .take(2)
                .eq(["%define", frame.name.as_str()])
        })
        .map(|(line, _)| line + 1)
        .collect::<Vec<_>>();
    if definitions.as_slice() != [frame.definition_line] {
        return Err(format!(
            "virtual-edge template frame {} is not a unique sealed definition",
            frame.name
        ));
    }
    Ok(())
}

fn expected_effect_identity(
    effect: &ArchEndpointEffect,
) -> Result<(&'static str, String, Vec<String>), String> {
    let (macro_name, mainmmake, tag, suffix) = match &effect.data {
        ArchEndpointEffectData::ArchModuleObjects {
            mainmmake,
            tag,
            module_sources,
            directory,
        } => {
            validate_component(mainmmake)?;
            validate_component(tag)?;
            if module_sources.is_empty()
                || module_sources
                    .iter()
                    .any(|source| validate_component(source).is_err())
            {
                return Err("architecture module effect has invalid source evidence".into());
            }
            let expected_directory = recipe_directory(&effect.recipe);
            if directory.as_str() != expected_directory {
                return Err("architecture module effect directory differs from its recipe".into());
            }
            ("build_archspecific", mainmmake, tag, "")
        }
        ArchEndpointEffectData::EmptyLinklibAggregate {
            mainmmake,
            tag,
            module_sources,
            compiler,
        } => {
            validate_component(mainmmake)?;
            validate_component(tag)?;
            if module_sources.is_empty()
                || module_sources
                    .iter()
                    .any(|source| validate_component(source).is_err())
            {
                return Err("empty linklib effect has invalid source evidence".into());
            }
            if !matches!(compiler.as_str(), "host" | "kernel" | "target") {
                return Err("empty linklib effect has an invalid compiler".into());
            }
            ("build_archspecific", mainmmake, tag, "-linklib")
        }
        ArchEndpointEffectData::SetArchIncludes {
            mainmmake,
            tag,
            modname,
            maindir,
            priority,
            priority_token,
            generated_file,
            order_only_directory,
            ..
        } => {
            validate_component(mainmmake)?;
            validate_component(tag)?;
            validate_component(modname)?;
            validate_relative_path(maindir)?;
            if priority_token.is_empty()
                || !priority_token.bytes().all(|byte| byte.is_ascii_digit())
                || priority_token.parse::<u32>().ok() != Some(*priority)
            {
                return Err("set-archincludes priority token/value mismatch".into());
            }
            let include_dir = format!("gen/{maindir}/{modname}/include");
            let expected_file =
                format!("{include_dir}/.{modname}.includeflag.{priority_token}.{tag}");
            if order_only_directory != &include_dir || generated_file != &expected_file {
                return Err(
                    "set-archincludes output paths disagree with resolved arguments".into(),
                );
            }
            ("set_archincludes", mainmmake, tag, "-set-archincludes")
        }
    };
    let endpoint = format!("{mainmmake}-{tag}{suffix}");
    let dependencies = match &effect.data {
        ArchEndpointEffectData::SetArchIncludes { .. } => Vec::new(),
        _ => vec![format!("{mainmmake}-{tag}-includes")],
    };
    Ok((macro_name, endpoint, dependencies))
}

fn verify_frame_arguments(
    frame: &MacroExpansionFrame,
    effect: &ArchEndpointEffect,
    macro_name: &str,
    projection_kernel_in_target_role: bool,
) -> Result<(), String> {
    let arg = |name: &str| {
        frame
            .resolved_arguments
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("{macro_name} frame lacks resolved {name}="))
    };
    if macro_name == "build_archspecific" {
        let mainmmake = arg("mainmmake")?;
        let tag = arg("arch")?;
        let target_compiler = match &effect.data {
            // A kernel-role declaration yields object groups only under the
            // contract's target-role declaration; the frame must say which.
            ArchEndpointEffectData::ArchModuleObjects { .. }
                if projection_kernel_in_target_role && arg("compiler")? == "kernel" =>
            {
                Some("kernel")
            }
            ArchEndpointEffectData::ArchModuleObjects { .. } => Some("target"),
            ArchEndpointEffectData::EmptyLinklibAggregate { compiler, .. } => {
                Some(compiler.as_str())
            }
            ArchEndpointEffectData::SetArchIncludes { .. } => None,
        };
        if mainmmake.is_empty()
            || tag.is_empty()
            || !["host", "kernel", "target"].contains(&target_compiler.unwrap_or_default())
            || arg("compiler")? != target_compiler.unwrap_or_default()
            || arg("maindir")?.trim().is_empty()
            || !arg("cxxfiles")?.trim().is_empty()
            || !arg("linklibfiles")?.trim().is_empty()
            || !arg("linklibobjs")?.trim().is_empty()
            || (arg("files")?.trim().is_empty() && arg("asmfiles")?.trim().is_empty())
        {
            return Err(
                "build_archspecific frame arguments do not prove an empty linklib lane".into(),
            );
        }
        let (effect_mainmmake, effect_tag, declaration_target) = match &effect.data {
            ArchEndpointEffectData::ArchModuleObjects { mainmmake, tag, .. }
            | ArchEndpointEffectData::EmptyLinklibAggregate { mainmmake, tag, .. } => {
                (mainmmake.as_str(), tag.as_str(), effect.endpoint.as_str())
            }
            ArchEndpointEffectData::SetArchIncludes { .. } => {
                return Err("build_archspecific effect has the wrong data kind".into())
            }
        };
        if effect_mainmmake != mainmmake || effect_tag != tag {
            return Err("build_archspecific resolved mainmmake/tag mismatch".into());
        }
        if declaration_target != format!("{mainmmake}-{tag}")
            && declaration_target != format!("{mainmmake}-{tag}-linklib")
        {
            return Err("build_archspecific frame does not name this declaration".into());
        }
    } else if macro_name == "set_archincludes" {
        let mainmmake = arg("mainmmake")?;
        let tag = arg("arch")?;
        let modname = arg("modname")?;
        let maindir = arg("maindir")?;
        let (effect_mainmmake, effect_tag, effect_modname, priority_token) = match &effect.data {
            ArchEndpointEffectData::SetArchIncludes {
                mainmmake,
                tag,
                modname,
                priority_token,
                ..
            } => (
                mainmmake.as_str(),
                tag.as_str(),
                modname.as_str(),
                priority_token.as_str(),
            ),
            _ => return Err("set_archincludes effect has the wrong data kind".into()),
        };
        if mainmmake.is_empty()
            || tag.is_empty()
            || modname.is_empty()
            || maindir.trim().is_empty()
            || arg("pri")? != priority_token
            || effect_mainmmake != mainmmake
            || effect_tag != tag
            || effect_modname != modname
            || arg("genincdir")? != "yes"
            || effect.endpoint != format!("{mainmmake}-{tag}-set-archincludes")
        {
            return Err("set_archincludes frame arguments do not name this declaration".into());
        }
    } else {
        return Err(format!("unsupported architecture macro {macro_name}"));
    }
    Ok(())
}

fn verify_sealed_macro_definition(
    projection: &NativeOwnerProjection,
    root: &Path,
    frame: &MacroExpansionFrame,
    macro_name: &str,
) -> Result<(), String> {
    let expected_hash = match macro_name {
        "build_archspecific" => BUILD_ARCHSPECIFIC_SHA256,
        "set_archincludes" => SET_ARCHINCLUDES_SHA256,
        "get_archincludes" => GET_ARCHINCLUDES_SHA256,
        _ => return Err(format!("unsupported architecture macro {macro_name}")),
    };
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let canonical_template =
        fs::canonicalize(&frame.template_path).map_err(|error| error.to_string())?;
    let relative = canonical_template
        .strip_prefix(&canonical_root)
        .map_err(|_| "architecture macro template is outside source root")?
        .to_str()
        .ok_or("non-UTF-8 architecture macro template path")?;
    let digest = projection
        .snapshots
        .get(relative)
        .ok_or("architecture macro template is not sealed in this projection")?;
    let (_, bytes) =
        aros_common::measure_regular_file_bounded(&canonical_template, MAX_TEMPLATE_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or("architecture macro template is missing")?;
    if aros_common::sha256_bytes(&bytes).as_str() != digest {
        return Err("architecture macro template changed since projection load".into());
    }
    let template =
        std::str::from_utf8(&bytes).map_err(|_| "architecture macro template is not UTF-8")?;
    verify_macro_definition(template, frame.definition_line, macro_name, expected_hash)
}

fn verify_macro_definition(
    template: &str,
    definition_line: usize,
    macro_name: &str,
    expected_hash: &[&str],
) -> Result<(), String> {
    let lines = template.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.split_whitespace().take(2).eq(["%define", macro_name]))
        .map(|(line, _)| line)
        .collect::<Vec<_>>();
    if starts.len() != 1 || starts[0] + 1 != definition_line {
        return Err(format!(
            "{macro_name} template definition is not unique or has moved"
        ));
    }
    let start = starts[0];
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("%end"))
        .map(|offset| start + 1 + offset)
        .ok_or_else(|| format!("{macro_name} template definition is unterminated"))?;
    if !expected_hash
        .contains(&aros_common::sha256_bytes(lines[start..=end].join("\n").as_bytes()).as_str())
    {
        return Err(format!("{macro_name} template semantics are unsupported"));
    }
    Ok(())
}

fn verify_callsite(
    root: &Path,
    canonical_recipe: &Path,
    recipe: &str,
    expected_line: usize,
    callsite: &SourceLineLocation,
) -> Result<(), String> {
    let lexical_recipe = root.join(recipe);
    if callsite.line != expected_line
        || (callsite.path != lexical_recipe && callsite.path != canonical_recipe)
    {
        return Err(format!(
            "architecture effect callsite does not match sealed source {recipe}:{expected_line}"
        ));
    }
    Ok(())
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
            "unsafe architecture effect recipe path: {recipe:?}"
        ));
    }
    Ok(recipe)
}

fn validate_component(component: &str) -> Result<(), String> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || !component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
    {
        return Err(format!(
            "unsafe architecture effect component: {component:?}"
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', ';', '|', '$'])
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("unsafe architecture effect path: {path:?}"));
    }
    for component in path.split('/') {
        validate_component(component)?;
    }
    Ok(())
}

fn recipe_directory(recipe: &str) -> &str {
    recipe
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory)
}

#[cfg(test)]
mod tests {
    use super::{
        verify_direct_source_dependency, verify_macro_definition, GET_ARCHINCLUDES_SHA256,
    };

    #[test]
    fn native_arch_flag_consumer_requires_exact_append_and_wildcard_semantics() {
        let source = concat!(
            "%define get_archincludes includeflag=USER_INCLUDES maindir=/A modname=/A\n",
            "%(includeflag)_INCFFILES:=$(call WILDCARD, $(GENDIR)/%(maindir)/%(modname)/include/.%(modname).includeflag.*)\n",
            "ifneq ($(%(includeflag)_INCFFILES),)\n",
            "%(includeflag)+=$(shell cat $(%(includeflag)_INCFFILES))\n",
            "endif\n%end",
        );
        assert!(
            verify_macro_definition(source, 1, "get_archincludes", GET_ARCHINCLUDES_SHA256).is_ok()
        );
        assert!(verify_macro_definition(
            &source.replace("+=", ":="),
            1,
            "get_archincludes",
            GET_ARCHINCLUDES_SHA256
        )
        .is_err());
        assert!(verify_macro_definition(
            &source.replace("includeflag.*", "includeflag.5.*"),
            1,
            "get_archincludes",
            GET_ARCHINCLUDES_SHA256
        )
        .is_err());
        assert!(verify_macro_definition(
            &format!("{source}\n{source}"),
            1,
            "get_archincludes",
            GET_ARCHINCLUDES_SHA256
        )
        .is_err());
    }

    #[test]
    fn direct_metadata_adds_required_edges_without_replacing_macro_proof() {
        use crate::genmf_projection::ExpandedLineProvenance;
        use crate::metamake_owner_graph::{DependencyProvenance, TargetDeclarationProvenance};
        use std::path::Path;

        let root = Path::new("/source");
        let path = root.join("arch/mmakefile.src");
        let declaration = TargetDeclarationProvenance {
            file: "arch/mmakefile".into(),
            expanded_line: 1,
            raw_target: "module-cpu".into(),
            target: "module-cpu".into(),
            virtual_target: false,
            bare_marker: false,
            claims_make_owner: true,
            dependencies: vec![DependencyProvenance {
                raw_expression: "required-headers".into(),
                concrete: "required-headers".into(),
            }],
        };
        let origin = ExpandedLineProvenance {
            output_start_byte: 0,
            output_end_byte: 0,
            output_line: 1,
            source_path: Some(path.clone()),
            source_line: Some(1),
            template_path: None,
            template_definition_line: None,
            macro_stack: Vec::new(),
            top_level_source_invocation: None,
        };
        let verify = |text: &str| {
            verify_direct_source_dependency(
                root,
                &path,
                "arch/mmakefile.src",
                &declaration,
                &origin,
                text,
            )
        };
        assert!(verify("#MM module-cpu : required-headers\n").is_ok());
        assert!(verify("#MM module-cpu : \\\n#MM required-headers\n").is_ok());
        assert!(
            verify("#MM module-cpu : \\\n##MM ignored-header \\\n#MM required-headers\n").is_ok()
        );
        for unsupported in [
            "#MM- module-cpu : required-headers\n",
            "#MM module-cpu : changed-headers\n",
            "#MM module-cpu other-owner : required-headers\n",
            "#MM\nmodule-cpu : required-headers\n",
            "#MM module-cpu :\n",
            "#MM module-cpu : \\\n#MM changed-headers\n",
            "#MM module-cpu : \\\nrequired-headers\n",
            "#MM module-cpu : \\\n",
            " #MM module-cpu : required-headers\n",
        ] {
            assert!(verify(unsupported).is_err(), "{unsupported}");
        }
    }

    #[test]
    fn macro_fingerprint_requires_unique_definition_and_exact_body() {
        let template = "%define probe value=\nline\n%end\n";
        let block_hash = aros_common::sha256_bytes(b"%define probe value=\nline\n%end").to_string();
        let block_hash = [block_hash.as_str()];
        let other_revision = "0".repeat(64);
        // Any listed reviewed revision admits the block; none admits a change.
        assert!(verify_macro_definition(
            template,
            1,
            "probe",
            &[other_revision.as_str(), block_hash[0]]
        )
        .is_ok());
        assert!(verify_macro_definition(template, 1, "probe", &[other_revision.as_str()]).is_err());
        assert!(verify_macro_definition(template, 1, "probe", &block_hash).is_ok());
        assert!(verify_macro_definition(template, 2, "probe", &block_hash).is_err());
        assert!(verify_macro_definition(
            "%define probe value=\nchanged\n%end\n",
            1,
            "probe",
            &block_hash
        )
        .is_err());
        assert!(verify_macro_definition(
            "%define probe value=\nline\n%end\n%define probe again=\nother\n%end\n",
            1,
            "probe",
            &block_hash
        )
        .is_err());
    }
}
