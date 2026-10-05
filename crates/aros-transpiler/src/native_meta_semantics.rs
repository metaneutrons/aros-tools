//! Native projection of declared virtual routes and explicit selector hooks.
//!
//! Classic missing-target tolerance is deliberately not an admission rule.
//! No physical Make owner can become a producer through this metadata bridge.

use super::NativeOwnerProjection;
use crate::ast::MetaTargetRule;
use crate::graph::native_endpoint;
use crate::{DependencyGraph, TargetContext};
use aros_common::native_build_contract::{NativeMetaAbsence, NativeOptionalMetaDependency};
use aros_common::Diagnostic;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_WORK: usize = 4_000_000;
const MAX_ALIASES: usize = 20_000;

#[path = "native_empty_library_lists.rs"]
mod empty_library_lists;
pub use empty_library_lists::VerifiedEmptyLibraryList;

#[path = "native_library_aliases.rs"]
mod library_aliases;
pub use library_aliases::VerifiedNativeLibraryAlias;

// Reviewed full GenMF definition-block fingerprints from the audited P4
// config/make.tmpl. Hashing the complete block makes added calls or side effects
// invalidate the caller-chain proof, not only edits to the visible callsite.
const GEN_ARCHSPECIFICRULES_SHA256: &str =
    "fc50253aa57b06aac5fdd739e2e871391d4e9a78d84989f83e52af71b162a062";
const BUILD_MODULE_CORE_SHA256: &str =
    "b98d01e189affeb9f09c965bb42816719e5faf55f156332ffeb810a402526b53";
const BUILD_MODULE_SHA256: &str =
    "8201f536deaec8ae141a7e66cbfda2665a21664bccab9437d7723f08a8b48705";
const BUILD_MODULE_ABI_SHA256: &str =
    "f2f9879bc5af8ecf68745a9878eb8efbb6f3a9d4e48752393bb528fe1bd367e3";
const BUILD_MODULE_LIBRARY_SHA256: &str =
    "b9d589d7528b09d17c187eae39c7bcce5df16e0a3575166b98489040d0b9ced5";
const BUILD_LINKLIB_SHA256: &str =
    "5cca7ef8788eb0fb09fdcd5e72def0b05e5a2e0b9cb00c7052858aee62ac357c";
const BUILD_PROG_SHA256: &str = "e276e953080db3a15e844057a5d2454c0c537da800904c9544c0c106566d375e";
const BUILD_PROGS_SHA256: &str = "4c0bbfa21ec9c3d27dbb2724d7b67f37db5326d1c814973389907f1269b32b5b";
// Exact small direct-caller fixture used by this module's adversarial tests.
const DIRECT_FIXTURE_BUILD_MODULE_SHA256: &str =
    "a6cd9dca47f203a9727e57048a155b0e1400f8fbdb4b2104d4887964a85d2715";

#[derive(Debug, Serialize)]
pub struct SourceVirtualAlias {
    pub name: String,
    pub dependencies: BTreeSet<String>,
    pub recipes: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct SourceArchitectureHookOmission {
    pub target: String,
    pub dependency: String,
    pub recipes: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct VerifiedSelectorContract {
    pub recipe: String,
    pub target: String,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct VerifiedTemplateHook {
    pub recipe: String,
    pub target: String,
    pub expression: String,
    pub expanded_line: usize,
    pub family: String,
    pub source_invocation_line: usize,
}

/// Source-backed metadata routing, never executable producer qualification.
#[derive(Debug, Default, Serialize)]
pub struct NativeMetaSemanticsEvidence {
    pub virtual_aliases: Vec<SourceVirtualAlias>,
    pub architecture_hook_omissions: Vec<SourceArchitectureHookOmission>,
    pub verified_selector_contracts: BTreeSet<VerifiedSelectorContract>,
    /// Exact callsites selected by the sealed source policy. This is not a
    /// target-name allowlist or an executable producer declaration.
    pub verified_template_hooks: BTreeSet<VerifiedTemplateHook>,
    /// Empty GenMF library-list substitutions proved at their exact callsite.
    /// Handwritten or nonempty missing library dependencies remain errors.
    pub empty_library_list_omissions: Vec<VerifiedEmptyLibraryList>,
    /// Generated library-interface spellings redirected to the independently
    /// source-bound typed archive producer, retaining the prerequisite.
    pub library_alias_bindings: Vec<VerifiedNativeLibraryAlias>,
}

/// Exact inputs of the native selection, with no ambient fallback.
#[derive(Clone, Copy)]
pub struct NativeMetaSemanticsSelection<'a> {
    pub context: &'a TargetContext,
    pub roots: &'a [String],
    pub declarations: &'a [NativeOptionalMetaDependency],
    pub diagnostics: &'a [Diagnostic],
    /// Actual parser inputs of each raw native metadata edge, not aggregate
    /// endpoint origins (which include unrelated sibling prerequisites).
    pub meta_edge_origins: &'a BTreeMap<(String, String), BTreeSet<String>>,
}

impl NativeMetaSemanticsEvidence {
    /// Whether the exact source contract edge was independently verified.
    #[must_use]
    pub fn verifies(&self, edge: &NativeOptionalMetaDependency) -> bool {
        edge.absence == NativeMetaAbsence::Selector
            && self
                .verified_selector_contracts
                .contains(&VerifiedSelectorContract {
                    recipe: edge.recipe.clone(),
                    target: edge.target.clone(),
                    expression: edge.dependency.clone(),
                })
    }
}

impl NativeOwnerProjection {
    /// Import only reachable, source-declared pure virtual routes. Optional
    /// leaves require an exact selector contract and complete edge provenance.
    /// Real source owners, literal typos and rejected producers stay required.
    ///
    /// # Errors
    /// Refuses changed inputs, unmatched source contracts, unsafe bindings and
    /// bounded traversal excess. Does not admit disabled concrete owners.
    pub fn bind_native_meta_semantics(
        &self,
        root: &Path,
        graph: &mut DependencyGraph,
        selection: NativeMetaSemanticsSelection<'_>,
        parser_origins: &mut BTreeMap<String, BTreeSet<String>>,
    ) -> Result<NativeMetaSemanticsEvidence, String> {
        let NativeMetaSemanticsSelection {
            context,
            roots,
            declarations,
            diagnostics,
            meta_edge_origins,
        } = selection;
        self.verify(root)?;
        if roots.is_empty() {
            return Err("native metadata semantics require resolved roots".into());
        }
        let mut work = 0usize;
        let mut evidence = NativeMetaSemanticsEvidence::default();
        let mut edge_origins = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for ((target, dependency), origins) in meta_edge_origins {
            charge(&mut work)?;
            if let (Ok(target), Ok(dependency)) = (
                native_endpoint(target, context),
                native_endpoint(dependency, context),
            ) {
                edge_origins
                    .entry((target, dependency))
                    .or_default()
                    .extend(origins.iter().cloned());
            }
        }
        let mut by_target = BTreeMap::<String, Vec<_>>::new();
        for declaration in self.graph.declarations() {
            charge(&mut work)?;
            by_target
                .entry(declaration.target.clone())
                .or_default()
                .push(declaration);
        }

        // Bind virtual declarations before looking at optional leaves. Real
        // owners never become producers here. For an already represented
        // endpoint, union virtual metadata without replacing its provider.
        let mut imported = BTreeSet::new();
        let mut imported_edges = BTreeSet::new();
        let mut native_meta_edges = bound_meta_edges(graph, context, &mut work)?;
        loop {
            let audit = graph.audit_native_dependency_graph(roots, context, diagnostics);
            let known_native = graph.native_metadata_endpoint_names(context);
            let mut added = false;
            for name in audit.reachable {
                charge(&mut work)?;
                if !self.graph.known_targets().contains(&name)
                    || imported.contains(&name)
                    || (self.graph.owner_files(&name).is_some() && !known_native.contains(&name))
                {
                    continue;
                }
                let source_declarations = by_target.get(&name).ok_or_else(|| {
                    format!("source virtual target {name} lacks declaration provenance")
                })?;
                let mut recipes = BTreeSet::new();
                let mut dependencies = BTreeSet::new();
                let mut dependency_recipes = BTreeMap::<String, BTreeSet<String>>::new();
                for declaration in source_declarations {
                    charge(&mut work)?;
                    if !declaration.virtual_target || declaration.claims_make_owner {
                        continue;
                    }
                    let recipe = self.bound_recipe(&declaration.file)?;
                    recipes.insert(recipe.clone());
                    for edge in &declaration.dependencies {
                        charge(&mut work)?;
                        dependencies.insert(edge.concrete.clone());
                        dependency_recipes
                            .entry(edge.concrete.clone())
                            .or_default()
                            .insert(recipe.clone());
                    }
                }
                if recipes.is_empty() {
                    imported.insert(name);
                    continue;
                }
                for dependency in &dependencies {
                    charge(&mut work)?;
                    require_concrete(dependency, context)?;
                    // Only edges introduced here can use a concrete spelling
                    // as selector proof. A pre-existing literal is not waived.
                    if !native_meta_edges.contains(&(name.clone(), dependency.clone())) {
                        imported_edges.insert((name.clone(), dependency.clone()));
                        edge_origins
                            .entry((name.clone(), dependency.clone()))
                            .or_default()
                            .extend(dependency_recipes[dependency].iter().cloned());
                    }
                }
                require_concrete(&name, context)?;
                let added_dependencies: Vec<_> = dependencies
                    .iter()
                    .filter(|dependency| {
                        !native_meta_edges.contains(&(name.clone(), (*dependency).clone()))
                    })
                    .cloned()
                    .collect();
                if known_native.contains(&name) && added_dependencies.is_empty() {
                    imported.insert(name);
                    continue;
                }
                native_meta_edges.extend(
                    added_dependencies
                        .iter()
                        .map(|child| (name.clone(), child.clone())),
                );
                graph.add_meta_rule(MetaTargetRule {
                    name: name.clone(),
                    dependencies: added_dependencies,
                });
                parser_origins
                    .entry(name.clone())
                    .or_default()
                    .extend(recipes.iter().cloned());
                evidence.virtual_aliases.push(SourceVirtualAlias {
                    name: name.clone(),
                    dependencies,
                    recipes,
                });
                imported.insert(name);
                if evidence.virtual_aliases.len() > MAX_ALIASES {
                    return Err("native virtual route limit exceeded".into());
                }
                added = true;
            }
            if !added {
                break;
            }
        }

        let selected = graph
            .audit_native_dependency_graph(roots, context, diagnostics)
            .reachable;
        // A concrete parent does not make its independently generated
        // variant extension mandatory. Permit that proof only for actual
        // architecture effects reverified against the sealed source; a bare
        // owner, arbitrary native alias or rejected producer is insufficient.
        self.verify_arch_endpoint_effects(root, &graph.arch_endpoint_effects)?;
        let qualified_physical_parents: BTreeSet<_> = graph
            .arch_endpoint_effects
            .iter()
            .map(|effect| effect.endpoint.clone())
            .collect();
        let mut seeds = BTreeMap::<(String, String), BTreeSet<String>>::new();
        let mut template_definitions = BTreeSet::new();
        for (target, source_declarations) in &by_target {
            if !selected.contains(target) {
                continue;
            }
            for declaration in source_declarations {
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    if let Some(proof) = self.template_variant_hook(
                        root,
                        declaration,
                        source_edge,
                        &qualified_physical_parents,
                        &mut template_definitions,
                    )? {
                        if native_meta_edges
                            .contains(&(target.clone(), source_edge.concrete.clone()))
                        {
                            seeds
                                .entry((target.clone(), source_edge.concrete.clone()))
                                .or_default()
                                .insert(proof.recipe.clone());
                            evidence.verified_template_hooks.insert(proof);
                        }
                    }
                }
            }
        }
        for edge in declarations
            .iter()
            .filter(|edge| edge.absence == NativeMetaAbsence::Selector)
        {
            charge(&mut work)?;
            let dependency =
                native_endpoint(&edge.dependency, context).map_err(|e| e.to_string())?;
            let mut matches = false;
            for declaration in by_target.get(&edge.target).into_iter().flatten() {
                charge(&mut work)?;
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    matches |= self.owners.get(&declaration.file) == Some(&edge.recipe)
                        && source_edge.concrete == dependency
                        && architecture_expression(&source_edge.raw_expression).as_deref()
                            == Some(edge.dependency.as_str());
                }
            }
            if !matches || !native_meta_edges.contains(&(edge.target.clone(), dependency.clone())) {
                return Err(format!(
                    "selector contract {} -> {} is not its exact source declaration in {}",
                    edge.target, edge.dependency, edge.recipe
                ));
            }
            evidence
                .verified_selector_contracts
                .insert(VerifiedSelectorContract {
                    recipe: edge.recipe.clone(),
                    target: edge.target.clone(),
                    expression: edge.dependency.clone(),
                });
            if selected.contains(&edge.target) {
                seeds
                    .entry((edge.target.clone(), dependency))
                    .or_default()
                    .insert(edge.recipe.clone());
            }
        }

        // Contract seeds are exact source-owned edges. Their declared virtual
        // routes may have selector leaves, but not arbitrary literal holes.
        let mut candidates = BTreeMap::<(String, String), bool>::new();
        for (target, dependency) in seeds.keys() {
            candidates.insert((target.clone(), dependency.clone()), true);
            let mut pending = BTreeSet::from([dependency.clone()]);
            let mut visited = BTreeSet::new();
            while let Some(name) = pending.pop_first() {
                charge(&mut work)?;
                if !visited.insert(name.clone()) || self.graph.owner_files(&name).is_some() {
                    continue;
                }
                if let Some(children) = self.graph.dependencies(&name) {
                    for child in children {
                        charge(&mut work)?;
                        candidates
                            .entry((name.clone(), child.clone()))
                            .or_insert(false);
                        pending.insert(child.clone());
                    }
                }
            }
        }
        let rejected: BTreeSet<_> = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()?
                    .target
                    .as_ref()
                    .and_then(|name| native_endpoint(name, context).ok())
            })
            .collect();
        // A seed can cut required ingress only when *all* claims agree,
        // including native origins/spellings. One colliding declaration must
        // not authorize omissions further down a shared virtual route.
        let mut cuts = BTreeSet::new();
        for (target, dependency) in seeds.keys() {
            charge(&mut work)?;
            let mut recipes = BTreeSet::new();
            let mut expressions = BTreeSet::new();
            let mut justified = true;
            let mut claims = 0usize;
            for declaration in by_target.get(target).into_iter().flatten() {
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    if &source_edge.concrete != dependency {
                        continue;
                    }
                    claims += 1;
                    let recipe = self.bound_recipe(&declaration.file)?;
                    if let Some(expression) = architecture_expression(&source_edge.raw_expression) {
                        justified &= evidence.verified_selector_contracts.contains(
                            &VerifiedSelectorContract {
                                recipe: recipe.clone(),
                                target: target.clone(),
                                expression: expression.clone(),
                            },
                        ) || template_hook_verified(
                            &evidence,
                            declaration,
                            &recipe,
                            &expression,
                        );
                        expressions.insert(expression);
                    } else {
                        justified = false;
                    }
                    recipes.insert(recipe);
                }
            }
            if claims > 0
                && justified
                && edge_origins
                    .get(&(target.clone(), dependency.clone()))
                    .is_some_and(|origins| origins.is_subset(&recipes))
                && native_edge_spellings_proven(
                    graph,
                    target,
                    dependency,
                    context,
                    &expressions,
                    imported_edges.contains(&(target.clone(), dependency.clone())),
                )
            {
                cuts.insert((target.clone(), dependency.clone()));
            }
        }
        let required = graph.native_required_metadata_reachability(roots, context, &cuts)?;
        // A native-only concrete provider also prevents omission. Missing
        // endpoints have no registry entry; diagnostic-only endpoints above
        // remain protected even when they have no source Make owner.
        let known_native = graph.native_metadata_endpoint_names(context);
        let mut omissions = Vec::new();
        for ((target, dependency), seed) in candidates {
            charge(&mut work)?;
            if self.graph.known_targets().contains(&dependency)
                || known_native.contains(&dependency)
                || rejected.contains(&dependency)
                || !native_meta_edges.contains(&(target.clone(), dependency.clone()))
                || (!seed && required.contains(&target))
            {
                continue;
            }
            let mut recipes = BTreeSet::new();
            let mut justified = true;
            let mut claims = 0usize;
            let mut expressions = BTreeSet::new();
            for declaration in by_target.get(&target).into_iter().flatten() {
                charge(&mut work)?;
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    if source_edge.concrete != dependency {
                        continue;
                    }
                    claims += 1;
                    let recipe = self.bound_recipe(&declaration.file)?;
                    let expression = architecture_expression(&source_edge.raw_expression);
                    if let Some(expression) = &expression {
                        expressions.insert(expression.clone());
                    }
                    let exact_seed = expression.as_ref().is_some_and(|expression| {
                        evidence
                            .verified_selector_contracts
                            .contains(&VerifiedSelectorContract {
                                recipe: recipe.clone(),
                                target: target.clone(),
                                expression: expression.clone(),
                            })
                            || template_hook_verified(&evidence, declaration, &recipe, expression)
                    });
                    // Every source declaration of this unioned edge must
                    // agree. A colliding literal or nonvirtual route cannot
                    // borrow another file's optional selector proof.
                    if expression.is_none()
                        || (seed && !exact_seed)
                        || (!seed && (!declaration.virtual_target || declaration.claims_make_owner))
                    {
                        justified = false;
                    }
                    recipes.insert(recipe);
                }
            }
            if claims == 0 || !justified {
                continue;
            }
            // Reject extra native origins and literal/native-generated edges
            // which merely collide with a proven selector's bound identity.
            if edge_origins
                .get(&(target.clone(), dependency.clone()))
                .is_none_or(|origins| !origins.is_subset(&recipes))
                || !native_edge_spellings_proven(
                    graph,
                    &target,
                    &dependency,
                    context,
                    &expressions,
                    imported_edges.contains(&(target.clone(), dependency.clone())),
                )
            {
                continue;
            }
            omissions.push(SourceArchitectureHookOmission {
                target,
                dependency,
                recipes,
            });
        }
        // No graph mutation of absent edges until every seed is verified.
        self.verify(root)?;
        for omission in &omissions {
            graph.remove_native_meta_edge(&omission.target, &omission.dependency, context);
        }
        evidence.architecture_hook_omissions = omissions;
        evidence.library_alias_bindings = self.bind_library_aliases(
            root,
            graph,
            NativeMetaSemanticsSelection {
                context,
                roots,
                declarations,
                diagnostics,
                meta_edge_origins: &edge_origins,
            },
        )?;
        evidence.empty_library_list_omissions = self.bind_empty_library_lists(
            root,
            graph,
            NativeMetaSemanticsSelection {
                context,
                roots,
                declarations,
                diagnostics,
                // Include exact provenance of virtual source edges imported
                // above; they have no earlier raw-parser origin. Existing
                // native edges retain their independently supplied origins.
                meta_edge_origins: &edge_origins,
            },
        )?;
        Ok(evidence)
    }

    /// Only the variant slot of the independently pinned template is optional.
    fn template_variant_hook(
        &self,
        root: &Path,
        declaration: &crate::metamake_owner_graph::TargetDeclarationProvenance,
        edge: &crate::metamake_owner_graph::DependencyProvenance,
        qualified_physical_parents: &BTreeSet<String>,
        verified_definitions: &mut BTreeSet<(std::path::PathBuf, usize, usize, String, usize)>,
    ) -> Result<Option<VerifiedTemplateHook>, String> {
        if self.architecture_hook_families.is_empty()
            || !declaration.virtual_target
            || declaration.bare_marker
            || declaration.claims_make_owner
            || (self.graph.owner_files(&declaration.target).is_some()
                && !qualified_physical_parents.contains(&declaration.target))
            || !edge.raw_expression.contains("$(AROS_TARGET_VARIANT)")
        {
            return Ok(None);
        }
        let Some(origin) = self
            .expansion_origins
            .get(&(declaration.file.clone(), declaration.expanded_line))
        else {
            return Ok(None);
        };
        let Some(frame) = origin
            .macro_stack
            .last()
            .filter(|frame| frame.name == "gen_archspecificrules")
        else {
            return Ok(None);
        };
        let Some(caller) = origin.macro_stack.iter().rev().nth(1).filter(|frame| {
            matches!(
                frame.name.as_str(),
                "build_module"
                    | "build_module_core"
                    | "build_linklib"
                    | "build_prog"
                    | "build_progs"
            )
        }) else {
            return Ok(None);
        };
        if caller.template_path != frame.template_path {
            return Ok(None);
        }
        let wrapper = if caller.name == "build_module_core" {
            let Some(wrapper) = origin.macro_stack.iter().rev().nth(2).filter(|frame| {
                matches!(
                    frame.name.as_str(),
                    "build_module" | "build_module_abi" | "build_module_library"
                )
            }) else {
                return Ok(None);
            };
            if wrapper.template_path != frame.template_path {
                return Ok(None);
            }
            wrapper
        } else {
            caller
        };
        let args = &frame.resolved_arguments;
        let Some(main) = args.get("mainmmake") else {
            return Ok(None);
        };
        let Some(suffix) = args.get("target") else {
            return Ok(None);
        };
        if args.get("subtarget").is_none_or(|value| !value.is_empty())
            || main.is_empty()
            || !main.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+')
            })
        {
            return Ok(None);
        }
        let family = match suffix.as_str() {
            "" => "module",
            "-linklib" => "linklib",
            "-set-archincludes" => "set-archincludes",
            _ => return Ok(None),
        };
        // Archives and programs declare only the base architecture slot. Their
        // quick or suffix-specific routes are not admitted by this proof.
        if matches!(
            caller.name.as_str(),
            "build_linklib" | "build_prog" | "build_progs"
        ) && !suffix.is_empty()
        {
            return Ok(None);
        }
        if !self.architecture_hook_families.contains(family)
            || declaration.raw_target != format!("{main}-$(ARCH)-$(CPU){suffix}")
            || edge.raw_expression
                != format!("{main}-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT){suffix}")
        {
            return Ok(None);
        }
        let Some(invocation) = &origin.top_level_source_invocation else {
            return Ok(None);
        };
        let recipe = self.bound_recipe(&declaration.file)?;
        let canonical_recipe = aros_common::canonical_source_file(root, Path::new(&recipe))
            .map_err(|error| error.to_string())?;
        if invocation.path != root.join(&recipe) && invocation.path != canonical_recipe {
            return Err("template hook invocation is not bound to its source recipe".into());
        }
        let identity = (
            frame.template_path.clone(),
            frame.definition_line,
            caller.definition_line,
            wrapper.name.clone(),
            wrapper.definition_line,
        );
        if verified_definitions.insert(identity) {
            let relative = frame
                .template_path
                .strip_prefix(root)
                .map_err(|_| "template hook definition is outside source")?;
            if !self.snapshots.contains_key(
                relative
                    .to_str()
                    .ok_or("non-UTF8 template hook definition")?,
            ) {
                return Err("template hook definition is not sealed".into());
            }
            let template =
                std::fs::read_to_string(&frame.template_path).map_err(|error| error.to_string())?;
            verify_variant_template(&template, frame.definition_line)?;
            if caller.name == "build_module_core" {
                verify_reviewed_template_macro(
                    &template,
                    "build_module_core",
                    caller.definition_line,
                    BUILD_MODULE_CORE_SHA256,
                )?;
                let wrapper_digest = match wrapper.name.as_str() {
                    "build_module" => BUILD_MODULE_SHA256,
                    "build_module_abi" => BUILD_MODULE_ABI_SHA256,
                    "build_module_library" => BUILD_MODULE_LIBRARY_SHA256,
                    _ => unreachable!("wrapper name was filtered above"),
                };
                verify_reviewed_template_macro(
                    &template,
                    &wrapper.name,
                    wrapper.definition_line,
                    wrapper_digest,
                )?;
            } else if let Some(digest) = match caller.name.as_str() {
                "build_linklib" => Some(BUILD_LINKLIB_SHA256),
                "build_prog" => Some(BUILD_PROG_SHA256),
                "build_progs" => Some(BUILD_PROGS_SHA256),
                _ => None,
            } {
                verify_reviewed_template_macro(
                    &template,
                    &caller.name,
                    caller.definition_line,
                    digest,
                )?;
            } else {
                // The small direct-call chain exists only as an exact closed
                // fixture contract; name equality alone never admits it.
                verify_reviewed_template_macro(
                    &template,
                    "build_module",
                    caller.definition_line,
                    DIRECT_FIXTURE_BUILD_MODULE_SHA256,
                )?;
            }
        }
        Ok(Some(VerifiedTemplateHook {
            recipe,
            target: declaration.target.clone(),
            expression: architecture_expression(&edge.raw_expression)
                .ok_or("unsafe template selector expression")?,
            expanded_line: declaration.expanded_line,
            family: family.into(),
            source_invocation_line: invocation.line,
        }))
    }

    pub(super) fn bound_recipe(&self, file: &str) -> Result<String, String> {
        let recipe = self
            .owners
            .get(file)
            .ok_or_else(|| format!("virtual route has unbound source input {file}"))?;
        if !self.snapshots.contains_key(recipe) {
            return Err(format!(
                "virtual route source {recipe} lacks captured bytes"
            ));
        }
        Ok(recipe.clone())
    }
}

fn template_hook_verified(
    evidence: &NativeMetaSemanticsEvidence,
    declaration: &crate::metamake_owner_graph::TargetDeclarationProvenance,
    recipe: &str,
    expression: &str,
) -> bool {
    evidence.verified_template_hooks.iter().any(|proof| {
        proof.recipe == recipe
            && proof.target == declaration.target
            && proof.expression == expression
            && proof.expanded_line == declaration.expanded_line
    })
}

fn verify_variant_template(template: &str, definition_line: usize) -> Result<(), String> {
    verify_reviewed_template_macro(
        template,
        "gen_archspecificrules",
        definition_line,
        GEN_ARCHSPECIFICRULES_SHA256,
    )
}

/// Fingerprint an entire reviewed GenMF definition block, including its
/// declaration and terminator. Invocation provenance supplies the line id.
fn verify_reviewed_template_macro(
    template: &str,
    name: &str,
    definition_line: usize,
    expected_sha256: &str,
) -> Result<(), String> {
    let lines: Vec<_> = template.lines().collect();
    let starts: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let mut words = line.split_whitespace();
            words.next() == Some("%define") && words.next() == Some(name)
        })
        .map(|(line, _)| line)
        .collect();
    if starts.len() != 1 || starts[0] + 1 != definition_line {
        return Err(format!(
            "template macro {name} definition is not unique/exact"
        ));
    }
    let start = starts[0];
    let end = (start + 1..lines.len())
        .find(|index| lines[*index].starts_with("%end"))
        .ok_or_else(|| format!("template macro {name} is unterminated"))?;
    if aros_common::sha256_bytes(lines[start..=end].join("\n").as_bytes()).as_str()
        != expected_sha256
    {
        return Err(format!(
            "template macro {name} differs from its supported semantics"
        ));
    }
    Ok(())
}

fn charge(work: &mut usize) -> Result<(), String> {
    *work = work.checked_add(1).ok_or("native metadata work overflow")?;
    if *work > MAX_WORK {
        return Err("native metadata work limit exceeded".into());
    }
    Ok(())
}

fn require_concrete(name: &str, context: &TargetContext) -> Result<(), String> {
    let bound = native_endpoint(name, context).map_err(|e| e.to_string())?;
    if bound != name {
        return Err(format!(
            "source virtual endpoint is not concretely bound: {name}"
        ));
    }
    Ok(())
}

fn bound_meta_edges(
    graph: &DependencyGraph,
    context: &TargetContext,
    work: &mut usize,
) -> Result<BTreeSet<(String, String)>, String> {
    let mut edges = BTreeSet::new();
    for (name, dependencies) in &graph.meta_targets {
        charge(work)?;
        let Ok(name) = native_endpoint(name, context) else {
            continue;
        };
        for dependency in dependencies {
            charge(work)?;
            if let Ok(dependency) = native_endpoint(dependency, context) {
                edges.insert((name.clone(), dependency));
            }
        }
    }
    Ok(edges)
}

fn native_edge_spellings_proven(
    graph: &DependencyGraph,
    target: &str,
    dependency: &str,
    context: &TargetContext,
    expressions: &BTreeSet<String>,
    imported: bool,
) -> bool {
    graph
        .meta_targets
        .iter()
        .filter(|(name, _)| native_endpoint(name, context).ok().as_deref() == Some(target))
        .flat_map(|(_, dependencies)| dependencies)
        .filter(|name| native_endpoint(name, context).ok().as_deref() == Some(dependency))
        .all(|name| expressions.contains(name) || (imported && name == dependency))
}

/// A strict source selector expression, without identifier sanitization.
fn architecture_expression(raw: &str) -> Option<String> {
    let mut rest = raw;
    let mut out = String::new();
    let mut selectors = 0usize;
    while let Some(start) = rest.find("$(") {
        let literal = &rest[..start];
        if !safe_literal(literal) {
            return None;
        }
        out.push_str(literal);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let canonical = match &after[..end] {
            "ARCH" | "AROS_TARGET_ARCH" => "AROS_TARGET_PLATFORM",
            "CPU" | "AROS_TARGET_CPU" => "AROS_TARGET_CPU",
            "FAMILY" | "AROS_TARGET_FAMILY" => "AROS_TARGET_FAMILY",
            "AROS_TARGET_PLATFORM" => "AROS_TARGET_LEGACY_PLATFORM",
            "AROS_TARGET_VARIANT" => "AROS_TARGET_VARIANT",
            "AROS_TARGET_CPU32" => "AROS_TARGET_CPU32",
            _ => return None,
        };
        out.push_str("${");
        out.push_str(canonical);
        out.push('}');
        selectors += 1;
        rest = &after[end + 1..];
    }
    if !safe_literal(rest) || selectors == 0 {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

fn safe_literal(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::architecture_expression;

    #[test]
    fn selectors_are_exact_not_sanitized_or_inferred() {
        assert_eq!(
            architecture_expression("hook-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)").as_deref(),
            Some("hook-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}-${AROS_TARGET_VARIANT}")
        );
        for raw in [
            "literal-typo",
            "hook-$(UNKNOWN)",
            "hook-$(shell echo hi)",
            "hook-$(CPU)+unsafe",
            "hook-${AROS_TARGET_CPU}",
        ] {
            assert!(architecture_expression(raw).is_none(), "accepted {raw}");
        }
    }
}
