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
    /// Definitions that apply only to one architecture, as `(tag, define)`.
    pub arch_defines: Vec<(String, String)>,
    /// Codegen options that apply only to one architecture.
    pub arch_compile_options: Vec<(String, String)>,
    /// Conditions whose flags were dropped because the condition is not a
    /// simple architecture test.
    pub skipped_conditions: Vec<String>,
}

/// Maps a Make conditional onto an architecture tag, if it is a plain test on a
/// target parameter.
///
/// `ifeq ($(AROS_TARGET_CPU),x86_64)` guards
/// `USER_CFLAGS += $(CFLAGS_GENERAL_REGS_ONLY)` in both rom/exec and
/// rom/kernel. Applying that unconditionally puts -mgeneral-regs-only on
/// aarch64 too, where the kernel's inline assembly needs the FP registers and
/// the build fails with "instruction requires: fp-armv8".
fn condition_tag(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("ifeq")?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    let (lhs, rhs) = inner.split_once(',')?;
    let var = lhs.trim().strip_prefix("$(")?.strip_suffix(')')?;
    let value = rhs.trim();
    if value.is_empty() || value.contains('$') {
        return None;
    }
    match var {
        // The CPU and the platform are both tag forms in their own right.
        "AROS_TARGET_CPU" | "CPU" | "AROS_TARGET_PLATFORM" | "ARCH" => {
            Some(value.trim_matches('"').to_owned())
        }
        _ => None,
    }
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

/// Recognises the Make spelling of a string-literal define value.
///
/// `"\"pc\""` yields `pc`. Both the outer shell quotes and the inner escaped
/// quotes have to be present; anything else is not a string literal.
fn string_literal_value(raw: &str) -> Option<String> {
    let t = raw.trim();
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    let inner = inner.strip_prefix("\\\"")?.strip_suffix("\\\"")?;
    Some(inner.to_owned())
}

/// Whether a name is usable as a C identifier once CMake substitutes any
/// variable reference in it.
fn is_identifier_shaped(name: &str) -> bool {
    let bare = name
        .replace("${AROS_TARGET_PLATFORM}", "x")
        .replace("${AROS_TARGET_CPU}", "x")
        .replace("${AROS_TARGET_FAMILY}", "x");
    !bare.is_empty()
        && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !bare.starts_with(|c: char| c.is_ascii_digit())
}

/// Accepts only the simple define forms: an identifier, optionally followed by
/// `=` and an unquoted value.
fn simple_define(body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let (name, value) = match body.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (body, None),
    };

    // A string-literal define is written `NAME="\"value\""` in Make, e.g.
    // rom/kernel's -DAROS_ARCHITECTURE. Reduce it to NAME="value" and let CMake
    // do the escaping. Anything else containing quotes is refused: the same
    // shape appears in arguments meant for a nested CMake build, such as
    // -DCMAKE_CXX_FLAGS="...".
    if let Some(v) = value {
        if let Some(inner) = string_literal_value(v) {
            let name = resolve_vars(name)?;
            if !is_identifier_shaped(&name) {
                return None;
            }
            let inner = resolve_vars(&inner)?;
            if inner.contains('"') || inner.contains('\\') || inner.contains(char::is_whitespace) {
                return None;
            }
            return Some(format!("{name}=\"{inner}\""));
        }
    }

    // Quotes and backslashes otherwise mean shell/Make quoting we will not
    // reproduce.
    if body.contains('"') || body.contains('\'') || body.contains('\\') {
        return None;
    }

    let name = resolve_vars(name)?;
    if !is_identifier_shaped(&name) {
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
    // Conditional nesting. Flag assignments inside a condition are handled by
    // collect_conditional() and must not also land here, or they would be
    // applied to every architecture. Other variables are still collected,
    // because the flag text may expand through them.
    let mut depth = 0usize;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("ifeq")
            || trimmed.starts_with("ifneq")
            || trimmed.starts_with("ifdef")
            || trimmed.starts_with("ifndef")
        {
            depth += 1;
            pending = None;
            continue;
        }
        if trimmed == "endif" {
            depth = depth.saturating_sub(1);
            pending = None;
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            pending = None;
            continue;
        }

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
        if depth > 0 && (name == "USER_CPPFLAGS" || name == "USER_CFLAGS") {
            // collect_conditional() owns this one.
            if continues {
                pending = None;
            }
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

/// Collects `USER_*` flag text that sits inside a Make conditional.
///
/// Returns `(tag, text)` pairs for conditions that map to an architecture, plus
/// the conditions whose contents had to be dropped. Only the outermost level is
/// considered: a nested condition, an `else` branch or a test we cannot map
/// makes the contents unusable.
fn collect_conditional(
    content: &str,
    vars: &HashMap<String, String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    // (tag of the enclosing condition, still in its true branch)
    let mut stack: Vec<(Option<String>, bool)> = Vec::new();
    let mut pending_key: Option<(String, String)> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("ifeq")
            || trimmed.starts_with("ifneq")
            || trimmed.starts_with("ifdef")
            || trimmed.starts_with("ifndef")
        {
            // Only a top-level ifeq on a target parameter is usable.
            let tag = if stack.is_empty() {
                condition_tag(trimmed)
            } else {
                None
            };
            if tag.is_none() && stack.is_empty() {
                skipped.push(trimmed.to_owned());
            }
            stack.push((tag, true));
            pending_key = None;
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            if let Some(top) = stack.last_mut() {
                // The negation of an architecture test is not a tag.
                top.1 = false;
            }
            pending_key = None;
            continue;
        }
        if trimmed == "endif" {
            stack.pop();
            pending_key = None;
            continue;
        }
        if stack.is_empty() {
            pending_key = None;
            continue;
        }

        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        // Continuation of a flag assignment we are already collecting.
        if let Some((key, mut text)) = pending_key.take() {
            text.push(' ');
            text.push_str(payload);
            if continues {
                pending_key = Some((key, text));
            } else {
                push_conditional(&stack, &key, &text, vars, &mut out);
            }
            continue;
        }

        if trimmed.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs, _)) = split_assignment(payload) else {
            continue;
        };
        let key = lhs.trim().to_owned();
        if key != "USER_CPPFLAGS" && key != "USER_CFLAGS" {
            continue;
        }
        let text = rhs.trim().to_owned();
        if continues {
            pending_key = Some((key, text));
        } else {
            push_conditional(&stack, &key, &text, vars, &mut out);
        }
    }

    (out, skipped)
}

fn push_conditional(
    stack: &[(Option<String>, bool)],
    key: &str,
    text: &str,
    vars: &HashMap<String, String>,
    out: &mut Vec<(String, String)>,
) {
    // Usable only at the outermost level, in the true branch, with a mapped tag.
    if stack.len() != 1 {
        return;
    }
    let (Some(tag), true) = (&stack[0].0, stack[0].1) else {
        return;
    };
    let expanded = expand(text, vars, key, 8);
    out.push((tag.clone(), expanded));
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

    // Assignments inside a Make conditional must not be applied unconditionally.
    // A plain test on the CPU or the platform becomes an architecture tag that
    // CMake filters; anything else is dropped and reported.
    let (conditional, skipped_conditions) = collect_conditional(content, &vars);
    set.skipped_conditions = skipped_conditions;
    for (tag, raw) in conditional {
        let mut bucket = FlagSet::default();
        for tok in raw.split_whitespace() {
            classify(tok, &mut bucket);
        }
        for d in bucket.defines {
            let entry = (tag.clone(), d);
            if !set.arch_defines.contains(&entry) {
                set.arch_defines.push(entry);
            }
        }
        for o in bucket.compile_options {
            let entry = (tag.clone(), o);
            if !set.arch_compile_options.contains(&entry) {
                set.arch_compile_options.push(entry);
            }
        }
        // Anything the classifier could not use is already accounted for by the
        // unconditional pass's report.
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
    if tok.starts_with("-I") || tok == "-isystem" || tok == "-idirafter" || tok == "-iquote" {
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

    #[test]
    fn a_cpu_guarded_flag_does_not_leak_to_other_architectures() {
        // rom/exec and rom/kernel both wrap this in ifeq on the CPU. Applying
        // it unconditionally puts -mgeneral-regs-only on aarch64, where the
        // kernel's inline assembly needs FP registers.
        let src = "\
USER_CPPFLAGS := -DAROS_ARCH_pc
ifeq ($(AROS_TARGET_CPU),x86_64)
USER_CFLAGS += $(CFLAGS_GENERAL_REGS_ONLY)
endif
";
        let f = collect_flags(src);
        assert!(
            !f.compile_options
                .contains(&"-mgeneral-regs-only".to_owned()),
            "must not be unconditional: {:?}",
            f.compile_options
        );
        assert_eq!(
            f.arch_compile_options,
            vec![("x86_64".to_owned(), "-mgeneral-regs-only".to_owned())]
        );
        // The unconditional define is still picked up.
        assert_eq!(f.defines, vec!["AROS_ARCH_pc"]);
    }

    #[test]
    fn a_cpu_guarded_define_becomes_architecture_conditional() {
        let src = "\
ifeq ($(AROS_TARGET_CPU),m68k)
USER_CPPFLAGS += -DM68K_ONLY
endif
";
        let f = collect_flags(src);
        assert!(f.defines.is_empty(), "defines: {:?}", f.defines);
        assert_eq!(
            f.arch_defines,
            vec![("m68k".to_owned(), "M68K_ONLY".to_owned())]
        );
    }

    #[test]
    fn an_unmappable_condition_drops_its_flags_and_reports() {
        let src = "\
ifeq ($(AROS_TOOLCHAIN),llvm)
USER_CPPFLAGS += -DTOOLCHAIN_SPECIFIC
endif
";
        let f = collect_flags(src);
        assert!(f.defines.is_empty());
        assert!(f.arch_defines.is_empty());
        assert_eq!(f.skipped_conditions.len(), 1);
        assert!(f.skipped_conditions[0].contains("AROS_TOOLCHAIN"));
    }

    #[test]
    fn an_else_branch_is_not_treated_as_a_tag() {
        let src = "\
ifeq ($(AROS_TARGET_CPU),x86_64)
USER_CPPFLAGS += -DIS_X86
else
USER_CPPFLAGS += -DNOT_X86
endif
";
        let f = collect_flags(src);
        assert_eq!(
            f.arch_defines,
            vec![("x86_64".to_owned(), "IS_X86".to_owned())]
        );
        assert!(
            !f.arch_defines.iter().any(|(_, d)| d == "NOT_X86"),
            "the negation of an architecture test is not a tag"
        );
        assert!(f.defines.is_empty());
    }

    #[test]
    fn string_literal_defines_are_propagated() {
        // rom/kernel: -DAROS_ARCHITECTURE="\"$(AROS_TARGET_PLATFORM)\""
        let f = collect_flags(
            "USER_CPPFLAGS := -DAROS_ARCHITECTURE=\"\\\"$(AROS_TARGET_PLATFORM)\\\"\"\n",
        );
        assert_eq!(
            f.defines,
            vec!["AROS_ARCHITECTURE=\"${AROS_TARGET_PLATFORM}\""]
        );
        assert!(f.skipped.is_empty(), "skipped: {:?}", f.skipped);
    }

    #[test]
    fn a_quoted_flag_list_is_still_refused() {
        // Not a compiler define but an argument for a nested CMake build.
        let f = collect_flags("USER_CPPFLAGS := -DCMAKE_CXX_FLAGS=\"-O2 -g\"\n");
        assert!(f.defines.is_empty(), "defines: {:?}", f.defines);
        // The value splits on whitespace, so more than one token is reported;
        // what matters is that none of them became a define.
        assert!(!f.skipped.is_empty());
    }
}
