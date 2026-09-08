//! Materialization of the engine-free source input for native compatibility.
//!
//! The native compatibility runner deliberately refuses a source tree with a
//! top-level `cmake/` directory: it always uses the engine embedded in the
//! selected `aros-tools` executable.  This module creates that probe input
//! from the raw, recursively audited Git material of a clean AROS checkout.
//! It never copies an untracked build tree, runs a source-owned program, or
//! follows source-tree symbolic links.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::{measure_tree_content_cas, CancellationToken, Sha256Digest};

use crate::filesystem::open_directory;
use crate::inspection::{self, Checkout};
use crate::package::validate_link_target;
use crate::recipe::GitObjectId;
use crate::source_audit::{self, Budget};
use crate::ContractError;

const MATERIALIZATION_TIMEOUT: Duration = Duration::from_mins(5);

/// Inputs for one non-overwriting engine-free compatibility source snapshot.
#[derive(Debug, Clone)]
pub struct EngineFreeSourceRequest {
    /// Clean recursively audited AROS checkout selected by the producer recipe.
    pub source_root: PathBuf,
    /// Absent output root for the source-owned (but engine-free) probe input.
    pub output_root: PathBuf,
}

/// Measured identity of one newly materialized compatibility source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFreeSourceOutput {
    /// Canonical fresh output root.
    pub root: PathBuf,
    /// Selected clean source commit.
    pub source_commit: String,
    /// Selected clean source tree object.
    pub source_tree: String,
    /// Content-only digest of the materialized engine-free source tree.
    pub source_tree_sha256: Sha256Digest,
}

/// Materialize a clean committed AROS source tree without its top-level CMake engine.
///
/// The input checkout is recursively audited against raw Git blobs, including
/// every initialized submodule.  Only those audited bytes are copied; the
/// caller's worktree files are never read.  The top-level `cmake/` directory
/// must be a committed directory and is omitted completely, leaving the
/// embedded `aros-tools` engine as the only CMake engine available to the
/// later compatibility runner.  The fresh destination is private and is never
/// overwritten or adopted; a failed materialization is retained for diagnosis.
///
/// # Errors
///
/// Returns a typed contract error for a dirty or unsafe checkout, a source
/// tree without a top-level CMake directory, an existing/unsafe destination,
/// or an I/O failure.  It has no network, cache, compiler, package, tag or
/// publication authority.
pub fn materialize_engine_free_source(
    request: &EngineFreeSourceRequest,
) -> Result<EngineFreeSourceOutput, ContractError> {
    let deadline = Instant::now()
        .checked_add(MATERIALIZATION_TIMEOUT)
        .ok_or_else(|| {
            ContractError::preflight("engine-free source deadline is not representable")
        })?;
    let source_root = inspection::directory(&request.source_root)?;
    let (commit, tree) = observed_identity(&source_root, deadline)?;
    let checkout = Checkout::inspect(&source_root, (&commit, &tree), deadline)?;
    let output_root = create_private_output_root(&request.output_root)?;
    let mut copied = SourceCopy::new(&output_root);
    let mut budget = Budget::new(deadline);
    source_audit::visit(&checkout, &mut budget, 0, "", &mut |path, entry, bytes| {
        copied.copy(path, entry.mode, bytes)
    })?;
    if !copied.saw_engine_directory {
        return Err(ContractError::compatibility(
            "selected compatibility source checkout has no committed top-level cmake directory",
        ));
    }
    if fs::symlink_metadata(output_root.join("cmake")).is_ok() {
        return Err(ContractError::compatibility(
            "engine-free compatibility source materialization retained a top-level cmake entry",
        ));
    }
    let source_tree_sha256 = measure_tree_content_cas(&output_root)
        .map_err(|_| {
            ContractError::compatibility("cannot measure engine-free compatibility source material")
        })?
        .payload_digest_excluding(None);
    Ok(EngineFreeSourceOutput {
        root: output_root,
        source_commit: commit.as_str().to_owned(),
        source_tree: tree.as_str().to_owned(),
        source_tree_sha256,
    })
}

fn observed_identity(
    root: &Path,
    deadline: Instant,
) -> Result<(GitObjectId, GitObjectId), ContractError> {
    let cancellation = CancellationToken::default();
    let bytes = inspection::git(
        root,
        &["rev-parse", "--show-toplevel", "HEAD^0", "HEAD:"],
        &[],
        4096,
        deadline,
        &cancellation,
    )?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ContractError::identity("Git identity response is not UTF-8"))?;
    let fields: Vec<_> = text.split_terminator('\n').collect();
    let [top, commit, tree] = fields.as_slice() else {
        return Err(ContractError::identity(
            "Git identity response must contain checkout, commit and tree",
        ));
    };
    if inspection::directory(Path::new(top))? != root {
        return Err(ContractError::identity(
            "selected compatibility source is not its Git checkout root",
        ));
    }
    Ok((
        GitObjectId::try_from((*commit).to_owned()).map_err(ContractError::identity)?,
        GitObjectId::try_from((*tree).to_owned()).map_err(ContractError::identity)?,
    ))
}

fn create_private_output_root(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(
            "engine-free compatibility source output must be absolute",
        ));
    }
    let leaf = path.file_name().ok_or_else(|| {
        ContractError::compatibility("engine-free compatibility source output has no final path")
    })?;
    if !matches!(
        Path::new(leaf).components().next(),
        Some(Component::Normal(_))
    ) {
        return Err(ContractError::compatibility(
            "engine-free compatibility source output must have one safe final path segment",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ContractError::compatibility("engine-free compatibility source output has no parent")
    })?;
    let parent = parent.canonicalize().map_err(|_| {
        ContractError::compatibility(
            "engine-free compatibility source output parent is unavailable",
        )
    })?;
    open_directory(&parent).map_err(|_| {
        ContractError::compatibility(
            "engine-free compatibility source output parent is not a real directory",
        )
    })?;
    let root = parent.join(leaf);
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(ContractError::compatibility(
                "engine-free compatibility source output already exists and cannot be adopted",
            ))
        }
        Err(_) => {
            return Err(ContractError::compatibility(
                "cannot inspect engine-free compatibility source output",
            ))
        }
    }
    fs::create_dir(&root).map_err(|_| {
        ContractError::compatibility("cannot create engine-free compatibility source output")
    })?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|_| {
        ContractError::compatibility("cannot make engine-free compatibility source output private")
    })?;
    open_directory(&root).map_err(|_| {
        ContractError::compatibility("fresh engine-free compatibility source output is unsafe")
    })?;
    Ok(root)
}

struct SourceCopy<'a> {
    root: &'a Path,
    saw_engine_directory: bool,
}

impl<'a> SourceCopy<'a> {
    const fn new(root: &'a Path) -> Self {
        Self {
            root,
            saw_engine_directory: false,
        }
    }

    fn copy(&mut self, path: &str, mode: &str, bytes: &[u8]) -> Result<(), ContractError> {
        if path == "cmake" {
            if mode != "040000" {
                return Err(ContractError::compatibility(
                    "selected compatibility source top-level cmake entry is not a directory",
                ));
            }
            self.saw_engine_directory = true;
            return Ok(());
        }
        if path.starts_with("cmake/") {
            return Ok(());
        }
        let destination = self.root.join(path);
        match mode {
            "040000" | "160000" => {
                fs::create_dir(&destination).map_err(|_| {
                    ContractError::compatibility(
                        "cannot create a committed compatibility source directory",
                    )
                })?;
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).map_err(
                    |_| {
                        ContractError::compatibility(
                            "cannot set a committed compatibility source directory mode",
                        )
                    },
                )?;
            }
            "100644" | "100755" => write_regular(&destination, bytes, mode == "100755")?,
            "120000" => write_symlink(self.root, path, &destination, bytes)?,
            _ => {
                return Err(ContractError::compatibility(
                    "committed compatibility source entry has an unsupported Git mode",
                ))
            }
        }
        Ok(())
    }
}

fn write_regular(destination: &Path, bytes: &[u8], executable: bool) -> Result<(), ContractError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|_| {
            ContractError::compatibility("cannot create committed compatibility source file")
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            ContractError::compatibility("cannot write committed compatibility source file")
        })?;
    fs::set_permissions(
        destination,
        fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )
    .map_err(|_| {
        ContractError::compatibility("cannot set committed compatibility source file mode")
    })
}

fn write_symlink(
    root: &Path,
    relative: &str,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), ContractError> {
    let target = std::str::from_utf8(bytes).map(Path::new).map_err(|_| {
        ContractError::compatibility("committed compatibility source link target is not UTF-8")
    })?;
    validate_link_target(Path::new(relative), target).map_err(|_| {
        ContractError::compatibility(
            "committed compatibility source link escapes the snapshot root",
        )
    })?;
    let parent = destination.parent().ok_or_else(|| {
        ContractError::compatibility("committed compatibility source link has no parent directory")
    })?;
    if !parent.starts_with(root) {
        return Err(ContractError::compatibility(
            "committed compatibility source link destination escapes the snapshot root",
        ));
    }
    symlink(target, destination).map_err(|_| {
        ContractError::compatibility("cannot create committed compatibility source symbolic link")
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;
    use std::process::Command;

    use super::{materialize_engine_free_source, EngineFreeSourceRequest};

    fn git(root: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=AROS test",
                "-c",
                "user.email=aros-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn materializes_only_committed_engine_free_source_without_overwriting() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q"]);
        fs::create_dir(source.join("cmake")).unwrap();
        fs::write(source.join("cmake/AROS.cmake"), "source engine").unwrap();
        fs::create_dir(source.join("arch")).unwrap();
        fs::write(
            source.join("arch/source.c"),
            "int main(void) { return 0; }\n",
        )
        .unwrap();
        fs::write(source.join("configure"), "#!/bin/sh\n").unwrap();
        let mut mode = fs::metadata(source.join("configure"))
            .unwrap()
            .permissions();
        mode.set_mode(0o755);
        fs::set_permissions(source.join("configure"), mode).unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-qm", "test: source"]);
        fs::create_dir(source.join("untracked")).unwrap();
        fs::write(source.join("untracked/ignored"), "must not be copied").unwrap();

        let rejected_output = temporary.path().join("rejected-engine-free");
        assert!(materialize_engine_free_source(&EngineFreeSourceRequest {
            source_root: source.clone(),
            output_root: rejected_output.clone(),
        })
        .is_err());
        assert!(rejected_output.exists());
        fs::remove_dir_all(source.join("untracked")).unwrap();

        let output = temporary.path().join("engine-free");
        let result = materialize_engine_free_source(&EngineFreeSourceRequest {
            source_root: source,
            output_root: output.clone(),
        })
        .unwrap();
        assert_eq!(result.root, output.canonicalize().unwrap());
        assert!(!result.root.join("cmake").exists());
        assert_eq!(
            fs::read(result.root.join("arch/source.c")).unwrap(),
            b"int main(void) { return 0; }\n"
        );
        assert_eq!(
            fs::metadata(result.root.join("configure"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0o111
        );
        assert!(materialize_engine_free_source(&EngineFreeSourceRequest {
            source_root: result.root,
            output_root: temporary.path().join("second"),
        })
        .is_err());
        assert!(materialize_engine_free_source(&EngineFreeSourceRequest {
            source_root: temporary.path().join("source"),
            output_root: output,
        })
        .is_err());
    }
}
