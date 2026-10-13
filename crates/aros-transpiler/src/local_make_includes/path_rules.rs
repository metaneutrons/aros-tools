//! Source-confined path validation shared by native include expansion.

use super::{
    issue, ExpansionState, LocalMakeFragmentPolicy, LocalMakeIncludeIssue,
    LocalMakeIncludeIssueKind,
};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

pub(super) fn resolve_local_path(
    raw: &str,
    source: &Path,
    line: usize,
    state: &ExpansionState,
) -> Result<PathBuf, Vec<LocalMakeIncludeIssue>> {
    let raw = raw.trim();
    if raw.is_empty() || raw.split_whitespace().count() != 1 {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::UnresolvedPath,
            raw,
            "a local include must name exactly one fragment",
        )]);
    }
    let root_text = state.root.to_string_lossy();
    let curdir_text = state.curdir.to_string_lossy();
    let expanded = raw
        .replace("$(SRCDIR)", &root_text)
        .replace("${SRCDIR}", &root_text)
        .replace("$(CURDIR)", &curdir_text)
        .replace("${CURDIR}", &curdir_text);
    if expanded.contains('$') || expanded.contains('*') || expanded.contains('?') {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::UnresolvedPath,
            raw,
            "the local include path is dynamic",
        )]);
    }

    let candidate = PathBuf::from(expanded);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        state.root.join(candidate)
    };
    let lexical = lexical_normalize(&candidate);
    if !lexical.starts_with(&state.root) {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::OutsideSourceTree,
            raw,
            "the local include path leaves the source tree",
        )]);
    }
    let canonical = match fs::canonicalize(&lexical) {
        Ok(path) => path,
        Err(error) => {
            return Err(vec![issue(
                source,
                line,
                LocalMakeIncludeIssueKind::Missing,
                raw,
                format!("the local fragment cannot be resolved: {error}"),
            )]);
        }
    };
    if !canonical.starts_with(&state.root) {
        return Err(vec![issue(
            source,
            line,
            LocalMakeIncludeIssueKind::OutsideSourceTree,
            raw,
            "the local include resolves outside the source tree",
        )]);
    }
    if state.policy == LocalMakeFragmentPolicy::NativeConfiguration {
        if canonical != lexical {
            return Err(vec![issue(
                source,
                line,
                LocalMakeIncludeIssueKind::OutsideSourceTree,
                raw,
                "native configuration may not traverse symlinks",
            )]);
        }
        assert_bound_regular(&canonical, &state.root, source, line, raw)?;
    }
    Ok(canonical)
}

pub(super) fn safe_bound_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['$', '\\'])
        && !path.contains(char::is_whitespace)
        && Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "mk")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn bound_include_key(raw: &str, state: &ExpansionState) -> Option<String> {
    let relative = raw
        .trim()
        .strip_prefix("$(SRCDIR)/")
        .or_else(|| raw.trim().strip_prefix("${SRCDIR}/"))
        .unwrap_or_else(|| raw.trim());
    let expanded = relative
        .replace("$(CURDIR)", &state.curdir.to_string_lossy())
        .replace("${CURDIR}", &state.curdir.to_string_lossy());
    if expanded.is_empty()
        || expanded.contains(['$', '\\'])
        || expanded.contains(char::is_whitespace)
        || !Path::new(&expanded)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(expanded)
}

pub(super) fn assert_bound_regular(
    path: &Path,
    root: &Path,
    source: &Path,
    line: usize,
    subject: &str,
) -> Result<(), Vec<LocalMakeIncludeIssue>> {
    let mut cursor = path.to_path_buf();
    while cursor != root {
        let valid = fs::symlink_metadata(&cursor).is_ok_and(|metadata| {
            !metadata.file_type().is_symlink()
                && if cursor == path {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                }
        });
        if !cursor.starts_with(root) || !valid || !cursor.pop() {
            return Err(vec![issue(source, line, LocalMakeIncludeIssueKind::OutsideSourceTree,
                subject, "native configuration endpoints must be regular source files without symlink ancestors")]);
        }
    }
    Ok(())
}
