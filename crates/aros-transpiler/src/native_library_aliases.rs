//! Source-bound repair of GenMF's punctuation-preserving link-library aliases.
//!
//! A nonempty `uselibs` token becomes `linklibs-<token>` in the sealed module
//! template. The native CMake graph already resolves that token to a typed
//! source provider; this proof only redirects the generated MetaMake edge to
//! that same provider. It never manufactures a provider or excuses a missing
//! handwritten dependency.

use super::{
    charge, native_edge_spellings_proven, verify_reviewed_template_macro,
    NativeMetaSemanticsSelection, NativeOwnerProjection, BUILD_LINKLIB_SHA256,
    BUILD_MODULE_ABI_SHA256, BUILD_MODULE_CORE_SHA256, BUILD_MODULE_LIBRARY_SHA256,
    BUILD_MODULE_SHA256,
};
use crate::ast::{InventoryTargetIdentity, ModuleType};
use crate::graph::{native_endpoint, DependencyGraph};
use crate::metamake_owner_graph::TargetDeclarationProvenance;
use crate::TargetContext;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct VerifiedNativeLibraryAlias {
    /// Consumer recipe which emitted the exact template edge.
    pub recipe: String,
    /// Provider recipe independently bound to the typed `%build_linklib`.
    pub provider_recipe: String,
    /// Concrete MetaMake consumer, including the `-kobj` lane where applicable.
    pub target: String,
    /// Generated `linklibs-<uselibs token>` spelling.
    pub dependency: String,
    /// Actual source-declared provider mmake id.
    pub provider: String,
    /// Exact requested `uselibs` token, punctuation preserved.
    pub library: String,
    pub expanded_line: usize,
    pub source_invocation_line: usize,
    /// True when typed resolution had already replaced this generated edge.
    pub already_rebound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceAliasClaim {
    recipe: String,
    target: String,
    dependency: String,
    main_mmake: String,
    library: String,
    expanded_line: usize,
    source_invocation_line: usize,
}

#[derive(Debug, Clone)]
struct TypedTarget {
    mmake_name: String,
    module_type: ModuleType,
    target_name: String,
    use_libs: Vec<String>,
    link_libs: Vec<String>,
}

impl From<&crate::ast::TargetDefinition> for TypedTarget {
    fn from(target: &crate::ast::TargetDefinition) -> Self {
        Self {
            mmake_name: target.mmake_name.clone(),
            module_type: target.module_type.clone(),
            target_name: target.target_name.clone(),
            use_libs: target.use_libs.clone(),
            link_libs: target.link_libs.clone(),
        }
    }
}

impl From<&InventoryTargetIdentity> for TypedTarget {
    fn from(target: &InventoryTargetIdentity) -> Self {
        Self {
            mmake_name: target.mmake_name.clone(),
            module_type: target.module_type.clone(),
            target_name: target.target_name.clone(),
            use_libs: target.use_libs.clone(),
            link_libs: target.link_libs.clone(),
        }
    }
}

impl NativeOwnerProjection {
    pub(super) fn bind_library_aliases(
        &self,
        root: &Path,
        graph: &mut DependencyGraph,
        selection: NativeMetaSemanticsSelection<'_>,
    ) -> Result<Vec<VerifiedNativeLibraryAlias>, String> {
        self.verify(root)?;
        if selection.roots.is_empty() {
            return Err("native library-alias proof requires resolved roots".into());
        }

        let mut work = 0usize;
        let reachable = graph
            .audit_native_dependency_graph(
                selection.roots,
                selection.context,
                selection.diagnostics,
            )
            .reachable;
        let mut pairs = BTreeSet::<(String, String)>::new();
        let mut claims_by_pair = BTreeMap::<(String, String), Vec<(usize, usize)>>::new();

        // A still-present native alias is a candidate only at its exact
        // reachable consumer edge. The source walk below also discovers the
        // provider edge when the typed CMake resolver has already rebound it.
        for (raw_parent, dependencies) in &graph.meta_targets {
            charge(&mut work)?;
            let Ok(parent) = native_endpoint(raw_parent, selection.context) else {
                continue;
            };
            if !reachable.contains(&parent) {
                continue;
            }
            for raw_child in dependencies {
                charge(&mut work)?;
                let Ok(child) = native_endpoint(raw_child, selection.context) else {
                    continue;
                };
                if is_nonempty_library_alias(&child) {
                    pairs.insert((parent.clone(), child));
                }
            }
        }

        for (declaration_index, declaration) in self.graph.declarations().iter().enumerate() {
            charge(&mut work)?;
            let Ok(target) = native_endpoint(&declaration.target, selection.context) else {
                continue;
            };
            if !reachable.contains(&target) {
                continue;
            }
            for (edge_index, edge) in declaration.dependencies.iter().enumerate() {
                charge(&mut work)?;
                let Ok(dependency) = native_endpoint(&edge.concrete, selection.context) else {
                    continue;
                };
                if is_nonempty_library_alias(&dependency) {
                    let pair = (target.clone(), dependency);
                    pairs.insert(pair.clone());
                    claims_by_pair
                        .entry(pair)
                        .or_default()
                        .push((declaration_index, edge_index));
                }
            }
        }

        let origins = canonical_origins(selection.meta_edge_origins, selection.context, &mut work)?;
        let source_owned_targets: BTreeSet<_> = self
            .graph
            .declarations()
            .iter()
            .filter_map(|declaration| native_endpoint(&declaration.target, selection.context).ok())
            .collect();
        let native_owner_targets: BTreeSet<_> = graph
            .meta_targets
            .keys()
            .filter_map(|target| native_endpoint(target, selection.context).ok())
            .chain(
                graph
                    .make_meta_providers
                    .iter()
                    .filter_map(|target| native_endpoint(target, selection.context).ok()),
            )
            .chain(
                graph
                    .targets
                    .keys()
                    .filter_map(|target| native_endpoint(target, selection.context).ok()),
            )
            .chain(
                graph
                    .inventory_targets
                    .iter()
                    .map(|target| target.mmake_name.clone()),
            )
            .chain(graph.fetches.iter().map(|fetch| fetch.name.clone()))
            .chain(
                graph
                    .source_archives
                    .iter()
                    .map(crate::source_archive_binding::BoundSourceArchive::provider_target),
            )
            .chain(
                graph
                    .external_cmake
                    .iter()
                    .flat_map(|build| [build.mmake_name.clone(), build.provider_target.clone()]),
            )
            .chain(graph.configure_builds.iter().flat_map(|build| {
                std::iter::once(build.mmake_name.clone())
                    .chain(build.provider_target.iter().cloned())
            }))
            .chain(
                graph
                    .default_link_set
                    .iter()
                    .map(|item| item.archive.clone()),
            )
            .collect();
        let rejected_endpoints: BTreeSet<_> = selection
            .diagnostics
            .iter()
            .filter_map(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()
                    .and_then(|context| context.target.as_deref())
                    .and_then(|target| native_endpoint(target, selection.context).ok())
            })
            .collect();
        let mut verified_definitions = BTreeSet::new();
        let mut accepted = Vec::new();
        let mut mutations = Vec::<(String, String, String)>::new();

        for (target, dependency) in pairs {
            charge(&mut work)?;
            if !reachable.contains(&target) || !is_nonempty_library_alias(&dependency) {
                continue;
            }
            if source_owned_targets.contains(&dependency)
                || native_owner_targets.contains(&dependency)
                || rejected_endpoints.contains(&dependency)
                || rejected_endpoints.contains(&target)
            {
                continue;
            }
            let Some(indices) = claims_by_pair.get(&(target.clone(), dependency.clone())) else {
                continue;
            };
            let mut claims = Vec::with_capacity(indices.len());
            let mut rejected_claim = false;
            for (declaration_index, edge_index) in indices {
                charge(&mut work)?;
                let declaration = &self.graph.declarations()[*declaration_index];
                let edge = &declaration.dependencies[*edge_index];
                match self.source_alias_claim(
                    root,
                    declaration,
                    edge,
                    selection.context,
                    &mut verified_definitions,
                )? {
                    Some(claim) if claim.target == target && claim.dependency == dependency => {
                        claims.push(claim);
                    }
                    _ => rejected_claim = true,
                }
            }
            if rejected_claim || claims.is_empty() {
                continue;
            }

            let libraries: BTreeSet<_> = claims.iter().map(|claim| claim.library.clone()).collect();
            let main_mmakes: BTreeSet<_> = claims
                .iter()
                .map(|claim| claim.main_mmake.clone())
                .collect();
            let recipes: BTreeSet<_> = claims.iter().map(|claim| claim.recipe.clone()).collect();
            if libraries.len() != 1 || main_mmakes.len() != 1 {
                continue;
            }
            let library = libraries.into_iter().next().expect("one library");
            let main_mmake = main_mmakes.into_iter().next().expect("one consumer");

            // The parser-origin set is exact, not a permissive subset: an
            // unrelated handwritten/native ingress cannot borrow this proof.
            if origins.get(&(target.clone(), dependency.clone())) != Some(&recipes) {
                continue;
            }
            let Some(provider) = typed_provider(graph, &main_mmake, &library) else {
                continue;
            };
            let Some(provider_recipe) =
                self.unique_source_recipe(root, &provider, &library, &mut verified_definitions)?
            else {
                continue;
            };
            if rejected_endpoints.contains(&provider)
                || rejected_endpoints.contains(&main_mmake)
                || rejected_endpoints.contains(&dependency)
                || rejected_endpoints.contains(&target)
            {
                continue;
            }

            let alias_edge_present =
                has_native_edge(graph, &target, &dependency, selection.context);
            let provider_edge_present =
                has_native_edge(graph, &target, &provider, selection.context);
            let (already_rebound, edge_spellings_proven) = if alias_edge_present {
                let spellings = BTreeSet::from([dependency.clone()]);
                (
                    false,
                    !native_edge_is_explicit(graph, &target, &dependency, selection.context)
                        && native_edge_spellings_proven(
                            graph,
                            &target,
                            &dependency,
                            selection.context,
                            &spellings,
                            false,
                        ),
                )
            } else if provider_edge_present {
                let spellings = BTreeSet::from([provider.clone()]);
                (
                    true,
                    !native_edge_is_explicit(graph, &target, &provider, selection.context)
                        && native_edge_spellings_proven(
                            graph,
                            &target,
                            &provider,
                            selection.context,
                            &spellings,
                            false,
                        ),
                )
            } else {
                continue;
            };
            if !edge_spellings_proven {
                continue;
            }

            for claim in claims {
                accepted.push(VerifiedNativeLibraryAlias {
                    recipe: claim.recipe,
                    provider_recipe: provider_recipe.clone(),
                    target: claim.target,
                    dependency: claim.dependency,
                    provider: provider.clone(),
                    library: claim.library,
                    expanded_line: claim.expanded_line,
                    source_invocation_line: claim.source_invocation_line,
                    already_rebound,
                });
            }
            if !already_rebound {
                mutations.push((target, dependency, provider));
            }
        }

        // Verify the sealed inputs once more immediately before graph mutation.
        self.verify(root)?;
        for (target, dependency, provider) in mutations {
            graph.remove_native_meta_edge(&target, &dependency, selection.context);
            graph.add_meta_rule(crate::ast::MetaTargetRule {
                name: target,
                dependencies: vec![provider],
            });
        }
        accepted.sort();
        Ok(accepted)
    }

    fn source_alias_claim(
        &self,
        root: &Path,
        declaration: &TargetDeclarationProvenance,
        edge: &crate::metamake_owner_graph::DependencyProvenance,
        context: &TargetContext,
        verified: &mut BTreeSet<(PathBuf, usize, String, usize)>,
    ) -> Result<Option<SourceAliasClaim>, String> {
        if declaration.bare_marker || !is_nonempty_library_alias(&edge.concrete) {
            return Ok(None);
        }
        let Some(origin) = self
            .expansion_origins
            .get(&(declaration.file.clone(), declaration.expanded_line))
        else {
            return Ok(None);
        };
        let [wrapper, core] = origin.macro_stack.as_slice() else {
            return Ok(None);
        };
        let wrapper_digest = match wrapper.name.as_str() {
            "build_module" => BUILD_MODULE_SHA256,
            "build_module_abi" => BUILD_MODULE_ABI_SHA256,
            "build_module_library" => BUILD_MODULE_LIBRARY_SHA256,
            _ => return Ok(None),
        };
        if core.name != "build_module_core"
            || core.template_path != wrapper.template_path
            || [core, wrapper].iter().any(|frame| {
                frame
                    .resolved_arguments
                    .get("uselibs")
                    .is_none_or(|value| value.trim().is_empty())
            })
        {
            return Ok(None);
        }
        let Some(main_mmake) = core.resolved_arguments.get("mmake") else {
            return Ok(None);
        };
        if wrapper.resolved_arguments.get("mmake") != Some(main_mmake)
            || wrapper.resolved_arguments.get("uselibs") != core.resolved_arguments.get("uselibs")
        {
            return Ok(None);
        }
        let Some(uselibs) = core.resolved_arguments.get("uselibs") else {
            return Ok(None);
        };
        let Some(library) = uselibs
            .split_whitespace()
            .find(|library| format!("linklibs-{library}") == edge.raw_expression)
        else {
            return Ok(None);
        };
        if uselibs
            .split_whitespace()
            .filter(|candidate| *candidate == library)
            .count()
            != 1
            || edge.raw_expression != format!("linklibs-{library}")
            || native_endpoint(&edge.raw_expression, context)
                .ok()
                .as_deref()
                != Some(edge.concrete.as_str())
        {
            return Ok(None);
        }
        let Some(body_line) = origin.source_line else {
            return Ok(None);
        };
        let recipe = self.bound_recipe(&declaration.file)?;
        let Some(invocation) = &origin.top_level_source_invocation else {
            return Ok(None);
        };
        if (invocation.path != root.join(&recipe)
            && invocation.path
                != aros_common::canonical_source_file(root, Path::new(&recipe))
                    .map_err(|error| error.to_string())?)
            || origin.source_path.as_ref() != Some(&core.template_path)
            || origin.template_path.as_ref() != Some(&core.template_path)
            || origin.template_definition_line != Some(core.definition_line)
        {
            return Err(
                "native library alias expansion is not bound to its source recipe/template".into(),
            );
        }

        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        let relative = core
            .template_path
            .strip_prefix(&canonical_root)
            .map_err(|_| "native library-alias template outside source")?;
        if !self
            .snapshots
            .contains_key(relative.to_str().ok_or("non-UTF8 library-alias template")?)
        {
            return Err("native library-alias template is not sealed".into());
        }
        let template =
            std::fs::read_to_string(&core.template_path).map_err(|error| error.to_string())?;
        let Some(raw) = template.lines().nth(body_line.saturating_sub(1)) else {
            return Ok(None);
        };
        let expected_target = match raw {
            "#M%(build_abi)- %(mmake) : %(mmake)-includes core-linklibs linklibs-%(uselibs)" => {
                main_mmake.clone()
            }
            "#%(build_library)M %(mmake)-kobj : core-linklibs linklibs-%(uselibs)"
            | "#%(build_library)%(build_abi) %(mmake)-kobj : %(mmake)-includes core-linklibs linklibs-%(uselibs)" =>
            {
                format!("{main_mmake}-kobj")
            }
            _ => return Ok(None),
        };
        if declaration.raw_target != expected_target
            || declaration.target != expected_target
            || native_endpoint(&expected_target, context).ok().as_deref()
                != Some(declaration.target.as_str())
        {
            return Ok(None);
        }
        if verified.insert((
            core.template_path.clone(),
            core.definition_line,
            wrapper.name.clone(),
            wrapper.definition_line,
        )) {
            verify_reviewed_template_macro(
                &template,
                "build_module_core",
                core.definition_line,
                BUILD_MODULE_CORE_SHA256,
            )?;
            verify_reviewed_template_macro(
                &template,
                &wrapper.name,
                wrapper.definition_line,
                wrapper_digest,
            )?;
        }
        Ok(Some(SourceAliasClaim {
            recipe,
            target: declaration.target.clone(),
            dependency: edge.concrete.clone(),
            main_mmake: main_mmake.clone(),
            library: library.to_owned(),
            expanded_line: declaration.expanded_line,
            source_invocation_line: invocation.line,
        }))
    }

    fn unique_source_recipe(
        &self,
        root: &Path,
        provider: &str,
        library: &str,
        verified: &mut BTreeSet<(PathBuf, usize, String, usize)>,
    ) -> Result<Option<String>, String> {
        let Some(files) = self.graph.owner_files(provider) else {
            return Ok(None);
        };
        let files: Vec<_> = files.iter().collect();
        let [file] = files.as_slice() else {
            return Ok(None);
        };
        let recipe = self.bound_recipe(file)?;
        if !root.join(&recipe).is_file() {
            return Ok(None);
        }

        let mut provider_claims = 0usize;
        let mut producer_claims = 0usize;
        let mut invocation_lines = BTreeSet::new();
        for declaration in self.graph.declarations() {
            if declaration.target != provider || !declaration.claims_make_owner {
                continue;
            }
            provider_claims += 1;
            if declaration.bare_marker
                || declaration.raw_target != provider
                || declaration.file.as_str() != file.as_str()
            {
                return Ok(None);
            }
            let Some(origin) = self
                .expansion_origins
                .get(&(declaration.file.clone(), declaration.expanded_line))
            else {
                return Ok(None);
            };
            if origin.macro_stack.is_empty() {
                // A direct #MM rule in the same provider recipe may add a
                // required fetch prerequisite (zlib's no-gzip archive does
                // exactly this). Bind its physical source line and raw edge
                // tokens before accepting it as supplemental ownership; it
                // never substitutes for the generated producer below.
                if !source_supplemental_provider_rule(root, &recipe, declaration, origin) {
                    return Ok(None);
                }
                continue;
            }
            let [caller] = origin.macro_stack.as_slice() else {
                return Ok(None);
            };
            if caller.name != "build_linklib"
                || caller.resolved_arguments.get("mmake").map(String::as_str) != Some(provider)
                || caller.resolved_arguments.get("libname").map(String::as_str) != Some(library)
            {
                return Ok(None);
            }
            let Some(body_line) = origin.source_line else {
                return Ok(None);
            };
            let Some(invocation) = &origin.top_level_source_invocation else {
                return Ok(None);
            };
            let canonical_recipe = aros_common::canonical_source_file(root, Path::new(&recipe))
                .map_err(|error| error.to_string())?;
            let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
            let relative_template = caller
                .template_path
                .strip_prefix(&canonical_root)
                .map_err(|_| "link-library provider template outside source")?;
            if !self.snapshots.contains_key(
                relative_template
                    .to_str()
                    .ok_or("non-UTF8 link-library provider template")?,
            ) {
                return Err("link-library provider template is not sealed".into());
            }
            let template = std::fs::read_to_string(&caller.template_path)
                .map_err(|error| error.to_string())?;
            let raw = template.lines().nth(body_line.saturating_sub(1));
            if (invocation.path != root.join(&recipe) && invocation.path != canonical_recipe)
                || origin.source_path.as_ref() != Some(&caller.template_path)
                || origin.template_path.as_ref() != Some(&caller.template_path)
                || origin.template_definition_line != Some(caller.definition_line)
                || raw != Some("#MM %(mmake) : includes-generate-deps")
            {
                return Ok(None);
            }
            let identity = (
                caller.template_path.clone(),
                caller.definition_line,
                caller.name.clone(),
                caller.definition_line,
            );
            if verified.insert(identity) {
                verify_reviewed_template_macro(
                    &template,
                    "build_linklib",
                    caller.definition_line,
                    BUILD_LINKLIB_SHA256,
                )?;
            }
            if !declaration.dependencies.iter().any(|edge| {
                edge.raw_expression == "includes-generate-deps"
                    && edge.concrete == "includes-generate-deps"
            }) {
                return Ok(None);
            }
            producer_claims += 1;
            invocation_lines.insert(invocation.line);
        }
        if provider_claims == 0 || producer_claims != 1 || invocation_lines.len() != 1 {
            return Ok(None);
        }
        Ok(Some(recipe))
    }
}

fn source_supplemental_provider_rule(
    root: &Path,
    recipe: &str,
    declaration: &TargetDeclarationProvenance,
    origin: &crate::genmf_projection::ExpandedLineProvenance,
) -> bool {
    const MAX_PROVIDER_RECIPE_BYTES: u64 = 1024 * 1024;

    let Ok(recipe_path) = aros_common::canonical_source_file(root, Path::new(recipe)) else {
        return false;
    };
    let Some(source_line) = origin.source_line else {
        return false;
    };
    let lexical_recipe_path = root.join(recipe);
    if (origin.source_path.as_ref() != Some(&recipe_path)
        && origin.source_path.as_ref() != Some(&lexical_recipe_path))
        || origin.template_path.is_some()
        || origin.template_definition_line.is_some()
        || origin.top_level_source_invocation.is_some()
        || origin.output_line != declaration.expanded_line
        || !origin.macro_stack.is_empty()
    {
        return false;
    }
    let Ok(Some((_, bytes))) =
        aros_common::measure_regular_file_bounded(&recipe_path, MAX_PROVIDER_RECIPE_BYTES)
    else {
        return false;
    };
    let Ok(source) = String::from_utf8(bytes) else {
        return false;
    };
    let Ok(logical) =
        crate::metamake_owner_graph::source_declaration_at(&source, source_line, recipe)
    else {
        return false;
    };
    let Some(raw_rule) = logical.strip_prefix("#MM") else {
        return false;
    };
    if raw_rule.starts_with('-') {
        return false;
    }
    let Some((raw_targets, raw_dependencies)) = raw_rule.split_once(':') else {
        return false;
    };
    let targets: Vec<_> = raw_targets.split_whitespace().collect();
    if targets.as_slice() != [declaration.raw_target.as_str()] {
        return false;
    }
    let dependencies: Vec<_> = raw_dependencies.split_whitespace().collect();
    if dependencies.len() != declaration.dependencies.len() {
        return false;
    }
    declaration
        .dependencies
        .iter()
        .zip(dependencies)
        .all(|(edge, raw)| edge.raw_expression == raw)
}

fn is_nonempty_library_alias(endpoint: &str) -> bool {
    endpoint
        .strip_prefix("linklibs-")
        .is_some_and(|library| !library.is_empty())
}

fn canonical_origins(
    raw: &BTreeMap<(String, String), BTreeSet<String>>,
    context: &TargetContext,
    work: &mut usize,
) -> Result<BTreeMap<(String, String), BTreeSet<String>>, String> {
    let mut origins = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for ((parent, dependency), files) in raw {
        charge(work)?;
        if let (Ok(parent), Ok(dependency)) = (
            native_endpoint(parent, context),
            native_endpoint(dependency, context),
        ) {
            origins
                .entry((parent, dependency))
                .or_default()
                .extend(files.iter().cloned());
        }
    }
    Ok(origins)
}

fn typed_provider(graph: &DependencyGraph, main_mmake: &str, library: &str) -> Option<String> {
    let declarations: Vec<TypedTarget> = graph
        .targets
        .values()
        .map(TypedTarget::from)
        .chain(graph.inventory_targets.iter().map(TypedTarget::from))
        .collect();
    let consumers: Vec<_> = declarations
        .iter()
        .filter(|target| target.mmake_name == main_mmake)
        .collect();
    let [consumer] = consumers.as_slice() else {
        return None;
    };
    if !consumer
        .use_libs
        .iter()
        .any(|requested| requested == library)
    {
        return None;
    }

    let candidates: BTreeSet<_> = declarations
        .iter()
        .filter(|target| target.module_type == ModuleType::LinkLib && target.target_name == library)
        .map(|target| target.mmake_name.clone())
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let selected: BTreeSet<_> = consumer
        .link_libs
        .iter()
        .filter(|provider| candidates.contains(*provider))
        .cloned()
        .collect();
    let selected: Vec<_> = selected.into_iter().collect();
    let [provider] = selected.as_slice() else {
        return None;
    };
    let source_declarations: Vec<_> = declarations
        .iter()
        .filter(|target| target.mmake_name == *provider)
        .collect();
    if source_declarations.len() != 1
        || source_declarations[0].module_type != ModuleType::LinkLib
        || source_declarations[0].target_name != library
    {
        return None;
    }
    Some(provider.clone())
}

fn has_native_edge(
    graph: &DependencyGraph,
    target: &str,
    dependency: &str,
    context: &TargetContext,
) -> bool {
    graph.meta_targets.iter().any(|(parent, children)| {
        native_endpoint(parent, context).ok().as_deref() == Some(target)
            && children
                .iter()
                .any(|child| native_endpoint(child, context).ok().as_deref() == Some(dependency))
    })
}

fn native_edge_is_explicit(
    graph: &DependencyGraph,
    target: &str,
    dependency: &str,
    context: &TargetContext,
) -> bool {
    graph.explicit_meta_edges.iter().any(|(parent, child)| {
        native_endpoint(parent, context).ok().as_deref() == Some(target)
            && native_endpoint(child, context).ok().as_deref() == Some(dependency)
    })
}

#[cfg(test)]
#[path = "native_library_alias_tests.rs"]
mod tests;
