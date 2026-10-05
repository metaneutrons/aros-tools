//! `%make_hidd_stubs`: the six declarations that fill `libhiddstubs.a`.
//!
//! A HIDD's public API is a set of hand-written stubs that turn a call into an
//! `OOP_DoMethod`. `config/make.tmpl:3551` compiles each declaration's
//! `$(STUBS)` into `$(GENDIR)/lib/hidd/`, and
//! `compiler/libhiddstubs/mmakefile.src` archives whatever is in that directory
//! into `libhiddstubs.a`:
//!
//! ```text
//! HIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))
//! $(HIDD_LIB) : $(HIDD_STUBS_OBJ)
//!         %mklib_q from=$^
//! ```
//!
//! Nothing modelled the macro, so 61 declarations that state
//! `uselibs=hiddstubs` had no archive to link -- reported all along in
//! `generated_targets.unresolved-uselibs.txt`. The visible consequence was one
//! module: `serialmouse.hidd` kept `HIDD_Serial_NewUnit` undefined, and the ELF
//! loader refuses a whole boot over one unresolved symbol, so no package could
//! be passed to the kickstart at all.
//!
//! The wildcard inputs can only be matched to the declarations after the whole
//! tree has been parsed. The source-local archive rule is therefore proved
//! separately from the macro declarations. Each declaration also needs its own
//! compile-input snapshot: the macro captures `CFLAGS` and `CPPFLAGS` at the
//! invocation, and those values include source-local `USER_CFLAGS`,
//! `USER_INCLUDES` and `USER_CPPFLAGS` through `config/target.cfg.in:118-120`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, AssignmentKind, VarScope};

/// One `%make_hidd_stubs` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddStubsDecl {
    /// The `hidd=` name, which names the MetaMake target but not the source.
    pub hidd: String,
    /// Directory of the declaring mmakefile, relative to the source root.
    pub directory: String,
    /// Source stems from `$(STUBS)`, relative to the source root.
    pub sources: Vec<String>,
}

/// A closed source-local proof of one archive owner and its object wildcard.
///
/// This records what the Make source proves; it does not itself create a graph
/// producer or authorize a global archive synthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddStubsArchiveProof {
    /// Literal `#MM` owner whose prerequisite is the archive.
    pub owner: String,
    /// Archive basename without `lib` or `.a`.
    pub archive_name: String,
    /// Archive under the configured `AROS_LIB` root.
    pub archive: String,
    /// Object wildcard under the configured `GENDIR` root.
    pub objects_pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ArchiveProofLine {
    Statement { text: String, line: usize },
    Marker,
    Recipe { text: String },
}

/// Proves the exact closed source shape that owns the HIDD-stubs archive.
///
/// `content` must be continuation-joined, and `scope` must have been collected
/// from that same snapshot. The recognized file is deliberately a whole-file
/// shape: unknown rules, recipes, aliases, includes, or Make controls invalidate
/// the proof instead of being ignored.
/// # Errors
/// Rejects any source rule or variable shape outside the exact archive contract.
pub fn prove_hidd_stubs_archive(
    content: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> Result<HiddStubsArchiveProof, String> {
    const EXPECTED_LINES: usize = 12;
    const HIDD_STUBS_OBJECTS: &str = "$(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))";

    let lines = archive_proof_lines(content)?;
    if lines.len() != EXPECTED_LINES {
        return Err(format!(
            "expected the closed HIDD-stubs archive file shape ({EXPECTED_LINES} active lines), found {}",
            lines.len()
        ));
    }

    require_archive_statement(&lines[0], "include $(SRCDIR)/config/aros.cfg")?;
    let hidd_lib_line = require_assignment(&lines[1], "HIDD_LIB", AssignmentKind::SimpleSet)?;
    let hidd_obj_line = require_assignment(&lines[2], "HIDD_STUBS_OBJ", AssignmentKind::SimpleSet)?;
    if assignment_value(&lines[2])? != HIDD_STUBS_OBJECTS {
        return Err("HIDD_STUBS_OBJ is not the exact generated HIDD object wildcard".to_owned());
    }

    require_archive_marker(&lines[3])?;
    let owner = parse_archive_owner(&lines[4])?;
    require_archive_statement(&lines[5], "$(HIDD_LIB) : $(HIDD_STUBS_OBJ)")?;
    require_archive_recipe(&lines[6], "%mklib_q from=$^")?;
    require_archive_statement(&lines[7], "setup ::")?;
    require_archive_recipe(&lines[8], "%mkdirs_q $(AROS_LIB) $(GENDIR)/lib/hidd")?;
    require_archive_marker(&lines[9])?;
    require_archive_statement(&lines[10], "clean ::")?;
    require_archive_recipe(&lines[11], "-@$(RM) $(HIDD_LIB) $(GENDIR)/lib/hidd")?;

    let assignment = assignment_value(&lines[1])?;
    let Some(archive_name) = assignment
        .strip_prefix("$(AROS_LIB)/lib")
        .and_then(|value| value.strip_suffix(".a"))
    else {
        return Err("HIDD_LIB must be a literal lib<name>.a under AROS_LIB".to_owned());
    };
    if !safe_make_name(archive_name) {
        return Err("HIDD_LIB archive name is not a safe literal".to_owned());
    }

    let archive_root = evaluate_at("$(AROS_LIB)", hidd_lib_line, scope, dirs, root, rel_dir)?;
    let generated_root = evaluate_at("$(GENDIR)", hidd_obj_line, scope, dirs, root, rel_dir)?;
    if !safe_configured_path(&archive_root) || !safe_configured_path(&generated_root) {
        return Err("configured AROS_LIB or GENDIR root is unresolved or unsafe".to_owned());
    }

    Ok(HiddStubsArchiveProof {
        owner,
        archive_name: archive_name.to_owned(),
        archive: format!("{}/lib{archive_name}.a", archive_root.trim_end_matches('/')),
        objects_pattern: format!("{}/lib/hidd/*.o", generated_root.trim_end_matches('/')),
    })
}

fn archive_proof_lines(content: &str) -> Result<Vec<ArchiveProofLine>, String> {
    let mut lines = Vec::new();
    for (line, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "#MM" {
            lines.push(ArchiveProofLine::Marker);
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }

        if let Some(recipe) = raw.strip_prefix('\t') {
            let recipe = strip_make_comment(recipe).trim_end();
            if recipe.is_empty() {
                return Err(format!("empty recipe at line {}", line + 1));
            }
            lines.push(ArchiveProofLine::Recipe {
                text: recipe.to_owned(),
            });
            continue;
        }
        if raw != trimmed {
            return Err(format!(
                "unexpected indented Make statement at line {}",
                line + 1
            ));
        }
        let statement = strip_make_comment(raw).trim_end();
        if statement.is_empty() {
            continue;
        }
        lines.push(ArchiveProofLine::Statement {
            text: statement.to_owned(),
            line,
        });
    }
    Ok(lines)
}

fn require_archive_statement(line: &ArchiveProofLine, expected: &str) -> Result<(), String> {
    match line {
        ArchiveProofLine::Statement { text, .. } if text == expected => Ok(()),
        other => Err(format!("expected `{expected}`, found {other:?}")),
    }
}

fn require_archive_marker(line: &ArchiveProofLine) -> Result<(), String> {
    match line {
        ArchiveProofLine::Marker => Ok(()),
        other => Err(format!("expected a literal #MM marker, found {other:?}")),
    }
}

fn require_archive_recipe(line: &ArchiveProofLine, expected: &str) -> Result<(), String> {
    match line {
        ArchiveProofLine::Recipe { text, .. } if text == expected => Ok(()),
        other => Err(format!("expected recipe `{expected}`, found {other:?}")),
    }
}

fn require_assignment(
    line: &ArchiveProofLine,
    expected_name: &str,
    expected_kind: AssignmentKind,
) -> Result<usize, String> {
    let ArchiveProofLine::Statement { text, line } = line else {
        return Err(format!(
            "expected {expected_name} assignment, found {line:?}"
        ));
    };
    let Some((name, _, kind)) = variable_assignment(text) else {
        return Err(format!(
            "expected {expected_name} assignment at line {}",
            line + 1
        ));
    };
    if name != expected_name
        || kind != expected_kind
        || !text.starts_with(&format!("{expected_name} := "))
    {
        return Err(format!("unexpected assignment at line {}", line + 1));
    }
    Ok(*line)
}

fn assignment_value(line: &ArchiveProofLine) -> Result<&str, String> {
    let ArchiveProofLine::Statement { text, .. } = line else {
        return Err(format!("expected assignment, found {line:?}"));
    };
    let Some((_, value)) = text.split_once(":=") else {
        return Err(format!("expected simple assignment, found `{text}`"));
    };
    Ok(value.trim())
}

fn parse_archive_owner(line: &ArchiveProofLine) -> Result<String, String> {
    let ArchiveProofLine::Statement { text, .. } = line else {
        return Err(format!("expected archive owner rule, found {line:?}"));
    };
    let Some((owner, prerequisites)) = text.split_once(':') else {
        return Err(format!(
            "archive owner rule has no prerequisite separator: `{text}`"
        ));
    };
    let owner = owner.trim();
    let prerequisites = prerequisites.trim();
    if !safe_make_name(owner) || prerequisites != "$(HIDD_LIB)" || text.contains("::") {
        return Err(format!(
            "archive owner rule is not a safe literal edge: `{text}`"
        ));
    }
    Ok(owner.to_owned())
}

fn safe_make_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
}

fn safe_configured_path(value: &str) -> bool {
    !value.is_empty()
        && (value.starts_with("${AROS_BUILD_DIR}/") || Path::new(value).is_absolute())
        && !value.contains(['*', ';', '\n', '\r'])
        && !value.contains("$(")
}

fn evaluate_at(
    expression: &str,
    line: usize,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> Result<String, String> {
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
}

/// Collects the `%make_hidd_stubs` declarations of one mmakefile.
///
/// Returns the declarations and, for reporting, the ones whose sources could not
/// be resolved. Nothing is dropped silently.
#[must_use]
pub fn collect_hidd_stubs(
    content: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> (Vec<HiddStubsDecl>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let directory = rel_dir.to_string_lossy().replace('\\', "/");

    for body in crate::includes::directive_bodies_pub(content, "%make_hidd_stubs") {
        let Some(hidd) = crate::includes::arg_value(&body, "hidd") else {
            skipped.push(format!(
                "{directory}: %make_hidd_stubs without hidd=, which the macro \
                 requires (make.tmpl:3551 marks it /A)"
            ));
            continue;
        };
        let expressions = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
        // `STUBS` is the macro's only source lane, and it is a file variable
        // rather than a macro argument.
        let stubs = match evaluate_make_expr("$(STUBS)", &expressions) {
            Ok(value) => value,
            Err(error) => {
                skipped.push(format!(
                    "{directory}: %make_hidd_stubs hidd={hidd} cannot resolve \
                     $(STUBS): {error}"
                ));
                continue;
            }
        };
        let mut sources = Vec::new();
        let mut unresolved = false;
        for stem in stubs.split_whitespace() {
            if stem.contains(['$', '*', ';']) {
                skipped.push(format!(
                    "{directory}: %make_hidd_stubs hidd={hidd} source `{stem}` \
                     is not a plain stem"
                ));
                unresolved = true;
                continue;
            }
            sources.push(if directory.is_empty() {
                stem.to_owned()
            } else {
                format!("{directory}/{stem}")
            });
        }
        if unresolved {
            continue;
        }
        if sources.is_empty() {
            skipped.push(format!(
                "{directory}: %make_hidd_stubs hidd={hidd} has an empty $(STUBS)"
            ));
            continue;
        }
        out.push(HiddStubsDecl {
            hidd,
            directory: directory.clone(),
            sources,
        });
    }

    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars;
    use crate::parser::join_continuations;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        crate::testing::root()
    }

    fn collect(rel: &str) -> (Vec<HiddStubsDecl>, Vec<String>) {
        let root = root();
        let rel = PathBuf::from(rel);
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        collect_hidd_stubs(&joined, &scope, &dirs, &root, &rel)
    }

    #[test]
    fn the_serial_stubs_resolve_through_modname() {
        let (decls, skipped) = collect("workbench/hidds/serial");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls.len(), 1, "{decls:#?}");
        assert_eq!(decls[0].hidd, "serial");
        assert_eq!(decls[0].sources, ["workbench/hidds/serial/serial_stubs"]);
    }

    #[test]
    fn the_hidd_name_is_not_the_source_name() {
        // `%make_hidd_stubs hidd=mstorage` with `STUBS := storage_stubs`, so a
        // model that built the file name from hidd= would miss this one.
        let (decls, skipped) = collect("workbench/devs/USB/classes/MassStorage");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls[0].hidd, "mstorage");
        assert_eq!(
            decls[0].sources,
            ["workbench/devs/USB/classes/MassStorage/storage_stubs"]
        );
    }

    fn collect_fixture(content: &str) -> (Vec<HiddStubsDecl>, Vec<String>) {
        let rel = PathBuf::from("fixture");
        let joined = join_continuations(content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root());
        collect_hidd_stubs(&joined, &scope, &dirs, &root(), &rel)
    }

    #[test]
    fn a_declaration_with_no_stubs_variable_is_reported() {
        let (decls, skipped) = collect_fixture("%make_hidd_stubs hidd=nothing\n");
        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(
            skipped[0].contains("cannot resolve $(STUBS)"),
            "{skipped:?}"
        );
    }

    #[test]
    fn a_declaration_with_an_empty_stubs_variable_is_reported() {
        let (decls, skipped) = collect_fixture("STUBS :=\n%make_hidd_stubs hidd=nothing\n");
        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("empty $(STUBS)"), "{skipped:?}");
    }

    fn prove_archive(content: &str) -> Result<HiddStubsArchiveProof, String> {
        let root = root();
        let rel = PathBuf::from("compiler/libhiddstubs");
        let joined = join_continuations(content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        prove_hidd_stubs_archive(&joined, &scope, &dirs, &root, &rel)
    }

    fn actual_archive_source() -> String {
        aros_common::read_source(&root().join("compiler/libhiddstubs").join("mmakefile.src"))
            .unwrap()
    }

    #[test]
    fn the_source_owned_hidd_archive_rule_has_a_closed_proof() {
        let proof = prove_archive(&actual_archive_source()).unwrap();
        assert_eq!(proof.owner, "linklibs-hiddstubs");
        assert_eq!(proof.archive_name, "hiddstubs");
        assert!(
            proof.archive.starts_with("${AROS_BUILD_DIR}/"),
            "{proof:#?}"
        );
        assert!(proof.archive.ends_with("/libhiddstubs.a"), "{proof:#?}");
        assert!(
            proof.objects_pattern.starts_with("${AROS_BUILD_DIR}/gen/"),
            "{proof:#?}"
        );
        assert!(
            proof.objects_pattern.ends_with("/gen/lib/hidd/*.o"),
            "{proof:#?}"
        );
    }

    #[test]
    fn the_archive_name_and_meta_owner_are_derived_from_source() {
        let source = actual_archive_source()
            .replace("linklibs-hiddstubs", "linklibs-source-owned")
            .replace("libhiddstubs.a", "libsource-owned.a");
        let proof = prove_archive(&source).unwrap();
        assert_eq!(proof.owner, "linklibs-source-owned");
        assert_eq!(proof.archive_name, "source-owned");
        assert!(proof.archive.ends_with("/libsource-owned.a"), "{proof:#?}");
    }

    #[test]
    fn the_hidd_archive_proof_rejects_recipe_drift_aliases_and_controls() {
        let source = actual_archive_source();
        let cases = [
            (
                "changed archive recipe",
                source.replace(
                    "%mklib_q from=$^",
                    "%mklib_q from=$(HIDD_STUBS_OBJ)",
                ),
            ),
            (
                "extra archive recipe",
                source.replace(
                    "\t%mklib_q from=$^",
                    "\t%mklib_q from=$^\n\t@echo bypass",
                ),
            ),
            (
                "duplicate archive rule",
                source.replace(
                    "$(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\t%mklib_q from=$^",
                    "$(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\t%mklib_q from=$^\n$(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\t%mklib_q from=$^",
                ),
            ),
            (
                "archive-root alias",
                source.replace(
                    "HIDD_LIB := $(AROS_LIB)/libhiddstubs.a",
                    "HIDD_LIB := $(ARCHIVE_ROOT)/libhiddstubs.a\nARCHIVE_ROOT := $(AROS_LIB)",
                ),
            ),
            (
                "conditional archive owner",
                source.replace(
                    "#MM\nlinklibs-hiddstubs: $(HIDD_LIB)",
                    "#MM\nifeq ($(AROS_TARGET_CPU),riscv32)\nlinklibs-hiddstubs: $(HIDD_LIB)\nendif",
                ),
            ),
        ];

        for (label, changed) in cases {
            assert_ne!(source, changed, "test mutation did not apply: {label}");
            assert!(
                prove_archive(&changed).is_err(),
                "unsafe source unexpectedly proved: {label}"
            );
        }
    }
}
