//! Bounded data-only binding for caller-selected executable files.
//!
//! This module checks consistency between independently selected roots,
//! invocation paths, roles, and executable bytes. It does not authenticate an
//! owner, grant execution authority, or prove interpreter/runtime dependencies.

mod aliases;
pub use aliases::{BoundProgramAlias, ProgramAliasLink, ProgramAliasSelection};

#[cfg(unix)]
use crate::measure_regular_file_digest_bounded;
use crate::{
    measure_tree_content_cas_bounded, publication::filesystem_paths_overlap, Sha256Digest,
    TreeContentCas, TreeContentEntryKind, TreeTraversalLimits,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    path::{Component, Path, PathBuf},
};

const MAX_ROOTS: usize = 8;
const MAX_PROGRAMS: usize = 64;
const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PROGRAM_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 8 * 1024;
const MAX_ROLE_BYTES: usize = 128;
const MAX_AGGREGATE_INPUT_BYTES: usize = 512 * 1024;

/// Independently selected complete root and its expected portable content.
#[derive(Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramRootSelection {
    pub root: PathBuf,
    pub content_sha256: Sha256Digest,
}

impl fmt::Debug for ProgramRootSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProgramRootSelection")
            .field("root", &"<redacted>")
            .field("content_sha256", &"<redacted>")
            .finish()
    }
}

/// One caller-selected program role and exact invocation spelling.
#[derive(Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSelection {
    pub role: String,
    pub root_index: usize,
    pub invocation_path: PathBuf,
    pub executable_sha256: Sha256Digest,
}

/// Portable caller selections, not a captured filesystem or execution receipt.
///
/// Deserialization adds no authority. Consumers must independently bind roots
/// and roles, then capture/revalidate the actual selected trees before use.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSetSelection {
    pub roots: Vec<ProgramRootSelection>,
    pub programs: Vec<ProgramSelection>,
}

impl ProgramSetSelection {
    /// Validate bounded literal selection data without reading the filesystem.
    /// This does not prove that any selected tree or executable exists.
    /// # Errors
    /// Refuses invalid counts, paths, roles, root references or ambiguous data.
    pub fn validate(&self) -> Result<(), String> {
        validate_selections(&self.roots, &self.programs)
    }
}

impl fmt::Debug for ProgramSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProgramSelection")
            .field("role", &"<redacted>")
            .field("root_index", &self.root_index)
            .field("invocation_path", &"<redacted>")
            .field("executable_sha256", &"<redacted>")
            .finish()
    }
}

#[derive(Clone)]
struct CapturedRoot {
    path: PathBuf,
    content_sha256: Sha256Digest,
    tree: TreeContentCas,
}

impl fmt::Debug for CapturedRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedRoot")
            .field("path", &"<redacted>")
            .field("content_sha256", &"<redacted>")
            .field("entry_count", &self.tree.entry_count())
            .finish()
    }
}

/// Immutable root snapshots and the exact invocation identities they bind.
#[derive(Clone)]
pub struct CapturedProgramSet {
    roots: Vec<CapturedRoot>,
    programs: Vec<CapturedProgramIdentity>,
}

impl fmt::Debug for CapturedProgramSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedProgramSet")
            .field("root_count", &self.roots.len())
            .field("program_count", &self.programs.len())
            .finish()
    }
}

/// A verified role, invocation spelling, resolved file, and byte digest.
///
/// Fields are private so callers can only obtain identities from a successful
/// [`CapturedProgramSet::capture`] operation.
#[derive(Clone, Eq, PartialEq)]
pub struct CapturedProgramIdentity {
    role: String,
    root_index: usize,
    invocation_path: PathBuf,
    resolved_path: PathBuf,
    executable_sha256: Sha256Digest,
}

impl fmt::Debug for CapturedProgramIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedProgramIdentity")
            .field("role", &"<redacted>")
            .field("root_index", &self.root_index)
            .field("invocation_path", &"<redacted>")
            .field("resolved_path", &"<redacted>")
            .field("executable_sha256", &"<redacted>")
            .finish()
    }
}

impl CapturedProgramIdentity {
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub fn invocation_path(&self) -> &Path {
        &self.invocation_path
    }

    #[must_use]
    pub fn resolved_path(&self) -> &Path {
        &self.resolved_path
    }

    #[must_use]
    pub const fn executable_sha256(&self) -> &Sha256Digest {
        &self.executable_sha256
    }

    #[must_use]
    pub const fn root_index(&self) -> usize {
        self.root_index
    }
}

fn validate_selections(
    roots: &[ProgramRootSelection],
    programs: &[ProgramSelection],
) -> Result<(), String> {
    if roots.is_empty() || roots.len() > MAX_ROOTS || programs.len() > MAX_PROGRAMS {
        return Err("program root or selection count exceeds limits".into());
    }
    if input_size(roots, programs)? > MAX_AGGREGATE_INPUT_BYTES {
        return Err("program selection input exceeds limit".into());
    }
    for root in roots {
        validate_absolute_normal_path(&root.root, "program root")?;
    }
    for (index, selection) in programs.iter().enumerate() {
        validate_role(&selection.role)?;
        validate_absolute_normal_path(&selection.invocation_path, "program invocation")?;
        let root = roots
            .get(selection.root_index)
            .ok_or("program selection references an unknown root")?;
        if strict_child_path(&root.root, &selection.invocation_path).is_none() {
            return Err("program invocation is outside its selected root".into());
        }
        if programs[..index].iter().any(|previous| {
            previous.role == selection.role
                || previous.invocation_path.as_os_str() == selection.invocation_path.as_os_str()
        }) {
            return Err("program role or invocation spelling is ambiguous".into());
        }
    }
    Ok(())
}

impl CapturedProgramSet {
    /// Capture complete, disjoint roots and bind each exact executable spelling.
    ///
    /// The root content hashes and executable hashes are supplied independently
    /// by the caller. A successful result is a data consistency check only; the
    /// caller remains responsible for its existing role and root authorization.
    ///
    /// # Errors
    /// Refuses noncanonical or overlapping roots, changed or mismatched content,
    /// invalid selections, unsafe executable paths, and resource-limit excess.
    pub fn capture(
        roots: &[ProgramRootSelection],
        programs: &[ProgramSelection],
    ) -> Result<Self, String> {
        Self::capture_with(roots, programs, |_| Ok(())).map(|(captured, ())| captured)
    }

    /// Capture programs and derive additional data within the same two root scans.
    ///
    /// The callback runs after all selected files/aliases are checked and before
    /// final whole-root revalidation. It must not publish intermediate results.
    /// This adds no process-launch authority and does not silently skip checks.
    /// # Errors
    /// Refuses every ordinary capture error, callback failure or subsequent
    /// input drift before returning either the set or callback result.
    pub fn capture_with<T>(
        roots: &[ProgramRootSelection],
        programs: &[ProgramSelection],
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<(Self, T), String> {
        validate_selections(roots, programs)?;
        for selection in roots {
            require_canonical(&selection.root, "program root")?;
        }

        let mut captured_roots: Vec<CapturedRoot> = Vec::with_capacity(roots.len());
        let mut total_entries = 0usize;
        let mut total_tree_bytes = 0_u64;
        for selection in roots {
            for previous in &captured_roots {
                if filesystem_paths_overlap(&previous.path, &selection.root)
                    .map_err(|_| "cannot establish disjoint program roots")?
                {
                    return Err("program roots overlap or repeat".into());
                }
            }

            let remaining_entries = MAX_TREE_ENTRIES.saturating_sub(total_entries).max(1);
            let remaining_bytes = MAX_TREE_BYTES.saturating_sub(total_tree_bytes).max(1);
            let limits = TreeTraversalLimits::new(remaining_entries, remaining_bytes)
                .map_err(|_| "invalid captured program tree limits")?;
            let tree = measure_tree_content_cas_bounded(&selection.root, limits)
                .map_err(|_| "cannot capture complete program root")?;
            if tree.payload_digest_excluding(None) != selection.content_sha256 {
                return Err("program root differs from independently selected content".into());
            }
            total_entries = total_entries
                .checked_add(tree.entry_count())
                .ok_or("combined program root entry count exceeds limit")?;
            total_tree_bytes = total_tree_bytes
                .checked_add(
                    tree.regular_file_bytes()
                        .ok_or("combined program root byte count is invalid")?,
                )
                .ok_or("combined program root byte count exceeds limit")?;
            if total_entries > MAX_TREE_ENTRIES || total_tree_bytes > MAX_TREE_BYTES {
                return Err("combined program roots exceed traversal limits".into());
            }
            captured_roots.push(CapturedRoot {
                path: selection.root.clone(),
                content_sha256: selection.content_sha256.clone(),
                tree,
            });
        }

        let mut identities = Vec::with_capacity(programs.len());
        let mut measured_programs = BTreeMap::new();
        for selection in programs {
            let root = &captured_roots[selection.root_index];
            let resolved = resolve_in_root(root, &selection.invocation_path)?;
            verify_program_once(
                root,
                &resolved,
                &selection.executable_sha256,
                &mut measured_programs,
            )?;
            if resolve_in_root(root, &selection.invocation_path)? != resolved {
                return Err("program invocation alias changed during capture".into());
            }
            identities.push(CapturedProgramIdentity {
                role: selection.role.clone(),
                root_index: selection.root_index,
                invocation_path: selection.invocation_path.clone(),
                resolved_path: resolved,
                executable_sha256: selection.executable_sha256.clone(),
            });
        }

        let captured = Self {
            roots: captured_roots,
            programs: identities,
        };
        let result = operation(&captured);
        captured.verify()?;
        Ok((captured, result?))
    }

    /// Recheck every captured root and executable alias against its snapshot.
    ///
    /// # Errors
    /// Refuses changed roots, alias resolution, file identity/content/mode or
    /// exceeded traversal limits without exposing selected values.
    pub fn verify(&self) -> Result<(), String> {
        let mut total_entries = 0usize;
        let mut total_tree_bytes = 0_u64;
        for root in &self.roots {
            require_canonical(&root.path, "program root")?;
            let remaining_entries = MAX_TREE_ENTRIES.saturating_sub(total_entries).max(1);
            let remaining_bytes = MAX_TREE_BYTES.saturating_sub(total_tree_bytes).max(1);
            let current = measure_tree_content_cas_bounded(
                &root.path,
                TreeTraversalLimits::new(remaining_entries, remaining_bytes)
                    .map_err(|_| "invalid captured program tree limits")?,
            )
            .map_err(|_| "cannot revalidate captured program root")?;
            if current != root.tree || current.payload_digest_excluding(None) != root.content_sha256
            {
                return Err("captured program root changed".into());
            }
            total_entries = total_entries
                .checked_add(current.entry_count())
                .ok_or("combined program root entry count exceeds limit")?;
            total_tree_bytes = total_tree_bytes
                .checked_add(
                    current
                        .regular_file_bytes()
                        .ok_or("combined program root byte count is invalid")?,
                )
                .ok_or("combined program root byte count exceeds limit")?;
            if total_entries > MAX_TREE_ENTRIES || total_tree_bytes > MAX_TREE_BYTES {
                return Err("combined program roots exceed traversal limits".into());
            }
        }

        let mut measured_programs = BTreeMap::new();
        for identity in &self.programs {
            let root = self
                .roots
                .get(identity.root_index)
                .ok_or("captured program root binding is invalid")?;
            let resolved = resolve_in_root(root, &identity.invocation_path)?;
            if resolved != identity.resolved_path {
                return Err("captured program invocation alias changed".into());
            }
            verify_program_once(
                root,
                &resolved,
                &identity.executable_sha256,
                &mut measured_programs,
            )?;
            if resolve_in_root(root, &identity.invocation_path)? != identity.resolved_path {
                return Err("captured program invocation alias changed".into());
            }
        }
        Ok(())
    }

    /// Run one operation only while the complete captured set remains stable.
    ///
    /// The callback's value is returned only after post-operation revalidation.
    /// Callbacks that publish results should keep those results private until
    /// this method returns successfully.
    ///
    /// # Errors
    /// Propagates callback failure and refuses any before/after input drift.
    pub fn transaction<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.verify()?;
        let result = operation(self);
        self.verify()?;
        result
    }

    /// Find a program by the exact original absolute invocation spelling.
    #[must_use]
    pub fn identity_for_word(&self, word: &str) -> Option<&CapturedProgramIdentity> {
        self.programs
            .iter()
            .find(|identity| identity.invocation_path.as_os_str() == std::ffi::OsStr::new(word))
    }

    /// Find a program by its exact caller-selected role.
    #[must_use]
    pub fn identity_for_role(&self, role: &str) -> Option<&CapturedProgramIdentity> {
        self.programs.iter().find(|identity| identity.role == role)
    }

    /// Match one exact captured root spelling and its independently selected digest.
    #[must_use]
    pub fn matches_root_selection(&self, root: &Path, content_sha256: &Sha256Digest) -> bool {
        self.roots.iter().any(|captured| {
            captured.path.as_os_str() == root.as_os_str()
                && captured.content_sha256 == *content_sha256
        })
    }
}

// Aliases may share one large executable. Hash each canonical leaf once per
// capture/verification pass, without merging the original invocation roles.
// Expected digests still agree independently for every alias; complete root
// snapshots and per-alias resolution checks remain in force.
fn verify_program_once(
    root: &CapturedRoot,
    resolved: &Path,
    expected: &Sha256Digest,
    measured: &mut BTreeMap<PathBuf, Sha256Digest>,
) -> Result<(), String> {
    if let Some(previous) = measured.get(resolved) {
        if previous != expected {
            return Err("selected program aliases disagree on executable content".into());
        }
        return Ok(());
    }
    verify_program_file(root, resolved, expected)?;
    measured.insert(resolved.to_owned(), expected.clone());
    Ok(())
}

fn input_size(
    roots: &[ProgramRootSelection],
    programs: &[ProgramSelection],
) -> Result<usize, String> {
    roots
        .iter()
        .try_fold(0usize, |total, selection| {
            total
                .checked_add(selection.root.as_os_str().len())?
                .checked_add(selection.content_sha256.as_str().len())
        })
        .and_then(|total| {
            programs.iter().try_fold(total, |total, selection| {
                total
                    .checked_add(selection.role.len())?
                    .checked_add(selection.invocation_path.as_os_str().len())?
                    .checked_add(std::mem::size_of::<usize>())?
                    .checked_add(selection.executable_sha256.as_str().len())
            })
        })
        .ok_or_else(|| "program selection input size overflows".into())
}

fn validate_role(role: &str) -> Result<(), String> {
    if role.is_empty()
        || role.len() > MAX_ROLE_BYTES
        || role == "."
        || role == ".."
        || role.chars().any(char::is_control)
    {
        return Err("program role is invalid or exceeds its limit".into());
    }
    Ok(())
}

fn validate_absolute_normal_path(path: &Path, label: &str) -> Result<(), String> {
    let Some(text) = path.to_str() else {
        return Err(format!("{label} path is not UTF-8"));
    };
    if !path.is_absolute()
        || text.len() > MAX_PATH_BYTES
        || text.chars().any(char::is_control)
        || (text != "/"
            && (!text.starts_with('/')
                || text.ends_with('/')
                || text[1..]
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")))
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!("{label} path is not a normalized absolute path"));
    }
    Ok(())
}

fn require_canonical(path: &Path, label: &str) -> Result<(), String> {
    if path
        .canonicalize()
        .map_err(|_| format!("cannot canonicalize {label}"))?
        != path
    {
        return Err(format!("{label} path is not canonical"));
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|_| format!("cannot inspect {label}"))?
        .file_type()
        .is_dir()
    {
        return Err(format!("{label} is not a directory"));
    }
    Ok(())
}

fn strict_child_path(root: &Path, candidate: &Path) -> Option<String> {
    let relative = candidate.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    relative.to_str().map(str::to_owned)
}

fn resolve_in_root(root: &CapturedRoot, invocation: &Path) -> Result<PathBuf, String> {
    // Resolve each link against captured in-root entries. A final canonical
    // path alone would admit a dependency on an external link that re-enters
    // this root. Neither an intermediate outside hop nor a new ambient path
    // may supply part of the selected executable's resolution.
    let relative = invocation
        .strip_prefix(&root.path)
        .map_err(|_| "program invocation is outside its selected root")?;
    let mut pending: VecDeque<_> = relative
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    let mut resolved = root.path.clone();
    let mut links = 0usize;
    let mut steps = 0usize;
    while let Some(part) = pending.pop_front() {
        steps += 1;
        if steps > 16_384 {
            return Err("program alias resolution exceeds step limit".into());
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            if resolved == root.path || !resolved.pop() {
                return Err("program alias leaves its selected root".into());
            }
            continue;
        }
        let candidate = resolved.join(part);
        let relative = strict_child_path(&root.path, &candidate)
            .ok_or("program alias leaves its selected root")?;
        match root.tree.entry_kind(&relative) {
            Some(TreeContentEntryKind::Directory) => resolved = candidate,
            Some(TreeContentEntryKind::RegularFile) if pending.is_empty() => {
                if candidate
                    .canonicalize()
                    .map_err(|_| "cannot resolve captured program file")?
                    != candidate
                {
                    return Err("captured program file path changed".into());
                }
                return Ok(candidate);
            }
            Some(TreeContentEntryKind::SymbolicLink) => {
                links += 1;
                if links > 32 {
                    return Err("program alias resolution exceeds link limit".into());
                }
                let target = std::fs::read_link(&candidate)
                    .map_err(|_| "cannot read captured program alias")?;
                if target.as_os_str().len() > MAX_PATH_BYTES {
                    return Err("program alias target exceeds path limit".into());
                }
                let target = if target.is_absolute() {
                    let target = target
                        .strip_prefix(&root.path)
                        .map_err(|_| "program alias leaves its selected root")?
                        .to_owned();
                    resolved.clone_from(&root.path);
                    target
                } else {
                    target
                };
                for component in target.components().rev() {
                    pending.push_front(component.as_os_str().to_owned());
                }
            }
            _ => return Err("program alias is not a captured file path".into()),
        }
    }
    Err("resolved program is not a captured regular file".into())
}

#[cfg(unix)]
fn verify_program_file(
    root: &CapturedRoot,
    resolved: &Path,
    expected_digest: &Sha256Digest,
) -> Result<(), String> {
    let relative = strict_child_path(&root.path, resolved)
        .ok_or("resolved program path is not a strict root child")?;
    let metadata =
        std::fs::symlink_metadata(resolved).map_err(|_| "cannot inspect resolved program file")?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("resolved program is not an executable regular file".into());
    }
    let mode = metadata.permissions().mode() & 0o7777;
    let (identity, measured) = measure_regular_file_digest_bounded(resolved, MAX_PROGRAM_BYTES)
        .map_err(|_| "cannot measure resolved program file")?;
    if measured.digest != *expected_digest
        || measured.size > MAX_PROGRAM_BYTES
        || !root
            .tree
            .matches_regular_file(&relative, mode, expected_digest)
        || !root
            .tree
            .matches_regular_file_identity(&relative, identity, expected_digest)
    {
        return Err("resolved program differs from independently selected content".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_program_file(
    _root: &CapturedRoot,
    _resolved: &Path,
    _expected_digest: &Sha256Digest,
) -> Result<(), String> {
    Err("captured program validation requires no-follow file support".into())
}
