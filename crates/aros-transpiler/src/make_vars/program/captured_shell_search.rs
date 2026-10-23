//! Preserve an independently bound search suffix under Source path prepends.
//!
//! No path is trusted by its name. Every added directory and every earlier
//! external-command candidate is checked in the same captured input transaction.
//! Unknown roots, links and ambiguous aliases refuse rather than fall back.

use aros_common::{make_input_snapshot::MakeInputReader, TreeContentEntryKind};
use std::path::{Component, Path, PathBuf};

const MAX_DIRECTORIES: usize = 256;
const MAX_PATH_BYTES: usize = 128 * 1024;

/// Caller-owned, revalidatable inherited command search.
///
/// Implementations retain the exact directory and executable identities, reject absent roles
/// and drift, and never execute commands or infer ambient fallbacks.
/// This is an input capability, not utility semantics or launch authority.
pub trait CapturedShellSearch: Send + Sync {
    /// Exact single inherited search directory, not an ambient path list.
    fn path(&self) -> &Path;
    /// Revalidate the complete inherited search directory and selected tools.
    /// # Errors
    /// Refuses changed directories, aliases or executable identities.
    fn revalidate(&self) -> Result<(), String>;
    /// Check one explicitly selected bare command role in the retained binding.
    /// The caller revalidates the complete capability before and after its
    /// staged transaction; this check must not repeat a complete tree scan.
    /// # Errors
    /// Refuses missing roles or any changed search/tool identity.
    fn validate_command(&self, command: &str) -> Result<(), String>;
}

/// Check selected bare commands under the original Source-prepended PATH.
///
/// Reuses the captured prefix/candidate checks; no ambient search, utility
/// execution or program binding is performed. The owning transaction must
/// revalidate this complete capability before and after the reader work,
/// and retain its input reads before using the result for a later launch.
/// The prefix guard is not overridable by the supplied search capability.
/// File-read manifests alone do not retain directory-query results: repeat
/// this search check inside the owning reader after any output transition
/// that could alter earlier command candidates. No query replay is implied.
/// # Errors
/// Refuses missing roles, changed suffixes, unsafe directories, shadows or
/// input/query limits. Every earlier entry refuses, even if non-executable.
pub fn validate_captured_source_commands(
    search: &dyn CapturedShellSearch,
    current: &str,
    commands: &[&str],
    reader: &mut MakeInputReader<'_>,
) -> Result<(), String> {
    let inherited = search
        .path()
        .to_str()
        .filter(|path| !path.is_empty())
        .ok_or("captured shell search directory is not text")?;
    if current.len() > MAX_PATH_BYTES || commands.len() > 64 {
        return Err("captured shell search exceeds its query bound".into());
    }
    for command in commands {
        if command.is_empty() || command.contains(['/', '\0', ':']) {
            return Err("captured shell search command is not a bare name".into());
        }
        search.validate_command(command)?;
    }
    if current == inherited {
        return Ok(());
    }
    validate_prefixes(Some(inherited), Some(current), commands, reader)
}

pub(super) fn validate_prefixes(
    inherited: Option<&str>,
    current: Option<&str>,
    commands: &[&str],
    reader: &mut MakeInputReader<'_>,
) -> Result<(), String> {
    let inherited = inherited
        .filter(|path| !path.is_empty())
        .ok_or("captured shell search lacks an inherited PATH")?;
    let current = current
        .filter(|path| path.len() <= MAX_PATH_BYTES)
        .ok_or("captured shell search PATH is absent or exceeds its bound")?;
    let suffix = format!(":{inherited}");
    let prefix = current
        .strip_suffix(&suffix)
        .filter(|prefix| !prefix.is_empty())
        .ok_or("captured shell search must preserve the exact inherited PATH suffix")?;
    if prefix.split(':').count() > MAX_DIRECTORIES || commands.len() > 64 {
        return Err("captured shell search exceeds its query bound".into());
    }
    for directory in prefix.split(':') {
        let path = Path::new(directory);
        let normalized = path.components().collect::<PathBuf>();
        if !path.is_absolute()
            || normalized.as_os_str() != path.as_os_str()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err("captured shell search has an empty, relative or aliased directory".into());
        }
        match reader.wildcard_entry_kind(directory)? {
            None | Some(TreeContentEntryKind::Directory) => {}
            Some(_) => {
                return Err(
                    "captured shell search directory is not a captured real directory".into(),
                )
            }
        }
        for command in commands {
            if command.is_empty() || command.contains(['/', '\0', ':']) {
                return Err("captured shell search command is not a bare name".into());
            }
            let candidate = format!("{directory}/{command}");
            if reader.wildcard_entry_kind(&candidate)?.is_some() {
                // Refuse all earlier entries, including non-executable files:
                // this avoids guessing access, format or shell fallback rules.
                return Err(
                    "captured shell search has an earlier external-command candidate".into(),
                );
            }
        }
    }
    Ok(())
}
