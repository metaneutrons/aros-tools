//! Strict GNU Make expression subset used by `%copy_includes`.

use super::ExternalVarLookup;
use std::collections::HashMap;

pub(super) fn split_assignment(line: &str) -> Option<(&str, &str, bool)> {
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

pub(super) fn collect_local_assignments(lines: &[&str], vars: &mut HashMap<String, String>) {
    let mut pending: Option<String> = None;
    for line in lines {
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
        if line.starts_with('\t') || trimmed.starts_with('#') || trimmed.starts_with('%') {
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
        if append && !entry.is_empty() && !rhs.trim().is_empty() {
            entry.push(' ');
        } else if !append {
            entry.clear();
        }
        entry.push_str(rhs.trim());
        if continues {
            pending = Some(name);
        }
    }
}

/// Make variables that map onto a CMake location.
///
/// `%fetch` unpacks third-party sources under the ports directory, and a module
/// then stages their headers from there: `arch/all-native/acpica` publishes
/// `$(ACPICA_INCLUDES)/*.h` as `acpica/*.h`. Without this mapping the
/// declaration cannot be resolved and `<acpica/actypes.h>` never reaches the
/// SDK, which is what the ACPI-dependent drivers fail on.
pub(super) fn map_var(name: &str) -> Option<&'static str> {
    match name {
        "SRCDIR" => Some("${CMAKE_SOURCE_DIR}"),
        "TOP" => Some("${AROS_BUILD_DIR}"),
        "PORTSDIR" => Some("${AROS_PORTS_DIR}"),
        "PORTSSOURCEDIR" => Some("${AROS_PORTS_SOURCE_DIR}"),
        // Same mapping as includes.rs: the generated per-module tree lives
        // under <build>/gen, not at the build root.
        "GENDIR" => Some("${CMAKE_BINARY_DIR}/gen"),
        "GENINCDIR" => Some("${CMAKE_BINARY_DIR}/GENINCDIR"),
        _ => None,
    }
}

/// Substitutes `$(VAR)` references from `vars`, leaving unknown ones in place.
pub(super) fn substitute(
    raw: &str,
    vars: &HashMap<String, String>,
    external: ExternalVarLookup<'_>,
    depth: usize,
) -> String {
    if depth == 0 || !raw.contains("$(") {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let Some(end) = matching_paren(rest, start + 1) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &rest[start + 2..end];
        // `$(call WILDCARD, ...)` is handled later, keep it verbatim.
        if name.starts_with("call ") || name.contains(' ') {
            out.push_str(&rest[start..=end]);
        } else if let Some(m) = map_var(name) {
            out.push_str(m);
        } else if let Some(v) = vars
            .get(name)
            .cloned()
            .or_else(|| external.and_then(|f| f(name)))
        {
            out.push_str(&substitute(&v, vars, external, depth - 1));
        } else {
            out.push_str(&rest[start..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Finds the `)` matching the `$(` that starts at `open`, counting nesting.
pub(super) fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            depth += 1;
        } else if bytes[i] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Expands a bracket character class, e.g. `*.[hi]` into `*.h` and `*.i`.
///
/// CMake's `file(GLOB)` does not understand character classes, so they are
/// turned into separate patterns here.
pub(super) fn expand_char_classes(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('[') else {
        return vec![pattern.to_owned()];
    };
    let Some(close) = pattern[open..].find(']').map(|i| i + open) else {
        return vec![pattern.to_owned()];
    };
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let mut out = Vec::new();
    for ch in pattern[open + 1..close].chars() {
        for tail in expand_char_classes(suffix) {
            out.push(format!("{prefix}{ch}{tail}"));
        }
    }
    if out.is_empty() {
        vec![pattern.to_owned()]
    } else {
        out
    }
}

/// Extracts the file list from an `includes=` value.
///
/// Returns `None` when the list still references a Make variable we cannot
/// resolve, which is the case for the third-party ports whose sources live
/// outside the tree.
#[derive(Debug, Default)]
pub(super) struct ResolvedFileList {
    pub(super) patterns: Vec<String>,
    pub(super) excludes: Vec<String>,
}

pub(super) fn outer_make_function<'a>(expression: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("$({name}");
    let remainder = expression.strip_prefix(&prefix)?;
    if !remainder.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let end = matching_paren(expression, 1)?;
    if end + 1 != expression.len() {
        return None;
    }
    Some(expression[prefix.len()..end].trim())
}

pub(super) fn split_top_level_comma(expression: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (index, ch) in expression.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => return Some((&expression[..index], &expression[index + 1..])),
            _ => {}
        }
    }
    None
}

pub(super) fn split_top_level_arguments(expression: &str) -> Option<Vec<&str>> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut out = Vec::new();
    for (index, ch) in expression.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                out.push(expression[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    out.push(expression[start..].trim());
    Some(out)
}

/// Splits a Make word list without cutting whitespace inside `$(...)`.
pub(super) fn split_make_words(expression: &str) -> Option<Vec<&str>> {
    let bytes = expression.as_bytes();
    let mut depth = 0usize;
    let mut start = None;
    let mut words = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            if start.is_none() {
                start = Some(cursor);
            }
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && depth > 0 {
            depth -= 1;
        } else if bytes[cursor].is_ascii_whitespace() && depth == 0 {
            if let Some(word_start) = start.take() {
                words.push(&expression[word_start..cursor]);
            }
            cursor += 1;
            continue;
        } else if start.is_none() {
            start = Some(cursor);
        }
        cursor += 1;
    }
    if depth != 0 {
        return None;
    }
    if let Some(word_start) = start {
        words.push(&expression[word_start..]);
    }
    Some(words)
}

pub(super) fn apply_percent_substitution(word: &str, pattern: &str, replacement: &str) -> String {
    let Some((prefix, suffix)) = pattern.split_once('%') else {
        return if word == pattern {
            replacement.to_owned()
        } else {
            word.to_owned()
        };
    };
    let Some(stem) = word
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return word.to_owned();
    };
    replacement.replace('%', stem)
}

pub(super) fn resolve_file_list(
    raw: &str,
    vars: &HashMap<String, String>,
    external: ExternalVarLookup<'_>,
) -> Option<ResolvedFileList> {
    let substituted = substitute(raw, vars, external, 8);
    resolve_file_list_expression(substituted.trim(), vars, external)
}

pub(super) fn resolve_file_list_expression(
    expression: &str,
    vars: &HashMap<String, String>,
    external: ExternalVarLookup<'_>,
) -> Option<ResolvedFileList> {
    for function in ["addprefix", "addsuffix"] {
        if let Some(arguments) = outer_make_function(expression, function) {
            let (affix, words) = split_top_level_comma(arguments)?;
            let affix = substitute(affix.trim(), vars, external, 8);
            if affix.contains("$(") || affix.contains(char::is_whitespace) {
                return None;
            }
            let mut resolved = resolve_file_list(words.trim(), vars, external)?;
            for list in [&mut resolved.patterns, &mut resolved.excludes] {
                for word in list {
                    *word = if function == "addprefix" {
                        format!("{affix}{word}")
                    } else {
                        format!("{word}{affix}")
                    };
                }
            }
            return Some(resolved);
        }
    }

    if let Some(arguments) = outer_make_function(expression, "filter") {
        let (raw_filters, raw_words) = split_top_level_comma(arguments)?;
        let filters = resolve_file_list(raw_filters.trim(), vars, external)?;
        let mut words = resolve_file_list(raw_words.trim(), vars, external)?;
        let matches = |word: &str| {
            filters.patterns.iter().any(|filter| {
                if let Some((prefix, suffix)) = filter.split_once('%') {
                    word.strip_prefix(prefix)
                        .and_then(|rest| rest.strip_suffix(suffix))
                        .is_some()
                } else {
                    word == filter
                }
            })
        };
        words.patterns.retain(|word| matches(word));
        words.excludes.retain(|word| matches(word));
        return Some(words);
    }

    if let Some(arguments) = outer_make_function(expression, "sort") {
        let mut resolved = resolve_file_list(arguments, vars, external)?;
        resolved.patterns.sort();
        resolved.patterns.dedup();
        resolved.excludes.sort();
        resolved.excludes.dedup();
        return Some(resolved);
    }

    // GNU Make's lowercase wildcard form is equivalent here to the AROS
    // WILDCARD wrapper: CMake receives the pattern and evaluates it against the
    // source directory. Keeping it declarative also works before a Port fetch.
    if let Some(arguments) = outer_make_function(expression, "wildcard") {
        let expanded = substitute(arguments, vars, external, 8);
        if expanded.contains("$(") {
            return None;
        }
        let mut resolved = ResolvedFileList::default();
        for pattern in expanded.split_whitespace() {
            resolved.patterns.extend(expand_char_classes(pattern));
        }
        return (!resolved.patterns.is_empty()).then_some(resolved);
    }

    if let Some(arguments) = outer_make_function(expression, "patsubst") {
        let arguments = split_top_level_arguments(arguments)?;
        if arguments.len() != 3 {
            return None;
        }
        let pattern = substitute(arguments[0], vars, external, 8);
        let replacement = substitute(arguments[1], vars, external, 8);
        if pattern.contains("$(") || replacement.contains("$(") {
            return None;
        }
        let mut resolved = resolve_file_list(arguments[2], vars, external)?;
        for list in [&mut resolved.patterns, &mut resolved.excludes] {
            for word in list.iter_mut() {
                *word = apply_percent_substitution(word, &pattern, &replacement);
            }
        }
        return Some(resolved);
    }

    if let Some(arguments) = outer_make_function(expression, "notdir") {
        let mut resolved = resolve_file_list(arguments, vars, external)?;
        for list in [&mut resolved.patterns, &mut resolved.excludes] {
            for entry in list.iter_mut() {
                *entry = entry.rsplit('/').next().unwrap_or(entry).to_owned();
            }
        }
        return Some(resolved);
    }

    if let Some(arguments) = outer_make_function(expression, "filter-out") {
        let (filters, words) = split_top_level_comma(arguments)?;
        let filters = resolve_file_list(filters.trim(), vars, external)?;
        // Pattern filters would need to be evaluated against the fetched
        // directory at build time.  Keep this deliberately bounded to the
        // literal exclusion form that the header publisher uses; anything
        // broader remains an explicit skipped declaration rather than a guess.
        if filters
            .patterns
            .iter()
            .chain(filters.excludes.iter())
            .any(|entry| entry.contains(['*', '?', '[', '%']))
        {
            return None;
        }
        let mut resolved = resolve_file_list(words.trim(), vars, external)?;
        resolved.excludes.extend(filters.patterns);
        resolved.excludes.extend(filters.excludes);
        resolved.excludes.sort();
        resolved.excludes.dedup();
        return Some(resolved);
    }

    let mut resolved = ResolvedFileList::default();
    let mut rest = expression;

    // Pull out every `$(call WILDCARD, <globs>)` group. The closing paren must
    // be matched with nesting in mind: `$(call WILDCARD, $(X)/*.h)` would
    // otherwise be cut short at the inner `)`.
    while let Some(start) = rest.find("$(call") {
        let head = &rest[..start];
        push_plain_tokens(head, &mut resolved.patterns)?;
        // `matching_paren` expects to start on the `(`.
        let end = matching_paren(rest, start + 1)?;
        let inner = &rest[start..end];
        let globs = inner.split_once(',').map_or("", |(_, g)| g);
        for g in globs.split_whitespace() {
            // The glob may itself be rooted in a variable, as in
            // `$(call WILDCARD, $(ACPICA_INCLUDES)/*.h)`. Substitute first; the
            // directory part is dropped later by the `dir=` flattening, so only
            // the trailing pattern matters.
            let g = substitute(g, vars, external, 8);
            // A Make construct that survived substitution means we cannot know
            // what this matches; skip the declaration rather than guess.
            if g.contains("$(") {
                return None;
            }
            resolved.patterns.extend(expand_char_classes(&g));
        }
        rest = &rest[end + 1..];
    }
    push_plain_tokens(rest, &mut resolved.patterns)?;

    if resolved.patterns.is_empty() {
        return None;
    }
    Some(resolved)
}

/// Adds literal file names, rejecting anything still holding a Make variable.
pub(super) fn push_plain_tokens(text: &str, out: &mut Vec<String>) -> Option<()> {
    for tok in text.split_whitespace() {
        if tok.contains("$(") {
            return None;
        }
        out.extend(expand_char_classes(tok));
    }
    Some(())
}
