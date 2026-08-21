use crate::arch_sources::collect_arch_sources;
use crate::ast::{MetaTargetRule, ModuleType, ParsedMmakefile, TargetDefinition};
use crate::copy_includes::collect_copy_includes_with_scope;
use crate::fetch::collect_fetches_with_scope;
use crate::flags::collect_flags;
use crate::genmodule_linklibs::resolve_generated_linklib_sources;
use crate::includes::{collect_arch_decls, collect_includes};
use crate::local_make_includes::{
    inline_local_make_includes, LocalMakeFragmentPolicy, LocalMakeIncludeLimits,
};
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::make_opts::collect_make_opts;
use aros_common::{read_source, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// One non-empty #MM rule. Horizontal whitespace is intentional: `\s*` also
/// consumes a newline, so an empty `#MM setup-ppc :` used to steal the next
/// ordinary Make rule and manufacture `setup-ppc -> setup-ppc` self-cycles.
static META_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#MM-?[ \t]+([^ \t\r\n:]+)[ \t]*:[ \t]*([^\r\n]+)").unwrap());
static CONTINUATION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[ \t]*\r?\n[ \t]*").unwrap());
const MAX_DEPTH_FOR_IMMEDIATE_EXPANSION: usize = 16;

/// Makes a name safe to use as a CMake target.
///
/// A dot survives: CMake admits it, and dropping it renamed the binary. The
/// reference builds `atheros5000.device` and `wasapiaudio.dll`, which came out
/// as `atheros5000_device` and `wasapiaudio_dll`.
fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Renders the target-configuration variables used in #MM rules as CMake
/// variable references.
///
/// Dropping every dependency containing `$(` removed the root edge from
/// `workbench` to the selected icon set, so 181 correctly generated icon rules
/// would still be unreachable. Only variables with an unambiguous counterpart
/// are translated; callers report every other dynamic token.
fn render_meta_token(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = raw.trim();
    while let Some(start) = rest.find("$(") {
        out.push_str(&sanitize_ident(&rest[..start]));
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];
        let cmake_name = match name {
            // The historic ARCH/AROS_TARGET_ARCH is the machine (pc, raspi),
            // which this build calls AROS_TARGET_PLATFORM.  Historic
            // AROS_TARGET_PLATFORM is instead the compound MetaMake selector
            // (pc-x86_64, raspi-arm, raspi-aarch64).
            "AROS_TARGET_ARCH" | "ARCH" => "AROS_TARGET_PLATFORM",
            "AROS_TARGET_PLATFORM" => "AROS_TARGET_LEGACY_PLATFORM",
            "AROS_TARGET_CPU" | "CPU" => "AROS_TARGET_CPU",
            "AROS_TARGET_FAMILY" | "FAMILY" => "AROS_TARGET_FAMILY",
            "AROS_TARGET_VARIANT" => "AROS_TARGET_VARIANT",
            "AROS_TARGET_ICONSET" => "AROS_TARGET_ICONSET",
            "AROS_TARGET_CPU32" => "AROS_TARGET_CPU32",
            _ => return None,
        };
        out.push_str("${");
        out.push_str(cmake_name);
        out.push('}');
        rest = &after[end + 1..];
    }
    out.push_str(&sanitize_ident(rest));
    (!out.is_empty()).then_some(out)
}

fn expand_file_list(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    expand_file_list_depth(raw, vars, 8)
}

/// Expands a file list, following variable references.
///
/// A list routinely names other lists, and those name further ones:
/// muimaster builds its sources from `$(FUNCS) $(FILES)` where `FILES` is
/// itself `$(FILES) $(CLASSFILES)`. Expanding only one level left it with 26
/// sources where the reference has about 94. Bounded, so a variable defined in
/// terms of itself cannot loop.
fn expand_file_list_depth(
    raw: &str,
    vars: &HashMap<String, Vec<String>>,
    depth: usize,
) -> Vec<String> {
    let mut result = Vec::new();
    for token in raw.split_whitespace() {
        let cleaned = token.replace(['"', '\\'], "").trim().to_string();

        // A plain `$(VAR)` expands to its list, whose items may be references
        // in turn.
        if let Some(name) = cleaned.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
            if depth > 0 && !name.contains(' ') && !name.contains(',') {
                if let Some(list) = vars.get(name) {
                    for item in list {
                        if item.contains("$(") {
                            result.extend(expand_file_list_depth(item, vars, depth - 1));
                        } else if keep_source_name(item) {
                            result.push(item.clone());
                        }
                    }
                }
            }
            continue;
        }

        // Anything still carrying Make syntax is not a file name.
        if cleaned.contains('$') || cleaned.contains('(') || cleaned.contains(',') {
            continue;
        }
        if keep_source_name(&cleaned) {
            result.push(cleaned);
        }
    }
    result.dedup();
    result
}

/// Whether a token names a source file.
///
/// Names are kept verbatim rather than passed through sanitize_ident: a source
/// is routinely a path relative to the mmakefile, and turning `libudis86/decode`
/// into `libudis86_decode` produced a name matching no file on disk. Only the
/// CMake target name needs sanitising, not the sources it is built from.
fn keep_source_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('$')
        && !s.contains('(')
        // A stray closing paren is the tail of a `$(call ...)` the tokeniser
        // split apart. Emitted verbatim it ended a CMake argument list early:
        // `SOURCES autoinit-aros)` made the whole generated file unparsable.
        && !s.contains(')')
        && !s.contains(',')
}

/// Resolves a name argument that may reference a Make variable.
///
/// Ten declarations name their output through a variable, for instance
/// `progname=$(EXE)` in external/openurl and `progname=$(EXENAME)` in
/// arch/all-pc/bootstrap. Sanitising those verbatim produced target names like
/// `__EXE_`, and two of them then collided on the same output file. A variable
/// that resolves to exactly one value is substituted; anything else returns
/// None so the caller can report it.
#[cfg(test)]
fn resolve_name(raw: &str, vars: &HashMap<String, Vec<String>>) -> Option<String> {
    if !raw.contains("$(") {
        return Some(sanitize_ident(raw));
    }
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];
        let values = vars.get(name)?;
        if values.len() != 1 {
            return None;
        }
        out.push_str(&values[0]);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    if out.is_empty() {
        return None;
    }
    Some(sanitize_ident(&out))
}

/// Collects the source lists a build macro declares.
///
/// The reference treats files, cxxfiles, objcfiles and asmfiles as one set and
/// falls back to a default when all four are empty (make.tmpl:1643 for
/// programs, 2857ff for modules). Returns `(sources, any_declared)`; the flag
/// separates "nothing was declared" from "a list was declared but its Make
/// variables are unresolved", which must not silently fall back.
#[cfg(test)]
fn macro_sources(args: &str, vars: &HashMap<String, Vec<String>>) -> (Vec<String>, bool) {
    let mut sources = Vec::new();
    let mut declared = false;
    for key in ["files", "cxxfiles", "objcfiles", "asmfiles"] {
        let Some(raw) = macro_arg(args, key) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        declared = true;
        sources.extend(expand_file_list(&raw, vars));
    }
    (sources, declared)
}

/// Source lists resolved with their compiler-language provenance intact.
///
/// A fetched C++ stem cannot be rediscovered by probing the filesystem during
/// CMake configure because the fetch target runs later. Flattening all four
/// macro arguments into one vector therefore makes a correct future-source
/// rule impossible. The legacy macros already distinguish these lanes, so the
/// transpiled model does the same.
#[derive(Debug, Default)]
struct EvaluatedSources {
    c: Vec<String>,
    cxx: Vec<String>,
    objc: Vec<String>,
    asm: Vec<String>,
    declared: bool,
    diagnostics: Vec<String>,
}

impl EvaluatedSources {
    fn is_empty(&self) -> bool {
        self.c.is_empty() && self.cxx.is_empty() && self.objc.is_empty() && self.asm.is_empty()
    }

    fn lane_mut(&mut self, key: &str) -> &mut Vec<String> {
        match key {
            "files" => &mut self.c,
            "cxxfiles" => &mut self.cxx,
            "objcfiles" => &mut self.objc,
            "asmfiles" => &mut self.asm,
            _ => unreachable!(),
        }
    }
}

fn simple_make_variable_reference(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    let body = raw
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))
        .or_else(|| {
            raw.strip_prefix("${")
                .and_then(|value| value.strip_suffix('}'))
        })?;
    (!body.is_empty()
        && body
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')))
    .then_some(body)
}

/// Splits whitespace-separated Make expressions without breaking a nested
/// `$(function ...)` argument list. GNU Make concatenates these top-level
/// fragments with spaces, so each can be evaluated independently.
fn split_make_fragments(raw: &str) -> Vec<String> {
    let mut fragments = Vec::new();
    let mut start = None;
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote = None;
    for (at, character) in raw.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            if start.is_none() {
                start = Some(at);
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                start.get_or_insert(at);
            }
            '(' => {
                paren_depth += 1;
                start.get_or_insert(at);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                start.get_or_insert(at);
            }
            '{' => {
                brace_depth += 1;
                start.get_or_insert(at);
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                start.get_or_insert(at);
            }
            _ if character.is_whitespace() && paren_depth == 0 && brace_depth == 0 => {
                if let Some(begin) = start.take() {
                    fragments.push(raw[begin..at].to_owned());
                }
            }
            _ => {
                start.get_or_insert(at);
            }
        }
    }
    if let Some(begin) = start {
        fragments.push(raw[begin..].to_owned());
    }
    fragments
}

fn expand_source_fragments(raw: &str, context: &MakeExprContext<'_>, depth: usize) -> Vec<String> {
    if depth == 0 {
        return vec![raw.to_owned()];
    }
    let mut output = Vec::new();
    for fragment in split_make_fragments(raw) {
        if let Some(name) = simple_make_variable_reference(&fragment) {
            if let Some(value) = context.safe_local_raw(name) {
                output.extend(expand_source_fragments(&value, context, depth - 1));
                continue;
            }
        }
        output.push(fragment);
    }
    output
}

fn contains_make_function(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] != b'$' || !matches!(bytes[cursor + 1], b'(' | b'{') {
            cursor += 1;
            continue;
        }
        let open = bytes[cursor + 1];
        let close = if open == b'(' { b')' } else { b'}' };
        let mut nesting = 1usize;
        let mut end = cursor + 2;
        while end < bytes.len() {
            if bytes[end] == b'$' && bytes.get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if bytes[end] == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == bytes.len() {
            return false;
        }
        let body = raw[cursor + 2..end].trim();
        if body.find(char::is_whitespace).is_some() || contains_make_function(body) {
            return true;
        }
        cursor = end + 1;
    }
    false
}

/// Evaluates all source-list arguments at the declaration line.
///
/// A conditional source value is always all-or-error: no unconditional subset
/// can stand in for an unknown branch. An unrelated language lane using an
/// unsupported expression may be reported and omitted when another lane is
/// fully resolved, which preserves existing mixed-language targets without
/// ever merging alternatives. [`MakeExprContext`] supplies the strict
/// conditional-variable guard.
fn evaluate_macro_sources(
    args: &str,
    legacy_vars: &HashMap<String, Vec<String>>,
    context: &MakeExprContext<'_>,
) -> std::result::Result<EvaluatedSources, String> {
    evaluate_macro_sources_with_files(args, legacy_vars, context, None)
}

/// Evaluates source lanes while allowing a caller to supply an exact C source
/// manifest for `files=`. Generated genmodule wildcards are the one supported
/// use: they are empty until build time, while the other language lanes retain
/// the ordinary bounded Make evaluation and diagnostics.
fn evaluate_macro_sources_with_files(
    args: &str,
    legacy_vars: &HashMap<String, Vec<String>>,
    context: &MakeExprContext<'_>,
    resolved_files: Option<&[String]>,
) -> std::result::Result<EvaluatedSources, String> {
    let mut sources = EvaluatedSources::default();
    let mut arguments = Vec::new();
    for key in ["files", "cxxfiles", "objcfiles", "asmfiles"] {
        let Some(raw) = macro_arg(args, key) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        sources.declared = true;
        if key == "files" {
            if let Some(values) = resolved_files {
                for value in values {
                    if value.is_empty() || value.contains(';') {
                        return Err(format!("files={raw} produced an invalid source `{value}`"));
                    }
                    if !sources.c.contains(value) {
                        sources.c.push(value.clone());
                    }
                }
                continue;
            }
        }
        arguments.push((key, raw));
    }

    let mut unresolved_lanes = Vec::new();
    for (key, raw) in arguments {
        let mut values = Vec::new();
        let mut first_error = None;
        for fragment in expand_source_fragments(&raw, context, 32) {
            match evaluate_make_list(&fragment, context) {
                Ok(fragment_values) => values.extend(fragment_values),
                Err(error @ MakeExprError::UnsafeVariable { .. }) => {
                    return Err(format!("{key}={raw} cannot be evaluated: {error}"));
                }
                Err(error) => {
                    let old_values = if contains_make_function(&fragment) {
                        Vec::new()
                    } else {
                        expand_file_list(&fragment, legacy_vars)
                    };
                    if old_values.is_empty() {
                        sources.diagnostics.push(format!(
                            "{key}={raw} omitted unresolved source fragment `{fragment}`: {error}"
                        ));
                    } else {
                        sources.diagnostics.push(format!(
                            "{key}={raw} kept the legacy subset of source fragment `{fragment}`: {error}"
                        ));
                        values.extend(old_values);
                    }
                    first_error.get_or_insert_with(|| {
                        format!("{key}={raw} cannot evaluate source fragment `{fragment}`: {error}")
                    });
                }
            }
        }
        if values.is_empty() {
            if let Some(error) = first_error {
                unresolved_lanes.push(error);
            }
        }
        let lane = sources.lane_mut(key);
        for value in values {
            if value.is_empty() || value.contains(';') {
                return Err(format!("{key}={raw} produced an invalid source `{value}`"));
            }
            if !lane.contains(&value) {
                lane.push(value);
            }
        }
    }
    if sources.is_empty() {
        if let Some(error) = unresolved_lanes.into_iter().next() {
            return Err(error);
        }
    }
    Ok(sources)
}

/// Resolves a single output name through the bounded Make evaluator.
fn evaluate_name(raw: &str, context: &MakeExprContext<'_>) -> std::result::Result<String, String> {
    let expanded = evaluate_make_expr(raw, context).map_err(|error| error.to_string())?;
    let mut words = expanded.split_whitespace();
    let Some(name) = words.next() else {
        return Err("expression expanded to an empty name".to_owned());
    };
    if words.next().is_some() {
        return Err(format!(
            "expression expanded to more than one name: `{expanded}`"
        ));
    }
    let name = sanitize_ident(name);
    if name.is_empty() {
        return Err("expression expanded to an empty name".to_owned());
    }
    Ok(name)
}

fn evaluate_output_directory(
    args: &str,
    context: &MakeExprContext<'_>,
) -> std::result::Result<Option<String>, String> {
    let Some(raw) = macro_arg(args, "targetdir") else {
        return Ok(None);
    };
    let expanded = evaluate_make_expr(&raw, context)
        .map_err(|error| format!("targetdir={raw} cannot be evaluated: {error}"))?;
    let expanded = expanded.trim();
    if expanded.is_empty() {
        return Err(format!("targetdir={raw} expanded to an empty path"));
    }
    Ok(Some(expanded.to_owned()))
}

fn record_partial_source_lists(
    output: &mut Vec<String>,
    sources: &EvaluatedSources,
    relative_dir: &Path,
    invocation: &Invocation,
    mmake: &str,
) {
    output.extend(sources.diagnostics.iter().map(|diagnostic| {
        format!(
            "{}:{}: %{} mmake={mmake} {diagnostic}",
            relative_dir.display(),
            invocation.line + 1,
            invocation.name
        )
    }));
}

/// Lists the C sources in a directory, for the macros whose `files` default is
/// `$(basename $(call WILDCARD, *.c))`.
fn wildcard_c_sources(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("c") {
                return None;
            }
            p.file_stem().map(|s| s.to_string_lossy().to_string())
        })
        .collect();
    out.sort_unstable();
    out
}

/// Joins `#MM` lines that continue over several source lines.
///
/// A continued dependency list repeats the `#MM` prefix on every line:
///
/// ```text
/// #MM kernel-bsp-pc-x86_64 :   \
/// #MM         kernel-log       \
/// #MM         kernel-ata
/// ```
///
/// so a per-line regex sees the first line with nothing after the colon but a
/// backslash, and the rest as separate rules with no colon at all. 2223 of the
/// tree's 5089 `#MM` lines are continuations, which is 44% of all metatarget
/// dependencies.
fn join_mm_continuations(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut pending = false;

    for line in content.lines() {
        let trimmed = line.trim_end();
        let is_mm = trimmed.trim_start().starts_with("#MM");
        let continues = trimmed.ends_with('\\');
        let body = trimmed.trim_end_matches('\\').trim_end();

        if pending {
            // Strip the repeated marker so the text reads as one rule.
            let stripped = body
                .trim_start()
                .strip_prefix("#MM-")
                .or_else(|| body.trim_start().strip_prefix("#MM"))
                .unwrap_or_else(|| body.trim_start());
            out.push(' ');
            out.push_str(stripped.trim());
        } else {
            out.push_str(body);
        }

        if is_mm && continues {
            pending = true;
        } else {
            pending = false;
            out.push('\n');
        }
    }
    out
}

/// One macro invocation from an mmakefile: its name, argument text, and the
/// line of the continuation-joined file it stands on.
///
/// The line is what makes positional variable lookup possible; see `VarScope`.
struct Invocation {
    name: String,
    args: String,
    line: usize,
}

/// Joins Make continuation lines, so an assignment or a declaration occupies
/// exactly one line.
///
/// Nearly every declaration spreads its arguments over several lines and
/// `mmake=` is often not on the first, and a file list is nearly always written
/// one name per continued line. Joining first means one pass can both read the
/// assignments and see where each declaration stands.
#[must_use]
pub fn join_continuations(content: &str) -> String {
    CONTINUATION_RE.replace_all(content, " ").into_owned()
}

/// Concrete target values available while scanning Make conditionals.
///
/// Every field is optional on purpose.  An omitted value is not the same as an
/// empty Make variable: the former means that the CMake configuration did not
/// provide enough information to select a branch, while the latter can make an
/// `ifeq ($(VAR),)` condition decidable.  Library callers that do not supply a
/// context retain the conservative, target-agnostic parser behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetContext {
    pub cpu: Option<String>,
    pub platform: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub toolchain: Option<String>,
    pub cpu32: Option<String>,
    pub use_mmu: Option<String>,
    pub float_abi: Option<String>,
}

impl TargetContext {
    fn value(&self, name: &str) -> Option<String> {
        match name {
            "AROS_TARGET_CPU" | "CPU" => self.cpu.clone(),
            // Historic MetaMake calls the machine ARCH.  Its
            // AROS_TARGET_PLATFORM is instead the compound machine/CPU name.
            "AROS_TARGET_ARCH" | "ARCH" => self.platform.clone(),
            "AROS_TARGET_PLATFORM" => Some(format!(
                "{}-{}",
                self.platform.as_deref()?,
                self.cpu.as_deref()?
            )),
            "AROS_TARGET_FAMILY" | "FAMILY" => self.family.clone(),
            "AROS_TARGET_VARIANT" => self.variant.clone(),
            "AROS_TOOLCHAIN" => self.toolchain.clone(),
            "AROS_TARGET_CPU32" => self.cpu32.clone(),
            "USE_MMU" => self.use_mmu.clone(),
            "GCC_CONFIG_FLOAT_ABI" => self.float_abi.clone(),
            _ => None,
        }
    }
}

/// Variable assignments in the order the file makes them.
///
/// Make expands a declaration's arguments where the declaration stands.
/// `%build_progs files=$(FILES)` therefore takes the value FILES held at that
/// line, because the macro emits `<mmake>_FILES := %(files)` -- a simple
/// assignment, evaluated in place (config/make.tmpl:1868).
///
/// Reading one file-global value instead gave every declaration the file's last
/// assignment. arch/m68k-amiga/c declares `FILES := gdbstub`, a %build_progs,
/// `FILES := gdbstop`, and a second %build_progs; both came out building
/// gdbstop, two targets claimed the output SYS/C/.../gdbstop, and Ninja refused
/// to generate the build at all. 16 declarations across 9 mmakefiles read a
/// variable that is reassigned later in the same file.
pub struct VarScope {
    /// Per name, the assignments in file order as (line, values).
    assignments: HashMap<String, Vec<(usize, Vec<String>)>>,
    /// Per name, the right-hand side as written, in file order.
    ///
    /// A list is not enough for a path. `EXEDIR := $(AROS_TOOLS)/QuickPart` is
    /// one word either way, but `dir=$(AROS_PRESETS)/Icons/Gorilla/Small/$(AROS_DIR_AROS)`
    /// has to keep its slashes and its references, so path resolution reads
    /// this instead of the word list.
    raw: HashMap<String, Vec<(usize, String)>>,
    /// Assignments made inside a Make conditional, by source line.
    ///
    /// The legacy list collector intentionally retains its historical
    /// last-assignment behaviour because the icon collector evaluates
    /// condition branches separately. Generic expression evaluation must be
    /// stricter: using the last textual branch would silently merge or select
    /// architecture-specific source lists without knowing the condition.
    conditional_assignments: HashMap<String, Vec<usize>>,
    /// Names introduced as file-local switches, including an assignment in a
    /// branch proven false and explicitly commented-out `#NAME=value` feature
    /// toggles. Once seen, absence of an active assignment has GNU Make's
    /// ordinary empty value. Names never introduced by the file remain unknown
    /// because they may come from an included configuration fragment.
    local_names: HashSet<String>,
}

impl VarScope {
    /// The variable state as Make would see it at `line`.
    ///
    /// A declaration on line N sees every assignment made before it and none of
    /// those made after.
    fn snapshot(&self, line: usize) -> HashMap<String, Vec<String>> {
        self.assignments
            .iter()
            .filter_map(|(name, history)| {
                history
                    .iter()
                    .rev()
                    .find(|(at, _)| *at < line)
                    .map(|(_, values)| (name.clone(), values.clone()))
            })
            .collect()
    }

    /// The right-hand side of `name` as written, as of `line`.
    #[must_use]
    pub fn raw_at(&self, name: &str, line: usize) -> Option<String> {
        self.raw
            .get(name)?
            .iter()
            .rev()
            .find(|(at, _)| *at < line)
            .map(|(_, v)| v.clone())
    }

    /// Whether `name` was assigned in a Make conditional before `line`.
    ///
    /// A caller without an evaluated condition context must reject such a
    /// value rather than taking whichever branch happened to occur last in the
    /// source file.
    #[must_use]
    pub fn conditionally_assigned_before(&self, name: &str, line: usize) -> bool {
        self.conditional_assignments
            .get(name)
            .is_some_and(|assignments| assignments.iter().any(|at| *at < line))
    }

    /// The most recent raw value of `name` while the assignment scan is in
    /// progress. Appending is defined in terms of the value accumulated so
    /// far, not merely the last right-hand side.
    fn latest_raw(&self, name: &str) -> Option<&str> {
        self.raw
            .get(name)
            .and_then(|h| h.last())
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignmentKind {
    SimpleSet,
    RecursiveSet,
    SetIfUnset,
    Append,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VariableFlavor {
    Simple,
    Recursive,
}

/// Splits a plain Make variable assignment without mistaking a rule for one.
///
/// The tree uses `:=`, `=`, `?=` and `+=`. Keeping the operator is important:
/// two icon lists are built incrementally, and treating their `+=` lines as
/// either invalid or ordinary assignments silently drops 118 generated files.
fn variable_assignment(line: &str) -> Option<(&str, &str, AssignmentKind)> {
    let trimmed = line.trim();
    let (at, width, kind) = [
        (":=", AssignmentKind::SimpleSet),
        ("+=", AssignmentKind::Append),
        ("?=", AssignmentKind::SetIfUnset),
        ("=", AssignmentKind::RecursiveSet),
    ]
    .into_iter()
    .filter_map(|(op, kind)| trimmed.find(op).map(|at| (at, op.len(), kind)))
    .min_by_key(|(at, _, _)| *at)?;

    let name = trimmed[..at].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((name, trimmed[at + width..].trim(), kind))
}

/// Removes an unescaped GNU Make comment from one logical line.
///
/// A `#` starts a comment even when it is attached to the preceding word.
/// Keeping it in an assignment made `FILES := a b #disabled` compile a bogus
/// source named `#disabled`. An odd run of backslashes escapes the marker.
fn strip_make_comment(line: &str) -> &str {
    for (at, character) in line.char_indices() {
        if character != '#' {
            continue;
        }
        let escaped = line[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1;
        if !escaped {
            return &line[..at];
        }
    }
    line
}

/// Freezes local variable references in a simply-expanded (`:=`) assignment.
///
/// Global/configured variables remain as Make references for [`DirVars`] to
/// render later. Function calls are retained too, but their nested local
/// arguments are frozen now, which preserves the source-order semantics the
/// bounded evaluator needs at the declaration line.
fn expand_immediate_locals(raw: &str, scope: &VarScope, depth: usize) -> String {
    if depth == 0 || !raw.contains('$') {
        return raw.to_owned();
    }

    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative;
        output.push_str(&raw[cursor..dollar]);
        let Some(next) = raw.as_bytes().get(dollar + 1) else {
            output.push('$');
            break;
        };
        if *next == b'$' {
            output.push('$');
            cursor = dollar + 2;
            continue;
        }
        let (open, close) = match *next {
            b'(' => (b'(', b')'),
            b'{' => (b'{', b'}'),
            _ => {
                output.push('$');
                cursor = dollar + 1;
                continue;
            }
        };

        let mut nesting = 1usize;
        let mut end = dollar + 2;
        while end < raw.len() {
            let byte = raw.as_bytes()[end];
            if byte == b'$' && raw.as_bytes().get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if byte == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == raw.len() {
            output.push_str(&raw[dollar..]);
            break;
        }

        let body = &raw[dollar + 2..end];
        let simple_name = (!body.is_empty()
            && body.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            }))
        .then_some(body);
        if let Some(value) = simple_name.and_then(|name| scope.latest_raw(name)) {
            output.push_str(&expand_immediate_locals(value, scope, depth - 1));
        } else {
            output.push('$');
            output.push(open as char);
            output.push_str(&expand_immediate_locals(body, scope, depth - 1));
            output.push(close as char);
        }
        cursor = end + 1;
    }
    output
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConditionalTruth {
    False,
    True,
    Unknown,
}

impl ConditionalTruth {
    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ConditionalFrame {
    parent: ConditionalTruth,
    matched: ConditionalTruth,
    current: ConditionalTruth,
}

impl ConditionalFrame {
    fn new(parent: ConditionalTruth, condition: ConditionalTruth) -> Self {
        Self {
            parent,
            matched: condition,
            current: parent.and(condition),
        }
    }

    fn else_if(&mut self, condition: ConditionalTruth) {
        self.current = self.parent.and(self.matched.not()).and(condition);
        self.matched = self.matched.or(condition);
    }

    fn otherwise(&mut self) {
        self.current = self.parent.and(self.matched.not());
        self.matched = ConditionalTruth::True;
    }
}

fn directive_tail<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(word)?;
    (tail.is_empty()
        || tail
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace() || character == '('))
    .then(|| tail.trim())
}

fn split_top_level_comma(raw: &str) -> Option<(&str, &str)> {
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote = None;
    for (at, character) in raw.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && brace_depth == 0 => {
                return Some((&raw[..at], &raw[at + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn take_condition_word(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim_start();
    let first = raw.chars().next()?;
    if matches!(first, '\'' | '"') {
        let after_quote = &raw[first.len_utf8()..];
        let end = after_quote.find(first)?;
        let word = &raw[..end + 2];
        return Some((word, &after_quote[end + 1..]));
    }
    let end = raw.find(char::is_whitespace).unwrap_or(raw.len());
    Some((&raw[..end], &raw[end..]))
}

fn equality_operands(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim();
    if raw.starts_with('(') && raw.ends_with(')') {
        return split_top_level_comma(&raw[1..raw.len() - 1]);
    }
    let (left, rest) = take_condition_word(raw)?;
    let (right, trailing) = take_condition_word(rest)?;
    trailing.trim().is_empty().then_some((left, right))
}

fn unquote_condition_value(raw: &str) -> &str {
    let raw = raw.trim();
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if matches!(bytes[0], b'\'' | b'"') && bytes[0] == bytes[raw.len() - 1] {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn condition_pattern_matches(pattern: &str, word: &str) -> bool {
    let Some(percent) = pattern.find('%') else {
        return pattern == word;
    };
    let prefix = &pattern[..percent];
    let suffix = &pattern[percent + 1..];
    word.len() >= prefix.len() + suffix.len() && word.starts_with(prefix) && word.ends_with(suffix)
}

fn expand_condition_function(
    body: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
) -> Option<String> {
    let split = body.find(char::is_whitespace)?;
    let name = body[..split].trim();
    let args = body[split..].trim();
    match name {
        "strip" => Some(
            expand_condition_operand(args, scope, context, depth - 1)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "findstring" => {
            let (needle, haystack) = split_top_level_comma(args)?;
            let needle = expand_condition_operand(needle, scope, context, depth - 1)?;
            let haystack = expand_condition_operand(haystack, scope, context, depth - 1)?;
            Some(if haystack.contains(&needle) {
                needle
            } else {
                String::new()
            })
        }
        "filter" | "filter-out" => {
            let (patterns, words) = split_top_level_comma(args)?;
            let patterns = expand_condition_operand(patterns, scope, context, depth - 1)?;
            let words = expand_condition_operand(words, scope, context, depth - 1)?;
            let keep_matches = name == "filter";
            Some(
                words
                    .split_whitespace()
                    .filter(|word| {
                        let matches = patterns
                            .split_whitespace()
                            .any(|pattern| condition_pattern_matches(pattern, word));
                        matches == keep_matches
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        _ => None,
    }
}

fn expand_condition_reference(
    body: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
) -> Option<String> {
    let body = body.trim();
    if !body.is_empty()
        && body.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        if scope
            .conditional_assignments
            .get(body)
            .is_some_and(|assignments| !assignments.is_empty())
        {
            return None;
        }
        if let Some(value) = scope.latest_raw(body) {
            return expand_condition_operand(value, scope, context, depth - 1);
        }
        if let Some(value) = context.value(body) {
            return Some(value);
        }
        return scope.local_names.contains(body).then(String::new);
    }
    expand_condition_function(body, scope, context, depth)
}

fn expand_condition_operand(
    raw: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative;
        output.push_str(&raw[cursor..dollar]);
        let next = *raw.as_bytes().get(dollar + 1)?;
        if next == b'$' {
            output.push('$');
            cursor = dollar + 2;
            continue;
        }
        let (open, close) = match next {
            b'(' => (b'(', b')'),
            b'{' => (b'{', b'}'),
            _ => return None,
        };
        let mut nesting = 1usize;
        let mut end = dollar + 2;
        while end < raw.len() {
            let byte = raw.as_bytes()[end];
            if byte == b'$' && raw.as_bytes().get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if byte == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == raw.len() {
            return None;
        }
        output.push_str(&expand_condition_reference(
            &raw[dollar + 2..end],
            scope,
            context,
            depth - 1,
        )?);
        cursor = end + 1;
    }
    Some(unquote_condition_value(output.trim()).to_owned())
}

fn evaluate_conditional(
    directive: &str,
    args: &str,
    scope: &VarScope,
    context: &TargetContext,
) -> ConditionalTruth {
    let value = match directive {
        "ifeq" | "ifneq" => equality_operands(args).and_then(|(left, right)| {
            Some(
                expand_condition_operand(left, scope, context, MAX_DEPTH_FOR_IMMEDIATE_EXPANSION)?
                    == expand_condition_operand(
                        right,
                        scope,
                        context,
                        MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    )?,
            )
        }),
        "ifdef" | "ifndef" => {
            let name = args.trim();
            let value = scope
                .latest_raw(name)
                .map(str::to_owned)
                .or_else(|| context.value(name));
            value.map(|value| !value.is_empty())
        }
        _ => None,
    };
    let Some(value) = value else {
        return ConditionalTruth::Unknown;
    };
    let value = if matches!(directive, "ifneq" | "ifndef") {
        !value
    } else {
        value
    };
    if value {
        ConditionalTruth::True
    } else {
        ConditionalTruth::False
    }
}

/// Reads every variable assignment from continuation-joined mmakefile text.
#[must_use]
pub fn collect_vars(joined: &str) -> VarScope {
    collect_vars_impl(joined, None).0
}

/// Reads variable assignments while selecting every Make conditional that the
/// concrete target context makes decidable.
///
/// Assignments in a false branch are discarded. Assignments in an unknown
/// branch are also kept out of the value history, but are recorded as unsafe so
/// expression evaluation reports the unresolved lane instead of silently
/// treating it as empty or merging it with its alternative.
#[must_use]
pub fn collect_vars_with_context(joined: &str, context: &TargetContext) -> VarScope {
    collect_vars_impl(joined, Some(context)).0
}

fn collect_vars_impl(
    joined: &str,
    context: Option<&TargetContext>,
) -> (VarScope, Vec<ConditionalTruth>) {
    let mut scope = VarScope {
        assignments: HashMap::new(),
        raw: HashMap::new(),
        conditional_assignments: HashMap::new(),
        local_names: HashSet::new(),
    };
    let mut conditional_depth = 0usize;
    let mut conditional_stack: Vec<ConditionalFrame> = Vec::new();
    let mut flavors: HashMap<String, VariableFlavor> = HashMap::new();
    let mut line_states = Vec::with_capacity(joined.lines().count());

    for (line_no, raw_line) in joined.lines().enumerate() {
        let branch_state = context.map_or_else(
            || {
                if conditional_depth > 0 {
                    ConditionalTruth::Unknown
                } else {
                    ConditionalTruth::True
                }
            },
            |_| {
                conditional_stack
                    .last()
                    .map_or(ConditionalTruth::True, |frame| frame.current)
            },
        );
        line_states.push(branch_state);

        if context.is_some() {
            let commented = raw_line.trim_start().strip_prefix('#').map(str::trim_start);
            if let Some((name, _, _)) = commented.and_then(variable_assignment) {
                scope.local_names.insert(name.to_owned());
            }
        }
        let line = strip_make_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }

        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            if let Some(context) = context {
                let parent = conditional_stack
                    .last()
                    .map_or(ConditionalTruth::True, |frame| frame.current);
                let condition = evaluate_conditional(directive, args, &scope, context);
                conditional_stack.push(ConditionalFrame::new(parent, condition));
            } else {
                conditional_depth += 1;
            }
            continue;
        }
        if trimmed == "endif" {
            if context.is_some() {
                conditional_stack.pop();
            } else {
                conditional_depth = conditional_depth.saturating_sub(1);
            }
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            if let Some(context) = context {
                if let Some(frame) = conditional_stack.last_mut() {
                    let tail = trimmed.strip_prefix("else").unwrap().trim();
                    if tail.is_empty() {
                        frame.otherwise();
                    } else if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                        .into_iter()
                        .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))
                    {
                        let condition = evaluate_conditional(directive, args, &scope, context);
                        frame.else_if(condition);
                    } else {
                        frame.else_if(ConditionalTruth::Unknown);
                    }
                }
            }
            continue;
        }

        // Make has four assignment forms and the tree uses all of them.
        // Reading only `:=` lost every list written with `=` or `?=`:
        // rom/hidds/pci/pcitool declares `FILES = main pciids support locale`
        // that way, while the icon sets append to two lists with `+=`.
        let Some((var_name, value, kind)) = variable_assignment(line) else {
            continue;
        };
        scope.local_names.insert(var_name.to_owned());

        if branch_state == ConditionalTruth::Unknown {
            scope
                .conditional_assignments
                .entry(var_name.to_owned())
                .or_default()
                .push(line_no);
        }
        if context.is_some() && branch_state != ConditionalTruth::True {
            continue;
        }

        if kind == AssignmentKind::SetIfUnset && scope.assignments.contains_key(var_name) {
            continue;
        }

        let flavor = match kind {
            AssignmentKind::SimpleSet => VariableFlavor::Simple,
            AssignmentKind::RecursiveSet | AssignmentKind::SetIfUnset => VariableFlavor::Recursive,
            AssignmentKind::Append => flavors
                .get(var_name)
                .copied()
                .unwrap_or(VariableFlavor::Recursive),
        };
        let expanded_rhs = if flavor == VariableFlavor::Simple {
            expand_immediate_locals(value, &scope, MAX_DEPTH_FOR_IMMEDIATE_EXPANSION)
        } else {
            value.to_owned()
        };
        let expanded = if kind == AssignmentKind::Append {
            match scope.latest_raw(var_name) {
                Some(old) if !old.is_empty() && !expanded_rhs.is_empty() => {
                    format!("{old} {expanded_rhs}")
                }
                Some(old) if !old.is_empty() => old.to_owned(),
                _ => expanded_rhs,
            }
        } else {
            expanded_rhs
        };

        let values: Vec<String> = expanded
            .split_whitespace()
            .filter(|s| *s != "\\")
            .map(|s| s.replace(['"', '\\'], "").trim().to_owned())
            .filter(|s| keep_list_item(s))
            .collect();
        scope
            .raw
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, expanded.trim().to_owned()));
        scope
            .assignments
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, values));
        flavors.insert(var_name.to_owned(), flavor);
    }

    (scope, line_states)
}

/// Whether a word from a Make list is usable as a list item.
///
/// A slash used to disqualify one, which threw away most of what these lists
/// hold: a source name is routinely a path relative to the mmakefile, as in
/// `libudis86/decode` or `../locale`. 58 declarations came out with an empty
/// file list for that reason alone. An unresolved `$(...)` is still dropped,
/// since substituting nothing would silently compile the wrong set.
fn keep_list_item(s: &str) -> bool {
    if s.is_empty() || s.contains(',') {
        return false;
    }
    // A whole `$(VAR)` reference is kept, so expand_file_list can follow it:
    // `FILES := $(FILES) $(CLASSFILES)` has to survive collection or the list
    // it names is lost. A fragment carrying a stray paren is Make syntax the
    // tokeniser split apart and cannot be resolved.
    if s.starts_with("$(") && s.ends_with(')') && !s[2..s.len() - 1].contains(')') {
        return true;
    }
    !s.contains('$') && !s.contains(')')
}

/// Splits continuation-joined mmakefile text into its macro invocations.
///
/// Takes text already run through `join_continuations`, and records each
/// invocation's line in that text, so a declaration's arguments can be resolved
/// against the variable state as of that point rather than the file's last word.
///
/// This replaces matching the whole file with one regex. With `(?s)` and a
/// non-greedy tail such as `(.*?)(?:%common|$)`, the first `%build_module` in a
/// file swallowed every later one, because most files carry a single `%common`
/// at the end. 14 files contributed one target each instead of all of theirs,
/// costing 60 targets, among them every Wanderer and Zune class.
fn macro_invocations(joined: &str) -> Vec<Invocation> {
    let mut out = Vec::new();
    for (line_no, line) in joined.lines().enumerate() {
        let t = line.trim_start();
        let Some(after) = t.strip_prefix('%') else {
            continue;
        };
        let (name, args) = match after.find(char::is_whitespace) {
            Some(i) => (&after[..i], after[i..].trim()),
            None => (after, ""),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        out.push(Invocation {
            name: name.to_owned(),
            args: args.to_owned(),
            line: line_no,
        });
    }
    out
}

fn is_concrete_build_invocation(name: &str) -> bool {
    matches!(
        name,
        "build_module"
            | "build_module_abi"
            | "build_module_library"
            | "build_prog"
            | "build_progs"
            | "build_linklib"
            | "build_module_simple"
    )
}

fn select_target_invocations(
    joined: &str,
    line_states: Option<&[ConditionalTruth]>,
    relative_dir: &Path,
    skipped: &mut Vec<String>,
) -> Vec<Invocation> {
    macro_invocations(joined)
        .into_iter()
        .filter_map(|invocation| {
            if !is_concrete_build_invocation(&invocation.name) {
                return Some(invocation);
            }
            let Some(states) = line_states else {
                return Some(invocation);
            };
            match states
                .get(invocation.line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
            {
                ConditionalTruth::True => Some(invocation),
                ConditionalTruth::False => None,
                ConditionalTruth::Unknown => {
                    let mmake = macro_arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |name| format!(" mmake={name}"));
                    skipped.push(format!(
                        "{}:{}: %{}{} is guarded by an unresolved Make conditional",
                        relative_dir.display(),
                        invocation.line + 1,
                        invocation.name,
                        mmake
                    ));
                    None
                }
            }
        })
        .collect()
}

/// Reads `key=value` or `key="value with spaces"` from an argument text.
///
/// The key must sit at a word boundary, or `files=` also matches the tail of
/// `linklibfiles=` and returns the wrong argument.
fn macro_arg(args: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let hit = args[from..].find(key)? + from;
        let before_ok = hit == 0
            || args[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &args[hit + key.len()..];
        if before_ok {
            if let Some(v) = rest.strip_prefix("=\"") {
                let end = v.find('"')?;
                return Some(v[..end].to_owned());
            }
            if let Some(v) = rest.strip_prefix('=') {
                let end = v.find(char::is_whitespace).unwrap_or(v.len());
                let value = v[..end].trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
        from = hit + 1;
    }
}

/// Whether a full library intentionally delegates all of its sources to
/// genmodule.
///
/// An evaluated expression that happens to be empty is not equivalent: it may
/// be an unresolved source list. Only the literal quoted-empty spelling used
/// by version.library opts into this mode, and no second language lane may be
/// present.
fn is_explicit_genmodule_only(invocation: &str, args: &str, mod_type: &str) -> bool {
    let literal = "files=\"\"";
    let has_literal_empty_files = args.match_indices(literal).any(|(start, _)| {
        let end = start + literal.len();
        (start == 0
            || args[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
            && (end == args.len() || args[end..].chars().next().is_some_and(char::is_whitespace))
    });
    invocation == "build_module"
        && mod_type == "library"
        && has_literal_empty_files
        && ["cxxfiles", "objcfiles", "asmfiles"]
            .iter()
            .all(|key| macro_arg(args, key).is_none())
}

fn implicit_module_meta_rules(
    mmake: &str,
    modname: &str,
    include_set: &str,
    use_libs: &[String],
    has_abi: bool,
    has_library: bool,
) -> Vec<MetaTargetRule> {
    const fn rule(name: String, dependencies: Vec<String>) -> MetaTargetRule {
        MetaTargetRule { name, dependencies }
    }

    let mut rules = Vec::new();
    for suffix in [
        "",
        "-quick",
        "-makefile",
        "-clean",
        "-genmakefile",
        "-genmodfiles",
    ] {
        rules.push(rule(format!("{mmake}{suffix}"), Vec::new()));
    }
    rules.push(rule(
        format!("{mmake}-genmodfiles"),
        vec![format!("{mmake}-genmakefile")],
    ));
    // The quick spelling is an alias for the complete module/linklib target,
    // not merely for its reduced include and architecture prerequisites
    // (make.tmpl:2671).
    rules.push(rule(format!("{mmake}-quick"), vec![mmake.to_owned()]));

    let linklibs: Vec<String> = use_libs
        .iter()
        .map(|name| format!("linklibs-{name}"))
        .collect();
    let includes: Vec<String> = use_libs
        .iter()
        .map(|name| format!("includes-{name}"))
        .collect();

    if has_abi {
        for suffix in [
            "-includes",
            "-includes-quick",
            "-includes-dirs",
            "-fd",
            "-linklib",
            "-set-archincludes",
        ] {
            rules.push(rule(format!("{mmake}{suffix}"), Vec::new()));
        }
        for alias in [
            format!("includes-{modname}"),
            format!("includes-{modname}_rel"),
        ] {
            rules.push(rule(alias, vec![format!("{mmake}-includes")]));
        }
        for alias in [
            format!("linklibs-{modname}"),
            format!("linklibs-{modname}_rel"),
        ] {
            rules.push(rule(alias, vec![format!("{mmake}-linklib")]));
        }
        rules.push(rule(
            include_set.to_owned(),
            vec![format!("{mmake}-includes")],
        ));

        let mut base_dependencies = vec![format!("{mmake}-includes"), "core-linklibs".to_owned()];
        base_dependencies.extend(linklibs.iter().cloned());
        rules.push(rule(mmake.to_owned(), base_dependencies));

        let mut linklib_dependencies = vec![format!("{mmake}-includes")];
        linklib_dependencies.extend(includes.iter().cloned());
        rules.push(rule(format!("{mmake}-linklib"), linklib_dependencies));
        rules.push(rule(
            format!("{mmake}-quick"),
            vec![format!("{mmake}-includes-quick")],
        ));
        rules.push(rule(
            format!("{mmake}-includes"),
            vec![
                format!("{mmake}-makefile"),
                format!("{mmake}-includes-dirs"),
                format!("{mmake}-set-archincludes"),
                "includes-generate-deps".to_owned(),
                format!("{mmake}-fd"),
            ],
        ));
    }

    if has_library {
        let mut kobj_dependencies = vec!["core-linklibs".to_owned()];
        kobj_dependencies.extend(linklibs);
        if has_abi {
            kobj_dependencies.insert(0, format!("{mmake}-includes"));
        }
        rules.push(rule(format!("{mmake}-kobj"), kobj_dependencies));
        rules.push(rule(
            format!("{mmake}-kobj-quick"),
            if has_abi {
                vec![format!("{mmake}-includes-quick")]
            } else {
                Vec::new()
            },
        ));
    }

    // `%gen_archspecificrules` is expanded unconditionally for these six
    // suffixes, including the kobj identities of an ABI-only declaration.
    // Register every endpoint as a meta key: generator phase two deliberately
    // filters dependencies whose identities are unknown.
    for suffix in [
        "",
        "-set-archincludes",
        "-linklib",
        "-kobj",
        "-kobj-quick",
        "-quick",
    ] {
        let base = format!("{mmake}{suffix}");
        let cpu = format!("{mmake}-${{AROS_TARGET_CPU}}{suffix}");
        let family = format!("{mmake}-${{AROS_TARGET_FAMILY}}{suffix}");
        let arch = format!("{mmake}-${{AROS_TARGET_PLATFORM}}{suffix}");
        let arch_variant =
            format!("{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_VARIANT}}{suffix}");
        let arch_cpu = format!("{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}{suffix}");
        let arch_cpu_variant = format!(
            "{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}{suffix}"
        );

        rules.push(rule(base, vec![cpu.clone()]));
        rules.push(rule(cpu, vec![family.clone()]));
        rules.push(rule(family, vec![arch.clone()]));
        rules.push(rule(arch, vec![arch_variant.clone()]));
        rules.push(rule(arch_variant, vec![arch_cpu.clone()]));
        rules.push(rule(arch_cpu, vec![arch_cpu_variant.clone()]));
        rules.push(rule(arch_cpu_variant, Vec::new()));
    }
    rules.push(rule(
        format!("{mmake}-kobj"),
        vec![format!("{mmake}-${{AROS_TARGET_CPU}}")],
    ));
    rules.push(rule(
        format!("{mmake}-kobj-quick"),
        vec![format!("{mmake}-${{AROS_TARGET_CPU}}-quick")],
    ));

    rules
}

/// The relative module directory genmodule chooses for a full module when no
/// `moduledir=` override is present (tools/genmodule/config.c:250-333).
///
/// This is normally left to the CMake module builder. It is needed here only
/// when a declaration explicitly changes `prefix=`, because that prefix and
/// the relative default together determine the complete output directory.
fn default_relative_module_dir(mod_type: &str) -> Option<&'static str> {
    match mod_type {
        "library" => Some("Libs"),
        "class" => Some("Classes"),
        "mcc" | "mui" | "mcp" => Some("Classes/Zune"),
        "device" | "resource" | "hook" => Some("Devs"),
        "gadget" => Some("Classes/Gadgets"),
        "image" => Some("Classes/Images"),
        "datatype" => Some("Classes/DataTypes"),
        "usbclass" => Some("Classes/USB"),
        "btclass" => Some("Classes/Bluetooth"),
        "hidd" => Some("Devs/Drivers"),
        "handler" => Some("L"),
        _ => None,
    }
}

fn rendered_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path == "${AROS_BUILD_DIR}"
        || path.starts_with("${AROS_BUILD_DIR}/")
}

fn join_module_prefix(prefix: &str, directory: &str) -> String {
    if rendered_absolute(directory) {
        return directory.to_owned();
    }
    let prefix = prefix.trim_end_matches('/');
    let directory = directory.trim_start_matches('/');
    if prefix.is_empty() {
        directory.to_owned()
    } else if directory.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{directory}")
    }
}

fn expand_module_arg(
    raw: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<String, Vec<String>> {
    let local = |name: &str| scope.raw_at(name, line);

    // A whole local variable names the value of its assignment, not a fresh
    // recursive lookup of that name. This matters when a file shadows a
    // configured variable with a simple assignment such as
    // TARGETDIR := $(AROS_TESTS)/Library: AROS_TESTS was derived from the
    // configured TARGETDIR before the local assignment took effect.
    if let Some(name) = raw
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))
    {
        if !name.contains(['$', ' ', ')']) {
            if let Some(value) = local(name) {
                return dirs.expand_with(&value, &|nested| {
                    if nested == name {
                        None
                    } else {
                        local(nested)
                    }
                });
            }
        }
    }

    dirs.expand_with(raw, &local)
}

/// Resolves a module's explicit output arguments at the declaration line.
///
/// Local variables shadow the shared `make.cfg.in` directory table. An
/// explicit but unresolved value is an error: treating it like an absent
/// override would silently install the module into its type's default path.
fn resolve_module_target_dir(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
    uses_prefix: bool,
    arch_specific: bool,
) -> std::result::Result<Option<String>, String> {
    let module_dir = match macro_arg(args, "moduledir") {
        Some(raw) => Some(
            expand_module_arg(&raw, scope, dirs, line)
                .map_err(|missing| format!("moduledir={raw} references {}", missing.join(", ")))?,
        ),
        None => None,
    };

    if !uses_prefix && !arch_specific {
        return Ok(module_dir);
    }

    let prefix = if uses_prefix {
        match macro_arg(args, "prefix") {
            Some(raw) => Some(
                expand_module_arg(&raw, scope, dirs, line)
                    .map_err(|missing| format!("prefix={raw} references {}", missing.join(", ")))?,
            ),
            None => None,
        }
    } else {
        None
    };

    // An explicit moduledir replaces DEFMODDIR after the archspecific prefix
    // is computed (make.tmpl:2398-2407), so it must never inherit boot/<arch>.
    // CMake supplies the ordinary AROSDIR prefix for an otherwise relative
    // override; only an explicitly changed prefix has to be joined here.
    if let Some(directory) = module_dir {
        if rendered_absolute(&directory) {
            return Ok(Some(directory));
        }
        return Ok(Some(prefix.map_or_else(
            || directory.clone(),
            |prefix| join_module_prefix(&prefix, &directory),
        )));
    }

    if prefix.is_none() && !arch_specific {
        return Ok(None);
    }

    let directory = default_relative_module_dir(mod_type)
        .ok_or_else(|| format!("no known default moduledir for modtype={mod_type}"))?
        .to_owned();
    if rendered_absolute(&directory) {
        return Ok(Some(directory));
    }

    if arch_specific {
        // build_module_core inserts AROS_DIR_BOOTARCH between prefix and the
        // module's relative default (make.tmpl:2400-2407). With the ordinary
        // prefix, use the canonical CMake directory directly. An explicitly
        // changed prefix instead receives the same relative boot path.
        return Ok(Some(prefix.map_or_else(
            || join_module_prefix("${AROS_BOOT_ARCH_DIR}", &directory),
            |prefix| {
                join_module_prefix(
                    &prefix,
                    &format!("boot/${{AROS_TARGET_PLATFORM}}/{directory}"),
                )
            },
        )));
    }

    Ok(prefix.map(|prefix| join_module_prefix(&prefix, &directory)))
}

fn resolve_yes_argument(
    args: &str,
    key: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<bool, String> {
    let Some(raw) = macro_arg(args, key) else {
        return Ok(false);
    };
    let local = |name: &str| scope.raw_at(name, line);
    dirs.expand_with(&raw, &local)
        .map(|value| value == "yes")
        .map_err(|missing| format!("{key}={raw} references {}", missing.join(", ")))
}

fn resolve_module_suffix(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
) -> std::result::Result<Option<String>, String> {
    if let Some(raw) = macro_arg(args, "modsuffix") {
        if raw.is_empty() {
            return Ok(None);
        }
        let local = |name: &str| scope.raw_at(name, line);
        return dirs
            .expand_with(&raw, &local)
            .map(|value| (!value.is_empty()).then_some(value))
            .map_err(|missing| format!("modsuffix={raw} references {}", missing.join(", ")));
    }
    Ok(matches!(mod_type, "usbclass" | "btclass").then(|| "class".to_owned()))
}

/// Parses a single `mmakefile.src` into target definitions and meta rules.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile(path: &Path, root: &Path) -> Result<ParsedMmakefile> {
    let dirs = crate::dirs::DirVars::load(root);
    parse_mmakefile_with_dirs(path, root, &dirs)
}

/// Parses one mmakefile for a concrete target configuration.
///
/// Unlike [`parse_mmakefile`], this form may select `ifeq`/`ifneq` branches
/// whose operands are completely known from `target`. Unknown target settings
/// remain unsafe and are reported rather than inferred.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_context(
    path: &Path,
    root: &Path,
    target: &TargetContext,
) -> Result<ParsedMmakefile> {
    let dirs = crate::dirs::DirVars::load(root);
    parse_mmakefile_with_dirs_and_context(path, root, &dirs, target)
}

/// Parses one mmakefile with the shared directory-variable table.
///
/// The command-line scanner calls this form so `config/make.cfg.in` is read
/// once for the whole tree rather than once per mmakefile. The two-argument
/// wrapper remains convenient for focused tests and library callers.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, None)
}

/// Parses one mmakefile for a concrete target using a shared directory table.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs_and_context(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: &TargetContext,
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, Some(target))
}

/// Inlines source-tree Make includes for collector variable evaluation.
///
/// Build declarations remain owned by their original mmakefile.  Only the
/// variable scope used by `%fetch` and `%copy_includes` sees these files; this
/// avoids manufacturing duplicate targets from common included fragments.
/// The supported path form is deliberately bounded to paths made concrete by
/// `SRCDIR` and `CURDIR`.  Includes rooted in the build or fetched sources stay
/// deferred and continue to be reported by the collector that needs them.
/// `CURDIR` remains the original mmakefile directory through recursion, and a
/// relative include stays relative to Make's source/build root rather than to
/// the directory of the including file.
fn inline_collector_make_includes(
    content: &str,
    root: &Path,
    mmake_curdir: &Path,
    visited: &mut HashSet<std::path::PathBuf>,
    depth: usize,
) -> String {
    if depth == 0 {
        return content.to_owned();
    }

    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let rel_text = mmake_curdir.to_string_lossy().replace('\\', "/");
    let mut output = String::with_capacity(content.len());
    for line in content.lines() {
        output.push_str(line);
        output.push('\n');

        let trimmed = line.trim();
        let path_text = trimmed
            .strip_prefix("-include ")
            .or_else(|| trimmed.strip_prefix("include "))
            .map(str::trim);
        let Some(path_text) = path_text else { continue };
        if path_text.is_empty() || path_text.split_whitespace().count() != 1 {
            continue;
        }

        let expanded = path_text
            .replace("$(SRCDIR)", &root_abs.to_string_lossy())
            .replace("$(CURDIR)", &rel_text);
        if expanded.contains('$') {
            continue;
        }
        let candidate = Path::new(&expanded);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            // GNU Make resolves an include without a directory against its
            // invocation working directory. It does not switch to the
            // directory of the file which contained the include.
            root_abs.join(candidate)
        };
        let Ok(candidate) = fs::canonicalize(candidate) else {
            continue;
        };
        if !candidate.starts_with(&root_abs) || !visited.insert(candidate.clone()) {
            continue;
        }
        let Ok(included) = read_source(&candidate) else {
            continue;
        };
        output.push_str(&inline_collector_make_includes(
            &included,
            &root_abs,
            mmake_curdir,
            visited,
            depth - 1,
        ));
    }
    output
}

/// Marks forward local variables without defining them.  GNU Make expands an
/// as-yet undefined local to the empty string, which is how option defaults
/// such as Mesa's `ifeq ($(OPT_MESAGL),)` become decidable.  The parser already
/// treats commented assignments as local-name declarations, so `?=` retains
/// its correct undefined-variable behaviour.
fn collector_forward_local_prelude(content: &str) -> String {
    let mut names = HashSet::new();
    for line in content.lines() {
        let line = strip_make_comment(line);
        if let Some((name, _, _)) = variable_assignment(line) {
            names.insert(name.to_owned());
        }
    }
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort();
    let mut prelude = String::new();
    for name in names {
        prelude.push_str("# ");
        prelude.push_str(&name);
        prelude.push_str(" =\n");
    }
    prelude
}

fn parse_mmakefile_impl(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: Option<&TargetContext>,
) -> Result<ParsedMmakefile> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();

    // A small number of declarations keep a plain source inventory in a
    // sibling Make fragment. Insert only the proven-safe single-list shape at
    // the original include site, so the ordinary positional scope evaluates
    // it without a target-specific variable name. Broader configuration files
    // and recipe-bearing fragments remain untouched and reportable.
    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    let local_make_scan = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::PlainSourceLists,
    );
    let skipped_local_make_includes = local_make_scan
        .issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Make evaluates ordinary build-macro arguments at their declaration
    // line, while `%fetch` recipes retain references until recipe execution
    // after the complete file has been read. Both use the same selected
    // conditional scope but deliberately query it at different positions.
    let joined = join_continuations(&local_make_scan.expanded);
    let (scope, conditional_line_states) = match target {
        Some(target) => {
            let (scope, states) = collect_vars_impl(&joined, Some(target));
            (scope, Some(states))
        }
        None => (collect_vars(&joined), None),
    };
    let mut collector_visited = HashSet::new();
    let collector_content =
        inline_collector_make_includes(&content, root, &rel_dir, &mut collector_visited, 8);
    let collector_joined = join_continuations(&collector_content);
    let collector_input = format!(
        "{}{}",
        collector_forward_local_prelude(&collector_joined),
        collector_joined
    );
    let collector_scope = target.map_or_else(
        || collect_vars(&collector_input),
        |target| collect_vars_impl(&collector_input, Some(target)).0,
    );
    let mut targets = Vec::new();
    let mut meta_rules = Vec::new();
    let mut skipped_meta_rules = Vec::new();

    // Include paths are a file-level property in Make: USER_INCLUDES applies to
    // every rule in the mmakefile, so the same set is attached to each target
    // parsed out of this file.
    let include_set = collect_includes(&content, &rel_dir);
    let arch_decls = collect_arch_decls(&content, &rel_dir);
    let copy_scan = collect_copy_includes_with_scope(&content, &rel_dir, &collector_scope);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let (packages, skipped_packages) = crate::packages::collect_packages(&content, &rel_dir);
    let (mut arch_sources, skipped_arch_sources) = collect_arch_sources(&content, &rel_dir);
    // A %build_archspecific file contributes to a target defined elsewhere, so
    // its own USER_INCLUDES and flags have to travel with the declaration.
    for d in &mut arch_sources {
        d.include_dirs = include_set.dirs.clone();
        d.defines = flag_set.defines.clone();
        d.compile_options = flag_set.compile_options.clone();
    }
    let (fetches, skipped_fetches) =
        collect_fetches_with_scope(&content, &rel_dir, &collector_scope);

    // Architecture option files. Their contents are tagged with the
    // architecture they belong to, so CMake can keep the ones that apply; the
    // transpiler itself stays target-agnostic.
    let (opts_files, skipped_make_opts) = collect_make_opts(&content, &rel_dir, root);
    let skipped_conditions = flag_set.skipped_conditions.clone();
    // Flags guarded by an `ifeq` on the CPU or platform are already tagged by
    // the flag collector; the make.opts contents are appended below.
    let mut arch_defines: Vec<(String, String)> = flag_set.arch_defines.clone();
    let mut arch_compile_options: Vec<(String, String)> = flag_set.arch_compile_options.clone();
    let mut opts_include_dirs: Vec<String> = Vec::new();
    let mut opts_arch_includes: Vec<(String, String)> = Vec::new();
    for f in &opts_files {
        let Ok(body) = read_source(&root.join(&f.path)) else {
            continue;
        };
        let opts_flags = collect_flags(&body);
        // Include paths from an option file are resolved against the including
        // mmakefile's directory, which is what Make does.
        let opts_incs = collect_includes(&body, &rel_dir);
        match &f.tag {
            Some(tag) => {
                for d in opts_flags.defines {
                    arch_defines.push((tag.clone(), d));
                }
                for o in opts_flags.compile_options {
                    arch_compile_options.push((tag.clone(), o));
                }
                for d in opts_incs.dirs {
                    opts_arch_includes.push((tag.clone(), d));
                }
            }
            None => {
                // A local make.opts always applies.
                flag_set.defines.extend(opts_flags.defines);
                flag_set.compile_options.extend(opts_flags.compile_options);
                opts_include_dirs.extend(opts_incs.dirs);
            }
        }
    }

    // Make evaluates a declaration's arguments where the declaration stands, so
    // the variable state is positional. Both scans read the same
    // continuation-joined text, which is what makes their line numbers
    // comparable.
    let icon_scan = crate::icons::collect_icons_all(&joined, dirs, &rel_dir);
    let catalog_scan = crate::catalogs::collect_catalogs_with_line_states(
        &joined,
        &scope,
        dirs,
        root,
        &rel_dir,
        conditional_line_states.as_deref(),
    );
    let mut skipped_programs: Vec<String> = Vec::new();
    let invocations = select_target_invocations(
        &joined,
        conditional_line_states.as_deref(),
        &rel_dir,
        &mut skipped_programs,
    );
    let mut partial_source_lists: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();

    // 1. Extract module definitions
    for inv in invocations.iter().filter(|i| {
        matches!(
            i.name.as_str(),
            "build_module" | "build_module_abi" | "build_module_library"
        )
    }) {
        // The three spellings wrap the same %build_module_core, but the ABI
        // form deliberately has no runtime compilation (make.tmpl:2828).
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let Some(mod_raw) = macro_arg(&inv.args, "modname") else {
            continue;
        };
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let mmake_name = sanitize_ident(&mmake_raw);
        let mod_name = sanitize_ident(&mod_raw);
        let mod_type_owned = macro_arg(&inv.args, "modtype").unwrap_or_default();
        let mod_type_str = mod_type_owned.as_str();
        let rest = inv.args.as_str();
        let is_abi = inv.name == "build_module_abi";

        let module_type = if is_abi {
            ModuleType::Abi
        } else {
            match mod_type_str {
                "library" => ModuleType::Library,
                "device" => ModuleType::Device,
                "resource" => ModuleType::Resource,
                "hidd" => ModuleType::Hidd,
                "datatype" => ModuleType::Datatype,
                "gadget" => ModuleType::Gadget,
                "mcc" => ModuleType::Mcc,
                _ => ModuleType::Custom,
            }
        };
        let genmodule_only = is_explicit_genmodule_only(&inv.name, rest, mod_type_str);

        let arch_specific = match resolve_yes_argument(rest, "archspecific", &scope, dirs, inv.line)
        {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let target_dir = match resolve_module_target_dir(
            rest,
            &scope,
            dirs,
            inv.line,
            mod_type_str,
            true,
            arch_specific,
        ) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let mod_suffix = match resolve_module_suffix(rest, &scope, dirs, inv.line, mod_type_str) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };

        // An ABI skeleton has no implementation sources, and the one explicit
        // genmodule-only library is implemented entirely by generated start/end
        // files. Every other empty result keeps the existing strict source-list
        // handling: unresolved lists may never turn into generated-only modules.
        let sources = if is_abi || genmodule_only {
            EvaluatedSources::default()
        } else {
            // The same source-list rules as every other build macro: the union
            // of all four lanes, with the reference's *.c default only when no
            // lane was declared (make.tmpl:2802).
            let mut sources = match evaluate_macro_sources(rest, &vars, &expression_context) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            };
            record_partial_source_lists(
                &mut partial_source_lists,
                &sources,
                &rel_dir,
                inv,
                &mmake_raw,
            );
            if sources.is_empty() {
                if sources.declared {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} has an unresolved file list",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
                sources.c = wildcard_c_sources(parent_dir);
                if sources.is_empty() {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} declares no sources",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
            }
            sources
        };

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
        });
        let declared_mod_type = matches!(module_type, ModuleType::Abi | ModuleType::Custom)
            .then(|| mod_type_owned.clone());

        if is_abi || genmodule_only {
            let include_set = match macro_arg(rest, "include_set") {
                Some(raw) => {
                    let Some(rendered) = render_meta_token(&raw) else {
                        skipped_programs.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} include_set={raw} contains an unmapped Make variable",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        continue;
                    };
                    rendered
                }
                None => "includes-all".to_owned(),
            };
            meta_rules.extend(implicit_module_meta_rules(
                &mmake_name,
                &mod_name,
                &include_set,
                &use_libs,
                inv.name != "build_module_library",
                inv.name != "build_module_abi",
            ));
        }

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            genmodule_only,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type,
            mod_suffix,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = include_set.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: include_set.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: flag_set.defines.clone(),
            undefines: flag_set.undefines.clone(),
            compile_options: flag_set.compile_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
        });
    }

    // 2. Extract program definitions
    //
    // %build_prog takes progname=/A and builds one executable from all its
    // files (make.tmpl:1810). %build_progs takes files=/A and builds one per
    // file (make.tmpl:1850). Both used to match the same regex, progname was
    // never read, and every file became its own program: the four sources of
    // `%build_prog progname=SysLog` came out as colorlist, hooks, main and str
    // instead of one SysLog. Only %build_prog is handled here; %build_progs
    // needs one mmake target to carry several executables, which the target
    // model does not express yet, so it is reported instead of guessed at.
    for inv in invocations.iter().filter(|i| i.name == "build_prog") {
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let mmake_name = sanitize_ident(&mmake_raw);

        // progname is declared /A, so a declaration without one is malformed
        // rather than something to guess a name for.
        let Some(prog_raw) = macro_arg(&inv.args, "progname") else {
            skipped_programs.push(format!(
                "{}: %build_prog mmake={mmake_raw} has no progname",
                rel_dir.display()
            ));
            continue;
        };
        let prog_name = match evaluate_name(&prog_raw, &expression_context) {
            Ok(name) => name,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} is unresolved: {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };

        let mut sources = match evaluate_macro_sources(&inv.args, &vars, &expression_context) {
            Ok(sources) => sources,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };
        record_partial_source_lists(
            &mut partial_source_lists,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() {
            if sources.declared {
                // A list was given but its Make variables are unresolved.
                // Falling back to the program name here would compile the
                // wrong file, so report instead.
                skipped_programs.push(format!(
                    "{}: %build_prog mmake={mmake_raw} progname={prog_raw} has an unresolved file list",
                    rel_dir.display()
                ));
                continue;
            }
            sources.c.push(prog_name.clone());
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let target_dir = match evaluate_output_directory(&inv.args, &expression_context) {
            Ok(directory) => directory,
            Err(reason) => {
                unresolved_output_paths.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                None
            }
        };

        targets.push(TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            genmodule_only: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type: None,
            mod_suffix: None,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = include_set.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: include_set.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: flag_set.defines.clone(),
            undefines: flag_set.undefines.clone(),
            compile_options: flag_set.compile_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
        });
    }

    // 2b. The remaining build macros.
    //
    // All four share the compile model and differ only in what they link:
    // %build_prog one executable, %build_progs one per file, %build_linklib a
    // static library, %build_module_simple a module without the genmodule
    // chain. Only the link kind and the name argument change here.
    for inv in &invocations {
        let (module_type, name_arg) = match inv.name.as_str() {
            "build_progs" => (ModuleType::ProgramGroup, None),
            "build_linklib" => (ModuleType::LinkLib, Some("libname")),
            "build_module_simple" => (ModuleType::SimpleModule, Some("modname")),
            _ => continue,
        };

        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let mmake_name = sanitize_ident(&mmake_raw);

        // %build_progs has no name of its own: each source file names its own
        // executable, so the mmake id carries the group.
        let target_name = match name_arg {
            None => mmake_name.clone(),
            Some(key) => {
                let Some(raw) = macro_arg(&inv.args, key) else {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} has no {key}",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                };
                match evaluate_name(&raw, &expression_context) {
                    Ok(name) => name,
                    Err(reason) => {
                        skipped_programs.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} {key}={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        continue;
                    }
                }
            }
        };

        let resolved_generated_files = if module_type == ModuleType::LinkLib {
            match macro_arg(&inv.args, "files") {
                Some(files) => {
                    match resolve_generated_linklib_sources(&files, &joined, &rel_dir, |name| {
                        expression_context.safe_local_raw(name)
                    }) {
                        Ok(Some(generated)) => Some(generated.sources),
                        Ok(None) => None,
                        Err(reason) => {
                            skipped_programs.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {reason}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            continue;
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let mut sources = match evaluate_macro_sources_with_files(
            &inv.args,
            &vars,
            &expression_context,
            resolved_generated_files.as_deref(),
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        record_partial_source_lists(
            &mut partial_source_lists,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() {
            if sources.declared {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} has an unresolved file list",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
            // %build_module_simple defaults files to every *.c in the
            // directory. The others have no default, and %build_progs even
            // declares files=/A, so a declaration without sources is
            // malformed.
            if matches!(module_type, ModuleType::SimpleModule) {
                sources.c = wildcard_c_sources(parent_dir);
            }
            if sources.is_empty() {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} declares no sources",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let is_simple_module = matches!(module_type, ModuleType::SimpleModule);
        let declared_mod_type = if is_simple_module {
            macro_arg(&inv.args, "modtype")
        } else {
            None
        };
        let is_program_group = matches!(module_type, ModuleType::ProgramGroup);
        let target_dir = if is_simple_module {
            match resolve_module_target_dir(
                &inv.args,
                &scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
                false,
                false,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else if is_program_group {
            match evaluate_output_directory(&inv.args, &expression_context) {
                Ok(directory) => directory,
                Err(reason) => {
                    unresolved_output_paths.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    None
                }
            }
        } else {
            None
        };
        let mod_suffix = if is_simple_module {
            match resolve_module_suffix(
                &inv.args,
                &scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        // The 32-bit flavour is told apart by where it writes, not by its
        // name: libdir=$(GENDIR)/lib32 and objdir=.../32bit.
        let variant_32bit = ["libdir", "objdir"].iter().any(|k| {
            macro_arg(&inv.args, k).is_some_and(|v| v.contains("lib32") || v.contains("32bit"))
        });

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            genmodule_only: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit,
            declared_mod_type,
            mod_suffix,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = include_set.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: include_set.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: flag_set.defines.clone(),
            undefines: flag_set.undefines.clone(),
            compile_options: flag_set.compile_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
        });
    }

    // %build_module_macro is invoked five times but defined nowhere in the
    // tree. Four of the five sit under arch/.unmaintained or an architecture
    // we do not build, and one carries a "converted without testing" note, so
    // the historic build cannot expand it either.
    for inv in invocations
        .iter()
        .filter(|i| i.name == "build_module_macro")
    {
        if let Some(m) = macro_arg(&inv.args, "mmake") {
            skipped_programs.push(format!(
                "{}: %build_module_macro mmake={m} (macro is not defined anywhere in the tree)",
                rel_dir.display()
            ));
        }
    }

    // 3. Extract #MM and #MM- meta-target rules
    let mm_content = join_mm_continuations(&content);
    for cap in META_RULE_RE.captures_iter(&mm_content) {
        let raw_meta = &cap[1];
        let Some(meta_name) = render_meta_token(raw_meta) else {
            skipped_meta_rules.push(format!(
                "{}: #MM target {raw_meta} contains an unmapped Make variable",
                rel_dir.display()
            ));
            continue;
        };
        let deps_str = &cap[2];
        let mut deps = Vec::new();
        for raw_dep in deps_str.split_whitespace() {
            match render_meta_token(raw_dep) {
                Some(dep) => deps.push(dep),
                None => skipped_meta_rules.push(format!(
                    "{}: #MM {raw_meta} dependency {raw_dep} contains an unmapped Make variable",
                    rel_dir.display()
                )),
            }
        }

        if !deps.is_empty() {
            meta_rules.push(MetaTargetRule {
                name: meta_name,
                dependencies: deps,
            });
        }
    }

    Ok(ParsedMmakefile {
        targets,
        meta_rules,
        icon_targets: icon_scan.targets,
        icons: icon_scan.sets,
        skipped_icons: icon_scan.skipped,
        catalogs: catalog_scan.declarations,
        skipped_catalogs: catalog_scan.skipped,
        skipped_meta_rules,
        arch_decls,
        unresolved_includes: include_set.unresolved,
        copy_includes: copy_scan.decls,
        skipped_copy_includes: copy_scan.skipped,
        adhoc_header_rules: copy_scan.adhoc,
        generated_file_rules: copy_scan.generated_files,
        flags: flag_set,
        arch_sources,
        skipped_arch_sources,
        fetches,
        skipped_fetches,
        skipped_make_opts,
        skipped_local_make_includes,
        skipped_conditions,
        skipped_programs,
        partial_source_lists,
        unresolved_output_paths,
        packages,
        skipped_packages,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        collect_vars, collect_vars_impl, collect_vars_with_context, evaluate_macro_sources,
        implicit_module_meta_rules, is_explicit_genmodule_only, join_continuations,
        join_mm_continuations, macro_arg, macro_invocations, render_meta_token,
        resolve_module_suffix, resolve_module_target_dir, sanitize_ident,
        select_target_invocations, MakeExprContext, TargetContext, META_RULE_RE,
    };
    use crate::ast::ModuleType;
    use crate::dirs::DirVars;
    use aros_common::read_source;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use walkdir::WalkDir;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "aros-parser-include-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn target_context(cpu: &str, platform: &str, float_abi: &str) -> TargetContext {
        TargetContext {
            cpu: Some(cpu.to_owned()),
            platform: Some(platform.to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some(if cpu == "x86_64" { "i386" } else { "" }.to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(float_abi.to_owned()),
        }
    }

    fn dirs() -> DirVars {
        DirVars::load(&root())
    }

    #[test]
    fn recursive_collector_includes_keep_original_curdir_and_make_root() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("shared")).unwrap();
        fs::create_dir_all(tree.0.join("module/path")).unwrap();
        fs::write(
            tree.0.join("shared/vars.mk"),
            "include nested.mk\ninclude $(SRCDIR)/$(CURDIR)/local.mk\n",
        )
        .unwrap();
        fs::write(tree.0.join("nested.mk"), "ROOT_RELATIVE_INCLUDE := yes\n").unwrap();
        fs::write(
            tree.0.join("shared/nested.mk"),
            "WRONG_INCLUDE_FILE_DIRECTORY := yes\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/path/local.mk"),
            "ORIGINAL_MMAKE_CURDIR := yes\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("shared/local.mk"),
            "WRONG_RECURSIVE_CURDIR := yes\n",
        )
        .unwrap();

        let mut visited = std::collections::HashSet::new();
        let inlined = super::inline_collector_make_includes(
            "include $(SRCDIR)/shared/vars.mk\n",
            &tree.0,
            Path::new("module/path"),
            &mut visited,
            8,
        );
        assert!(
            inlined.contains("ROOT_RELATIVE_INCLUDE := yes"),
            "{inlined}"
        );
        assert!(
            inlined.contains("ORIGINAL_MMAKE_CURDIR := yes"),
            "{inlined}"
        );
        assert!(
            !inlined.contains("WRONG_INCLUDE_FILE_DIRECTORY"),
            "{inlined}"
        );
        assert!(!inlined.contains("WRONG_RECURSIVE_CURDIR"), "{inlined}");
    }

    #[test]
    fn every_declaration_in_a_file_is_seen() {
        // workbench/system/Wanderer/Classes and 13 other files declare several
        // modules with one %common at the end. The previous whole-file regex
        // ended on `(.*?)(?:%common|$)`, so the first match swallowed the rest
        // and 60 targets went missing.
        let src = "\
%build_module  mmake=wanderer-classes-icon modname=Icon modtype=mui files=icon
%build_module  mmake=wanderer-classes-iconlist modname=IconList modtype=mui files=iconlist
%build_module  mmake=wanderer-classes-iconlistview modname=IconListview modtype=mui files=iconlistview

%common
";
        let names: Vec<String> = macro_invocations(src)
            .iter()
            .filter(|i| i.name == "build_module")
            .filter_map(|i| macro_arg(&i.args, "mmake"))
            .collect();
        assert_eq!(
            names,
            vec![
                "wanderer-classes-icon",
                "wanderer-classes-iconlist",
                "wanderer-classes-iconlistview"
            ]
        );
    }

    #[test]
    fn arguments_spread_over_lines_belong_to_their_declaration() {
        let src = "\
%build_prog mmake=aros-tcpip-apps-syslog \\
    progname=SysLog targetdir=$(EXEDIR) \\
    files=$(FILES)

%build_prog mmake=other progname=Other files=other
";
        let joined = join_continuations(src);
        let invs = macro_invocations(&joined);
        let progs: Vec<&super::Invocation> =
            invs.iter().filter(|i| i.name == "build_prog").collect();
        assert_eq!(progs.len(), 2);
        assert_eq!(macro_arg(&progs[0].args, "progname").unwrap(), "SysLog");
        assert_eq!(macro_arg(&progs[0].args, "files").unwrap(), "$(FILES)");
        assert_eq!(macro_arg(&progs[1].args, "progname").unwrap(), "Other");
    }

    #[test]
    fn only_a_literal_empty_library_file_list_is_genmodule_only() {
        assert!(is_explicit_genmodule_only(
            "build_module",
            r#"mmake=x modname=x modtype=library files="""#,
            "library"
        ));
        for (invocation, args, mod_type) in [
            (
                "build_module",
                "mmake=x modname=x modtype=library files=$(EMPTY)",
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library files="" cxxfiles=x"#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=device files="""#,
                "device",
            ),
            (
                "build_module_abi",
                r#"mmake=x modname=x modtype=library files="""#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library files=""junk"#,
                "library",
            ),
            (
                "build_module",
                r#"mmake=x modname=x modtype=library notfiles="""#,
                "library",
            ),
        ] {
            assert!(
                !is_explicit_genmodule_only(invocation, args, mod_type),
                "unexpected generated-only acceptance: %{invocation} {args}"
            );
        }
    }

    #[test]
    fn generated_module_meta_rules_keep_aliases_and_every_arch_endpoint() {
        let rules = implicit_module_meta_rules(
            "module-id",
            "module",
            "includes-set",
            &["dependency_rel".to_owned()],
            true,
            true,
        );
        let mut metas: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for rule in rules {
            metas
                .entry(rule.name)
                .or_default()
                .extend(rule.dependencies);
        }

        for (name, dependency) in [
            ("includes-set", "module-id-includes"),
            ("includes-module", "module-id-includes"),
            ("includes-module_rel", "module-id-includes"),
            ("linklibs-module", "module-id-linklib"),
            ("linklibs-module_rel", "module-id-linklib"),
            ("module-id-genmodfiles", "module-id-genmakefile"),
        ] {
            assert!(metas[name].contains(dependency), "{name} -> {dependency}");
        }
        for dependency in [
            "module-id-includes",
            "core-linklibs",
            "linklibs-dependency_rel",
            "module-id-${AROS_TARGET_CPU}",
        ] {
            assert!(metas["module-id"].contains(dependency), "{dependency}");
        }
        assert!(metas["module-id-quick"].contains("module-id"));
        for dependency in [
            "module-id-includes",
            "includes-dependency_rel",
            "module-id-${AROS_TARGET_CPU}-linklib",
        ] {
            assert!(
                metas["module-id-linklib"].contains(dependency),
                "{dependency}"
            );
        }
        for dependency in [
            "module-id-includes",
            "core-linklibs",
            "linklibs-dependency_rel",
            "module-id-${AROS_TARGET_CPU}-kobj",
            "module-id-${AROS_TARGET_CPU}",
        ] {
            assert!(metas["module-id-kobj"].contains(dependency), "{dependency}");
        }

        for suffix in [
            "",
            "-set-archincludes",
            "-linklib",
            "-kobj",
            "-kobj-quick",
            "-quick",
        ] {
            let leaf = format!(
                "module-id-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}{suffix}"
            );
            assert!(metas.contains_key(&leaf), "missing {leaf}");
        }
        assert!(metas
            .values()
            .flatten()
            .all(|dependency| { dependency != "linklibs-" && dependency != "includes-" }));
    }

    #[test]
    fn target_context_selects_build_invocations_and_reports_unknown_guards() {
        let joined = join_continuations(
            "ifneq ($(AROS_TARGET_CPU32),)\n\
             %build_linklib mmake=linklibs-only32 libname=only32 files=only32\n\
             else\n\
             %build_linklib mmake=linklibs-native libname=native files=native\n\
             endif\n\
             ifeq ($(EXTERNAL_SWITCH),yes)\n\
             %build_prog mmake=unknown progname=unknown files=unknown\n\
             endif\n",
        );

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "linklibs-only32"),
            (target_context("arm", "raspi", "hard"), "linklibs-native"),
        ] {
            let (_, states) = collect_vars_impl(&joined, Some(&context));
            let mut skipped = Vec::new();
            let invocations = select_target_invocations(
                &joined,
                Some(&states),
                Path::new("fixture"),
                &mut skipped,
            );
            let names: Vec<String> = invocations
                .iter()
                .filter_map(|invocation| macro_arg(&invocation.args, "mmake"))
                .collect();
            assert_eq!(names, [expected]);
            assert_eq!(skipped.len(), 1, "{skipped:#?}");
            assert!(skipped[0].contains("mmake=unknown"), "{skipped:#?}");
        }
    }

    #[test]
    fn target_context_selects_catalog_branches_and_reports_unknown_guards() {
        let tree = TempTree::new();
        let catalogs = tree.0.join("catalogs");
        fs::create_dir_all(&catalogs).unwrap();
        fs::write(catalogs.join("messages.cd"), "").unwrap();
        fs::write(catalogs.join("german.ct"), "").unwrap();
        let declaration = |mmake: &str| {
            format!(
                "%build_catalogs mmake={mmake} name=Sample subdir=Tools \
                 catalogs=german description=messages source=\"\" \
                 dir=$(TARGETDIR)/SYS/Locale/Catalogs\n"
            )
        };
        let source = format!(
            "ifeq ($(AROS_TARGET_CPU),x86_64)\n{}endif\n\
             ifeq ($(AROS_TARGET_CPU),arm)\n{}endif\n\
             ifeq ($(EXTERNAL_CATALOG_SWITCH),yes)\n{}endif\n",
            declaration("catalogs-x86"),
            declaration("catalogs-arm"),
            declaration("catalogs-unknown")
        );
        let file = catalogs.join("mmakefile.src");
        fs::write(&file, source).unwrap();
        let dirs = DirVars::load(&tree.0);

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "catalogs-x86"),
            (target_context("arm", "raspi", "hard"), "catalogs-arm"),
        ] {
            let parsed =
                super::parse_mmakefile_with_dirs_and_context(&file, &tree.0, &dirs, &context)
                    .unwrap();
            let names: Vec<_> = parsed
                .catalogs
                .iter()
                .map(|catalog| catalog.mmake.as_str())
                .collect();
            assert_eq!(names, [expected]);
            assert_eq!(parsed.skipped_catalogs.len(), 1);
            assert!(
                parsed.skipped_catalogs[0].contains("mmake=catalogs-unknown"),
                "{:#?}",
                parsed.skipped_catalogs
            );
        }
    }

    #[test]
    fn a_reassigned_list_is_read_as_of_each_declaration() {
        // arch/m68k-amiga/c/mmakefile.src, reduced. Reading the file-global
        // value gave both declarations `gdbstop`, so two targets claimed the
        // same output path and Ninja refused to generate the build.
        let src = "\
FILES := gdbstub

%build_progs mmake=workbench-c-m68k-gdbstub files=$(FILES) targetdir=$(AROS_C)

FILES := gdbstop

%build_progs mmake=workbench-c-m68k-misc files=$(FILES) targetdir=$(AROS_C)
";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(invs.len(), 2);

        let first = scope.snapshot(invs[0].line);
        assert_eq!(first.get("FILES").unwrap(), &vec!["gdbstub".to_owned()]);
        let second = scope.snapshot(invs[1].line);
        assert_eq!(second.get("FILES").unwrap(), &vec!["gdbstop".to_owned()]);
    }

    #[test]
    fn a_declaration_does_not_see_a_later_assignment() {
        let src = "%build_prog mmake=a progname=A files=$(F)\nF := late\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert!(
            !scope.snapshot(invs[0].line).contains_key("F"),
            "a declaration must not read an assignment made after it"
        );
    }

    #[test]
    fn a_self_referential_assignment_keeps_the_earlier_value() {
        let src =
            "FILES := a b\nFILES := $(FILES) c\n%build_prog mmake=m progname=M files=$(FILES)\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(
            scope.snapshot(invs[0].line).get("FILES").unwrap(),
            &vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
    }

    #[test]
    fn appended_values_accumulate_in_the_positional_snapshot_and_raw_value() {
        let src = "ICONS := A B\nICONS += C D\n%build_icons mmake=x icons=$(ICONS) dir=x\nICONS += late\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let inv = &macro_invocations(&joined)[0];
        assert_eq!(
            scope.snapshot(inv.line).get("ICONS").unwrap(),
            &vec![
                "A".to_owned(),
                "B".to_owned(),
                "C".to_owned(),
                "D".to_owned()
            ]
        );
        assert_eq!(scope.raw_at("ICONS", inv.line).as_deref(), Some("A B C D"));
    }

    #[test]
    fn conditional_assignments_are_visible_to_strict_expression_callers() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             else\n\
             FILES += other-only\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars(&joined);

        assert!(scope.conditionally_assigned_before("FILES", invocation.line));
        // Preserve the existing raw view for collectors that partition and
        // evaluate Make branches themselves.
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common pc-only other-only")
        );
        assert!(!scope.conditionally_assigned_before("UNRELATED", invocation.line));
    }

    #[test]
    fn target_context_selects_one_conditional_branch_without_merging() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             FILES += x86-only\n\
             else ifeq ($(AROS_TARGET_CPU),aarch64)\n\
             FILES += arm64-only\n\
             else\n\
             FILES += other-only\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);

        let x86 = collect_vars_with_context(&joined, &target_context("x86_64", "pc", ""));
        assert_eq!(
            x86.raw_at("FILES", invocation.line).as_deref(),
            Some("common x86-only")
        );
        assert!(!x86.conditionally_assigned_before("FILES", invocation.line));

        let aarch64 = collect_vars_with_context(&joined, &target_context("aarch64", "raspi", ""));
        assert_eq!(
            aarch64.raw_at("FILES", invocation.line).as_deref(),
            Some("common arm64-only")
        );
        assert!(!aarch64.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn unknown_target_condition_is_unsafe_and_never_merged() {
        let joined = join_continuations(
            "FILES := common\n\
             ifeq ($(UNCONFIGURED_SWITCH),yes)\n\
             FILES += enabled\n\
             else\n\
             FILES += disabled\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars_with_context(&joined, &target_context("x86_64", "pc", ""));
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common")
        );
        assert!(scope.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn a_seen_local_switch_has_make_empty_value_but_an_external_name_stays_unknown() {
        let joined = join_continuations(
            "FILES := common\n\
             #LOCAL_DISABLED=yes\n\
             ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             LOCAL_CPU_FEATURE=yes\n\
             endif\n\
             ifeq ($(LOCAL_DISABLED),yes)\n\
             FILES += disabled-comment-option\n\
             endif\n\
             ifeq ($(LOCAL_CPU_FEATURE),yes)\n\
             FILES += cpu-feature\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let arm = collect_vars_with_context(&joined, &target_context("arm", "raspi", "hard"));
        assert_eq!(
            arm.raw_at("FILES", invocation.line).as_deref(),
            Some("common")
        );
        assert!(!arm.conditionally_assigned_before("FILES", invocation.line));

        let external = join_continuations(
            "FILES := common\n\
             ifeq ($(EXTERNAL_CONFIG),yes)\n\
             FILES += configured\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&external).remove(0);
        let arm = collect_vars_with_context(&external, &target_context("arm", "raspi", "hard"));
        assert!(arm.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn target_context_evaluates_local_constants_and_make_filters() {
        let joined = join_continuations(
            "DEBUG_ACPI := no\n\
             FILES := common\n\
             ifeq ($(DEBUG_ACPI),yes)\n\
             FILES += debug\n\
             else\n\
             FILES += release\n\
             endif\n\
             ifneq (,$(filter arm aarch64,$(AROS_TARGET_CPU)))\n\
             FILES += arm-family\n\
             endif\n\
             %build_prog mmake=x progname=x files=$(FILES)\n",
        );
        let invocation = macro_invocations(&joined).remove(0);
        let scope = collect_vars_with_context(&joined, &target_context("aarch64", "raspi", ""));
        assert_eq!(
            scope.raw_at("FILES", invocation.line).as_deref(),
            Some("common release arm-family")
        );
        assert!(!scope.conditionally_assigned_before("FILES", invocation.line));
    }

    #[test]
    fn a_conditional_assignment_does_not_overwrite_an_existing_value() {
        let scope = collect_vars("A := first\nA ?= second\n%build_prog mmake=x progname=X\n");
        assert_eq!(scope.raw_at("A", usize::MAX).as_deref(), Some("first"));
    }

    #[test]
    fn an_assignment_comment_is_not_a_list_item() {
        let scope = collect_vars(
            "FILES := SerialClass SerialUnitClass #unix_funcs\n\
             %build_module mmake=x modname=x files=$(FILES)\n",
        );
        assert_eq!(
            scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("SerialClass SerialUnitClass")
        );
    }

    #[test]
    fn a_continued_list_is_one_assignment() {
        let src = "QPARTFILES  := \\\n    QP_Main \\\n    QP_Gui\n%build_prog mmake=m progname=M files=$(QPARTFILES)\n";
        let joined = join_continuations(src);
        let scope = collect_vars(&joined);
        let invs = macro_invocations(&joined);
        assert_eq!(
            scope.snapshot(invs[0].line).get("QPARTFILES").unwrap(),
            &vec!["QP_Main".to_owned(), "QP_Gui".to_owned()]
        );
    }

    #[test]
    fn an_argument_name_must_match_at_a_word_boundary() {
        // Searching for `files=` as a substring also hits `linklibfiles=` and
        // `cxxfiles=`, and would return the wrong list.
        let args = "mmake=x linklibfiles=\"a b\" cxxfiles=c files=\"d e\"";
        assert_eq!(macro_arg(args, "files").unwrap(), "d e");
        assert_eq!(macro_arg(args, "linklibfiles").unwrap(), "a b");
        assert_eq!(macro_arg(args, "cxxfiles").unwrap(), "c");
    }

    #[test]
    fn a_missing_argument_is_none() {
        assert!(macro_arg("mmake=x files=y", "progname").is_none());
        // An empty value is not a value.
        assert!(macro_arg("mmake=x progname= files=y", "progname").is_none());
    }

    #[test]
    fn a_dot_survives_sanitising() {
        assert_eq!(sanitize_ident("atheros5000.device"), "atheros5000.device");
        assert_eq!(sanitize_ident("wasapiaudio.dll"), "wasapiaudio.dll");
        assert_eq!(sanitize_ident("odd/name"), "odd_name");
    }

    #[test]
    fn known_dynamic_meta_target_variables_become_cmake_references() {
        assert_eq!(
            render_meta_token("iconset-$(AROS_TARGET_ICONSET)-wbench-icons").unwrap(),
            "iconset-${AROS_TARGET_ICONSET}-wbench-icons"
        );
        assert_eq!(
            render_meta_token("includes-$(ARCH)-$(CPU)").unwrap(),
            "includes-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}"
        );
        assert_eq!(
            render_meta_token("distfiles-$(AROS_TARGET_PLATFORM)").unwrap(),
            "distfiles-${AROS_TARGET_LEGACY_PLATFORM}"
        );
        assert_eq!(
            render_meta_token("grub2-efi32-$(AROS_TARGET_CPU32)-quick").unwrap(),
            "grub2-efi32-${AROS_TARGET_CPU32}-quick"
        );
        assert!(render_meta_token("target-$(SOMETHING_UNKNOWN)").is_none());
    }

    #[test]
    fn an_empty_meta_rule_does_not_consume_the_next_make_rule() {
        let source = "#MM setup-ppc :\nsetup-ppc : preplink\n";
        let joined = join_mm_continuations(source);
        assert!(META_RULE_RE.captures_iter(&joined).next().is_none());
    }

    #[test]
    fn non_macro_lines_are_ignored() {
        let src = "FILES := a b c\n# %build_module in a comment\n%common\n";
        let invs = macro_invocations(src);
        let names: Vec<&str> = invs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["common"]);
    }

    #[test]
    fn a_name_argument_resolves_through_a_variable() {
        // external/openurl declares progname=$(EXE) with EXE := OpenURL.
        // Sanitising it verbatim produced the target name __EXE_, and two such
        // targets then collided on the same output file.
        let mut vars = std::collections::HashMap::new();
        vars.insert("EXE".to_owned(), vec!["OpenURL".to_owned()]);
        assert_eq!(super::resolve_name("$(EXE)", &vars).unwrap(), "OpenURL");
        assert_eq!(
            super::resolve_name("mesa3dgl$(EXE)", &vars).unwrap(),
            "mesa3dglOpenURL"
        );
    }

    #[test]
    fn an_unresolvable_name_is_refused() {
        let vars = std::collections::HashMap::new();
        assert!(super::resolve_name("$(EXENAME)", &vars).is_none());
        // A variable holding a list cannot name one target.
        let mut many = std::collections::HashMap::new();
        many.insert("L".to_owned(), vec!["a".to_owned(), "b".to_owned()]);
        assert!(super::resolve_name("$(L)", &many).is_none());
    }

    #[test]
    fn all_four_source_lists_are_read() {
        // developer/debug/test/cplusplus declares files="" cxxfiles="exception".
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = super::macro_sources(
            r#"mmake=x progname=exception files="" cxxfiles="exception""#,
            &vars,
        );
        assert!(declared);
        assert_eq!(srcs, vec!["exception"]);
    }

    #[test]
    fn nothing_declared_is_distinct_from_nothing_resolved() {
        let vars = std::collections::HashMap::new();
        let (srcs, declared) = super::macro_sources("mmake=x progname=p", &vars);
        assert!(srcs.is_empty());
        assert!(!declared, "no list was given at all");

        let (srcs, declared) = super::macro_sources("mmake=x files=$(UNKNOWN)", &vars);
        assert!(srcs.is_empty());
        assert!(declared, "a list was given but did not resolve");
    }

    #[test]
    fn strict_expression_fallback_keeps_language_lanes_and_rejects_conditions() {
        let root = root();
        let dirs = dirs();
        let joined = join_continuations(
            "PORTROOT := $(PORTSDIR)/fixture\n\
             CFILES := one two\n\
             CXXFILES := three four\n\
             %build_linklib mmake=ok libname=ok \\\n+                 files=\"$(addprefix $(PORTROOT)/,$(CFILES))\" \\\n+                 cxxfiles=\"$(addprefix $(PORTROOT)/,$(CXXFILES))\"\n",
        );
        let scope = collect_vars(&joined);
        let invocation = macro_invocations(&joined).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let sources = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap();
        assert_eq!(
            sources.c,
            [
                "${AROS_PORTS_DIR}/fixture/one",
                "${AROS_PORTS_DIR}/fixture/two"
            ]
        );
        assert_eq!(
            sources.cxx,
            [
                "${AROS_PORTS_DIR}/fixture/three",
                "${AROS_PORTS_DIR}/fixture/four"
            ]
        );

        let conditional = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             endif\n\
             %build_linklib mmake=unsafe libname=unsafe \\\n+                 files=\"$(addprefix source/,$(FILES))\"\n",
        );
        let scope = collect_vars(&conditional);
        let invocation = macro_invocations(&conditional).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let error = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap_err();
        assert!(error.contains("unevaluated Make conditional"), "{error}");

        let partial = join_continuations(
            "FILES := common\n\
             ifeq ($(ARCH),pc)\n\
             FILES += pc-only\n\
             else\n\
             FILES += arm-only\n\
             endif\n\
             %build_linklib mmake=legacy libname=legacy \\\n+                 files=$(FILES) cxxfiles=$(UNKNOWN_CXX)\n",
        );
        let scope = collect_vars(&partial);
        let invocation = macro_invocations(&partial).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let error = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap_err();
        assert!(error.contains("unevaluated Make conditional"), "{error}");

        let mixed = join_continuations(
            "FILES := common\n\
             %build_linklib mmake=legacy libname=legacy \\\n+                 files=$(FILES) cxxfiles=$(UNKNOWN_CXX)\n",
        );
        let scope = collect_vars(&mixed);
        let invocation = macro_invocations(&mixed).remove(0);
        let legacy = scope.snapshot(invocation.line);
        let context =
            MakeExprContext::new(&scope, &dirs, invocation.line, &root, Path::new("fixture"));
        let sources = evaluate_macro_sources(&invocation.args, &legacy, &context).unwrap();
        assert_eq!(sources.c, ["common"]);
        assert!(sources.cxx.is_empty());
        assert_eq!(sources.diagnostics.len(), 1, "{:#?}", sources.diagnostics);
        assert!(sources.diagnostics[0].contains("UNKNOWN_CXX"));
    }

    #[test]
    fn freetype_keeps_independent_prefixed_source_fragments() {
        let root = root();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/freetype2/mmakefile.src"),
            &root,
            &dirs(),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "workbench-libs-freetype-linklib")
            .expect("the independently resolvable FT2 source block must retain the target");
        assert!(!target.source_files.is_empty());
        assert!(target
            .source_files
            .iter()
            .all(|source| source.starts_with("${AROS_PORTS_DIR}/freetype2/freetype-2.14.3/src/")));
        assert!(target
            .source_files
            .iter()
            .any(|source| source.ends_with("/gzip/ftgzip")));
        assert!(!target
            .source_files
            .iter()
            .any(|source| source == "gzip/ftgzip"));
        assert!(parsed.partial_source_lists.iter().any(|diagnostic| {
            diagnostic.contains("workbench-libs-freetype-linklib")
                && diagnostic.contains("omitted unresolved source fragment")
        }));
    }

    #[test]
    fn mesa_included_config_resolves_fetch_and_public_headers_for_all_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/mesa/mmakefile.src");

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();

            assert!(
                parsed.skipped_fetches.is_empty(),
                "{cpu}: {:#?}",
                parsed.skipped_fetches
            );
            assert!(
                parsed.skipped_copy_includes.is_empty(),
                "{cpu}: {:#?}",
                parsed.skipped_copy_includes
            );
            assert_eq!(parsed.fetches.len(), 1, "{cpu}");
            let fetch = &parsed.fetches[0];
            assert_eq!(fetch.name, "mesa3d-fetch");
            assert_eq!(fetch.archive, "mesa-20.0.8");
            assert_eq!(fetch.suffixes, "tar.xz tar.gz");
            assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/mesa");
            assert_eq!(fetch.location, "${AROS_PORTS_SOURCE_DIR}");
            assert!(fetch.origins.ends_with("older-versions/20.x"));
            assert_eq!(fetch.patches, "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1");

            assert_eq!(parsed.copy_includes.len(), 4, "{cpu}");
            assert!(parsed
                .copy_includes
                .iter()
                .all(|copy| copy.name == "mesa3d-includes-copy" && copy.flatten));
            let headers: BTreeMap<_, _> = parsed
                .copy_includes
                .iter()
                .map(|copy| (copy.dest.as_str(), copy.patterns.as_slice()))
                .collect();
            assert_eq!(headers["GL"], ["gl.h", "glext.h"]);
            assert_eq!(headers["KHR"], ["khrplatform.h"]);
            assert_eq!(
                headers["EGL"],
                [
                    "egl.h",
                    "eglext.h",
                    "eglplatform.h",
                    "eglmesaext.h",
                    "eglextchromium.h"
                ]
            );
            assert_eq!(
                headers["vulkan"],
                ["vulkan.h", "vulkan_core.h", "vk_icd.h", "vk_platform.h"]
            );
            assert_eq!(
                parsed
                    .copy_includes
                    .iter()
                    .map(|copy| copy.patterns.len())
                    .sum::<usize>(),
                12
            );
            assert!(parsed.copy_includes.iter().all(|copy| copy
                .source_dir
                .starts_with("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/")));
        }
    }

    #[test]
    fn real_cpu32_build_invocations_are_absent_on_arm_and_present_on_x86() {
        let root = root();
        let dirs = dirs();
        for (path, mmake) in [
            ("compiler/alib/mmakefile.src", "linklibs-amiga32"),
            (
                "compiler/arossupport/mmakefile.src",
                "linklibs-arossupport32",
            ),
            ("compiler/autoinit/mmakefile.src", "linklibs-autoinit32"),
        ] {
            let arm = super::parse_mmakefile_with_dirs_and_context(
                &root.join(path),
                &root,
                &dirs,
                &target_context("arm", "raspi", "hard"),
            )
            .unwrap();
            assert!(
                arm.targets.iter().all(|target| target.mmake_name != mmake),
                "{mmake} leaked into ARM"
            );

            let x86 = super::parse_mmakefile_with_dirs_and_context(
                &root.join(path),
                &root,
                &dirs,
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(
                x86.targets.iter().any(|target| target.mmake_name == mmake),
                "{mmake} was lost on x86_64"
            );
        }
    }

    #[test]
    fn real_tree_e1_resolves_exactly_48_targets_without_merging_cxx_sources() {
        let root = root();
        let dirs = dirs();
        let files = [
            "developer/debug/test/freetype/mmakefile.src",
            "external/bz2/mmakefile.src",
            "tools/mkamikeymap/mmakefile.src",
            "workbench/classes/datatypes/heic/mmakefile.src",
            "workbench/classes/datatypes/jpegxl/mmakefile.src",
            "workbench/classes/datatypes/webp/mmakefile.src",
            "workbench/libs/codesets/mmakefile.src",
            "workbench/libs/expat/mmakefile.src",
            "workbench/libs/jpeg/mmakefile.src",
            "workbench/libs/lzma/mmakefile.src",
            "workbench/libs/utf8proc/mmakefile.src",
        ];
        let expected: BTreeSet<&str> = "
            test-freetype-lib-graph test-freetype-lib-common test-freetype-lib-ftcommon
            test-freetype-ftstring test-freetype-ftstring-static test-freetype-ftview
            test-freetype-ftview-static external-bz2-lib linklibs-bz2-nostdio
            external-bz2-bzip2-bin external-bz2-bzip2recover-bin tools-mkkeymap
            tools-mkamikeymap datatypes-heic-linklibs-de265 datatypes-heic-linklibs-heif
            datatypes-jpegxl-linklibs-brotli datatypes-jpegxl-linklibs-hwy
            datatypes-jpegxl-linklibs-jxl datatypes-webp-linklibs-webpdecode
            datatypes-webp-linklibs-webpencode datatypes-webp-linklibs-webputils
            workbench-libs-codesets-library linklibs-codesets libcodesets-test-b64d
            libcodesets-test-b64e libcodesets-test-detectcodeset
            libcodesets-test-utf8tostrhook libcodesets-test-demo1 libcodesets-test-convert
            libcodesets-test-autoopen workbench-libs-expat-lib workbench-libs-expat-examples
            workbench-libs-jpeg workbench-libs-lzma-library linklibs-lzma
            workbench-libs-utf8proc-library linklibs-utf8proc
            workbench-libs-utf8proc-tests-case workbench-libs-utf8proc-tests-charwidth
            workbench-libs-utf8proc-tests-custom workbench-libs-utf8proc-tests-grapheme
            workbench-libs-utf8proc-tests-iscase workbench-libs-utf8proc-tests-iterate
            workbench-libs-utf8proc-tests-maxdecomposition workbench-libs-utf8proc-tests-misc
            workbench-libs-utf8proc-tests-norm workbench-libs-utf8proc-tests-printproperty
            workbench-libs-utf8proc-tests-valid
        "
        .split_whitespace()
        .collect();
        assert_eq!(expected.len(), 48);

        let mut targets = BTreeMap::new();
        for file in files {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs).unwrap();
            for target in parsed.targets {
                if expected.contains(target.mmake_name.as_str()) {
                    targets.insert(target.mmake_name.clone(), target);
                }
            }
        }
        assert_eq!(
            targets.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected
        );

        let cxx_targets: BTreeSet<&str> = targets
            .iter()
            .filter(|(_, target)| !target.cxx_source_files.is_empty())
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            cxx_targets,
            BTreeSet::from([
                "datatypes-heic-linklibs-de265",
                "datatypes-heic-linklibs-heif",
                "datatypes-jpegxl-linklibs-hwy",
                "datatypes-jpegxl-linklibs-jxl",
            ])
        );
        assert_eq!(
            targets["datatypes-heic-linklibs-de265"]
                .cxx_source_files
                .len(),
            34
        );
        assert_eq!(
            targets["datatypes-heic-linklibs-heif"]
                .cxx_source_files
                .len(),
            119
        );
        assert_eq!(
            targets["datatypes-jpegxl-linklibs-hwy"]
                .cxx_source_files
                .len(),
            7
        );
        assert_eq!(
            targets["datatypes-jpegxl-linklibs-jxl"]
                .cxx_source_files
                .len(),
            76
        );

        let port_targets = targets
            .values()
            .filter(|target| {
                target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .any(|source| source.starts_with("${AROS_PORTS_DIR}/"))
            })
            .count();
        assert_eq!(port_targets, 46);
        assert!(targets.values().all(|target| target
            .source_files
            .iter()
            .chain(&target.cxx_source_files)
            .all(|source| !source.contains("/Volumes/Dev/"))));
    }

    #[test]
    fn concrete_profiles_keep_core_conditional_targets_and_select_png_sources() {
        let root = root();
        let dirs = dirs();
        let files = [
            "arch/all-hosted/filesys/emul_handler/mmakefile.src",
            "arch/all-native/acpica/mmakefile.src",
            "arch/all-unix/hidd/unixio/mmakefile.src",
            "arch/arm-all/arm-aeabi/mmakefile.src",
            "rom/kernel/mmakefile.src",
            "workbench/libs/png/mmakefile.src",
        ];
        let expected: BTreeSet<&str> = BTreeSet::from([
            "kernel-fs-emul",
            "kernel-acpica-sharedlib",
            "kernel-unixio",
            "linklibs-aeabi",
            "kernel-kernel",
            "workbench-libs-png",
            "linklibs-png-nostdio",
        ]);

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let target = target_context(cpu, platform, float_abi);
            let mut parsed_targets = BTreeMap::new();
            let mut skipped = Vec::new();
            for file in files {
                let parsed = super::parse_mmakefile_with_dirs_and_context(
                    &root.join(file),
                    &root,
                    &dirs,
                    &target,
                )
                .unwrap();
                skipped.extend(parsed.skipped_programs);
                for parsed_target in parsed.targets {
                    if expected.contains(parsed_target.mmake_name.as_str()) {
                        parsed_targets.insert(parsed_target.mmake_name.clone(), parsed_target);
                    }
                }
            }
            assert_eq!(
                parsed_targets
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                expected,
                "{cpu}-{platform}: {skipped:#?}"
            );

            let png = &parsed_targets["workbench-libs-png"].source_files;
            assert_eq!(
                png.iter().any(|source| source.contains("intel/")),
                cpu == "x86_64",
                "{cpu}-{platform} selected the wrong Intel PNG branch"
            );
            assert_eq!(
                png.iter().any(|source| source.contains("arm/")),
                cpu == "aarch64",
                "{cpu}-{platform} selected the wrong Arm PNG branch"
            );
            assert!(parsed_targets["kernel-kernel"]
                .source_files
                .iter()
                .any(|source| source == "kernel_mm"));
        }

        let arm = target_context("arm", "raspi", "hard");
        let aeabi = super::parse_mmakefile_with_dirs_and_context(
            &root.join("arch/arm-all/arm-aeabi/mmakefile.src"),
            &root,
            &dirs,
            &arm,
        )
        .unwrap();
        let aeabi = aeabi
            .targets
            .iter()
            .find(|target| target.mmake_name == "linklibs-aeabi")
            .unwrap();
        assert!(aeabi.source_files.iter().any(|source| source == "i2d"));
        assert!(!aeabi
            .source_files
            .iter()
            .any(|source| source == "softfloat"));

        let kernel_file = root.join("rom/kernel/mmakefile.src");
        let mut no_mmu = target_context("x86_64", "pc", "");
        no_mmu.use_mmu = Some("0".to_owned());
        let kernel =
            super::parse_mmakefile_with_dirs_and_context(&kernel_file, &root, &dirs, &no_mmu)
                .unwrap();
        let kernel = kernel
            .targets
            .iter()
            .find(|target| target.mmake_name == "kernel-kernel")
            .unwrap();
        assert!(kernel
            .source_files
            .iter()
            .all(|source| source != "kernel_mm"));

        let mut unknown_mmu = target_context("x86_64", "pc", "");
        unknown_mmu.use_mmu = None;
        let kernel =
            super::parse_mmakefile_with_dirs_and_context(&kernel_file, &root, &dirs, &unknown_mmu)
                .unwrap();
        assert!(kernel
            .targets
            .iter()
            .all(|target| target.mmake_name != "kernel-kernel"));
        assert!(kernel
            .skipped_programs
            .iter()
            .any(|diagnostic| diagnostic.contains("unevaluated Make conditional")));
    }

    #[test]
    fn btcore_plain_local_source_inventory_is_real_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("rom/bluetooth/stack/mmakefile.src");
        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let btcore = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == "linklibs-btcore")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            assert_eq!(btcore.module_type, ModuleType::LinkLib);
            assert_eq!(btcore.target_name, "btcore");
            assert_eq!(btcore.source_files.len(), 28, "{cpu}-{platform}");
            assert!(btcore
                .source_files
                .iter()
                .all(|source| source.starts_with("${CMAKE_SOURCE_DIR}/rom/bluetooth/stack/")));
            assert!(btcore
                .source_files
                .iter()
                .any(|source| source.ends_with("/core/security/smp_manager")));
            assert!(btcore
                .source_files
                .iter()
                .any(|source| source.ends_with("/aros/input_bridge")));
            assert!(parsed.skipped_local_make_includes.is_empty());
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("linklibs-btcore")));
        }
    }

    #[test]
    fn generated_linklib_wildcards_are_exact_manifests_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let expected = BTreeMap::from([
            (
                "compiler-posixc-lfa-linklib",
                vec!["@AROS_GENMODULE|normal|stackstubs,regcallstubs|posixc|library|posixc_lfa.conf"],
            ),
            (
                "compiler-posixc-lfa-linklib-rel",
                vec!["@AROS_GENMODULE|rel|stackstubs,regcallstubs|posixc|library|posixc_lfa.conf"],
            ),
            (
                "workbench-libs-gl-linklib",
                vec![
                    "gl_funcs",
                    "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf",
                ],
            ),
            (
                "workbench-libs-gl-linklib-rel",
                vec![
                    "gl_funcs",
                    "@AROS_GENMODULE|rel|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf",
                ],
            ),
        ]);

        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let target_context = target_context(cpu, platform, float_abi);
            let mut targets = BTreeMap::new();
            let mut diagnostics = Vec::new();
            for file in [
                "compiler/crt/posixc/mmakefile.src",
                "workbench/libs/gl/mmakefile.src",
            ] {
                let parsed = super::parse_mmakefile_with_dirs_and_context(
                    &root.join(file),
                    &root,
                    &dirs,
                    &target_context,
                )
                .unwrap();
                diagnostics.extend(parsed.skipped_programs);
                diagnostics.extend(parsed.partial_source_lists);
                targets.extend(
                    parsed
                        .targets
                        .into_iter()
                        .filter(|target| expected.contains_key(target.mmake_name.as_str()))
                        .map(|target| (target.mmake_name.clone(), target)),
                );
            }

            assert_eq!(
                targets.len(),
                expected.len(),
                "{cpu}-{platform}: {diagnostics:#?}"
            );
            for (mmake, sources) in &expected {
                let target = targets.get(*mmake).unwrap_or_else(|| {
                    panic!("{cpu}-{platform}: missing {mmake}: {diagnostics:#?}")
                });
                assert_eq!(target.module_type, ModuleType::LinkLib);
                assert_eq!(
                    target
                        .source_files
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    *sources,
                    "{cpu}-{platform}: {mmake}"
                );
            }
            assert!(
                diagnostics
                    .iter()
                    .all(|message| { expected.keys().all(|mmake| !message.contains(mmake)) }),
                "{cpu}-{platform}: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn concrete_profiles_keep_webp_dsp_targets_and_select_only_x86_sse2() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/classes/datatypes/webp/mmakefile.src");
        for (cpu, platform, float_abi) in [
            ("x86_64", "pc", ""),
            ("arm", "raspi", "hard"),
            ("aarch64", "raspi", ""),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &file,
                &root,
                &dirs,
                &target_context(cpu, platform, float_abi),
            )
            .unwrap();
            let targets: BTreeMap<_, _> = parsed
                .targets
                .iter()
                .map(|target| (target.mmake_name.as_str(), target))
                .collect();
            let sharpyuv = targets
                .get("datatypes-webp-linklibs-sharpyuv")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let webpdsp = targets
                .get("datatypes-webp-linklibs-webpdsp")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let sources: Vec<_> = sharpyuv
                .source_files
                .iter()
                .chain(&webpdsp.source_files)
                .collect();
            assert_eq!(
                sources.iter().any(|source| source.contains("_sse2")),
                cpu == "x86_64",
                "{cpu}-{platform} selected the wrong WebP SSE2 branch"
            );
            assert!(
                sources.iter().all(|source| !source.contains("_sse41")),
                "{cpu}-{platform} unexpectedly selected disabled WebP SSE4.1"
            );
        }
    }

    #[test]
    fn the_two_mkamikeymap_programs_keep_distinct_output_directories() {
        let root = root();
        let parsed = super::parse_mmakefile_with_dirs(
            &root.join("tools/mkamikeymap/mmakefile.src"),
            &root,
            &dirs(),
        )
        .unwrap();
        let outputs: BTreeMap<_, _> = parsed
            .targets
            .iter()
            .map(|target| (target.mmake_name.as_str(), target.target_dir.as_deref()))
            .collect();

        assert_eq!(
            outputs["tools-mkkeymap"],
            Some("${AROS_BUILD_DIR}/hosttools/")
        );
        assert_eq!(
            outputs["tools-mkamikeymap"],
            Some("${AROS_BUILD_DIR}/SYS/Extras/Developer/Build")
        );
    }

    #[test]
    fn module_directory_expansion_is_positional_and_reports_unknowns() {
        let joined = join_continuations(
            "MODDIR := Devs/First\n\
             %build_module mmake=one modname=one modtype=device files=one moduledir=$(MODDIR)\n\
             MODDIR := Storage/Second\n\
             %build_module mmake=two modname=two modtype=device files=two moduledir=$(MODDIR)\n",
        );
        let scope = collect_vars(&joined);
        let invocations = macro_invocations(&joined);
        assert_eq!(
            resolve_module_target_dir(
                &invocations[0].args,
                &scope,
                &dirs(),
                invocations[0].line,
                "device",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("Devs/First")
        );
        assert_eq!(
            resolve_module_target_dir(
                &invocations[1].args,
                &scope,
                &dirs(),
                invocations[1].line,
                "device",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("Storage/Second")
        );

        let error = resolve_module_target_dir(
            "moduledir=$(NOT_DEFINED)",
            &scope,
            &dirs(),
            usize::MAX,
            "device",
            true,
            false,
        )
        .unwrap_err();
        assert!(error.contains("NOT_DEFINED"), "{error}");
    }

    #[test]
    fn explicit_prefix_and_arch_specific_defaults_are_complete_paths() {
        let scope = collect_vars("");
        assert_eq!(
            resolve_module_target_dir(
                "prefix=$(TARGETDIR)",
                &scope,
                &dirs(),
                0,
                "library",
                true,
                false,
            )
            .unwrap()
            .as_deref(),
            Some("${AROS_BUILD_DIR}/Libs")
        );
        assert_eq!(
            resolve_module_target_dir("", &scope, &dirs(), 0, "library", true, true)
                .unwrap()
                .as_deref(),
            Some("${AROS_BOOT_ARCH_DIR}/Libs")
        );
        assert_eq!(
            resolve_module_target_dir(
                "moduledir=Storage/Foo archspecific=yes",
                &scope,
                &dirs(),
                0,
                "library",
                true,
                true,
            )
            .unwrap()
            .as_deref(),
            Some("Storage/Foo")
        );
    }

    #[test]
    fn module_suffix_override_is_separate_from_declared_type() {
        let scope = collect_vars("");
        assert_eq!(
            resolve_module_suffix("modsuffix=logger", &scope, &dirs(), 0, "library")
                .unwrap()
                .as_deref(),
            Some("logger")
        );
        assert_eq!(
            resolve_module_suffix("", &scope, &dirs(), 0, "usbclass")
                .unwrap()
                .as_deref(),
            Some("class")
        );
        assert_eq!(
            resolve_module_suffix("", &scope, &dirs(), 0, "printer").unwrap(),
            None
        );
    }

    #[test]
    fn real_tree_retains_exactly_four_abi_skeletons_and_zero_source_version() {
        let root = root();
        let dirs = dirs();
        let skip_dirs = ["build", "target", ".git"];
        let abi_invocations = WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || entry.depth() == 0
                    || !skip_dirs
                        .iter()
                        .any(|dir| entry.file_name().to_string_lossy() == *dir)
            })
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file() && entry.file_name() == "mmakefile.src")
            .map(|entry| {
                read_source(entry.path())
                    .unwrap()
                    .matches("%build_module_abi")
                    .count()
            })
            .sum::<usize>();
        assert_eq!(abi_invocations, 4);

        let abi_files = [
            (
                "rom/bluetooth/classes/mmakefile.src",
                "kernel-bluetooth-btclass",
                "btclass",
            ),
            (
                "rom/usb/classes/mmakefile.src",
                "kernel-usb-usbclass",
                "usbclass",
            ),
            (
                "rom/usb/classes/arosx/include/mmakefile.src",
                "kernel-usb-classes-arosx-library",
                "arosx",
            ),
            (
                "workbench/libs/dxtn/mmakefile.src",
                "workbench-libs-dxtn",
                "dxtn",
            ),
        ];

        for (file, mmake, modname) in abi_files {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs).unwrap();
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap_or_else(|| panic!("{file} did not retain {mmake}"));
            assert_eq!(target.module_type, ModuleType::Abi);
            assert_eq!(target.target_name, modname);
            assert_eq!(target.declared_mod_type.as_deref(), Some("library"));
            assert!(!target.genmodule_only);
            assert!(target.source_files.is_empty());
            assert!(target.cxx_source_files.is_empty());
            assert!(target.objc_source_files.is_empty());
            assert!(target.asm_source_files.is_empty());

            let mut metas: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for rule in &parsed.meta_rules {
                metas
                    .entry(&rule.name)
                    .or_default()
                    .extend(rule.dependencies.iter().map(String::as_str));
            }
            assert!(metas[mmake].contains(&format!("{mmake}-includes").as_str()));
            assert!(metas[&format!("linklibs-{modname}").as_str()]
                .contains(&format!("{mmake}-linklib").as_str()));
            assert!(metas.contains_key(format!("{mmake}-kobj").as_str()));
            assert!(metas.contains_key(
                format!(
                    "{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}-quick"
                )
                .as_str()
            ));
        }

        let parsed = super::parse_mmakefile_with_dirs(
            &root.join("workbench/libs/version/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap();
        let version = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "workbench-libs-version")
            .expect("version.library must be retained");
        assert_eq!(version.module_type, ModuleType::Library);
        assert!(version.genmodule_only);
        assert!(version.source_files.is_empty());
        assert!(parsed
            .meta_rules
            .iter()
            .any(|rule| rule.name == "linklibs-version"
                && rule.dependencies == ["workbench-libs-version-linklib"]));
    }

    #[test]
    fn real_tree_module_output_metadata_has_expected_coverage() {
        let root = root();
        let dirs = dirs();
        let target = target_context("x86_64", "pc", "");
        let mut install_dirs = Vec::new();
        let mut suffixes = Vec::new();
        let mut output_errors = Vec::new();

        let skip_dirs = ["build", "target", ".git"];
        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || entry.depth() == 0
                    || !skip_dirs
                        .iter()
                        .any(|dir| entry.file_name().to_string_lossy() == *dir)
            })
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() != "mmakefile.src" {
                continue;
            }
            let source = read_source(entry.path()).unwrap();
            if !source.contains("moduledir=")
                && !source.contains("prefix=$(TARGETDIR)")
                && !source.contains("archspecific=yes")
                && !source.contains("modsuffix=")
            {
                continue;
            }
            let parsed =
                super::parse_mmakefile_with_dirs_and_context(entry.path(), &root, &dirs, &target)
                    .unwrap();
            install_dirs.extend(parsed.targets.iter().filter_map(|target| {
                if matches!(
                    target.module_type,
                    ModuleType::Program | ModuleType::ProgramGroup
                ) {
                    return None;
                }
                target
                    .target_dir
                    .as_ref()
                    .map(|directory| (target.mmake_name.clone(), directory.clone()))
            }));
            suffixes.extend(parsed.targets.iter().filter_map(|target| {
                target
                    .mod_suffix
                    .as_ref()
                    .map(|suffix| (target.mmake_name.clone(), suffix.clone()))
            }));
            output_errors.extend(parsed.skipped_programs.into_iter().filter(|message| {
                ["moduledir=", "prefix=", "archspecific=", "modsuffix="]
                    .iter()
                    .any(|needle| message.contains(needle))
            }));
        }

        assert!(output_errors.is_empty(), "{output_errors:#?}");
        assert_eq!(install_dirs.len(), 61);
        assert_eq!(suffixes.len(), 44);
        assert_eq!(
            install_dirs
                .iter()
                .filter(|(mmake, directory)| {
                    mmake.starts_with("test-library-")
                        && directory == "${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/Library/Libs"
                })
                .count(),
            4
        );
        assert_eq!(
            install_dirs
                .iter()
                .filter(|(_, directory)| directory.starts_with("${AROS_BOOT_ARCH_DIR}/"))
                .count(),
            12
        );
    }
}
