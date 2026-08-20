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

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One `%build_archspecific` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchSourceDecl {
    /// The `mainmmake=` target these sources belong to.
    pub mainmmake: String,
    /// `arch=`: the architecture tag this declaration applies to.
    pub tag: String,
    /// Directory holding the sources, relative to the source root.
    pub dir: String,
    /// Source base names. Extensions are resolved by the build, so a `.S` file
    /// overriding a `.c` file needs no special handling here.
    pub files: Vec<String>,
}

/// Collects `VAR := / = / += value` file lists, keeping plain names only.
fn collect_file_vars(content: &str) -> HashMap<String, Vec<String>> {
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

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
        .filter(|t| {
            !t.is_empty() && !t.contains('$') && !t.contains('(') && !t.contains('/')
        })
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
) -> (Vec<ArchSourceDecl>, Vec<String>) {
    let vars = collect_file_vars(content);
    let dir = rel_dir.to_string_lossy().replace('\\', "/");
    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for body in crate::includes::directive_bodies_pub(content, "%build_archspecific") {
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
            tag,
            dir: dir.clone(),
            files,
        });
    }

    (out, skipped)
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
    fn parses_the_exec_declaration() {
        let (decls, skipped) =
            collect_arch_sources(EXEC_X86_64, &PathBuf::from("arch/x86_64-all/exec"));
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
            collect_arch_sources(EXEC_X86_64, &PathBuf::from("arch/x86_64-all/exec"));
        assert!(decls[0].files.contains(&"stackswap".to_owned()));
    }

    #[test]
    fn inline_file_list_without_variables() {
        let src = "%build_archspecific mainmmake=kernel-kernel arch=pc files=\"a b c\" maindir=rom/kernel\n";
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/kernel"));
        assert_eq!(decls[0].files, vec!["a", "b", "c"]);
        assert_eq!(decls[0].tag, "pc");
    }

    #[test]
    fn linklibfiles_are_not_overrides() {
        // linklibfiles land in linklib/arch, which the module's ARCHOBJS
        // wildcard does not cover.
        let src = "%build_archspecific mainmmake=m arch=pc linklibfiles=\"x\" files=\"y\" maindir=d\n";
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/d"));
        assert_eq!(decls[0].files, vec!["y"]);
    }

    #[test]
    fn declaration_without_resolvable_files_is_reported() {
        let src = "%build_archspecific mainmmake=m arch=pc files=$(UNKNOWN) maindir=d\n";
        let (decls, skipped) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/d"));
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
        let (decls, _) = collect_arch_sources(src, &PathBuf::from("arch/all-pc/x"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].files, vec!["one", "two"]);
    }
}
