//! Exact empty-list normalization, not classic missing-target tolerance.
//!
//! GenMF preserves a substitution's prefix when its list is empty. The
//! reviewed module template consequently emits `linklibs-` even with no
//! requested libraries. Only those exact source-backed claims may disappear.

use super::{
    charge, native_edge_spellings_proven, verify_reviewed_template_macro,
    NativeMetaSemanticsSelection, NativeOwnerProjection, BUILD_MODULE_ABI_SHA256,
    BUILD_MODULE_CORE_SHA256, BUILD_MODULE_LIBRARY_SHA256, BUILD_MODULE_SHA256,
};
use crate::graph::native_endpoint;
use crate::metamake_owner_graph::TargetDeclarationProvenance;
use crate::DependencyGraph;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct VerifiedEmptyLibraryList {
    pub recipe: String,
    pub target: String,
    pub dependency: String,
    pub expanded_line: usize,
    pub source_invocation_line: usize,
}

impl NativeOwnerProjection {
    pub(super) fn bind_empty_library_lists(
        &self,
        root: &Path,
        graph: &mut DependencyGraph,
        selection: NativeMetaSemanticsSelection<'_>,
    ) -> Result<Vec<VerifiedEmptyLibraryList>, String> {
        let mut work = 0;
        let reachable = graph
            .audit_native_dependency_graph(
                selection.roots,
                selection.context,
                selection.diagnostics,
            )
            .reachable;
        let known = graph.native_metadata_endpoint_names(selection.context);
        let rejected = selection.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .context
                .as_ref()
                .and_then(|context| context.target.as_ref())
                .and_then(|target| native_endpoint(target, selection.context).ok())
                .as_deref()
                == Some("linklibs-")
        });
        if self.graph.known_targets().contains("linklibs-")
            || known.contains("linklibs-")
            || rejected
        {
            return Ok(Vec::new());
        }
        let mut claims = BTreeMap::<String, Vec<&TargetDeclarationProvenance>>::new();
        for declaration in self.graph.declarations() {
            charge(&mut work)?;
            if reachable.contains(&declaration.target)
                && declaration
                    .dependencies
                    .iter()
                    .any(|edge| edge.concrete == "linklibs-")
            {
                claims
                    .entry(declaration.target.clone())
                    .or_default()
                    .push(declaration);
            }
        }
        let mut verified_definitions = BTreeSet::new();
        let mut accepted = Vec::new();
        for (target, declarations) in claims {
            let mut proofs = Vec::new();
            let mut recipes = BTreeSet::new();
            let mut proven = true;
            for declaration in declarations {
                charge(&mut work)?;
                let Some(proof) =
                    self.empty_library_list_proof(root, declaration, &mut verified_definitions)?
                else {
                    proven = false;
                    break;
                };
                recipes.insert(proof.recipe.clone());
                proofs.push(proof);
            }
            let mut origins = BTreeSet::new();
            for ((parent, child), files) in selection.meta_edge_origins {
                charge(&mut work)?;
                if native_endpoint(parent, selection.context).ok().as_deref()
                    == Some(target.as_str())
                    && native_endpoint(child, selection.context).ok().as_deref()
                        == Some("linklibs-")
                {
                    origins.extend(files.iter().cloned());
                }
            }
            if proven
                && !proofs.is_empty()
                && !origins.is_empty()
                && origins.is_subset(&recipes)
                && native_edge_spellings_proven(
                    graph,
                    &target,
                    "linklibs-",
                    selection.context,
                    &BTreeSet::from(["linklibs-".into()]),
                    false,
                )
                && graph.meta_targets.iter().any(|(parent, children)| {
                    native_endpoint(parent, selection.context).ok().as_deref()
                        == Some(target.as_str())
                        && children.contains("linklibs-")
                })
            {
                accepted.extend(proofs);
            }
        }
        // Recheck sealed source before removing only the individually proved
        // edges. An independent handwritten ingress remains in the graph.
        self.verify(root)?;
        for proof in &accepted {
            graph.remove_native_meta_edge(&proof.target, &proof.dependency, selection.context);
        }
        Ok(accepted)
    }

    fn empty_library_list_proof(
        &self,
        root: &Path,
        declaration: &TargetDeclarationProvenance,
        verified: &mut BTreeSet<(std::path::PathBuf, usize, String, usize)>,
    ) -> Result<Option<VerifiedEmptyLibraryList>, String> {
        // Module and KOBJ owners retain their actual producers. This proof
        // concerns only the list-valued prerequisite, not owner qualification.
        if declaration.bare_marker
            || declaration
                .dependencies
                .iter()
                .any(|edge| edge.concrete == "linklibs-" && edge.raw_expression != "linklibs-")
        {
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
                    .is_none_or(|value| !value.chars().all(char::is_whitespace))
            })
        {
            return Ok(None);
        }
        let Some(main) = core.resolved_arguments.get("mmake") else {
            return Ok(None);
        };
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
        {
            return Err(
                "empty library-list expansion is not bound to its source recipe/template".into(),
            );
        }
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        let relative = core
            .template_path
            .strip_prefix(&canonical_root)
            .map_err(|_| "library-list template outside source")?;
        if !self
            .snapshots
            .contains_key(relative.to_str().ok_or("non-UTF8 library-list template")?)
        {
            return Err("empty library-list template is not sealed".into());
        }
        let template =
            std::fs::read_to_string(&core.template_path).map_err(|error| error.to_string())?;
        let Some(raw) = template.lines().nth(body_line.saturating_sub(1)) else {
            return Ok(None);
        };
        let expected_target = match raw {
            "#M%(build_abi)- %(mmake) : %(mmake)-includes core-linklibs linklibs-%(uselibs)" => main.clone(),
            "#%(build_library)M %(mmake)-kobj : core-linklibs linklibs-%(uselibs)"
            | "#%(build_library)%(build_abi) %(mmake)-kobj : %(mmake)-includes core-linklibs linklibs-%(uselibs)" => format!("{main}-kobj"),
            _ => return Ok(None),
        };
        if declaration.raw_target != expected_target || declaration.target != expected_target {
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
        Ok(Some(VerifiedEmptyLibraryList {
            recipe,
            target: declaration.target.clone(),
            dependency: "linklibs-".into(),
            expanded_line: declaration.expanded_line,
            source_invocation_line: invocation.line,
        }))
    }
}
