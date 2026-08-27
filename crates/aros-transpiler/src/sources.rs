//! Source lists, as a declaration's `files=` argument evaluates to.
//!
//! A file list is nearly always written one name per continued line and often
//! as a Make variable or a function call over one, so getting from the argument
//! text to a list of file names is its own job. Four lanes come out of it, not
//! one: C, C++, Objective-C and assembler are kept apart because the legacy
//! macros keep them apart, and a single compile rule over the union would be
//! wrong.
//!
//! An expression this cannot evaluate is reported rather than guessed at. That
//! is the difference between a target that is missing a file and a target that
//! silently builds a different set than Make would.

use crate::make_expr::{evaluate_make_list, MakeExprContext, MakeExprError};
use crate::parser::macro_arg;
// `resolve_name` and `macro_sources` are `#[cfg(test)]`: they were the earlier
// way of reading a declaration's name and file list, kept because their tests
// still state what that reading has to do. Their only caller is those tests, so
// what they need is a test-only import.
#[cfg(test)]
use crate::parser::sanitize_ident;
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn expand_file_list(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
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
pub(crate) fn resolve_name(raw: &str, vars: &HashMap<String, Vec<String>>) -> Option<String> {
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
pub(crate) fn macro_sources(
    args: &str,
    vars: &HashMap<String, Vec<String>>,
) -> (Vec<String>, bool) {
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
pub(crate) struct EvaluatedSources {
    pub(crate) c: Vec<String>,
    pub(crate) cxx: Vec<String>,
    pub(crate) objc: Vec<String>,
    pub(crate) asm: Vec<String>,
    pub(crate) declared: bool,
    pub(crate) diagnostics: Vec<String>,
    /// Wildcards rooted in a fetched build tree whose source inventory is not
    /// present during this configure pass. The generated graph uses their
    /// owning fetches to force one ordered reconfigure.
    pub(crate) deferred_wildcards: Vec<String>,
}

impl EvaluatedSources {
    pub(crate) const fn is_empty(&self) -> bool {
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
pub(crate) fn evaluate_macro_sources(
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
pub(crate) fn evaluate_macro_sources_with_files(
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
                    if let MakeExprError::DeferredWildcard { pattern } = &error {
                        if !sources.deferred_wildcards.contains(pattern) {
                            sources.deferred_wildcards.push(pattern.clone());
                        }
                    }
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

pub(crate) fn evaluate_linklib_list(
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
pub(crate) fn map_linklib_object_sources(
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
