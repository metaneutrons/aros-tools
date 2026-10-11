//! Bounded path-closure checks for one ordinary LLVM SDK application link.
//!
//! The trace is the authoritative used-input list, including source-selected
//! archives and Clang's one short-lived compilation object. The map is bounded
//! retained evidence and checked for original-SDK leakage after relocation,
//! not parsed as another input list. This does not prove ABI facts, full header
//! closure, archive contents, or SDK completeness.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};

use crate::ContractError;

const LLVM_TEMPORARY_SUFFIX_BYTES: usize = 6;
const MAX_LINK_TRACE_BYTES: usize = 256 * 1024 + 256;
const MAX_LINK_MAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_SELECTED_LIBRARIES: usize = 64;
const MAX_LIBRARY_NAME_BYTES: usize = 64;

/// Require each traced non-temporary input to resolve inside the selected SDK
/// or compiler tree, and admit only the direct Clang temporary derived from the
/// sealed fixture name.
///
/// The map and trace are text produced by one ordinary `--trace` link. Archive
/// member spellings are accepted only as `absolute-archive.a(member)`; the
/// archive path itself remains the input whose location is checked. This proves
/// a bounded input-path closure, not ABI correctness, complete header closure,
/// SDK completeness, or that the files contain valid archives.
pub(super) fn validate_link_paths(
    trace: &[u8],
    map: &[u8],
    sdk: &Path,
    compiler: &Path,
    temporary: &Path,
    original_sdk: &Path,
    selection: (bool, &[String], &Path),
) -> Result<(), ContractError> {
    let (relocated, libraries, fixture) = selection;
    if trace.len() > MAX_LINK_TRACE_BYTES || map.len() > MAX_LINK_MAP_BYTES {
        return Err(error("LLVM SDK linker evidence exceeds its byte bound"));
    }
    let trace =
        std::str::from_utf8(trace).map_err(|_| error("LLVM SDK linker trace is not UTF-8"))?;
    let map = std::str::from_utf8(map).map_err(|_| error("LLVM SDK linker map is not UTF-8"))?;
    if trace.contains(" bytes omitted by aros]") || map.contains(" bytes omitted by aros]") {
        return Err(error(
            "LLVM SDK linker evidence is truncated and cannot prove its input paths",
        ));
    }
    if map.is_empty() {
        return Err(error("LLVM SDK link map is empty"));
    }

    let sdk = canonical_directory(sdk, "LLVM SDK root")?;
    let compiler = canonical_directory(compiler, "LLVM compiler root")?;
    let temporary = validate_temporary_root(temporary)?;
    let original_sdk_text = original_sdk
        .to_str()
        .ok_or_else(|| error("original LLVM SDK path is not UTF-8"))?;
    let original_sdk_canonical = canonical_directory(original_sdk, "original LLVM SDK root")?;
    let original_sdk_canonical_text = original_sdk_canonical
        .to_str()
        .ok_or_else(|| error("original LLVM SDK path is not UTF-8"))?;
    if relocated
        && (trace.contains(original_sdk_text)
            || map.contains(original_sdk_text)
            || trace.contains(original_sdk_canonical_text)
            || map.contains(original_sdk_canonical_text))
    {
        return Err(error(
            "relocated LLVM SDK link retained its original SDK path",
        ));
    }

    let fixture_stem = fixture
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty() && !stem.chars().any(char::is_control))
        .ok_or_else(|| error("sealed LLVM SDK fixture has no portable file stem"))?;
    let temporary_prefix = format!("{fixture_stem}-");
    let selected_archives = selected_archive_names(libraries)?;
    let mut resolved_sdk_archives = BTreeSet::new();
    let mut saw_sdk_archive = false;
    let mut linked_inputs = 0usize;

    for line in trace.lines().filter(|line| !line.is_empty()) {
        let input = parse_trace_input(line)?;
        let path = Path::new(input.path);

        // Clang deletes the temporary before exiting, so a missing direct child
        // is expected. Its basename is derived from the sealed source fixture;
        // LLVM's mkstemp suffix is six ASCII alphanumeric bytes, not hex-only.
        if path.starts_with(&temporary) {
            validate_driver_temporary(path, &temporary, &temporary_prefix)?;
            linked_inputs = linked_inputs.saturating_add(1);
            continue;
        }

        let resolved = path
            .canonicalize()
            .map_err(|_| error("LLVM SDK linker trace names an unresolved input"))?;
        let metadata = fs::metadata(&resolved)
            .map_err(|_| error("LLVM SDK linker trace names an unresolved input"))?;
        if !metadata.is_file() {
            return Err(error("LLVM SDK linker trace names a non-file input"));
        }
        let under_sdk = resolved.starts_with(&sdk);
        if !under_sdk && !resolved.starts_with(&compiler) {
            return Err(error(
                "LLVM SDK linker trace selected an input outside the SDK or compiler roots",
            ));
        }

        if path.extension().is_some_and(|extension| extension == "a") {
            if under_sdk {
                saw_sdk_archive = true;
            }
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if selected_archives.contains(name) {
                    if !under_sdk {
                        return Err(error(
                            "source-declared LLVM SDK library resolved outside the selected SDK",
                        ));
                    }
                    resolved_sdk_archives.insert(name.to_owned());
                }
            }
        }
        linked_inputs = linked_inputs.saturating_add(1);
    }

    if !saw_sdk_archive || linked_inputs == 0 {
        return Err(error(
            "ordinary LLVM SDK link lacks a traced SDK archive or input",
        ));
    }
    if resolved_sdk_archives.len() != selected_archives.len() {
        return Err(error(
            "source-declared LLVM SDK library did not resolve from the selected SDK",
        ));
    }
    Ok(())
}

struct TraceInput<'a> {
    path: &'a str,
}

fn parse_trace_input(line: &str) -> Result<TraceInput<'_>, ContractError> {
    if line.trim() != line || line.chars().any(char::is_control) || line.contains([',', '\\', ':'])
    {
        return Err(error(
            "LLVM SDK linker trace contains an ambiguous raw path",
        ));
    }

    let path_text = if line.ends_with(')') {
        let open = line
            .rfind('(')
            .ok_or_else(|| error("LLVM SDK linker trace has malformed archive syntax"))?;
        let archive = &line[..open];
        let member = &line[open + 1..line.len() - 1];
        if Path::new(archive).extension() != Some(std::ffi::OsStr::new("a"))
            || member.is_empty()
            || member.trim() != member
            || member.chars().any(|character| {
                matches!(character, '(' | ')' | '\\' | ',' | ':') || character.is_control()
            })
            || member.starts_with('/')
            || member
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(error("LLVM SDK linker trace has malformed archive syntax"));
        }
        archive
    } else {
        line
    };

    validate_absolute_raw_path(path_text)?;
    Ok(TraceInput { path: path_text })
}

fn validate_absolute_raw_path(path: &str) -> Result<(), ContractError> {
    let candidate = Path::new(path);
    if !candidate.is_absolute()
        || path.contains("//")
        || path
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || candidate
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || candidate.file_name().is_none()
    {
        return Err(error(
            "LLVM SDK linker trace contains a non-absolute or ambiguous input path",
        ));
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(error(format!("{label} must be absolute")));
    }
    let resolved = path
        .canonicalize()
        .map_err(|_| error(format!("{label} cannot be resolved")))?;
    if !resolved.is_dir() {
        return Err(error(format!("{label} is not a directory")));
    }
    Ok(resolved)
}

fn validate_temporary_root(path: &Path) -> Result<PathBuf, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| error("cannot inspect the private LLVM driver temporary root"))?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(error(
            "LLVM driver temporary root is not a private real directory",
        ));
    }
    let resolved = path
        .canonicalize()
        .map_err(|_| error("cannot resolve the private LLVM driver temporary root"))?;
    if resolved != path {
        return Err(error(
            "LLVM driver temporary root is not given by its canonical path",
        ));
    }
    Ok(resolved)
}

fn validate_driver_temporary(
    path: &Path,
    temporary: &Path,
    expected_prefix: &str,
) -> Result<(), ContractError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| error("LLVM driver temporary name is not portable"))?;
    let suffix = name
        .strip_prefix(expected_prefix)
        .and_then(|name| name.strip_suffix(".o"));
    if path.parent() != Some(temporary)
        || !suffix.is_some_and(|suffix| {
            suffix.len() == LLVM_TEMPORARY_SUFFIX_BYTES
                && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(error(
            "LLVM trace contains an unrecognized driver temporary input",
        ));
    }

    match fs::symlink_metadata(path) {
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            let resolved = path
                .canonicalize()
                .map_err(|_| error("cannot resolve the LLVM driver temporary input"))?;
            if resolved.parent() != Some(temporary) {
                return Err(error(
                    "LLVM driver temporary input escapes its private root",
                ));
            }
            Ok(())
        }
        _ => Err(error("LLVM driver temporary input is unsafe or unresolved")),
    }
}

fn selected_archive_names(libraries: &[String]) -> Result<BTreeSet<String>, ContractError> {
    if libraries.len() > MAX_SELECTED_LIBRARIES {
        return Err(error("LLVM SDK library selection exceeds its item bound"));
    }
    let mut selected = BTreeSet::new();
    for library in libraries {
        if library.is_empty()
            || library.len() > MAX_LIBRARY_NAME_BYTES
            || !library
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
        {
            return Err(error("LLVM SDK library selection is not a bare identifier"));
        }
        if !selected.insert(format!("lib{library}.a")) {
            return Err(error("LLVM SDK library selection contains a duplicate"));
        }
    }
    Ok(selected)
}

fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "llvm_sdk_link_paths_tests.rs"]
mod tests;
