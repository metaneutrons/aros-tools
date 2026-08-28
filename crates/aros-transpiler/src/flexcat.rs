//! Safe modelling of paired hand-written FlexCat source rules.
//!
//! A few historic MUI preference modules predate `%build_catalogs`.  They use
//! a small, regular Make recipe instead:
//!
//! ```make
//! locale.h: locale.c
//! locale.c: locale/Module.pot C_h.sd C_c.sd
//!     $(FLEXCAT) locale/Module.pot locale.h=C_h.sd locale.c=C_c.sd
//! ```
//!
//! The recipe line begins with a tab in the file, as Make requires; it is
//! spelled with spaces above so the doc comment stays tab-free.
//!
//! The generated `locale.c` is a real translation unit and `locale.h` exposes
//! `OpenCat`, `CloseCat`, `tr`, and the `MSG_*` constants used by the module.
//! Treating the rule as arbitrary Make silently drops both products and leaves
//! the compiler to report a misleading cascade of undeclared symbols.  This
//! module accepts only that bounded, literal rule shape; other FlexCat recipes
//! stay outside the executable graph rather than being guessed at.

use crate::dirs::DirVars;
use crate::make_vars::VarScope;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// One paired source/header generation rule owned by a normal MetaMake target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlexCatSourceDecl {
    /// Real MetaMake target that owns the generated source, header and
    /// optional translated catalog resources.
    pub owner: String,
    /// Source-root-relative directory containing the hand-written rule.
    pub declaring_dir: String,
    /// One-based line of the `locale.c:` rule.
    pub line: usize,
    /// Generated C source name, relative to `declaring_dir`.
    pub source: String,
    /// Generated C header name, relative to `declaring_dir`.
    pub header: String,
    /// FlexCat catalog description, as `${CMAKE_SOURCE_DIR}/<relative>`.
    pub description: String,
    /// Header source-description template, in the same form.
    pub header_template: String,
    /// C source-description template, in the same form.
    pub source_template: String,
    /// Base build-tree directory for translated `.catalog` resources.  Empty
    /// only if the rule did not have the canonical PO catalog companion.
    pub catalog_destination: Option<String>,
    /// Catalog basename without the `.catalog` suffix.
    pub catalog_name: Option<String>,
    /// Source-directory-relative location of the PO files for the optional
    /// catalog companion rule.
    pub catalog_source_dir: Option<String>,
    /// PO languages discovered from the rule's source directory.
    pub languages: Vec<String>,
    /// Concrete compilation targets which contain `source` in their source
    /// list. Filled once the whole graph is available.
    #[serde(default)]
    pub consumers: Vec<String>,
}

/// One hand-written FlexCat rule which produces only a generated header.
///
/// Unlike [`FlexCatSourceDecl`], its owner is reached through an ordinary #MM
/// dependency, so the generated include directory can be propagated while
/// that edge is attached; no source-list substitution is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlexCatHeaderDecl {
    pub owner: String,
    pub declaring_dir: String,
    pub line: usize,
    pub header: String,
    pub description: String,
    pub header_template: String,
}

/// Result of scanning one mmakefile for the narrow paired FlexCat capability.
#[derive(Debug, Default)]
pub struct FlexCatSourceScan {
    pub declarations: Vec<FlexCatSourceDecl>,
    pub headers: Vec<FlexCatHeaderDecl>,
    pub skipped: Vec<String>,
}

#[derive(Debug)]
struct Rule<'a> {
    target: &'a str,
    prereqs: &'a str,
    line: usize,
    recipes: Vec<&'a str>,
}

/// Collects safely recognised paired FlexCat rules from one mmakefile.
///
/// The accepted paths are explicitly source-tree relative through
/// `$(SRCDIR)/$(CURDIR)/`; that is the form the four in-tree MUI rules use.
/// It keeps output inputs independent of the host working directory and makes
/// a later CMake containment check meaningful.
#[must_use]
pub fn collect_flexcat_source_rules(
    content: &str,
    root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> FlexCatSourceScan {
    let rules = rules(content);
    let declaring_dir = slash_path(rel_dir);
    let mut scan = FlexCatSourceScan::default();

    for rule in &rules {
        let header = rule.target.trim();
        if !simple_file_name(header, ".h") {
            continue;
        }
        let Some((description_raw, outputs)) = flexcat_recipe(rule) else {
            continue;
        };
        let [(output, template_raw)] = outputs.as_slice() else {
            continue;
        };
        if output != header {
            continue;
        }

        let owners: Vec<_> = rules
            .iter()
            .filter(|candidate| {
                simple_target_name(candidate.target.trim())
                    && words(candidate.prereqs).contains(&header.to_owned())
            })
            .map(|candidate| candidate.target.trim().to_owned())
            .collect();
        if owners.len() != 1 {
            scan.skipped.push(format!(
                "{}:{}: FlexCat header {} has {} possible MetaMake owners",
                declaring_dir,
                rule.line,
                header,
                owners.len()
            ));
            continue;
        }

        let Some(description_path) = source_path(&description_raw, root, rel_dir) else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat header {} has no safe source-tree description path",
                declaring_dir, rule.line, header
            ));
            continue;
        };
        let Some(template_path) = source_path(template_raw, root, rel_dir) else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat header {} has no safe source-description template",
                declaring_dir, rule.line, header
            ));
            continue;
        };
        let Some(description) = cmake_source_path(&description_path, root) else {
            continue;
        };
        let Some(header_template) = cmake_source_path(&template_path, root) else {
            continue;
        };

        scan.headers.push(FlexCatHeaderDecl {
            owner: owners[0].clone(),
            declaring_dir: declaring_dir.clone(),
            line: rule.line,
            header: header.to_owned(),
            description,
            header_template,
        });
    }

    for rule in &rules {
        let source = rule.target.trim();
        if !simple_file_name(source, ".c") {
            continue;
        }
        let Some((description_raw, outputs)) = flexcat_recipe(rule) else {
            continue;
        };
        if outputs.len() != 2 {
            continue;
        }

        let mut header = None;
        let mut header_template = None;
        let mut source_template = None;
        for (output, template) in outputs {
            if output == source {
                source_template = source_path(&template, root, rel_dir);
            } else if simple_file_name(&output, ".h") {
                if header.is_some() {
                    header = None;
                    break;
                }
                header = Some(output);
                header_template = source_path(&template, root, rel_dir);
            } else {
                header = None;
                break;
            }
        }

        let Some(header) = header else { continue };
        let Some(description) = source_path(&description_raw, root, rel_dir) else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has no safe source-tree description path",
                declaring_dir, rule.line, source
            ));
            continue;
        };
        let Some(header_template) = header_template else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has no safe header template",
                declaring_dir, rule.line, source
            ));
            continue;
        };
        let Some(source_template) = source_template else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has no safe C source template",
                declaring_dir, rule.line, source
            ));
            continue;
        };
        if !description.exists() || !header_template.exists() || !source_template.exists() {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has a missing description/template input",
                declaring_dir, rule.line, source
            ));
            continue;
        }
        if !rules.iter().any(|candidate| {
            candidate.target.trim() == header && words(candidate.prereqs).as_slice() == [source]
        }) {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} lacks the paired {}: {} header rule",
                declaring_dir, rule.line, source, header, source
            ));
            continue;
        }

        let owners: Vec<_> = rules
            .iter()
            .filter(|candidate| {
                simple_target_name(candidate.target.trim())
                    && words(candidate.prereqs).contains(&source.to_owned())
                    && words(candidate.prereqs).contains(&header)
            })
            .map(|candidate| candidate.target.trim().to_owned())
            .collect();
        if owners.len() != 1 {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has {} possible MetaMake owners",
                declaring_dir,
                rule.line,
                source,
                owners.len()
            ));
            continue;
        }

        let catalog = catalog_outputs(&rules, root, rel_dir, scope, dirs);

        let rendered = cmake_source_path(&description, root).zip(
            cmake_source_path(&header_template, root)
                .zip(cmake_source_path(&source_template, root)),
        );
        let Some((description, (header_template, source_template))) = rendered else {
            scan.skipped.push(format!(
                "{}:{}: FlexCat {} has a description or template outside the source tree",
                declaring_dir, rule.line, source
            ));
            continue;
        };

        scan.declarations.push(FlexCatSourceDecl {
            owner: owners[0].clone(),
            declaring_dir: declaring_dir.clone(),
            line: rule.line,
            source: source.to_owned(),
            header,
            description,
            header_template,
            source_template,
            catalog_destination: catalog.destination,
            catalog_name: catalog.name,
            catalog_source_dir: catalog.source_dir,
            languages: catalog.languages,
            consumers: Vec::new(),
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

fn flexcat_recipe(rule: &Rule<'_>) -> Option<(String, Vec<(String, String)>)> {
    let [recipe] = rule.recipes.as_slice() else {
        return None;
    };
    let recipe = recipe.strip_prefix('@').unwrap_or(recipe);
    let mut words = recipe.split_whitespace();
    if words.next()? != "$(FLEXCAT)" {
        return None;
    }
    let description = words.next()?.to_owned();
    let mut outputs = Vec::new();
    for word in words {
        let (output, template) = word.split_once('=')?;
        if output.is_empty() || template.is_empty() {
            return None;
        }
        outputs.push((output.to_owned(), template.to_owned()));
    }
    (!outputs.is_empty()).then_some((description, outputs))
}

/// The catalog products that accompany a hand-written FlexCat source rule.
/// Each field stays absent unless the declaring directory carries exactly one
/// `.catalog` rule of the accepted shape; naming them keeps the four-place
/// tuple of `Option`s out of the signature.
#[derive(Default)]
struct CatalogOutputs {
    destination: Option<String>,
    name: Option<String>,
    source_dir: Option<String>,
    languages: Vec<String>,
}

fn catalog_outputs(
    rules: &[Rule<'_>],
    root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> CatalogOutputs {
    let mut matches = Vec::new();
    for rule in rules {
        let target = rule.target.trim();
        let Some(catalog_name) = target
            .strip_prefix("$(TARGETDIR)/%/")
            .and_then(|value| value.strip_suffix(".catalog"))
        else {
            continue;
        };
        if !simple_name(catalog_name) {
            continue;
        }
        let Some(po_dir) = rule
            .prereqs
            .trim()
            .strip_prefix("$(SRCDIR)/$(CURDIR)/")
            .and_then(|value| value.strip_suffix("/%.po"))
        else {
            continue;
        };
        let Some(po_dir) = safe_relative(po_dir) else {
            continue;
        };
        let [echo, mkdir, flexcat] = rule.recipes.as_slice() else {
            continue;
        };
        if !echo.contains("Building catalog")
            || !mkdir.contains("$(MKDIR)")
            || !is_po_catalog_recipe(flexcat)
        {
            continue;
        }
        let Some(destination_raw) = scope.raw_at("TARGETDIR", usize::MAX) else {
            continue;
        };
        let local = |name: &str| scope.raw_at(name, usize::MAX);
        // Common output roots (including AROS_CATALOGS) are inherited from
        // config/make.cfg.in.  Prefer that authoritative mapping before the
        // flattened local scope, which also contains the included config and
        // can otherwise shadow its CMake-specific seed values.
        let Some(destination) = dirs
            .expand(&destination_raw)
            .or_else(|| dirs.expand_with(&destination_raw, &local).ok())
        else {
            continue;
        };
        if destination.is_empty() {
            continue;
        }
        let po_root = root.join(rel_dir).join(&po_dir);
        let Some(languages) = po_languages(&po_root) else {
            continue;
        };
        matches.push((
            destination,
            catalog_name.to_owned(),
            slash_path(&po_dir),
            languages,
        ));
    }
    if matches.len() != 1 {
        return CatalogOutputs::default();
    }
    let (destination, name, source_dir, languages) = matches.pop().expect("one catalog match");
    CatalogOutputs {
        destination: Some(destination),
        name: Some(name),
        source_dir: Some(source_dir),
        languages,
    }
}

fn is_po_catalog_recipe(recipe: &str) -> bool {
    let recipe = recipe.trim();
    let recipe = recipe.strip_prefix('@').unwrap_or(recipe);
    recipe == "$(FLEXCAT) POFILE $< CATALOG $@"
}

fn po_languages(directory: &Path) -> Option<Vec<String>> {
    let entries = fs::read_dir(directory).ok()?;
    let mut languages = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("po"))
            .then(|| path.file_stem()?.to_str().map(str::to_owned))
            .flatten()
        })
        .filter(|language| {
            !language.is_empty()
                && language
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        })
        .collect::<Vec<_>>();
    languages.sort();
    languages.dedup();
    (!languages.is_empty()).then_some(languages)
}

fn source_path(raw: &str, root: &Path, rel_dir: &Path) -> Option<PathBuf> {
    let relative = raw.strip_prefix("$(SRCDIR)/$(CURDIR)/")?;
    if relative.is_empty() || relative.contains(['$', ';']) || Path::new(relative).is_absolute() {
        return None;
    }
    let canonical_root = fs::canonicalize(root).ok()?;
    let path = fs::canonicalize(root.join(rel_dir).join(relative)).ok()?;
    (path.is_file() && path.starts_with(&canonical_root)).then_some(path)
}

fn safe_relative(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains('$') || raw.contains(';') || Path::new(raw).is_absolute() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        .then_some(path)
}

fn simple_file_name(value: &str, suffix: &str) -> bool {
    !value.is_empty()
        && value.ends_with(suffix)
        && !value.contains(['/', '\\', '$', ';'])
        && simple_name(value.trim_end_matches(suffix))
}

fn simple_target_name(value: &str) -> bool {
    simple_name(value)
}

fn simple_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn words(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_owned).collect()
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Renders a source-tree path in the form the generated CMake uses everywhere
/// else, so the emitted file does not depend on where the tree is checked out.
///
/// It used to normalise separators and nothing more, which left the absolute
/// host path of every FlexCat description and template in
/// generated_targets.cmake: twelve lines that made the file unusable anywhere
/// but the machine that wrote it.
///
/// `None` if the path is not below `root`, which cannot happen for a path this
/// module built, and must be reported rather than emitted if it ever does.
fn cmake_source_path(path: &Path, root: &Path) -> Option<String> {
    let canonical_root = fs::canonicalize(root).ok()?;
    let relative = path.strip_prefix(canonical_root).ok()?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() {
        return None;
    }
    Some(format!("${{CMAKE_SOURCE_DIR}}/{relative}"))
}

#[cfg(test)]
mod tests {
    use super::collect_flexcat_source_rules;
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars;
    use crate::parser::join_continuations;
    use aros_common::read_source;
    use std::path::Path;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    #[test]
    fn nlisttree_hand_rule_becomes_source_and_po_catalog_outputs() {
        let root = root();
        let rel = Path::new("workbench/classes/zune/nlist/nlisttree_mcp");
        let content = read_source(&root.join(rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scan = collect_flexcat_source_rules(
            &content,
            &root,
            rel,
            &collect_vars(&joined),
            &DirVars::load(&root),
        );

        assert!(scan.skipped.is_empty(), "{:#?}", scan.skipped);
        assert_eq!(scan.declarations.len(), 1);
        let declaration = &scan.declarations[0];
        assert_eq!(declaration.owner, "classes-zune-nlisttree-mcp-catalogs");
        assert_eq!(declaration.source, "locale.c");
        assert_eq!(declaration.header, "locale.h");
        assert_eq!(declaration.catalog_name.as_deref(), Some("NListtree_mcp"));
        assert_eq!(declaration.catalog_source_dir.as_deref(), Some("locale"));
        assert_eq!(
            declaration.catalog_destination.as_deref(),
            Some("${AROS_BUILD_DIR}/SYS/Locale/Catalogs")
        );
        assert!(declaration.languages.contains(&"german".to_owned()));
        assert!(declaration.languages.contains(&"russian".to_owned()));
    }

    #[test]
    fn openurl_header_only_rule_keeps_parent_description_path() {
        let root = root();
        let rel = Path::new("external/openurl/prefs");
        let content = read_source(&root.join(rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scan = collect_flexcat_source_rules(
            &content,
            &root,
            rel,
            &collect_vars(&joined),
            &DirVars::load(&root),
        );

        assert!(scan.skipped.is_empty(), "{:#?}", scan.skipped);
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.headers.len(), 1);
        let header = &scan.headers[0];
        assert_eq!(header.owner, "external-openurl-prefs-setup");
        assert_eq!(header.header, "locale.h");
        assert_eq!(
            header.description,
            "${CMAKE_SOURCE_DIR}/external/openurl/locale/OpenURL.pot"
        );
        assert_eq!(
            header.header_template,
            "${CMAKE_SOURCE_DIR}/external/openurl/prefs/locale_h.sd"
        );
    }

    #[test]
    fn arbitrary_flexcat_recipe_is_not_admitted() {
        let root = root();
        let input = concat!(
            "owner : locale.h locale.c\n",
            "locale.h: locale.c\n",
            "locale.c: $(SRCDIR)/$(CURDIR)/locale/Messages.pot $(SRCDIR)/$(CURDIR)/C_h.sd $(SRCDIR)/$(CURDIR)/C_c.sd\n",
            "\t$(FLEXCAT) $(SRCDIR)/$(CURDIR)/locale/Messages.pot locale.h=$(SRCDIR)/$(CURDIR)/C_h.sd locale.c=$(SRCDIR)/$(CURDIR)/C_c.sd EXTRA\n"
        );
        let rel = Path::new("workbench/classes/zune/nlist/nlisttree_mcp");
        let scan = collect_flexcat_source_rules(
            input,
            &root,
            rel,
            &collect_vars(input),
            &DirVars::load(&root),
        );
        assert!(scan.declarations.is_empty());
    }
}
