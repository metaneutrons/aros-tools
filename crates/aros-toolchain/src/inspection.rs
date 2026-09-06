//! Bounded read-only input inspection; never refresh indexes or run filters.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::{
    exit_signal, run_output_with_timeout, sha256_bytes, sha256_reader, DiagnosticContext,
    Sha256Digest,
};

use crate::recipe::{safe_relative_path, GitObjectId};
use crate::{canonical::MAX_DOCUMENT_BYTES, ContractError};

pub fn absolute(path: &Path) -> Result<PathBuf, ContractError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || path
            .to_str()
            .is_none_or(|text| text.len() > 4096 || text.chars().any(char::is_control))
    {
        return Err(ContractError::preflight(
            "input path must be nonempty UTF-8 without traversal or control characters",
        ));
    }
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|_| ContractError::preflight("cannot resolve the invocation directory"))
    }
}

pub fn directory(path: &Path) -> Result<PathBuf, ContractError> {
    let path = absolute(path)?
        .canonicalize()
        .map_err(|_| ContractError::preflight("selected checkout root is not accessible"))?;
    if !path.is_dir() || path.parent().is_none() {
        return Err(ContractError::preflight(
            "selected root must be an existing directory, not a filesystem root",
        ));
    }
    absolute(&path)
}

/// Resolve the nearest existing ancestor, without creating even the final leaf.
pub fn destination(path: &Path) -> Result<PathBuf, ContractError> {
    let mut parent = absolute(path)?;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(&parent) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let leaf = parent
                    .file_name()
                    .ok_or_else(|| {
                        ContractError::preflight("destination has no existing ancestor")
                    })?
                    .to_owned();
                missing.push(leaf);
                parent.pop();
            }
            Err(_) => {
                return Err(ContractError::preflight(
                    "destination ancestor cannot be inspected",
                ))
            }
        }
    }
    let mut resolved = directory(&parent)?;
    for leaf in missing.into_iter().rev() {
        resolved.push(leaf);
    }
    Ok(resolved)
}

/// Descriptor-relative no-follow traversal also prevents FIFO/device blocking.
#[cfg(unix)]
fn open_regular(path: &Path) -> Result<File, ContractError> {
    use rustix::fs::{open, openat, Mode, OFlags};
    let path = absolute(path)?;
    let names: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let (leaf, ancestors) = names
        .split_last()
        .ok_or_else(|| ContractError::preflight("expected a regular input file"))?;
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let failed =
        |_| ContractError::preflight("cannot open a regular input through non-symlink ancestors");
    let mut parent = open("/", flags | OFlags::DIRECTORY, Mode::empty()).map_err(failed)?;
    for name in ancestors {
        parent =
            openat(&parent, *name, flags | OFlags::DIRECTORY, Mode::empty()).map_err(failed)?;
    }
    let file = File::from(
        openat(&parent, *leaf, flags | OFlags::NONBLOCK, Mode::empty()).map_err(failed)?,
    );
    if !file
        .metadata()
        .map_err(|_| ContractError::preflight("cannot inspect input metadata"))?
        .is_file()
    {
        return Err(ContractError::preflight("input is not a regular file"));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular(_path: &Path) -> Result<File, ContractError> {
    Err(ContractError::preflight(
        "producer inspection requires a supported Unix host",
    ))
}

pub fn read(path: &Path) -> Result<Vec<u8>, ContractError> {
    let mut bytes = Vec::new();
    open_regular(path)?
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::preflight("cannot read the selected regular input"))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(ContractError::invalid("inspected input exceeds 1 MiB"));
    }
    Ok(bytes)
}

pub fn frontend_digest() -> Result<Sha256Digest, ContractError> {
    // A measured file identifies the inspected executable, not its origin.
    // It does not attest that a caller's on-disk executable was not replaced.
    let path = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| ContractError::preflight("cannot locate the running frontend file"))?;
    let limit = 512 * 1024 * 1024;
    let observed = sha256_reader(&mut open_regular(&path)?.take(limit + 1))
        .map_err(|_| ContractError::preflight("cannot measure the frontend file"))?;
    if observed.size > limit {
        return Err(ContractError::preflight(
            "frontend file exceeds the 512 MiB inspection limit",
        ));
    }
    Ok(observed.digest)
}

pub struct Checkout<'a> {
    root: &'a Path,
    commit: &'a GitObjectId,
    tree: &'a GitObjectId,
    deadline: Instant,
}

impl<'a> Checkout<'a> {
    pub(crate) fn inspect(
        root: &'a Path,
        identity: (&'a GitObjectId, &'a GitObjectId),
        deadline: Instant,
    ) -> Result<Self, ContractError> {
        let checkout = Self {
            root,
            commit: identity.0,
            tree: identity.1,
            deadline,
        };
        let top = checkout.git(&["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(text(&top)?.trim_end_matches('\n'));
        if directory(&top)? != root {
            return Err(ContractError::identity(
                "selected input is not the Git checkout root",
            ));
        }
        checkout.recheck()?;
        Ok(checkout)
    }

    pub(crate) fn recheck(&self) -> Result<(), ContractError> {
        let head = self.git(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let observed_tree = self.git(&["rev-parse", "--verify", "HEAD^{tree}"])?;
        let head = GitObjectId::try_from(text(&head)?.trim().to_owned())
            .map_err(ContractError::identity)?;
        let observed_tree = GitObjectId::try_from(text(&observed_tree)?.trim().to_owned())
            .map_err(ContractError::identity)?;
        if head != *self.commit || observed_tree != *self.tree {
            return Err(ContractError::identity(format!(
                "checkout identity mismatch: expected commit {} / tree {}; observed commit {} / tree {}",
                self.commit.as_str(),
                self.tree.as_str(), head.as_str(), observed_tree.as_str()
            )));
        }
        Ok(())
    }

    pub(crate) fn file(&self, path: &str) -> Result<Option<Vec<u8>>, ContractError> {
        if !safe_relative_path(path) {
            return Err(ContractError::invalid("selected committed path is unsafe"));
        }
        let entry = self.git(&["ls-tree", "-z", self.tree.as_str(), "--", path])?;
        if entry.is_empty() {
            return Ok(None);
        }
        let entry = text(&entry)?.trim_end_matches('\0');
        let (metadata, actual_path) = entry
            .split_once('\t')
            .ok_or_else(|| ContractError::identity("invalid Git tree response"))?;
        let fields: Vec<_> = metadata.split(' ').collect();
        if fields.len() != 3
            || !matches!(fields[0], "100644" | "100755")
            || fields[1] != "blob"
            || actual_path != path
        {
            return Err(ContractError::identity(
                "selected committed input is not exactly one regular blob",
            ));
        }
        let oid = GitObjectId::try_from(fields[2].to_owned()).map_err(ContractError::identity)?;
        let blob = self.git(&["cat-file", "blob", oid.as_str()])?;
        // Compare raw files, never git diff/status (which may invoke filters).
        if read(&self.root.join(path))? != blob {
            return Err(ContractError::identity(
                "selected input differs from its committed raw bytes",
            ));
        }
        Ok(Some(blob))
    }

    pub(crate) fn required_file(&self, path: &str) -> Result<Vec<u8>, ContractError> {
        self.file(path)?.ok_or_else(|| {
            ContractError::invalid(format!("required committed input is missing: {path}"))
        })
    }

    pub(crate) fn source_lock(&self, digest: &Sha256Digest) -> Result<(), ContractError> {
        // Historical recipes lack a path selector. Select one direct committed
        // lock by its measured identity, not an LLVM filename/version pin.
        let tree = format!("{}:toolchains", self.tree.as_str());
        let listing = self.git(&["ls-tree", "--name-only", "-z", &tree])?;
        let mut selected = 0;
        let entries: Vec<_> = text(&listing)?
            .split('\0')
            .filter(|name| !name.is_empty())
            .collect();
        if entries.len() > 128 {
            return Err(ContractError::invalid(
                "producer toolchains directory exceeds 128 entries",
            ));
        }
        for name in entries {
            if name.ends_with(".sources.json") {
                let path = format!("toolchains/{name}");
                let bytes = self.required_file(&path)?;
                if sha256_bytes(&bytes) == *digest {
                    selected += 1;
                }
            }
        }
        if selected != 1 {
            return Err(ContractError::identity(
                "recipe must select exactly one committed source lock by digest",
            ));
        }
        Ok(())
    }

    fn git(&self, arguments: &[&str]) -> Result<Vec<u8>, ContractError> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                ContractError::prerequisite(
                    "read-only Git inspection exceeded its 60-second budget",
                )
            })?;
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "")
            .env("LC_ALL", "C")
            .current_dir(self.root)
            .args([
                "--no-pager",
                "--no-replace-objects",
                "--no-lazy-fetch",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
            ])
            .args(arguments);
        let timeout = remaining.min(Duration::from_secs(10));
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let result = run_output_with_timeout(&mut command, MAX_DOCUMENT_BYTES, timeout).map_err(
            |error| {
                let reason = match error.kind() {
                    std::io::ErrorKind::NotFound => {
                        "trusted Git executable or selected working directory is missing"
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        "permission denied while executing read-only Git inspection"
                    }
                    std::io::ErrorKind::TimedOut => {
                        "read-only Git inspection timed out and process cleanup failed"
                    }
                    _ => "I/O failure while executing or capturing read-only Git inspection",
                };
                ContractError::prerequisite(reason).context(DiagnosticContext {
                    tool: Some("git".into()),
                    timed_out: Some(error.kind() == std::io::ErrorKind::TimedOut),
                    timeout_ms: Some(timeout_ms),
                    ..DiagnosticContext::default()
                })
            },
        )?;
        if result.timed_out || !result.status.success() {
            let reason = if result.timed_out {
                "read-only Git inspection exceeded its process deadline"
            } else {
                "read-only Git inspection exited unsuccessfully; check local objects, access and Git capability"
            };
            return Err(
                ContractError::prerequisite(reason).context(DiagnosticContext {
                    tool: Some("git".into()),
                    exit_code: result.status.code(),
                    signal: exit_signal(result.status),
                    timed_out: Some(result.timed_out),
                    timeout_ms: Some(timeout_ms),
                    ..DiagnosticContext::default()
                }),
            );
        }
        // Never forward potentially private Git stderr or truncate an identity.
        result
            .stdout
            .exact_bytes()
            .map(<[u8]>::to_vec)
            .ok_or_else(|| ContractError::invalid("Git inspection output exceeds 1 MiB"))
    }
}

fn text(bytes: &[u8]) -> Result<&str, ContractError> {
    std::str::from_utf8(bytes)
        .map_err(|_| ContractError::invalid("Git inspection returned non-UTF-8 material"))
}
