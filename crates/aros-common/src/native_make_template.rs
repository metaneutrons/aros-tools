//! Explicit, source-sealed projections of configure-owned Make templates.
//!
//! This module never reads generated build-tree files and never runs
//! Autoconf. A caller must name the generated path, its `.in` template, the
//! configure source that declares the exact `output:template` pair, the
//! substitution values, and hashes for both source files.

use crate::{ArosError, Result, Sha256Digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_BINDINGS: usize = 16;
const MAX_CONFIGURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_SUBSTITUTIONS: usize = 32;
const MAX_SUBSTITUTION_BYTES: usize = 128;
const MAX_ASSIGNMENTS: usize = 64;

/// One explicit source-owned binding from a generated Make include to its
/// configure template and literal substitutions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedMakeTemplateBinding {
    /// Source-relative `.in` template selected by the generated path key.
    /// The template must be present in `sealed_inputs` and is read without
    /// following symlinks.
    pub template: String,
    /// Source-relative configure input containing the exact output/template
    /// pair. The file must be present in `sealed_inputs`.
    pub configure_source: String,
    /// Exact `@NAME@` token-to-scalar mapping. Values are not recursively
    /// expanded and cannot contain Make, shell, quote, or control syntax.
    #[serde(default, deserialize_with = "deserialize_substitutions")]
    pub substitutions: BTreeMap<String, String>,
}

/// One safely expanded template. The map key returned by
/// [`resolve_generated_make_templates`] is its generated path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGeneratedMakeTemplate {
    pub template_relative: String,
    pub expanded_text: String,
    pub substitutions: BTreeMap<String, String>,
}

fn deserialize_unique_map<'de, D, V>(
    deserializer: D,
    limit: usize,
) -> std::result::Result<BTreeMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMap<V>(usize, std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for UniqueMap<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded object with unique string keys")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                }
                if values.len() > self.0 {
                    return Err(serde::de::Error::custom(
                        "template map exceeds its entry limit",
                    ));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(UniqueMap(limit, std::marker::PhantomData))
}

fn deserialize_substitutions<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_unique_map(deserializer, MAX_SUBSTITUTIONS)
}

/// Contract deserializer which rejects duplicate generated paths.
///
/// # Errors
/// Rejects duplicate keys, null/non-object values and excessive entries.
pub fn deserialize_bindings<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, GeneratedMakeTemplateBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_unique_map(deserializer, MAX_BINDINGS)
}

/// Validate explicit bindings, verify their source seals and configure
/// ownership, and expand their bounded safe-assignment templates.
///
/// `sealed_inputs` maps source-relative paths to the SHA-256 values already
/// recorded by the source contract. Every referenced template and configure
/// file must appear there and match its digest. Generated paths are never
/// opened.
///
/// # Errors
/// Returns a configuration error for unsafe paths, absent/stale seals,
/// missing or ambiguous configure declarations, unsafe substitutions, or
/// template text outside the plain-assignment subset.
pub fn resolve_generated_make_templates(
    root: &Path,
    bindings: &BTreeMap<String, GeneratedMakeTemplateBinding>,
    sealed_inputs: &BTreeMap<String, Sha256Digest>,
) -> Result<BTreeMap<String, ResolvedGeneratedMakeTemplate>> {
    if bindings.len() > MAX_BINDINGS {
        return Err(invalid(format!(
            "generated Make template bindings exceed {MAX_BINDINGS} entries"
        )));
    }

    let root = root
        .canonicalize()
        .map_err(|error| invalid(format!("cannot canonicalize source root: {error}")))?;
    if !root.is_dir() {
        return Err(invalid("source root is not a directory"));
    }

    validate_sealed_input_paths(sealed_inputs)?;

    let mut all_paths = BTreeMap::<String, String>::new();
    let mut configure_snapshots = BTreeMap::<String, String>::new();
    let mut output_paths = BTreeSet::<String>::new();
    let mut resolved = BTreeMap::new();

    for (generated_path, binding) in bindings {
        validate_relative_path(generated_path)
            .map_err(|reason| invalid(format!("generated path {generated_path:?} {reason}")))?;
        validate_relative_path(&binding.template)
            .map_err(|reason| invalid(format!("template path {:?} {reason}", binding.template)))?;
        validate_relative_path(&binding.configure_source).map_err(|reason| {
            invalid(format!(
                "configure source {:?} {reason}",
                binding.configure_source
            ))
        })?;

        if Path::new(&binding.template)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            != Some("in")
        {
            return Err(invalid(format!(
                "template {:?} must have the exact .in extension",
                binding.template
            )));
        }

        insert_path_identity(&mut all_paths, generated_path, "generated output")?;
        insert_path_identity(&mut all_paths, &binding.template, "template")?;
        insert_path_identity(
            &mut all_paths,
            &binding.configure_source,
            "configure source",
        )?;

        let output_key = folded_path(generated_path);
        if output_paths
            .iter()
            .any(|previous| paths_overlap(&output_key, previous))
        {
            return Err(invalid(format!(
                "generated output {generated_path:?} duplicates, case-fold-collides, or prefix-overlaps another output"
            )));
        }
        output_paths.insert(output_key);

        let template_bytes =
            read_sealed_source(&root, &binding.template, sealed_inputs, MAX_TEMPLATE_BYTES)?;
        let template_text = String::from_utf8(template_bytes).map_err(|error| {
            invalid(format!(
                "template {:?} is not UTF-8: {error}",
                binding.template
            ))
        })?;

        if !configure_snapshots.contains_key(&binding.configure_source) {
            let bytes = read_sealed_source(
                &root,
                &binding.configure_source,
                sealed_inputs,
                MAX_CONFIGURE_BYTES,
            )?;
            let text = String::from_utf8(bytes).map_err(|error| {
                invalid(format!(
                    "configure source {:?} is not UTF-8: {error}",
                    binding.configure_source
                ))
            })?;
            configure_snapshots.insert(binding.configure_source.clone(), text);
        }
        let configure_text = configure_snapshots
            .get(&binding.configure_source)
            .ok_or_else(|| invalid("configure source snapshot is unexpectedly absent"))?;

        prove_configure_pair(configure_text, generated_path, &binding.template)?;
        let expanded_text = expand_template(&template_text, &binding.substitutions)?;
        validate_plain_make_assignments(generated_path, &expanded_text)?;

        if resolved
            .insert(
                generated_path.clone(),
                ResolvedGeneratedMakeTemplate {
                    template_relative: binding.template.clone(),
                    expanded_text,
                    substitutions: binding.substitutions.clone(),
                },
            )
            .is_some()
        {
            return Err(invalid(format!(
                "duplicate generated output path {generated_path:?}"
            )));
        }
    }

    // Outputs may not shadow any sealed source input, even if the generated
    // file does not currently exist. Otherwise a later build could overwrite
    // the very template/configure bytes used to prove its provenance.
    for generated_path in resolved.keys() {
        let generated_key = folded_path(generated_path);
        if sealed_inputs.keys().any(|input| {
            let input_key = folded_path(input);
            paths_overlap(&generated_key, &input_key)
        }) {
            return Err(invalid(format!(
                "generated output {generated_path:?} overlaps a sealed source input path"
            )));
        }
    }

    Ok(resolved)
}

fn validate_sealed_input_paths(sealed_inputs: &BTreeMap<String, Sha256Digest>) -> Result<()> {
    let mut seen = BTreeMap::<String, String>::new();
    for path in sealed_inputs.keys() {
        validate_relative_path(path)
            .map_err(|reason| invalid(format!("sealed input path {path:?} {reason}")))?;
        insert_path_identity(&mut seen, path, "sealed input")?;
    }
    Ok(())
}

fn insert_path_identity(seen: &mut BTreeMap<String, String>, path: &str, role: &str) -> Result<()> {
    let key = folded_path(path);
    if let Some(previous) = seen.get(&key) {
        if previous != path {
            return Err(invalid(format!(
                "{role} path {path:?} case-fold-collides with {previous:?}"
            )));
        }
    } else {
        seen.insert(key, path.to_owned());
    }
    Ok(())
}

fn read_sealed_source(
    root: &Path,
    relative: &str,
    sealed_inputs: &BTreeMap<String, Sha256Digest>,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let expected = sealed_inputs.get(relative).ok_or_else(|| {
        invalid(format!(
            "source input {relative:?} is not present in the sealed input map"
        ))
    })?;
    let relative_path = Path::new(relative);
    let mut candidate = root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(name) = component else {
            return Err(invalid(format!(
                "source input {relative:?} is not a canonical relative path"
            )));
        };
        candidate.push(name);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            invalid(format!("cannot inspect source input {relative:?}: {error}"))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "source input {relative:?} crosses a symlink"
            )));
        }
        if candidate != root.join(relative_path) && !metadata.is_dir() {
            return Err(invalid(format!(
                "source input {relative:?} has a non-directory parent"
            )));
        }
    }

    crate::canonical_source_file(root, relative_path).map_err(|error| {
        invalid(format!(
            "source input {relative:?} is not a regular file below the source root: {error}"
        ))
    })?;
    let file = crate::open_regular_file_nofollow(&candidate).map_err(|error| {
        invalid(format!(
            "cannot open source input {relative:?} without following links: {error}"
        ))
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| invalid(format!("cannot inspect source input {relative:?}: {error}")))?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(invalid(format!(
            "source input {relative:?} is not a bounded regular file"
        )));
    }

    let limit = max_bytes
        .checked_add(1)
        .ok_or_else(|| invalid("source input byte limit overflow"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("cannot read source input {relative:?}: {error}")))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(invalid(format!(
            "source input {relative:?} exceeds its byte limit"
        )));
    }
    if crate::sha256_bytes(&bytes) != *expected {
        return Err(invalid(format!(
            "source input {relative:?} differs from its sealed SHA-256"
        )));
    }
    Ok(bytes)
}

fn prove_configure_pair(configure: &str, generated: &str, template: &str) -> Result<()> {
    let calls = active_config_files_arguments(configure)?;
    let expected = format!("{generated}:{template}");
    let folded_generated = folded_path(generated);
    let mut exact_count = 0_usize;

    for argument in calls {
        for entry in literal_entries(argument)? {
            if entry == expected {
                exact_count += 1;
                continue;
            }
            if entry == generated {
                return Err(invalid(format!(
                    "AC_CONFIG_FILES declares {generated:?} without an explicit template"
                )));
            }
            if let Some((output, other_template)) = entry.split_once(':') {
                if folded_path(output) == folded_generated {
                    return Err(invalid(format!(
                        "AC_CONFIG_FILES maps {generated:?} to {other_template:?}, not {template:?}"
                    )));
                }
            } else if folded_path(&entry) == folded_generated {
                return Err(invalid(format!(
                    "AC_CONFIG_FILES declares case-mismatched output {entry:?}"
                )));
            }
        }
    }

    if exact_count != 1 {
        return Err(invalid(format!(
            "expected exactly one active literal AC_CONFIG_FILES entry {expected:?}, found {exact_count}"
        )));
    }
    Ok(())
}

/// Return the first macro argument for every unquoted, uncommented literal
/// AC_CONFIG_FILES invocation. Unrelated macro bodies and M4-quoted text are
/// skipped; malformed invocations fail closed.
fn active_config_files_arguments(source: &str) -> Result<Vec<&str>> {
    let bytes = source.as_bytes();
    let mut arguments = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        if let Some(end) = line_comment_end(bytes, cursor) {
            cursor = end;
            continue;
        }
        if bytes[cursor] == b'[' {
            cursor = m4_quote_end(bytes, cursor)
                .ok_or_else(|| invalid("configure source contains an unterminated M4 quote"))?;
            continue;
        }
        if token_at(bytes, cursor, b"AC_CONFIG_FILES") && line_prefix_is_space(bytes, cursor) {
            let name_end = cursor + b"AC_CONFIG_FILES".len();
            let mut open = name_end;
            while bytes.get(open).is_some_and(u8::is_ascii_whitespace) {
                open += 1;
            }
            if bytes.get(open) == Some(&b'(') {
                let (argument_start, argument_end) = first_macro_argument(bytes, open)?;
                let call_end = macro_call_end(bytes, open)?;
                arguments.push(&source[argument_start..argument_end]);
                cursor = call_end;
                continue;
            }
            cursor = name_end;
            continue;
        }
        cursor += 1;
    }
    Ok(arguments)
}

fn line_prefix_is_space(bytes: &[u8], cursor: usize) -> bool {
    let line_start = bytes[..cursor]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    bytes[line_start..cursor]
        .iter()
        .all(u8::is_ascii_whitespace)
}

fn token_at(bytes: &[u8], start: usize, token: &[u8]) -> bool {
    if bytes.get(start..start + token.len()) != Some(token) {
        return false;
    }
    let before_ok = start == 0 || !is_m4_name_byte(bytes[start - 1]);
    let after = start + token.len();
    let after_ok = bytes.get(after).is_none_or(|byte| !is_m4_name_byte(*byte));
    before_ok && after_ok
}

const fn is_m4_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn line_comment_end(bytes: &[u8], cursor: usize) -> Option<usize> {
    if bytes.get(cursor) == Some(&b'#') {
        return Some(
            bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset + 1),
        );
    }
    if token_at(bytes, cursor, b"dnl") && bytes.get(cursor + 3).is_some_and(u8::is_ascii_whitespace)
    {
        return Some(
            bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset + 1),
        );
    }
    None
}

fn m4_quote_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut depth = 1_usize;
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'[' => depth = depth.checked_add(1)?,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn first_macro_argument(bytes: &[u8], open: usize) -> Result<(usize, usize)> {
    let mut cursor = open + 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if bytes.get(cursor).is_none() {
        return Err(invalid("AC_CONFIG_FILES has no first argument"));
    }

    if bytes[cursor] == b'[' {
        let end = m4_quote_end(bytes, cursor)
            .ok_or_else(|| invalid("AC_CONFIG_FILES has an unterminated M4 argument"))?;
        let mut after = end;
        while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
            after += 1;
        }
        if !matches!(bytes.get(after), Some(b',' | b')')) {
            return Err(invalid(
                "AC_CONFIG_FILES first argument is not a single literal M4 bracket",
            ));
        }
        return Ok((cursor + 1, end - 1));
    }

    let start = cursor;
    let mut bracket_depth = 0_usize;
    let mut paren_depth = 0_usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'[' => bracket_depth += 1,
            b']' if bracket_depth > 0 => bracket_depth -= 1,
            b'(' if bracket_depth == 0 => paren_depth += 1,
            b')' if bracket_depth == 0 => {
                if paren_depth == 0 {
                    break;
                }
                paren_depth -= 1;
            }
            b',' if bracket_depth == 0 && paren_depth == 0 => break,
            _ => {}
        }
        cursor += 1;
    }
    if cursor == start || bracket_depth != 0 || paren_depth != 0 {
        return Err(invalid("AC_CONFIG_FILES has a malformed first argument"));
    }
    Ok((start, cursor))
}

fn macro_call_end(bytes: &[u8], open: usize) -> Result<usize> {
    let mut cursor = open + 1;
    let mut paren_depth = 1_usize;
    let mut bracket_depth = 0_usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'[' => bracket_depth = bracket_depth.saturating_add(1),
            b']' if bracket_depth > 0 => bracket_depth -= 1,
            b'(' if bracket_depth == 0 => paren_depth += 1,
            b')' if bracket_depth == 0 => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    return Ok(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    Err(invalid("AC_CONFIG_FILES invocation is unterminated"))
}

fn literal_entries(argument: &str) -> Result<Vec<String>> {
    // This is already the macro argument after one M4 quote layer has been
    // removed. Shell-comment stripping would change a literal template name
    // such as config/x.in#suffix. Nested quoting is outside this closed list.
    if argument.contains(['[', ']']) {
        return Err(invalid(
            "AC_CONFIG_FILES first argument contains nested M4 quoting",
        ));
    }
    Ok(argument.split_whitespace().map(str::to_owned).collect())
}

fn expand_template(template: &str, substitutions: &BTreeMap<String, String>) -> Result<String> {
    if substitutions.len() > MAX_SUBSTITUTIONS {
        return Err(invalid(format!(
            "template substitutions exceed {MAX_SUBSTITUTIONS} entries"
        )));
    }
    for (token, value) in substitutions {
        if !valid_substitution_token(token) {
            return Err(invalid(format!(
                "unsafe template substitution token {token:?}"
            )));
        }
        if value.len() > MAX_SUBSTITUTION_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(invalid(format!(
                "template substitution {token} must be a bounded ASCII scalar"
            )));
        }
    }

    let tokens = template_tokens(template)?;
    let declared = substitutions.keys().cloned().collect::<BTreeSet<_>>();
    if tokens != declared {
        let missing = tokens.difference(&declared).cloned().collect::<Vec<_>>();
        let unused = declared.difference(&tokens).cloned().collect::<Vec<_>>();
        return Err(invalid(format!(
            "template substitution coverage is not exact (undeclared: {missing:?}; unused: {unused:?})"
        )));
    }

    let mut expanded = template.to_owned();
    for (token, value) in substitutions {
        expanded = expanded.replace(token, value);
        if expanded.len() > MAX_OUTPUT_BYTES {
            return Err(invalid("expanded Make template exceeds its byte limit"));
        }
    }
    if expanded.contains('@') {
        return Err(invalid(
            "expanded Make template contains a residual @ token marker",
        ));
    }
    Ok(expanded)
}

fn template_tokens(template: &str) -> Result<BTreeSet<String>> {
    let bytes = template.as_bytes();
    let mut tokens = BTreeSet::new();
    let mut cursor = 0;
    while let Some(relative_at) = bytes[cursor..].iter().position(|byte| *byte == b'@') {
        let start = cursor + relative_at;
        let Some(relative_end) = bytes[start + 1..].iter().position(|byte| *byte == b'@') else {
            return Err(invalid("template contains an unterminated @ token"));
        };
        let end = start + 1 + relative_end;
        let token = &template[start..=end];
        if !valid_substitution_token(token) {
            return Err(invalid(format!("template contains unsafe token {token:?}")));
        }
        tokens.insert(token.to_owned());
        cursor = end + 1;
    }
    Ok(tokens)
}

fn valid_substitution_token(token: &str) -> bool {
    let Some(name) = token
        .strip_prefix('@')
        .and_then(|rest| rest.strip_suffix('@'))
    else {
        return false;
    };
    (1..=64).contains(&name.len())
        && name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn validate_plain_make_assignments(path: &str, text: &str) -> Result<()> {
    if text.len() > MAX_OUTPUT_BYTES {
        return Err(invalid(format!(
            "expanded template {path:?} exceeds its output byte limit"
        )));
    }
    if !text.is_ascii() {
        return Err(invalid(format!(
            "expanded template {path:?} must contain only ASCII text"
        )));
    }

    let mut variables = BTreeSet::new();
    let mut assignment_count = 0_usize;
    let mut common_section_seen = false;
    for (line_number, line) in text.lines().enumerate() {
        let line_number = line_number + 1;
        if line.is_empty() || line.bytes().all(|byte| byte == b' ') {
            continue;
        }
        if line == "%common" {
            if common_section_seen || assignment_count != 0 {
                return Err(invalid(format!(
                    "template {path:?} line {line_number} has a duplicate or misplaced %common marker"
                )));
            }
            common_section_seen = true;
            continue;
        }
        if line.starts_with('#') {
            if !line
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
            {
                return Err(invalid(format!(
                    "template {path:?} line {line_number} contains control text"
                )));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(invalid(format!(
                "template {path:?} line {line_number} is indented and could be a recipe"
            )));
        }
        if line.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(invalid(format!(
                "template {path:?} line {line_number} contains control text"
            )));
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            return Err(invalid(format!(
                "template {path:?} line {line_number} is not a plain assignment"
            )));
        };
        if lhs.ends_with('+')
            || lhs.ends_with(':')
            || lhs.ends_with('?')
            || lhs.ends_with('!')
            || rhs.contains('=')
        {
            return Err(invalid(format!(
                "template {path:?} line {line_number} uses an unsupported Make assignment form"
            )));
        }
        let name = lhs.trim();
        if crate::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES.contains(&name)
            || name.starts_with("NATIVE_TARGET_")
        {
            return Err(invalid(format!(
                "template {path:?} line {line_number} shadows a reserved target identity or command role"
            )));
        }
        if name.is_empty()
            || name.len() > 64
            || !name
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(invalid(format!(
                "template {path:?} line {line_number} has an unsafe assignment name"
            )));
        }
        if !variables.insert(name.to_owned()) {
            return Err(invalid(format!(
                "template {path:?} assigns {name} more than once"
            )));
        }
        let value = rhs.trim();
        if !safe_assignment_value(value) {
            return Err(invalid(format!(
                "template {path:?} line {line_number} has an unsafe Make value"
            )));
        }
        assignment_count += 1;
        if assignment_count > MAX_ASSIGNMENTS {
            return Err(invalid(format!(
                "template {path:?} exceeds {MAX_ASSIGNMENTS} assignments"
            )));
        }
    }
    if assignment_count == 0 {
        return Err(invalid(format!(
            "template {path:?} contains no safe Make assignments"
        )));
    }
    Ok(())
}

fn safe_assignment_value(value: &str) -> bool {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte));
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn validate_relative_path(value: &str) -> std::result::Result<PathBuf, &'static str> {
    if value.is_empty() || value.len() > 4096 || value.starts_with('/') || value.contains('\\') {
        return Err("must be a bounded canonical source-relative path");
    }
    let mut result = PathBuf::new();
    for component in Path::new(value).components() {
        let Component::Normal(segment) = component else {
            return Err("must not contain absolute, current, or parent components");
        };
        let Some(segment) = segment.to_str() else {
            return Err("contains a non-UTF-8 component");
        };
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        {
            return Err("contains an unsafe path component");
        }
        result.push(segment);
    }
    if result.to_str() != Some(value) {
        return Err("must use canonical slash-separated syntax");
    }
    Ok(result)
}

fn folded_path(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn paths_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn invalid(message: impl Into<String>) -> ArosError {
    ArosError::Configuration {
        file: "generated Make template binding".to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn duplicate_contract_keys_are_rejected_before_projection() {
        let document = r#"{"template":"config/x.in","configure_source":"configure.ac","substitutions":{"@X@":"","@X@":"yes"}}"#;
        assert!(serde_json::from_str::<GeneratedMakeTemplateBinding>(document).is_err());
        let binding = r#"{"template":"config/x.in","configure_source":"configure.ac"}"#;
        let document = format!("{{\"gen/x\":{binding},\"gen/x\":{binding}}}");
        assert!(deserialize_bindings(&mut serde_json::Deserializer::from_str(&document)).is_err());
    }

    #[test]
    fn templates_cannot_override_target_selectors_or_admitted_tools() {
        for variable in ["AROS_TARGET_CPU", "CPU", "USE_MMU", "NATIVE_TARGET_AR"] {
            let mut fixture = Fixture::new();
            fixture.write(
                "config/include.cfg.in",
                format!("{variable}=@FEATURE@\n").as_bytes(),
            );
            assert!(fixture.resolve().is_err(), "{variable}");
        }
    }

    #[test]
    fn configure_argument_characters_are_not_reinterpreted_as_comments() {
        for argument in [
            "gen/include.cfg:config/include.cfg.in#suffix",
            "gen/include.cfg:config/include.cfg.in# dnl suffix",
            "[ gen/include.cfg:config/include.cfg.in ]",
        ] {
            let mut fixture = Fixture::new();
            fixture.write(
                "configure.ac",
                format!("AC_CONFIG_FILES([{argument}])\n").as_bytes(),
            );
            assert!(fixture.resolve().is_err(), "{argument}");
        }
    }

    struct Fixture {
        root: TempDir,
        bindings: BTreeMap<String, GeneratedMakeTemplateBinding>,
        seals: BTreeMap<String, Sha256Digest>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let mut fixture = Self {
                root,
                bindings: BTreeMap::new(),
                seals: BTreeMap::new(),
            };
            fixture.write(
                "configure.ac",
                b"AC_INIT([fixture], [1])\nAC_CONFIG_FILES([\n  gen/include.cfg:config/include.cfg.in\n])\n",
            );
            fixture.write("config/include.cfg.in", b"FEATURE=\"@FEATURE@\"\n");
            fixture.bindings.insert(
                "gen/include.cfg".into(),
                GeneratedMakeTemplateBinding {
                    template: "config/include.cfg.in".into(),
                    configure_source: "configure.ac".into(),
                    substitutions: BTreeMap::from([("@FEATURE@".into(), String::new())]),
                },
            );
            fixture
        }

        fn write(&mut self, path: &str, bytes: &[u8]) {
            let full = self.root.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, bytes).unwrap();
            self.seals.insert(path.into(), crate::sha256_bytes(bytes));
        }

        fn resolve(&self) -> Result<BTreeMap<String, ResolvedGeneratedMakeTemplate>> {
            resolve_generated_make_templates(self.root.path(), &self.bindings, &self.seals)
        }
    }

    #[test]
    fn preserves_template_quotes_and_resolves_the_exact_configure_pair() {
        let fixture = Fixture::new();
        let result = fixture.resolve().unwrap();
        let output = &result["gen/include.cfg"];
        assert_eq!(output.template_relative, "config/include.cfg.in");
        assert_eq!(output.expanded_text, "FEATURE=\"\"\n");
        assert_eq!(output.substitutions["@FEATURE@"], "");
    }

    #[test]
    fn admits_only_the_exact_leading_common_section_marker() {
        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"%common\nEXECSMP=\"@FEATURE@\"\n");
        let output = fixture.resolve().unwrap();
        assert_eq!(
            output["gen/include.cfg"].expanded_text,
            "%common\nEXECSMP=\"\"\n"
        );

        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"%other\nEXECSMP=\"@FEATURE@\"\n");
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"EXECSMP=\"@FEATURE@\"\n%common\n");
        assert!(fixture.resolve().is_err());
    }

    #[test]
    fn rejects_missing_unused_and_malformed_substitution_tokens() {
        let mut fixture = Fixture::new();
        fixture
            .bindings
            .get_mut("gen/include.cfg")
            .unwrap()
            .substitutions
            .clear();
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture
            .bindings
            .get_mut("gen/include.cfg")
            .unwrap()
            .substitutions
            .insert("@UNUSED@".into(), "value".into());
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"FEATURE=@FEATURE\n");
        assert!(fixture.resolve().is_err());
    }

    #[test]
    fn rejects_wrong_or_ambiguous_configure_ownership() {
        let mut fixture = Fixture::new();
        fixture.write(
            "configure.ac",
            b"AC_CONFIG_FILES([gen/include.cfg:config/other.cfg.in])\n",
        );
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.write(
            "configure.ac",
            b"# AC_CONFIG_FILES([gen/include.cfg:config/include.cfg.in])\n",
        );
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.write(
            "configure.ac",
            b"AC_CONFIG_FILES([gen/include.cfg:config/include.cfg.in])\nAC_CONFIG_FILES(gen/include.cfg:config/include.cfg.in)\n",
        );
        assert!(fixture.resolve().is_err());
    }

    #[test]
    fn rejects_stale_seals_unsealed_inputs_and_symlinked_sources() {
        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"FEATURE=\"@FEATURE@\"\n");
        fixture.seals.insert(
            "config/include.cfg.in".into(),
            crate::sha256_bytes(b"stale bytes"),
        );
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.seals.remove("configure.ac");
        assert!(fixture.resolve().is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let fixture = Fixture::new();
            fs::remove_file(fixture.root.path().join("config/include.cfg.in")).unwrap();
            symlink(
                fixture.root.path().join("configure.ac"),
                fixture.root.path().join("config/include.cfg.in"),
            )
            .unwrap();
            assert!(fixture.resolve().is_err());
        }
    }

    #[test]
    fn rejects_unsafe_values_paths_and_make_forms() {
        for value in ["$(shell touch /tmp/x)", "\"\"", "a b", "@OTHER@", "a\nB=x"] {
            let mut fixture = Fixture::new();
            fixture
                .bindings
                .get_mut("gen/include.cfg")
                .unwrap()
                .substitutions
                .insert("@FEATURE@".into(), value.into());
            assert!(
                fixture.resolve().is_err(),
                "accepted substitution {value:?}"
            );
        }

        let mut fixture = Fixture::new();
        fixture.write(
            "config/include.cfg.in",
            b"include other.mk\nFEATURE=@FEATURE@\n",
        );
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        fixture.write("config/include.cfg.in", b"FEATURE := @FEATURE@\n");
        assert!(fixture.resolve().is_err());

        let mut fixture = Fixture::new();
        let duplicate = fixture.bindings["gen/include.cfg"].clone();
        fixture.bindings.insert("GEN/include.cfg".into(), duplicate);
        assert!(fixture.resolve().is_err());
    }

    #[test]
    fn configure_search_ignores_m4_quoted_macro_text() {
        let source = "AC_DEFUN([FOO], [AC_CONFIG_FILES([gen/a:src/a.in])])\nAC_CONFIG_FILES([gen/b:src/b.in])\n";
        let calls = active_config_files_arguments(source).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(literal_entries(calls[0]).unwrap(), ["gen/b:src/b.in"]);
    }
}
