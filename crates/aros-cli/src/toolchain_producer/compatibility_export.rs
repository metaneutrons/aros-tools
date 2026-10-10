//! Durable local compatibility exports; never execution authentication.

use std::path::{Component, Path, PathBuf};

use aros_common::publication::{
    create_unique_directory_nofollow, publish_atomic_file, publish_flat_tree_noclobber,
    publish_prepared_tree_noclobber, validate_existing_directory_prefix_nofollow, AtomicFilePolicy,
    PortableOutputName,
};
use aros_common::Sha256Digest;
use aros_toolchain::compatibility::NativeCompatibilityExecutionExport;

const INPUTS_FILE: &str = "inputs.json";
const EVIDENCE_DIRECTORY: &str = "evidence";

/// Output identities selected by the caller, not by a downloaded report.
pub(super) struct PublishedCompatibilityExport {
    pub directory: PathBuf,
    pub inputs_sha256: Sha256Digest,
    pub manifest_sha256: Sha256Digest,
}

/// Nonmutating early validation. Publication rechecks the path independently.
pub(super) fn preflight(destination: &Path, protected: &[&Path]) -> miette::Result<()> {
    if !destination.is_absolute()
        || destination
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(miette::miette!(
            "compatibility --evidence-dir must be an absolute normalized path"
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("compatibility evidence output has no parent"))?;
    let leaf = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| miette::miette!("compatibility evidence output has no portable leaf"))?;
    PortableOutputName::new(leaf)
        .map_err(|_| miette::miette!("compatibility evidence output has no portable leaf"))?;
    validate_existing_directory_prefix_nofollow(parent).map_err(|_| {
        miette::miette!("compatibility evidence parent must have no symlink components")
    })?;
    if !parent.is_dir() {
        return Err(miette::miette!(
            "compatibility evidence parent must already exist"
        ));
    }
    match std::fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(miette::miette!(
                "compatibility evidence output already exists or is unavailable"
            ));
        }
    }
    let destination = resolve_input_path(destination)?;
    for path in protected {
        let path = resolve_input_path(path)?;
        if filesystem_overlap(&destination, &path)? {
            return Err(miette::miette!(
                "compatibility evidence output must be separate from every input and execution root"
            ));
        }
    }
    Ok(())
}

/// Existing ancestors are compared by identity, not caller spelling. This also
/// catches APFS case/Unicode aliases when canonicalize preserves a spelling.
/// Prospective suffixes are compared conservatively without ASCII case, since
/// an absent output leaf cannot yet supply a filesystem identity.
pub(super) fn filesystem_overlap(left: &Path, right: &Path) -> miette::Result<bool> {
    aros_common::publication::filesystem_paths_overlap(left, right)
        .map_err(|_| miette::miette!("cannot establish compatibility path ancestry"))
}

/// Resolve existing prefixes to catch aliases even when the final leaf is absent.
fn resolve_input_path(path: &Path) -> miette::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| miette::miette!("cannot resolve compatibility input path"))?
            .join(path)
    };
    let mut prefix = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match prefix.canonicalize() {
            Ok(mut resolved) => {
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    prefix.file_name().ok_or_else(|| {
                        miette::miette!("cannot resolve compatibility input path")
                    })?,
                );
                prefix = prefix
                    .parent()
                    .ok_or_else(|| miette::miette!("cannot resolve compatibility input path"))?;
            }
            Err(_) => return Err(miette::miette!("cannot resolve compatibility input path")),
        }
    }
}

pub(super) fn publish(
    destination: &Path,
    execution: &NativeCompatibilityExecutionExport,
) -> miette::Result<PublishedCompatibilityExport> {
    publish_files(
        destination,
        execution.inputs().bytes(),
        execution.evidence().files(),
    )?;
    Ok(PublishedCompatibilityExport {
        directory: destination.to_path_buf(),
        inputs_sha256: execution.inputs().sha256().clone(),
        manifest_sha256: execution.evidence().manifest_sha256().clone(),
    })
}

/// One no-clobber directory rename exposes inputs and complete evidence together.
/// Failure before rename retains only our uniquely named staging tree; it never
/// exposes a partial final export or removes another process's output.
fn publish_files<'a>(
    destination: &Path,
    inputs: &[u8],
    files: impl Iterator<Item = (&'a str, &'a [u8])>,
) -> miette::Result<()> {
    preflight(destination, &[])?;
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("compatibility evidence output has no parent"))?;
    let members = files
        .map(|(name, bytes)| {
            PortableOutputName::new(name)
                .map(|name| (name, bytes))
                .map_err(|_| miette::miette!("compatibility export contains an unsafe member name"))
        })
        .collect::<miette::Result<Vec<_>>>()?;
    let staging = create_unique_directory_nofollow(parent, ".aros-compatibility-export").map_err(
        |error| miette::miette!("cannot create owned compatibility export stage: {error}"),
    )?;
    publish_atomic_file(
        &staging.join(INPUTS_FILE),
        inputs,
        AtomicFilePolicy::NoClobber,
    )
    .map_err(|error| miette::miette!("cannot durably stage compatibility inputs: {error}"))?;
    publish_flat_tree_noclobber(&staging.join(EVIDENCE_DIRECTORY), &members).map_err(|error| {
        miette::miette!("cannot durably stage complete compatibility evidence: {error}")
    })?;
    publish_prepared_tree_noclobber(&staging, destination).map_err(|error| {
        miette::miette!("cannot publish complete compatibility export without clobbering: {error}")
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "compatibility_export_tests.rs"]
mod tests;
