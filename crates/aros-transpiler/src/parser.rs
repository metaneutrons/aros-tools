use crate::arch_sources::collect_arch_sources;
use crate::ast::{
    AhiBuildDecl, ConfigureBuildDecl, CopyDirectoryDecl, DefineHeaderDecl, ExternalCMakeDecl,
    GenmoduleLinklibs, GrubBuildDecl, MetaTargetRule, ModuleType, ParsedMmakefile,
    PythonGeneratorJob, PythonOutputsDecl, PythonPackageDecl, TargetDefinition,
};
use crate::copy_includes::collect_copy_includes_with_scope;
use crate::fetch::{collect_fetches_with_scope, FetchDecl};
use crate::flags::{collect_flags, collect_flags_at};
use crate::flexcat::collect_flexcat_source_rules;
use crate::genmodule_linklibs::resolve_generated_linklib_sources;
use crate::includes::{collect_arch_decls, collect_includes, collect_includes_at};
use crate::local_make_includes::{
    inline_local_make_includes, LocalMakeFragmentPolicy, LocalMakeIncludeLimits,
    LocalMakeIncludeScan,
};
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::make_opts::collect_make_opts;
use aros_common::{read_source, Result};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
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

fn evaluate_linklib_list(
    args: &str,
    key: &str,
    legacy_vars: &HashMap<String, Vec<String>>,
    context: &MakeExprContext<'_>,
) -> std::result::Result<Vec<String>, String> {
    let Some(raw) = macro_arg(args, key) else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    if raw.contains('"') {
        return Err(format!("{key}={raw} contains an unsupported quote"));
    }
    let synthetic = format!("files=\"{raw}\"");
    let evaluated = evaluate_macro_sources(&synthetic, legacy_vars, context)
        .map_err(|error| format!("{key}={raw} cannot be evaluated: {error}"))?;
    if evaluated.c.is_empty() {
        return Err(format!("{key}={raw} expanded to no inputs"));
    }
    Ok(evaluated.c)
}

fn source_basename(source: &str) -> Option<String> {
    Path::new(source)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// Maps the legacy `linklibobjs=` paths back to declaration-owned sources.
///
/// Generated objects below a `linklib/` component are already represented by
/// `linklibfiles=` or the genmodule manifest. Every other object must have one
/// unambiguous implementation source with the same basename; otherwise CMake
/// cannot reproduce the archive composition safely.
fn map_linklib_object_sources(
    objects: &[String],
    implementation_sources: &[String],
) -> std::result::Result<Vec<String>, String> {
    let mut mapped = Vec::new();
    for object in objects {
        let normalized = object.replace('\\', "/");
        if normalized
            .split('/')
            .any(|component| component == "linklib")
        {
            continue;
        }
        // MetaMake's object suffix is case-sensitive; `.O` is not an object
        // reference here even on a case-insensitive host filesystem.
        if Path::new(&normalized).extension() != Some(std::ffi::OsStr::new("o")) {
            return Err(format!("linklibobjs contains non-object `{object}`"));
        }
        let Some(stem) = source_basename(&normalized) else {
            return Err(format!("cannot determine object basename for `{object}`"));
        };
        let candidates: Vec<&String> = implementation_sources
            .iter()
            .filter(|source| source_basename(source).as_deref() == Some(stem.as_str()))
            .collect();
        if candidates.len() != 1 {
            return Err(format!(
                "linklib object `{object}` maps to {} implementation sources named `{stem}`",
                candidates.len()
            ));
        }
        if !mapped.contains(candidates[0]) {
            mapped.push(candidates[0].clone());
        }
    }
    Ok(mapped)
}

/// The subset of a genmodule config that decides the client archive.
struct GenmoduleConfigFacts {
    has_relative: bool,
    relative_libraries: Vec<String>,
    /// `options stubs` or `options autoinit` stated in the config. Either one
    /// puts a generated source into `<mod>_LINKLIBFILES` regardless of the
    /// module type, so either one makes the archive exist.
    forces_client_archive: bool,
}

fn read_genmodule_linklib_config(directory: &Path, module: &str) -> Option<GenmoduleConfigFacts> {
    let content = fs::read_to_string(directory.join(format!("{module}.conf"))).ok()?;
    let mut in_config = false;
    let mut has_relative = false;
    let mut forces_client_archive = false;
    let mut relative_libraries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        match trimmed {
            "##begin config" => {
                in_config = true;
                continue;
            }
            "##end config" => {
                in_config = false;
                continue;
            }
            _ => {}
        }
        if !in_config || trimmed.starts_with('#') {
            continue;
        }
        if let Some(options) = trimmed.strip_prefix("options ") {
            let mut stated = options.split([',', ' ', '\t']);
            // `options` may appear on several lines; each one contributes.
            for option in stated.by_ref() {
                match option {
                    "rellinklib" => has_relative = true,
                    "stubs" | "autoinit" => forces_client_archive = true,
                    _ => {}
                }
            }
        } else if let Some(library) = trimmed.strip_prefix("rellib ") {
            let library = library.split_whitespace().next().unwrap_or_default();
            if !library.is_empty() && !relative_libraries.iter().any(|value| value == library) {
                relative_libraries.push(library.to_owned());
            }
        }
    }
    Some(GenmoduleConfigFacts {
        has_relative,
        relative_libraries,
        forces_client_archive,
    })
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
#[derive(Clone, Debug)]
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
    conditional_assignments: HashMap<String, Vec<(usize, AssignmentKind)>>,
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
            .is_some_and(|assignments| assignments.iter().any(|(at, _)| *at < line))
    }

    /// Whether every unresolved conditional assignment before `line` merely
    /// appends to the known value accumulated outside those branches.
    ///
    /// This is useful for flag bundles: their unconditional prefix remains a
    /// sound lower bound when optional feature probes only add flags. An
    /// unresolved replacement (`=`, `:=` or `?=`) invalidates the whole value.
    #[must_use]
    pub(crate) fn conditionally_appended_only_before(&self, name: &str, line: usize) -> bool {
        let Some(assignments) = self.conditional_assignments.get(name) else {
            return false;
        };
        let mut assignments = assignments.iter().filter(|(at, _)| *at < line).peekable();
        assignments.peek().is_some() && assignments.all(|(_, kind)| *kind == AssignmentKind::Append)
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
/// The tree uses `::=`, `:=`, `=`, `?=` and `+=`. Keeping the operator is important:
/// two icon lists are built incrementally, and treating their `+=` lines as
/// either invalid or ordinary assignments silently drops 118 generated files.
fn variable_assignment(line: &str) -> Option<(&str, &str, AssignmentKind)> {
    let trimmed = line.trim();
    let (at, width, kind) = [
        ("::=", AssignmentKind::SimpleSet),
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
    collect_vars_impl_with_forward_locals(joined, context, false)
}

fn collect_vars_impl_with_forward_locals(
    joined: &str,
    context: Option<&TargetContext>,
    forward_locals: bool,
) -> (VarScope, Vec<ConditionalTruth>) {
    let mut scope = VarScope {
        assignments: HashMap::new(),
        raw: HashMap::new(),
        conditional_assignments: HashMap::new(),
        local_names: HashSet::new(),
    };
    if context.is_some() && forward_locals {
        for raw_line in joined.lines() {
            let commented = raw_line.trim_start().strip_prefix('#').map(str::trim_start);
            let assignment = commented
                .and_then(variable_assignment)
                .or_else(|| variable_assignment(strip_make_comment(raw_line)));
            if let Some((name, _, _)) = assignment {
                scope.local_names.insert(name.to_owned());
            }
        }
    }
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

        // Make has five assignment spellings here and the tree uses them.
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
                .push((line_no, kind));
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
            | "build_with_cmake"
            | "build_with_configure"
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

/// Returns the top-level keyword names in one macro invocation.
///
/// Values may contain quoted whitespace or nested Make functions. Neither may
/// manufacture another macro argument: only an identifier beginning at a
/// top-level word boundary and followed immediately by `=` is retained.
fn macro_argument_names(args: &str) -> Vec<String> {
    let bytes = args.as_bytes();
    let mut names = Vec::new();
    let mut cursor = 0usize;
    let mut quote = None;
    let mut make_depth = 0usize;
    let mut word_boundary = true;

    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            word_boundary = false;
            cursor += 1;
            continue;
        }
        if byte == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            word_boundary = false;
            cursor += 2;
            continue;
        }
        if byte == b')' && make_depth > 0 {
            make_depth -= 1;
            cursor += 1;
            continue;
        }
        if make_depth == 0 && byte.is_ascii_whitespace() {
            word_boundary = true;
            cursor += 1;
            continue;
        }
        if make_depth == 0 && word_boundary && (byte.is_ascii_alphabetic() || byte == b'_') {
            let start = cursor;
            cursor += 1;
            while bytes
                .get(cursor)
                .is_some_and(|candidate| candidate.is_ascii_alphanumeric() || *candidate == b'_')
            {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'=') {
                names.push(args[start..cursor].to_owned());
            }
            word_boundary = false;
            continue;
        }
        word_boundary = false;
        cursor += 1;
    }
    names
}

/// The directory roots which map to stable CMake locations in a generated
/// copy target.  A `%copy_dir_recursive` recipe runs below the source tree;
/// accepting an arbitrary host path here would make the generated graph both
/// non-reproducible and unsafe to configure.
const COPY_DIRECTORY_CMAKE_ROOTS: &[&str] = &[
    "${CMAKE_SOURCE_DIR}",
    "${CMAKE_BINARY_DIR}",
    "${AROS_BUILD_DIR}",
    "${AROS_PORTS_DIR}",
    "${AROS_PORTS_SOURCE_DIR}",
    "${AROS_SDK_INCLUDE_DIR}",
    "${AROS_GENINC_DIR}",
];

/// Maps the two historic staging roots to the CMake roots which actually feed
/// compilation.  `AROS_INCLUDES` is the target SDK bootstrap tree in this
/// build, while `GENINCDIR` is the host-tool header tree; expanding the legacy
/// config literally would otherwise point at its unused `gen/include` and
/// `SYS/Developer/include` layouts.
fn normalize_copy_directory_root_alias(path: &str) -> String {
    for (legacy, cmake) in [
        ("${AROS_BUILD_DIR}/gen/include", "${AROS_GENINC_DIR}"),
        (
            "${AROS_BUILD_DIR}/SYS/Developer/include",
            "${AROS_SDK_INCLUDE_DIR}",
        ),
    ] {
        if path == legacy {
            return cmake.to_owned();
        }
        if let Some(tail) = path
            .strip_prefix(legacy)
            .filter(|tail| tail.starts_with('/'))
        {
            return format!("{cmake}{tail}");
        }
    }
    path.to_owned()
}

/// Accepts only ordinary path components.  CMake receives every path quoted,
/// but rejecting list separators, quotes, newlines and deferred variables here
/// keeps a declaration from changing CMake syntax or from acquiring a
/// machine-local meaning later.
fn safe_copy_directory_component(component: &str) -> bool {
    !component.is_empty()
        && component.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | ' ' | '+' | '@')
        })
}

/// Normalises a path already rooted in one CMake variable.
fn normalize_cmake_copy_directory_path(raw: &str) -> Option<String> {
    let raw = normalize_copy_directory_root_alias(raw.trim());
    let root = COPY_DIRECTORY_CMAKE_ROOTS.iter().find(|root| {
        raw == ***root
            || raw
                .strip_prefix(**root)
                .is_some_and(|tail| tail.starts_with('/'))
    })?;
    let tail = raw.strip_prefix(*root).unwrap_or_default();
    let mut components = Vec::new();
    for component in tail.split('/') {
        match component {
            "" | "." => {}
            // A CMake-rooted path has no lexical source-tree owner above its
            // root.  Rejecting this is both stricter and clearer than relying
            // on CMake's normalisation after a variable expansion.
            ".." => return None,
            value if safe_copy_directory_component(value) => components.push(value),
            _ => return None,
        }
    }
    if components.is_empty() {
        Some((*root).to_owned())
    } else {
        Some(format!("{root}/{}", components.join("/")))
    }
}

/// Normalises one path relative to the declaring mmakefile directory.
fn normalize_relative_copy_directory_path(raw: &str, relative_dir: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in relative_dir.components() {
        let Component::Normal(value) = component else {
            return None;
        };
        components.push(value.to_str()?.to_owned());
    }
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value if safe_copy_directory_component(value) => components.push(value.to_owned()),
            _ => return None,
        }
    }
    if components.is_empty() {
        Some("${CMAKE_SOURCE_DIR}".to_owned())
    } else {
        Some(format!("${{CMAKE_SOURCE_DIR}}/{}", components.join("/")))
    }
}

/// Renders a `%copy_dir_recursive` path at the declaration site.
fn render_copy_directory_path(
    raw: &str,
    context: &MakeExprContext<'_>,
    relative_dir: &Path,
) -> std::result::Result<String, String> {
    let value = evaluate_make_expr(raw, context)
        .map_err(|error| format!("cannot evaluate `{raw}`: {error}"))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("`{raw}` expands to an empty path"));
    }
    if value.starts_with("${") {
        return normalize_cmake_copy_directory_path(value)
            .ok_or_else(|| format!("`{value}` is not a safe CMake-rooted path"));
    }
    if value.starts_with('/') || value.contains('$') {
        return Err(format!("`{value}` is not a safe source-tree-relative path"));
    }
    normalize_relative_copy_directory_path(value, relative_dir)
        .ok_or_else(|| format!("`{value}` escapes or is not a safe source-tree-relative path"))
}

fn copy_directory_source_is_owned_path(path: &str) -> bool {
    ["${CMAKE_SOURCE_DIR}", "${AROS_PORTS_DIR}"]
        .iter()
        .any(|root| {
            path == *root
                || path
                    .strip_prefix(root)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
}

/// Ensures that a source-tree copy names a real directory below the checked
/// out tree.  Port paths deliberately cannot be tested here: their owner is
/// fetched at build time and may be absent during a clean configure.
fn in_tree_copy_directory_source_is_safe(path: &str, source_root: &Path) -> bool {
    let Some(tail) = path.strip_prefix("${CMAKE_SOURCE_DIR}") else {
        return true;
    };
    if !tail.is_empty() && !tail.starts_with('/') {
        return false;
    }
    let Ok(root) = fs::canonicalize(source_root) else {
        return false;
    };
    let Ok(candidate) = fs::canonicalize(root.join(tail.trim_start_matches('/'))) else {
        return false;
    };
    candidate.is_dir() && candidate.starts_with(root)
}

fn copy_directory_destination_is_build_path(path: &str) -> bool {
    [
        "${CMAKE_BINARY_DIR}",
        "${AROS_BUILD_DIR}",
        "${AROS_SDK_INCLUDE_DIR}",
        "${AROS_GENINC_DIR}",
    ]
    .iter()
    .any(|root| {
        path == *root
            || path
                .strip_prefix(root)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

fn valid_copy_directory_target_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

/// Extracts the bounded `%copy_dir_recursive` capability.
///
/// The legacy macro accepts a general recursive copy and therefore could
/// receive arbitrary source/destination text.  Only declarations whose paths
/// reduce to a source-tree or known fetched-port root and to a build-owned
/// destination are admitted.  This means the CMake graph has a concrete owner
/// for every product rather than a configure-time host leak.
fn collect_copy_directories(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<CopyDirectoryDecl>, Vec<String>) {
    let file = if relative_dir.as_os_str().is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{}/mmakefile.src", relative_dir.display())
    };
    let mut declarations = Vec::new();
    let mut skipped = Vec::new();

    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "copy_dir_recursive")
    {
        match line_states
            .and_then(|states| states.get(invocation.line))
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive is guarded by an unresolved Make conditional",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            ConditionalTruth::True => {}
        }

        let names = macro_argument_names(&invocation.args);
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();
        let has_only_supported_arguments = unique_names
            .iter()
            .all(|name| matches!(name.as_str(), "mmake" | "src" | "dst" | "excludefiles"));
        if names.len() != unique_names.len() || !has_only_supported_arguments {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive has unsupported or duplicate arguments",
                file,
                invocation.line + 1
            ));
            continue;
        }

        let Some(name) = macro_arg(&invocation.args, "mmake") else {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive has no concrete mmake= owner",
                file,
                invocation.line + 1
            ));
            continue;
        };
        if !valid_copy_directory_target_name(&name) {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} is not a concrete target name",
                file,
                invocation.line + 1
            ));
            continue;
        }
        if macro_arg(&invocation.args, "excludefiles").is_some_and(|value| !value.trim().is_empty())
        {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} uses excludefiles=, which has no audited CMake equivalent",
                file,
                invocation.line + 1
            ));
            continue;
        }

        let source_raw = macro_arg(&invocation.args, "src").unwrap_or_else(|| ".".to_owned());
        let Some(destination_raw) = macro_arg(&invocation.args, "dst") else {
            skipped.push(format!(
                "{}:{}: %copy_dir_recursive mmake={name} has no dst=",
                file,
                invocation.line + 1
            ));
            continue;
        };
        let context = MakeExprContext::new(scope, dirs, invocation.line, root, relative_dir);
        let source = match render_copy_directory_path(&source_raw, &context, relative_dir) {
            Ok(value) if !copy_directory_source_is_owned_path(&value) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} source {value} has no source-tree or port owner",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Ok(value) if !in_tree_copy_directory_source_is_safe(&value, root) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} source {value} is not a real in-tree directory",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Ok(value) => value,
            Err(reason) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} src={source_raw} {reason}",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
        };
        let destination = match render_copy_directory_path(&destination_raw, &context, relative_dir)
        {
            Ok(value) if copy_directory_destination_is_build_path(&value) => value,
            Ok(value) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} destination {value} is not build-owned",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
            Err(reason) => {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive mmake={name} dst={destination_raw} {reason}",
                    file,
                    invocation.line + 1
                ));
                continue;
            }
        };

        declarations.push(CopyDirectoryDecl {
            name,
            source,
            destination,
            file: file.clone(),
            line: invocation.line + 1,
            dependencies: Vec::new(),
        });
    }

    (declarations, skipped)
}

/// Canonicalises one audited Make capability block for exact comparison.
///
/// Continuation layout and comments have no GNU Make semantics, so they may
/// vary without changing the capability. Every remaining logical line,
/// conditional and assignment is retained in order with whitespace collapsed.
fn normalized_make_capability_block(
    content: &str,
    first_line_prefix: &str,
    end_line_prefix: &str,
) -> Option<String> {
    let joined = join_continuations(content);
    let mut active = false;
    let mut lines = Vec::new();
    for raw_line in joined.lines() {
        let semantic = strip_make_comment(raw_line).trim();
        if !active {
            if !semantic.starts_with(first_line_prefix) {
                continue;
            }
            active = true;
        } else if semantic.starts_with(end_line_prefix) {
            return Some(lines.join("\n"));
        }
        if !semantic.is_empty() {
            lines.push(semantic.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    None
}

const AOM_DECLARED_CAPABILITY: &str = "\
LIBAOM_CMAKEOPTIONS := -DBUILD_SHARED_LIBS=OFF -DENABLE_NASM=ON -DENABLE_EXAMPLES=OFF -DENABLE_TESTS=OFF -DENABLE_TOOLS=OFF -DCONFIG_AV1_ENCODER=0 -DCONFIG_AV1_DECODER=1 -DCONFIG_MULTITHREAD=0\n\
ifneq (,$(findstring x86_64,$(AROS_TARGET_CPU)))\n\
ifeq ($(NASM),)\n\
LIBAOM_TARGET_CPU=generic\n\
endif\n\
else\n\
ifneq (,$(findstring i386,$(AROS_TARGET_CPU)))\n\
ifeq ($(NASM),)\n\
LIBAOM_TARGET_CPU=generic\n\
endif\n\
endif\n\
endif\n\
ifeq ($(LIBAOM_TARGET_CPU),)\n\
LIBAOM_CMAKEOPTIONS += -DAOM_TARGET_CPU=$(AROS_TARGET_CPU)\n\
else\n\
LIBAOM_CMAKEOPTIONS += -DAOM_TARGET_CPU=$(LIBAOM_TARGET_CPU)\n\
endif\n\
ifneq (,$(findstring arm,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
LIBAOM_CMAKEOPTIONS += -DENABLE_NEON=0\n\
endif\n\
ifneq (,$(findstring riscv64,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
else\n\
ifneq (,$(findstring riscv,$(AROS_TARGET_CPU)))\n\
LIBAOM_CMAKEOPTIONS += -DENABLE_RVV=0\n\
AOM_NOCPUDETECT=yes\n\
endif\n\
endif\n\
ifneq (,$(findstring ppc,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
endif\n\
ifeq ($(AOM_NOCPUDETECT),yes)\n\
LIBAOM_CMAKEOPTIONS += -DCONFIG_RUNTIME_CPU_DETECT=0\n\
endif\n\
LIBAOM_LDFLAGS+=$(TARGET_CXX_LDFLAGS)\n\
ifneq ($(TARGET_CXX_LIBS),)\n\
LIBAOM_LDFLAGS+=-Wl,--start-group $(TARGET_CXX_LIBS) -Wl,--end-group\n\
endif";

const AOM_COMMON_OPTIONS: &[&str] = &[
    "-DBUILD_SHARED_LIBS=OFF",
    "-DENABLE_NASM=ON",
    "-DENABLE_EXAMPLES=OFF",
    "-DENABLE_TESTS=OFF",
    "-DENABLE_TOOLS=OFF",
    "-DCONFIG_AV1_ENCODER=0",
    "-DCONFIG_AV1_DECODER=1",
    "-DCONFIG_MULTITHREAD=0",
    // config/make-cmake.tmpl supplies this legacy default outside
    // LIBAOM_CMAKEOPTIONS. Make it explicit in the standalone capability.
    "-DCMAKE_BUILD_TYPE=Release",
];

fn aom_profile_options(target: Option<&TargetContext>) -> std::result::Result<Vec<String>, String> {
    let Some(target) = target else {
        return Err("AOM capability requires a concrete target profile".to_owned());
    };
    let profile = (
        target.cpu.as_deref(),
        target.platform.as_deref(),
        target.toolchain.as_deref(),
        target.cpu32.as_deref(),
        target.use_mmu.as_deref(),
        target.float_abi.as_deref(),
    );
    let specific: &[&str] = match profile {
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => &[
            "-DAOM_TARGET_CPU=arm",
            "-DENABLE_NEON=0",
            "-DCONFIG_RUNTIME_CPU_DETECT=0",
        ],
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some(""))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            // The legacy expression expands to `aarch64`, but the audited
            // migration contract deliberately retains the probe-proven scalar
            // configuration shared with the reproducible x86_64 profile.
            &["-DAOM_TARGET_CPU=generic"]
        }
        _ => {
            return Err(format!(
                "AOM capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                target.cpu.as_deref().unwrap_or("<unset>"),
                target.platform.as_deref().unwrap_or("<unset>"),
                target.toolchain.as_deref().unwrap_or("<unset>"),
                target.cpu32.as_deref().unwrap_or("<unset>"),
                target.use_mmu.as_deref().unwrap_or("<unset>"),
                target.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };
    Ok(AOM_COMMON_OPTIONS
        .iter()
        .chain(specific)
        .map(|option| (*option).to_owned())
        .collect())
}

fn parse_aom_external_cmake_invocation(
    invocation: &Invocation,
    expression_context: &MakeExprContext<'_>,
    target: Option<&TargetContext>,
    make_source: &str,
    fetches: &[FetchDecl],
    relative_dir: &Path,
    mmake: String,
) -> std::result::Result<ExternalCMakeDecl, String> {
    const AOM_FETCH: &str = "linklibs-aom-fetch";
    const AOM_SOURCE: &str = "${AROS_PORTS_DIR}/libaom/libaom-3.12.1";
    const AOM_PREFIX: &str = "${AROS_BUILD_DIR}/SYS/Developer";

    let argument_names = macro_argument_names(&invocation.args);
    let mut unique_names = argument_names.clone();
    unique_names.sort();
    unique_names.dedup();
    if unique_names.len() != argument_names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = vec![
        "extraoptions",
        "extraldflags",
        "mmake",
        "package",
        "prefix",
        "srcdir",
    ];
    expected_names.sort_unstable();
    if unique_names != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited AOM capability [{}]",
            unique_names.join(", "),
            expected_names.join(", ")
        ));
    }

    for (key, expected) in [
        ("package", "aom"),
        ("srcdir", "$(AOMARCHSRCDIR)"),
        ("prefix", "$(AROS_DEVELOPER)"),
        ("extraoptions", "$(LIBAOM_CMAKEOPTIONS)"),
        ("extraldflags", "$(LIBAOM_LDFLAGS)"),
    ] {
        let actual = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        if actual != expected {
            return Err(format!(
                "{key} uses `{actual}`, expected audited form `{expected}`"
            ));
        }
    }

    let declared_block = normalized_make_capability_block(
        make_source,
        "LIBAOM_CMAKEOPTIONS :=",
        "%build_with_cmake",
    )
    .ok_or_else(|| "AOM option/extraldflags capability block is missing".to_owned())?;
    if declared_block != AOM_DECLARED_CAPABILITY {
        return Err(
            "AOM option/extraldflags declaration block differs from audited capability".to_owned(),
        );
    }

    let evaluate_path = |key: &str| -> std::result::Result<String, String> {
        let raw = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        let value = evaluate_make_expr(&raw, expression_context)
            .map_err(|reason| format!("{key}={raw} cannot be evaluated: {reason}"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{key}={raw} expanded to an empty value"));
        }
        Ok(value.to_owned())
    };
    let source_dir = evaluate_path("srcdir")?;
    if source_dir != AOM_SOURCE {
        return Err(format!(
            "srcdir resolves to {source_dir}, expected {AOM_SOURCE}"
        ));
    }
    let install_prefix = evaluate_path("prefix")?;
    if install_prefix != AOM_PREFIX {
        return Err(format!(
            "prefix resolves to {install_prefix}, expected {AOM_PREFIX}"
        ));
    }
    let options = aom_profile_options(target)?;

    let matching_fetches: Vec<_> = fetches
        .iter()
        .filter(|fetch| fetch.name == AOM_FETCH)
        .collect();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={AOM_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    for (field, actual, expected) in [
        ("archive", fetch.archive.as_str(), "libaom-3.12.1"),
        ("suffixes", fetch.suffixes.as_str(), "tar.gz"),
        (
            "archive_origins",
            fetch.origins.as_str(),
            "https://storage.googleapis.com/aom-releases",
        ),
        (
            "location",
            fetch.location.as_str(),
            "${AROS_PORTS_SOURCE_DIR}",
        ),
        (
            "destination",
            fetch.destination.as_str(),
            "${AROS_PORTS_DIR}/libaom",
        ),
        (
            "patches_specs",
            fetch.patches.as_str(),
            "libaom-3.12.1-aros.diff:libaom-3.12.1:-f,-p1",
        ),
        (
            "patches_origins",
            fetch.patch_origins.as_str(),
            "${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic",
        ),
        ("base", fetch.base.as_str(), ""),
        (
            "declaring directory",
            fetch.dir.as_str(),
            "workbench/classes/datatypes/heic",
        ),
    ] {
        if actual != expected {
            return Err(format!(
                "%fetch mmake={AOM_FETCH} {field} is {actual}, expected {expected}"
            ));
        }
    }

    Ok(ExternalCMakeDecl {
        mmake_name: mmake,
        source_dir,
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/workbench/classes/datatypes/heic/aom"
            .to_owned(),
        install_prefix: install_prefix.clone(),
        fetch_target: AOM_FETCH.to_owned(),
        source_archive: "${AROS_PORTS_SOURCE_DIR}/libaom-3.12.1.tar.gz".to_owned(),
        source_sha256: "9e9775180dec7dfd61a79e00bda3809d43891aee6b2e331ff7f26986207ea22e"
            .to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic/libaom-3.12.1-aros.diff"
                .to_owned(),
        ],
        local_patch_sha256: vec![
            "c3caf62de4cd3524ddcf7c1b0111909c6d0f44081200324ab12090fcd8fb48ce".to_owned(),
        ],
        provided_library: "aom".to_owned(),
        provider_target: "datatypes-heic-linklibs-aom-external-aom".to_owned(),
        library_products: vec![format!("{install_prefix}/lib/libaom.a")],
        header_products: [
            "aom.h",
            "aom_codec.h",
            "aom_decoder.h",
            "aom_frame_buffer.h",
            "aom_image.h",
            "aom_integer.h",
            "aomdx.h",
        ]
        .into_iter()
        .map(|header| format!("{install_prefix}/include/aom/{header}"))
        .collect(),
        auxiliary_products: vec![format!("{install_prefix}/lib/pkgconfig/aom.pc")],
        public_include_dirs: vec![format!("{install_prefix}/include")],
        options,
        dir_path: relative_dir.to_path_buf(),
    })
}

/// Parses one deliberately narrow `%build_with_cmake` capability.
///
/// Generic external-project passthrough would let a newly added host compiler,
/// source tree or install prefix silently execute in target builds. Each
/// admitted declaration must match its complete audited arguments, owning
/// fetch and target-profile contract. Everything else remains an explicit
/// skip with a precise diagnostic.
fn parse_external_cmake_invocation(
    invocation: &Invocation,
    expression_context: &MakeExprContext<'_>,
    relative_dir: &Path,
    fetches: &[FetchDecl],
    target: Option<&TargetContext>,
    make_source: &str,
) -> std::result::Result<ExternalCMakeDecl, String> {
    const CUNIT_MMAKE: &str = "linklibs-yes-cunit";
    const CUNIT_SOURCE: &str = "${AROS_PORTS_DIR}/cunit/cunit-3.5.5";
    const CUNIT_PREFIX: &str = "${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras";
    const CUNIT_FETCH: &str = "cunit-fetch";
    const CUNIT_ARCHIVE: &str = "${AROS_PORTS_SOURCE_DIR}/cunit-3.5.5.tar.bz2";
    const CUNIT_SHA256: &str = "a0a49b37c731303168481f387bb551b8381422d1b447d32f9e558293ceea9a10";
    const DECLARED_OPTIONS: &[&str] = &[
        "-DCUNIT_DISABLE_EXAMPLES=yes",
        "-DCUNIT_DISABLE_TESTS=yes",
        "-DCMAKE_BUILD_TYPE=DEBUG",
        "-Wno-error=dev",
    ];

    let mmake_raw = macro_arg(&invocation.args, "mmake")
        .ok_or_else(|| "missing required mmake= argument".to_owned())?;
    let mmake = evaluate_name(&mmake_raw, expression_context)
        .map_err(|reason| format!("mmake={mmake_raw} is unresolved: {reason}"))?;
    if relative_dir == Path::new("workbench/classes/datatypes/heic")
        && mmake == "datatypes-heic-linklibs-aom"
    {
        return parse_aom_external_cmake_invocation(
            invocation,
            expression_context,
            target,
            make_source,
            fetches,
            relative_dir,
            mmake,
        );
    }
    if relative_dir != Path::new("compiler/cunit") || mmake != CUNIT_MMAKE {
        return Err(format!(
            "unsupported external-CMake capability (modelled: compiler/cunit mmake={CUNIT_MMAKE}; workbench/classes/datatypes/heic mmake=datatypes-heic-linklibs-aom)"
        ));
    }

    let argument_names = macro_argument_names(&invocation.args);
    let mut unique_names = argument_names.clone();
    unique_names.sort();
    unique_names.dedup();
    if unique_names.len() != argument_names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = vec!["extraoptions", "mmake", "prefix", "srcdir"];
    expected_names.sort_unstable();
    if unique_names != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited CUnit capability [{}]",
            unique_names.join(", "),
            expected_names.join(", ")
        ));
    }

    let evaluate_path = |key: &str| -> std::result::Result<String, String> {
        let raw = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        let value = evaluate_make_expr(&raw, expression_context)
            .map_err(|reason| format!("{key}={raw} cannot be evaluated: {reason}"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{key}={raw} expanded to an empty value"));
        }
        Ok(value.to_owned())
    };
    let source_dir = evaluate_path("srcdir")?;
    if source_dir != CUNIT_SOURCE {
        return Err(format!(
            "srcdir resolves to {source_dir}, expected {CUNIT_SOURCE}"
        ));
    }
    let install_prefix = evaluate_path("prefix")?;
    if install_prefix != CUNIT_PREFIX {
        return Err(format!(
            "prefix resolves to {install_prefix}, expected {CUNIT_PREFIX}"
        ));
    }

    let options_raw = macro_arg(&invocation.args, "extraoptions")
        .ok_or_else(|| "missing required extraoptions= argument".to_owned())?;
    let options = evaluate_make_list(&options_raw, expression_context)
        .map_err(|reason| format!("extraoptions={options_raw} cannot be evaluated: {reason}"))?;
    if options != DECLARED_OPTIONS {
        return Err(format!(
            "extraoptions resolve to [{}], expected [{}]",
            options.join(" "),
            DECLARED_OPTIONS.join(" ")
        ));
    }

    let matching_fetches: Vec<_> = fetches
        .iter()
        .filter(|fetch| fetch.name == CUNIT_FETCH)
        .collect();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={CUNIT_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    for (field, actual, expected) in [
        ("archive", fetch.archive.as_str(), "cunit-3.5.5"),
        ("suffixes", fetch.suffixes.as_str(), "tar.bz2"),
        (
            "location",
            fetch.location.as_str(),
            "${AROS_PORTS_SOURCE_DIR}",
        ),
        (
            "destination",
            fetch.destination.as_str(),
            "${AROS_PORTS_DIR}/cunit",
        ),
        (
            "patches_specs",
            fetch.patches.as_str(),
            "cunit-3.5.5-aros.diff:cunit-3.5.5:-f,-p1",
        ),
        (
            "patches_origins",
            fetch.patch_origins.as_str(),
            "${CMAKE_SOURCE_DIR}/compiler/cunit",
        ),
        ("base", fetch.base.as_str(), ""),
        ("declaring directory", fetch.dir.as_str(), "compiler/cunit"),
    ] {
        if actual != expected {
            return Err(format!(
                "%{CUNIT_FETCH} {field} is {actual}, expected {expected}"
            ));
        }
    }

    Ok(ExternalCMakeDecl {
        mmake_name: mmake,
        source_dir,
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/compiler/cunit".to_owned(),
        install_prefix: install_prefix.clone(),
        fetch_target: CUNIT_FETCH.to_owned(),
        source_archive: CUNIT_ARCHIVE.to_owned(),
        source_sha256: CUNIT_SHA256.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/compiler/cunit/cunit-3.5.5-aros.diff".to_owned()
        ],
        local_patch_sha256: vec![
            "481b9d4544e7fae9f47dc821f343cbb5f417ea8abc76c1a8f9f9177ab7420197".to_owned(),
        ],
        provided_library: "cunit".to_owned(),
        provider_target: "linklibs-yes-cunit-external-cunit".to_owned(),
        library_products: vec![format!("{install_prefix}/lib/libcunit.a")],
        header_products: [
            "Automated.h",
            "AutomatedJUnitXml.h",
            "Basic.h",
            "CUAssert.h",
            "CUCurses.h",
            "CUError.h",
            "CUnit.h",
            "CUnitCI.h",
            "CUnitCITypes.h",
            "CUnit_intl.h",
            "Console.h",
            "MessageHandlers.h",
            "MyMem.h",
            "Simple.h",
            "TestDB.h",
            "TestFixture.h",
            "TestRun.h",
            "Util.h",
            "wxWidget.h",
        ]
        .into_iter()
        .map(|header| format!("{install_prefix}/include/CUnit/{header}"))
        .collect(),
        // CUnit also installs build-system source files and CMake package
        // metadata, but no AROS target consumes them. Only public, repaired
        // capability products belong in this contract.
        auxiliary_products: Vec::new(),
        public_include_dirs: vec![format!("{install_prefix}/include")],
        options: vec![
            "-DCUNIT_DISABLE_EXAMPLES=yes".to_owned(),
            "-DCUNIT_DISABLE_TESTS=yes".to_owned(),
            "-DCMAKE_BUILD_TYPE=DEBUG".to_owned(),
            "-Wno-error=dev".to_owned(),
        ],
        dir_path: relative_dir.to_path_buf(),
    })
}

const ADFLIB_CONFIGURE_DIR: &str = "tools/ADFlib";
const ADFLIB_CONFIGURE_MANIFEST: &str = "tools/ADFlib/adflib-configure.inputs";
const ADFLIB_CONFIGURE_MANIFEST_SHA256: &str =
    "a63a7498752d68175093b94dba873cc8a343d75179feec5cd6a6020e56e779a5";
const WIRELESS_CONFIGURE_DIR: &str = "workbench/network/WirelessManager/wpa_supplicant";
const WIRELESS_CONFIGURE_SOURCE_ROOT: &str = "workbench/network/WirelessManager";
const WIRELESS_CONFIGURE_MANIFEST: &str =
    "workbench/network/WirelessManager/wirelessmanager-configure.inputs";
const WIRELESS_CONFIGURE_MANIFEST_SHA256: &str =
    "27e629e694f6cbc8dd036f7b188604fd03ed901d181962db143a0789512af760";
const AHI_CONFIGURE_DIR: &str = "workbench/devs/AHI";
const AHI_CONFIGURE_MMAKE_SHA256: &str =
    "c1a539d23bf935cce2b0c097b83faeb89d2597970e6380bb6c0c39d3abff1385";
const GRUB2_HOST_DIR: &str = "arch/all-pc/boot/grub2-host";
const GRUB2_HOST_MMAKE_SHA256: &str =
    "66c464606f16a8ce594aac96875498beb9429836c3036095193a3e135e5b85f8";
const GRUB2_AROS_MMAKE_SHA256: &str =
    "74cf4a179dd75163c40f6931bab1521d9cfe251a09552d76abac8e98eb640f83";
const GRUB2_VERSION_FILE_SHA256: &str =
    "f487eb5226f64295c10fa7fcd3aba777dd763d2a0d2e58d9e2e43cf5f3023d0c";

const ADFLIB_PUBLIC_HEADERS: &[&str] = &[
    "adf_defs.h",
    "adf_blk.h",
    "adf_err.h",
    "adf_str.h",
    "adflib.h",
    "adf_bitm.h",
    "adf_cache.h",
    "adf_dir.h",
    "adf_disk.h",
    "adf_dump.h",
    "adf_env.h",
    "adf_file.h",
    "adf_hd.h",
    "adf_link.h",
    "adf_raw.h",
    "adf_salv.h",
    "adf_util.h",
    "defendian.h",
    "hd_blk.h",
    "prefix.h",
    "adf_nativ.h",
];

/// Verifies the complete source allowlist and every content digest before a
/// configure-style capability is admitted.  The same manifest is checked and
/// staged by CMake at build time; doing it here too ensures source drift turns
/// the declaration back into an explicit skip on the next configure.
fn configure_input_manifest_is_pinned(
    root: &Path,
    source_dir: &str,
    manifest: &str,
    expected_manifest_sha256: &str,
) -> std::result::Result<(), String> {
    let manifest_path = root.join(manifest);
    let bytes = fs::read(&manifest_path)
        .map_err(|reason| format!("cannot read input manifest {manifest}: {reason}"))?;
    let actual_manifest_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if actual_manifest_sha256 != expected_manifest_sha256 {
        return Err(format!(
            "input manifest digest is {actual_manifest_sha256}, expected {expected_manifest_sha256}"
        ));
    }
    let body = std::str::from_utf8(&bytes)
        .map_err(|_| format!("input manifest {manifest} is not UTF-8"))?;
    let source_root = root.join(source_dir);
    let mut paths = HashSet::new();
    let mut count = 0usize;
    for (index, line) in body.lines().enumerate() {
        let (digest, relative) = line.split_once("  ").ok_or_else(|| {
            format!(
                "input manifest {manifest}:{} is not `<sha256>  <relative-path>`",
                index + 1
            )
        })?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(format!(
                "input manifest {manifest}:{} has an invalid SHA-256",
                index + 1
            ));
        }
        let path = Path::new(relative);
        if relative.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "input manifest {manifest}:{} has an unsafe path `{relative}`",
                index + 1
            ));
        }
        if !paths.insert(relative.to_owned()) {
            return Err(format!(
                "input manifest {manifest}:{} repeats `{relative}`",
                index + 1
            ));
        }
        let input = source_root.join(path);
        let input_bytes = fs::read(&input).map_err(|reason| {
            format!(
                "input manifest {manifest}:{} cannot read {relative}: {reason}",
                index + 1
            )
        })?;
        let actual = format!("{:x}", Sha256::digest(&input_bytes));
        if actual != digest {
            return Err(format!(
                "input manifest {manifest}:{} digest for {relative} is {actual}, expected {digest}",
                index + 1
            ));
        }
        count += 1;
    }
    if count == 0 {
        return Err(format!("input manifest {manifest} is empty"));
    }
    Ok(())
}

fn configure_profile_is_supported(
    target: Option<&TargetContext>,
) -> std::result::Result<(), String> {
    let Some(profile) = target else {
        return Err("configure-style capability requires a concrete target profile".to_owned());
    };
    let key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    match key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some(""))
        | (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (
            Some("aarch64"),
            Some("raspi"),
            Some("llvm"),
            Some(""),
            Some("1"),
            Some(""),
        ) => Ok(()),
        _ => Err(format!(
            "configure-style capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

fn require_exact_macro_arguments(
    invocation: &Invocation,
    expected: &[(&str, &str)],
) -> std::result::Result<(), String> {
    let names = macro_argument_names(&invocation.args);
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = expected
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    expected_names.sort();
    if unique != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited capability [{}]",
            unique.join(", "),
            expected_names.join(", ")
        ));
    }
    for (name, expected_value) in expected {
        // MetaMake accepts a literal empty `key=` argument. `macro_arg`
        // intentionally returns None for it, because ordinary consumers use
        // absence and emptiness equivalently. Closed capabilities sometimes
        // need to distinguish it, however: the name-set check above proves
        // the key exists, and this branch proves it has no value.
        if expected_value.is_empty() {
            if let Some(actual) = macro_arg(&invocation.args, name) {
                return Err(format!(
                    "{name} uses `{actual}`, expected an audited empty argument"
                ));
            }
            continue;
        }
        let actual = macro_arg(&invocation.args, name)
            .ok_or_else(|| format!("missing required {name}= argument"))?;
        if actual != *expected_value {
            return Err(format!(
                "{name} uses `{actual}`, expected audited form `{expected_value}`"
            ));
        }
    }
    Ok(())
}

/// Parses the deliberately small, local-source subset of
/// `%build_with_configure`.  No legacy command/environment text is forwarded:
/// a target is admitted only when its identity, argument set, profile and full
/// source manifest match one of these closed contracts.
fn parse_configure_build_invocation(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<ConfigureBuildDecl, String> {
    configure_profile_is_supported(target)?;
    let mmake = macro_arg(&invocation.args, "mmake")
        .ok_or_else(|| "missing required mmake= argument".to_owned())?;

    if relative_dir == Path::new(ADFLIB_CONFIGURE_DIR) && mmake == "host-adflib" {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "host-adflib"),
                ("compiler", "host"),
                ("prefix", "$(CROSSTOOLSDIR)"),
            ],
        )?;
        configure_input_manifest_is_pinned(
            root,
            ADFLIB_CONFIGURE_DIR,
            ADFLIB_CONFIGURE_MANIFEST,
            ADFLIB_CONFIGURE_MANIFEST_SHA256,
        )?;
        let binary_dir = "${AROS_BUILD_DIR}/gen/configure/tools/ADFlib/host".to_owned();
        let install_prefix = "${AROS_BUILD_DIR}/hosttools".to_owned();
        let mut install_products = vec![format!("{install_prefix}/lib/libadf.a")];
        install_products.extend(
            ADFLIB_PUBLIC_HEADERS
                .iter()
                .map(|header| format!("{install_prefix}/include/{header}")),
        );
        install_products.push(format!("{install_prefix}/lib/pkgconfig/adflib.pc"));
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "adflib-host".to_owned(),
            source_dir: "${CMAKE_SOURCE_DIR}/tools/ADFlib".to_owned(),
            binary_dir: binary_dir.clone(),
            install_prefix,
            input_manifest: "${CMAKE_SOURCE_DIR}/tools/ADFlib/adflib-configure.inputs".to_owned(),
            input_manifest_sha256: ADFLIB_CONFIGURE_MANIFEST_SHA256.to_owned(),
            private_products: vec![format!("{binary_dir}/build/libadf.a")],
            install_products,
            dependency_products: Vec::new(),
            provided_library: None,
            provider_target: None,
            dir_path: relative_dir.to_path_buf(),
        });
    }

    if relative_dir == Path::new(ADFLIB_CONFIGURE_DIR) && mmake == "linklib-adflib" {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "linklib-adflib"),
                ("prefix", "$(AROS_DEVELOPER)"),
                ("extraoptions", "$(AROSADFLIB_OPTS)"),
                ("config_env_extra", "$(AROSADFLIB_ENV)"),
                ("use_build_env", "yes"),
                ("nlsflag", "no"),
                ("xflag", "no"),
            ],
        )?;
        configure_input_manifest_is_pinned(
            root,
            ADFLIB_CONFIGURE_DIR,
            ADFLIB_CONFIGURE_MANIFEST,
            ADFLIB_CONFIGURE_MANIFEST_SHA256,
        )?;
        let binary_dir = "${AROS_BUILD_DIR}/gen/configure/tools/ADFlib/target".to_owned();
        let install_prefix = "${AROS_BUILD_DIR}/SYS/Developer".to_owned();
        let mut install_products = vec![format!("{install_prefix}/lib/libadf.a")];
        install_products.extend(
            ADFLIB_PUBLIC_HEADERS
                .iter()
                .map(|header| format!("{install_prefix}/include/{header}")),
        );
        install_products.push(format!("{install_prefix}/lib/pkgconfig/adflib.pc"));
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "adflib-target".to_owned(),
            source_dir: "${CMAKE_SOURCE_DIR}/tools/ADFlib".to_owned(),
            binary_dir: binary_dir.clone(),
            install_prefix,
            input_manifest: "${CMAKE_SOURCE_DIR}/tools/ADFlib/adflib-configure.inputs".to_owned(),
            input_manifest_sha256: ADFLIB_CONFIGURE_MANIFEST_SHA256.to_owned(),
            private_products: vec![format!("{binary_dir}/build/libadf.a")],
            install_products,
            dependency_products: Vec::new(),
            provided_library: Some("adf".to_owned()),
            provider_target: Some("linklib-adflib-configure-adf".to_owned()),
            dir_path: relative_dir.to_path_buf(),
        });
    }

    if relative_dir == Path::new(WIRELESS_CONFIGURE_DIR)
        && mmake == "workbench-network-wirelessmanager"
    {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "workbench-network-wirelessmanager"),
                ("install_env", "BINDIR=$(AROS_C)"),
                ("use_build_env", "yes"),
            ],
        )?;
        configure_input_manifest_is_pinned(
            root,
            WIRELESS_CONFIGURE_SOURCE_ROOT,
            WIRELESS_CONFIGURE_MANIFEST,
            WIRELESS_CONFIGURE_MANIFEST_SHA256,
        )?;
        let binary_dir =
            "${AROS_BUILD_DIR}/gen/configure/workbench/network/WirelessManager".to_owned();
        let private_root = format!("{binary_dir}/source/wpa_supplicant");
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "wirelessmanager".to_owned(),
            source_dir:
                "${CMAKE_SOURCE_DIR}/workbench/network/WirelessManager".to_owned(),
            binary_dir,
            install_prefix: "${AROS_BUILD_DIR}/SYS".to_owned(),
            input_manifest: "${CMAKE_SOURCE_DIR}/workbench/network/WirelessManager/wirelessmanager-configure.inputs".to_owned(),
            input_manifest_sha256: WIRELESS_CONFIGURE_MANIFEST_SHA256.to_owned(),
            private_products: ["wpa_supplicant", "wpa_passphrase", "wpa_cli"]
                .into_iter()
                .map(|product| format!("{private_root}/{product}"))
                .collect(),
            install_products: vec!["${AROS_BUILD_DIR}/SYS/C/WirelessManager".to_owned()],
            dependency_products: vec!["${AROS_BUILD_DIR}/liblinklibs-mui.a".to_owned()],
            provided_library: None,
            provider_target: None,
            dir_path: relative_dir.to_path_buf(),
        });
    }

    Err(format!(
        "unsupported configure-style capability (modelled: tools/ADFlib mmake=host-adflib,linklib-adflib; workbench/network/WirelessManager/wpa_supplicant mmake=workbench-network-wirelessmanager)"
    ))
}

/// Parses the one current AHI subsystem declaration without turning the
/// legacy `%build_with_configure` macro into a general command runner.
///
/// The AHI helper owns its fixed local source closure, complete products and
/// tool contract.  The transpiler only accepts the exact audited mmakefile
/// and macro shape, selects a supported target profile, and passes the two
/// already-established host-tool variables by name.
fn parse_ahi_build_invocation(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<AhiBuildDecl>, String> {
    if relative_dir != Path::new(AHI_CONFIGURE_DIR) {
        return Ok(None);
    }
    let Some(mmake) = macro_arg(&invocation.args, "mmake") else {
        return Ok(None);
    };
    if mmake != "workbench-devs-AHI-subsystem" {
        return Ok(None);
    }

    if !file_has_sha256(
        root,
        "workbench/devs/AHI/mmakefile.src",
        AHI_CONFIGURE_MMAKE_SHA256,
    ) {
        return Err("AHI subsystem mmakefile differs from the audited capability".to_owned());
    }

    let Some(profile) = target else {
        return Err("AHI subsystem capability requires a concrete target profile".to_owned());
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let mode = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => "x86_64",
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => "arm",
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => "aarch64",
        _ => {
            return Err(format!(
                "AHI subsystem capability only supports x86_64-pc, arm-raspi and aarch64-raspi LLVM profiles (cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={})",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    require_exact_macro_arguments(
        invocation,
        &[
            ("mmake", "workbench-devs-AHI-subsystem"),
            ("prefix", "$(EXEDIR)"),
            ("extraoptions", "$(AHI_OPTIONS)"),
            ("usecppflags", "no"),
            ("gnuflags", "no"),
            (
                "config_env_extra",
                "OBJCOPY=$(OBJCOPY) STRIP=$(STRIP_PLAIN)",
            ),
        ],
    )?;

    Ok(Some(AhiBuildDecl {
        mmake_name: mmake,
        mode: mode.to_owned(),
        binary_dir: format!("${{AROS_BUILD_DIR}}/gen/configure/workbench/devs/AHI/{mode}"),
        install_prefix: "${AROS_BUILD_DIR}/SYS".to_owned(),
        host_sfdc: "${AROS_HOST_SFDC}".to_owned(),
        host_perl: "${AROS_HOST_PERL}".to_owned(),
        dir_path: relative_dir.to_path_buf(),
    }))
}

/// Parses the three x86 GRUB 2.12 host-tool lanes without admitting the
/// legacy macro's open-ended configure environment.  The downstream helper
/// owns the source URL, patch, cross targets, host dependencies and complete
/// product manifests; this parser verifies that the legacy declaration is the
/// exact audited input before selecting its fixed lane roots.
fn parse_grub2_build_invocation(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<GrubBuildDecl>, String> {
    if relative_dir != Path::new(GRUB2_HOST_DIR) {
        return Ok(None);
    }
    let Some(mmake) = macro_arg(&invocation.args, "mmake") else {
        return Ok(None);
    };
    if !matches!(
        mmake.as_str(),
        "grub2-host" | "grub2-efi-host" | "grub2-efi32-host"
    ) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err("GRUB2 host-tool capability requires a concrete target profile".to_owned());
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    if profile_key
        != (
            Some("x86_64"),
            Some("pc"),
            Some("llvm"),
            Some("i386"),
            Some("1"),
            Some(""),
        )
    {
        return Err(format!(
            "GRUB2 host-tool capability only supports x86_64-pc LLVM with the i386 companion (cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={})",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        ));
    }
    if !file_has_sha256(
        root,
        "arch/all-pc/boot/grub2-host/mmakefile.src",
        GRUB2_HOST_MMAKE_SHA256,
    ) || !file_has_sha256(
        root,
        "arch/all-pc/boot/grub2-aros/mmakefile.src",
        GRUB2_AROS_MMAKE_SHA256,
    ) || !file_has_sha256(
        root,
        "arch/all-pc/boot/grub2_def",
        GRUB2_VERSION_FILE_SHA256,
    ) {
        return Err(
            "GRUB2 host, fetch-owner or version declaration differs from the audited 2.12 capability"
                .to_owned(),
        );
    }

    let (mode, lane) = match mmake.as_str() {
        "grub2-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("package", "pc"),
                    ("extraoptions", "$(GRUB2_HOST_OPTS) --with-platform=pc"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_HOST_ENV)"),
                ],
            )?;
            ("pc", "pc")
        }
        "grub2-efi-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-efi-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("touch", "no"),
                    ("package", "efi-$(AROS_TARGET_CPU)"),
                    ("extraoptions", "$(GRUB2_HOST_OPTS) --with-platform=efi"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_EFI_ENV)"),
                ],
            )?;
            ("efi64", "efi-x86_64")
        }
        "grub2-efi32-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-efi32-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("touch", "no"),
                    ("package", "efi-$(AROS_TARGET_CPU32)"),
                    ("extraoptions", "$(GRUB2_EFI32_OPTS) --with-platform=efi"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_EFI32_ENV)"),
                ],
            )?;
            ("efi32", "efi-i386")
        }
        _ => unreachable!("the GRUB2 identity was checked above"),
    };

    Ok(Some(GrubBuildDecl {
        mmake_name: mmake,
        mode: mode.to_owned(),
        binary_dir: format!("${{AROS_BUILD_DIR}}/gen/configure/arch/all-pc/boot/grub2-host/{lane}"),
        install_prefix: format!("${{AROS_BUILD_DIR}}/hosttools/grub2/{lane}"),
        dir_path: relative_dir.to_path_buf(),
    }))
}

const MESA20_SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
const MESA20_BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
const MESA20_PRIVATE_LIBDIR: &str = "${AROS_BUILD_DIR}/gen/lib/mesa20.0.8";
const MESA20_CXX_COMPAT_NEW: &str = "workbench/libs/mesa/libcompiler/cxx-compat/new";
const MESA20_CXX_COMPAT_NEW_SHA256: &str =
    "a1163dd966449e85f08deeb4775716a34c69b68831e1ac5fc75ea121814bf0ba";

/// Exact, version-pinned source lanes for the remaining Mesa 20.0.8 private
/// archives.  The adjacent manifests contain only literal upstream-relative
/// inventories; generated products are kept in separate variables so they can
/// acquire real build owners before CMake resolves the source lanes.
fn mesa20_inventory(
    root: &Path,
    relative: &str,
    variable: &str,
) -> std::result::Result<Vec<String>, String> {
    let path = root.join(relative);
    let content =
        read_source(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let joined = join_continuations(&content);
    let scope = collect_vars(&joined);
    let values = scope
        .snapshot(usize::MAX)
        .remove(variable)
        .ok_or_else(|| format!("{relative} does not define {variable}"))?;
    if values.is_empty()
        || values.iter().any(|value| {
            value.contains("$(")
                || value.contains("${")
                || value.starts_with('/')
                || value.split('/').any(|part| part == "..")
        })
    {
        return Err(format!(
            "{relative} contains an empty or unsafe {variable} inventory"
        ));
    }
    Ok(values)
}

fn mesa20_inventory_stems(
    root: &Path,
    relative: &str,
    variable: &str,
    suffix: &str,
    prefix: &str,
) -> std::result::Result<Vec<String>, String> {
    mesa20_inventory(root, relative, variable)?
        .into_iter()
        .map(|source| {
            source
                .strip_suffix(suffix)
                .map(|stem| format!("{prefix}/{stem}"))
                .ok_or_else(|| format!("{relative} {variable} entry lacks {suffix}: {source}"))
        })
        .collect()
}

fn mesa20_inventory_paths(
    root: &Path,
    relative: &str,
    variable: &str,
    suffix: &str,
    prefix: &str,
) -> std::result::Result<Vec<String>, String> {
    mesa20_inventory(root, relative, variable)?
        .into_iter()
        .map(|source| {
            if source.ends_with(suffix) {
                Ok(format!("{prefix}/{source}"))
            } else {
                Err(format!(
                    "{relative} {variable} entry lacks {suffix}: {source}"
                ))
            }
        })
        .collect()
}

fn mesa20_current_profile(
    target: Option<&TargetContext>,
) -> std::result::Result<&'static str, String> {
    let Some(profile) = target else {
        return Err("Mesa 20.0.8 archive capability requires a concrete target profile".to_owned());
    };
    match (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    ) {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok("x86_64")
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => {
            Ok("arm")
        }
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            Ok("aarch64")
        }
        _ => Err(format!(
            "Mesa 20.0.8 archive capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

struct Mesa20CompileContract {
    defines: Vec<String>,
    includes: Vec<String>,
    options: Vec<String>,
}

fn mesa20_base_defines(profile: &str) -> Vec<String> {
    let mut defines = [
        "__STDC_CONSTANT_MACROS",
        "__STDC_FORMAT_MACROS",
        "__STDC_LIMIT_MACROS",
        "_GNU_SOURCE",
        "HAVE_PTHREAD",
        "HAVE_TIMESPEC_GET",
        "POSIXC_SLOWSTACK_VAARGS",
        "USE_GCC_ATOMIC_BUILTINS",
        "HAVE_ZLIB",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if profile == "x86_64" {
        defines.extend(["USE_X86_64_ASM".to_owned(), "USE_SSE41".to_owned()]);
    }
    defines.extend(["MAPI_MODE_GLAPI".to_owned(), "MAPI_MODE_UTIL".to_owned()]);
    defines
}

fn mesa20_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<Mesa20CompileContract>, String> {
    let supported = matches!(
        (relative_dir.to_str(), mmake),
        (
            Some("workbench/libs/mesa/libcompiler"),
            "mesa3d-linklib-compiler"
        ) | (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary"
        ) | (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa")
            | (
                Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4"
            )
    );
    if !supported {
        return Ok(None);
    }
    let profile = mesa20_current_profile(target)?;
    let base = [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
    ];
    let (mut defines, includes, options) = match (relative_dir.to_str(), mmake) {
        (Some("workbench/libs/mesa/libcompiler"), "mesa3d-linklib-compiler") => (
            mesa20_base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl/glcpp",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/spirv",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl/glcpp",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/spirv",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary",
        ) => (
            mesa20_base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary/util",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary/indices",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa") => (
            mesa20_base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (
            Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
            "linklibs-gallium_vc4",
        ) if profile != "x86_64" => {
            let mut defines = mesa20_base_defines(profile);
            defines.extend(
                [
                    "GALLIUM_VC4",
                    "HAVE_STRUCT_TIMESPEC",
                    "USE_ARM_ASM",
                    "GCA_CONSUMER_MODULE",
                ]
                .into_iter()
                .map(str::to_owned),
            );
            (
                defines,
                base.into_iter()
                    .chain([
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/drm_compat",
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/vc4",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/broadcom",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/broadcom",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/include",
                    ])
                    .map(str::to_owned)
                    .collect(),
                vec!["-std=gnu99".to_owned(), "-fno-strict-aliasing".to_owned()],
            )
        }
        _ => return Ok(None),
    };
    if mmake == "mesa3d-linklib-mesa" {
        defines.extend([
            "PACKAGE_VERSION=\"20.0.8\"".to_owned(),
            "PACKAGE_BUGREPORT=\"https://bugs.freedesktop.org/enter_bug.cgi?product=Mesa\""
                .to_owned(),
        ]);
    }
    // Keep declarations deterministic even when a legacy include repeats one
    // of the common paths.
    let mut seen = HashSet::new();
    defines.retain(|define| seen.insert(define.clone()));
    Ok(Some(Mesa20CompileContract {
        defines,
        includes,
        options,
    }))
}

fn mesa20_remaining_linklib_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    let supported_declaration = matches!(
        (relative_dir.to_str(), mmake),
        (
            Some("workbench/libs/mesa/libcompiler"),
            "mesa3d-linklib-compiler"
        ) | (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary"
        ) | (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa")
            | (
                Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4"
            )
    );
    if !supported_declaration {
        return Ok(None);
    }
    let profile = mesa20_current_profile(target)?;
    let mut sources = EvaluatedSources {
        declared: true,
        ..EvaluatedSources::default()
    };
    match (relative_dir.to_str(), mmake) {
        (Some("workbench/libs/mesa/libcompiler"), "mesa3d-linklib-compiler") => {
            const MANIFEST: &str = "workbench/libs/mesa/libcompiler/compiler-20.0.8.sources";
            sources.c = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_STATIC_C_SOURCES",
                ".c",
                &format!("{MESA20_SOURCE_ROOT}/src/compiler"),
            )?;
            sources.c.extend(mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_GENERATED_C_SOURCES",
                ".c",
                &format!("{MESA20_BUILD_ROOT}/src/compiler"),
            )?);
            sources.cxx = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_STATIC_CXX_SOURCES",
                ".cpp",
                &format!("{MESA20_SOURCE_ROOT}/src/compiler"),
            )?;
            sources.cxx.extend(mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_GENERATED_CXX_SOURCES",
                ".cpp",
                &format!("{MESA20_BUILD_ROOT}/src/compiler"),
            )?);
        }
        (Some("workbench/libs/mesa/libgalliumaux"), "mesa3d-linklib-galliumauxiliary") => {
            const MANIFEST: &str = "workbench/libs/mesa/libgalliumaux/galliumaux-20.0.8.sources";
            sources.c = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_GALLIUMAUX_STATIC_C_SOURCES",
                ".c",
                &format!("{MESA20_SOURCE_ROOT}/src/gallium/auxiliary"),
            )?;
            sources.c.extend(mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_GALLIUMAUX_GENERATED_C_SOURCES",
                ".c",
                &format!("{MESA20_BUILD_ROOT}/src/gallium/auxiliary"),
            )?);
        }
        (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa") => {
            const MANIFEST: &str = "workbench/libs/mesa/libmesa/mesa-20.0.8.sources";
            sources.c = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_CORE_C_SOURCES",
                ".c",
                &format!("{MESA20_SOURCE_ROOT}/src/mesa"),
            )?;
            for generated in [
                "main/api_exec.c",
                "main/enums.c",
                "main/format_pack.c",
                "main/format_unpack.c",
                "main/format_fallback.c",
                "main/marshal_generated.c",
                "program/program_parse.tab.c",
                "program/lex.yy.c",
            ] {
                sources.c.push(format!(
                    "{MESA20_BUILD_ROOT}/src/mesa/{}",
                    generated.trim_end_matches(".c")
                ));
            }
            sources.cxx = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA20_CORE_CXX_SOURCES",
                ".cpp",
                &format!("{MESA20_SOURCE_ROOT}/src/mesa"),
            )?;
            if profile == "x86_64" {
                sources.c.extend(mesa20_inventory_stems(
                    root,
                    MANIFEST,
                    "MESA20_CORE_X86_64_C_SOURCES",
                    ".c",
                    &format!("{MESA20_SOURCE_ROOT}/src/mesa"),
                )?);
                sources.asm = mesa20_inventory_paths(
                    root,
                    MANIFEST,
                    "MESA20_CORE_X86_64_ASM_SOURCES",
                    ".S",
                    &format!("{MESA20_SOURCE_ROOT}/src/mesa"),
                )?;
            }
        }
        (Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"), "linklibs-gallium_vc4")
            if profile != "x86_64" =>
        {
            const MANIFEST: &str =
                "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4-20.0.8.sources";
            sources.c = mesa20_inventory_stems(
                root,
                MANIFEST,
                "MESA3D_VC4_C_SOURCES",
                ".c",
                &format!("{MESA20_SOURCE_ROOT}/src/gallium/drivers/vc4"),
            )?;
        }
        _ => return Ok(None),
    }
    Ok(Some(sources))
}

const NOUVEAU_DRM_DIR: &str = "workbench/hidds/nouveau";
const NOUVEAU_DRM_MMAKE: &str = "hidd-nouveau-drm";
const NOUVEAU_DRM_MMAKEFILE: &str = "workbench/hidds/nouveau/mmakefile.src";
const NOUVEAU_DRM_SOURCE_MANIFEST: &str = "workbench/hidds/nouveau/sources.drm.mak";
const NOUVEAU_DRM_MMAKE_SHA256: &str =
    "4c0fd8b41d3590b4303c84be7c670220567b8b86e7e29fd6d05c4a36c7d4ee56";
const NOUVEAU_DRM_SOURCE_MANIFEST_SHA256: &str =
    "f51d30d4b9f182aca412e535b32dab35b9bbcadffc4a480b3bacf55ab8afc28a";
const NOUVEAU_DRM_CORE_SOURCE_COUNT: usize = 67;
const NOUVEAU_DRM_NVIDIA_SOURCE_COUNT: usize = 758;
const NOUVEAU_DRM_TOTAL_SOURCE_COUNT: usize =
    NOUVEAU_DRM_CORE_SOURCE_COUNT + NOUVEAU_DRM_NVIDIA_SOURCE_COUNT;
const NOUVEAU_DRM_SOURCE_PREFIX: &str = "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau";
const NOUVEAU_GALLIUM_MMAKE: &str = "hidd-nouveau-gallium";
const NOUVEAU_GALLIUM_SOURCE_MANIFEST: &str =
    "workbench/hidds/nouveau/nouveau-gallium-20.0.8.sources";
const NOUVEAU_GALLIUM_SOURCE_MANIFEST_SHA256: &str =
    "86ffb0c1e959615833b9d7b937dfcaf237c5f25da8d5706d8354ba5314acc15f";
const NOUVEAU_GALLIUM_C_SOURCE_COUNT: usize = 81;
const NOUVEAU_GALLIUM_CXX_SOURCE_COUNT: usize = 24;
const NOUVEAU_GALLIUM_SOURCE_PREFIX: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium";

/// Selects the three profiles for which the DRM-side Nouveau source snapshot
/// was audited.  The archive has no architecture-specific source lane, but a
/// concrete profile is still required so unsupported configurations cannot
/// silently inherit this closed capability.
fn nouveau_current_profile(
    target: Option<&TargetContext>,
) -> std::result::Result<&'static str, String> {
    let Some(profile) = target else {
        return Err("Nouveau archive capability requires a concrete target profile".to_owned());
    };
    match (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    ) {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok("x86_64")
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => {
            Ok("arm")
        }
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            Ok("aarch64")
        }
        _ => Err(format!(
            "Nouveau archive capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

/// Loads the two literal source inventories kept with the Nouveau DRM port.
/// This deliberately does not broaden the local-Make include parser: the
/// capability owns this exact, SHA-pinned fragment and nothing else in the
/// surrounding mixed DRM/Gallium makefile.
fn nouveau_drm_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != NOUVEAU_DRM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;

    let core = mesa20_inventory(root, NOUVEAU_DRM_SOURCE_MANIFEST, "AROS_DRM_CORE_SOURCES")?;
    let nvidia = mesa20_inventory(root, NOUVEAU_DRM_SOURCE_MANIFEST, "AROS_DRM_NVIDIA_SOURCES")?;
    if core.len() != NOUVEAU_DRM_CORE_SOURCE_COUNT
        || nvidia.len() != NOUVEAU_DRM_NVIDIA_SOURCE_COUNT
    {
        return Err(format!(
            "{NOUVEAU_DRM_SOURCE_MANIFEST} source inventory has {} core and {} NVIDIA entries, expected {NOUVEAU_DRM_CORE_SOURCE_COUNT} and {NOUVEAU_DRM_NVIDIA_SOURCE_COUNT}",
            core.len(),
            nvidia.len()
        ));
    }

    let mut sources = EvaluatedSources {
        declared: true,
        ..EvaluatedSources::default()
    };
    for source in core.into_iter().chain(nvidia) {
        let physical_source = root.join(NOUVEAU_DRM_DIR).join(format!("{source}.c"));
        if !physical_source.is_file() {
            return Err(format!(
                "{NOUVEAU_DRM_SOURCE_MANIFEST} declares missing C source {}",
                physical_source.display()
            ));
        }
        sources
            .c
            .push(format!("{NOUVEAU_DRM_SOURCE_PREFIX}/{source}"));
    }
    if sources.c.len() != NOUVEAU_DRM_TOTAL_SOURCE_COUNT {
        return Err(format!(
            "{NOUVEAU_DRM_SOURCE_MANIFEST} materialized {} C sources, expected {NOUVEAU_DRM_TOTAL_SOURCE_COUNT}",
            sources.c.len()
        ));
    }
    Ok(Some(sources))
}

struct NouveauDrmCompileContract {
    defines: Vec<String>,
    includes: Vec<String>,
    options: Vec<String>,
}

/// The compile inputs of the legacy `hidd-nouveau-drm` target, written as an
/// explicit CMake contract so each source can be materialised on a cold tree.
///
/// The legacy target inherits the LLVM toolchain's normal-build `-O2` through
/// `OPTIMIZATION_CFLAGS`; it is not optional for this source snapshot.
/// `drm_edid.c` uses an `__always_inline` table lookup in a `BUILD_BUG_ON`,
/// which Clang cannot reduce at `-O0`.
fn nouveau_drm_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<NouveauDrmCompileContract>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != NOUVEAU_DRM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;
    Ok(Some(NouveauDrmCompileContract {
        defines: [
            "__KERNEL__",
            "CONFIG_NOUVEAU_DEBUG=5",
            "CONFIG_NOUVEAU_DEBUG_DEFAULT=3",
            "CONFIG_DRM_NOUVEAU_GSP_DEFAULT=1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        includes: [
            "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
            "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include/nvkm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm/subdev/gsp",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        options: [
            "-O2",
            "-Wno-uninitialized",
            "-Wno-strict-aliasing",
            "-Wno-unused-but-set-variable",
            "-Wno-unused-variable",
            "-Wno-unused-function",
            "-Wno-missing-braces",
            "-std=gnu11",
            "-fno-strict-aliasing",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }))
}

fn validate_nouveau_drm_capability(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) {
        return Ok(());
    }
    if !file_has_sha256(root, NOUVEAU_DRM_MMAKEFILE, NOUVEAU_DRM_MMAKE_SHA256)
        || !file_has_sha256(
            root,
            NOUVEAU_DRM_SOURCE_MANIFEST,
            NOUVEAU_DRM_SOURCE_MANIFEST_SHA256,
        )
    {
        return Err(
            "mmakefile or sources.drm.mak differs from the audited Nouveau DRM capability"
                .to_owned(),
        );
    }
    let expected_sources = nouveau_drm_sources(root, relative_dir, NOUVEAU_DRM_MMAKE, target)?
        .ok_or_else(|| format!("missing source capability for {NOUVEAU_DRM_MMAKE}"))?;
    let expected_flags = nouveau_drm_compile_contract(relative_dir, NOUVEAU_DRM_MMAKE, target)?
        .ok_or_else(|| format!("missing compile capability for {NOUVEAU_DRM_MMAKE}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == NOUVEAU_DRM_MMAKE)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {NOUVEAU_DRM_MMAKE} declaration, found {}",
            matching.len()
        ));
    };
    let exact = declaration.target_name == "drm_nouveau"
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files.is_empty()
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files.is_empty()
        && declaration.use_libs.is_empty()
        && declaration.dependencies.is_empty()
        && declaration.dir_path == relative_dir
        && declaration.target_dir.is_none()
        && !declaration.variant_32bit
        && declaration.link_libs.is_empty()
        && declaration.declared_mod_type.is_none()
        && declaration.mod_suffix.is_none()
        && declaration.linklib_name.is_none()
        && declaration.genmodule_linklibs.is_none()
        && declaration.linklib_output_dir.is_none()
        && declaration.canonical_linklib_output
        && declaration.canonical_linklib_eligible
        && declaration.compiler_flags.is_empty()
        && declaration.arch_modules.is_empty()
        && declaration.arch_includes.is_empty()
        && declaration.undefines.is_empty()
        && declaration.link_options.is_empty()
        && declaration.arch_sources.is_empty()
        && declaration.arch_defines.is_empty()
        && declaration.arch_compile_options.is_empty()
        && declaration.defines == expected_flags.defines
        && (declaration.include_dirs == expected_flags.includes)
        && declaration.compile_options == expected_flags.options;
    if !exact {
        return Err(
            "source, language, flag, include or canonical-output contract differs from the audited Nouveau DRM capability"
                .to_owned(),
        );
    }
    Ok(())
}

/// Loads the exact Mesa 20.0.8 Nouveau Gallium source lanes.  The upstream
/// `Makefile.sources` lives below the fetched port tree and cannot be read on
/// a cold configure, so the AROS port keeps this versioned, literal inventory
/// beside the declaring mmakefile.  It deliberately names only extensionless
/// stems: the C/C++ lane is the declaration's source-language authority.
fn nouveau_gallium_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != NOUVEAU_GALLIUM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;

    let c = mesa20_inventory(
        root,
        NOUVEAU_GALLIUM_SOURCE_MANIFEST,
        "NOUVEAU20_GALLIUM_C_SOURCES",
    )?;
    let cxx = mesa20_inventory(
        root,
        NOUVEAU_GALLIUM_SOURCE_MANIFEST,
        "NOUVEAU20_GALLIUM_CXX_SOURCES",
    )?;
    if c.len() != NOUVEAU_GALLIUM_C_SOURCE_COUNT || cxx.len() != NOUVEAU_GALLIUM_CXX_SOURCE_COUNT {
        return Err(format!(
            "{NOUVEAU_GALLIUM_SOURCE_MANIFEST} has {} C and {} C++ entries, expected {NOUVEAU_GALLIUM_C_SOURCE_COUNT} and {NOUVEAU_GALLIUM_CXX_SOURCE_COUNT}",
            c.len(),
            cxx.len()
        ));
    }

    let materialize = |sources: Vec<String>, language: &str| {
        sources
            .into_iter()
            .map(|source| {
                if Path::new(&source).extension().is_some() {
                    Err(format!(
                        "{NOUVEAU_GALLIUM_SOURCE_MANIFEST} {language} inventory must contain extensionless stems: {source}"
                    ))
                } else {
                    Ok(format!("{NOUVEAU_GALLIUM_SOURCE_PREFIX}/{source}"))
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()
    };
    Ok(Some(EvaluatedSources {
        c: materialize(c, "C")?,
        cxx: materialize(cxx, "C++")?,
        declared: true,
        ..EvaluatedSources::default()
    }))
}

/// The concrete compile contract for the Mesa 20.0.8 Nouveau Gallium port.
/// Its C++ lane is intentionally an ordinary C++14 lane, not the tiny Mesa
/// compiler `cxx-compat/new` shim: Nouveau uses the real STL container API.
/// A target toolchain must therefore provide its own compatible C++ headers
/// and runtime before this archive can be built.
fn nouveau_gallium_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<Mesa20CompileContract>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != NOUVEAU_GALLIUM_MMAKE {
        return Ok(None);
    }
    let profile = nouveau_current_profile(target)?;
    Ok(Some(Mesa20CompileContract {
        defines: mesa20_base_defines(profile),
        includes: [
            "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
            "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/libdrm/nouveau",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/libdrm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/libdrm/nouveau",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi/drm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        options: [
            "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>",
            "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>",
            "-fno-strict-aliasing",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }))
}

fn validate_nouveau_gallium_capability(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) {
        return Ok(());
    }
    if !file_has_sha256(root, NOUVEAU_DRM_MMAKEFILE, NOUVEAU_DRM_MMAKE_SHA256)
        || !file_has_sha256(
            root,
            NOUVEAU_GALLIUM_SOURCE_MANIFEST,
            NOUVEAU_GALLIUM_SOURCE_MANIFEST_SHA256,
        )
    {
        return Err(
            "mmakefile or Nouveau Gallium source manifest differs from the audited capability"
                .to_owned(),
        );
    }
    let expected_sources =
        nouveau_gallium_sources(root, relative_dir, NOUVEAU_GALLIUM_MMAKE, target)?
            .ok_or_else(|| format!("missing source capability for {NOUVEAU_GALLIUM_MMAKE}"))?;
    let expected_flags =
        nouveau_gallium_compile_contract(relative_dir, NOUVEAU_GALLIUM_MMAKE, target)?
            .ok_or_else(|| format!("missing compile capability for {NOUVEAU_GALLIUM_MMAKE}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == NOUVEAU_GALLIUM_MMAKE)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {NOUVEAU_GALLIUM_MMAKE} declaration, found {}",
            matching.len()
        ));
    };
    let exact = declaration.target_name == "gallium_nouveau"
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files == expected_sources.cxx
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files.is_empty()
        && declaration.use_libs.is_empty()
        && declaration.dependencies.is_empty()
        && declaration.dir_path == relative_dir
        && declaration.target_dir.is_none()
        && !declaration.variant_32bit
        && declaration.link_libs.is_empty()
        && declaration.declared_mod_type.is_none()
        && declaration.mod_suffix.is_none()
        && declaration.linklib_name.is_none()
        && declaration.genmodule_linklibs.is_none()
        && declaration.linklib_output_dir.is_none()
        && declaration.canonical_linklib_output
        && declaration.canonical_linklib_eligible
        && declaration.compiler_flags.is_empty()
        && declaration.arch_modules.is_empty()
        && declaration.arch_includes.is_empty()
        && declaration.undefines.is_empty()
        && declaration.link_options.is_empty()
        && declaration.arch_sources.is_empty()
        && declaration.arch_defines.is_empty()
        && declaration.arch_compile_options.is_empty()
        && declaration.defines == expected_flags.defines
        && declaration.include_dirs == expected_flags.includes
        && declaration.compile_options == expected_flags.options;
    if !exact {
        return Err(
            "source, language, flag, include or canonical-output contract differs from the audited Nouveau Gallium capability"
                .to_owned(),
        );
    }
    Ok(())
}

const MESA_SSE41_DIR: &str = "workbench/libs/mesa/libmesa";
const MESA_SSE41_MMAKE: &str = "mesa3d-linklib-mesa-sse41";
const MESA_SSE41_CAPABILITY_SHA256: &str =
    "70cd3cc7603b73fba1f5048621cd95bfcde8632d1425664fa9982c6aab4e0fac";
const MESA_SSE41_LOCAL_CONTEXT_SHA256: &str =
    "c954ef928824194f9b91208946e00482d9d1d83cc9496f137cf3a9d92e93b320";
const MESA_SSE41_CONFIG_CONTEXT_SHA256: &str =
    "2614a0d07eaf97b18de5a120a61b124c40375b0bedaeac9af2fd1660797e2176";
const MESA_SSE41_MANIFEST_SHA256: &str =
    "4cf786a7beef96b541213b992adf4df84e866afb8f8b2ef855b119f097538497";
const MESA_PATCH_SHA256: &str = "153e644bc854ff1a29bb04271c1e7effccbcd7e6989b2c0333c88626dc62f53e";
const MESA_SSE41_INCLUDES: &[&str] = &[
    "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
    "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
    "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa",
    "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa/main",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
    "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/compiler/glsl",
    "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main",
    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
];

fn mesa_sse41_sources(x86_64: bool) -> Vec<String> {
    if x86_64 {
        vec![
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/streaming-load-memcpy".to_owned(),
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/sse_minmax".to_owned(),
        ]
    } else {
        Vec::new()
    }
}

fn mesa_sse41_defines(x86_64: bool) -> Vec<String> {
    let mut defines = [
        "__STDC_CONSTANT_MACROS",
        "__STDC_FORMAT_MACROS",
        "__STDC_LIMIT_MACROS",
        "_GNU_SOURCE",
        "HAVE_PTHREAD",
        "HAVE_TIMESPEC_GET",
        "POSIXC_SLOWSTACK_VAARGS",
        "USE_GCC_ATOMIC_BUILTINS",
        "HAVE_ZLIB",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if x86_64 {
        defines.extend(["USE_X86_64_ASM".to_owned(), "USE_SSE41".to_owned()]);
    }
    defines.extend(["MAPI_MODE_GLAPI".to_owned(), "MAPI_MODE_UTIL".to_owned()]);
    defines
}

fn mesa_sse41_compile_options(x86_64: bool) -> Vec<String> {
    let mut options = vec!["-std=gnu11".to_owned(), "-fno-strict-aliasing".to_owned()];
    if x86_64 {
        options.push("-msse4.1".to_owned());
    }
    options
}

/// Classifies the three target profiles covered by the audited Mesa 20.0.8
/// SSE4.1 declaration. The boolean is true only for the profile which has
/// actual SSE sources; the two Raspberry Pi profiles intentionally archive no
/// objects but still publish the library required by the common link line.
fn mesa_sse41_profile(
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<bool>, String> {
    if relative_dir != Path::new(MESA_SSE41_DIR) {
        return Ok(None);
    }
    let Some(profile) = target else {
        return Err("Mesa SSE4.1 capability requires a concrete target profile".to_owned());
    };
    let key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    match key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok(Some(true))
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (
            Some("aarch64"),
            Some("raspi"),
            Some("llvm"),
            Some(""),
            Some("1"),
            Some(""),
        ) => Ok(Some(false)),
        _ => Err(format!(
            "Mesa SSE4.1 capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

fn file_has_sha256(root: &Path, relative: &str, expected: &str) -> bool {
    fs::read(root.join(relative))
        .ok()
        .is_some_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == expected)
}

fn mesa_sse41_fetch_edge_is_pinned(make_source: &str) -> bool {
    let joined = join_mm_continuations(make_source);
    let matching = META_RULE_RE
        .captures_iter(&joined)
        .filter(|capture| &capture[1] == MESA_SSE41_MMAKE)
        .collect::<Vec<_>>();
    let [edge] = matching.as_slice() else {
        return false;
    };
    !edge[0].starts_with("#MM-") && edge[2].split_whitespace().eq(["mesa3d-fetch"])
}

fn mesa_sse41_static_contract_is_pinned(root: &Path, make_source: &str) -> bool {
    let Some(block) = normalized_make_capability_block(
        make_source,
        "MESA3D_GALLIUM_SSE41_SOURCES :=",
        "%build_linklib mmake=mesa3d-linklib-mesa-sse41",
    ) else {
        return false;
    };
    let Some(local_context) = normalized_make_capability_block(
        make_source,
        "include $(SRCDIR)/config/aros.cfg",
        "%common",
    ) else {
        return false;
    };
    let Ok(mesa_config) = fs::read_to_string(root.join("workbench/libs/mesa/mesa.cfg")) else {
        return false;
    };
    let Some(config_context) = normalized_make_capability_block(
        &mesa_config,
        "aros_mesadir :=",
        "MESA3DGL_GALLIUMCORE :=",
    ) else {
        return false;
    };
    let block_digest = format!("{:x}", Sha256::digest(block.as_bytes()));
    let local_context_digest = format!("{:x}", Sha256::digest(local_context.as_bytes()));
    let config_context_digest = format!("{:x}", Sha256::digest(config_context.as_bytes()));
    block_digest == MESA_SSE41_CAPABILITY_SHA256
        && local_context_digest == MESA_SSE41_LOCAL_CONTEXT_SHA256
        && config_context_digest == MESA_SSE41_CONFIG_CONTEXT_SHA256
        && mesa_sse41_fetch_edge_is_pinned(make_source)
        && file_has_sha256(
            root,
            "workbench/libs/mesa/libmesa/mesa-sse41-20.0.8.sources",
            MESA_SSE41_MANIFEST_SHA256,
        )
        && file_has_sha256(
            root,
            "workbench/libs/mesa/mesa-20.0.8-aros.diff",
            MESA_PATCH_SHA256,
        )
}

fn validate_mesa_sse41_capability(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<(), String> {
    let Some(x86_64) = mesa_sse41_profile(relative_dir, target)? else {
        return Ok(());
    };
    if !mesa_sse41_static_contract_is_pinned(root, make_source) {
        return Err("Mesa SSE4.1 recipe, configuration context, source manifest or local patch differs from the audited capability".to_owned());
    }

    let matching_targets = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MESA_SSE41_MMAKE)
        .collect::<Vec<_>>();
    let [sse41] = matching_targets.as_slice() else {
        return Err(format!(
            "requires exactly one {MESA_SSE41_MMAKE} declaration, found {}",
            matching_targets.len()
        ));
    };
    let expected_sources = mesa_sse41_sources(x86_64);
    let expected_defines = mesa_sse41_defines(x86_64);
    let expected_options = mesa_sse41_compile_options(x86_64);
    let target_contract_ok = sse41.target_name == "mesa-sse41"
        && sse41.module_type == ModuleType::LinkLib
        && !sse41.genmodule_only
        && sse41.empty_archive != x86_64
        && sse41.source_files == expected_sources
        && sse41.cxx_source_files.is_empty()
        && sse41.objc_source_files.is_empty()
        && sse41.asm_source_files.is_empty()
        && sse41.use_libs.is_empty()
        && sse41.dependencies.is_empty()
        && sse41.dir_path == relative_dir
        && sse41.target_dir.is_none()
        && !sse41.variant_32bit
        && sse41.link_libs.is_empty()
        && sse41.declared_mod_type.is_none()
        && sse41.mod_suffix.is_none()
        && sse41.linklib_name.is_none()
        && sse41.genmodule_linklibs.is_none()
        && sse41.linklib_output_dir.as_deref() == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
        && !sse41.canonical_linklib_output
        && !sse41.canonical_linklib_eligible
        && sse41.compiler_flags.is_empty()
        && sse41.arch_modules.is_empty()
        && sse41.arch_includes.is_empty()
        && sse41.undefines.is_empty()
        && sse41.link_options.is_empty()
        && sse41.arch_sources.is_empty()
        && sse41.arch_defines.is_empty()
        && sse41.arch_compile_options.is_empty()
        && sse41
            .defines
            .iter()
            .map(String::as_str)
            .eq(expected_defines.iter().map(String::as_str))
        && sse41
            .include_dirs
            .iter()
            .map(String::as_str)
            .eq(MESA_SSE41_INCLUDES.iter().copied())
        && sse41
            .compile_options
            .iter()
            .map(String::as_str)
            .eq(expected_options.iter().map(String::as_str));
    if !target_contract_ok {
        return Err("Mesa SSE4.1 source, empty-archive, flag, include or output contract differs from the audited capability".to_owned());
    }

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == "mesa3d-fetch")
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake=mesa3d-fetch declaration, found {}",
            matching_fetches.len()
        ));
    };
    let origin_words = fetch.origins.split_whitespace().collect::<Vec<_>>();
    if fetch.archive != "mesa-20.0.8"
        || fetch.suffixes != "tar.xz tar.gz"
        || origin_words
            != [
                "cache://",
                "https://archive.mesa3d.org/",
                "https://archive.mesa3d.org/older-versions/20.x",
            ]
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err(
            "central Mesa 20.0.8 fetch declaration differs from the audited SSE4.1 capability"
                .to_owned(),
        );
    }
    Ok(())
}

const GLAPI_GENERATOR_CAPABILITY_SHA256: &str =
    "c42d77ef950bf439e04c36df203309c24a3a931b0c294edce875ae969d8143e5";
const MESAUTIL_GENERATOR_CAPABILITY_SHA256: &str =
    "9ff6ca66c503c1671c6bbd2a2d0b8a9a449a3368a8fcbed29425eb7d7d3fd908";

/// Admits the one hand-written Python generator family needed by Mesa 20.0.8
/// libglapi.
///
/// The legacy recipes are not treated as a general command language.  Their
/// complete semantic block is pinned, as are the selected source/flag profile,
/// central fetch declaration and local patch.  Any drift leaves the ordinary
/// target visible but does not emit executable Python commands.
fn parse_glapi_python_outputs(
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    const GLAPI_DIR: &str = "workbench/libs/mesa/libglapi";
    const GLAPI_MMAKE: &str = "mesa3d-linklib-glapi";
    const GLAPI_FETCH: &str = "mesa3d-fetch";
    const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
    const SOURCE_ARCHIVE: &str = "${AROS_PORTS_SOURCE_DIR}/mesa-20.0.8.tar.xz";
    const SOURCE_SHA256: &str = "6cf0c010df89680f9b2bc6432ff01400031795e39bceda7535fa00af06740b6c";
    const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
    const XML: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen/gl_and_es_API.xml";

    if relative_dir != Path::new(GLAPI_DIR) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err(
            "Mesa glapi generator capability requires a concrete target profile".to_owned(),
        );
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let x86_64 = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => true,
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => false,
        _ => {
            return Err(format!(
                "Mesa glapi generator capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    let matching_targets = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == GLAPI_MMAKE)
        .collect::<Vec<_>>();
    let [glapi] = matching_targets.as_slice() else {
        return Err(format!(
            "requires exactly one {GLAPI_MMAKE} declaration, found {}",
            matching_targets.len()
        ));
    };
    let expected_sources = [
        "glapi/glapi_dispatch",
        "glapi/glapi_entrypoint",
        "glapi/glapi_getproc",
        "glapi/glapi_nop",
        "glapi/glapi",
        "u_current",
        "u_execmem",
    ]
    .into_iter()
    .map(|source| format!("{SOURCE_ROOT}/src/mapi/{source}"))
    .collect::<Vec<_>>();
    let expected_asm = if x86_64 {
        vec![format!("{BUILD_ROOT}/src/mapi/glapi/glapi_x86-64")]
    } else {
        Vec::new()
    };
    let mut expected_defines = vec![
        "__STDC_CONSTANT_MACROS",
        "__STDC_FORMAT_MACROS",
        "__STDC_LIMIT_MACROS",
        "_GNU_SOURCE",
        "HAVE_PTHREAD",
        "HAVE_TIMESPEC_GET",
        "POSIXC_SLOWSTACK_VAARGS",
        "USE_GCC_ATOMIC_BUILTINS",
        "HAVE_ZLIB",
    ];
    if x86_64 {
        expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
    }
    expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
    let mut expected_includes = vec![
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mapi",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mapi/glapi",
        "${CMAKE_SOURCE_DIR}/workbench/libs/mesa",
    ];
    if x86_64 {
        expected_includes.push("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa");
    }
    let target_contract_ok = glapi.target_name == "glapi"
        && glapi.module_type == ModuleType::LinkLib
        && glapi.source_files == expected_sources
        && glapi.cxx_source_files.is_empty()
        && glapi.objc_source_files.is_empty()
        && glapi.asm_source_files == expected_asm
        && glapi.linklib_output_dir.as_deref() == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
        && !glapi.canonical_linklib_output
        && glapi
            .defines
            .iter()
            .map(String::as_str)
            .eq(expected_defines)
        && glapi
            .include_dirs
            .iter()
            .map(String::as_str)
            .eq(expected_includes)
        && glapi.compile_options == ["-std=gnu11", "-fno-strict-aliasing"];
    if !target_contract_ok {
        return Err("Mesa glapi source, flag, include or output contract differs from the audited capability".to_owned());
    }

    let generator_block = normalized_make_capability_block(
        make_source,
        "$(top_builddir)/$(CUR_MESADIR)/glapi/glapitemp.h:",
        "%build_linklib",
    )
    .ok_or_else(|| "Mesa glapi generator recipe block is missing".to_owned())?;
    let generator_digest = format!("{:x}", Sha256::digest(generator_block.as_bytes()));
    if generator_digest != GLAPI_GENERATOR_CAPABILITY_SHA256 {
        return Err(format!(
            "Mesa glapi generator recipe block differs from the audited capability ({generator_digest})"
        ));
    }

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == GLAPI_FETCH)
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={GLAPI_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    let origin_words = fetch.origins.split_whitespace().collect::<Vec<_>>();
    if fetch.archive != "mesa-20.0.8"
        || fetch.suffixes != "tar.xz tar.gz"
        || origin_words
            != [
                "cache://",
                "https://archive.mesa3d.org/",
                "https://archive.mesa3d.org/older-versions/20.x",
            ]
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err(
            "central Mesa 20.0.8 fetch declaration differs from the audited glapi capability"
                .to_owned(),
        );
    }

    let mut jobs = vec![
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_apitemp.py".to_owned(),
            output: "src/mapi/glapi/glapitemp.h".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        },
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_table.py".to_owned(),
            output: "src/mapi/glapi/glapitable.h".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        },
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_procs.py".to_owned(),
            output: "src/mapi/glapi/glprocs.h".to_owned(),
            arguments: vec!["-c".to_owned(), "-f".to_owned(), XML.to_owned()],
        },
    ];
    if x86_64 {
        jobs.push(PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_x86-64_asm.py".to_owned(),
            output: "src/mapi/glapi/glapi_x86-64.s".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        });
    }

    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-glapi-generate".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: GLAPI_FETCH.to_owned(),
        source_archive: SOURCE_ARCHIVE.to_owned(),
        source_sha256: SOURCE_SHA256.to_owned(),
        source_inputs: vec!["src/mapi/glapi/gen/gl_and_es_API.xml".to_owned()],
        jobs,
        driver_script: None,
        driver_sha256: None,
        python_packages: Vec::new(),
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        local_patch_sha256: vec![
            "153e644bc854ff1a29bb04271c1e7effccbcd7e6989b2c0333c88626dc62f53e".to_owned(),
        ],
        consumers: vec![GLAPI_MMAKE.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

/// Admits the two Mesa 20.0.8 utility archives and their two live generated C
/// sources. The dead `u_format_pack.h` rule is intentionally outside the
/// pinned block: it is absent from `MESA_UTIL_GENERATED_FILES` and is not a
/// prerequisite of either archive.
fn parse_mesautil_python_outputs(
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    const MESAUTIL_DIR: &str = "workbench/libs/mesa/libmesautil";
    const MESAUTIL_MMAKE: &str = "mesa3d-linklib-mesautil";
    const MESADEVUTIL_MMAKE: &str = "mesa3d-linklib-mesadevutil";
    const MESA_FETCH: &str = "mesa3d-fetch";
    const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
    const SOURCE_ARCHIVE: &str = "${AROS_PORTS_SOURCE_DIR}/mesa-20.0.8.tar.xz";
    const SOURCE_SHA256: &str = "6cf0c010df89680f9b2bc6432ff01400031795e39bceda7535fa00af06740b6c";
    const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
    const CSV: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv";
    const STATIC_SOURCES: &[&str] = &[
        "anon_file",
        "bitscan",
        "blob",
        "build_id",
        "crc32",
        "dag",
        "debug",
        "disk_cache",
        "double",
        "fast_idiv_by_const",
        "format/u_format",
        "format/u_format_bptc",
        "format/u_format_etc",
        "format/u_format_latc",
        "format/u_format_other",
        "format/u_format_rgtc",
        "format/u_format_s3tc",
        "format/u_format_tests",
        "format/u_format_yuv",
        "format/u_format_zs",
        "half_float",
        "hash_table",
        "mesa-sha1",
        "os_time",
        "os_file",
        "os_socket",
        "os_misc",
        "u_process",
        "sha1/sha1",
        "ralloc",
        "rand_xor",
        "rb_tree",
        "register_allocate",
        "rgtc",
        "set",
        "slab",
        "softfloat",
        "sparse_array",
        "string_buffer",
        "strtod",
        "u_atomic",
        "u_math",
        "u_queue",
        "u_vector",
        "u_debug",
        "u_debug_memory",
        "u_cpu_detect",
        "u_mm",
        "vma",
    ];

    if relative_dir != Path::new(MESAUTIL_DIR) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err(
            "Mesa utility generator capability requires a concrete target profile".to_owned(),
        );
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let x86_64 = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => true,
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => false,
        _ => {
            return Err(format!(
                "Mesa utility generator capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    let matching_mesautil = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MESAUTIL_MMAKE)
        .collect::<Vec<_>>();
    let [mesautil] = matching_mesautil.as_slice() else {
        return Err(format!(
            "requires exactly one {MESAUTIL_MMAKE} declaration, found {}",
            matching_mesautil.len()
        ));
    };
    let matching_mesadevutil = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MESADEVUTIL_MMAKE)
        .collect::<Vec<_>>();
    let [mesadevutil] = matching_mesadevutil.as_slice() else {
        return Err(format!(
            "requires exactly one {MESADEVUTIL_MMAKE} declaration, found {}",
            matching_mesadevutil.len()
        ));
    };

    let mut expected_sources = STATIC_SOURCES
        .iter()
        .map(|source| format!("{SOURCE_ROOT}/src/util/{source}"))
        .collect::<Vec<_>>();
    expected_sources.extend([
        format!("{BUILD_ROOT}/src/util/format_srgb"),
        format!("{BUILD_ROOT}/src/util/format/u_format_table"),
    ]);
    let mut expected_defines = vec![
        "__STDC_CONSTANT_MACROS",
        "__STDC_FORMAT_MACROS",
        "__STDC_LIMIT_MACROS",
        "_GNU_SOURCE",
        "HAVE_PTHREAD",
        "HAVE_TIMESPEC_GET",
        "POSIXC_SLOWSTACK_VAARGS",
        "USE_GCC_ATOMIC_BUILTINS",
        "HAVE_ZLIB",
    ];
    if x86_64 {
        expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
    }
    expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
    let expected_includes = [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/util",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/util/format",
        "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7",
    ];
    let target_contract_ok = |declaration: &TargetDefinition, name: &str, embedded_device: bool| {
        let mut defines = expected_defines.clone();
        if embedded_device {
            defines.push("EMBEDDED_DEVICE");
        }
        declaration.target_name == name
            && declaration.module_type == ModuleType::LinkLib
            && declaration.source_files == expected_sources
            && declaration.cxx_source_files.is_empty()
            && declaration.objc_source_files.is_empty()
            && declaration.asm_source_files.is_empty()
            && declaration.linklib_output_dir.as_deref()
                == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
            && !declaration.canonical_linklib_output
            && declaration.defines.iter().map(String::as_str).eq(defines)
            && declaration
                .include_dirs
                .iter()
                .map(String::as_str)
                .eq(expected_includes)
            && declaration.compile_options == ["-std=gnu11", "-fno-strict-aliasing"]
    };
    if !target_contract_ok(mesautil, "mesautil", false)
        || !target_contract_ok(mesadevutil, "mesadevutil", true)
    {
        return Err(
            "Mesa utility source, flag, include or output contract differs from the audited capability"
                .to_owned(),
        );
    }

    let generator_block = normalized_make_capability_block(
        make_source,
        "$(top_builddir)/$(CUR_MESADIR)/%.c:",
        "%common",
    )
    .ok_or_else(|| "Mesa utility generator recipe block is missing".to_owned())?;
    let generator_digest = format!("{:x}", Sha256::digest(generator_block.as_bytes()));
    if generator_digest != MESAUTIL_GENERATOR_CAPABILITY_SHA256 {
        return Err(format!(
            "Mesa utility generator recipe block differs from the audited capability ({generator_digest})"
        ));
    }

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == MESA_FETCH)
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={MESA_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    let origin_words = fetch.origins.split_whitespace().collect::<Vec<_>>();
    if fetch.archive != "mesa-20.0.8"
        || fetch.suffixes != "tar.xz tar.gz"
        || origin_words
            != [
                "cache://",
                "https://archive.mesa3d.org/",
                "https://archive.mesa3d.org/older-versions/20.x",
            ]
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err(
            "central Mesa 20.0.8 fetch declaration differs from the audited utility capability"
                .to_owned(),
        );
    }

    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-mesautil-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: MESA_FETCH.to_owned(),
        source_archive: SOURCE_ARCHIVE.to_owned(),
        source_sha256: SOURCE_SHA256.to_owned(),
        source_inputs: vec![
            "src/util/format/u_format.csv".to_owned(),
            "src/util/format/u_format_pack.py".to_owned(),
            "src/util/format/u_format_parse.py".to_owned(),
        ],
        jobs: vec![
            PythonGeneratorJob {
                script: "src/util/format_srgb.py".to_owned(),
                output: "src/util/format_srgb.c".to_owned(),
                arguments: vec![CSV.to_owned()],
            },
            PythonGeneratorJob {
                script: "src/util/format/u_format_table.py".to_owned(),
                output: "src/util/format/u_format_table.c".to_owned(),
                arguments: vec![CSV.to_owned()],
            },
        ],
        driver_script: None,
        driver_sha256: None,
        python_packages: Vec::new(),
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        local_patch_sha256: vec![
            "153e644bc854ff1a29bb04271c1e7effccbcd7e6989b2c0333c88626dc62f53e".to_owned(),
        ],
        consumers: vec![MESAUTIL_MMAKE.to_owned(), MESADEVUTIL_MMAKE.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

const MESA20_SOURCE_ARCHIVE: &str = "${AROS_PORTS_SOURCE_DIR}/mesa-20.0.8.tar.xz";
const MESA20_SOURCE_SHA256: &str =
    "6cf0c010df89680f9b2bc6432ff01400031795e39bceda7535fa00af06740b6c";
const MESA20_DRIVER: &str = "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py";
const MESA20_DRIVER_SHA256: &str =
    "773b7c856a83be11bdc205f2e43a1bfaeab1533d658fcb854b16207970ee4599";
const MESA20_MAIN_MMAKE_SHA256: &str =
    "9b3842c1d004b0b761b451b967e4c6e804a7e47fa19d9d0bb0b57aefa20aaac1";
const MESA20_CONFIG_SHA256: &str =
    "db45d23fc15d771df7811341af9834c720f552dabcd87db58876018a5142987c";
const MESA20_MAKO_SHA256: &str = "99579a6f39583fa7e5630a28c3c1f440e4e97a414b80372649c0ce338da2ea28";
const MESA20_MARKUPSAFE_SHA256: &str =
    "ee55d3edf80167e48ea11a923c7386f4669df67d7994554387f84e7d8b0a2bf0";

fn mesa20_generator_job(script: &str, output: &str, arguments: &[&str]) -> PythonGeneratorJob {
    PythonGeneratorJob {
        script: script.to_owned(),
        output: output.to_owned(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
    }
}

fn mesa20_python_packages() -> Vec<PythonPackageDecl> {
    vec![
        PythonPackageDecl {
            fetch_target: "mesa3d-mako-fetch".to_owned(),
            source_root: "${AROS_PORTS_DIR}/mesa-python/mako-1.3.10".to_owned(),
            source_archive: "${AROS_PORTS_SOURCE_DIR}/mako-1.3.10.tar.gz".to_owned(),
            source_sha256: MESA20_MAKO_SHA256.to_owned(),
            python_path: ".".to_owned(),
        },
        PythonPackageDecl {
            fetch_target: "mesa3d-markupsafe-fetch".to_owned(),
            source_root: "${AROS_PORTS_DIR}/mesa-python/markupsafe-3.0.2".to_owned(),
            source_archive: "${AROS_PORTS_SOURCE_DIR}/markupsafe-3.0.2.tar.gz".to_owned(),
            source_sha256: MESA20_MARKUPSAFE_SHA256.to_owned(),
            python_path: "src".to_owned(),
        },
    ]
}

fn mesa20_fetch_is_exact(fetch: &FetchDecl, name: &str) -> bool {
    match name {
        "mesa3d-fetch" => {
            fetch.archive == "mesa-20.0.8"
                && fetch.suffixes == "tar.xz tar.gz"
                && fetch.origins.split_whitespace().eq([
                    "cache://",
                    "https://archive.mesa3d.org/",
                    "https://archive.mesa3d.org/older-versions/20.x",
                ])
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
                && fetch.dir == "workbench/libs/mesa"
        }
        "mesa3d-mako-fetch" => {
            fetch.archive == "mako-1.3.10"
                && fetch.suffixes == "tar.gz"
                && fetch.origins
                    == "https://files.pythonhosted.org/packages/9e/38/bd5b78a920a64d708fe6bc8e0a2c075e1389d53bef8413725c63ba041535"
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa-python"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "::"
                && fetch.dir == "workbench/libs/mesa"
        }
        "mesa3d-markupsafe-fetch" => {
            fetch.archive == "markupsafe-3.0.2"
                && fetch.suffixes == "tar.gz"
                && fetch.origins
                    == "https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62"
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa-python"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "::"
                && fetch.dir == "workbench/libs/mesa"
        }
        _ => false,
    }
}

fn require_mesa20_fetches(fetches: &[FetchDecl]) -> std::result::Result<(), String> {
    for name in [
        "mesa3d-fetch",
        "mesa3d-mako-fetch",
        "mesa3d-markupsafe-fetch",
    ] {
        let matching = fetches
            .iter()
            .filter(|fetch| fetch.name == name)
            .collect::<Vec<_>>();
        let [fetch] = matching.as_slice() else {
            return Err(format!(
                "requires exactly one %fetch mmake={name} declaration, found {}",
                matching.len()
            ));
        };
        if !mesa20_fetch_is_exact(fetch, name) {
            return Err(format!(
                "%fetch mmake={name} differs from the audited Mesa 20.0.8 generator capability"
            ));
        }
    }
    Ok(())
}

fn mesa20_target_contract_is_exact(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    let expected_sources = mesa20_remaining_linklib_sources(root, relative_dir, mmake, target)?
        .ok_or_else(|| format!("missing source capability for {mmake}"))?;
    let expected_flags = mesa20_compile_contract(relative_dir, mmake, target)?
        .ok_or_else(|| format!("missing compile capability for {mmake}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == mmake)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {mmake} declaration, found {}",
            matching.len()
        ));
    };
    let target_name = match mmake {
        "mesa3d-linklib-compiler" => "compiler",
        "mesa3d-linklib-galliumauxiliary" => "galliumauxiliary",
        "mesa3d-linklib-mesa" => "mesa",
        "linklibs-gallium_vc4" => "gallium_vc4",
        _ => return Err(format!("unsupported Mesa target contract {mmake}")),
    };
    let exact = declaration.target_name == target_name
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files == expected_sources.cxx
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files == expected_sources.asm
        && declaration.use_libs.is_empty()
        && declaration.dependencies.is_empty()
        && declaration.dir_path == relative_dir
        && declaration.target_dir.is_none()
        && !declaration.variant_32bit
        && declaration.link_libs.is_empty()
        && declaration.declared_mod_type.is_none()
        && declaration.mod_suffix.is_none()
        && declaration.linklib_name.is_none()
        && declaration.genmodule_linklibs.is_none()
        && declaration.linklib_output_dir.as_deref() == Some(MESA20_PRIVATE_LIBDIR)
        && !declaration.canonical_linklib_output
        && !declaration.canonical_linklib_eligible
        && declaration.compiler_flags.is_empty()
        && declaration.arch_modules.is_empty()
        && declaration.arch_includes.is_empty()
        && declaration.undefines.is_empty()
        && declaration.link_options.is_empty()
        && declaration.arch_sources.is_empty()
        && declaration.arch_defines.is_empty()
        && declaration.arch_compile_options.is_empty()
        && declaration.defines == expected_flags.defines
        && declaration.include_dirs == expected_flags.includes
        && declaration.compile_options == expected_flags.options;
    if !exact {
        return Err(format!(
            "{mmake} source, language, flag, include or private-output contract differs from the audited capability"
        ));
    }
    Ok(())
}

fn mesa20_compiler_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const NIR: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir";
    const GLSL: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl";
    const SPIRV: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/spirv";
    let inputs = [
        "src/compiler/nir/nir_opcodes.py",
        "src/compiler/nir/nir_intrinsics.py",
        "src/compiler/nir/nir_algebraic.py",
        "src/compiler/nir/nir_constant_expressions.h",
        "src/compiler/glsl/float64.glsl",
        "src/compiler/spirv/spirv.core.grammar.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = vec![
        mesa20_generator_job(
            "src/compiler/nir/nir_builder_opcodes_h.py",
            "src/compiler/nir/nir_builder_opcodes.h",
            &["python-stdout"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_constant_expressions.py",
            "src/compiler/nir/nir_constant_expressions.c",
            &["python-stdout"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_intrinsics_c.py",
            "src/compiler/nir/nir_intrinsics.c",
            &["python-outdir", "--outdir", "@OUTDIR@"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_intrinsics_h.py",
            "src/compiler/nir/nir_intrinsics.h",
            &["python-outdir", "--outdir", "@OUTDIR@"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_opcodes_c.py",
            "src/compiler/nir/nir_opcodes.c",
            &["python-stdout"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_opcodes_h.py",
            "src/compiler/nir/nir_opcodes.h",
            &["python-stdout"],
        ),
        mesa20_generator_job(
            "src/compiler/nir/nir_opt_algebraic.py",
            "src/compiler/nir/nir_opt_algebraic.c",
            &["python-stdout"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation.h",
            &["python-stdout", "enum"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation_constant.h",
            &["python-stdout", "constant"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation_strings.h",
            &["python-stdout", "strings"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/xxd.py",
            "src/compiler/glsl/float64_glsl.h",
            &[
                "python-output",
                &format!("{GLSL}/float64.glsl"),
                "@OUTPUT@",
                "-n",
                "float64_source",
            ],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glcpp/glcpp-lex.l",
            "src/compiler/glsl/glcpp/glcpp-lex.c",
            &["flex", "--nounistd"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glcpp/glcpp-parse.y",
            "src/compiler/glsl/glcpp/glcpp-parse.c",
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glcpp/glcpp-parse.y",
            "src/compiler/glsl/glcpp/glcpp-parse.h",
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glsl_lexer.ll",
            "src/compiler/glsl/glsl_lexer.cpp",
            &["flex", "--nounistd"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glsl_parser.yy",
            "src/compiler/glsl/glsl_parser.cpp",
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        mesa20_generator_job(
            "src/compiler/glsl/glsl_parser.yy",
            "src/compiler/glsl/glsl_parser.h",
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        mesa20_generator_job(
            "src/compiler/spirv/spirv_info_c.py",
            "src/compiler/spirv/spirv_info.c",
            &[
                "python-output",
                &format!("{SPIRV}/spirv.core.grammar.json"),
                "@OUTPUT@",
            ],
        ),
        mesa20_generator_job(
            "src/compiler/spirv/vtn_gather_types_c.py",
            "src/compiler/spirv/vtn_gather_types.c",
            &[
                "python-output",
                &format!("{SPIRV}/spirv.core.grammar.json"),
                "@OUTPUT@",
            ],
        ),
    ];
    let _ = NIR;
    (inputs, jobs)
}

fn mesa20_galliumaux_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    (
        Vec::new(),
        vec![
            mesa20_generator_job(
                "src/gallium/auxiliary/indices/u_indices_gen.py",
                "src/gallium/auxiliary/indices/u_indices_gen.c",
                &["python-stdout"],
            ),
            mesa20_generator_job(
                "src/gallium/auxiliary/indices/u_unfilled_gen.py",
                "src/gallium/auxiliary/indices/u_unfilled_gen.c",
                &["python-stdout"],
            ),
        ],
    )
}

fn mesa20_mesa_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const GLAPI: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen";
    const MAIN: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main";
    const XML: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen/gl_and_es_API.xml";
    let inputs = [
        "src/mapi/glapi/gen/gl_and_es_API.xml",
        "src/mapi/glapi/gen/gl_XML.py",
        "src/mapi/glapi/gen/glX_XML.py",
        "src/mapi/glapi/gen/license.py",
        "src/mapi/glapi/gen/static_data.py",
        "src/mesa/main/get_hash_params.py",
        "src/mesa/main/formats.csv",
        "src/mesa/main/format_parser.py",
        "VERSION",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = vec![
        mesa20_generator_job(
            "src/mapi/glapi/gen/gl_table.py",
            "src/mesa/main/dispatch.h",
            &["python-stdout", "-m", "remap_table", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mapi/glapi/gen/remap_helper.py",
            "src/mesa/main/remap_helper.h",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mapi/glapi/gen/gl_enums.py",
            "src/mesa/main/enums.c",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mapi/glapi/gen/gl_genexec.py",
            "src/mesa/main/api_exec.c",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mapi/glapi/gen/gl_marshal_h.py",
            "src/mesa/main/marshal_generated.h",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mapi/glapi/gen/gl_marshal.py",
            "src/mesa/main/marshal_generated.c",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mesa/main/get_hash_generator.py",
            "src/mesa/main/get_hash.h",
            &["python-stdout", "-f", XML],
        ),
        mesa20_generator_job(
            "src/mesa/main/format_info.py",
            "src/mesa/main/format_info.h",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        mesa20_generator_job(
            "src/mesa/main/format_fallback.py",
            "src/mesa/main/format_fallback.c",
            &["python-output", &format!("{MAIN}/formats.csv"), "@OUTPUT@"],
        ),
        mesa20_generator_job(
            "src/mesa/main/format_pack.py",
            "src/mesa/main/format_pack.c",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        mesa20_generator_job(
            "src/mesa/main/format_unpack.py",
            "src/mesa/main/format_unpack.c",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        mesa20_generator_job("VERSION", "src/mesa/main/git_sha1.h", &["mesa-git-sha1"]),
        mesa20_generator_job(
            "src/mesa/program/program_lexer.l",
            "src/mesa/program/lex.yy.c",
            &["flex", "--nounistd", "--never-interactive"],
        ),
        mesa20_generator_job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.c",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
        mesa20_generator_job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.h",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
    ];
    let _ = GLAPI;
    (inputs, jobs)
}

fn mesa20_vc4_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const CLE: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/broadcom/cle";
    let inputs = [
        "src/broadcom/cle/v3d_packet_v21.xml",
        "src/broadcom/cle/v3d_packet_v33.xml",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = [
        ("v3d_packet_v21.xml", "v3d_packet_v21_pack.h", "21"),
        ("v3d_packet_v33.xml", "v3d_packet_v33_pack.h", "33"),
        ("v3d_packet_v33.xml", "v3d_packet_v41_pack.h", "41"),
        ("v3d_packet_v33.xml", "v3d_packet_v42_pack.h", "42"),
    ]
    .into_iter()
    .map(|(xml, output, version)| {
        mesa20_generator_job(
            "src/broadcom/cle/gen_pack_header.py",
            &format!("src/broadcom/cle/{output}"),
            &["python-stdout", &format!("{CLE}/{xml}"), version],
        )
    })
    .collect();
    (inputs, jobs)
}

fn parse_mesa20_remaining_python_outputs(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    if !matches!(
        relative_dir.to_str(),
        Some("workbench/libs/mesa/libcompiler")
            | Some("workbench/libs/mesa/libgalliumaux")
            | Some("workbench/libs/mesa/libmesa")
            | Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium")
    ) {
        return Ok(None);
    }
    let profile = mesa20_current_profile(target)?;
    let (mmake, owner, mmake_sha256, manifest, manifest_sha256, source_inputs, jobs, packages) =
        match relative_dir.to_str() {
            Some("workbench/libs/mesa/libcompiler") => {
                let (inputs, jobs) = mesa20_compiler_jobs();
                (
                    "mesa3d-linklib-compiler",
                    "mesa3d-linklib-compiler-generated",
                    "77af02d75be9c1c4e35c64dfe5e084b9735a2acd2636a8641b420295e7f91f15",
                    "workbench/libs/mesa/libcompiler/compiler-20.0.8.sources",
                    "88cdeedf3091fadf1678af939ed582329523081b748f2b0abd39ae3e6f5f2481",
                    inputs,
                    jobs,
                    mesa20_python_packages(),
                )
            }
            Some("workbench/libs/mesa/libgalliumaux") => {
                let (inputs, jobs) = mesa20_galliumaux_jobs();
                (
                    "mesa3d-linklib-galliumauxiliary",
                    "mesa3d-linklib-galliumauxiliary-generated",
                    "20f6eb054f0aa4313a33ae6e2bf5cfa1fcf132bfabe5cf64085039e7ecf4f1a4",
                    "workbench/libs/mesa/libgalliumaux/galliumaux-20.0.8.sources",
                    "eebe8fe19dd4cc1531d93a72ac8ca8e38408a7ecad3799f3f896663a2f996705",
                    inputs,
                    jobs,
                    Vec::new(),
                )
            }
            Some("workbench/libs/mesa/libmesa") => {
                let (inputs, jobs) = mesa20_mesa_jobs();
                (
                    "mesa3d-linklib-mesa",
                    "mesa3d-linklib-mesa-generated",
                    "899ffe50dd00f767f33acdee91f01083d79461f17dd27194c6dae07919d47c40",
                    "workbench/libs/mesa/libmesa/mesa-20.0.8.sources",
                    "61c034fdbd34bf963c73cf1d89765dbc10ad45865c0d42f4d6f8c60dd0bbbfcc",
                    inputs,
                    jobs,
                    mesa20_python_packages(),
                )
            }
            Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium") if profile != "x86_64" => {
                let (inputs, jobs) = mesa20_vc4_jobs();
                (
                    "linklibs-gallium_vc4",
                    "linklibs-gallium_vc4-gen-cle",
                    "a6482a1b4758ff74b76b479ea226e2ffab17b7f50095687a252facf96530be20",
                    "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4-20.0.8.sources",
                    "27067482f43902b58872ae0c2e92a9e4f6bc51328b6b035e79d75357ec002a72",
                    inputs,
                    jobs,
                    Vec::new(),
                )
            }
            Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium") => return Ok(None),
            _ => return Ok(None),
        };

    let make_digest = format!("{:x}", Sha256::digest(make_source.as_bytes()));
    if make_digest != mmake_sha256
        || !file_has_sha256(root, manifest, manifest_sha256)
        || !file_has_sha256(
            root,
            "workbench/libs/mesa/mesa20_generate.py",
            MESA20_DRIVER_SHA256,
        )
        || !file_has_sha256(
            root,
            "workbench/libs/mesa/mmakefile.src",
            MESA20_MAIN_MMAKE_SHA256,
        )
        || !file_has_sha256(root, "workbench/libs/mesa/mesa.cfg", MESA20_CONFIG_SHA256)
        || !file_has_sha256(
            root,
            "workbench/libs/mesa/mesa-20.0.8-aros.diff",
            MESA_PATCH_SHA256,
        )
        || (matches!(mmake, "mesa3d-linklib-compiler" | "mesa3d-linklib-mesa")
            && !file_has_sha256(root, MESA20_CXX_COMPAT_NEW, MESA20_CXX_COMPAT_NEW_SHA256))
    {
        return Err(format!(
            "{mmake} declaration, inventory, driver, central Mesa context or patch differs from the audited capability"
        ));
    }
    require_mesa20_fetches(fetches)?;
    mesa20_target_contract_is_exact(root, relative_dir, mmake, target, targets)?;

    Ok(Some(PythonOutputsDecl {
        owner: owner.to_owned(),
        source_root: MESA20_SOURCE_ROOT.to_owned(),
        build_root: MESA20_BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_archive: MESA20_SOURCE_ARCHIVE.to_owned(),
        source_sha256: MESA20_SOURCE_SHA256.to_owned(),
        source_inputs,
        jobs,
        driver_script: Some(MESA20_DRIVER.to_owned()),
        driver_sha256: Some(MESA20_DRIVER_SHA256.to_owned()),
        python_packages: packages,
        audited_source_dir: MESA20_SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        local_patch_sha256: vec![MESA_PATCH_SHA256.to_owned()],
        consumers: vec![mmake.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
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
    emit_archspecific_rules: bool,
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

    if emit_archspecific_rules {
        // `%gen_archspecificrules` is expanded for the ABI/genmodule-only
        // forms.  Sourceful modules deliberately do not receive this CMake
        // translation: MetaMake marks its architecture chain virtual and uses
        // a pre-marked traversal to break its circular return to the concrete
        // module producer.  CMake rejects that strong cycle.  Their ordinary
        // ABI/linklib aliases above remain real dependencies, while explicit
        // source-tree architecture selectors retain their own mappings.
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
            let arch_cpu =
                format!("{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}{suffix}");
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
    }

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
    parse_mmakefile_impl(path, root, dirs, None, &[])
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
    parse_mmakefile_impl(path, root, dirs, Some(target), &[])
}

/// Parses one mmakefile with a tree-wide inventory of proven `%fetch`
/// declarations available for declaration-local input ownership checks.
///
/// The inventory does not add fetches to this file's result. It only lets a
/// safe local variable fragment prove that a source or include path belongs to
/// a fetch declared centrally elsewhere in the tree.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn parse_mmakefile_with_dirs_and_context_and_fetches(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: &TargetContext,
    known_fetches: &[FetchDecl],
) -> Result<ParsedMmakefile> {
    parse_mmakefile_impl(path, root, dirs, Some(target), known_fetches)
}

/// Collects the target-selected `%fetch` declarations of one mmakefile.
///
/// This cheap first pass supplies the tree-wide ownership inventory required
/// by centrally declared ports without parsing the file's build targets.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
#[allow(clippy::missing_panics_doc)]
pub fn collect_mmakefile_fetches_with_context(
    path: &Path,
    root: &Path,
    target: &TargetContext,
) -> Result<Vec<FetchDecl>> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();
    let mut visited = HashSet::new();
    let collector_content =
        inline_collector_make_includes(&content, root, &rel_dir, &mut visited, 8);
    let collector_joined = join_continuations(&collector_content);
    let collector_input = format!(
        "{}{}",
        collector_forward_local_prelude(&collector_joined),
        collector_joined
    );
    let scope = collect_vars_impl(&collector_input, Some(target)).0;
    Ok(collect_fetches_with_scope(&content, &rel_dir, &scope).0)
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

/// Whether a broad-but-syntactically-safe local variable fragment may be used
/// for this mmakefile's concrete declarations.
///
/// This is deliberately stricter than enabling `SafeVariableScopes` for every
/// file. The candidate is accepted only for a concrete target profile, only
/// when every build declaration resolves to sources owned by a `%fetch` from
/// the same mmakefile, and only when it makes at least one previously
/// unresolved declaration concrete. Consequently a safe-looking configuration
/// fragment cannot silently affect an unrelated in-tree target or imply an
/// unmodelled source owner.
fn owns_fetched_source(source: &str, fetches: &[crate::fetch::FetchDecl]) -> bool {
    const PORTS_ROOT: &str = "${AROS_PORTS_DIR}";
    if source == PORTS_ROOT || !source.starts_with("${AROS_PORTS_DIR}/") {
        return false;
    }
    fetches.iter().any(|fetch| {
        let destination = fetch.destination.trim_end_matches('/');
        (destination == PORTS_ROOT || destination.starts_with("${AROS_PORTS_DIR}/"))
            && (source == destination
                || source
                    .strip_prefix(destination)
                    .is_some_and(|tail| tail.starts_with('/')))
    })
}

fn all_sources_are_fetch_owned(
    sources: &EvaluatedSources,
    fetches: &[crate::fetch::FetchDecl],
) -> bool {
    !sources.is_empty()
        && sources
            .c
            .iter()
            .chain(&sources.cxx)
            .chain(&sources.objc)
            .chain(&sources.asm)
            .all(|source| owns_fetched_source(source, fetches))
}

fn declaration_owned_port_scope(
    plain: &LocalMakeIncludeScan,
    candidate: &LocalMakeIncludeScan,
    target: Option<&TargetContext>,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
    fetches: &[crate::fetch::FetchDecl],
) -> bool {
    let Some(target) = target else {
        return false;
    };
    if candidate.expanded == plain.expanded
        || candidate.fragments.is_empty()
        || !candidate.issues.is_empty()
        || fetches.is_empty()
    {
        return false;
    }

    let plain_joined = join_continuations(&plain.expanded);
    let candidate_joined = join_continuations(&candidate.expanded);
    let (plain_scope, plain_states) =
        collect_vars_impl_with_forward_locals(&plain_joined, Some(target), true);
    let (candidate_scope, candidate_states) =
        collect_vars_impl_with_forward_locals(&candidate_joined, Some(target), true);
    let mut ignored = Vec::new();
    let plain_invocations =
        select_target_invocations(&plain_joined, Some(&plain_states), rel_dir, &mut ignored)
            .into_iter()
            .filter(|invocation| is_concrete_build_invocation(&invocation.name))
            .collect::<Vec<_>>();
    ignored.clear();
    let candidate_invocations = select_target_invocations(
        &candidate_joined,
        Some(&candidate_states),
        rel_dir,
        &mut ignored,
    )
    .into_iter()
    .filter(|invocation| is_concrete_build_invocation(&invocation.name))
    .collect::<Vec<_>>();

    if candidate_invocations.is_empty()
        || candidate_invocations.len() != plain_invocations.len()
        || candidate_invocations
            .iter()
            .zip(&plain_invocations)
            .any(|(candidate, plain)| candidate.name != plain.name || candidate.args != plain.args)
    {
        return false;
    }

    let mut newly_resolved = false;
    for (candidate_invocation, plain_invocation) in
        candidate_invocations.iter().zip(&plain_invocations)
    {
        let candidate_vars = candidate_scope.snapshot(candidate_invocation.line);
        let candidate_context = MakeExprContext::new(
            &candidate_scope,
            dirs,
            candidate_invocation.line,
            root,
            rel_dir,
        );
        let Ok(candidate_sources) = evaluate_macro_sources(
            &candidate_invocation.args,
            &candidate_vars,
            &candidate_context,
        ) else {
            return false;
        };
        if !candidate_sources.declared || candidate_sources.is_empty() {
            return false;
        }
        let plain_vars = plain_scope.snapshot(plain_invocation.line);
        let plain_context =
            MakeExprContext::new(&plain_scope, dirs, plain_invocation.line, root, rel_dir);
        let plain_sources =
            evaluate_macro_sources(&plain_invocation.args, &plain_vars, &plain_context).ok();
        for source in candidate_sources
            .c
            .iter()
            .chain(&candidate_sources.cxx)
            .chain(&candidate_sources.objc)
            .chain(&candidate_sources.asm)
        {
            if source.starts_with("${AROS_PORTS_DIR}/") {
                if !owns_fetched_source(source, fetches) {
                    return false;
                }
                let existed = plain_sources.as_ref().is_some_and(|sources| {
                    sources
                        .c
                        .iter()
                        .chain(&sources.cxx)
                        .chain(&sources.objc)
                        .chain(&sources.asm)
                        .any(|plain| plain == source)
                });
                newly_resolved |= !existed;
            }
        }

        let candidate_includes = collect_includes_at(
            &candidate_joined,
            &candidate_scope,
            candidate_invocation.line,
            rel_dir,
        );
        if !candidate_includes.unresolved.is_empty() {
            return false;
        }
        let plain_includes =
            collect_includes_at(&plain_joined, &plain_scope, plain_invocation.line, rel_dir);
        for directory in &candidate_includes.dirs {
            if directory.starts_with("${AROS_PORTS_DIR}/") {
                if !owns_fetched_source(directory, fetches) {
                    return false;
                }
                newly_resolved |= !plain_includes.dirs.contains(directory);
            }
        }
    }
    newly_resolved
}

fn references_any_make_variable(raw: &str, names: &[String]) -> bool {
    names
        .iter()
        .any(|name| raw.contains(&format!("$({name})")) || raw.contains(&format!("${{{name}}}")))
}

fn make_variable_reference_count(raw: &str, name: &str) -> usize {
    raw.match_indices(&format!("$({name})")).count()
        + raw.match_indices(&format!("${{{name}}}")).count()
}

fn make_reference_end(raw: &str, dollar: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let first = *bytes.get(dollar + 1)?;
    let mut closers = vec![match first {
        b'(' => b')',
        b'{' => b'}',
        _ => return None,
    }];
    let mut cursor = dollar + 2;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' {
            match bytes.get(cursor + 1) {
                Some(b'(') => {
                    closers.push(b')');
                    cursor += 2;
                    continue;
                }
                Some(b'{') => {
                    closers.push(b'}');
                    cursor += 2;
                    continue;
                }
                Some(b'$') => {
                    cursor += 2;
                    continue;
                }
                _ => {}
            }
        }
        if Some(&bytes[cursor]) == closers.last() {
            closers.pop();
            if closers.is_empty() {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

fn top_level_make_whitespace(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut closers = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' {
            match bytes.get(cursor + 1) {
                Some(b'(') => {
                    closers.push(b')');
                    cursor += 2;
                    continue;
                }
                Some(b'{') => {
                    closers.push(b'}');
                    cursor += 2;
                    continue;
                }
                Some(b'$') => {
                    cursor += 2;
                    continue;
                }
                _ => {}
            }
        }
        if Some(&bytes[cursor]) == closers.last() {
            closers.pop();
        } else if closers.is_empty() && bytes[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn collect_make_expression_dependencies(
    raw: &str,
    dependencies: &mut HashSet<String>,
) -> Option<()> {
    let mut cursor = 0usize;
    while let Some(relative) = raw[cursor..].find('$') {
        let dollar = cursor + relative;
        match raw.as_bytes().get(dollar + 1) {
            Some(b'$') => {
                cursor = dollar + 2;
            }
            Some(b'(' | b'{') => {
                let end = make_reference_end(raw, dollar)?;
                let body = &raw[dollar + 2..end];
                let trimmed = body.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if let Some(space) = top_level_make_whitespace(trimmed) {
                    let function = &trimmed[..space];
                    if !matches!(
                        function,
                        "addprefix"
                            | "addsuffix"
                            | "filter"
                            | "filter-out"
                            | "patsubst"
                            | "subst"
                            | "notdir"
                            | "dir"
                            | "basename"
                            | "suffix"
                            | "sort"
                            | "strip"
                            | "wildcard"
                            | "call"
                    ) {
                        return None;
                    }
                    collect_make_expression_dependencies(
                        trimmed[space..].trim_start(),
                        dependencies,
                    )?;
                } else {
                    let raw_name = trimmed.split_once(':').map_or(trimmed, |(name, _)| name);
                    // Computed variable names can select a hidden target or
                    // file-scope control property. The literal-header scope
                    // deliberately admits only statically named dependencies.
                    if raw_name.is_empty()
                        || raw_name.contains('$')
                        || !raw_name.chars().all(|character| {
                            character.is_ascii_alphanumeric()
                                || matches!(character, '_' | '-' | '.')
                        })
                    {
                        return None;
                    }
                    dependencies.insert(raw_name.to_owned());
                    if let Some((_, substitution)) = trimmed.split_once(':') {
                        collect_make_expression_dependencies(substitution, dependencies)?;
                    }
                }
                cursor = end + 1;
            }
            // Single-character automatic variables and an unterminated `$`
            // have no stable declaration-time meaning here.
            _ => return None,
        }
    }
    Some(())
}

fn make_expression_dependencies(raw: &str) -> Option<HashSet<String>> {
    let mut dependencies = HashSet::new();
    collect_make_expression_dependencies(raw, &mut dependencies)?;
    Some(dependencies)
}

fn make_conditional_dependencies(directive: &str, args: &str) -> Option<HashSet<String>> {
    if matches!(directive, "ifdef" | "ifndef") {
        let name = args.trim();
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
        {
            return None;
        }
        return Some(std::iter::once(name.to_owned()).collect());
    }
    make_expression_dependencies(args)
}

fn make_semantic_lines(content: &str) -> String {
    let mut semantic = String::with_capacity(content.len());
    for line in content.lines() {
        // A #MM marker has no expression of its own. Its following ordinary
        // rule remains visible and is counted like every other Make line.
        if !line.trim_start().starts_with("#MM") {
            semantic.push_str(strip_make_comment(line));
        }
        semantic.push('\n');
    }
    semantic
}

const ATHEROS_HAL_LITERAL_DEFINE_VARIABLES: &[&str] = &[
    "AH_ASSERT",
    "AH_DEBUG",
    "AH_DEBUG_ALQ",
    "AH_DEBUG_COUNTRY",
    "AH_DISABLE_WME",
    "AH_EEPROM_V1",
    "AH_EEPROM_V14",
    "AH_EEPROM_V3",
    "AH_EEPROM_V4K",
    "AH_ENABLE_AP_SUPPORT",
    "AH_ENABLE_FORCEBIAS",
    "AH_NEED_DESC_SWAP",
    "AH_PRIVATE_DIAG",
    "AH_REGOPS_FUNC",
    "AH_SUPPORT_2133",
    "AH_SUPPORT_2316",
    "AH_SUPPORT_2317",
    "AH_SUPPORT_2413",
    "AH_SUPPORT_2417",
    "AH_SUPPORT_2425",
    "AH_SUPPORT_5111",
    "AH_SUPPORT_5112",
    "AH_SUPPORT_5413",
    "AH_SUPPORT_AR5210",
    "AH_SUPPORT_AR5211",
    "AH_SUPPORT_AR5212",
    "AH_SUPPORT_AR5312",
    "AH_SUPPORT_AR5416",
    "AH_WRITE_EEPROM",
    "AH_WRITE_REGDOMAIN",
    "HAL_OBJS",
    "OPT_AH_PATH",
];

/// Grants one reviewed recipe-bearing fragment the exact Make capabilities it
/// needs. A syntactic or prefix-based policy cannot prove that an assignment
/// is declaration-local: names such as AROS_LIB, TARGET_CC, ECHO and MKDEPEND
/// are read implicitly by MetaMake recipes even when the declaring mmakefile
/// never references them. New fragments therefore require a deliberate exact
/// manifest entry instead of widening an ambient-variable denylist.
fn literal_define_fragment_has_capability(
    fragment: &Path,
    provider: &str,
    owner: &str,
    provider_files: &str,
    raw_output: &str,
    owner_prerequisite: &str,
    assigned_variables: &[String],
) -> bool {
    if fragment != Path::new("workbench/devs/networks/atheros5000/hal/Makefile.inc")
        || provider != "workbench-devs-networks-atheros5000-hal"
        || owner != "workbench-devs-networks-atheros5000-hal-opts"
        || !assigned_variables
            .iter()
            .map(String::as_str)
            .eq(ATHEROS_HAL_LITERAL_DEFINE_VARIABLES.iter().copied())
    {
        return false;
    }

    let assigned: HashSet<&str> = assigned_variables.iter().map(String::as_str).collect();
    let source_roots = make_expression_dependencies(provider_files)
        .map(|names| {
            names
                .into_iter()
                .filter(|name| assigned.contains(name.as_str()))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let output_roots = make_expression_dependencies(raw_output)
        .and_then(|mut names| {
            names.extend(make_expression_dependencies(owner_prerequisite)?);
            Some(
                names
                    .into_iter()
                    .filter(|name| assigned.contains(name.as_str()))
                    .collect::<HashSet<_>>(),
            )
        })
        .unwrap_or_default();
    source_roots == HashSet::from(["HAL_OBJS".to_owned()])
        && output_roots == HashSet::from(["OPT_AH_PATH".to_owned()])
}

/// Computes the complete fragment-local assignment closure which can affect
/// the provider's source list or the generated header.
///
/// Every assignment records both its right-hand-side references and the
/// condition variables which decide whether it runs. Header recipe conditions
/// are roots alongside the source expression, rule target and marked owner
/// prerequisite. Callers require this closure to equal the fragment's full
/// assignment set, so an otherwise harmless-looking unused assignment cannot
/// leak a Make file-scope build property into the declaration.
fn literal_define_fragment_product_closure(
    content: &str,
    provider_files: &str,
    raw_output: &str,
    owner_prerequisite: &str,
    assigned_variables: &[String],
) -> Option<HashSet<String>> {
    let assigned: HashSet<String> = assigned_variables.iter().cloned().collect();
    let mut assignment_dependencies: HashMap<String, HashSet<String>> = assigned
        .iter()
        .map(|name| (name.clone(), HashSet::new()))
        .collect();
    let mut condition_stack: Vec<HashSet<String>> = Vec::new();
    let mut header_condition_roots = HashSet::new();
    let mut rule_seen = false;

    for raw_line in join_continuations(content).lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|args| (word, args)))
        {
            condition_stack.push(make_conditional_dependencies(directive, args)?);
            continue;
        }
        if trimmed == "endif" {
            condition_stack.pop()?;
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            let current = condition_stack.last_mut()?;
            let tail = trimmed.strip_prefix("else").unwrap().trim();
            if !tail.is_empty() {
                let (directive, args) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                    .into_iter()
                    .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))?;
                current.extend(make_conditional_dependencies(directive, args)?);
            }
            continue;
        }
        if raw_line.starts_with('\t') {
            if !rule_seen {
                return None;
            }
            for conditions in &condition_stack {
                header_condition_roots.extend(conditions.iter().cloned());
            }
            continue;
        }
        let semantic = strip_make_comment(trimmed).trim();
        if semantic.is_empty() {
            continue;
        }
        if let Some((name, value, _)) = variable_assignment(semantic) {
            let dependencies = assignment_dependencies.get_mut(name)?;
            dependencies.extend(make_expression_dependencies(value)?);
            for conditions in &condition_stack {
                dependencies.extend(conditions.iter().cloned());
            }
            continue;
        }
        if semantic
            .split_once(':')
            .is_some_and(|(target, prerequisites)| {
                !target.trim().is_empty() && prerequisites.trim().is_empty()
            })
        {
            if rule_seen || !condition_stack.is_empty() {
                return None;
            }
            rule_seen = true;
            continue;
        }
        return None;
    }
    if !condition_stack.is_empty() || !rule_seen {
        return None;
    }

    let mut pending = make_expression_dependencies(provider_files)?;
    pending.extend(make_expression_dependencies(raw_output)?);
    pending.extend(make_expression_dependencies(owner_prerequisite)?);
    pending.extend(header_condition_roots);
    let mut closure = HashSet::new();
    while let Some(variable) = pending.iter().next().cloned() {
        pending.remove(&variable);
        if !assigned.contains(&variable) || !closure.insert(variable.clone()) {
            continue;
        }
        pending.extend(assignment_dependencies.get(&variable)?.iter().cloned());
    }
    Some(closure)
}

fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

fn in_tree_c_source_exists(root: &Path, relative_dir: &Path, source: &str) -> bool {
    if source.is_empty() || source.contains('$') || source.contains(';') || source.contains('\\') {
        return false;
    }
    let source_relative = if let Some(relative) = source.strip_prefix("${CMAKE_SOURCE_DIR}/") {
        PathBuf::from(relative)
    } else {
        let path = Path::new(source);
        if path.is_absolute() {
            return false;
        }
        relative_dir.join(path)
    };
    let Some(source_relative) = normalize_relative_path(&source_relative) else {
        return false;
    };
    let Some(owner) = normalize_relative_path(relative_dir) else {
        return false;
    };
    if !source_relative.starts_with(&owner) {
        return false;
    }
    let source_path = root.join(&source_relative);
    source_path.is_file() || PathBuf::from(format!("{}.c", source_path.display())).is_file()
}

fn safe_define_header_output(output: &str) -> bool {
    let Some(relative) = output.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    if relative.is_empty()
        || relative.contains('$')
        || relative.contains(';')
        || relative.contains('\\')
        || Path::new(relative).extension() != Some(std::ffi::OsStr::new("h"))
    {
        return false;
    }
    normalize_relative_path(Path::new(relative)).is_some_and(|normalized| {
        normalized == Path::new(relative)
            && normalized.file_name().is_some_and(|name| !name.is_empty())
    })
}

fn safe_build_tree_output_directory(output: &str) -> bool {
    let Some(relative) = output.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    if relative.is_empty()
        || relative.contains('$')
        || relative.contains(';')
        || relative.contains('\\')
    {
        return false;
    }
    normalize_relative_path(Path::new(relative)).is_some_and(|normalized| {
        normalized == Path::new(relative) && !normalized.as_os_str().is_empty()
    })
}

fn parse_literal_define_recipe_line(line: &str) -> Option<(&str, &str)> {
    let quoted = line.trim().strip_prefix("echo \"")?;
    let close = quoted.rfind('"')?;
    let definition = quoted[..close].strip_prefix("#define ")?.trim();
    let redirect = quoted[close + 1..].trim();
    let destination = redirect
        .strip_prefix(">>")
        .or_else(|| redirect.strip_prefix('>'))?
        .trim();
    (!definition.is_empty() && !destination.is_empty()).then_some((definition, destination))
}

fn collect_active_literal_defines(
    content: &str,
    scope: &VarScope,
    target: &TargetContext,
) -> std::result::Result<(usize, String, String, Vec<String>), String> {
    let mut rule: Option<(usize, String)> = None;
    let mut destination: Option<String> = None;
    let mut definitions = Vec::new();
    let mut definition_names = HashSet::new();
    let mut stack: Vec<ConditionalFrame> = Vec::new();

    for (index, raw_line) in content.lines().enumerate() {
        let trimmed = raw_line.trim();
        if rule.is_none() {
            if raw_line.starts_with('\t') || trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            let uncommented = strip_make_comment(trimmed).trim();
            if let Some((output, prerequisites)) = uncommented.split_once(':') {
                if prerequisites.trim().is_empty() {
                    rule = Some((index + 1, output.trim().to_owned()));
                }
            }
            continue;
        }

        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            let parent = stack
                .last()
                .map_or(ConditionalTruth::True, |frame| frame.current);
            let condition = evaluate_conditional(directive, args, scope, target);
            stack.push(ConditionalFrame::new(parent, condition));
            continue;
        }
        if trimmed == "endif" {
            stack.pop();
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            let Some(frame) = stack.last_mut() else {
                return Err(format!("line {} has an unmatched else", index + 1));
            };
            let tail = trimmed.strip_prefix("else").unwrap().trim();
            if tail.is_empty() {
                frame.otherwise();
            } else if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                .into_iter()
                .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))
            {
                frame.else_if(evaluate_conditional(directive, args, scope, target));
            } else {
                return Err(format!(
                    "line {} has an unsupported else directive",
                    index + 1
                ));
            }
            continue;
        }
        if !raw_line.starts_with('\t') {
            continue;
        }
        let state = stack
            .last()
            .map_or(ConditionalTruth::True, |frame| frame.current);
        if state == ConditionalTruth::Unknown {
            return Err(format!(
                "line {} is guarded by an unresolved Make conditional",
                index + 1
            ));
        }
        let Some((definition, recipe_destination)) = parse_literal_define_recipe_line(trimmed)
        else {
            return Err(format!("line {} is not a literal define recipe", index + 1));
        };
        if let Some(previous) = &destination {
            if previous != recipe_destination {
                return Err(format!(
                    "line {} redirects to a different header",
                    index + 1
                ));
            }
        } else {
            destination = Some(recipe_destination.to_owned());
        }
        if state == ConditionalTruth::True {
            let Some(name) = definition.split_whitespace().next() else {
                return Err(format!("line {} has an empty literal define", index + 1));
            };
            if !definition_names.insert(name.to_owned()) {
                return Err(format!("line {} repeats literal define {name}", index + 1));
            }
            definitions.push(definition.to_owned());
        }
    }

    let Some((line, output)) = rule else {
        return Err("the fragment has no literal header output rule".to_owned());
    };
    let Some(destination) = destination else {
        return Err("the fragment has no literal define recipes".to_owned());
    };
    if definitions.is_empty() {
        return Err("the selected profile produces an empty define header".to_owned());
    }
    Ok((line, output, destination, definitions))
}

fn marked_header_owner(
    content: &str,
    output: &str,
    expression_context: &MakeExprContext<'_>,
) -> Option<(String, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut owners = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#MM" {
            continue;
        }
        let Some(rule) = lines[index + 1..]
            .iter()
            .map(|candidate| candidate.trim())
            .find(|candidate| !candidate.is_empty() && !candidate.starts_with('#'))
        else {
            continue;
        };
        let Some((owner, prerequisites)) = rule.split_once(':') else {
            continue;
        };
        let owner = owner.trim();
        let words: Vec<&str> = prerequisites.split_whitespace().collect();
        if owner.is_empty()
            || owner.contains('$')
            || !owner.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
            || words.len() != 1
        {
            continue;
        }
        if evaluate_make_expr(words[0], expression_context)
            .ok()
            .as_deref()
            == Some(output)
        {
            owners.push((owner.to_owned(), words[0].to_owned()));
        }
    }
    owners.sort();
    owners.dedup();
    (owners.len() == 1).then(|| owners.remove(0))
}

/// Adopts a recipe-bearing local fragment only when one concrete declaration
/// proves ownership of both its in-tree sources and its complete literal
/// generated header.
// Each argument is an independent ownership boundary: the two competing
// scans, active target, directory-variable table, source root, declaring file
// and its directory/content. Grouping those paths would make it easier to mix
// a fragment's source-relative identity with the declaring mmakefile identity.
#[allow(clippy::too_many_arguments)]
fn declaration_owned_literal_define_scope(
    plain: &LocalMakeIncludeScan,
    candidate: &LocalMakeIncludeScan,
    target: Option<&TargetContext>,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    relative_path: &Path,
    relative_dir: &Path,
    original_content: &str,
) -> Option<Vec<DefineHeaderDecl>> {
    let target = target?;
    if candidate.expanded == plain.expanded
        || !candidate.issues.is_empty()
        || candidate.fragments.len() != 1
        || !candidate.fragments[0].literal_define_header
    {
        return None;
    }
    let fragment = &candidate.fragments[0];
    if fragment.included_from != relative_path || fragment.assigned_variables.is_empty() {
        return None;
    }

    let plain_joined = join_continuations(&plain.expanded);
    let candidate_joined = join_continuations(&candidate.expanded);
    let (plain_scope, plain_states) = collect_vars_impl(&plain_joined, Some(target));
    let (candidate_scope, candidate_states) = collect_vars_impl(&candidate_joined, Some(target));
    let mut ignored = Vec::new();
    let plain_invocations = select_target_invocations(
        &plain_joined,
        Some(&plain_states),
        relative_dir,
        &mut ignored,
    );
    ignored.clear();
    let candidate_invocations = select_target_invocations(
        &candidate_joined,
        Some(&candidate_states),
        relative_dir,
        &mut ignored,
    );
    if candidate_invocations.len() != plain_invocations.len()
        || candidate_invocations
            .iter()
            .zip(&plain_invocations)
            .any(|(candidate, plain)| candidate.name != plain.name || candidate.args != plain.args)
    {
        return None;
    }

    let mut provider: Option<(String, usize, String)> = None;
    if candidate_invocations
        .iter()
        .filter(|invocation| is_concrete_build_invocation(&invocation.name))
        .count()
        != 1
    {
        // Fragment assignments are file scope in Make. With more than one
        // concrete declaration they could alter a target which does not own
        // either the source inventory or the generated header.
        return None;
    }
    for (candidate_invocation, plain_invocation) in
        candidate_invocations.iter().zip(&plain_invocations)
    {
        if !is_concrete_build_invocation(&candidate_invocation.name) {
            continue;
        }
        let Some(files) = macro_arg(&candidate_invocation.args, "files") else {
            continue;
        };
        if !references_any_make_variable(&files, &fragment.assigned_variables) {
            continue;
        }
        let vars = candidate_scope.snapshot(candidate_invocation.line);
        let context = MakeExprContext::new(
            &candidate_scope,
            dirs,
            candidate_invocation.line,
            root,
            relative_dir,
        );
        let Ok(sources) = evaluate_macro_sources(&candidate_invocation.args, &vars, &context)
        else {
            return None;
        };
        if !sources.declared
            || sources.c.is_empty()
            || !sources.cxx.is_empty()
            || !sources.objc.is_empty()
            || !sources.asm.is_empty()
            || !sources.diagnostics.is_empty()
            || !sources
                .c
                .iter()
                .all(|source| in_tree_c_source_exists(root, relative_dir, source))
        {
            return None;
        }
        let plain_vars = plain_scope.snapshot(plain_invocation.line);
        let plain_context = MakeExprContext::new(
            &plain_scope,
            dirs,
            plain_invocation.line,
            root,
            relative_dir,
        );
        if evaluate_macro_sources(&plain_invocation.args, &plain_vars, &plain_context)
            .is_ok_and(|plain_sources| !plain_sources.is_empty())
        {
            return None;
        }
        let mmake = sanitize_ident(&macro_arg(&candidate_invocation.args, "mmake")?);
        if provider
            .replace((mmake, candidate_invocation.line, files))
            .is_some()
        {
            return None;
        }
    }
    let (provider, declaration_line, provider_files) = provider?;

    let fragment_content = read_source(&root.join(&fragment.path)).ok()?;
    let (rule_line, raw_output, recipe_destination, definitions) =
        collect_active_literal_defines(&fragment_content, &candidate_scope, target).ok()?;
    let context =
        MakeExprContext::new(&candidate_scope, dirs, declaration_line, root, relative_dir);
    let output = evaluate_make_expr(&raw_output, &context).ok()?;
    if !safe_define_header_output(&output)
        || output.rsplit('/').next() != Some(recipe_destination.as_str())
    {
        return None;
    }
    let (owner, owner_prerequisite) = marked_header_owner(original_content, &output, &context)?;

    if !literal_define_fragment_has_capability(
        &fragment.path,
        &provider,
        &owner,
        &provider_files,
        &raw_output,
        &owner_prerequisite,
        &fragment.assigned_variables,
    ) {
        return None;
    }

    // Work backwards from the only two products this mode may introduce: the
    // provider's source inventory and the one literal generated header. Every
    // fragment assignment must be in their transitive data/control closure;
    // merely being unused is not safe in Make because templates also consume
    // ambient file-scope properties without spelling them in this mmakefile.
    let product_closure = literal_define_fragment_product_closure(
        &fragment_content,
        &provider_files,
        &raw_output,
        &owner_prerequisite,
        &fragment.assigned_variables,
    )?;
    if product_closure.len() != fragment.assigned_variables.len()
        || fragment
            .assigned_variables
            .iter()
            .any(|variable| !product_closure.contains(variable))
    {
        return None;
    }

    // A reference elsewhere in the declaring file could alter a macro name,
    // output directory, link input, flags, or another rule. Count every
    // semantic reference and admit only the source expression and proven
    // owner's prerequisite. The exact capability manifest above separately
    // excludes provider-private and ambient template variables, even when one
    // has been made part of the product closure.
    let semantic_original = make_semantic_lines(original_content);
    for variable in &fragment.assigned_variables {
        let total = make_variable_reference_count(&semantic_original, variable);
        let allowed = make_variable_reference_count(&provider_files, variable)
            + make_variable_reference_count(&owner_prerequisite, variable);
        if total != allowed {
            return None;
        }
    }
    let mut dependencies = vec![
        format!("${{CMAKE_SOURCE_DIR}}/{}", fragment.path.display()),
        format!("${{CMAKE_SOURCE_DIR}}/{}", relative_path.display()),
    ];
    dependencies.sort();
    dependencies.dedup();

    Some(vec![DefineHeaderDecl {
        owner,
        file: fragment.path.display().to_string(),
        line: rule_line,
        output,
        definitions,
        dependencies,
        provider,
        consumers: Vec::new(),
    }])
}

fn parse_mmakefile_impl(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: Option<&TargetContext>,
    known_fetches: &[FetchDecl],
) -> Result<ParsedMmakefile> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();

    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    // Fetch recipes expand after the complete file has been read. Their
    // collector owns the existing bounded include traversal and supplies the
    // ownership proof used by the declaration-specific port scope below.
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
    let (fetches, skipped_fetches) =
        collect_fetches_with_scope(&content, &rel_dir, &collector_scope);
    let mut ownership_fetches = known_fetches.to_vec();
    ownership_fetches.extend(fetches.iter().cloned());

    // A small number of declarations keep a plain source inventory in a
    // sibling Make fragment. This remains the global default. A broader safe
    // variable scope is considered separately and adopted only when every
    // declaration is proven to compile sources owned by one of the fetches
    // above; there is deliberately no broad fallback.
    let plain_local_make_scan = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::PlainSourceLists,
    );
    let port_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    let port_scope_adopted = declaration_owned_port_scope(
        &plain_local_make_scan,
        &port_scope_candidate,
        target,
        dirs,
        root,
        &rel_dir,
        &ownership_fetches,
    );
    let define_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::LiteralDefineHeader,
    );
    let define_headers = (!port_scope_adopted)
        .then(|| {
            declaration_owned_literal_define_scope(
                &plain_local_make_scan,
                &define_scope_candidate,
                target,
                dirs,
                root,
                &relative_path,
                &rel_dir,
                &content,
            )
        })
        .flatten()
        .unwrap_or_default();
    let define_scope_adopted = !define_headers.is_empty();
    let local_make_scan = if port_scope_adopted {
        port_scope_candidate
    } else if define_scope_adopted {
        define_scope_candidate
    } else {
        plain_local_make_scan
    };
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
            let (scope, states) = if port_scope_adopted {
                collect_vars_impl_with_forward_locals(&joined, Some(target), true)
            } else {
                collect_vars_impl(&joined, Some(target))
            };
            (scope, Some(states))
        }
        None => (collect_vars(&joined), None),
    };
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
    // `%copy_dir_recursive` owns filesystem output, so unlike a generic
    // auxiliary macro it must not survive an inactive or unknown conditional.
    // Non-profiled parser callers still get a line-state scan: only their
    // unconditional declarations are safe to materialise.
    let fallback_copy_directory_states = conditional_line_states
        .is_none()
        .then(|| collect_vars_impl(&joined, None).1);
    let copy_directory_line_states = conditional_line_states
        .as_deref()
        .or(fallback_copy_directory_states.as_deref());
    let (copy_directories, skipped_copy_directories) = collect_copy_directories(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
    );
    let mut external_cmake = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_cmake")
    {
        let expression_context =
            MakeExprContext::new(&scope, dirs, invocation.line, root, &rel_dir);
        match parse_external_cmake_invocation(
            invocation,
            &expression_context,
            &rel_dir,
            &fetches,
            target,
            &content,
        ) {
            Ok(declaration) => external_cmake.push(declaration),
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                skipped_programs.push(format!(
                    "{}:{}: %build_with_cmake{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut configure_builds = Vec::new();
    let mut grub_builds = Vec::new();
    let mut ahi_builds = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_configure")
    {
        match parse_ahi_build_invocation(root, invocation, &rel_dir, target) {
            Ok(Some(declaration)) => ahi_builds.push(declaration),
            Ok(None) => match parse_grub2_build_invocation(root, invocation, &rel_dir, target) {
                Ok(Some(declaration)) => grub_builds.push(declaration),
                Ok(None) => {
                    match parse_configure_build_invocation(root, invocation, &rel_dir, target) {
                        Ok(declaration) => configure_builds.push(declaration),
                        Err(reason) => {
                            let mmake = macro_arg(&invocation.args, "mmake")
                                .map_or_else(String::new, |name| format!(" mmake={name}"));
                            skipped_programs.push(format!(
                                "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                                rel_dir.display(),
                                invocation.line + 1
                            ));
                        }
                    }
                }
                Err(reason) => {
                    let mmake = macro_arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |name| format!(" mmake={name}"));
                    skipped_programs.push(format!(
                        "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                        rel_dir.display(),
                        invocation.line + 1
                    ));
                }
            },
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                skipped_programs.push(format!(
                    "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut partial_source_lists: Vec<String> = Vec::new();
    let mut skipped_client_archives: Vec<String> = Vec::new();
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
        let declaration_flags =
            target.map_or_else(|| flag_set.clone(), |_| collect_flags_at(&scope, inv.line));
        let declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
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
        let linklib_name = match macro_arg(rest, "linklibname") {
            Some(raw) if !raw.is_empty() => match evaluate_name(&raw, &expression_context) {
                Ok(name) => Some(name),
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} linklibname={raw} is unresolved: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            },
            _ => None,
        };

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
        let always_cxx_link =
            match resolve_yes_argument(rest, "alwayscxxlink", &scope, dirs, inv.line) {
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

        // Upstream creates the client archive when `<mod>_LINKLIB` is
        // non-empty, and make.tmpl derives that from the file set, not from
        // `linklibname=`:
        //
        //   config/make.tmpl:2270  _LINKLIB is empty exactly when
        //                          _LINKLIBFILES, _LINKLIBAFILES,
        //                          linklibfiles= and _ARCHNLIBFILES are all
        //                          empty; linklibname= only renames it
        //   tools/genmodule/writemakefile.c:78
        //                          _LINKLIBFILES gets <mod>_getlibbase for
        //                          every LIBRARY, <mod>_autoinit under
        //                          OPTION_AUTOINIT and the stubs under
        //                          OPTION_STUBS
        //   tools/genmodule/config.c:797
        //                          a LIBRARY defaults to OPTION_AUTOINIT,
        //                          every other module type to NOAUTOINIT
        //
        // So every modtype=library module has a client archive, and so does
        // any other module whose config states `options stubs` or
        // `options autoinit` (rom/timer is the one such case in the tree).
        // Keying it on linklibname= left 100 library archives unbuilt, which
        // is what the symbol audit sees as undefined DOSBase, UtilityBase and
        // the rest: the base is defined by AROS_LIBSET in <mod>_autoinit.c
        // (compiler/include/aros/symbolsets.h:118), and that object lives in
        // exactly this archive.
        if module_type != ModuleType::Library {
            if let Some(facts) = read_genmodule_linklib_config(parent_dir, &mod_name) {
                if facts.forces_client_archive {
                    skipped_client_archives.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} modtype={mod_type_owned}: \
                         config states `options stubs` or `options autoinit`, so upstream builds \
                         lib{mod_name}.a; the generated client sources are only derived for \
                         modtype=library",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                }
            }
        }
        let genmodule_linklibs = if module_type == ModuleType::Library {
            read_genmodule_linklib_config(parent_dir, &mod_name).map(
                |GenmoduleConfigFacts {
                     has_relative,
                     relative_libraries,
                     forces_client_archive,
                 }| {
                    let mut inputs_exact = true;
                    let source_files = match evaluate_linklib_list(
                        rest,
                        "linklibfiles",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(files) => files,
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    let object_sources = match evaluate_linklib_list(
                        rest,
                        "linklibobjs",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(objects) => match map_linklib_object_sources(&objects, &sources.c) {
                            Ok(mapped) => mapped,
                            Err(error) => {
                                partial_source_lists.push(format!(
                                    "{}:{}: %{} mmake={mmake_raw} {error}",
                                    rel_dir.display(),
                                    inv.line + 1,
                                    inv.name
                                ));
                                inputs_exact = false;
                                Vec::new()
                            }
                        },
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    GenmoduleLinklibs {
                        enabled: linklib_name.is_some()
                            || forces_client_archive
                            || module_type == ModuleType::Library
                            || !source_files.is_empty()
                            || !object_sources.is_empty(),
                        has_relative,
                        relative_libraries,
                        source_files,
                        object_sources,
                        inputs_exact,
                    }
                },
            )
        } else {
            None
        };

        // All three %build_module* forms expand the implicit MetaMake
        // aliases and architecture endpoints.  `genmodule_only` describes
        // only how sources are materialised; using it as a guard here made
        // ordinary sourceful modules lose their upstream prerequisite graph.
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
            is_abi || genmodule_only,
        ));

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            genmodule_only,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
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
            linklib_name,
            genmodule_linklibs,
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
            linklib_output_dir: None,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
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
        let declaration_flags =
            target.map_or_else(|| flag_set.clone(), |_| collect_flags_at(&scope, inv.line));
        let declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
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
        let always_cxx_link =
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1
                    ));
                    continue;
                }
            };
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
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
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
            linklib_name: None,
            genmodule_linklibs: None,
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
            linklib_output_dir: None,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
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
        let mut declaration_flags =
            target.map_or_else(|| flag_set.clone(), |_| collect_flags_at(&scope, inv.line));
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        let mesa20_capability_sources =
            match mesa20_remaining_linklib_sources(root, &rel_dir, &mmake_name, target) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                    continue;
                }
            };
        let mesa20_capability_active = mesa20_capability_sources.is_some();
        let nouveau_drm_capability_sources =
            match nouveau_drm_sources(root, &rel_dir, &mmake_name, target) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                    continue;
                }
            };
        let nouveau_drm_capability_active = nouveau_drm_capability_sources.is_some();
        let nouveau_gallium_capability_sources = match nouveau_gallium_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium archive capability skipped: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                continue;
            }
        };
        let nouveau_gallium_capability_active = nouveau_gallium_capability_sources.is_some();
        match mesa20_compile_contract(&rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        match nouveau_drm_compile_contract(&rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        match nouveau_gallium_compile_contract(&rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        let mesa_sse41_profile = (mmake_name == MESA_SSE41_MMAKE
            && mesa_sse41_static_contract_is_pinned(root, &content))
        .then(|| mesa_sse41_profile(&rel_dir, target).ok().flatten())
        .flatten();
        let empty_archive = mesa_sse41_profile == Some(false);
        if let Some(x86_64) = mesa_sse41_profile {
            // The ordinary local-include scanner cannot adopt mesa.cfg for
            // this file on a cold tree: the neighbouring full libmesa target
            // still depends on the not-yet-fetched upstream inventory. Admit
            // the exact declaration-local view only together with the pinned
            // source, patch and profile contract validated below.
            declaration_flags.defines = mesa_sse41_defines(x86_64);
            declaration_flags.undefines.clear();
            declaration_flags.compile_options = mesa_sse41_compile_options(x86_64);
            declaration_flags.link_options.clear();
            declaration_includes.dirs = MESA_SSE41_INCLUDES
                .iter()
                .map(|include| (*include).to_owned())
                .collect();
            declaration_includes.arch_modules.clear();
        }

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
        let capability_files = mesa_sse41_profile.map(mesa_sse41_sources);
        let mut sources = if let Some(sources) = mesa20_capability_sources {
            sources
        } else if let Some(sources) = nouveau_drm_capability_sources {
            sources
        } else if let Some(sources) = nouveau_gallium_capability_sources {
            sources
        } else {
            match evaluate_macro_sources_with_files(
                &inv.args,
                &vars,
                &expression_context,
                capability_files
                    .as_deref()
                    .or(resolved_generated_files.as_deref()),
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
            }
        };
        record_partial_source_lists(
            &mut partial_source_lists,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() && !empty_archive {
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
        let always_cxx_link = if is_simple_module {
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
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
            false
        };
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
        let canonical_linklib_eligible = matches!(module_type, ModuleType::LinkLib)
            && macro_arg(&inv.args, "libdir").is_none()
            && macro_arg(&inv.args, "compiler").is_none_or(|value| value == "target")
            && !variant_32bit;
        let canonical_linklib_output = canonical_linklib_eligible
            && (all_sources_are_fetch_owned(&sources, &fetches)
                || nouveau_drm_capability_active
                || nouveau_gallium_capability_active);
        let linklib_output_dir = if mesa_sse41_profile.is_some() || mesa20_capability_active {
            Some(MESA20_PRIVATE_LIBDIR.to_owned())
        } else if matches!(module_type, ModuleType::LinkLib) {
            macro_arg(&inv.args, "libdir").and_then(|raw| {
                match evaluate_make_expr(&raw, &expression_context) {
                    Ok(directory) if safe_build_tree_output_directory(&directory) => {
                        Some(directory)
                    }
                    Ok(directory) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} resolves outside the build tree ({directory})",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                    Err(reason) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                }
            })
        } else {
            None
        };

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            genmodule_only: false,
            empty_archive,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
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
            linklib_name: None,
            genmodule_linklibs: None,
            canonical_linklib_output,
            canonical_linklib_eligible,
            linklib_output_dir,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
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

    if let Err(reason) = validate_mesa_sse41_capability(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        // The ordinary parser may have resolved part of this declaration, but
        // executable empty-archive support and the target-only ISA flag are
        // admitted as one atomic capability. Any drift removes the target.
        targets.retain(|candidate| candidate.mmake_name != MESA_SSE41_MMAKE);
        skipped_programs.push(format!(
            "{}: Mesa SSE4.1 link library skipped: {reason}",
            rel_dir.display()
        ));
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == NOUVEAU_DRM_MMAKE)
    {
        if let Err(reason) = validate_nouveau_drm_capability(root, &rel_dir, target, &targets) {
            // The DRM source fragment is intentionally admitted only as one
            // closed capability.  Do not leave a partially inferred target in
            // the graph when its recipe, inventory or canonical archive proof
            // has drifted.
            targets.retain(|candidate| candidate.mmake_name != NOUVEAU_DRM_MMAKE);
            skipped_programs.push(format!(
                "{}: Nouveau DRM link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == NOUVEAU_GALLIUM_MMAKE)
    {
        if let Err(reason) = validate_nouveau_gallium_capability(root, &rel_dir, target, &targets) {
            // The fetched Mesa lane contains a C++ source inventory. Keep it
            // atomic with its pinned source and flag contract rather than
            // leaving an inferred C-only or private-output approximation in
            // the graph.
            targets.retain(|candidate| candidate.mmake_name != NOUVEAU_GALLIUM_MMAKE);
            skipped_programs.push(format!(
                "{}: Nouveau Gallium link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    let mut python_outputs = Vec::new();
    match parse_glapi_python_outputs(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => skipped_programs.push(format!(
            "{}: Mesa glapi Python generator skipped: {reason}",
            rel_dir.display()
        )),
    }
    match parse_mesautil_python_outputs(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => skipped_programs.push(format!(
            "{}: Mesa utility Python generator skipped: {reason}",
            rel_dir.display()
        )),
    }
    let mesa20_required_target = match rel_dir.to_str() {
        Some("workbench/libs/mesa/libcompiler") => Some("mesa3d-linklib-compiler"),
        Some("workbench/libs/mesa/libgalliumaux") => Some("mesa3d-linklib-galliumauxiliary"),
        Some("workbench/libs/mesa/libmesa") => Some("mesa3d-linklib-mesa"),
        Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium")
            if mesa20_current_profile(target).ok() != Some("x86_64") =>
        {
            Some("linklibs-gallium_vc4")
        }
        _ => None,
    };
    match parse_mesa20_remaining_python_outputs(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            if let Some(mmake) = mesa20_required_target {
                // Source admission and every generator product form one
                // capability. A partial archive with missing generated
                // translation units is never an executable fallback.
                targets.retain(|candidate| candidate.mmake_name != mmake);
            }
            skipped_programs.push(format!(
                "{}: Mesa 20.0.8 archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    // Paired FlexCat recipes are normal Make rules rather than a MetaMake
    // macro.  Parse them after all concrete source lists are known, so the
    // graph can bind their generated `locale.c` only to real consumers.
    let flexcat_scan = collect_flexcat_source_rules(&content, root, &rel_dir, &scope, dirs);

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
        external_cmake,
        configure_builds,
        grub_builds,
        ahi_builds,
        python_outputs,
        flexcat_sources: flexcat_scan.declarations,
        skipped_flexcat_sources: flexcat_scan.skipped,
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
        copy_directories,
        skipped_copy_directories,
        adhoc_header_rules: copy_scan.adhoc,
        header_transforms: copy_scan.transforms,
        define_headers,
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
        skipped_client_archives,
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
        join_mm_continuations, macro_arg, macro_argument_names, macro_invocations,
        mesa20_compile_contract, mesa20_remaining_linklib_sources,
        mesa_sse41_static_contract_is_pinned, parse_ahi_build_invocation,
        parse_external_cmake_invocation, parse_glapi_python_outputs, parse_mesautil_python_outputs,
        render_meta_token, resolve_module_suffix, resolve_module_target_dir, sanitize_ident,
        select_target_invocations, validate_mesa_sse41_capability, MakeExprContext, TargetContext,
        MESA_PATCH_SHA256, MESA_SSE41_MMAKE, META_RULE_RE,
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
    fn target_context_selects_external_cmake_invocations() {
        let joined = join_continuations(
            "ifeq ($(AROS_TARGET_CPU),x86_64)\n\
             %build_with_cmake mmake=cmake-x86 srcdir=x prefix=x extraoptions=x\n\
             endif\n\
             ifeq ($(AROS_TARGET_CPU),arm)\n\
             %build_with_cmake mmake=cmake-arm srcdir=x prefix=x extraoptions=x\n\
             endif\n\
             ifeq ($(UNKNOWN_EXTERNAL_SWITCH),yes)\n\
             %build_with_cmake mmake=cmake-unknown srcdir=x prefix=x extraoptions=x\n\
             endif\n",
        );

        for (context, expected) in [
            (target_context("x86_64", "pc", ""), "cmake-x86"),
            (target_context("arm", "raspi", "hard"), "cmake-arm"),
        ] {
            let (_, states) = collect_vars_impl(&joined, Some(&context));
            let mut skipped = Vec::new();
            let invocations = select_target_invocations(
                &joined,
                Some(&states),
                Path::new("fixture"),
                &mut skipped,
            );
            let selected: Vec<_> = invocations
                .iter()
                .filter(|invocation| invocation.name == "build_with_cmake")
                .filter_map(|invocation| macro_arg(&invocation.args, "mmake"))
                .collect();
            assert_eq!(selected, [expected]);
            assert_eq!(skipped.len(), 1, "{skipped:#?}");
            assert!(
                skipped[0].contains("%build_with_cmake mmake=cmake-unknown"),
                "{skipped:#?}"
            );
        }
    }

    #[test]
    fn macro_argument_scanner_ignores_nested_and_quoted_assignments() {
        assert_eq!(
            macro_argument_names(
                "mmake=x extraoptions=\"-DFOO=yes INNER=not-an-argument\" \
                 srcdir=$(if $(COND),A=B,C=D) prefix=x"
            ),
            ["mmake", "extraoptions", "srcdir", "prefix"]
        );
    }

    fn parsed_cunit_capability() -> (
        super::Invocation,
        super::VarScope,
        Vec<crate::fetch::FetchDecl>,
    ) {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let profile = target_context("x86_64", "pc", "");
        let (scope, states) = collect_vars_impl(&joined, Some(&profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| invocation.name == "build_with_cmake")
                .unwrap();
        assert!(skipped.is_empty(), "{skipped:#?}");
        let (fetches, skipped_fetches) =
            crate::fetch::collect_fetches_with_scope(&content, relative_dir, &scope);
        assert!(skipped_fetches.is_empty(), "{skipped_fetches:#?}");
        (invocation, scope, fetches)
    }

    fn parsed_ahi_capability() -> (super::Invocation, String) {
        let root = root();
        let relative_dir = Path::new("workbench/devs/AHI");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let profile = target_context("x86_64", "pc", "");
        let (_, states) = collect_vars_impl(&joined, Some(&profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| {
                    invocation.name == "build_with_configure"
                        && macro_arg(&invocation.args, "mmake").as_deref()
                            == Some("workbench-devs-AHI-subsystem")
                })
                .unwrap();
        assert!(skipped.is_empty(), "{skipped:#?}");
        (invocation, content)
    }

    #[test]
    fn ahi_capability_rejects_macro_profile_and_mmakefile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/devs/AHI");
        let profile = target_context("x86_64", "pc", "");
        let (invocation, content) = parsed_ahi_capability();

        assert!(
            parse_ahi_build_invocation(&root, &invocation, relative_dir, Some(&profile))
                .unwrap()
                .is_some()
        );

        let mut changed = invocation.clone();
        changed.args = changed.args.replace("gnuflags=no", "gnuflags=yes");
        assert!(
            parse_ahi_build_invocation(&root, &changed, relative_dir, Some(&profile))
                .unwrap_err()
                .contains("gnuflags uses")
        );

        let unsupported_profile = target_context("arm", "raspi", "soft");
        assert!(parse_ahi_build_invocation(
            &root,
            &invocation,
            relative_dir,
            Some(&unsupported_profile)
        )
        .unwrap_err()
        .contains("AHI subsystem capability only supports"));

        let tree = TempTree::new();
        let drifted = tree.0.join(relative_dir).join("mmakefile.src");
        fs::create_dir_all(drifted.parent().unwrap()).unwrap();
        fs::write(&drifted, format!("{content}\n# audited-input drift\n")).unwrap();
        assert!(
            parse_ahi_build_invocation(&tree.0, &invocation, relative_dir, Some(&profile))
                .unwrap_err()
                .contains("AHI subsystem mmakefile differs")
        );
    }

    #[test]
    fn cunit_external_cmake_capability_is_complete_and_exact() {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let (invocation, scope, fetches) = parsed_cunit_capability();
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let declaration = parse_external_cmake_invocation(
            &invocation,
            &expression_context,
            relative_dir,
            &fetches,
            None,
            "",
        )
        .unwrap();

        assert_eq!(declaration.mmake_name, "linklibs-yes-cunit");
        assert_eq!(
            declaration.source_dir,
            "${AROS_PORTS_DIR}/cunit/cunit-3.5.5"
        );
        assert_eq!(
            declaration.install_prefix,
            "${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras"
        );
        assert_eq!(declaration.fetch_target, "cunit-fetch");
        assert_eq!(declaration.provided_library, "cunit");
        assert_eq!(
            declaration.provider_target,
            "linklibs-yes-cunit-external-cunit"
        );
        assert_eq!(
            declaration.library_products,
            ["${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/lib/libcunit.a"]
        );
        assert_eq!(
            declaration.public_include_dirs,
            ["${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include"]
        );
        assert_eq!(declaration.header_products.len(), 19);
        assert!(declaration.auxiliary_products.is_empty());
        assert_eq!(
            declaration.header_products.first().map(String::as_str),
            Some("${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/Automated.h")
        );
        assert_eq!(
            declaration.header_products.last().map(String::as_str),
            Some("${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/wxWidget.h")
        );
        assert_eq!(
            declaration.options,
            [
                "-DCUNIT_DISABLE_EXAMPLES=yes",
                "-DCUNIT_DISABLE_TESTS=yes",
                "-DCMAKE_BUILD_TYPE=DEBUG",
                "-Wno-error=dev",
            ]
        );
    }

    #[test]
    fn cunit_external_cmake_capability_rejects_any_contract_drift() {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let (invocation, scope, fetches) = parsed_cunit_capability();
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let parse = |invocation: &super::Invocation, fetches: &[crate::fetch::FetchDecl]| {
            parse_external_cmake_invocation(
                invocation,
                &expression_context,
                relative_dir,
                fetches,
                None,
                "",
            )
            .unwrap_err()
        };

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "srcdir=$(PORTSDIR)/cunit/$(ARCHBASE)",
            "srcdir=$(AROS_DEVELOPER)",
        );
        assert!(parse(&changed, &fetches).contains("srcdir resolves to"));

        let mut changed = invocation.clone();
        changed.args = changed
            .args
            .replace("prefix=$(AROS_CONTRIB_SDK)", "prefix=$(AROS_DEVELOPER)");
        assert!(parse(&changed, &fetches).contains("prefix resolves to"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "extraoptions=$(CUNIT_CMAKE_FLAGS)",
            "extraoptions=-DUNAUDITED=yes",
        );
        assert!(parse(&changed, &fetches).contains("extraoptions resolve to"));

        let mut changed = invocation.clone();
        changed.args.push_str(" compiler=host");
        assert!(parse(&changed, &fetches).contains("argument set"));

        let mut changed_fetches = fetches;
        changed_fetches[0].archive = "cunit-unreviewed".to_owned();
        assert!(parse(&invocation, &changed_fetches).contains("archive is"));

        assert!(parse(&invocation, &[]).contains("exactly one"));
    }

    fn parsed_aom_capability(
        profile: &TargetContext,
    ) -> (
        super::Invocation,
        super::VarScope,
        Vec<crate::fetch::FetchDecl>,
        String,
    ) {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let (scope, states) = collect_vars_impl(&joined, Some(profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| {
                    invocation.name == "build_with_cmake"
                        && macro_arg(&invocation.args, "mmake").as_deref()
                            == Some("datatypes-heic-linklibs-aom")
                })
                .unwrap();
        assert!(
            skipped
                .iter()
                .all(|diagnostic| !diagnostic.contains("datatypes-heic-linklibs-aom")),
            "{skipped:#?}"
        );
        let (fetches, skipped_fetches) =
            crate::fetch::collect_fetches_with_scope(&content, relative_dir, &scope);
        assert!(
            skipped_fetches
                .iter()
                .all(|diagnostic| !diagnostic.contains("linklibs-aom-fetch")),
            "{skipped_fetches:#?}"
        );
        (invocation, scope, fetches, content)
    }

    #[test]
    fn aom_external_cmake_capability_is_profile_exact() {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let directory_vars = dirs();
        for (profile, specific) in [
            (
                target_context("x86_64", "pc", ""),
                vec!["-DAOM_TARGET_CPU=generic"],
            ),
            (
                target_context("arm", "raspi", "hard"),
                vec![
                    "-DAOM_TARGET_CPU=arm",
                    "-DENABLE_NEON=0",
                    "-DCONFIG_RUNTIME_CPU_DETECT=0",
                ],
            ),
            (
                target_context("aarch64", "raspi", ""),
                vec!["-DAOM_TARGET_CPU=generic"],
            ),
        ] {
            let (invocation, scope, fetches, content) = parsed_aom_capability(&profile);
            let expression_context = MakeExprContext::new(
                &scope,
                &directory_vars,
                invocation.line,
                &root,
                relative_dir,
            );
            let declaration = parse_external_cmake_invocation(
                &invocation,
                &expression_context,
                relative_dir,
                &fetches,
                Some(&profile),
                &content,
            )
            .unwrap();
            let mut expected: Vec<_> = super::AOM_COMMON_OPTIONS
                .iter()
                .map(|option| (*option).to_owned())
                .collect();
            expected.extend(specific.into_iter().map(str::to_owned));

            assert_eq!(declaration.mmake_name, "datatypes-heic-linklibs-aom");
            assert_eq!(
                declaration.provider_target,
                "datatypes-heic-linklibs-aom-external-aom"
            );
            assert_eq!(
                declaration.source_dir,
                "${AROS_PORTS_DIR}/libaom/libaom-3.12.1"
            );
            assert_eq!(
                declaration.binary_dir,
                "${AROS_BUILD_DIR}/gen/external-cmake/workbench/classes/datatypes/heic/aom"
            );
            assert_eq!(
                declaration.install_prefix,
                "${AROS_BUILD_DIR}/SYS/Developer"
            );
            assert_eq!(declaration.fetch_target, "linklibs-aom-fetch");
            assert_eq!(declaration.provided_library, "aom");
            assert_eq!(declaration.header_products.len(), 7);
            assert!(declaration
                .options
                .contains(&"-DCMAKE_BUILD_TYPE=Release".to_owned()));
            assert_eq!(
                declaration.auxiliary_products,
                ["${AROS_BUILD_DIR}/SYS/Developer/lib/pkgconfig/aom.pc"]
            );
            assert_eq!(declaration.options, expected);
        }
    }

    #[test]
    fn aom_external_cmake_capability_rejects_declaration_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/classes/datatypes/heic");
        let profile = target_context("x86_64", "pc", "");
        let (invocation, scope, fetches, content) = parsed_aom_capability(&profile);
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let parse = |invocation: &super::Invocation,
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext,
                     content: &str| {
            parse_external_cmake_invocation(
                invocation,
                &expression_context,
                relative_dir,
                fetches,
                Some(profile),
                content,
            )
            .unwrap_err()
        };

        let mut changed = invocation.clone();
        changed.args.push_str(" compiler=host");
        assert!(parse(&changed, &fetches, &profile, &content).contains("argument set"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace("package=aom", "package=other");
        assert!(parse(&changed, &fetches, &profile, &content).contains("package uses"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "extraldflags=\"$(LIBAOM_LDFLAGS)\"",
            "extraldflags=\"-lstdc++\"",
        );
        assert!(parse(&changed, &fetches, &profile, &content).contains("extraldflags uses"));

        let changed_content = content.replace("-DENABLE_TESTS=OFF", "-DENABLE_TESTS=ON");
        assert!(parse(&invocation, &fetches, &profile, &changed_content)
            .contains("declaration block differs"));

        let changed_content = content.replace(
            "LIBAOM_LDFLAGS+=$(TARGET_CXX_LDFLAGS)",
            "LIBAOM_LDFLAGS+=-Wl,--unreviewed",
        );
        assert!(parse(&invocation, &fetches, &profile, &changed_content)
            .contains("declaration block differs"));

        let mut changed_fetches = fetches.clone();
        changed_fetches[0].origins = "https://unreviewed.invalid".to_owned();
        assert!(
            parse(&invocation, &changed_fetches, &profile, &content).contains("archive_origins")
        );

        let mut changed_fetches = fetches.clone();
        changed_fetches[0].patches = "unreviewed.diff".to_owned();
        assert!(parse(&invocation, &changed_fetches, &profile, &content).contains("patches_specs"));

        let mut unsupported = profile;
        unsupported.toolchain = Some("gnu".to_owned());
        assert!(parse(&invocation, &fetches, &unsupported, &content)
            .contains("does not support target profile"));
    }

    #[test]
    fn glapi_python_capability_rejects_recipe_source_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libglapi");
        let profile = target_context("x86_64", "pc", "");
        let central_fetches = super::collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join(relative_dir).join("mmakefile.src"),
            &root,
            &dirs(),
            &profile,
            &central_fetches,
        )
        .unwrap();
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let parse = |content: &str,
                     targets: &[crate::ast::TargetDefinition],
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext| {
            parse_glapi_python_outputs(relative_dir, Some(profile), content, targets, fetches)
                .unwrap_err()
        };

        let changed_content = content.replace("gl_table.py", "unreviewed_table.py");
        assert!(parse(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe block differs"));

        let mut changed_targets = parsed.targets.clone();
        let glapi = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-glapi")
            .unwrap();
        glapi.source_files.pop();
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(parse(&content, &parsed.targets, &changed_fetches, &profile)
            .contains("fetch declaration differs"));
        assert!(parse(&content, &parsed.targets, &[], &profile).contains("exactly one"));

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(parse(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));
    }

    #[test]
    fn mesautil_python_capability_rejects_recipe_source_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libmesautil");
        let profile = target_context("x86_64", "pc", "");
        let mut central_fetches = super::collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        central_fetches.extend(
            super::collect_mmakefile_fetches_with_context(
                &root.join("workbench/libs/z/mmakefile.src"),
                &root,
                &profile,
            )
            .unwrap(),
        );
        let parsed = super::parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join(relative_dir).join("mmakefile.src"),
            &root,
            &dirs(),
            &profile,
            &central_fetches,
        )
        .unwrap();
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let parse = |content: &str,
                     targets: &[crate::ast::TargetDefinition],
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext| {
            parse_mesautil_python_outputs(relative_dir, Some(profile), content, targets, fetches)
                .unwrap_err()
        };

        let changed_content =
            content.replace("$(Q)$(PYTHON)  $^ > $@", "$(Q)python-unreviewed $^ > $@");
        assert!(parse(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe block differs"));

        let mut changed_targets = parsed.targets.clone();
        let mesautil = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-mesautil")
            .unwrap();
        mesautil.source_files.pop();
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_targets = parsed.targets.clone();
        let mesadevutil = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-mesadevutil")
            .unwrap();
        mesadevutil
            .defines
            .retain(|define| define != "EMBEDDED_DEVICE");
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(parse(&content, &parsed.targets, &changed_fetches, &profile)
            .contains("fetch declaration differs"));
        assert!(parse(&content, &parsed.targets, &[], &profile).contains("exactly one"));

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(parse(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));
    }

    #[test]
    fn mesa_sse41_capability_rejects_recipe_target_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libmesa");
        let profile = target_context("x86_64", "pc", "");
        let central_fetches = super::collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join(relative_dir).join("mmakefile.src"),
            &root,
            &dirs(),
            &profile,
            &central_fetches,
        )
        .unwrap();
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let validate = |content: &str,
                        targets: &[crate::ast::TargetDefinition],
                        fetches: &[crate::fetch::FetchDecl],
                        profile: &TargetContext| {
            validate_mesa_sse41_capability(
                &root,
                relative_dir,
                Some(profile),
                content,
                targets,
                fetches,
            )
            .unwrap_err()
        };

        let changed_content = content.replace(
            "TARGET_ISA_CFLAGS += -msse4.1",
            "TARGET_ISA_CFLAGS += -msse4.2",
        );
        assert!(validate(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe, configuration context, source manifest or local patch differs"));

        let changed_local_context = content.replace(
            "-iquote $(top_builddir)/$(CUR_MESADIR)/main",
            "-iquote $(top_builddir)/$(CUR_MESADIR)/unreviewed-main",
        );
        assert!(validate(
            &changed_local_context,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe, configuration context, source manifest or local patch differs"));

        let changed_manifest_include = content.replace(
            "include $(SRCDIR)/$(CURDIR)/mesa-sse41-20.0.8.sources",
            "include $(SRCDIR)/$(CURDIR)/mesa-sse41-unreviewed.sources",
        );
        assert!(validate(
            &changed_manifest_include,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe, configuration context, source manifest or local patch differs"));

        let changed_intervening_context = content.replace(
            "MESA3D_GALLIUM_SSE41_SOURCES :=",
            "USER_CFLAGS += -funreviewed\n\nMESA3D_GALLIUM_SSE41_SOURCES :=",
        );
        assert!(validate(
            &changed_intervening_context,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe, configuration context, source manifest or local patch differs"));

        let disabled_fetch_edge = content.replace(
            "#MM mesa3d-linklib-mesa-sse41 : mesa3d-fetch",
            "#MM- mesa3d-linklib-mesa-sse41 : mesa3d-fetch",
        );
        assert!(validate(
            &disabled_fetch_edge,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("recipe, configuration context, source manifest or local patch differs"));

        let mut changed_targets = parsed.targets.clone();
        let sse41 = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == MESA_SSE41_MMAKE)
            .unwrap();
        sse41.source_files.pop();
        assert!(
            validate(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, empty-archive, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(
            validate(&content, &parsed.targets, &changed_fetches, &profile)
                .contains("fetch declaration differs")
        );

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(validate(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));

        let pinned_tree = TempTree::new();
        for relative in [
            "workbench/libs/mesa/mesa.cfg",
            "workbench/libs/mesa/mesa-20.0.8-aros.diff",
            "workbench/libs/mesa/libmesa/mesa-sse41-20.0.8.sources",
        ] {
            let destination = pinned_tree.0.join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(root.join(relative), destination).unwrap();
        }
        assert!(mesa_sse41_static_contract_is_pinned(
            &pinned_tree.0,
            &content
        ));
        let config_path = pinned_tree.0.join("workbench/libs/mesa/mesa.cfg");
        let changed_config = read_source(&config_path).unwrap().replace(
            "aros_mesadir := workbench/libs/mesa",
            "aros_mesadir := workbench/libs/mesa-unreviewed",
        );
        fs::write(config_path, changed_config).unwrap();
        assert!(!mesa_sse41_static_contract_is_pinned(
            &pinned_tree.0,
            &content
        ));
    }

    #[test]
    fn mesa20_placement_new_shim_is_limited_to_two_cxx_lanes() {
        let profile = target_context("x86_64", "pc", "");
        let shim = "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>";
        for (relative_dir, mmake, expects_shim) in [
            (
                "workbench/libs/mesa/libcompiler",
                "mesa3d-linklib-compiler",
                true,
            ),
            (
                "workbench/libs/mesa/libgalliumaux",
                "mesa3d-linklib-galliumauxiliary",
                false,
            ),
            ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa", true),
        ] {
            let contract = mesa20_compile_contract(Path::new(relative_dir), mmake, Some(&profile))
                .unwrap()
                .unwrap();
            assert_eq!(
                contract.options.iter().any(|option| option == shim),
                expects_shim,
                "{mmake}"
            );
            assert!(
                contract
                    .includes
                    .iter()
                    .all(|include| !include.contains("cxx-compat")),
                "{mmake}: the shim must never become a C-visible include directory"
            );
        }

        for cpu in ["arm", "aarch64"] {
            let profile = target_context(cpu, "raspi", if cpu == "arm" { "hard" } else { "" });
            let contract = mesa20_compile_contract(
                Path::new("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert!(
                contract.options.iter().all(|option| option != shim),
                "{cpu}"
            );
            assert!(
                contract
                    .includes
                    .iter()
                    .all(|include| !include.contains("cxx-compat")),
                "{cpu}"
            );
        }
    }

    #[test]
    fn mesa20_release_patch_and_archive_inventories_are_exact() {
        let root = root();
        let patch_relative = "workbench/libs/mesa/mesa-20.0.8-aros.diff";
        assert!(super::file_has_sha256(
            &root,
            patch_relative,
            MESA_PATCH_SHA256
        ));
        let patch = read_source(&root.join(patch_relative)).unwrap();
        for required in [
            "-#include <algorithm>",
            "st_glsl_to_tgsi_private.h mesa-20.0.8.aros/src/mesa/state_tracker/st_glsl_to_tgsi_private.h",
            "+#ifndef NDEBUG",
            "while (j > 0 && sorter(value, decls[j - 1]))",
            "while (j > 0 && sort_by_begin(value, ranges[j - 1]))",
            "int *idx_map = (int *) CALLOC(narrays + 1, sizeof(*idx_map));",
            "if (!idx_map || (narrays > 0 && !old_sizes))",
            "if (narrays > 0)\n+      memcpy(&old_sizes[0]",
            "temp_comp_access::conditionality_untouched = INT_MAX;",
            "qsort(reg_access, used_temps, sizeof(register_merge_record)",
        ] {
            assert!(
                patch.contains(required),
                "missing release-patch contract: {required}"
            );
        }
        for forbidden in [
            "+#include <memory>",
            "+#include <limits>",
            "+#include <algorithm>",
            "+   std::sort(inout_decls.begin(), inout_decls.end()",
            "+   unique_ptr<int[]>",
        ] {
            assert!(
                !patch.contains(forbidden),
                "release patch reintroduced a target STL dependency: {forbidden}"
            );
        }

        for (cpu, platform, float_abi, expected) in [
            ("x86_64", "pc", "", (239, 11, 1)),
            ("arm", "raspi", "hard", (238, 11, 0)),
            ("aarch64", "raspi", "", (238, 11, 0)),
        ] {
            let profile = target_context(cpu, platform, float_abi);
            let sources = mesa20_remaining_linklib_sources(
                &root,
                Path::new("workbench/libs/mesa/libmesa"),
                "mesa3d-linklib-mesa",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                expected,
                "{cpu}"
            );
            if cpu == "x86_64" {
                assert_eq!(
                    sources.asm,
                    ["${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/x86-64/xform4.S"]
                );
            }
        }

        let x86 = target_context("x86_64", "pc", "");
        for (relative, mmake, expected) in [
            (
                "workbench/libs/mesa/libcompiler",
                "mesa3d-linklib-compiler",
                (154, 105, 0),
            ),
            (
                "workbench/libs/mesa/libgalliumaux",
                "mesa3d-linklib-galliumauxiliary",
                (176, 0, 0),
            ),
        ] {
            let sources =
                mesa20_remaining_linklib_sources(&root, Path::new(relative), mmake, Some(&x86))
                    .unwrap()
                    .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                expected,
                "{mmake}"
            );
        }

        for (cpu, float_abi) in [("arm", "hard"), ("aarch64", "")] {
            let profile = target_context(cpu, "raspi", float_abi);
            let sources = mesa20_remaining_linklib_sources(
                &root,
                Path::new("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                (43, 0, 0)
            );
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
    fn boost_recursive_copies_render_sdk_roots_and_port_source() {
        let root = root();
        let dirs = dirs();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &root.join("compiler/boost/mmakefile.src"),
            &root,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert!(parsed.skipped_copy_directories.is_empty(), "{parsed:#?}");
        assert_eq!(parsed.copy_directories.len(), 4);
        let geninc = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-geninc-copy")
            .expect("GENINCDIR staging declaration");
        assert_eq!(geninc.source, "${AROS_PORTS_DIR}/boost/boost_1_89_0/boost");
        assert_eq!(geninc.destination, "${AROS_GENINC_DIR}/boost");
        let sdk = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-includes-copy")
            .expect("SDK staging declaration");
        assert_eq!(sdk.source, geninc.source);
        assert_eq!(sdk.destination, "${AROS_SDK_INCLUDE_DIR}/boost");

        // The in-tree subset stages the same two destinations from
        // compiler/boost/include, for the release closure that must not fetch.
        let subset_geninc = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-subset-geninc-copy")
            .expect("subset GENINCDIR staging declaration");
        assert_eq!(
            subset_geninc.source,
            "${CMAKE_SOURCE_DIR}/compiler/boost/include/boost"
        );
        assert_eq!(subset_geninc.destination, geninc.destination);
        let subset_sdk = parsed
            .copy_directories
            .iter()
            .find(|declaration| declaration.name == "compiler-boost-subset-includes-copy")
            .expect("subset SDK staging declaration");
        assert_eq!(subset_sdk.source, subset_geninc.source);
        assert_eq!(subset_sdk.destination, sdk.destination);
    }

    #[test]
    fn recursive_copy_collector_rejects_host_paths_and_unaudited_excludes() {
        let tree = TempTree::new();
        let module = tree.0.join("module");
        fs::create_dir_all(&module).unwrap();
        fs::create_dir_all(module.join("assets")).unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%copy_dir_recursive mmake=safe-copy src=assets/. dst=$(TARGETDIR)/staged\n\
             %copy_dir_recursive mmake=host-copy src=/tmp/host dst=$(TARGETDIR)/staged\n\
             %copy_dir_recursive mmake=filtered-copy src=assets dst=$(TARGETDIR)/staged excludefiles=\"*.py\"\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert_eq!(parsed.copy_directories.len(), 1, "{parsed:#?}");
        assert_eq!(parsed.copy_directories[0].name, "safe-copy");
        assert_eq!(
            parsed.copy_directories[0].source,
            "${CMAKE_SOURCE_DIR}/module/assets"
        );
        assert_eq!(
            parsed.copy_directories[0].destination,
            "${AROS_BUILD_DIR}/staged"
        );
        assert_eq!(parsed.skipped_copy_directories.len(), 2, "{parsed:#?}");
        assert!(parsed
            .skipped_copy_directories
            .iter()
            .any(|message| message.contains("host-copy")));
        assert!(parsed
            .skipped_copy_directories
            .iter()
            .any(|message| message.contains("filtered-copy")));
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
    fn a_posix_simple_assignment_is_not_mistaken_for_colon_equals() {
        let scope = collect_vars("A ::= immediate\n%build_prog mmake=x progname=X\n");
        assert_eq!(scope.raw_at("A", usize::MAX).as_deref(), Some("immediate"));
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
            assert_eq!(parsed.fetches.len(), 3, "{cpu}");
            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "mesa3d-fetch")
                .unwrap();
            assert_eq!(fetch.name, "mesa3d-fetch");
            assert_eq!(fetch.archive, "mesa-20.0.8");
            assert_eq!(fetch.suffixes, "tar.xz tar.gz");
            assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/mesa");
            assert_eq!(fetch.location, "${AROS_PORTS_SOURCE_DIR}");
            assert!(fetch.origins.ends_with("older-versions/20.x"));
            assert_eq!(fetch.patches, "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1");
            for (name, archive, origin) in [
                (
                    "mesa3d-mako-fetch",
                    "mako-1.3.10",
                    "https://files.pythonhosted.org/packages/9e/38/bd5b78a920a64d708fe6bc8e0a2c075e1389d53bef8413725c63ba041535",
                ),
                (
                    "mesa3d-markupsafe-fetch",
                    "markupsafe-3.0.2",
                    "https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62",
                ),
            ] {
                let package = parsed
                    .fetches
                    .iter()
                    .find(|fetch| fetch.name == name)
                    .unwrap();
                assert_eq!(package.archive, archive);
                assert_eq!(package.suffixes, "tar.gz");
                assert_eq!(package.origins, origin);
                assert_eq!(package.destination, "${AROS_PORTS_DIR}/mesa-python");
                assert_eq!(package.location, "${AROS_PORTS_SOURCE_DIR}");
                assert_eq!(package.patches, "::");
            }

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
    fn zstd_plain_source_inventory_is_cold_fetch_exact_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/zstd/mmakefile.src");
        let expected: Vec<String> = [
            "lib/common/debug",
            "lib/common/entropy_common",
            "lib/common/error_private",
            "lib/common/fse_decompress",
            "lib/common/pool",
            "lib/common/threading",
            "lib/common/xxhash",
            "lib/common/zstd_common",
            "lib/compress/fse_compress",
            "lib/compress/hist",
            "lib/compress/huf_compress",
            "lib/compress/zstd_compress",
            "lib/compress/zstd_compress_literals",
            "lib/compress/zstd_compress_sequences",
            "lib/compress/zstd_compress_superblock",
            "lib/compress/zstd_double_fast",
            "lib/compress/zstd_fast",
            "lib/compress/zstd_lazy",
            "lib/compress/zstd_ldm",
            "lib/compress/zstd_opt",
            "lib/compress/zstd_preSplit",
            "lib/compress/zstdmt_compress",
            "lib/decompress/huf_decompress",
            "lib/decompress/zstd_ddict",
            "lib/decompress/zstd_decompress",
            "lib/decompress/zstd_decompress_block",
            "lib/dictBuilder/cover",
            "lib/dictBuilder/divsufsort",
            "lib/dictBuilder/fastcover",
            "lib/dictBuilder/zdict",
        ]
        .into_iter()
        .map(|stem| format!("${{AROS_PORTS_DIR}}/zstd/zstd-1.5.7/{stem}"))
        .collect();

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

            let module = targets
                .get("workbench-libs-zstd-library")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            let static_lib = targets
                .get("linklibs-zstd")
                .unwrap_or_else(|| panic!("{cpu}-{platform}: {:#?}", parsed.skipped_programs));
            for target in [module, static_lib] {
                assert_eq!(
                    target.source_files, expected,
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert_eq!(
                    target.include_dirs,
                    ["${CMAKE_SOURCE_DIR}/workbench/libs/zstd"],
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert_eq!(
                    target.defines,
                    ["ZSTD_NO_TRACE"],
                    "{cpu}: {}",
                    target.mmake_name
                );
                assert!(
                    target.link_options.is_empty(),
                    "{cpu}: {}",
                    target.mmake_name
                );
            }

            assert_eq!(module.module_type, ModuleType::Library);
            assert_eq!(module.target_name, "zstd");
            assert_eq!(module.linklib_name.as_deref(), Some("zstd"));
            let genmodule = module.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.enabled && genmodule.has_relative && genmodule.inputs_exact);
            assert_eq!(genmodule.relative_libraries, ["posixc", "stdc"]);
            assert!(genmodule.source_files.is_empty());
            assert!(genmodule.object_sources.is_empty());

            assert_eq!(static_lib.module_type, ModuleType::LinkLib);
            assert_eq!(static_lib.target_name, "zstd-static");
            assert!(static_lib.canonical_linklib_output);
            assert!(parsed.flags.skipped.iter().any(|flag| flag == "-static"));

            let copy = parsed
                .copy_includes
                .iter()
                .find(|copy| copy.name == "workbench-libs-zstd-includes-copy")
                .unwrap();
            assert_eq!(copy.dest, ".");
            assert_eq!(copy.source_dir, "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib");
            assert_eq!(copy.patterns, ["zstd.h", "zstd_errors.h", "zdict.h"]);
            assert!(copy.flatten);

            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "workbench-libs-zstd-fetch")
                .unwrap();
            assert_eq!(fetch.archive, "zstd-1.5.7");
            assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/zstd");
            assert!(fetch.origins.contains("/v1.5.7"));
            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("workbench-libs-zstd-library")
                    && !message.contains("linklibs-zstd")));
        }
    }

    #[test]
    fn atheros_hal_literal_fragment_is_exact_in_all_current_profiles() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/devs/networks/atheros5000/hal/mmakefile.src");
        let expected_sources = [
            "ah",
            "ah_regdomain",
            "ah_eeprom_v3",
            "ah_eeprom_v14",
            "ah_eeprom_v4k",
            "ar5211/ar5211_attach",
            "ar5211/ar5211_beacon",
            "ar5211/ar5211_interrupts",
            "ar5211/ar5211_keycache",
            "ar5211/ar5211_misc",
            "ar5211/ar5211_power",
            "ar5211/ar5211_phy",
            "ar5211/ar5211_recv",
            "ar5211/ar5211_reset",
            "ar5211/ar5211_xmit",
            "ar5212/ar5212_attach",
            "ar5212/ar5212_beacon",
            "ar5212/ar5212_eeprom",
            "ar5212/ar5212_gpio",
            "ar5212/ar5212_interrupts",
            "ar5212/ar5212_keycache",
            "ar5212/ar5212_misc",
            "ar5212/ar5212_power",
            "ar5212/ar5212_phy",
            "ar5212/ar5212_recv",
            "ar5212/ar5212_reset",
            "ar5212/ar5212_xmit",
            "ar5212/ar5212_ani",
            "ar5212/ar5212_rfgain",
            "ar5416/ar5416_ani",
            "ar5416/ar5416_attach",
            "ar5416/ar5416_beacon",
            "ar5416/ar5416_cal",
            "ar5416/ar5416_cal_adcdc",
            "ar5416/ar5416_cal_adcgain",
            "ar5416/ar5416_cal_iq",
            "ar5416/ar5416_eeprom",
            "ar5416/ar5416_gpio",
            "ar5416/ar5416_interrupts",
            "ar5416/ar5416_keycache",
            "ar5416/ar5416_misc",
            "ar5416/ar5416_power",
            "ar5416/ar5416_phy",
            "ar5416/ar5416_recv",
            "ar5416/ar5416_reset",
            "ar5416/ar5416_xmit",
            "ar5416/ar9160_attach",
            "ar5416/ar9280_attach",
            "ar5416/ar9280",
            "ar5416/ar9285_attach",
            "ar5416/ar9285",
            "ar5416/ar9285_reset",
            "ar5212/ar2316",
            "ar5212/ar2317",
            "ar5416/ar2133",
            "ar5212/ar2413",
            "ar5212/ar2425",
            "ar5212/ar5111",
            "ar5212/ar5112",
            "ar5212/ar5413",
        ];
        let expected_definitions = [
            "AH_HAS_RF 1",
            "AH_SUPPORT_AR5211 1",
            "AH_SUPPORT_AR5212 1",
            "AH_SUPPORT_AR5416 1",
            "AH_SUPPORT_2316 1",
            "AH_SUPPORT_2317 1",
            "AH_SUPPORT_2133 1",
            "AH_SUPPORT_2413 1",
            "AH_SUPPORT_2417 1",
            "AH_SUPPORT_2425 1",
            "AH_SUPPORT_5111 1",
            "AH_SUPPORT_5112 1",
            "AH_SUPPORT_5413 1",
            "AH_ENABLE_FORCEBIAS 1",
        ];

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
            let hal = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == "workbench-devs-networks-atheros5000-hal")
                .unwrap_or_else(|| panic!("{cpu}: {:#?}", parsed.skipped_programs));
            assert_eq!(hal.module_type, ModuleType::LinkLib, "{cpu}");
            assert_eq!(hal.target_name, "athhal", "{cpu}");
            assert_eq!(hal.source_files, expected_sources, "{cpu}");
            assert!(hal.cxx_source_files.is_empty(), "{cpu}");
            assert!(hal.objc_source_files.is_empty(), "{cpu}");
            assert!(hal.asm_source_files.is_empty(), "{cpu}");

            assert_eq!(parsed.define_headers.len(), 1, "{cpu}");
            let header = &parsed.define_headers[0];
            assert_eq!(
                header.owner, "workbench-devs-networks-atheros5000-hal-opts",
                "{cpu}"
            );
            assert_eq!(header.provider, hal.mmake_name, "{cpu}");
            assert_eq!(
                header.output, "${AROS_BUILD_DIR}/workbench/devs/networks/atheros5000/hal/opt_ah.h",
                "{cpu}"
            );
            assert_eq!(header.definitions, expected_definitions, "{cpu}");
            assert_eq!(
                header.file, "workbench/devs/networks/atheros5000/hal/Makefile.inc",
                "{cpu}"
            );
            assert_eq!(header.line, 265, "{cpu}");
            assert_eq!(
                header.dependencies,
                [
                    "${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/Makefile.inc",
                    "${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/mmakefile.src",
                ],
                "{cpu}"
            );
            assert!(header.consumers.is_empty(), "{cpu}");
            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed.partial_source_lists.is_empty(), "{cpu}");
            assert!(parsed.generated_file_rules.is_empty(), "{cpu}");
            assert!(parsed.adhoc_header_rules.is_empty(), "{cpu}");
            assert!(
                parsed
                    .skipped_programs
                    .iter()
                    .all(|message| !message.contains(&hal.mmake_name)),
                "{cpu}"
            );
        }
    }

    #[test]
    fn literal_define_header_adoption_rejects_output_traversal() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\n$(OUT):\n\techo \"#define SAFE 1\" >escape.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/../escape.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();

        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
        assert!(!parsed.skipped_local_make_includes.is_empty());
    }

    #[test]
    fn literal_define_capability_is_an_exact_path_provider_and_variable_manifest() {
        let fragment = Path::new("workbench/devs/networks/atheros5000/hal/Makefile.inc");
        let provider = "workbench-devs-networks-atheros5000-hal";
        let owner = "workbench-devs-networks-atheros5000-hal-opts";
        let provider_files = "$(basename $(HAL_OBJS))";
        let output = "${OPT_AH_PATH}";
        let owner_prerequisite = "$(TOP)/$(CURDIR)/opt_ah.h";
        let variables = super::ATHEROS_HAL_LITERAL_DEFINE_VARIABLES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();

        assert!(super::literal_define_fragment_has_capability(
            fragment,
            provider,
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));

        for ambient in [
            "AROS_LIB",
            "Q",
            "ECHO",
            "IF",
            "TEST",
            "NOP",
            "MKDIR",
            "MKDEPEND",
            "TARGET_CC",
            "USER_CFLAGS",
        ] {
            let mut broadened = variables.clone();
            broadened.push(ambient.to_owned());
            broadened.sort();
            assert!(
                !super::literal_define_fragment_has_capability(
                    fragment,
                    provider,
                    owner,
                    provider_files,
                    output,
                    owner_prerequisite,
                    &broadened,
                ),
                "ambient Make capability {ambient} was accepted"
            );
        }
        assert!(!super::literal_define_fragment_has_capability(
            Path::new("elsewhere/Makefile.inc"),
            provider,
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));
        assert!(!super::literal_define_fragment_has_capability(
            fragment,
            "different-provider",
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));
        assert!(!super::literal_define_fragment_has_capability(
            fragment,
            provider,
            owner,
            "$(basename $(OPT_AH_PATH))",
            output,
            owner_prerequisite,
            &variables,
        ));
    }

    #[test]
    fn literal_define_fragment_cannot_change_non_source_build_properties() {
        for escaped_use in [
            "USER_CFLAGS += -DFRAGMENT_MODE=$(MODE)\n",
            "%build_linklib mmake=provider libname=provider files=\"$(FILES)\" libdir=$(MODE)\n",
        ] {
            let tree = TempTree::new();
            fs::create_dir_all(tree.0.join("module")).unwrap();
            fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
            fs::write(
                tree.0.join("module/options.mk"),
                "FILES := one\nMODE := private\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
            )
            .unwrap();
            let declaration = if escaped_use.starts_with("%build_linklib") {
                escaped_use.to_owned()
            } else {
                format!(
                    "{escaped_use}%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n"
                )
            };
            fs::write(
                tree.0.join("module/mmakefile.src"),
                format!(
                    "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n{declaration}#MM\nprovider-opts: $(OUT)\n"
                ),
            )
            .unwrap();

            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &tree.0.join("module/mmakefile.src"),
                &tree.0,
                &DirVars::load(&tree.0),
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(parsed.define_headers.is_empty(), "{escaped_use}");
            assert!(
                parsed
                    .targets
                    .iter()
                    .all(|target| target.mmake_name != "provider"),
                "{escaped_use}"
            );
            assert!(
                !parsed.skipped_local_make_includes.is_empty(),
                "{escaped_use}"
            );
        }

        // Global Make controls are consumed by the reference templates even
        // without a textual reference in this mmakefile. They therefore must
        // not enter the otherwise closed source/header fragment scope.
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\nCFLAGS := -O0\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));

        // Even a variable which genuinely controls a header branch cannot be
        // an ambient template property or one of the provider's implicit
        // private variables. Backward product closure alone is deliberately
        // insufficient for these names.
        for control in [
            "TARGET_CC",
            "TARGET_SYSROOT",
            "TARGET_LTO",
            "SAFETY_CFLAGS",
            "CFLAGS_IQUOTE_END",
            "AR",
            "RANLIB",
            "provider_FILES",
            "provider_OBJDIR",
            "provider_C_FILES",
        ] {
            let tree = TempTree::new();
            fs::create_dir_all(tree.0.join("module")).unwrap();
            fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
            fs::write(
                tree.0.join("module/options.mk"),
                format!(
                    "FILES := one\n{control} := 1\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\nifeq ($({control}),1)\n\techo \"#define SELECTED 1\" >>options.h\nendif\n"
                ),
            )
            .unwrap();
            fs::write(
                tree.0.join("module/mmakefile.src"),
                "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
            )
            .unwrap();
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &tree.0.join("module/mmakefile.src"),
                &tree.0,
                &DirVars::load(&tree.0),
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            assert!(parsed.define_headers.is_empty(), "{control}");
            assert!(
                parsed
                    .targets
                    .iter()
                    .all(|target| target.mmake_name != "provider"),
                "{control}"
            );
        }

        // An innocuous unused assignment is outside both permitted product
        // closures and is rejected without trying to enumerate every way a
        // future Make template might consume it.
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\nUNUSED_FEATURE := 1\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
    }

    #[test]
    fn literal_define_header_rejects_duplicate_active_macro_names() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(tree.0.join("module/one.c"), "int one;\n").unwrap();
        fs::write(
            tree.0.join("module/options.mk"),
            "FILES := one\n$(OUT):\n\techo \"#define SAFE 1\" >options.h\n\techo \"#define SAFE 2\" >>options.h\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "OUT := $(TOP)/$(CURDIR)/options.h\ninclude $(SRCDIR)/$(CURDIR)/options.mk\n%build_linklib mmake=provider libname=provider files=\"$(FILES)\"\n#MM\nprovider-opts: $(OUT)\n",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed.define_headers.is_empty());
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "provider"));
    }

    #[test]
    fn zlib_port_scope_is_declaration_owned_and_profile_exact() {
        let root = root();
        let dirs = dirs();
        let file = root.join("workbench/libs/z/mmakefile.src");
        for (cpu, platform, float_abi, source_count) in [
            ("x86_64", "pc", "", 21),
            ("arm", "raspi", "hard", 15),
            ("aarch64", "raspi", "", 20),
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

            for mmake in [
                "workbench-libs-z",
                "linklibs-z-static",
                "linklibs-z-nogzip-static",
            ] {
                let target = targets.get(mmake).unwrap_or_else(|| {
                    panic!(
                        "{cpu}-{platform}: missing {mmake}: {:#?}",
                        parsed.skipped_programs
                    )
                });
                assert_eq!(target.source_files.len(), source_count, "{cpu}: {mmake}");
                assert!(target.source_files.iter().all(|source| source.starts_with(
                    "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/"
                )));
                assert_eq!(
                    target.include_dirs,
                    ["${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7"],
                    "{cpu}: {mmake}"
                );
                assert_eq!(target.link_options, ["-lpthread"], "{cpu}: {mmake}");
            }

            let module = targets["workbench-libs-z"];
            assert_eq!(module.linklib_name.as_deref(), Some("z"));
            let genmodule = module.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.enabled && genmodule.has_relative);
            assert!(genmodule.inputs_exact);
            assert_eq!(genmodule.relative_libraries, ["posixc", "stdc"]);
            assert!(genmodule.source_files.is_empty());
            assert!(genmodule.object_sources.is_empty());
            for define in ["_XOPEN_SOURCE=600", "STDC", "AMIGA"] {
                assert!(
                    module.defines.iter().any(|value| value == define),
                    "{cpu}: {define}"
                );
            }
            assert!(!module
                .defines
                .iter()
                .any(|value| { matches!(value.as_str(), "NO_STRERROR" | "NDEBUG" | "NO_GZIP") }));

            let static_lib = targets["linklibs-z-static"];
            assert!(static_lib
                .defines
                .iter()
                .any(|value| value == "NO_STRERROR"));
            assert!(static_lib.defines.iter().any(|value| value == "NDEBUG"));
            assert!(!static_lib.defines.iter().any(|value| value == "NO_GZIP"));

            let no_gzip = targets["linklibs-z-nogzip-static"];
            assert!(no_gzip.defines.iter().any(|value| value == "NO_GZIP"));
            assert!(static_lib.canonical_linklib_output, "{cpu}: z.static");
            assert!(no_gzip.canonical_linklib_output, "{cpu}: z-nogzip.static");
            assert!(!module.canonical_linklib_output);

            let minigzip = targets["workbench-libs-z-minigzip"];
            assert_eq!(
                minigzip.source_files,
                ["${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/test/minigzip"]
            );
            assert!(minigzip.defines.iter().any(|value| value == "NO_GZIP"));
            assert_eq!(minigzip.link_options, ["-lpthread"]);
            assert!(!minigzip.canonical_linklib_output);

            assert_eq!(parsed.header_transforms.len(), 1, "{cpu}: transforms");
            let fetch = parsed
                .fetches
                .iter()
                .find(|fetch| fetch.name == "zlib-fetch")
                .expect("production zlib fetch");
            assert_eq!(
                fetch.base,
                "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7"
            );
            assert_eq!(fetch.destination, fetch.base);
            assert!(fetch.location.contains("chromium-da752eb2a3660cf1"));
            assert!(!fetch.origins.contains("cache://"));
            assert!(fetch
                .origins
                .contains("da752eb2a3660cf1bf8dac620f6380b89dd953a7"));
            let transform = &parsed.header_transforms[0];
            assert_eq!(transform.name, "workbench-libs-z-geninc");
            assert_eq!(
                transform.input,
                "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7/zconf.h.chr"
            );
            assert_eq!(transform.output, "${AROS_SDK_INCLUDE_DIR}/zconf.h");
            assert!(parsed
                .adhoc_header_rules
                .iter()
                .all(|rule| rule.dest != "zconf.h"));

            let x86_define = module
                .defines
                .iter()
                .any(|value| value == "INFLATE_CHUNK_SIMD_SSE2");
            let arm64_define = module
                .defines
                .iter()
                .any(|value| value == "INFLATE_CHUNK_SIMD_NEON");
            assert_eq!(x86_define, cpu == "x86_64", "{cpu}: x86 flags");
            assert_eq!(arm64_define, cpu == "aarch64", "{cpu}: arm64 flags");
            assert_eq!(
                module.compile_options,
                if cpu == "aarch64" {
                    vec!["-march=armv8-a+crc+crypto".to_owned()]
                } else {
                    Vec::new()
                },
                "{cpu}: compile options"
            );

            assert!(parsed.skipped_local_make_includes.is_empty(), "{cpu}");
            assert!(parsed
                .skipped_programs
                .iter()
                .all(|message| !message.contains("workbench-libs-z")
                    && !message.contains("linklibs-z")));
        }
    }

    #[test]
    fn relative_zlib_dependencies_have_exact_full_module_archive_inputs() {
        let root = root();
        let dirs = dirs();
        for (relative, mmake, source_count, object_count) in [
            ("compiler/crt/posixc/mmakefile.src", "compiler-posixc", 8, 1),
            ("compiler/crt/stdc/mmakefile.src", "compiler-stdc", 9, 13),
        ] {
            let parsed = super::parse_mmakefile_with_dirs_and_context(
                &root.join(relative),
                &root,
                &dirs,
                &target_context("x86_64", "pc", ""),
            )
            .unwrap();
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap();
            let genmodule = target.genmodule_linklibs.as_ref().unwrap();
            assert!(genmodule.has_relative, "{mmake}");
            assert!(
                genmodule.inputs_exact,
                "{mmake}: {:#?}",
                parsed.partial_source_lists
            );
            assert_eq!(genmodule.source_files.len(), source_count, "{mmake}");
            assert_eq!(genmodule.object_sources.len(), object_count, "{mmake}");
        }
    }

    #[test]
    fn broad_safe_fragment_without_a_fetch_owner_remains_deferred() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(
            tree.0.join("module/make.opt"),
            "ARCHSRCDIR := $(PORTSDIR)/unowned/src\nUSER_INCLUDES += -I$(ARCHSRCDIR)\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "include $(SRCDIR)/$(CURDIR)/make.opt\nFILES := one two\n%build_linklib mmake=unowned libname=unowned files=\"$(addprefix $(ARCHSRCDIR)/,$(FILES))\"\n",
        )
        .unwrap();

        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        assert!(parsed
            .targets
            .iter()
            .all(|target| target.mmake_name != "unowned"));
        assert!(parsed
            .skipped_local_make_includes
            .iter()
            .any(|message| message.contains("broader than one plain source-list")));
    }

    #[test]
    fn canonical_linklib_output_requires_target_owned_port_sources() {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("module")).unwrap();
        fs::write(
            tree.0.join("module/mmakefile.src"),
            "\
%fetch mmake=owned-fetch archive=owned destination=$(PORTSDIR)/owned
%build_linklib mmake=owned libname=owned files=$(PORTSDIR)/owned/x
%build_linklib mmake=owned-target libname=owned-target files=$(PORTSDIR)/owned/x compiler=target
%build_linklib mmake=owned-host libname=owned-host files=$(PORTSDIR)/owned/x compiler=host
%build_linklib mmake=owned-libdir libname=owned-libdir files=$(PORTSDIR)/owned/x libdir=$(GENDIR)/lib
%build_linklib mmake=owned-32 libname=owned-32 files=$(PORTSDIR)/owned/x objdir=$(GENDIR)/module/32bit
%build_linklib mmake=foreign libname=foreign files=$(PORTSDIR)/foreign/x
",
        )
        .unwrap();
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &tree.0.join("module/mmakefile.src"),
            &tree.0,
            &DirVars::load(&tree.0),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        let targets: BTreeMap<_, _> = parsed
            .targets
            .iter()
            .map(|target| (target.mmake_name.as_str(), target))
            .collect();
        assert!(targets["owned"].canonical_linklib_output);
        assert!(targets["owned-target"].canonical_linklib_output);
        for mmake in ["owned-host", "owned-libdir", "owned-32", "foreign"] {
            assert!(!targets[mmake].canonical_linklib_output, "{mmake}");
        }

        let zopfli = super::parse_mmakefile_with_dirs_and_context(
            &root().join("tools/zopfli/mmakefile.src"),
            &root(),
            &dirs(),
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();
        for target in zopfli.targets.iter().filter(|target| {
            matches!(
                target.mmake_name.as_str(),
                "linklibs-zopfli" | "host-linklibs-zopfli"
            )
        }) {
            assert!(!target.canonical_linklib_output, "{}", target.mmake_name);
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
    fn every_library_module_materialises_its_client_archive() {
        let tree = TempTree::new();
        let module = tree.0.join("rom/thing");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("thing.c"), "").unwrap();
        fs::write(
            module.join("thing.conf"),
            "##begin config\n\
             basename Thing\n\
             libbasetype struct ThingBase\n\
             ##end config\n",
        )
        .unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%build_module mmake=kernel-thing modname=thing modtype=library files=thing\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        // No linklibname=, no linklibfiles=: upstream still archives
        // thing_getlibbase and thing_autoinit into libthing.a, because the
        // module type alone puts them into <mod>_LINKLIBFILES.
        let genmodule = parsed.targets[0]
            .genmodule_linklibs
            .as_ref()
            .expect("library client-archive metadata");
        assert!(genmodule.enabled);
        assert!(genmodule.source_files.is_empty());
        assert!(parsed.skipped_client_archives.is_empty(), "{parsed:#?}");
    }

    #[test]
    fn non_library_module_needing_a_client_archive_is_reported() {
        let tree = TempTree::new();
        let module = tree.0.join("rom/clock");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("clock.c"), "").unwrap();
        fs::write(
            module.join("clock.conf"),
            "##begin config\n\
             basename Clock\n\
             options autoinit\n\
             ##end config\n",
        )
        .unwrap();
        let file = module.join("mmakefile.src");
        fs::write(
            &file,
            "%build_module mmake=kernel-clock modname=clock modtype=device files=clock\n",
        )
        .unwrap();
        let dirs = DirVars::load(&tree.0);
        let parsed = super::parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &dirs,
            &target_context("x86_64", "pc", ""),
        )
        .unwrap();

        assert!(parsed.targets[0].genmodule_linklibs.is_none());
        assert_eq!(parsed.skipped_client_archives.len(), 1, "{parsed:#?}");
        assert!(parsed.skipped_client_archives[0].contains("libclock.a"));
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
    fn sourceful_module_forms_keep_their_noncyclic_implicit_metamake_graph() {
        let root = root();
        let dirs = dirs();
        for (file, mmake, modname, has_abi) in [
            (
                "compiler/crt/stdc/mmakefile.src",
                "compiler-stdc",
                "stdc",
                true,
            ),
            (
                "rom/usb/classes/serialpl2303/mmakefile.src",
                "kernel-usb-classes-serialpl2303",
                "serialpl2303",
                false,
            ),
        ] {
            let parsed = super::parse_mmakefile_with_dirs(&root.join(file), &root, &dirs)
                .unwrap_or_else(|error| panic!("{file}: {error}"));
            let mut metas: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for rule in &parsed.meta_rules {
                metas
                    .entry(&rule.name)
                    .or_default()
                    .extend(rule.dependencies.iter().map(String::as_str));
            }

            let quick = format!("{mmake}-quick");
            let kobj = format!("{mmake}-kobj");
            assert!(metas[quick.as_str()].contains(mmake), "{file}");
            assert!(metas[kobj.as_str()].contains("core-linklibs"), "{file}");
            // MetaMake's virtual architecture chain returns to the concrete
            // sourceful producer, which MetaMake breaks through pre-marked
            // traversal. CMake rejects that cycle, so only ABI/genmodule-only
            // forms emit it in the translated graph.
            let arch_cpu = format!("{mmake}-${{AROS_TARGET_CPU}}");
            assert!(!metas[kobj.as_str()].contains(arch_cpu.as_str()), "{file}");

            let includes_alias = format!("includes-{modname}");
            if has_abi {
                let includes = format!("{mmake}-includes");
                assert!(
                    metas[includes_alias.as_str()].contains(includes.as_str()),
                    "{file}"
                );
            } else {
                assert!(!metas.contains_key(includes_alias.as_str()), "{file}");
            }
        }
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
