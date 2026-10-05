//! Architecture-specific source overrides from `%build_archspecific`.
//!
//! A module's generic implementation can be replaced per architecture. The
//! generic `rom/exec/stackswap.c` is a stub whose body is `#error The function
//! StackSwap() has not been implemented in the kernel.`; the real code lives in
//! `arch/x86_64-all/exec/stackswap.S`. The architecture directory declares this
//! with `%build_archspecific`, naming the target it contributes to.
//!
//! The reference build compiles the architecture files into a separate `arch/`
//! object directory and then removes the same-named generic files from the
//! module's own list (`config/make.tmpl:1661`):
//!
//! ```text
//! <mmake>_ARCHFILES     := $(basename $(notdir $(<mmake>_ARCHOBJS)))
//! <mmake>_C_NARCHFILES  := $(filter-out $(<mmake>_ARCHFILES),$(<mmake>_FILES))
//! ```
//!
//! So the override is by base name, and the architecture object comes first in
//! the link. Both properties are reproduced here.
//!
//! Selection stays with CMake: the declarations carry the architecture tag from
//! `arch=`, and only the tags that apply to the configured target are used.
//! That keeps the transpiler target-agnostic, as with `%set_archincludes`.

use crate::parser::TargetContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One `%build_archspecific` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchSourceDecl {
    /// The `mainmmake=` target these sources belong to.
    pub mainmmake: String,
    /// `maindir=`: the directory of the declaration being extended. Needed
    /// because a second declaration in that directory can share the arch
    /// object root and so inherit these overrides; see
    /// `DependencyGraph::resolve_arch_sources`.
    pub maindir: Option<String>,
    /// `modname=`: needed with maindir to name the arch object root, which is
    /// `$(GENDIR)/<maindir>/<modname>/arch` (config/make.tmpl:3296).
    pub modname: Option<String>,
    /// `arch=`: the architecture tag this declaration applies to.
    pub tag: String,
    /// Directory holding the sources, relative to the source root.
    pub dir: String,
    /// Source base names. Extensions are resolved by the build, so a `.S` file
    /// overriding a `.c` file needs no special handling here.
    pub files: Vec<String>,
    /// Explicit `incextra=` quote-only search path for this declaration's
    /// architecture sources. Bound in declaration-local scope before emission.
    #[serde(default)]
    pub extra_quote_include: Option<String>,
    /// Include directories the declaring mmakefile sets. These belong to the
    /// target being extended, not to this file: arch/arm-native/kernel adds
    /// -I$(SRCDIR)/rom/openfirmware, and without it the kernel cannot find
    /// of_intern.h.
    pub include_dirs: Vec<String>,
    /// Definitions the declaring mmakefile sets.
    pub defines: Vec<String>,
    /// Codegen options the declaring mmakefile sets.
    pub compile_options: Vec<String>,
    /// 0-based line the declaration starts on. The flags belong to the
    /// declaration, not to the file: `arch/i386-all/hidd/gfx` sets
    /// `USER_CFLAGS` three times, once per lane.
    pub line: usize,
}

/// Collects `VAR := / = / += value` file lists, keeping plain names only.
/// Expands a single `$(VAR)` from the declaring file's own variables.
///
/// `maindir=` and `modname=` are written either literally
/// (arch/x86_64-all/stdc) or through a file-local variable
/// (arch/x86_64-pc/kernel says `maindir=$(MAINDIR)`). Both name the arch object
/// root, so a raw value silently fails to match it.
fn file_local(value: Option<&String>, raw: &HashMap<String, String>) -> Option<String> {
    let stated = value?.trim();
    if stated.is_empty() {
        return None;
    }
    let Some(name) = stated.strip_prefix("$(").and_then(|v| v.strip_suffix(')')) else {
        return (!stated.contains('$')).then(|| stated.to_owned());
    };
    let resolved = raw.get(name)?.trim();
    (!resolved.is_empty() && !resolved.contains('$') && !resolved.contains(char::is_whitespace))
        .then(|| resolved.to_owned())
}

/// State of one `ifeq`/`ifneq` block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Branch {
    Active,
    Inactive,
    /// The condition could not be decided, so neither branch may be applied.
    Unknown,
}

/// Decides one `ifeq`/`ifneq` condition, or `None` if it cannot be decided.
///
/// Both sides are compared after substituting `$(VAR)` from the variables
/// assigned earlier in this file and from the target parameters. A reference
/// this cannot resolve, or any Make function, makes the condition undecidable
/// rather than false.
fn decide_condition(
    kind: &str,
    raw: &str,
    vars: &HashMap<String, Vec<String>>,
    target: Option<&TargetContext>,
) -> Option<bool> {
    let inner = raw.trim();
    let inner = inner
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))?;
    let (left, right) = inner.split_once(',')?;
    let substitute = |text: &str| -> Option<String> {
        let mut out = String::new();
        let mut rest = text.trim().trim_matches('"');
        while let Some(start) = rest.find("$(") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find(')')?;
            let name = &after[..end];
            // A Make function call, which this does not evaluate.
            if name.contains(' ') {
                return None;
            }
            if let Some(local) = vars.get(name) {
                out.push_str(&local.join(" "));
            } else {
                let value = target.and_then(|target| target.value_of(name))?;
                out.push_str(&value);
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out.trim().to_owned())
    };
    let equal = substitute(left)? == substitute(right)?;
    match kind {
        "ifeq" => Some(equal),
        "ifneq" => Some(!equal),
        _ => None,
    }
}

/// Collects the plain variable assignments of one arch `mmakefile.src`.
///
/// Make conditionals are honoured. Without that, `arch/x86_64-pc/kernel`
/// contributed `kernel_early` and `kernel_trapdebug` to the kernel although
/// its own lines 5 and 7 set `KERNEL_USE_EARLYTRAP=` and
/// `KERNEL_USE_TRAPDEBUG=` empty, so the reference build compiles neither.
/// kernel_early.c defines `print_crash_info` static and calls it only from
/// inline assembly, which left it undefined in kernel.resource.
///
/// An undecidable condition applies neither branch and is reported: a guessed
/// branch is worse than a stated gap.
fn collect_file_vars(
    content: &str,
    target: Option<&TargetContext>,
    undecided: &mut Vec<String>,
    raw: &mut HashMap<String, String>,
) -> HashMap<String, Vec<String>> {
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending: Option<String> = None;
    let mut stack: Vec<Branch> = Vec::new();

    for (number, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        // Conditional bookkeeping runs whatever the current state is, so a
        // nested block inside an inactive one is still balanced correctly.
        let word = trimmed.split_whitespace().next().unwrap_or_default();
        match word {
            "ifeq" | "ifneq" => {
                pending = None;
                let rest = trimmed[word.len()..].trim();
                let branch = if stack.iter().any(|state| *state != Branch::Active) {
                    // Inside a block that does not apply, the inner condition
                    // is irrelevant and must not be reported.
                    Branch::Inactive
                } else {
                    match decide_condition(word, rest, &vars, target) {
                        Some(true) => Branch::Active,
                        Some(false) => Branch::Inactive,
                        None => {
                            undecided.push(format!("{}: {trimmed}", number + 1));
                            Branch::Unknown
                        }
                    }
                };
                stack.push(branch);
                continue;
            }
            "ifdef" | "ifndef" => {
                pending = None;
                // Nothing here knows the global variable state, so this is
                // never decided.
                if stack.iter().all(|state| *state == Branch::Active) {
                    undecided.push(format!("{}: {trimmed}", number + 1));
                }
                stack.push(Branch::Unknown);
                continue;
            }
            "else" => {
                pending = None;
                if let Some(state) = stack.last_mut() {
                    *state = match *state {
                        Branch::Active => Branch::Inactive,
                        Branch::Inactive => Branch::Active,
                        Branch::Unknown => Branch::Unknown,
                    };
                }
                continue;
            }
            "endif" => {
                pending = None;
                stack.pop();
                continue;
            }
            _ => {}
        }
        if stack.iter().any(|state| *state != Branch::Active) {
            pending = None;
            continue;
        }

        if let Some(name) = pending.take() {
            let toks = plain_tokens(payload);
            vars.entry(name.clone()).or_default().extend(toks);
            if continues {
                pending = Some(name);
            }
            continue;
        }

        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }

        let Some((lhs, rhs, append)) = split_assignment(payload) else {
            continue;
        };
        let name = lhs.trim().to_owned();
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        // plain_tokens keeps only usable source base names, so a path value
        // like `MAINDIR := rom/kernel` is filtered out of `vars` entirely.
        // Keep the raw right-hand side too: maindir= is written through such a
        // variable, and without it the arch object root cannot be named.
        let trimmed_rhs = rhs.trim();
        if append {
            raw.entry(name.clone())
                .and_modify(|value| {
                    value.push(' ');
                    value.push_str(trimmed_rhs);
                })
                .or_insert_with(|| trimmed_rhs.to_owned());
        } else {
            raw.insert(name.clone(), trimmed_rhs.to_owned());
        }
        let entry = vars.entry(name.clone()).or_default();
        if !append {
            entry.clear();
        }
        entry.extend(plain_tokens(rhs));
        if continues {
            pending = Some(name);
        }
    }

    vars
}

/// Keeps only tokens that are usable as a source base name.
fn plain_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|t| *t != "\\")
        .map(|t| t.trim_matches('"').to_owned())
        .filter(|t| !t.is_empty() && !t.contains('$') && !t.contains('(') && !t.contains('/'))
        .collect()
}

fn split_assignment(line: &str) -> Option<(&str, &str, bool)> {
    if let Some((l, r)) = line.split_once(":=") {
        return Some((l, r, false));
    }
    if let Some((l, r)) = line.split_once("+=") {
        return Some((l, r, true));
    }
    let (l, r) = line.split_once('=')?;
    if l.contains(':') || l.is_empty() {
        return None;
    }
    Some((l, r, false))
}

/// Expands a file list argument, substituting `$(VAR)` from the same file.
fn expand_list(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for tok in raw.split_whitespace() {
        let t = tok.trim_matches('"');
        if let Some(name) = t.strip_prefix("$(").and_then(|x| x.strip_suffix(')')) {
            if let Some(v) = vars.get(name) {
                for item in v {
                    if !out.contains(item) {
                        out.push(item.clone());
                    }
                }
            }
            continue;
        }
        for item in plain_tokens(t) {
            if !out.contains(&item) {
                out.push(item);
            }
        }
    }
    out
}

/// Parses the `%build_archspecific` declarations of one `mmakefile.src`.
///
/// Returns the resolved declarations and, for reporting, the ones whose file
/// list could not be resolved.
#[must_use]
pub fn collect_arch_sources(
    content: &str,
    rel_dir: &Path,
    target: Option<&TargetContext>,
) -> (Vec<ArchSourceDecl>, Vec<String>) {
    let dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut undecided = Vec::new();
    let mut raw_vars: HashMap<String, String> = HashMap::new();
    let vars = collect_file_vars(content, target, &mut undecided, &mut raw_vars);
    let mut out = Vec::new();
    let mut skipped: Vec<String> = undecided
        .into_iter()
        .map(|line| {
            format!("{dir}/mmakefile.src:{line}: condition not decided, neither branch applied")
        })
        .collect();

    for (line, body) in crate::includes::directive_bodies_at(content, "%build_archspecific") {
        let Some(mainmmake) = crate::includes::arg_value(&body, "mainmmake") else {
            continue;
        };
        let Some(tag) = crate::includes::arg_value(&body, "arch") else {
            continue;
        };

        // files, cxxfiles and asmfiles all land in the module's arch object
        // directory. linklibfiles go to linklib/arch and are not part of the
        // module's own object list, so they are not overrides.
        let mut files = Vec::new();
        for key in ["files", "cxxfiles", "asmfiles", "objcfiles"] {
            let raw = crate::includes::arg_value_quoted(&body, key)
                .or_else(|| crate::includes::arg_value(&body, key));
            if let Some(raw) = raw {
                for f in expand_list(&raw, &vars) {
                    if !files.contains(&f) {
                        files.push(f);
                    }
                }
            }
        }

        if files.is_empty() {
            skipped.push(format!("{dir}: mainmmake={mainmmake} arch={tag}"));
            continue;
        }

        out.push(ArchSourceDecl {
            mainmmake,
            maindir: file_local(
                crate::includes::arg_value(&body, "maindir").as_ref(),
                &raw_vars,
            ),
            modname: file_local(
                crate::includes::arg_value(&body, "modname").as_ref(),
                &raw_vars,
            ),
            tag,
            dir: dir.clone(),
            files,
            extra_quote_include: crate::includes::arg_value_quoted(&body, "incextra")
                .or_else(|| crate::includes::arg_value(&body, "incextra")),
            // Filled in by the caller, which has already collected them.
            include_dirs: Vec::new(),
            defines: Vec::new(),
            compile_options: Vec::new(),
            line,
        });
    }

    (out, skipped)
}

/// Parses `%build_archspecific` declarations against the Make configuration
/// and variable scope at each declaration. `content`, `scope`, and `states`
/// must all describe the same continuation-joined, configuration-inlined text.
///
/// Unlike the legacy collector above, this path does not reconstruct file
/// variables from the final textual assignments. It uses the shared
/// declaration-position evaluator, and rejects a declaration as a whole if
/// any language lane is incomplete or depends on an unresolved selector.
pub(crate) fn collect_arch_sources_with_scope(
    content: &str,
    rel_dir: &Path,
    target: Option<&TargetContext>,
    scope: &crate::make_vars::VarScope,
    states: &[crate::make_vars::ConditionalTruth],
    dirs: &crate::dirs::DirVars,
    root: &Path,
) -> (Vec<ArchSourceDecl>, Vec<String>) {
    use crate::make_vars::ConditionalTruth;

    let dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut declarations = Vec::new();
    let mut rejected = Vec::new();
    let no_legacy_fallback = HashMap::new();

    for (line, body) in crate::includes::directive_bodies_at(content, "%build_archspecific") {
        match states
            .get(line)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                rejected.push(arch_source_rejection(
                    &dir,
                    line,
                    "declaration is guarded by an unresolved Make conditional",
                    target,
                ));
                continue;
            }
            ConditionalTruth::True => {}
        }

        let Some(mainmmake) = crate::includes::arg_value(&body, "mainmmake") else {
            rejected.push(arch_source_rejection(
                &dir,
                line,
                "declaration has no mainmmake= target",
                target,
            ));
            continue;
        };
        let Some(tag) = crate::includes::arg_value(&body, "arch") else {
            rejected.push(arch_source_rejection(
                &dir,
                line,
                &format!("mainmmake={mainmmake} declaration has no arch= tag"),
                target,
            ));
            continue;
        };

        let guard = |name: &str| scope.flavor_uncertainty_reason_at(name, line);
        let expr_context = crate::make_expr::MakeExprContext::new(scope, dirs, line, root, rel_dir)
            .with_guard(&guard)
            .without_filesystem();
        let evaluated =
            match crate::sources::evaluate_macro_sources(&body, &no_legacy_fallback, &expr_context)
            {
                Ok(evaluated) => evaluated,
                Err(error) => {
                    rejected.push(arch_source_rejection(
                        &dir,
                        line,
                        &format!("mainmmake={mainmmake} arch={tag}: {error}"),
                        target,
                    ));
                    continue;
                }
            };

        // evaluate_macro_sources deliberately preserves useful partial
        // diagnostics for other consumers. Architecture overrides change the
        // selected implementation, so accepting one lane while another is
        // unresolved would silently produce a different module.
        if !evaluated.diagnostics.is_empty() || !evaluated.deferred_wildcards.is_empty() {
            let mut details = evaluated.diagnostics;
            details.extend(
                evaluated
                    .deferred_wildcards
                    .into_iter()
                    .map(|pattern| format!("deferred source wildcard `{pattern}`")),
            );
            rejected.push(arch_source_rejection(
                &dir,
                line,
                &format!(
                    "mainmmake={mainmmake} arch={tag}: incomplete source declaration: {}",
                    details.join("; ")
                ),
                target,
            ));
            continue;
        }

        // Keep the same stable lane order as the legacy collector while
        // evaluating each lane with its correct Make and language semantics.
        let mut files = Vec::new();
        for lane in [evaluated.c, evaluated.cxx, evaluated.asm, evaluated.objc] {
            for file in lane {
                if !files.contains(&file) {
                    files.push(file);
                }
            }
        }
        if files.is_empty() {
            rejected.push(arch_source_rejection(
                &dir,
                line,
                &format!("mainmmake={mainmmake} arch={tag}: source list resolved empty"),
                target,
            ));
            continue;
        }

        let maindir = match resolve_arch_source_scalar(&body, "maindir", &expr_context) {
            Ok(value) => value,
            Err(error) => {
                rejected.push(arch_source_rejection(
                    &dir,
                    line,
                    &format!("mainmmake={mainmmake} arch={tag}: {error}"),
                    target,
                ));
                continue;
            }
        };
        let modname = match resolve_arch_source_scalar(&body, "modname", &expr_context) {
            Ok(value) => value,
            Err(error) => {
                rejected.push(arch_source_rejection(
                    &dir,
                    line,
                    &format!("mainmmake={mainmmake} arch={tag}: {error}"),
                    target,
                ));
                continue;
            }
        };

        declarations.push(ArchSourceDecl {
            mainmmake,
            maindir,
            modname,
            tag,
            dir: dir.clone(),
            files,
            extra_quote_include: crate::includes::arg_value_quoted(&body, "incextra")
                .or_else(|| crate::includes::arg_value(&body, "incextra")),
            // The parser pipeline binds these from the very same declaration
            // position after this complete source declaration is accepted.
            include_dirs: Vec::new(),
            defines: Vec::new(),
            compile_options: Vec::new(),
            line,
        });
    }

    (declarations, rejected)
}

fn resolve_arch_source_scalar(
    body: &str,
    key: &str,
    context: &crate::make_expr::MakeExprContext<'_>,
) -> std::result::Result<Option<String>, String> {
    let Some(raw) = crate::parser::macro_arg(body, key) else {
        return Ok(None);
    };
    let value = crate::make_expr::evaluate_make_expr(&raw, context)
        .map_err(|error| format!("cannot resolve {key}={raw}: {error}"))?;
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.contains('$') || value.contains(char::is_whitespace) {
        return Err(format!(
            "{key}={raw} did not resolve to one literal path component"
        ));
    }
    Ok(Some(value.to_owned()))
}

fn arch_source_rejection(
    dir: &str,
    line: usize,
    reason: &str,
    target: Option<&TargetContext>,
) -> String {
    let contract = if target.is_some() {
        "selected native source contract"
    } else {
        "explicit source configuration"
    };
    format!(
        "{dir}/mmakefile.src:{}: %build_archspecific was rejected: {reason}; fix the {contract} by binding every selector that affects this declaration explicitly (including known-empty values)",
        line + 1
    )
}

/// Bind architecture flags and paths where the declaration stands, not from
/// the final assignments in a multi-lane file. The input must be the same
/// joined/inlined text used to construct `scope` and the declaration lines.
pub(crate) fn bind_declaration_context(
    declarations: &mut [ArchSourceDecl],
    joined: &str,
    scope: &crate::make_vars::VarScope,
    rel_dir: &Path,
) -> aros_common::Result<()> {
    for declaration in declarations {
        let includes =
            crate::includes::collect_includes_at(joined, scope, declaration.line, rel_dir);
        let flags = crate::flags::collect_flags_at(scope, declaration.line);
        declaration.include_dirs = includes.dirs;
        declaration.defines = flags.defines;
        declaration.compile_options = flags.compile_options;
        if let Some(raw) = declaration.extra_quote_include.take() {
            declaration.extra_quote_include = crate::includes::resolve_include_argument_at(
                &raw,
                scope,
                declaration.line,
                rel_dir,
            )
            .map_err(|argument| aros_common::ArosError::Configuration {
                file: format!(
                    "{}/mmakefile.src:{}",
                    rel_dir.display(),
                    declaration.line + 1
                ),
                message: format!("cannot resolve %build_archspecific incextra={argument:?}"),
            })?;
            if let Some(path) = &declaration.extra_quote_include {
                declaration
                    .compile_options
                    .insert(0, format!("-iquote{path}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const EXEC_X86_64: &str = r"
FILES  := \
        alert_cpu \
        copymem \
        newstackswap \
        preparecontext

AFILES := \
        execstubs \
        stackswap \
        taskexit

%build_archspecific \
  mainmmake=kernel-exec maindir=rom/exec \
  asmfiles=$(AFILES) files=$(FILES) \
  arch=x86_64 modname=exec
";

    #[test]
    fn each_gfx_lane_gets_its_own_codegen_flags() {
        // One mmakefile, three lanes, three different USER_CFLAGS. Read
        // file-wide instead of at the declaration, the SSE lane would get the
        // AVX flag or none, and rgbconv_avx.c does not compile without -mavx2.
        let root = crate::testing::root();
        let rel = PathBuf::from("arch/i386-all/hidd/gfx");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = crate::parser::join_continuations(&content);
        let scope = crate::make_vars::collect_vars(&joined);
        let (decls, _) = collect_arch_sources(&joined, &rel, None);

        let mut seen: Vec<(String, Vec<String>, Vec<String>)> = decls
            .iter()
            .map(|decl| {
                let flags = crate::flags::collect_flags_at(&scope, decl.line);
                (decl.tag.clone(), decl.files.clone(), flags.compile_options)
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            [
                (
                    "i386".to_owned(),
                    vec!["rgbconv_arch".to_owned()],
                    Vec::<String>::new()
                ),
                (
                    "x86_avx".to_owned(),
                    vec!["rgbconv_avx".to_owned()],
                    vec!["-mavx2".to_owned()]
                ),
                (
                    "x86_sse".to_owned(),
                    vec!["rgbconv_sse".to_owned()],
                    vec!["-msse2".to_owned()]
                ),
            ]
        );
    }

    #[test]
    fn parses_the_exec_declaration() {
        let (decls, skipped) =
            collect_arch_sources(EXEC_X86_64, &PathBuf::from("arch/x86_64-all/exec"), None);
        assert!(skipped.is_empty());
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert_eq!(d.mainmmake, "kernel-exec");
        assert_eq!(d.tag, "x86_64");
        assert_eq!(d.dir, "arch/x86_64-all/exec");
        // C and assembly sources are merged: the override is by base name, and
        // the extension is resolved when the sources are looked up.
        assert_eq!(
            d.files,
            vec![
                "alert_cpu",
                "copymem",
                "newstackswap",
                "preparecontext",
                "execstubs",
                "stackswap",
                "taskexit"
            ]
        );
    }

    #[test]
    fn stackswap_is_present_so_the_generic_stub_can_be_dropped() {
        let (decls, _) =
            collect_arch_sources(EXEC_X86_64, &PathBuf::from("arch/x86_64-all/exec"), None);
        assert!(decls[0].files.contains(&"stackswap".to_owned()));
    }

    #[test]
    fn inline_file_list_without_variables() {
        let src = "%build_archspecific mainmmake=kernel-kernel arch=pc files=\"a b c\" maindir=rom/kernel\n";
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/kernel"), None);
        assert_eq!(decls[0].files, vec!["a", "b", "c"]);
        assert_eq!(decls[0].tag, "pc");
    }

    #[test]
    fn linklibfiles_are_not_overrides() {
        // linklibfiles land in linklib/arch, which the module's ARCHOBJS
        // wildcard does not cover.
        let src =
            "%build_archspecific mainmmake=m arch=pc linklibfiles=\"x\" files=\"y\" maindir=d\n";
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/d"), None);
        assert_eq!(decls[0].files, vec!["y"]);
    }

    #[test]
    fn declaration_without_resolvable_files_is_reported() {
        let src = "%build_archspecific mainmmake=m arch=pc files=$(UNKNOWN) maindir=d\n";
        let (decls, skipped) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/d"), None);
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("mainmmake=m"));
    }

    #[test]
    fn multiline_directive_is_joined() {
        let src = "\
FILES := one two
%build_archspecific \\
  mainmmake=kernel-x \\
  arch=pc \\
  files=$(FILES) \\
  maindir=rom/x
";
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/x"), None);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].files, vec!["one", "two"]);
    }

    fn collect_scoped(
        content: &str,
        target: Option<&TargetContext>,
    ) -> (Vec<ArchSourceDecl>, Vec<String>) {
        let rel_dir = PathBuf::from("arch/test/kernel");
        let joined = crate::parser::join_continuations(content);
        let (scope, states) = crate::make_vars::collect_vars_impl(&joined, target);
        let root = Path::new("/");
        let dirs = crate::dirs::DirVars::load(Path::new("/no-arch-source-config-for-test"));
        collect_arch_sources_with_scope(&joined, &rel_dir, target, &scope, &states, &dirs, root)
    }

    #[test]
    fn scoped_sources_use_known_empty_selector_and_make_conditional_stack() {
        let content = "\
FILES := baseline
ifeq ($(FEATURE),1)
  ifeq ($(UNBOUND_NESTED),yes)
    FILES += nested-wrong
  endif
  FILES += feature-only
else ifeq ($(FEATURE),0)
  FILES += zero-only
else
endif
define UNUSED_TEMPLATE
FILES += from-define-body
endef
%build_archspecific mainmmake=kernel-kernel arch=test files=$(FILES) maindir=rom/kernel modname=kernel
";
        let mut target = TargetContext::default();
        // Presence with an empty value is a proven Make value, not an omitted
        // target selector. The false branch must not add its optional source.
        target
            .make_variables
            .insert("FEATURE".into(), String::new());
        let (declarations, rejected) = collect_scoped(content, Some(&target));

        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].files, vec!["baseline".to_owned()]);
        assert_eq!(declarations[0].maindir.as_deref(), Some("rom/kernel"));
        assert_eq!(declarations[0].modname.as_deref(), Some("kernel"));
    }

    #[test]
    fn scoped_sources_reject_the_whole_declaration_for_an_unbound_feature() {
        let content = "\
FILES := baseline
ifeq ($(OPTIONAL_RUNTIME),1)
FILES += optional-runtime
endif
%build_archspecific mainmmake=kernel-kernel arch=test files=$(FILES) cxxfiles=certain-cxx
";
        let (declarations, rejected) = collect_scoped(content, Some(&TargetContext::default()));

        assert!(declarations.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(
            rejected[0].contains("unevaluated Make conditional"),
            "{rejected:?}"
        );
        assert!(rejected[0].contains("selected native source contract"));
        assert!(rejected[0].contains("binding every selector"));
    }

    #[test]
    fn scoped_sources_preserve_position_and_evaluate_all_language_lanes() {
        let content = "\
BASE := first second
CXXFILES := before
OBJCFILES := objective
AFILES := assembler
%build_archspecific mainmmake=kernel-kernel arch=test files=\"$(addsuffix .c,$(BASE))\" cxxfiles=$(CXXFILES) objcfiles=$(OBJCFILES) asmfiles=$(AFILES)
CXXFILES := after
%build_archspecific mainmmake=kernel-kernel arch=test files=$(BASE) cxxfiles=$(CXXFILES)
";
        let (declarations, rejected) = collect_scoped(content, None);

        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(declarations.len(), 2);
        assert_eq!(
            declarations[0].files,
            vec![
                "first.c".to_owned(),
                "second.c".to_owned(),
                "before".to_owned(),
                "assembler".to_owned(),
                "objective".to_owned()
            ]
        );
        assert_eq!(
            declarations[1].files,
            vec!["first".to_owned(), "second".to_owned(), "after".to_owned()]
        );
    }
}
