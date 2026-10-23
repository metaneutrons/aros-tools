//! Bind explicit literal recipe fields to original captured file inputs/outputs.
//!
//! This does not infer compiler options, file roles, process cwd changes or
//! producer authority. The owning adapter selects exact fields and output scope.

use super::{program_binding::RecipeProgramBindings, RecipeRedirectionKind};
use aros_common::{
    make_input_snapshot::MakeInputReader, sha256_bytes, Sha256Digest, TreeContentEntryKind,
};
use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
};

#[cfg(all(test, unix))]
mod tests;

const MAX_REQUESTS: usize = 65_536;
const MAX_STORAGE: usize = 64 * 1024 * 1024;
type FieldKey = (usize, usize, usize, bool);
type OccupiedFields = BTreeMap<FieldKey, BTreeMap<usize, usize>>;
type SelectedField<'a> = (&'a str, FieldKey, Range<usize>);

/// An explicit literal path field, never a guessed option value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecipeArtifactSlot {
    /// UTF-8 byte range within a non-command argv word. The rest is untouched.
    Word { index: usize, range: Range<usize> },
    /// Complete file redirection target; descriptor duplication is not a file.
    Redirection(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeArtifactDirection {
    Input,
    Output,
}

/// Positions in the original recipe batch and its complete command enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipeArtifactRequest {
    pub recipe: usize,
    pub command: usize,
    pub slot: RecipeArtifactSlot,
    pub direction: RecipeArtifactDirection,
}

/// A selected field checked within its owning captured input transaction.
///
/// Output membership/absence is not permission to overwrite, proof of one
/// producer, or a completed file. Input content is descriptor-read; executable
/// mode is deliberately not required for source files or interpreter scripts.
pub struct BoundRecipeArtifact {
    request: RecipeArtifactRequest,
    path: PathBuf,
    previous_kind: Option<TreeContentEntryKind>,
    input_sha256: Option<Sha256Digest>,
}

impl std::fmt::Debug for BoundRecipeArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundRecipeArtifact")
            .field("request", &self.request)
            .field("previous_kind", &self.previous_kind)
            .finish_non_exhaustive()
    }
}

impl BoundRecipeArtifact {
    #[must_use]
    pub const fn request(&self) -> &RecipeArtifactRequest {
        &self.request
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub const fn previous_kind(&self) -> Option<TreeContentEntryKind> {
        self.previous_kind
    }
    #[must_use]
    pub const fn input_sha256(&self) -> Option<&Sha256Digest> {
        self.input_sha256.as_ref()
    }
}

/// Bind explicit fields using the reader supplied by the owning transaction.
///
/// The caller must independently bind `output_root` to the selected original
/// prepared-build root. This primitive checks physical membership, not that
/// the root is writable or every required field/producer was selected. It
/// returns no bytes, performs no write, and never reassembles compiler flags.
/// All repeated output paths remain ordered records, not a unique-owner claim.
///
/// # Errors
/// Refuses missing/overlapping slots, unsafe paths/kinds, absent input bytes,
/// unbound output scope, or bounded reader/result exhaustion. A later refusal
/// discards the complete batch; errors omit raw path/argv/content values.
pub(crate) fn bind_recipe_artifacts(
    bindings: &[RecipeProgramBindings<'_>],
    reader: &mut MakeInputReader<'_>,
    output_root: Option<&Path>,
    requests: &[RecipeArtifactRequest],
) -> Result<Vec<BoundRecipeArtifact>, String> {
    if requests.len() > MAX_REQUESTS {
        return Err("recipe artifact batch exceeds request limit".into());
    }
    let mut occupied = OccupiedFields::new();
    let mut storage = 0usize;
    let mut results = Vec::with_capacity(requests.len());
    for request in requests {
        let (identity, key, range) = select_field(bindings, request)?;
        storage = storage
            .checked_add(identity.len() + 256)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("recipe artifact batch exceeds storage limit")?;
        if identity.ends_with('/') || identity.ends_with("/.") {
            return Err("recipe artifact has a directory-only suffix".into());
        }
        let ranges = occupied.entry(key).or_default();
        if ranges
            .range(..=range.start)
            .next_back()
            .is_some_and(|(_, end)| *end > range.start)
            || ranges
                .range(range.start..)
                .next()
                .is_some_and(|(start, _)| *start < range.end)
        {
            return Err("recipe artifact fields overlap".into());
        }
        ranges.insert(range.start, range.end);
        let (path, previous_kind, input_sha256) = match request.direction {
            RecipeArtifactDirection::Input => {
                let input = reader
                    .read_bytes(identity)?
                    .ok_or("recipe artifact input is absent")?;
                let path = reader
                    .realpath(identity)?
                    .ok_or("recipe artifact input path is unavailable")?;
                (
                    path,
                    Some(TreeContentEntryKind::RegularFile),
                    Some(sha256_bytes(&input.bytes)),
                )
            }
            RecipeArtifactDirection::Output => {
                let root = output_root.ok_or("recipe artifact output scope is absent")?;
                let (path, kind) = bind_output(reader, identity, root)?;
                (path, kind, None)
            }
        };
        storage = storage
            .checked_add(path.as_os_str().len())
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("recipe artifact batch exceeds storage limit")?;
        results.push(BoundRecipeArtifact {
            request: request.clone(),
            path,
            previous_kind,
            input_sha256,
        });
    }
    Ok(results)
}

fn select_field<'a>(
    bindings: &'a [RecipeProgramBindings<'_>],
    request: &RecipeArtifactRequest,
) -> Result<SelectedField<'a>, String> {
    let command = bindings
        .get(request.recipe)
        .and_then(|recipe| recipe.commands().get(request.command))
        .ok_or("recipe artifact command is absent")?
        .command();
    match &request.slot {
        RecipeArtifactSlot::Word { index, range } => {
            if *index == 0 {
                return Err("recipe artifact cannot select the program word".into());
            }
            let word = command
                .words()
                .get(*index)
                .ok_or("recipe artifact word is absent")?;
            let identity = word
                .get(range.clone())
                .filter(|value| !value.is_empty())
                .ok_or("recipe artifact word range is invalid")?;
            Ok((
                identity,
                (request.recipe, request.command, *index, false),
                range.clone(),
            ))
        }
        RecipeArtifactSlot::Redirection(index) => {
            let redirection = command
                .redirections()
                .get(*index)
                .ok_or("recipe artifact redirection is absent")?;
            let compatible = matches!(
                (request.direction, redirection.kind()),
                (RecipeArtifactDirection::Input, RecipeRedirectionKind::Input)
                    | (
                        RecipeArtifactDirection::Output,
                        RecipeRedirectionKind::Output | RecipeRedirectionKind::Append
                    )
            );
            if !compatible {
                return Err("recipe artifact redirection has incompatible direction".into());
            }
            Ok((
                redirection.target(),
                (request.recipe, request.command, *index, true),
                0..redirection.target().len(),
            ))
        }
    }
}

fn bind_output(
    reader: &mut MakeInputReader<'_>,
    identity: &str,
    root: &Path,
) -> Result<(PathBuf, Option<TreeContentEntryKind>), String> {
    let nominal = Path::new(identity);
    let leaf = nominal
        .file_name()
        .ok_or("recipe artifact output leaf is absent")?;
    let parent = nominal
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_text = parent
        .to_str()
        .ok_or("recipe artifact output parent is not UTF-8")?;
    if reader.entry_kind(parent_text)? != Some(TreeContentEntryKind::Directory) {
        return Err("recipe artifact output parent is not a captured directory".into());
    }
    let parent = reader
        .realpath(parent_text)?
        .ok_or("recipe artifact output parent path is unavailable")?;
    let path = parent.join(leaf);
    if !path.starts_with(root) || path == root {
        return Err("recipe artifact output is outside selected scope".into());
    }
    let kind = reader.entry_kind(identity)?;
    if !matches!(kind, None | Some(TreeContentEntryKind::RegularFile)) {
        return Err("recipe artifact output is not absent or a regular file".into());
    }
    Ok((path, kind))
}
