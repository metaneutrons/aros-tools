//! Positioned selective Make directory searches, without executing a shell.
//!
//! Expressions are evaluated by the caller at their original declaration line.
//! Candidate observation is also supplied by the caller, which must enforce its
//! own roots, no-follow file policy and retained-input identity requirements.

use crate::make_rule_events::SearchPathDeclaration;
use crate::make_vars::{MakeFileEventKind, MakeFileProgram};
use aros_common::{make_input_snapshot::MakeInputReader, TreeContentEntryKind};
use std::path::{Path, PathBuf};

mod frozen;
pub use frozen::{DeclaredInputMembership, FrozenDirectorySearch};

const MAX_ENTRIES: usize = 16_384;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_VALUE_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_EVENTS: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPathEntry {
    pub output_line: usize,
    pub source_line: usize,
    pub pattern: String,
    pub directories: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPaths {
    entries: Vec<SearchPathEntry>,
}

impl SearchPaths {
    #[must_use]
    pub fn entries(&self) -> &[SearchPathEntry] {
        &self.entries
    }

    /// Find the first captured regular input using this reader's bound cwd.
    ///
    /// Absolute inputs are checked directly without directory search. Relative
    /// inputs use cwd, selective vpath, then the caller's evaluated global VPATH.
    /// Unsafe/nonregular first candidates refuse instead of falling through.
    /// The caller must use this within the snapshot transaction and bind the
    /// global VPATH value to the final global program, not a target-local
    /// variable view. GNU Make constructs this search list before target
    /// traversal. Prefer `Self::freeze_global` for a bound search. No recipes run.
    /// This is membership evidence, not a claim that the file was read or that
    /// GNU retained this path during timestamp/remake processing.
    ///
    /// # Errors
    /// Refuses unsupported spellings, unsafe/changed/unbound candidates and
    /// exhausted search or reader limits. Missing intermediate components count
    /// as absent only after the captured reader proves the ancestor's absence;
    /// links and non-directory ancestors still refuse.
    pub fn find_regular_input(
        &self,
        file: &str,
        global_vpath: &str,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<Option<PathBuf>, String> {
        validate_word(file)?;
        let candidates = if Path::new(file).is_absolute() {
            vec![PathBuf::from(file)]
        } else {
            self.candidates(file, reader.working_directory(), global_vpath)?
        };
        for candidate in candidates {
            let identity = candidate.to_str().ok_or("Make search input is not UTF-8")?;
            match reader.wildcard_entry_kind(identity)? {
                None => {}
                Some(TreeContentEntryKind::RegularFile) => return Ok(Some(candidate)),
                Some(_) => {
                    return Err("Make search candidate is not a captured regular input".into())
                }
            }
        }
        Ok(None)
    }

    /// Search the literal relative prerequisite first, then matching selective
    /// entries in declaration order, then the caller's evaluated global VPATH.
    /// No candidate is assumed to exist, be regular or be an allowed input.
    ///
    /// # Errors
    /// Rejects nonliteral/absolute file names, nonabsolute working coordinates,
    /// unsupported path syntax or candidate-count/byte exhaustion.
    pub fn candidates(
        &self,
        file: &str,
        working_directory: &Path,
        global_vpath: &str,
    ) -> Result<Vec<PathBuf>, String> {
        validate_word(file)?;
        if Path::new(file).is_absolute() || !working_directory.is_absolute() {
            return Err(
                "Make vpath requires a relative file and absolute working directory".into(),
            );
        }
        let mut result = vec![working_directory.join(file)];
        let mut bytes = result[0].as_os_str().len();
        if bytes > MAX_BYTES {
            return Err("Make vpath exceeds candidate byte limit".into());
        }
        for entry in &self.entries {
            if pattern_matches(&entry.pattern, file) {
                for directory in &entry.directories {
                    append_candidate(&mut result, &mut bytes, working_directory, directory, file)?;
                }
            }
        }
        for directory in directory_words(global_vpath)? {
            append_candidate(&mut result, &mut bytes, working_directory, &directory, file)?;
        }
        Ok(result)
    }
}

/// Project selective-search directives frozen by the source interpreter.
///
/// Active `SearchPath` payloads already expanded at their original read events;
/// they must not be evaluated again using final variables. Includes/eval remain
/// distinct through their event indices; input/generated provenance stays in
/// the original file. This discharges no other source policy or native binding.
///
/// # Errors
/// Refuses excessive events/copied declarations and the existing bounded
/// resolver's unsupported search syntax. No partial result is returned.
pub fn project_interpreted(file: &MakeFileProgram) -> Result<SearchPaths, String> {
    if file.events.len() > MAX_SOURCE_EVENTS {
        return Err("Make search projection exceeds source event limit".into());
    }
    let mut declarations = Vec::new();
    let mut retained_bytes = 0usize;
    for (event_index, event) in file.events.iter().enumerate() {
        if let MakeFileEventKind::SearchPath(arguments) = &event.kind {
            if !event.active {
                continue;
            }
            retained_bytes = retained_bytes
                .checked_add(arguments.len())
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<SearchPathDeclaration>()))
                .filter(|bytes| *bytes <= MAX_BYTES)
                .ok_or("Make search projection exceeds retained byte limit")?;
            if declarations.len() == MAX_ENTRIES || arguments.len() > MAX_VALUE_BYTES {
                return Err("Make search projection exceeds declaration limit".into());
            }
            declarations.push(SearchPathDeclaration {
                output_line: event_index,
                source_line: event.source_line,
                arguments: arguments.clone(),
            });
        }
    }
    project(&declarations, &|frozen, _| Ok(frozen.to_owned()))
}

/// Evaluate and apply selective-search declarations as one complete operation.
///
/// Bare `vpath` clears all entries; `vpath PATTERN` clears that exact pattern;
/// repeated pattern declarations with directories append, not replace.
///
/// # Errors
/// Rejects unordered declarations, expression errors, unsupported pattern/path
/// syntax and resource exhaustion. Empty entries are not invented for unknown
/// values: the supplied evaluator must distinguish unknown from known-empty.
pub fn project(
    declarations: &[SearchPathDeclaration],
    evaluate: &impl Fn(&str, usize) -> Result<String, String>,
) -> Result<SearchPaths, String> {
    if declarations.len() > MAX_ENTRIES {
        return Err("Make vpath exceeds declaration limit".into());
    }
    let mut result = SearchPaths::default();
    let mut previous = None;
    let mut total_bytes = 0usize;
    let mut total_directories = 0usize;
    for declaration in declarations {
        if previous.is_some_and(|line| line >= declaration.output_line) {
            return Err("Make vpath declarations are not in unique output order".into());
        }
        previous = Some(declaration.output_line);
        let value = evaluate(&declaration.arguments, declaration.source_line)?;
        total_bytes = total_bytes
            .checked_add(value.len())
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or("Make vpath exceeds evaluated byte limit")?;
        if value.len() > MAX_VALUE_BYTES {
            return Err("Make vpath declaration exceeds value byte limit".into());
        }
        let value = value.trim();
        if value.is_empty() {
            result.entries.clear();
            continue;
        }
        let (pattern, directories) = value
            .find(char::is_whitespace)
            .map_or((value, ""), |end| (&value[..end], value[end..].trim()));
        validate_word(pattern)?;
        if pattern.matches('%').count() > 1 || pattern.contains(':') {
            return Err("Make vpath pattern is outside the single-percent model".into());
        }
        if directories.is_empty() {
            result.entries.retain(|entry| entry.pattern != pattern);
            continue;
        }
        let directories = directory_words(directories)?;
        if directories.is_empty() {
            // A supplied separator-only list is not the `vpath PATTERN`
            // clearing form; GNU Make leaves earlier entries intact.
            continue;
        }
        total_directories = total_directories
            .checked_add(directories.len())
            .filter(|count| *count <= MAX_ENTRIES)
            .ok_or("Make vpath exceeds directory count limit")?;
        result.entries.push(SearchPathEntry {
            output_line: declaration.output_line,
            source_line: declaration.source_line,
            pattern: pattern.to_owned(),
            directories,
        });
    }
    Ok(result)
}

/// Observe candidates in order and stop on the FIRST present one.
///
/// An unsafe,
/// unreadable or otherwise unproved first candidate must be an error from the
/// observer, never treated as absent to select a later desired source.
///
/// # Errors
/// Rejects oversized candidate lists or propagates the observer's error.
pub fn first_present(
    candidates: &[PathBuf],
    observe: &impl Fn(&Path) -> Result<bool, String>,
) -> Result<Option<PathBuf>, String> {
    if candidates.len() > MAX_ENTRIES
        || candidates
            .iter()
            .try_fold(0usize, |sum, path| sum.checked_add(path.as_os_str().len()))
            .is_none_or(|bytes| bytes > MAX_BYTES)
    {
        return Err("Make directory search exceeds candidate limit".into());
    }
    for candidate in candidates {
        if observe(candidate)? {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

fn directory_words(value: &str) -> Result<Vec<String>, String> {
    if value.len() > MAX_VALUE_BYTES {
        return Err("Make vpath directory list exceeds value byte limit".into());
    }
    let mut result = Vec::new();
    for word in value.split(|character: char| character.is_ascii_whitespace() || character == ':') {
        if word.is_empty() {
            continue;
        }
        validate_word(word)?;
        if word.contains('%') {
            return Err("Make vpath directory contains an unresolved percent pattern".into());
        }
        if result.len() == MAX_ENTRIES {
            return Err("Make vpath directory list exceeds count limit".into());
        }
        result.push(word.to_owned());
    }
    Ok(result)
}

fn validate_word(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_VALUE_BYTES
        || value.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || "$\\'\"#;|(){}".contains(character)
        })
    {
        return Err("Make vpath has a nonliteral or unsupported path word".into());
    }
    Ok(())
}

fn pattern_matches(pattern: &str, file: &str) -> bool {
    pattern
        .split_once('%')
        .map_or(pattern == file, |(prefix, suffix)| {
            file.strip_prefix(prefix)
                .is_some_and(|tail| tail.ends_with(suffix))
        })
}

fn append_candidate(
    paths: &mut Vec<PathBuf>,
    bytes: &mut usize,
    working_directory: &Path,
    directory: &str,
    file: &str,
) -> Result<(), String> {
    let candidate = working_directory.join(directory).join(file);
    *bytes = bytes
        .checked_add(candidate.as_os_str().len())
        .filter(|bytes| *bytes <= MAX_BYTES)
        .ok_or("Make vpath exceeds candidate byte limit")?;
    if paths.len() == MAX_ENTRIES {
        return Err("Make vpath exceeds candidate count limit".into());
    }
    paths.push(candidate);
    Ok(())
}

#[cfg(test)]
#[path = "make_vpath_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "make_vpath_captured_tests.rs"]
mod captured_tests;
