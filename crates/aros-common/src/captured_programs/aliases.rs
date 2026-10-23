//! Explicit cross-root invocation links, separate from ordinary in-root capture.

use super::{CapturedProgramIdentity, CapturedProgramSet};
use crate::TreeContentEntryKind;
use std::{
    collections::VecDeque,
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

const MAX_LINKS: usize = 32;
const MAX_STEPS: usize = 16_384;

/// One exact retained link, including its raw target spelling.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramAliasLink {
    pub path: PathBuf,
    pub target: Vec<u8>,
}

impl fmt::Debug for ProgramAliasLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgramAliasLink").finish_non_exhaustive()
    }
}

/// Independently selected invocation, ordered links and existing program role.
///
/// This is selection data only. Roots and canonical roles must already belong
/// to the captured set; no expected digest or role is inferred from a link.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramAliasSelection {
    pub alias_root_index: usize,
    pub invocation_path: PathBuf,
    pub role: String,
    pub links: Vec<ProgramAliasLink>,
}

impl fmt::Debug for ProgramAliasSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgramAliasSelection")
            .field("link_count", &self.links.len())
            .finish_non_exhaustive()
    }
}

/// A consistency join preserving both original invocation and canonical role.
///
/// It borrows the original captured roots. This is not process-launch authority,
/// interpreter/runtime authentication or a reason to replace argv[0]. Callers
/// must revalidate before using the binding in a later owning transaction.
pub struct BoundProgramAlias<'a> {
    programs: &'a CapturedProgramSet,
    selection: ProgramAliasSelection,
    program: &'a CapturedProgramIdentity,
}

impl fmt::Debug for BoundProgramAlias<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundProgramAlias")
            .field("link_count", &self.selection.links.len())
            .finish_non_exhaustive()
    }
}

impl BoundProgramAlias<'_> {
    #[must_use]
    pub fn invocation_path(&self) -> &Path {
        &self.selection.invocation_path
    }

    #[must_use]
    pub const fn program(&self) -> &CapturedProgramIdentity {
        self.program
    }

    /// Recheck the same selected links and complete original roots.
    /// # Errors
    /// Refuses root, role, link-target or file-identity drift.
    pub fn verify(&self) -> Result<(), String> {
        self.programs
            .transaction(|set| verify_alias(set, &self.selection))
    }
}

impl CapturedProgramSet {
    /// Bind an explicit alias chain from one captured root into a selected role root.
    ///
    /// Ordinary program capture remains root-confined. This separate operation
    /// permits exactly one absolute cross-root transition, from the selected
    /// alias root to the independently selected role's root. Other links may
    /// stay within those roots. Relative escapes, third roots and re-entry are
    /// refused; no ambient canonicalization supplies missing parent evidence.
    /// Every encountered link must match the caller's complete ordered list,
    /// including raw target bytes. The final captured regular file must be the
    /// role's resolved file, not merely another file with equal bytes.
    ///
    /// # Errors
    /// Refuses invalid/bounded selections, absent roles, unselected links or
    /// boundaries, incomplete/cyclic chains, wrong endpoints and root drift.
    pub fn bind_alias(
        &self,
        selection: &ProgramAliasSelection,
    ) -> Result<BoundProgramAlias<'_>, String> {
        validate_selection(self, selection)?;
        self.transaction(|set| verify_alias(set, selection))?;
        let program = self
            .identity_for_role(&selection.role)
            .ok_or("selected alias role is absent")?;
        Ok(BoundProgramAlias {
            programs: self,
            selection: selection.clone(),
            program,
        })
    }
}

fn validate_selection(
    set: &CapturedProgramSet,
    selection: &ProgramAliasSelection,
) -> Result<(), String> {
    super::validate_role(&selection.role)?;
    super::validate_absolute_normal_path(&selection.invocation_path, "alias invocation")?;
    let root = set
        .roots
        .get(selection.alias_root_index)
        .ok_or("selected alias root is absent")?;
    if super::strict_child_path(&root.path, &selection.invocation_path).is_none()
        || selection.links.is_empty()
        || selection.links.len() > MAX_LINKS
    {
        return Err("alias invocation or link count is invalid".into());
    }
    let program = set
        .identity_for_role(&selection.role)
        .ok_or("selected alias role is absent")?;
    if program.root_index() == selection.alias_root_index {
        return Err("cross-root alias requires distinct selected roots".into());
    }
    let mut bytes = selection.invocation_path.as_os_str().len() + selection.role.len();
    for link in &selection.links {
        super::validate_absolute_normal_path(&link.path, "alias link")?;
        let target = std::str::from_utf8(&link.target).map_err(|_| "alias target is not UTF-8")?;
        if target.is_empty()
            || target.starts_with("//")
            || target.len() > super::MAX_PATH_BYTES
            || target.chars().any(char::is_control)
        {
            return Err("alias target is invalid or exceeds limit".into());
        }
        bytes = bytes
            .checked_add(link.path.as_os_str().len())
            .and_then(|size| size.checked_add(target.len()))
            .filter(|size| *size <= super::MAX_AGGREGATE_INPUT_BYTES)
            .ok_or("alias selection exceeds storage limit")?;
    }
    Ok(())
}

fn verify_alias(set: &CapturedProgramSet, selection: &ProgramAliasSelection) -> Result<(), String> {
    // The public descriptor may be arbitrary; validation also precedes every
    // revalidation, not only initial construction of a borrowed binding.
    validate_selection(set, selection)?;
    let program = set
        .identity_for_role(&selection.role)
        .ok_or("selected alias role is absent")?;
    let mut root_index = selection.alias_root_index;
    let mut resolved = set.roots[root_index].path.clone();
    let relative = selection
        .invocation_path
        .strip_prefix(&resolved)
        .map_err(|_| "alias invocation differs from selected root")?;
    let mut pending: VecDeque<_> = relative
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let mut links = 0usize;
    let mut steps = 0usize;
    let mut crossed = false;
    while let Some(part) = pending.pop_front() {
        steps += 1;
        if steps > MAX_STEPS {
            return Err("alias traversal exceeds step limit".into());
        }
        if part == "." {
            continue;
        }
        let root = &set.roots[root_index];
        if part == ".." {
            if resolved == root.path || !resolved.pop() {
                return Err("relative alias leaves its captured root".into());
            }
            continue;
        }
        let candidate = resolved.join(part);
        let relative = super::strict_child_path(&root.path, &candidate)
            .ok_or("alias traversal leaves its selected root")?;
        match root.tree.entry_kind(&relative) {
            Some(TreeContentEntryKind::Directory) => resolved = candidate,
            Some(TreeContentEntryKind::RegularFile) if pending.is_empty() => {
                if !crossed
                    || links != selection.links.len()
                    || candidate != program.resolved_path()
                {
                    return Err("alias endpoint or complete link selection differs".into());
                }
                return Ok(());
            }
            Some(TreeContentEntryKind::SymbolicLink) => {
                let selected = selection
                    .links
                    .get(links)
                    .ok_or("alias link selection is incomplete")?;
                if selected.path != candidate
                    || root.tree.symbolic_link_target(&relative) != Some(selected.target.as_slice())
                {
                    return Err("alias link differs from exact selected target".into());
                }
                links += 1;
                let target = Path::new(
                    std::str::from_utf8(&selected.target)
                        .map_err(|_| "alias target is not UTF-8")?,
                );
                let target = if target.is_absolute() {
                    let destination = if target.starts_with(&set.roots[program.root_index()].path) {
                        program.root_index()
                    } else if target.starts_with(&set.roots[selection.alias_root_index].path) {
                        selection.alias_root_index
                    } else {
                        return Err("absolute alias uses an unselected root".into());
                    };
                    if destination != root_index {
                        if crossed
                            || root_index != selection.alias_root_index
                            || destination != program.root_index()
                        {
                            return Err("alias uses an unselected root transition".into());
                        }
                        crossed = true;
                    }
                    root_index = destination;
                    resolved.clone_from(&set.roots[root_index].path);
                    target
                        .strip_prefix(&resolved)
                        .map_err(|_| "alias target root differs")?
                        .to_owned()
                } else {
                    target.to_owned()
                };
                // Path::components removes a terminal slash or '/.'. Keep its
                // directory requirement: a regular-file endpoint must not
                // become valid merely because that spelling was normalized.
                let raw = std::str::from_utf8(&selected.target)
                    .map_err(|_| "alias target is not UTF-8")?;
                if raw.ends_with('/') || raw.ends_with("/.") {
                    pending.push_front(OsString::from("."));
                }
                for component in target.components().rev() {
                    if !matches!(
                        component,
                        Component::Normal(_) | Component::CurDir | Component::ParentDir
                    ) {
                        return Err("alias target has unsupported components".into());
                    }
                    pending.push_front(component.as_os_str().to_owned());
                }
            }
            _ => return Err("alias is not a captured executable path".into()),
        }
    }
    Err("alias traversal has no regular-file endpoint".into())
}

#[cfg(all(test, unix))]
#[path = "aliases_tests.rs"]
mod tests;
