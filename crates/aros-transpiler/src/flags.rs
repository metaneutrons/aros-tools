//! Preprocessor and codegen flag propagation from `mmakefile.src` to CMake.
//!
//! The historic build folds `USER_CPPFLAGS` and `USER_CFLAGS` into the
//! `CPPFLAGS`/`CFLAGS` handed to every compile (see `config/make.tmpl`, the
//! `%compile_q` definition). Modules rely on this for semantics, not just for
//! tuning: `rom/devs/ahci` sets `-D__OOP_NOMETHODBASES__`, and its library base
//! struct declares the method-base fields only inside
//! `#if defined(__OOP_NOMETHODBASES__)`. Without the define, every use of those
//! fields fails to compile.
//!
//! Scope is deliberately narrow. Flags can change code generation, so only two
//! classes are propagated:
//!
//! * `-D` / `-U` in their simple forms, i.e. a plain identifier with an
//!   optional unquoted value. These carry meaning the source cannot do without.
//! * A small allowlist of codegen flags reached through well-known Make
//!   variables, currently `$(CFLAGS_GENERAL_REGS_ONLY)`, which `rom/exec` needs
//!   on x86_64 so interrupt paths avoid SSE registers.
//!
//! Everything else is skipped and reported: warning-suppression bundles
//! (`$(NOWARN_FLAGS)`, `$(PARANOIA_CFLAGS)`) do not affect whether code
//! compiles and hiding warnings during a bring-up is the wrong trade; defines
//! built with `$(shell ...)` would make output non-reproducible; and quoted
//! values are frequently not compiler defines at all but arguments for a nested
//! CMake build.
//!
//! Assignments are read file-globally, with a later `:=` replacing an earlier
//! one and `+=` appending. That matches Make for the common layout, where the
//! flags are set once above the `%build_module` line. A file that reassigns the
//! flags *and* builds several modules could disagree, so those are reported.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Flags collected from one `mmakefile.src`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlagSet {
    /// Preprocessor definitions without the `-D`, e.g. `FOO` or `BAR=1`.
    pub defines: Vec<String>,
    /// Names to undefine, without the `-U`.
    pub undefines: Vec<String>,
    /// Allowlisted codegen options, verbatim.
    pub compile_options: Vec<String>,
    /// Flag tokens that were not propagated, for reporting.
    pub skipped: Vec<String>,
    /// True when the file reassigns the flags and also builds more than one
    /// module, so the file-global reading may not match Make.
    pub ambiguous: bool,
}

/// Make variables usable inside a define name or value.
fn map_var(name: &str) -> Option<&'static str> {
    match name {
        // AROS calls the platform ARCH and the processor CPU.
        "ARCH" | "AROS_TARGET_PLATFORM" => Some("${AROS_TARGET_PLATFORM}"),
        "CPU" | "AROS_TARGET_CPU" => Some("${AROS_TARGET_CPU}"),
        "FAMILY" => Some("${AROS_TARGET_FAMILY}"),
        _ => None,
    }
}

/// Make variables that expand to a codegen flag we are willing to pass on.
fn map_flag_var(name: &str) -> Option<&'static str> {
    match name {
        // rom/exec needs this on x86_64: interrupt-context code must not touch
        // SSE registers.
        "CFLAGS_GENERAL_REGS_ONLY" => Some("-mgeneral-regs-only"),
        _ => None,
    }
}

/// Resolves `$(VAR)` occurrences in a define name or value.
///
/// Returns `None` if any variable is not one we map, so the caller can skip the
/// token rather than emit a half-substituted define.
fn resolve_vars(text: &str) -> Option<String> {
    if !text.contains("$(") {
        return Some(text.to_owned());
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];
        out.push_str(map_var(name)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Accepts only the simple define forms: an identifier, optionally followed by
/// `=` and an unquoted value.
fn simple_define(body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    // Quotes and backslashes mean shell/Make quoting we will not reproduce.
    if body.contains('"') || body.contains('\'') || body.contains('\\') {
        return None;
    }
    let (name, value) = match body.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (body, None),
    };

    let name = resolve_vars(name)?;
    // The name must be a C identifier once variables are substituted; a CMake
    // reference is allowed because it expands to one.
    let bare = name.replace("${AROS_TARGET_PLATFORM}", "x")
        .replace("${AROS_TARGET_CPU}", "x")
        .replace("${AROS_TARGET_FAMILY}", "x");
    if bare.is_empty()
        || !bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        || bare.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }

    match value {
        None => Some(name),
        Some(v) => {
            let v = resolve_vars(v)?;
            // Keep values simple: identifiers, numbers, dots.
            let probe = v
                .replace("${AROS_TARGET_PLATFORM}", "x")
                .replace("${AROS_TARGET_CPU}", "x")
                .replace("${AROS_TARGET_FAMILY}", "x");
            if probe.is_empty()
                || !probe
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
            {
                return None;
            }
            Some(format!("{name}={v}"))
        }
    }
}

/// Collects `VAR := / = / += value` assignments, keeping raw right-hand sides.
fn collect_raw(content: &str) -> (HashMap<String, String>, HashMap<String, usize>) {
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut assign_count: HashMap<String, usize> = HashMap::new();
    let mut pending: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        if let Some(name) = pending.take() {
            let entry = vars.entry(name.clone()).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(payload);
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
        *assign_count.entry(name.clone()).or_default() += 1;

        let entry = vars.entry(name.clone()).or_default();
        if append {
            if !entry.is_empty() {
                entry.push(' ');
            }
        } else {
            entry.clear();
        }
        entry.push_str(rhs.trim());
        if continues {
            pending = Some(name);
        }
    }

    (vars, assign_count)
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

/// Expands `$(VAR)` references against `vars`, one level at a time.
fn expand(raw: &str, vars: &HashMap<String, String>, self_name: &str, depth: usize) -> String {
    if depth == 0 || !raw.contains("$(") {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let verbatim = &rest[start..=start + 2 + end];
        if name == self_name || name.starts_with("shell") || name.contains(' ') {
            // Self-reference from `+=`, or a shell call we refuse to run.
            out.push_str(verbatim);
        } else if map_var(name).is_some() || map_flag_var(name).is_some() {
            out.push_str(verbatim);
        } else if let Some(v) = vars.get(name) {
            out.push_str(&expand(v, vars, self_name, depth - 1));
        } else {
            out.push_str(verbatim);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Collects the flags one `mmakefile.src` contributes.
#[must_use]
pub fn collect_flags(content: &str) -> FlagSet {
    let (vars, counts) = collect_raw(content);
    let mut set = FlagSet::default();

    let reassigned = ["USER_CPPFLAGS", "USER_CFLAGS"]
        .iter()
        .any(|k| counts.get(*k).copied().unwrap_or(0) > 1);
    let modules = content.matches("%build_module").count();
    set.ambiguous = reassigned && modules > 1;

    for key in ["USER_CPPFLAGS", "USER_CFLAGS"] {
        let Some(raw) = vars.get(key) else { continue };
        let expanded = expand(raw, &vars, key, 8);
        for tok in expanded.split_whitespace() {
            classify(tok, &mut set);
        }
    }

    set.defines.dedup();
    set.undefines.dedup();
    set.compile_options.dedup();
    set.skipped.dedup();
    set
}

fn classify(tok: &str, set: &mut FlagSet) {
    // A bare variable reference: only an allowlisted codegen flag survives.
    if let Some(name) = tok.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
        if let Some(flag) = map_flag_var(name) {
            push_unique(&mut set.compile_options, flag.to_owned());
        } else {
            push_unique(&mut set.skipped, tok.to_owned());
        }
        return;
    }

    if let Some(body) = tok.strip_prefix("-D") {
        match simple_define(body) {
            Some(d) => push_unique(&mut set.defines, d),
            None => push_unique(&mut set.skipped, tok.to_owned()),
        }
        return;
    }
    if let Some(body) = tok.strip_prefix("-U") {
        match resolve_vars(body) {
            Some(u) if !u.is_empty() && !u.contains('"') => {
                push_unique(&mut set.undefines, u);
            }
            _ => push_unique(&mut set.skipped, tok.to_owned()),
        }
        return;
    }

    // Include flags are the include collector's business; reporting them here
    // too would be noise, and a report full of noise gets ignored.
    if tok.starts_with("-I")
        || tok == "-isystem"
        || tok == "-idirafter"
        || tok == "-iquote"
    {
        return;
    }

    // Anything else is a compiler option we do not second-guess.
    if tok.starts_with('-') {
        push_unique(&mut set.skipped, tok.to_owned());
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propagates_the_ahci_case() {
        // rom/devs/ahci relies on this define for its library base layout.
        let src = "USER_CPPFLAGS := -D__OOP_NOMETHODBASES__ -D__OOP_NOATTRBASES__\n";
        let f = collect_flags(src);
        assert_eq!(
            f.defines,
            vec!["__OOP_NOMETHODBASES__", "__OOP_NOATTRBASES__"]
        );
        assert!(f.skipped.is_empty());
    }

    #[test]
    fn handles_multiline_and_append() {
        let src = "\
USER_CPPFLAGS := \\
               -DUSE_EXEC_DEBUG \\
               -D__OOP_NOLIBBASE__
USER_CPPFLAGS += -DINTUITION_INLINE_NEWOBJECT
";
        let f = collect_flags(src);
        assert_eq!(
            f.defines,
            vec![
                "USE_EXEC_DEBUG",
                "__OOP_NOLIBBASE__",
                "INTUITION_INLINE_NEWOBJECT"
            ]
        );
    }

    #[test]
    fn keeps_simple_values() {
        let f = collect_flags("USER_CPPFLAGS := -DDEBUG=0 -DAROS_ABI_V1\n");
        assert_eq!(f.defines, vec!["DEBUG=0", "AROS_ABI_V1"]);
    }

    #[test]
    fn substitutes_target_variables() {
        let f = collect_flags("USER_CPPFLAGS := -DHOST_OS_$(ARCH) -DAROS_ARCH_$(CPU)\n");
        assert_eq!(
            f.defines,
            vec![
                "HOST_OS_${AROS_TARGET_PLATFORM}",
                "AROS_ARCH_${AROS_TARGET_CPU}"
            ]
        );
    }

    #[test]
    fn refuses_shell_built_defines() {
        let f = collect_flags(
            "USER_CPPFLAGS := -DADATE=\"\\\"$(shell date '+%d.%m.%Y')\\\"\" -DKEEP=1\n",
        );
        assert_eq!(f.defines, vec!["KEEP=1"]);
        assert!(!f.skipped.is_empty(), "the shell define must be reported");
    }

    #[test]
    fn refuses_quoted_values() {
        let f = collect_flags("USER_CPPFLAGS := -DAROS_ARCHITECTURE=\"pc\" -DOK\n");
        assert_eq!(f.defines, vec!["OK"]);
        assert_eq!(f.skipped.len(), 1);
    }

    #[test]
    fn skips_unknown_variables_but_keeps_allowlisted_codegen_flag() {
        let src = "USER_CFLAGS := $(NOWARN_FLAGS) $(CFLAGS_GENERAL_REGS_ONLY)\n";
        let f = collect_flags(src);
        assert_eq!(f.compile_options, vec!["-mgeneral-regs-only"]);
        assert_eq!(f.skipped, vec!["$(NOWARN_FLAGS)"]);
    }

    #[test]
    fn resolves_local_variables() {
        let src = "\
MY_DEFS := -DONE -DTWO
USER_CPPFLAGS := $(MY_DEFS) -DTHREE
";
        let f = collect_flags(src);
        assert_eq!(f.defines, vec!["ONE", "TWO", "THREE"]);
    }

    #[test]
    fn self_reference_does_not_recurse() {
        let src = "USER_CPPFLAGS := -DA\nUSER_CPPFLAGS := $(USER_CPPFLAGS) -DB\n";
        let f = collect_flags(src);
        assert!(f.defines.contains(&"DB".to_owned()) || f.defines.contains(&"B".to_owned()));
    }

    #[test]
    fn collects_defines_from_user_cflags_too() {
        let f = collect_flags("USER_CFLAGS := -DFROM_CFLAGS -Wno-attributes\n");
        assert_eq!(f.defines, vec!["FROM_CFLAGS"]);
        assert_eq!(f.skipped, vec!["-Wno-attributes"]);
    }

    #[test]
    fn flags_a_file_that_reassigns_and_builds_several_modules() {
        let src = "\
USER_CPPFLAGS := -DA
%build_module mmake=one modname=one modtype=library files=\"a\"
USER_CPPFLAGS := -DB
%build_module mmake=two modname=two modtype=library files=\"b\"
";
        let f = collect_flags(src);
        assert!(
            f.ambiguous,
            "per-module flag differences must be reported, not guessed"
        );
    }

    #[test]
    fn single_module_with_reassignment_is_not_ambiguous() {
        let src = "\
USER_CPPFLAGS := -DA
USER_CPPFLAGS := -DB
%build_module mmake=one modname=one modtype=library files=\"a\"
";
        let f = collect_flags(src);
        assert!(!f.ambiguous);
        // The last := wins, matching Make.
        assert_eq!(f.defines, vec!["B"]);
    }

    #[test]
    fn undefine_is_carried_through() {
        let f = collect_flags("USER_CPPFLAGS := -UNDEBUG\n");
        assert_eq!(f.undefines, vec!["NDEBUG"]);
    }
}
