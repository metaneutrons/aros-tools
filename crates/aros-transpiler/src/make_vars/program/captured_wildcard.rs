//! GNU-style component globs over captured directory data, never host globbing.

use super::captured_glob::ComponentPattern;
use aros_common::{make_input_snapshot::MakeInputReader, TreeContentEntryKind};
use std::cell::Cell;

const MAX_PATTERN: usize = 8192;
const MAX_MATCHES: usize = 65_536;
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn query(
    reader: &mut MakeInputReader<'_>,
    pattern: &str,
    work: &Cell<usize>,
) -> Result<Vec<String>, String> {
    if pattern.is_empty()
        || pattern.len() > MAX_PATTERN
        || pattern.chars().any(char::is_control)
        // Callers supply fully expanded pathname data. A dollar in that data
        // is a literal filename character, not a Make reference to re-expand.
        || pattern.contains(['\\', '~', '\u{e000}'])
    {
        return Err("unsupported captured Make wildcard spelling".into());
    }
    let parts = pattern.split('/').collect::<Vec<_>>();
    let Some(first_pattern) = parts.iter().position(|part| part.contains(['*', '?', '['])) else {
        // GNU Make's literal wildcard fast path removes a trailing slash from
        // an existing regular file, unlike its patterned directory lookup.
        // Native file reading itself does NOT have this wildcard behavior.
        if pattern.ends_with('/') {
            let without_slashes = pattern.trim_end_matches('/');
            if !without_slashes.is_empty()
                && reader.wildcard_entry_kind(without_slashes)?
                    == Some(TreeContentEntryKind::RegularFile)
            {
                return Ok(vec![without_slashes.into()]);
            }
        }
        return match reader.wildcard_entry_kind(pattern)? {
            Some(TreeContentEntryKind::SymbolicLink) => {
                Err("captured Make wildcard cannot follow a symbolic link".into())
            }
            Some(kind)
                if !(pattern.ends_with('/') || pattern.ends_with("/."))
                    || kind == TreeContentEntryKind::Directory =>
            {
                Ok(vec![pattern.into()])
            }
            _ => Ok(Vec::new()),
        };
    };
    let mut current = vec![parts[..first_pattern].join("/")];
    if first_pattern == 1 && pattern.starts_with('/') {
        current[0] = "/".into();
    }
    for part in &parts[first_pattern..] {
        let mut next = Vec::new();
        let mut bytes = 0usize;
        for prefix in current {
            let directory = if prefix.is_empty() { "." } else { &prefix };
            match reader.wildcard_entry_kind(directory)? {
                Some(TreeContentEntryKind::SymbolicLink) => {
                    return Err("captured Make wildcard cannot traverse a symbolic link".into());
                }
                Some(TreeContentEntryKind::Directory) => {}
                _ => continue,
            }
            if part.is_empty() || *part == "." || *part == ".." {
                let identity = append(&prefix, part);
                if reader.wildcard_entry_kind(&identity)? == Some(TreeContentEntryKind::Directory) {
                    retain(&mut next, &mut bytes, identity)?;
                }
                continue;
            }
            let matcher = ComponentPattern::parse(part, work)?;
            for special in [".", ".."] {
                if part.starts_with('.') && matcher.matches(special, work)? {
                    let identity = append(&prefix, special);
                    if reader.wildcard_entry_kind(&identity)?
                        == Some(TreeContentEntryKind::Directory)
                    {
                        retain(&mut next, &mut bytes, identity)?;
                    }
                }
            }
            for (name, kind) in reader.directory_entries(directory)?.unwrap_or_default() {
                if name.starts_with('.') && !part.starts_with('.') {
                    continue;
                }
                if matcher.matches(&name, work)? {
                    if kind == TreeContentEntryKind::SymbolicLink {
                        return Err("captured Make wildcard cannot follow a symbolic link".into());
                    }
                    retain(&mut next, &mut bytes, append(&prefix, &name))?;
                }
            }
        }
        current = next;
    }
    current.sort();
    Ok(current)
}

fn append(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.into()
    } else if prefix == "/" {
        format!("/{name}")
    } else {
        format!("{prefix}/{name}")
    }
}

fn retain(values: &mut Vec<String>, bytes: &mut usize, value: String) -> Result<(), String> {
    *bytes = bytes
        .checked_add(value.len())
        .filter(|bytes| *bytes <= MAX_BYTES)
        .ok_or("captured Make wildcard result exceeds byte limit")?;
    if value.len() > MAX_PATTERN || values.len() >= MAX_MATCHES {
        return Err("captured Make wildcard result exceeds path/count limit".into());
    }
    values.push(value);
    Ok(())
}
