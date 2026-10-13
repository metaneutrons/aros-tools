//! Exact, sealed native source-option includes. No legacy glob discovery.

use super::{
    issue, parse_include_directive, ExpansionState, IncludeDirective, LocalMakeIncludeIssue,
    LocalMakeIncludeIssueKind, LocalMakeIncludeScan,
};
use crate::make_vars::ConditionalTruth;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) enum Resolution {
    Inactive,
    Absent(PathBuf),
    Present(String),
}

pub(super) struct ResolutionError {
    pub issue: LocalMakeIncludeIssue,
    pub options_possible: bool,
}

pub(super) fn is_candidate(raw: &str) -> bool {
    let raw = raw.trim();
    raw.split_whitespace()
        .any(|word| is_options_path(Path::new(word)))
        || raw.contains('$') && !(raw.starts_with("$(TOP)/") || raw.starts_with("${TOP}/"))
}

pub(super) fn is_options_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("opts"))
}

pub(super) fn potential_options_path(raw: &str) -> bool {
    raw.split_whitespace()
        .any(|word| is_options_path(Path::new(word)) || word.contains('$'))
}

pub const fn contains_options(scan: &LocalMakeIncludeScan) -> bool {
    scan.options_scope_required
}

pub(super) fn resolve(
    include: IncludeDirective<'_>,
    prefix: &str,
    physical: &str,
    source: &Path,
    line: usize,
    state: &mut ExpansionState,
) -> Result<Resolution, ResolutionError> {
    let fail = |kind, detail: String| ResolutionError {
        issue: issue(source, line, kind, include.path, detail),
        options_possible: potential_options_path(include.path),
    };
    let context = state
        .native_context
        .as_ref()
        .expect("native context checked by caller");
    if state.option_directives > state.limits.files {
        return Err(fail(
            LocalMakeIncludeIssueKind::FileLimit,
            "native include path evaluation exceeded its directive budget".into(),
        ));
    }
    if prefix.len() > state.limits.bytes {
        return Err(fail(
            LocalMakeIncludeIssueKind::ByteLimit,
            "native include path evaluation exceeded its bounded scope".into(),
        ));
    }
    if inside_define(prefix) {
        return Ok(Resolution::Inactive);
    }
    let snapshot = crate::parser::join_continuations(&format!("{prefix}{physical}\n"));
    let position = snapshot.lines().count().saturating_sub(1);
    let (scope, states) = crate::make_vars::collect_vars_impl(&snapshot, Some(context));
    match states
        .get(position)
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
    {
        ConditionalTruth::False => return Ok(Resolution::Inactive),
        ConditionalTruth::Unknown => {
            return Err(fail(
                LocalMakeIncludeIssueKind::UnresolvedPath,
                "native source options include is inside an unresolved Make conditional".into(),
            ))
        }
        ConditionalTruth::True => {}
    }
    let raw = include.path.trim();
    if raw.len() > 4096 || !safe_expression(raw) {
        return Err(fail(
            LocalMakeIncludeIssueKind::UnresolvedPath,
            "native source options need one source-root expression with simple variable references"
                .into(),
        ));
    }
    if state.native_dirs.is_none() {
        let (dirs, measured_bytes) = source_dirs(state)
            .map_err(|detail| fail(LocalMakeIncludeIssueKind::UnsafeSyntax, detail))?;
        if let Some(bytes) = measured_bytes {
            state.files_read += 1;
            state.bytes_read += bytes;
        }
        state.native_dirs = Some(dirs);
    }
    let dirs = state
        .native_dirs
        .as_ref()
        .expect("source directory snapshot initialized");
    let guard = |name: &str| {
        (scope.path_is_conditional_at(name, position)
            || matches!(name, "SRCDIR" | "CURDIR") && scope.is_known_local(name))
            .then(|| format!("options path variable {name} is conditional, opaque or overrides the source location"))
    };
    if scope.is_known_local("SRCDIR") || scope.is_known_local("CURDIR") {
        return Err(fail(
            LocalMakeIncludeIssueKind::UnresolvedPath,
            "native options may not override the declaring source location".into(),
        ));
    }
    let expression =
        crate::MakeExprContext::new(&scope, dirs, position, &state.root, &state.curdir)
            .with_guard(&guard)
            .without_filesystem();
    let expanded = crate::evaluate_make_expr(raw, &expression)
        .map_err(|error| fail(LocalMakeIncludeIssueKind::UnresolvedPath, error.to_string()))?;
    // After complete expansion, classify the actual tokens. A generated
    // entrypoints include is not source options; a variable-expanded list
    // containing an options file remains an error even with a .mk suffix.
    let fail = |kind, detail: String| ResolutionError {
        issue: issue(source, line, kind, include.path, detail),
        options_possible: expanded
            .split_whitespace()
            .any(|word| is_options_path(Path::new(word))),
    };
    // The expression evaluator intentionally retains SRCDIR as an opaque
    // CMake root even with filesystem access disabled. Materialize that one
    // proven leading root here; no other deferred path becomes a local input.
    let relative_text = expanded
        .strip_prefix("${AROS_SOURCE_DIR}/")
        .ok_or_else(|| {
            fail(
                LocalMakeIncludeIssueKind::OutsideSourceTree,
                "native source options leave the source root".into(),
            )
        })?;
    let relative = Path::new(relative_text);
    if !safe_relative(relative)
        || relative_text.contains(['$', '\\', '*', '?', '[', ']', ':'])
        || relative_text.contains(char::is_whitespace)
    {
        return Err(fail(
            LocalMakeIncludeIssueKind::UnresolvedPath,
            "native source options path is not a normalized single source file".into(),
        ));
    }
    let relative = relative.components().collect::<PathBuf>();
    if is_options_path(&relative)
        && context
            .make_include_bindings
            .contains_key(relative.to_string_lossy().as_ref())
    {
        return Err(fail(
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            "source options may not be replaced by a native configuration binding".into(),
        ));
    }
    if state.files_read >= state.limits.files {
        return Err(fail(
            LocalMakeIncludeIssueKind::FileLimit,
            "the source options include-count limit was reached".into(),
        ));
    }
    let absent = prove_path(&state.root, &relative)
        .map_err(|detail| fail(LocalMakeIncludeIssueKind::OutsideSourceTree, detail))?;
    if absent {
        if !include.optional {
            return Err(fail(
                LocalMakeIncludeIssueKind::Missing,
                format!(
                    "required source options are missing: {}",
                    relative.display()
                ),
            ));
        }
        return Ok(Resolution::Absent(relative));
    }
    Ok(Resolution::Present(relative.to_string_lossy().into_owned()))
}

fn source_dirs(state: &ExpansionState) -> Result<(crate::dirs::DirVars, Option<usize>), String> {
    let context = state
        .native_context
        .as_ref()
        .expect("native context checked");
    let path = "config/make.cfg.in";
    let Some(expected) = context.native_configuration_inputs.get(path) else {
        return Ok((crate::dirs::DirVars::default(), None));
    };
    if state.files_read >= state.limits.files {
        return Err("sealed directory configuration exceeds its file limit".into());
    }
    if prove_path(&state.root, Path::new(path))? {
        return Err("sealed directory configuration is missing".into());
    }
    let absolute = state.root.join(path);
    let (_, bytes) = aros_common::publication::measure_regular_file_bounded(
        &absolute,
        state.limits.bytes.saturating_sub(state.bytes_read) as u64,
    )
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "sealed directory configuration disappeared".to_owned())?;
    if bytes.len() > state.limits.bytes.saturating_sub(state.bytes_read)
        || aros_common::sha256_bytes(&bytes).as_str() != expected
    {
        return Err("sealed directory configuration bytes changed".into());
    }
    let measured = bytes.len();
    Ok((
        crate::dirs::DirVars::from_config_text(&aros_common::decode_source_bytes(bytes)),
        Some(measured),
    ))
}

fn inside_define(prefix: &str) -> bool {
    let mut depth = 0usize;
    for raw in prefix.lines() {
        update_define_depth(raw, &mut depth);
    }
    depth != 0
}

fn update_define_depth(raw: &str, depth: &mut usize) {
    if raw.starts_with('\t') {
        return;
    }
    let mut words = raw.split_whitespace();
    let mut word = words.next();
    while matches!(word, Some("override" | "export" | "private" | "unexport")) {
        word = words.next();
    }
    match word {
        Some("define") => *depth += 1,
        Some("endef") => *depth = depth.saturating_sub(1),
        _ => {}
    }
}

pub(super) fn report_unbound_includes(
    scan: &mut LocalMakeIncludeScan,
    original: &str,
    root: &Path,
    source: &Path,
    context: &crate::TargetContext,
) {
    let joined = crate::parser::join_continuations(&scan.expanded);
    let (_, states) = crate::make_vars::collect_vars_impl(&joined, Some(context));
    let positions =
        crate::parser::architecture_scope_positions(root, source, original, scan, &joined);
    let mut define_depth = 0usize;
    for (line, raw) in joined.lines().enumerate() {
        if define_depth == 0
            && parse_include_directive(raw).is_some()
            && states.get(line) != Some(&ConditionalTruth::False)
        {
            let physical = positions.as_ref().and_then(|positions| {
                positions
                    .iter()
                    .position(|position| *position == Some(line))
            });
            scan.issues.push(issue(source, physical.map_or(1, |line| line + 1),
                LocalMakeIncludeIssueKind::UnresolvedPath, raw,
                "active native Make include remains unbound; no partial configuration is authoritative"));
        }
        update_define_depth(raw, &mut define_depth);
    }
}

/// Only simple references and portable literal path characters; functions,
/// escaped dollars and shell forms cannot enter even a lazy Make branch.
fn safe_expression(raw: &str) -> bool {
    let mut rest = raw;
    while let Some(at) = rest.find('$') {
        if !safe_literal(&rest[..at]) {
            return false;
        }
        let tail = &rest[at + 1..];
        let (close, body) = match tail.as_bytes().first() {
            Some(b'(') => (')', &tail[1..]),
            Some(b'{') => ('}', &tail[1..]),
            _ => return false,
        };
        let Some(end) = body.find(close) else {
            return false;
        };
        let name = &body[..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return false;
        }
        rest = &body[end + 1..];
    }
    safe_literal(rest)
}

fn safe_literal(raw: &str) -> bool {
    raw.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
}

fn safe_relative(relative: &Path) -> bool {
    !relative.as_os_str().is_empty()
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// Returns true only for actual NotFound under regular, non-symlink source
/// directories. Permission, dangling symlink and non-directory errors refuse.
fn prove_path(root: &Path, relative: &Path) -> Result<bool, String> {
    if !safe_relative(relative) {
        return Err("unsafe source options path".into());
    }
    let mut path = root.to_path_buf();
    let parts = relative.components().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        path.push(part.as_os_str());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(format!("source options path cannot be checked: {error}")),
        };
        if metadata.file_type().is_symlink()
            || if index + 1 == parts.len() {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err(
                "source options require regular files and directories without symlinks".into(),
            );
        }
    }
    Ok(false)
}

pub fn verify_absent(root: &Path, relative: &Path) -> Result<(), String> {
    if prove_path(root, relative)? {
        Ok(())
    } else {
        Err(format!(
            "optional source options appeared during replay: {}",
            relative.display()
        ))
    }
}

pub(super) fn verify_bytes(
    path: &Path,
    bytes: &[u8],
    source: &Path,
    line: usize,
    state: &ExpansionState,
) -> Result<(), Vec<LocalMakeIncludeIssue>> {
    let Some(context) = &state.native_context else {
        return Ok(());
    };
    if context.native_configuration_inputs.is_empty() && !is_options_path(path) {
        return Ok(());
    }
    let relative = path.strip_prefix(&state.root).map_err(|_| {
        vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::OutsideSourceTree,
            path.display().to_string(),
            "source options leave source root",
        )]
    })?;
    let key = relative.to_string_lossy();
    if context
        .native_configuration_inputs
        .get(key.as_ref())
        .is_none_or(|hash| hash != aros_common::sha256_bytes(bytes).as_str())
    {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::UnsafeSyntax,
            key.as_ref(),
            "native source options input is unsealed or its bytes changed",
        )]);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
