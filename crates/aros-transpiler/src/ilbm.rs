//! Deterministic modelling of hand-written ILBM-to-C include generators.
//!
//! Historic preference editors embed small images by including C files made by
//! the in-tree `ilbmtoc` host tool.  Their Make rules are pattern rules, but
//! their concrete products are enumerated by one ordinary MetaMake owner.  We
//! admit only that closed shape and materialise every concrete input/output
//! pair; a changed recipe remains an explicit compatibility failure.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_vars::VarScope;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IlbmSourcePair {
    /// Source path relative to the declaring mmakefile directory.
    pub input: String,
    /// Generated C include filename in that directory's private build root.
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IlbmSourceDecl {
    pub owner: String,
    pub declaring_dir: String,
    pub line: usize,
    pub pairs: Vec<IlbmSourcePair>,
}

#[derive(Debug, Default)]
pub struct IlbmSourceScan {
    pub declarations: Vec<IlbmSourceDecl>,
    pub skipped: Vec<String>,
}

#[derive(Debug)]
struct Rule<'a> {
    target: &'a str,
    prereqs: &'a str,
    line: usize,
    recipes: Vec<&'a str>,
}

#[must_use]
pub fn collect_ilbm_sources(
    content: &str,
    root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> IlbmSourceScan {
    let rules = rules(content);
    let declaring_dir = slash_path(rel_dir);
    let mut scan = IlbmSourceScan::default();

    for rule in rules.iter().filter(|rule| {
        // Literal header outputs are already a separate generated-header
        // capability (including the m68k-only planar flag). This scanner owns
        // only the pattern-rule form which instantiates embedded `.c` includes.
        rule.target.contains('%')
            && rule
                .recipes
                .iter()
                .any(|recipe| recipe.contains("$(ILBMTOC)"))
    }) {
        let failure = |detail: &str| {
            format!(
                "{}:{}: ILBM-to-C rule cannot be represented safely: {}",
                declaring_dir, rule.line, detail
            )
        };

        let [recipe] = rule.recipes.as_slice() else {
            scan.skipped
                .push(failure("expected exactly one recipe line"));
            continue;
        };
        if recipe.strip_prefix('@').unwrap_or(recipe).trim() != "$(ILBMTOC) $< >$@" {
            scan.skipped
                .push(failure("expected the exact `$(ILBMTOC) $< >$@` recipe"));
            continue;
        }

        let Some(output_pattern) = safe_pattern(rule.target.trim(), ".c", true) else {
            scan.skipped.push(failure(
                "target must be a safe relative C filename pattern with one `%`",
            ));
            continue;
        };
        let prereqs: Vec<_> = rule.prereqs.split_whitespace().collect();
        let [input_raw] = prereqs.as_slice() else {
            scan.skipped
                .push(failure("expected exactly one ILBM prerequisite pattern"));
            continue;
        };
        let Some(input_pattern) = safe_pattern(input_raw, ".ilbm", false) else {
            scan.skipped.push(failure(
                "prerequisite must be a safe relative ILBM filename pattern with one `%`",
            ));
            continue;
        };

        let eval_context = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
        let mut candidates = Vec::new();
        for owner_rule in &rules {
            let owner = owner_rule.target.trim();
            if !owner_rule.recipes.is_empty() || !simple_target_name(owner) {
                continue;
            }
            let Ok(prerequisites) = evaluate_make_list(owner_rule.prereqs, &eval_context) else {
                continue;
            };
            let outputs = prerequisites
                .into_iter()
                .filter(|word| pattern_stem(&output_pattern, word).is_some())
                .collect::<Vec<_>>();
            if !outputs.is_empty() {
                candidates.push((owner.to_owned(), outputs));
            }
        }
        if candidates.len() != 1 {
            scan.skipped.push(failure(&format!(
                "expected one concrete MetaMake owner, found {}",
                candidates.len()
            )));
            continue;
        }

        let Some((owner, outputs)) = candidates.pop() else {
            scan.skipped.push(failure(
                "the concrete MetaMake owner disappeared during evaluation",
            ));
            continue;
        };
        let mut pairs = Vec::new();
        let mut invalid = None;
        for output in outputs {
            let Some(stem) = pattern_stem(&output_pattern, &output) else {
                invalid = Some(format!(
                    "owner output no longer matches its ILBM pattern: {output}"
                ));
                break;
            };
            let input = input_pattern.replace('%', stem);
            if !safe_relative(&input) || !root.join(rel_dir).join(&input).is_file() {
                invalid = Some(format!("input for {output} is missing or unsafe: {input}"));
                break;
            }
            let output = output.strip_prefix("./").unwrap_or(&output).to_owned();
            if output.contains(['/', '\\']) || !safe_leaf(&output, ".c") {
                invalid = Some(format!(
                    "generated output is not a safe local C file: {output}"
                ));
                break;
            }
            pairs.push(IlbmSourcePair { input, output });
        }
        if let Some(detail) = invalid {
            scan.skipped.push(failure(&detail));
            continue;
        }
        pairs.sort_by(|left, right| left.output.cmp(&right.output));
        pairs.dedup_by(|left, right| left.output == right.output && left.input == right.input);
        if pairs.is_empty() {
            scan.skipped
                .push(failure("the owner expands to no concrete products"));
            continue;
        }
        scan.declarations.push(IlbmSourceDecl {
            owner,
            declaring_dir: declaring_dir.clone(),
            line: rule.line,
            pairs,
        });
    }

    scan
}

fn rules(content: &str) -> Vec<Rule<'_>> {
    let lines: Vec<_> = content.lines().collect();
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if line.chars().next().is_some_and(char::is_whitespace)
            || line.trim_start().starts_with('#')
        {
            index += 1;
            continue;
        }
        let Some((target, prereqs)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        if target.contains('=') || prereqs.starts_with('=') {
            index += 1;
            continue;
        }
        let mut recipes = Vec::new();
        let mut next = index + 1;
        while let Some(recipe) = lines.get(next) {
            if !recipe.starts_with('\t') {
                break;
            }
            recipes.push(recipe.trim());
            next += 1;
        }
        out.push(Rule {
            target: target.trim(),
            prereqs: prereqs.trim(),
            line: index + 1,
            recipes,
        });
        index = next;
    }
    out
}

fn safe_pattern(raw: &str, suffix: &str, allow_dot_prefix: bool) -> Option<String> {
    let pattern = if allow_dot_prefix {
        raw.strip_prefix("./").unwrap_or(raw)
    } else {
        raw
    };
    if pattern.matches('%').count() != 1
        || !pattern.ends_with(suffix)
        || pattern.contains(['$', ';', '\\'])
        || Path::new(pattern).is_absolute()
        || !safe_relative(pattern)
    {
        return None;
    }
    Some(pattern.to_owned())
}

fn pattern_stem<'a>(pattern: &str, value: &'a str) -> Option<&'a str> {
    let value = value.strip_prefix("./").unwrap_or(value);
    let (prefix, suffix) = pattern.split_once('%')?;
    let stem = value.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!stem.is_empty()
        && stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
    .then_some(stem)
}

fn safe_relative(raw: &str) -> bool {
    !raw.is_empty()
        && Path::new(raw)
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn safe_leaf(value: &str, suffix: &str) -> bool {
    value.ends_with(suffix)
        && value
            .trim_end_matches(suffix)
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn simple_target_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::collect_ilbm_sources;
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars;
    use crate::parser::join_continuations;
    use aros_common::read_source;
    use std::path::Path;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn scan(relative: &str) -> super::IlbmSourceScan {
        let root = root();
        let path = root.join(relative).join("mmakefile.src");
        let content = read_source(&path).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        collect_ilbm_sources(&content, &root, Path::new(relative), &scope, &dirs)
    }

    #[test]
    fn resolves_the_real_icontrol_image_matrix() {
        let scan = scan("workbench/prefs/icontrol");
        assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
        assert_eq!(scan.declarations.len(), 1);
        let declaration = &scan.declarations[0];
        assert_eq!(declaration.owner, "workbench-prefs-icontrol-images");
        assert_eq!(declaration.pairs.len(), 4);
        assert!(declaration.pairs.iter().any(|pair| {
            pair.input == "images/menupopup3d.ilbm" && pair.output == "menupopup3d_image.c"
        }));
    }

    #[test]
    fn resolves_the_real_locale_image_matrix() {
        let scan = scan("workbench/prefs/locale");
        assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
        assert_eq!(scan.declarations.len(), 1);
        let declaration = &scan.declarations[0];
        assert_eq!(declaration.owner, "workbench-prefs-locale-images");
        assert_eq!(declaration.pairs.len(), 2);
        assert_eq!(declaration.pairs[0].input, "pics/earthmap_small.ilbm");
        assert_eq!(declaration.pairs[0].output, "earthmap_small_image.c");
    }
}
