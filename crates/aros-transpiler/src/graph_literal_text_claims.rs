//! Portable selected file and directory ownership for literal text producers.

use super::source_headers::{output_key, selected_concrete_outputs};
use super::DependencyGraph;
use std::collections::{BTreeMap, BTreeSet};

/// Match the engine's absolute-or-build-relative output convention, without
/// evaluating unknown Make/CMake variables.
fn rooted_build_output(path: &str, relative_root: &str) -> String {
    if path.starts_with('/') || path.starts_with("${") || path.starts_with("$(") {
        path.to_owned()
    } else {
        format!("{}/{path}", relative_root.trim_end_matches('/'))
    }
}

impl DependencyGraph {
    /// Literal writers cannot collide with another selected typed file
    /// producer. Diagnostic-only, unowned projections are not admitted here.
    pub(crate) fn validate_literal_text_claims(
        &self,
        selected: &BTreeSet<String>,
        context: &crate::TargetContext,
    ) -> Result<(), String> {
        let has_selected_literal_text = self
            .source_literal_text_rules
            .iter()
            .any(|rule| selected.contains(&rule.owner));
        let has_selected_literal_objects = self
            .literal_object_groups
            .iter()
            .any(|group| selected.contains(&group.owner));
        if !has_selected_literal_text && !has_selected_literal_objects {
            return Ok(());
        }
        let additional = crate::literal_additional_outputs::collect(self, selected);
        let mut counts = BTreeMap::<String, usize>::new();
        for (_, output) in &additional.files {
            *counts
                .entry(output_key(&rooted_build_output(
                    output,
                    "${AROS_BUILD_DIR}",
                )))
                .or_default() += 1;
        }
        for output in selected_concrete_outputs(self, selected) {
            *counts.entry(output_key(&output)).or_default() += 1;
        }
        let mut literal_object_claims = BTreeMap::<String, usize>::new();
        for group in &self.literal_object_groups {
            if selected.contains(&group.owner) {
                for object in &group.objects {
                    for output in object.produced_outputs() {
                        *literal_object_claims
                            .entry(output_key(&output))
                            .or_default() += 1;
                    }
                }
            }
        }
        let mut add = |owner: &str, output: String| {
            if selected.contains(owner) {
                *counts.entry(output_key(&output)).or_default() += 1;
            }
        };
        // Header pipeline binding reconciles physical legacy mirrors itself.
        // Count them here, not in its shared pre-reconciliation claim list.
        for declaration in &self.header_transforms {
            add(&declaration.name, declaration.output.clone());
        }
        for declaration in &self.binary_objects {
            add(&declaration.name, declaration.output.clone());
        }
        for declaration in &self.assembly_headers {
            add(&declaration.owner, declaration.assembly_output.clone());
            add(&declaration.owner, declaration.header_output.clone());
        }
        for declaration in &self.flexcat_sources {
            add(
                &declaration.owner,
                format!(
                    "${{AROS_BUILD_DIR}}/gen/{}/{}",
                    declaration.declaring_dir, declaration.source
                ),
            );
            if let (Some(destination), Some(name)) =
                (&declaration.catalog_destination, &declaration.catalog_name)
            {
                let destination = rooted_build_output(destination, "${AROS_BUILD_DIR}");
                for language in &declaration.languages {
                    add(
                        &declaration.owner,
                        format!("{destination}/{language}/{name}.catalog"),
                    );
                }
            }
        }
        for declaration in &self.ilbm_sources {
            for pair in &declaration.pairs {
                add(
                    &declaration.owner,
                    format!(
                        "${{AROS_BUILD_DIR}}/gen/{}/{}",
                        declaration.declaring_dir, pair.output
                    ),
                );
            }
        }
        for declaration in &self.catalogs {
            if let Some(source) = &declaration.source {
                add(
                    &declaration.mmake,
                    rooted_build_output(
                        source,
                        &format!("${{AROS_BUILD_DIR}}/gen/{}", declaration.declaring_dir),
                    ),
                );
            }
            let destination = rooted_build_output(&declaration.dir, "${AROS_BUILD_DIR}");
            for language in &declaration.catalogs {
                add(
                    &declaration.mmake,
                    format!(
                        "{destination}/{language}/{}/{}.catalog",
                        declaration.subdir, declaration.name
                    ),
                );
            }
        }
        for declaration in &self.icons {
            if declaration.condition.as_ref().is_some_and(|condition| {
                crate::literal_icon_condition::condition_matches(condition, context) == Some(false)
            }) {
                continue;
            }
            // ICONSET is not a sealed CMake selector in TargetContext. Without
            // that fact the rule remains a possible writer, never guessed inactive.
            let destination = rooted_build_output(&declaration.dir, "${AROS_BUILD_DIR}");
            for icon in &declaration.icons {
                add(&declaration.mmake, format!("{destination}/{icon}.info"));
            }
        }
        // GENMODULE writefiles uses the private engine root
        // <binary>/genmodule/linklibs/<hash>, not <binary>/gen/<source-dir>.
        // It therefore cannot overlap the admitted direct literal outputs.
        if (has_selected_literal_text || has_selected_literal_objects)
            && counts
                .keys()
                .any(|output| output.split('/').any(|component| component == ".."))
        {
            // Traversal above an unresolved root can return to the build tree
            // after CMake expands its absolute path. Do not infer disjointness
            // from a symbolic spelling whose final location is not proved.
            return Err("selected literal text output ownership cannot be proved across unresolved output root traversal".into());
        }
        for (output, claims) in &literal_object_claims {
            if counts.get(output).copied().unwrap_or_default() != *claims {
                return Err(format!(
                    "selected literal object output {output} has conflicting concrete file producers"
                ));
            }
        }
        for rule in self
            .source_literal_text_rules
            .iter()
            .filter(|rule| selected.contains(&rule.owner))
        {
            if counts.get(&output_key(&rule.output)).copied() != Some(1) {
                return Err(format!(
                    "selected literal text output {} has conflicting concrete file producers",
                    rule.output
                ));
            }
            let output = output_key(&rule.output);
            if counts.keys().any(|other| {
                other.starts_with(&format!("{output}/")) || output.starts_with(&format!("{other}/"))
            }) {
                return Err(format!(
                    "selected literal text output {} has a file/directory conflict with another concrete producer",
                    rule.output
                ));
            }
            for (owner, root, populated) in additional
                .trees
                .iter()
                .map(|(owner, root)| (owner, root, true))
                .chain(
                    additional
                        .directories
                        .iter()
                        .map(|(owner, root)| (owner, root, false)),
                )
            {
                let root = output_key(&rooted_build_output(root, "${AROS_BUILD_DIR}"));
                if root.split('/').any(|component| component == "..") {
                    return Err(format!(
                        "selected literal text output ownership cannot be proved across unresolved output root traversal in {owner}"
                    ));
                }
                if output == root
                    || root.starts_with(&format!("{output}/"))
                    || populated && output.starts_with(&format!("{root}/"))
                {
                    return Err(format!(
                        "selected literal text output {} overlaps directory/tree producer {owner}",
                        rule.output
                    ));
                }
            }
            for copy in self
                .copy_directories
                .iter()
                .filter(|copy| selected.contains(&copy.name))
            {
                let destination = output_key(&copy.destination);
                if destination.split('/').any(|component| component == "..") {
                    return Err(format!(
                        "selected literal text output ownership cannot be proved across unresolved output root traversal in {}",
                        copy.name
                    ));
                }
                if output == destination
                    || output.starts_with(&format!("{destination}/"))
                    || destination.starts_with(&format!("{output}/"))
                {
                    return Err(format!(
                        "selected literal text output {} overlaps recursive directory producer {}",
                        rule.output, copy.name
                    ));
                }
            }
        }
        Ok(())
    }
}
