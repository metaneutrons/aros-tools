//! Closed scanner for local hand-written `GENMODULE writefiles` stamps.
//!
//! This models only the generic per-directory `.stubs-generated` pattern.
//! Anything that cannot be represented by that bounded rule is returned with
//! its likely owner so graph selection can fail closed when the owner is used.

use crate::genmodule_header_rules::{
    confined_source_file, evaluate_at_line, logical_lines, parse_rules, safe_basename, safe_echo,
    safe_relative_directory, safe_target_name, source_local_config_path,
    GenmoduleHeaderRuleRejection, GenmoduleType,
};
use crate::make_vars::ConditionalTruth;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STAMP: &str = "$(GENDIR)/$(CURDIR)/.stubs-generated";
const MODULE_DIRECTORY: &str = "$(GENDIR)/$(CURDIR)";

/// One safely represented local `GENMODULE writefiles` target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleWritefilesRuleDecl {
    /// The ordinary named Make target which depends on the local stamp.
    pub owner: String,
    /// Source-root-relative mmakefile path.
    pub file: String,
    /// One-based line of the stamp rule.
    pub line: usize,
    /// Source-root-relative directory represented by `$(CURDIR)`.
    pub declaring_dir: String,
    /// Source-root-relative `.conf` path.
    pub config: String,
    /// Literal `genmodule` module name.
    pub module: String,
    /// Literal `genmodule` module type.
    pub modtype: GenmoduleType,
}

/// Results from scanning one mmakefile for local `writefiles` stamps.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleWritefilesRuleScan {
    pub declarations: Vec<GenmoduleWritefilesRuleDecl>,
    pub rejected: Vec<GenmoduleHeaderRuleRejection>,
}

/// Scans a source mmakefile for the narrow local `.stubs-generated` contract.
///
/// The scanner deliberately emits a typed invocation only. The build engine
/// remains responsible for associating that invocation with the concrete
/// filenames declared by its existing `GenmoduleManifest`.
pub(crate) fn collect_genmodule_writefiles_rules_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> GenmoduleWritefilesRuleScan {
    let mut scan = GenmoduleWritefilesRuleScan::default();
    let directory = safe_relative_directory(rel_dir);
    let file = diagnostic_file_path(rel_dir, directory.as_ref().ok());
    let rules = parse_rules(logical_lines(content, line_states));

    // Preserve source order while grouping repeated definitions of one stamp.
    let mut stamp_groups: Vec<(String, Vec<&crate::genmodule_header_rules::Rule>)> = Vec::new();
    for rule in &rules {
        if rule.state == ConditionalTruth::False || !rule.target.trim().contains(".stubs-generated")
        {
            continue;
        }
        let target = rule.target.trim().to_owned();
        if let Some((_, definitions)) = stamp_groups.iter_mut().find(|(name, _)| *name == target) {
            definitions.push(rule);
        } else {
            stamp_groups.push((target, vec![rule]));
        }
    }

    for (stamp_target, definitions) in &stamp_groups {
        let owners: Vec<_> = rules
            .iter()
            .filter(|rule| {
                rule.state != ConditionalTruth::False
                    && rule
                        .prerequisites
                        .split_whitespace()
                        .any(|word| word == stamp_target)
            })
            .collect();
        let owner_names = distinct_owner_names(&owners);
        let line = definitions[0].line + 1;

        if owners.len() != 1 {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "stamp has {} possible owner rules; exactly one safe named owner is required",
                    owners.len()
                ),
            );
            continue;
        }

        let stamp = definitions[0];
        let owner = owners[0];
        let owner_name = owner.target.trim();

        let Some(destination_expression) = local_generated_directory(stamp_target) else {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "stamp target `{stamp_target}` is not a canonical local stamp with a literal safe subdirectory"
                ),
            );
            continue;
        };
        let has_subdirectory = destination_expression != MODULE_DIRECTORY;

        if definitions.len() != 1 {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                "stamp target has multiple rule definitions",
            );
            continue;
        }
        if stamp.state != ConditionalTruth::True
            || stamp.conditional_syntax
            || stamp
                .recipes
                .iter()
                .any(|recipe| recipe.state != ConditionalTruth::True || recipe.conditional_syntax)
            || owner.state != ConditionalTruth::True
            || owner.conditional_syntax
        {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                "owner, stamp, or recipe is affected by an unresolved Make conditional",
            );
            continue;
        }
        if !safe_target_name(owner_name) {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!("owner target `{owner_name}` is not one safe named target"),
            );
            continue;
        }
        if owner.prerequisites.trim() != stamp_target || !owner.recipes.is_empty() {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                "owner must depend only on the local stamp and have no recipe",
            );
            continue;
        }
        if owner.continued || stamp.continued {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                "continued owner or stamp rule is not represented",
            );
            continue;
        }

        let Some(recipe) = parse_writefiles_recipe(&stamp.recipes) else {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "stamp recipe has {} commands or a command outside the closed ECHO/GENMODULE/TOUCH form",
                    stamp.recipes.len()
                ),
            );
            continue;
        };

        let declaring_dir = match directory.as_ref() {
            Ok(value) => value,
            Err(reason) => {
                reject_for_owners(&mut scan, &owner_names, &file, line, reason.clone());
                continue;
            }
        };

        let resolved_config = match evaluate_at_line(
            &recipe.config,
            recipe.source_line,
            scope,
            dirs,
            source_root,
            rel_dir,
        ) {
            Ok(value) => value,
            Err(reason) => {
                reject_for_owners(&mut scan, &owner_names, &file, line, reason);
                continue;
            }
        };
        let config = match source_local_config_path(&resolved_config, declaring_dir) {
            Ok(value) => value,
            Err(reason) => {
                reject_for_owners(&mut scan, &owner_names, &file, line, reason);
                continue;
            }
        };
        if let Err(reason) = confined_source_file(source_root, &config) {
            reject_for_owners(&mut scan, &owner_names, &file, line, reason);
            continue;
        }

        let expected_directory = match evaluate_at_line(
            &destination_expression,
            recipe.source_line,
            scope,
            dirs,
            source_root,
            rel_dir,
        ) {
            Ok(value) => value,
            Err(reason) => {
                reject_for_owners(&mut scan, &owner_names, &file, line, reason);
                continue;
            }
        };
        let destination = match evaluate_at_line(
            &recipe.destination,
            recipe.source_line,
            scope,
            dirs,
            source_root,
            rel_dir,
        ) {
            Ok(value) => value,
            Err(reason) => {
                reject_for_owners(&mut scan, &owner_names, &file, line, reason);
                continue;
            }
        };
        if recipe.destination != destination_expression || destination != expected_directory {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "GENMODULE destination `{}` is not the canonical local module directory",
                    recipe.destination
                ),
            );
            continue;
        }

        if !safe_basename(&recipe.module, "") {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "GENMODULE module `{}` is not a safe basename",
                    recipe.module
                ),
            );
            continue;
        }
        let Some(modtype) = GenmoduleType::parse(&recipe.modtype) else {
            reject_for_owners(
                &mut scan,
                &owner_names,
                &file,
                line,
                format!(
                    "GENMODULE module type `{}` is not supported by the reference tool",
                    recipe.modtype
                ),
            );
            continue;
        };

        if let Err(reason) = validate_stamp_prerequisites(
            stamp,
            &config,
            declaring_dir,
            &destination_expression,
            has_subdirectory,
            StampPrerequisiteContext {
                scope,
                dirs,
                source_root,
                rel_dir,
            },
        ) {
            reject_for_owners(&mut scan, &owner_names, &file, line, reason);
            continue;
        }

        scan.declarations.push(GenmoduleWritefilesRuleDecl {
            owner: owner_name.to_owned(),
            file: file.clone(),
            line,
            declaring_dir: declaring_dir.clone(),
            config,
            module: recipe.module,
            modtype,
        });
    }

    // Report a dangling owner even when its stamp producer was removed or
    // renamed. False conditional branches are absent from this check.
    for owner in rules.iter().filter(|rule| {
        rule.state != ConditionalTruth::False
            && rule
                .prerequisites
                .split_whitespace()
                .any(|word| word.contains(".stubs-generated"))
    }) {
        let has_local_producer = owner.prerequisites.split_whitespace().any(|prerequisite| {
            rules.iter().any(|candidate| {
                candidate.state != ConditionalTruth::False
                    && candidate.target.trim() == prerequisite
            })
        });
        if !has_local_producer {
            scan.rejected.push(GenmoduleHeaderRuleRejection {
                owner: owner.target.trim().to_owned(),
                file: file.clone(),
                line: owner.line + 1,
                reason: "owner depends on a `.stubs-generated` target with no local producer"
                    .into(),
            });
        }
    }

    scan
}

#[derive(Debug)]
struct WritefilesRecipe {
    config: String,
    destination: String,
    module: String,
    modtype: String,
    source_line: usize,
}

fn parse_writefiles_recipe(
    recipes: &[crate::genmodule_header_rules::RecipeLine],
) -> Option<WritefilesRecipe> {
    if recipes.len() != 3
        || recipes.iter().any(|recipe| recipe.continued)
        || !safe_echo(&recipes[0].text)
        || recipes[2].text.trim() != "@$(TOUCH) $@"
    {
        return None;
    }

    let words: Vec<_> = recipes[1].text.split_whitespace().collect();
    if words.len() != 8
        || words[0] != "@$(GENMODULE)"
        || words[1] != "-c"
        || words[3] != "-d"
        || words[5] != "writefiles"
    {
        return None;
    }

    Some(WritefilesRecipe {
        config: words[2].to_owned(),
        destination: words[4].to_owned(),
        module: words[6].to_owned(),
        modtype: words[7].to_owned(),
        source_line: recipes[1].source_line,
    })
}

#[derive(Clone, Copy)]
struct StampPrerequisiteContext<'a> {
    scope: &'a crate::make_vars::VarScope,
    dirs: &'a crate::dirs::DirVars,
    source_root: &'a Path,
    rel_dir: &'a Path,
}

fn validate_stamp_prerequisites(
    stamp: &crate::genmodule_header_rules::Rule,
    recipe_config: &str,
    declaring_dir: &str,
    destination_expression: &str,
    has_subdirectory: bool,
    context: StampPrerequisiteContext<'_>,
) -> Result<(), String> {
    let StampPrerequisiteContext {
        scope,
        dirs,
        source_root,
        rel_dir,
    } = context;
    let raw = stamp.prerequisites.trim();
    if raw.is_empty() {
        if has_subdirectory {
            return Err(
                "subdirectory stamp requires its exact output directory as an order-only prerequisite".into(),
            );
        }
        return Ok(());
    }

    let mut sections = raw.split('|');
    let normal_raw = sections.next().unwrap_or_default().trim();
    let order_only_raw = sections.next().map(str::trim);
    if sections.next().is_some() {
        return Err("stamp prerequisite list has multiple order-only separators".into());
    }
    if order_only_raw.is_some_and(str::is_empty) {
        return Err("stamp has an empty order-only prerequisite list".into());
    }

    let normal: Vec<_> = normal_raw.split_whitespace().collect();
    if normal.len() > 2
        || normal
            .iter()
            .filter(|prerequisite| **prerequisite == "$(GENMODULE)")
            .count()
            != usize::from(!normal.is_empty())
    {
        return Err(
            "normal stamp prerequisites must be empty or contain only `$(GENMODULE)` and the exact source config".into(),
        );
    }
    for prerequisite in normal
        .iter()
        .copied()
        .filter(|prerequisite| *prerequisite != "$(GENMODULE)")
    {
        let resolved =
            evaluate_at_line(prerequisite, stamp.line, scope, dirs, source_root, rel_dir)?;
        let relative = source_local_config_path(&resolved, declaring_dir)?;
        if relative != recipe_config {
            return Err(format!(
                "source config prerequisite `{prerequisite}` does not resolve to recipe config `{recipe_config}`"
            ));
        }
    }

    let order_only: Vec<_> = order_only_raw
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if has_subdirectory {
        if order_only.as_slice() != [destination_expression] {
            return Err(format!(
                "subdirectory stamp requires exactly its output directory `{destination_expression}` as an order-only prerequisite"
            ));
        }
        let expected = evaluate_at_line(
            destination_expression,
            stamp.line,
            scope,
            dirs,
            source_root,
            rel_dir,
        )?;
        let resolved =
            evaluate_at_line(order_only[0], stamp.line, scope, dirs, source_root, rel_dir)?;
        if resolved != expected {
            return Err(format!(
                "order-only output directory `{}` does not resolve to the local writefiles destination",
                order_only[0]
            ));
        }
    } else if !order_only.is_empty() {
        return Err("root-level stamp may not have order-only prerequisites".into());
    }
    Ok(())
}

fn local_generated_directory(stamp_target: &str) -> Option<String> {
    if stamp_target == STAMP {
        return Some(MODULE_DIRECTORY.to_owned());
    }

    let prefix = format!("{MODULE_DIRECTORY}/");
    let subdirectory = stamp_target
        .strip_prefix(&prefix)?
        .strip_suffix("/.stubs-generated")?;
    if subdirectory.is_empty()
        || safe_relative_directory(Path::new(subdirectory)).ok()? != subdirectory
    {
        return None;
    }
    Some(format!("{MODULE_DIRECTORY}/{subdirectory}"))
}

fn distinct_owner_names(owners: &[&crate::genmodule_header_rules::Rule]) -> Vec<String> {
    let mut names = Vec::new();
    for owner in owners {
        let name = owner.target.trim();
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    if names.is_empty() {
        names.push("<unknown-owner>".into());
    }
    names
}

fn reject_for_owners(
    scan: &mut GenmoduleWritefilesRuleScan,
    owners: &[String],
    file: &str,
    line: usize,
    reason: impl Into<String>,
) {
    let reason = reason.into();
    for owner in owners {
        scan.rejected.push(GenmoduleHeaderRuleRejection {
            owner: owner.clone(),
            file: file.to_owned(),
            line,
            reason: reason.clone(),
        });
    }
}

fn diagnostic_file_path(rel_dir: &Path, safe_dir: Option<&String>) -> String {
    let directory = safe_dir.map_or_else(
        || rel_dir.to_string_lossy().replace('\\', "/"),
        Clone::clone,
    );
    if directory.is_empty() {
        "mmakefile.src".into()
    } else {
        PathBuf::from(directory)
            .join("mmakefile.src")
            .to_string_lossy()
            .replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use crate::testing::TempTree;
    use std::fs;

    const RECIPE: &str = concat!(
        "generic-stubs-owner: $(GENDIR)/$(CURDIR)/.stubs-generated\n",
        "$(GENDIR)/$(CURDIR)/.stubs-generated :\n",
        "\t@$(ECHO) \"Generating generic API stubs...\"\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(GENDIR)/$(CURDIR) writefiles generic_module resource\n",
        "\t@$(TOUCH) $@\n",
    );
    const SUBDIR_RECIPE: &str = concat!(
        "posixc-lfa-gen-stubs: $(GENDIR)/$(CURDIR)/lfa/.stubs-generated\n",
        "$(GENDIR)/$(CURDIR)/lfa/.stubs-generated : | $(GENDIR)/$(CURDIR)/lfa\n",
        "\t@$(ECHO) \"Generating Large File Access stubs...\"\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/posixc_lfa.conf -d $(GENDIR)/$(CURDIR)/lfa writefiles posixc library\n",
        "\t@$(TOUCH) $@\n",
    );

    fn source_tree() -> TempTree {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("unit")).unwrap();
        fs::create_dir_all(tree.0.join("config")).unwrap();
        fs::write(tree.0.join("config/make.cfg.in"), "").unwrap();
        fs::write(tree.0.join("unit/module.conf"), "module config\n").unwrap();
        fs::write(tree.0.join("unit/posixc_lfa.conf"), "module config\n").unwrap();
        tree
    }

    fn scan(
        content: &str,
        tree: &TempTree,
        states: Option<&[ConditionalTruth]>,
    ) -> GenmoduleWritefilesRuleScan {
        let (scope, _) = collect_vars_impl(content, None);
        let dirs = crate::dirs::DirVars::load(&tree.0);
        collect_genmodule_writefiles_rules_with_context(
            content,
            &tree.0,
            Path::new("unit"),
            &scope,
            &dirs,
            states,
        )
    }

    #[test]
    fn generic_gl_shaped_rule_is_typed_without_declaring_filenames() {
        let tree = source_tree();
        let result = scan(RECIPE, &tree, None);
        assert!(result.rejected.is_empty(), "{:#?}", result.rejected);
        assert_eq!(result.declarations.len(), 1);
        assert_eq!(result.declarations[0].owner, "generic-stubs-owner");
        assert_eq!(result.declarations[0].file, "unit/mmakefile.src");
        assert_eq!(result.declarations[0].line, 2);
        assert_eq!(result.declarations[0].declaring_dir, "unit");
        assert_eq!(result.declarations[0].config, "unit/module.conf");
        assert_eq!(result.declarations[0].module, "generic_module");
        assert_eq!(result.declarations[0].modtype, GenmoduleType::Resource);

        let with_config_prerequisite = RECIPE.replace(
            ".stubs-generated :\n",
            ".stubs-generated : $(GENMODULE) $(SRCDIR)/$(CURDIR)/module.conf\n",
        );
        let result = scan(&with_config_prerequisite, &tree, None);
        assert!(result.rejected.is_empty(), "{:#?}", result.rejected);
        assert_eq!(result.declarations.len(), 1);
    }

    #[test]
    fn posixc_lfa_subdirectory_stamp_requires_its_matching_directory_endpoint() {
        let tree = source_tree();
        let result = scan(SUBDIR_RECIPE, &tree, None);
        assert!(result.rejected.is_empty(), "{:#?}", result.rejected);
        assert_eq!(result.declarations.len(), 1);
        assert_eq!(result.declarations[0].owner, "posixc-lfa-gen-stubs");
        assert_eq!(result.declarations[0].config, "unit/posixc_lfa.conf");
        assert_eq!(result.declarations[0].module, "posixc");
        assert_eq!(result.declarations[0].modtype, GenmoduleType::Library);

        let unsafe_stamp = SUBDIR_RECIPE.replace("/lfa/", "/../escape/");
        let result = scan(&unsafe_stamp, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0]
            .reason
            .contains("literal safe subdirectory"));

        let wrong_destination = SUBDIR_RECIPE.replace(
            "-d $(GENDIR)/$(CURDIR)/lfa writefiles",
            "-d $(GENDIR)/$(CURDIR)/other writefiles",
        );
        let result = scan(&wrong_destination, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0]
            .reason
            .contains("canonical local module directory"));

        let wrong_order_only = SUBDIR_RECIPE.replace(
            ": | $(GENDIR)/$(CURDIR)/lfa\n",
            ": | $(GENDIR)/$(CURDIR)/other\n",
        );
        let result = scan(&wrong_order_only, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0].reason.contains("output directory"));

        let missing_order_only = SUBDIR_RECIPE.replace(" : | $(GENDIR)/$(CURDIR)/lfa\n", " :\n");
        let result = scan(&missing_order_only, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0]
            .reason
            .contains("order-only prerequisite"));
    }

    #[test]
    fn unknown_candidate_is_owner_specific_and_false_branch_is_ignored() {
        let tree = source_tree();
        let mut states = vec![ConditionalTruth::True; RECIPE.lines().count()];
        let recipe_line = RECIPE
            .lines()
            .position(|line| line.contains("GENMODULE"))
            .unwrap();
        states[recipe_line] = ConditionalTruth::Unknown;
        let result = scan(RECIPE, &tree, Some(&states));
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].owner, "generic-stubs-owner");
        assert!(result.rejected[0].reason.contains("conditional"));

        let states = vec![ConditionalTruth::False; RECIPE.lines().count()];
        let result = scan(RECIPE, &tree, Some(&states));
        assert!(result.declarations.is_empty());
        assert!(result.rejected.is_empty());
    }

    #[test]
    fn missing_repeated_and_multi_owner_stamps_are_rejected() {
        let tree = source_tree();
        let missing = "generic-stubs-owner: $(GENDIR)/$(CURDIR)/.stubs-generated\n";
        let result = scan(missing, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].owner, "generic-stubs-owner");
        assert!(result.rejected[0].reason.contains("no local producer"));

        let repeated = RECIPE.replace(
            "\t@$(TOUCH) $@\n",
            "\t@$(TOUCH) $@\n$(GENDIR)/$(CURDIR)/.stubs-generated :\n\t@$(ECHO) \"Generating generic API stubs...\"\n\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(GENDIR)/$(CURDIR) writefiles generic_module resource\n\t@$(TOUCH) $@\n",
        );
        let result = scan(&repeated, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert!(result.rejected[0]
            .reason
            .contains("multiple rule definitions"));

        let multiple_owners =
            format!("{RECIPE}other-stubs-owner: $(GENDIR)/$(CURDIR)/.stubs-generated\n");
        let result = scan(&multiple_owners, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 2);
        assert!(result
            .rejected
            .iter()
            .any(|rejection| rejection.owner == "generic-stubs-owner"));
        assert!(result
            .rejected
            .iter()
            .any(|rejection| rejection.owner == "other-stubs-owner"));
    }

    #[test]
    fn shell_traversal_wrong_destination_unsafe_module_and_prerequisite_fail_closed() {
        let tree = source_tree();
        let cases = [
            (
                RECIPE.replace(
                    "writefiles generic_module resource",
                    "writefiles generic_module resource; touch /tmp/escape",
                ),
                "recipe",
            ),
            (
                RECIPE.replace(
                    "$(SRCDIR)/$(CURDIR)/module.conf",
                    "$(SRCDIR)/$(CURDIR)/../outside.conf",
                ),
                "safe source-relative",
            ),
            (
                RECIPE.replace(
                    "-d $(GENDIR)/$(CURDIR) writefiles",
                    "-d $(GENDIR)/$(CURDIR)/../escape writefiles",
                ),
                "destination",
            ),
            (
                RECIPE.replace("generic_module resource", "../escape resource"),
                "safe basename",
            ),
            (
                RECIPE.replace(
                    ".stubs-generated :\n",
                    ".stubs-generated : $(GENMODULE) /tmp/injected\n",
                ),
                "does not resolve under AROS_SOURCE_DIR",
            ),
        ];
        for (content, expected_reason) in cases {
            let result = scan(&content, &tree, None);
            assert!(result.declarations.is_empty(), "{:#?}", result.declarations);
            assert_eq!(result.rejected.len(), 1, "{:#?}", result.rejected);
            assert_eq!(result.rejected[0].owner, "generic-stubs-owner");
            assert!(
                result.rejected[0].reason.contains(expected_reason),
                "{:#?}",
                result.rejected
            );
        }
    }

    #[test]
    fn symlink_config_is_rejected_as_non_regular_confined_source() {
        let tree = source_tree();
        fs::write(tree.0.join("outside.conf"), "outside\n").unwrap();
        std::os::unix::fs::symlink(tree.0.join("outside.conf"), tree.0.join("unit/link.conf"))
            .unwrap();
        let content = RECIPE.replace(
            "$(SRCDIR)/$(CURDIR)/module.conf",
            "$(SRCDIR)/$(CURDIR)/link.conf",
        );
        let result = scan(&content, &tree, None);
        assert!(result.declarations.is_empty());
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.rejected[0].owner, "generic-stubs-owner");
        assert!(result.rejected[0].reason.contains("regular non-symlink"));
    }
}
