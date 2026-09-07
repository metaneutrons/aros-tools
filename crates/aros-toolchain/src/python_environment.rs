//! Isolated, lock-owned host Python environment preparation.
//!
//! This module does not replace any AROS Python script.  It prepares the
//! module roots consumed by the selected upstream configure/build phase and
//! proves that a host interpreter imports those exact extracted modules.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aros_common::{run_output_with_timeout, Sha256Digest};
use aros_fetch::engine::cache::snapshot_verified_cache_payload;
use flate2::read::GzDecoder;
use serde::Serialize;
use tar::{Archive, EntryType};

use crate::source_lock::{HostPythonPackage, SourceLock};
use crate::ContractError;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_LIMIT: usize = 16 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// A host interpreter observation used in native producer evidence.
#[derive(Debug, Clone, Serialize)]
pub struct PythonInterpreter {
    /// Canonical path selected from `PATH` at preparation time.
    pub path: PathBuf,
    /// First exact version line emitted by the interpreter.
    pub version: String,
}

/// Prepared private import roots and sanitized environment entries.
#[derive(Debug, Clone)]
pub struct PythonEnvironment {
    interpreter: PythonInterpreter,
    import_roots: Vec<PathBuf>,
    packages: Vec<PythonPackageObservation>,
}

/// Lock-bound Python package identity suitable for a producer receipt.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PythonPackageObservation {
    /// Python import/module name.
    pub name: String,
    /// Locked module version.
    pub version: String,
    /// Exact source-cache payload filename.
    pub filename: String,
    /// Locked package archive SHA-256.
    pub sha256: Sha256Digest,
    /// Locked package archive byte length.
    pub size: u64,
}

impl PythonEnvironment {
    /// Prepare a fresh private module root from lock-owned cache archives.
    ///
    /// The destination must not exist.  Extraction accepts only regular files
    /// and directories below each declared single archive root.  Every archive
    /// is first copied by `aros-fetch` into a private no-follow snapshot.
    ///
    /// # Errors
    ///
    /// Returns AX0401 when the host interpreter, cache package payload or
    /// isolated import probe is unusable.  It never installs packages, invokes
    /// `pip`, uses site packages or alters the selected cache.
    pub fn prepare(
        lock: &SourceLock,
        cache_root: &Path,
        destination: &Path,
    ) -> Result<Self, ContractError> {
        let interpreter = find_interpreter()?;
        Self::prepare_with_interpreter(lock, cache_root, destination, &interpreter)
    }

    /// Prepare the private runtime with one already observed host interpreter.
    ///
    /// Native lifecycle code uses this form so the interpreter that imports
    /// lock-owned modules is identical to the interpreter recorded in host
    /// preflight. The path is still canonicalized and version-probed here;
    /// callers cannot substitute an unchecked string.
    ///
    /// # Errors
    ///
    /// Returns AX0401 under the same conditions as [`Self::prepare`], plus an
    /// error when the caller-selected interpreter is not a usable Python 3
    /// executable.
    pub fn prepare_with_interpreter(
        lock: &SourceLock,
        cache_root: &Path,
        destination: &Path,
        interpreter: &PythonInterpreter,
    ) -> Result<Self, ContractError> {
        verify_selected_package_contract(lock)?;
        if !cache_root.is_absolute() {
            return Err(ContractError::environment(
                "verified host Python source cache must be absolute",
            ));
        }
        if !destination.is_absolute() {
            return Err(ContractError::environment(
                "private host Python environment destination must be absolute",
            ));
        }
        if destination.exists() {
            return Err(ContractError::environment(
                "private host Python environment destination already exists",
            ));
        }
        fs::create_dir(destination).map_err(|_| {
            ContractError::environment("cannot create private host Python environment directory")
        })?;
        ensure_private_directory(destination)?;
        let interpreter = revalidate_interpreter(interpreter)?;
        let mut import_roots = Vec::with_capacity(lock.host_python_packages().len());
        let mut packages = Vec::with_capacity(lock.host_python_packages().len());
        for package in lock.host_python_packages() {
            if !package.payload().filename().ends_with(".tar.gz") {
                return Err(ContractError::environment(
                    "selected host Python package archive is not a supported .tar.gz payload",
                ));
            }
            let payload = package.payload();
            let snapshot = snapshot_verified_cache_payload(
                cache_root,
                payload.filename(),
                payload.size(),
                payload.sha256(),
            )
            .map_err(|_| {
                ContractError::environment(
                    "locked host Python package is unavailable or does not match the verified cache",
                )
            })?;
            let root = destination.join(package.source_root());
            extract_package(snapshot.path(), &root, package)?;
            snapshot.revalidate().map_err(|_| {
                ContractError::environment(
                    "locked host Python package cache object changed during environment preparation",
                )
            })?;
            let import_root = package_import_root(&root, package)?;
            import_roots.push(import_root);
            packages.push(PythonPackageObservation {
                name: package.name().to_owned(),
                version: package.version().to_owned(),
                filename: payload.filename().to_owned(),
                sha256: payload.sha256().clone(),
                size: payload.size(),
            });
        }
        let environment = Self {
            interpreter,
            import_roots,
            packages,
        };
        environment.verify_runtime(lock)?;
        Ok(environment)
    }

    /// Interpreter identity selected for this environment.
    #[must_use]
    pub const fn interpreter(&self) -> &PythonInterpreter {
        &self.interpreter
    }

    /// Private import roots in lock declaration order.
    #[must_use]
    pub fn import_roots(&self) -> &[PathBuf] {
        &self.import_roots
    }

    /// Locked package identities and hashes verified for this environment.
    #[must_use]
    pub fn packages(&self) -> &[PythonPackageObservation] {
        &self.packages
    }

    /// Apply only the controlled Python variables to a child process command.
    ///
    /// Callers remain responsible for their wider build environment.  This
    /// method deliberately removes `PYTHONHOME` and prevents user-site imports.
    pub fn apply_to(&self, command: &mut Command) {
        command
            .env_remove("PYTHONHOME")
            .env("PYTHON", &self.interpreter.path)
            .env("PYTHONPATH", joined_paths(&self.import_roots))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONHASHSEED", "0");
    }

    fn verify_runtime(&self, lock: &SourceLock) -> Result<(), ContractError> {
        let expected = lock
            .host_python_packages()
            .iter()
            .zip(&self.import_roots)
            .map(|(package, root)| RuntimePackage {
                name: package.name(),
                version: package.version(),
                root,
            })
            .collect::<Vec<_>>();
        let expected = serde_json::to_string(&expected)
            .map_err(|_| ContractError::environment("cannot encode Python runtime probe"))?;
        let mut command = Command::new(&self.interpreter.path);
        self.apply_to(&mut command);
        command.args(["-s", "-B", "-c", RUNTIME_PROBE, &expected]);
        let output =
            run_output_with_timeout(&mut command, CAPTURE_LIMIT, PROBE_TIMEOUT).map_err(|_| {
                ContractError::environment("cannot start the selected host Python interpreter")
            })?;
        if output.timed_out || !output.status.success() {
            return Err(ContractError::environment(
                "selected host Python interpreter cannot import the exact lock-owned module environment",
            ));
        }
        Ok(())
    }
}

fn verify_selected_package_contract(lock: &SourceLock) -> Result<(), ContractError> {
    let packages = lock
        .host_python_packages()
        .iter()
        .map(HostPythonPackage::name)
        .collect::<BTreeSet<_>>();
    if packages != BTreeSet::from(["mako", "markupsafe"]) {
        return Err(ContractError::environment(
            "selected AROS source contract requires exactly the locked mako and markupsafe modules",
        ));
    }
    if !lock
        .host_python_packages()
        .iter()
        .all(|package| python_module_name(package.name()))
    {
        return Err(ContractError::environment(
            "selected host Python package declaration has an unsupported module name",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct RuntimePackage<'a> {
    name: &'a str,
    version: &'a str,
    root: &'a Path,
}

fn find_interpreter() -> Result<PythonInterpreter, ContractError> {
    let selected = which::which("python3").map_err(|_| {
        ContractError::environment("required host interpreter 'python3' is unavailable")
    })?;
    let path = fs::canonicalize(selected).map_err(|_| {
        ContractError::environment("required host interpreter 'python3' is unavailable")
    })?;
    let mut command = Command::new(&path);
    command.args(["-s", "-B", "--version"]);
    let output =
        run_output_with_timeout(&mut command, CAPTURE_LIMIT, PROBE_TIMEOUT).map_err(|_| {
            ContractError::environment("cannot start the selected host Python interpreter")
        })?;
    if output.timed_out || !output.status.success() {
        return Err(ContractError::environment(
            "selected host Python interpreter did not report a usable version",
        ));
    }
    let version = output
        .stdout
        .exact_bytes()
        .or_else(|| output.stderr.exact_bytes())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .filter(|line| line.starts_with("Python 3."))
        .ok_or_else(|| ContractError::environment("selected host interpreter is not Python 3"))?
        .to_owned();
    Ok(PythonInterpreter { path, version })
}

fn revalidate_interpreter(
    expected: &PythonInterpreter,
) -> Result<PythonInterpreter, ContractError> {
    let path = fs::canonicalize(&expected.path).map_err(|_| {
        ContractError::environment("selected host Python interpreter changed before preparation")
    })?;
    if !path.is_file() {
        return Err(ContractError::environment(
            "selected host Python interpreter is not a regular file",
        ));
    }
    let mut command = Command::new(&path);
    command.args(["-s", "-B", "--version"]);
    let output =
        run_output_with_timeout(&mut command, CAPTURE_LIMIT, PROBE_TIMEOUT).map_err(|_| {
            ContractError::environment("cannot revalidate the selected host Python interpreter")
        })?;
    let version = output
        .stdout
        .exact_bytes()
        .or_else(|| output.stderr.exact_bytes())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .filter(|line| line.starts_with("Python 3."))
        .ok_or_else(|| ContractError::environment("selected host interpreter is not Python 3"))?
        .to_owned();
    if path != expected.path || version != expected.version {
        return Err(ContractError::environment(
            "selected host Python interpreter changed after native preflight",
        ));
    }
    Ok(PythonInterpreter { path, version })
}

fn extract_package(
    archive: &Path,
    destination: &Path,
    package: &HostPythonPackage,
) -> Result<(), ContractError> {
    if destination.exists() {
        return Err(ContractError::environment(
            "host Python package extraction root already exists",
        ));
    }
    let file = fs::File::open(archive).map_err(|_| {
        ContractError::environment("cannot read private host Python package snapshot")
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|_| {
        ContractError::environment("locked host Python package is not a readable gzip tar archive")
    })?;
    let parent = destination.parent().ok_or_else(|| {
        ContractError::environment("private host Python extraction root has no parent directory")
    })?;
    let mut count = 0_usize;
    let mut total = 0_u64;
    for entry in entries {
        let mut entry = entry.map_err(|_| {
            ContractError::environment("locked host Python package archive entry is unreadable")
        })?;
        count = count.checked_add(1).ok_or_else(|| {
            ContractError::environment("host Python package archive entry count overflow")
        })?;
        if count > MAX_ENTRIES {
            return Err(ContractError::environment(
                "host Python package archive exceeds the 100000-entry safety limit",
            ));
        }
        let relative = archive_relative(&entry, package.source_root())?;
        let output = parent.join(&relative);
        let kind = entry.header().entry_type();
        if kind == EntryType::Directory {
            create_directory(&output)?;
            continue;
        }
        if kind != EntryType::Regular {
            return Err(ContractError::environment(
                "host Python package archive contains a non-regular non-directory entry",
            ));
        }
        let size = entry.header().size().map_err(|_| {
            ContractError::environment("host Python package archive has an invalid entry size")
        })?;
        if size > MAX_FILE_BYTES {
            return Err(ContractError::environment(
                "host Python package archive contains an oversized regular file",
            ));
        }
        total = total.checked_add(size).ok_or_else(|| {
            ContractError::environment("host Python package archive size accounting overflow")
        })?;
        if total > MAX_TOTAL_BYTES {
            return Err(ContractError::environment(
                "host Python package archive exceeds the 512 MiB extracted-size safety limit",
            ));
        }
        let output_parent = output.parent().ok_or_else(|| {
            ContractError::environment("host Python package file has no safe extraction parent")
        })?;
        create_directory(output_parent)?;
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|_| {
                ContractError::environment("cannot create a private host Python package file")
            })?;
        let copied = io::copy(
            &mut entry.by_ref().take(size.saturating_add(1)),
            &mut target,
        )
        .map_err(|_| ContractError::environment("cannot extract host Python package bytes"))?;
        if copied != size {
            return Err(ContractError::environment(
                "host Python package archive entry changed or ended before its declared size",
            ));
        }
    }
    if !destination.is_dir()
        || fs::symlink_metadata(destination).is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(ContractError::environment(
            "host Python package archive did not create its declared root directory",
        ));
    }
    Ok(())
}

fn archive_relative<R: Read>(
    entry: &tar::Entry<'_, R>,
    root: &str,
) -> Result<PathBuf, ContractError> {
    let path = entry.path().map_err(|_| {
        ContractError::environment("host Python package archive contains a non-UTF-8 path")
    })?;
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return Err(ContractError::environment(
            "host Python package archive entry escapes its declared root",
        ));
    };
    if first != OsStr::new(root) {
        return Err(ContractError::environment(
            "host Python package archive entry is outside its declared root",
        ));
    }
    let mut result = PathBuf::from(first);
    for component in components {
        let Component::Normal(part) = component else {
            return Err(ContractError::environment(
                "host Python package archive entry has an unsafe path component",
            ));
        };
        result.push(part);
    }
    Ok(result)
}

fn create_directory(path: &Path) -> Result<(), ContractError> {
    fs::create_dir_all(path).map_err(|_| {
        ContractError::environment("cannot create a private host Python package directory")
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ContractError::environment("cannot inspect a private host Python package directory")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::environment(
            "private host Python package extraction encountered a non-directory path",
        ));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ContractError::environment("cannot inspect private host Python environment directory")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::environment(
            "private host Python environment root is not a directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
            ContractError::environment("cannot make private host Python environment owner-only")
        })?;
        let metadata = fs::symlink_metadata(path).map_err(|_| {
            ContractError::environment("cannot recheck private host Python environment permissions")
        })?;
        if metadata.mode() & 0o077 != 0 {
            return Err(ContractError::environment(
                "private host Python environment is not owner-only",
            ));
        }
    }
    Ok(())
}

fn package_import_root(root: &Path, package: &HostPythonPackage) -> Result<PathBuf, ContractError> {
    let import_root = if package.python_path() == "." {
        root.to_owned()
    } else {
        root.join(package.python_path())
    };
    let metadata = fs::symlink_metadata(&import_root).map_err(|_| {
        ContractError::environment("locked host Python package has no declared import root")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::environment(
            "locked host Python package import root is not a private directory",
        ));
    }
    Ok(import_root)
}

fn joined_paths(paths: &[PathBuf]) -> OsString {
    std::env::join_paths(paths).unwrap_or_default()
}

fn python_module_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

const RUNTIME_PROBE: &str = r#"
import importlib
import json
from pathlib import Path
import sys

expected = json.loads(sys.argv[1])
for item in expected:
    module = importlib.import_module(item["name"])
    if getattr(module, "__version__", None) != item["version"]:
        raise SystemExit("locked module version mismatch")
    module_path = Path(module.__file__).resolve()
    root = Path(item["root"]).resolve()
    try:
        module_path.relative_to(root)
    except ValueError:
        raise SystemExit("locked module origin mismatch")
if {item["name"] for item in expected} != {"mako", "markupsafe"}:
    raise SystemExit("current AROS source contract requires exactly mako and markupsafe")
from mako.template import Template
if Template("locked runtime").render() != "locked runtime":
    raise SystemExit("Mako template validation failed")
"#;
