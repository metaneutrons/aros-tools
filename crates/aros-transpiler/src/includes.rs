//! Include path propagation from `mmakefile.src` to CMake.
//!
//! The historic build feeds `USER_INCLUDES` straight into a target's CFLAGS
//! (see `config/make.tmpl`, `%(mmake)_CFLAGS := $(strip $(CFLAGS) $(USER_INCLUDES))`).
//! Individual modules also legitimately put `-I`-style options in
//! `USER_CPPFLAGS` and `USER_CFLAGS`. Without all three sources, modules such
//! as `exec` cannot find their private and architecture-specific headers.
//!
//! Two mechanisms are covered.
//!
//! 1. Plain `USER_INCLUDES`, `USER_CPPFLAGS`, and `USER_CFLAGS` assignments,
//!    including Make variables defined in the same file
//!    (`PRIV_EXEC_INCLUDES`, `KERNEL_INCLUDES`, ...).
//!
//! 2. The `%set_archincludes` / `%get_archincludes` pair. `%set_archincludes`
//!    is declared in the architecture directory that owns the headers and
//!    records a priority and an architecture tag; `%get_archincludes` pulls
//!    every matching declaration into a `TARGET_<X>_INCLUDES` variable. In the
//!    Make build this happens through generated flag files under `$(GENDIR)`;
//!    here the declarations are collected across the whole tree and resolved
//!    statically, leaving the architecture filter to CMake.
//!
//! Tokens whose Make variables cannot be resolved safely are reported rather
//! than guessed at, so an unresolved path shows up as a missing include instead
//! of a wrong one.

use crate::parser::VarScope;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// An `%set_archincludes` declaration found in an architecture directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchIncludeDecl {
    /// The `mainmmake=` target these includes belong to.
    pub mainmmake: String,
    /// The `modname=` key that `%get_archincludes` joins on.
    pub modname: String,
    /// `pri=`: lower values are read first by the Make build's wildcard glob.
    pub pri: u32,
    /// `arch=`: the architecture tag this declaration applies to.
    pub tag: String,
    /// Include directory, relative to the source root.
    pub dir: String,
}

/// Include information collected from one `mmakefile.src`.
#[derive(Debug, Clone, Default)]
pub struct IncludeSet {
    /// Resolved include directories, ready to be emitted as CMake paths.
    pub dirs: Vec<String>,
    /// `modname` keys whose architecture includes this file asked for via
    /// `%get_archincludes`.
    pub arch_modules: Vec<String>,
    /// Tokens that referenced a Make variable we deliberately do not resolve.
    pub unresolved: Vec<String>,
}

/// Make variables that carry an include path we can map onto a CMake location.
fn map_known_var(name: &str) -> Option<&'static str> {
    match name {
        "SRCDIR" | "TOP" => Some("${CMAKE_SOURCE_DIR}"),
        "GENINCDIR" => Some("${CMAKE_BINARY_DIR}/GENINCDIR"),
        // Third-party sources fetched via %fetch. Modules that build against
        // them reach their headers through variables rooted here, e.g.
        // ACPICASRCDIR := $(PORTSDIR)/acpica/$(ACPICAARCHBASE).
        "PORTSDIR" => Some("${AROS_PORTS_DIR}"),
        "PORTSSOURCEDIR" => Some("${AROS_PORTS_SOURCE_DIR}"),
        // Generated per-module output. BootstrapSDK.cmake puts this under
        // <build>/gen, so the mapping needs that segment: a module reaching
        // its own generated header writes -I$(GENDIR)/$(CURDIR)/<sub>, and
        // without it the path pointed one level too high and the include was
        // simply absent.
        "GENDIR" | "OBJDIR" => Some("${CMAKE_BINARY_DIR}/gen"),
        "AROS_INCLUDES" => Some("${CMAKE_BINARY_DIR}/SDK/include"),
        // Target parameters are passed through as CMake variables so the
        // transpiler stays target-agnostic.
        "CPU" => Some("${AROS_TARGET_CPU}"),
        "ARCH" => Some("${AROS_TARGET_PLATFORM}"),
        "FAMILY" => Some("${AROS_TARGET_FAMILY}"),
        _ => None,
    }
}

/// Variables that hold the result of a `%get_archincludes` call.
fn arch_include_var(name: &str) -> Option<String> {
    // TARGET_EXEC_INCLUDES -> exec, TARGET_KERNEL_INCLUDES -> kernel
    let inner = name.strip_prefix("TARGET_")?.strip_suffix("_INCLUDES")?;
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_lowercase())
}

/// Collects `VAR := value` / `VAR = value` / `VAR += value` assignments,
/// preserving path-like values. This is deliberately separate from the file
/// list variables in `parser.rs`, which strip anything containing `/` or `$`.
fn collect_flag_vars(content: &str) -> HashMap<String, Vec<String>> {
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<(String, bool)> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        if let Some((name, append)) = current.take() {
            let toks: Vec<String> = payload.split_whitespace().map(str::to_owned).collect();
            let entry = vars.entry(name.clone()).or_default();
            if append || !entry.is_empty() {
                entry.extend(toks);
            } else {
                *entry = toks;
            }
            if continues {
                current = Some((name, true));
            }
            continue;
        }

        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }

        // Match `NAME :=`, `NAME +=` or `NAME =` at the start of the line.
        let Some((lhs, rhs, append)) = split_assignment(payload) else {
            continue;
        };
        let name = lhs.trim().to_owned();
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }

        let toks: Vec<String> = rhs.split_whitespace().map(str::to_owned).collect();
        let entry = vars.entry(name.clone()).or_default();
        if append {
            entry.extend(toks);
        } else {
            *entry = toks;
        }
        if continues {
            current = Some((name, true));
        }
    }

    vars
}

/// Splits a Make assignment, returning `(lhs, rhs, is_append)`.
fn split_assignment(line: &str) -> Option<(&str, &str, bool)> {
    if let Some((l, r)) = line.split_once(":=") {
        return Some((l, r, false));
    }
    if let Some((l, r)) = line.split_once("+=") {
        return Some((l, r, true));
    }
    // A plain `=` must not swallow `==` or rule syntax.
    let (l, r) = line.split_once('=')?;
    if l.contains(':') || l.is_empty() {
        return None;
    }
    Some((l, r, false))
}

/// Expands `$(VAR)` references inside a raw token list.
fn expand(
    tokens: &[String],
    vars: &HashMap<String, Vec<String>>,
    depth: usize,
    guard: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    if depth == 0 {
        out.extend(tokens.iter().cloned());
        return;
    }
    for tok in tokens {
        // A token that is exactly one variable reference can be substituted.
        if let Some(name) = tok.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
            if map_known_var(name).is_some() || arch_include_var(name).is_some() {
                out.push(tok.clone());
                continue;
            }
            if guard.iter().any(|g| g == name) {
                continue; // self-reference, e.g. USER_INCLUDES += $(USER_INCLUDES)
            }
            if let Some(v) = vars.get(name) {
                guard.push(name.to_owned());
                expand(v, vars, depth - 1, guard, out);
                guard.pop();
                continue;
            }
        }
        // A variable can also sit inside a longer token, as in
        // `-I$(ACPICASRCDIR)/source/include`. Substitute those in place, so a
        // path rooted in a local variable still resolves.
        out.push(substitute_inline(tok, vars, depth, guard));
    }
}

/// Replaces `$(VAR)` occurrences inside a token using local assignments.
///
/// Variables with a CMake mapping are left alone; `to_cmake_path` handles those
/// and knows where they point. Unknown ones are left verbatim so the caller can
/// report them instead of emitting a half-substituted path.
fn substitute_inline(
    tok: &str,
    vars: &HashMap<String, Vec<String>>,
    depth: usize,
    guard: &mut Vec<String>,
) -> String {
    if depth == 0 || !tok.contains("$(") {
        return tok.to_owned();
    }
    let mut out = String::with_capacity(tok.len() + 32);
    let mut rest = tok;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let verbatim = &rest[start..=start + 2 + end];

        if map_known_var(name).is_some() || arch_include_var(name).is_some() {
            out.push_str(verbatim);
        } else if guard.iter().any(|g| g == name) {
            // Self-reference; drop it rather than recurse.
        } else if let Some(v) = vars.get(name) {
            // A path-valued variable is a single token in practice.
            if v.len() == 1 {
                guard.push(name.to_owned());
                out.push_str(&substitute_inline(&v[0], vars, depth - 1, guard));
                guard.pop();
            } else {
                out.push_str(verbatim);
            }
        } else {
            out.push_str(verbatim);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Converts one `-I`-style path token into a CMake path.
///
/// Returns `Err(token)` when a Make variable in the token is not one we map.
fn to_cmake_path(raw: &str, rel_dir: &Path) -> Result<String, String> {
    let dir = rel_dir.to_string_lossy().replace('\\', "/");

    // Resolve every $(VAR) occurrence; bail out on the first unknown one.
    let mut resolved = String::with_capacity(raw.len() + 32);
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        resolved.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            return Err(raw.to_owned());
        };
        let name = &after[..end];
        if name == "CURDIR" {
            resolved.push_str(&dir);
        } else if let Some(m) = map_known_var(name) {
            resolved.push_str(m);
        } else {
            return Err(raw.to_owned());
        }
        rest = &after[end + 1..];
    }
    resolved.push_str(rest);

    // Normalise: make bare relative paths source-relative.
    let path = if resolved.starts_with("${") || resolved.starts_with('/') {
        resolved
    } else if resolved == "." {
        format!("${{CMAKE_SOURCE_DIR}}/{dir}")
    } else if let Some(stripped) = resolved.strip_prefix("./") {
        format!("${{CMAKE_SOURCE_DIR}}/{dir}/{stripped}")
    } else {
        format!("${{CMAKE_SOURCE_DIR}}/{dir}/{resolved}")
    };

    Ok(collapse_dot_dot(&path))
}

/// Collapses `a/b/../c` into `a/c` so CMake receives tidy paths.
fn collapse_dot_dot(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        if seg == ".." {
            // Never climb past a variable reference or the root.
            if parts
                .last()
                .is_some_and(|p| *p != ".." && !p.starts_with("${") && !p.is_empty())
            {
                parts.pop();
                continue;
            }
        }
        if seg == "." {
            continue;
        }
        parts.push(seg);
    }
    parts.join("/")
}

/// Make variables that can legitimately carry `-I`-style include options.
const INCLUDE_FLAG_VARIABLES: [&str; 3] = ["USER_INCLUDES", "USER_CPPFLAGS", "USER_CFLAGS"];

/// Flags that introduce an include directory as the following token.
const SEPARATE_FLAGS: [&str; 4] = ["-I", "-isystem", "-idirafter", "-iquote"];

/// Extracts include directories from an expanded token list.
fn tokens_to_dirs(tokens: &[String], rel_dir: &Path, set: &mut IncludeSet) {
    let mut expect_path = false;
    for tok in tokens {
        if expect_path {
            expect_path = false;
            push_dir(tok, rel_dir, set);
            continue;
        }
        if SEPARATE_FLAGS.contains(&tok.as_str()) {
            expect_path = true;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("-I") {
            if !rest.is_empty() {
                push_dir(rest, rel_dir, set);
            }
            continue;
        }
        // A bare $(TARGET_X_INCLUDES) reference asks for architecture includes.
        if let Some(name) = tok.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
            if let Some(module) = arch_include_var(name) {
                if !set.arch_modules.contains(&module) {
                    set.arch_modules.push(module);
                }
            }
        }
    }
}

fn push_dir(raw: &str, rel_dir: &Path, set: &mut IncludeSet) {
    match to_cmake_path(raw, rel_dir) {
        Ok(p) => {
            if !set.dirs.contains(&p) {
                set.dirs.push(p);
            }
        }
        Err(tok) => {
            if !set.unresolved.contains(&tok) {
                set.unresolved.push(tok);
            }
        }
    }
}

/// Collects the include directories a single `mmakefile.src` contributes.
#[must_use]
pub fn collect_includes(content: &str, rel_dir: &Path) -> IncludeSet {
    let vars = collect_flag_vars(content);
    let mut set = IncludeSet::default();

    for name in INCLUDE_FLAG_VARIABLES {
        let Some(raw) = vars.get(name) else {
            continue;
        };
        let mut expanded = Vec::new();
        let mut guard = vec![name.to_owned()];
        expand(raw, &vars, 8, &mut guard, &mut expanded);
        tokens_to_dirs(&expanded, rel_dir, &mut set);
    }

    collect_get_archincludes(content, &mut set);
    set
}

/// Collects `-I`-style user flags as they stand at one build declaration.
///
/// This is the include-path counterpart of the parser's positional source
/// scope. It prevents a later reassignment in a multi-target mmakefile from
/// changing an earlier target, and it can see a proven-safe local fragment
/// which the declaration-aware parser selected for a fetched port.
#[must_use]
pub(crate) fn collect_includes_at(
    content: &str,
    scope: &VarScope,
    line: usize,
    rel_dir: &Path,
) -> IncludeSet {
    let mut set = IncludeSet::default();
    for name in INCLUDE_FLAG_VARIABLES {
        if scope.conditionally_assigned_before(name, line) {
            set.unresolved.push(format!("$({name})"));
        } else if let Some(raw) = scope.raw_at(name, line) {
            let mut tokens = Vec::new();
            let mut guard = vec![name.to_owned()];
            expand_scoped_tokens(&raw, scope, line, 8, &mut guard, &mut tokens);
            tokens_to_dirs(&tokens, rel_dir, &mut set);
        }
    }
    collect_get_archincludes(content, &mut set);
    set
}

fn expand_scoped_tokens(
    raw: &str,
    scope: &VarScope,
    line: usize,
    depth: usize,
    guard: &mut Vec<String>,
    output: &mut Vec<String>,
) {
    if depth == 0 {
        output.extend(raw.split_whitespace().map(str::to_owned));
        return;
    }
    for token in raw.split_whitespace() {
        if let Some(name) = token
            .strip_prefix("$(")
            .and_then(|value| value.strip_suffix(')'))
        {
            if map_known_var(name).is_some() || arch_include_var(name).is_some() {
                output.push(token.to_owned());
                continue;
            }
            if !guard.iter().any(|item| item == name)
                && !scope.conditionally_assigned_before(name, line)
            {
                if let Some(value) = scope.raw_at(name, line) {
                    guard.push(name.to_owned());
                    expand_scoped_tokens(&value, scope, line, depth - 1, guard, output);
                    guard.pop();
                    continue;
                }
            }
        }
        output.push(substitute_inline_scoped(token, scope, line, depth, guard));
    }
}

fn substitute_inline_scoped(
    token: &str,
    scope: &VarScope,
    line: usize,
    depth: usize,
    guard: &mut Vec<String>,
) -> String {
    if depth == 0 || !token.contains("$(") {
        return token.to_owned();
    }
    let mut output = String::with_capacity(token.len() + 32);
    let mut rest = token;
    while let Some(start) = rest.find("$(") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let name = &after[..end];
        let verbatim = &rest[start..=start + 2 + end];
        if map_known_var(name).is_some()
            || arch_include_var(name).is_some()
            || guard.iter().any(|item| item == name)
            || scope.conditionally_assigned_before(name, line)
        {
            output.push_str(verbatim);
        } else if let Some(value) = scope.raw_at(name, line) {
            // An inline replacement must be one path token. A multi-token
            // value cannot be embedded safely inside another token.
            if value.split_whitespace().count() == 1 {
                guard.push(name.to_owned());
                output.push_str(&substitute_inline_scoped(
                    &value,
                    scope,
                    line,
                    depth - 1,
                    guard,
                ));
                guard.pop();
            } else {
                output.push_str(verbatim);
            }
        } else {
            output.push_str(verbatim);
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    output
}

/// Records `%get_archincludes modname=<x>` requests.
fn collect_get_archincludes(content: &str, set: &mut IncludeSet) {
    for raw in directive_bodies(content, "%get_archincludes") {
        if let Some(modname) = arg_value(&raw, "modname") {
            if !set.arch_modules.contains(&modname) {
                set.arch_modules.push(modname);
            }
        }
    }
}

/// Parses `%set_archincludes` declarations in one `mmakefile.src`.
#[must_use]
pub fn collect_arch_decls(content: &str, rel_dir: &Path) -> Vec<ArchIncludeDecl> {
    let mut out = Vec::new();
    for raw in directive_bodies(content, "%set_archincludes") {
        let Some(mainmmake) = arg_value(&raw, "mainmmake") else {
            continue;
        };
        let Some(modname) = arg_value(&raw, "modname") else {
            continue;
        };
        let Some(tag) = arg_value(&raw, "arch") else {
            continue;
        };
        let pri = arg_value(&raw, "pri")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(u32::MAX);

        // Every declaration in the tree uses includes="-I$(SRCDIR)/$(CURDIR)",
        // i.e. the directory holding the mmakefile. Resolve it generically all
        // the same, and skip anything we cannot map.
        let includes = arg_value_quoted(&raw, "includes")
            .unwrap_or_else(|| "-I$(SRCDIR)/$(CURDIR)".to_owned());
        let tokens: Vec<String> = includes.split_whitespace().map(str::to_owned).collect();
        let mut set = IncludeSet::default();
        tokens_to_dirs(&tokens, rel_dir, &mut set);

        for dir in set.dirs {
            out.push(ArchIncludeDecl {
                mainmmake: mainmmake.clone(),
                modname: modname.clone(),
                pri,
                tag: tag.clone(),
                dir,
            });
        }
    }
    out
}

/// Returns the body of each occurrence of `directive`, joining continuations.
pub(crate) fn directive_bodies_pub(content: &str, directive: &str) -> Vec<String> {
    directive_bodies(content, directive)
}

/// The same bodies, each with the 0-based line the directive starts on.
///
/// A declaration's flags are positional: `arch/i386-all/hidd/gfx` sets
/// `USER_CFLAGS` three times, once before each `%build_archspecific`, and the
/// file-wide value is whichever assignment happens to win. Reading the flags at
/// the declaration's own line is the only way to give the SSE lane `-msse2` and
/// the AVX lane `-mavx2`.
pub(crate) fn directive_bodies_at(content: &str, directive: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut number = 0usize;
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let start = number;
        number += 1;
        let trimmed = line.trim();
        if !trimmed.starts_with(directive) {
            continue;
        }
        let mut body = trimmed.trim_end_matches('\\').to_owned();
        let mut cont = trimmed.ends_with('\\');
        while cont {
            let Some(next) = lines.next() else { break };
            number += 1;
            let t = next.trim();
            cont = t.ends_with('\\');
            body.push(' ');
            body.push_str(t.trim_end_matches('\\').trim());
        }
        out.push((start, body));
    }
    out
}

fn directive_bodies(content: &str, directive: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if !trimmed.starts_with(directive) {
            continue;
        }
        let mut body = trimmed.trim_end_matches('\\').to_owned();
        let mut cont = trimmed.ends_with('\\');
        while cont {
            let Some(next) = lines.next() else { break };
            let t = next.trim();
            cont = t.ends_with('\\');
            body.push(' ');
            body.push_str(t.trim_end_matches('\\').trim());
        }
        out.push(body);
    }
    out
}

/// Reads `key=value` from a directive body, stopping at whitespace.
pub(crate) fn arg_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    for tok in body.split_whitespace() {
        if let Some(v) = tok.strip_prefix(needle.as_str()) {
            if !v.is_empty() {
                return Some(v.trim_matches('"').to_owned());
            }
        }
    }
    None
}

/// Reads `key="value with spaces"` from a directive body.
///
/// The match must sit at a word boundary: searching for `files="` as a plain
/// substring also hits `linklibfiles="`, and would then return the wrong
/// argument's value.
pub(crate) fn arg_value_quoted(body: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let mut from = 0usize;
    loop {
        let hit = body[from..].find(needle.as_str())? + from;
        let boundary = hit == 0
            || body[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if boundary {
            let start = hit + needle.len();
            let rest = &body[start..];
            let end = rest.find('"')?;
            return Some(rest[..end].to_owned());
        }
        from = hit + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    #[test]
    fn resolves_exec_private_includes() {
        // Condensed from rom/exec/mmakefile.src.
        let src = r"
%get_archincludes modname=kernel \
    includeflag=TARGET_KERNEL_INCLUDES maindir=rom/kernel

%get_archincludes modname=exec \
    includeflag=TARGET_EXEC_INCLUDES maindir=$(CURDIR)

PRIV_EXEC_INCLUDES = \
    $(TARGET_EXEC_INCLUDES) \
	-I$(SRCDIR)/rom/exec \
	$(TARGET_KERNEL_INCLUDES) \
	-I$(SRCDIR)/rom/kernel

USER_INCLUDES += $(PRIV_EXEC_INCLUDES) -I$(SRCDIR)/rom/debug
";
        let set = collect_includes(src, &dir("rom/exec"));
        assert!(set
            .dirs
            .contains(&"${CMAKE_SOURCE_DIR}/rom/exec".to_owned()));
        assert!(set
            .dirs
            .contains(&"${CMAKE_SOURCE_DIR}/rom/kernel".to_owned()));
        assert!(set
            .dirs
            .contains(&"${CMAKE_SOURCE_DIR}/rom/debug".to_owned()));
        assert!(set.arch_modules.contains(&"exec".to_owned()));
        assert!(set.arch_modules.contains(&"kernel".to_owned()));
        assert!(
            set.unresolved.is_empty(),
            "unresolved: {:?}",
            set.unresolved
        );
    }

    #[test]
    fn parses_set_archincludes() {
        let src = "\
%set_archincludes mainmmake=kernel-exec maindir=rom/exec \\
  modname=exec pri=7 arch=pc \\
  includes=\"-I$(SRCDIR)/$(CURDIR)\"
";
        let decls = collect_arch_decls(src, &dir("arch/all-pc/exec"));
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].mainmmake, "kernel-exec");
        assert_eq!(decls[0].modname, "exec");
        assert_eq!(decls[0].pri, 7);
        assert_eq!(decls[0].tag, "pc");
        assert_eq!(decls[0].dir, "${CMAKE_SOURCE_DIR}/arch/all-pc/exec");
    }

    #[test]
    fn handles_curdir_and_relative_forms() {
        let src = "USER_INCLUDES := -I$(SRCDIR)/$(CURDIR)/include -I. -I$(GENINCDIR)\n";
        let set = collect_includes(src, &dir("rom/dos"));
        assert!(set
            .dirs
            .contains(&"${CMAKE_SOURCE_DIR}/rom/dos/include".to_owned()));
        assert!(set.dirs.contains(&"${CMAKE_SOURCE_DIR}/rom/dos".to_owned()));
        assert!(set
            .dirs
            .contains(&"${CMAKE_BINARY_DIR}/GENINCDIR".to_owned()));
    }

    #[test]
    fn handles_separate_flag_forms() {
        let src = "USER_INCLUDES := -I $(SRCDIR)/rom/a -isystem $(SRCDIR)/rom/b -idirafter $(SRCDIR)/rom/c -iquote $(SRCDIR)/rom/d\n";
        let set = collect_includes(src, &dir("x"));
        assert!(set.dirs.contains(&"${CMAKE_SOURCE_DIR}/rom/a".to_owned()));
        assert!(set.dirs.contains(&"${CMAKE_SOURCE_DIR}/rom/b".to_owned()));
        assert!(set.dirs.contains(&"${CMAKE_SOURCE_DIR}/rom/c".to_owned()));
        assert!(set.dirs.contains(&"${CMAKE_SOURCE_DIR}/rom/d".to_owned()));
    }

    #[test]
    fn collects_include_flags_from_all_user_flag_bundles() {
        let src = "\
USER_CPPFLAGS := -I$(SRCDIR)/cpp\n\
USER_CFLAGS := -I $(SRCDIR)/c -isystem $(SRCDIR)/system\n";
        let set = collect_includes(src, &dir("x"));
        assert_eq!(
            set.dirs,
            vec![
                "${CMAKE_SOURCE_DIR}/cpp",
                "${CMAKE_SOURCE_DIR}/c",
                "${CMAKE_SOURCE_DIR}/system",
            ]
        );
    }

    #[test]
    fn collects_user_cflags_at_the_declaration_position() {
        let src = "\
USER_CFLAGS := -I$(SRCDIR)/first\n\
%build_module mmake=first modname=first modtype=library files=first\n\
USER_CFLAGS := -I$(SRCDIR)/later\n";
        let scope = crate::parser::collect_vars(src);
        let set = collect_includes_at(src, &scope, 1, &dir("x"));
        assert_eq!(set.dirs, vec!["${CMAKE_SOURCE_DIR}/first"]);
    }

    #[test]
    fn collapses_parent_references() {
        let src = "USER_INCLUDES := -I$(SRCDIR)/$(CURDIR)/../include\n";
        let set = collect_includes(src, &dir("workbench/libs/foo"));
        assert_eq!(set.dirs, vec!["${CMAKE_SOURCE_DIR}/workbench/libs/include"]);
    }

    #[test]
    fn reports_unresolved_variables_instead_of_guessing() {
        let src = "USER_INCLUDES := -I$(AROS_CONTRIB_INCLUDES) -I$(SRCDIR)/rom/ok\n";
        let set = collect_includes(src, &dir("x"));
        assert_eq!(set.dirs, vec!["${CMAKE_SOURCE_DIR}/rom/ok"]);
        assert_eq!(set.unresolved, vec!["$(AROS_CONTRIB_INCLUDES)"]);
    }

    #[test]
    fn self_reference_does_not_recurse() {
        let src = "USER_INCLUDES := -I$(SRCDIR)/a $(USER_INCLUDES)\n";
        let set = collect_includes(src, &dir("x"));
        assert_eq!(set.dirs, vec!["${CMAKE_SOURCE_DIR}/a"]);
    }

    #[test]
    fn passes_target_parameters_through_as_cmake_vars() {
        let src = "USER_INCLUDES := -I$(SRCDIR)/arch/$(CPU)-$(ARCH)/include\n";
        let set = collect_includes(src, &dir("x"));
        assert_eq!(
            set.dirs,
            vec!["${CMAKE_SOURCE_DIR}/arch/${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}/include"]
        );
    }
}
