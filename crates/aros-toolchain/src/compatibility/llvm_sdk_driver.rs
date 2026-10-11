//! Read-only evidence for the two historical LLVM SDK driver roles.
//!
//! This boundary measures the complete compiler tree and asks `bin/clang` and
//! `bin/clang++` for their reported version and selected target triple. It does
//! not authenticate the selected compiler package, establish SDK link success,
//! prove the target's scalar ABI facts, or admit a release. Callers must retain
//! the complete [`SdkTarget`] and independently authenticate the package.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aros_common::sdk_manifest::SdkTarget;
use aros_common::{
    measure_tree_content_cas_bounded, open_regular_file_nofollow, process::run_output_with_control,
    sha256_bytes, sha256_reader, ArosCompilerIdentity, CancellationToken, Sha256Digest,
    TreeContentCas, TreeTraversalLimits,
};

use crate::ContractError;

const MAX_COMPILER_TREE_ENTRIES: usize = 100_000;
const MAX_COMPILER_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_QUERY_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_QUERY_TIMEOUT: Duration = Duration::from_secs(60);
const CLANG_RELATIVE_PATH: &str = "bin/clang";
const CLANGXX_RELATIVE_PATH: &str = "bin/clang++";

/// Explicit local inputs for read-only LLVM SDK driver inspection.
#[derive(Debug, Clone)]
pub struct LlvmSdkDriverRequest {
    /// Installed compiler root. The selected directory is canonicalized before
    /// measurement and execution.
    pub compiler_root: PathBuf,
    /// Independently selected compiler identity; GNU identities are rejected.
    pub compiler: ArosCompilerIdentity,
    /// Complete source-selected SDK target descriptor, retained unchanged.
    pub target: SdkTarget,
    /// Per-query child-process timeout, bounded to 60 seconds.
    pub timeout: Duration,
}

/// One exact query output retained as bounded bytes, digest and byte length.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LlvmSdkDriverQueryIdentity {
    sha256: Sha256Digest,
    size: u64,
    stdout: Vec<u8>,
}

impl LlvmSdkDriverQueryIdentity {
    /// SHA-256 of the complete captured stdout bytes.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Exact captured stdout byte length.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Exact bounded stdout bytes returned by this driver query.
    #[must_use]
    pub fn stdout_bytes(&self) -> &[u8] {
        &self.stdout
    }
}

/// The fixed command role whose invocation spelling is preserved in `argv[0]`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LlvmSdkDriverRole {
    /// C compiler driver role at `bin/clang`.
    Clang,
    /// C++ compiler driver role at `bin/clang++`.
    Clangxx,
}

/// Closed, measured evidence for one LLVM SDK driver role.
///
/// All fields are private so callers can consume a measured role identity but
/// cannot assemble one from independent claims.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LlvmSdkDriverRoleIdentity {
    role: LlvmSdkDriverRole,
    invocation_path: PathBuf,
    resolved_executable: PathBuf,
    file_sha256: Sha256Digest,
    file_size: u64,
    file_mode: u32,
    version_stdout: LlvmSdkDriverQueryIdentity,
    target_stdout: LlvmSdkDriverQueryIdentity,
}

impl LlvmSdkDriverRoleIdentity {
    /// Fixed compiler role.
    #[must_use]
    pub const fn role(&self) -> LlvmSdkDriverRole {
        self.role
    }

    /// Original `bin/clang` or `bin/clang++` command path used for `argv[0]`.
    ///
    /// This deliberately preserves `clang++` when that path is an internal
    /// symlink to `clang`.
    #[must_use]
    pub fn invocation_path(&self) -> &Path {
        &self.invocation_path
    }

    /// Canonical in-root regular executable reached by the invocation path.
    #[must_use]
    pub fn resolved_executable(&self) -> &Path {
        &self.resolved_executable
    }

    /// SHA-256 of the resolved executable bytes.
    #[must_use]
    pub const fn file_sha256(&self) -> &Sha256Digest {
        &self.file_sha256
    }

    /// Resolved executable byte length.
    #[must_use]
    pub const fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Measured permission bits of the resolved executable.
    #[must_use]
    pub const fn file_mode(&self) -> u32 {
        self.file_mode
    }

    /// Complete stdout evidence for the `--version` query.
    #[must_use]
    pub const fn version_query(&self) -> &LlvmSdkDriverQueryIdentity {
        &self.version_stdout
    }

    /// Complete stdout evidence for the selected `-dumpmachine` query.
    #[must_use]
    pub const fn target_query(&self) -> &LlvmSdkDriverQueryIdentity {
        &self.target_stdout
    }
}

/// Measured binding for one LLVM compiler tree, target and both driver roles.
///
/// This record is process-local evidence, not compiler package authentication
/// or execution authorization. Its private tree snapshot detects later changes
/// when [`Self::revalidate`] is called.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LlvmSdkDriverBinding {
    selected_root: PathBuf,
    compiler_root: PathBuf,
    compiler: ArosCompilerIdentity,
    target: SdkTarget,
    compiler_tree_sha256: Sha256Digest,
    compiler_tree_snapshot: TreeContentCas,
    clang: LlvmSdkDriverRoleIdentity,
    clangxx: LlvmSdkDriverRoleIdentity,
}

impl LlvmSdkDriverBinding {
    /// Original `bin/clang` invocation path, not a symlink-resolved target.
    #[must_use]
    pub fn clang_path(&self) -> &Path {
        self.clang.invocation_path()
    }

    /// Original `bin/clang++` invocation path, preserving its role spelling.
    #[must_use]
    pub fn clangxx_path(&self) -> &Path {
        self.clangxx.invocation_path()
    }

    /// Validated LLVM compiler identity supplied by the caller.
    #[must_use]
    pub const fn compiler(&self) -> &ArosCompilerIdentity {
        &self.compiler
    }

    /// Complete validated SDK target supplied by the caller.
    ///
    /// Query evidence confirms only the reported target triple. It does not
    /// prove the descriptor's other scalar ABI facts.
    #[must_use]
    pub const fn target(&self) -> &SdkTarget {
        &self.target
    }

    /// Portable content digest of the complete measured compiler tree,
    /// including file modes and symbolic-link target bytes.
    #[must_use]
    pub const fn compiler_tree_sha256(&self) -> &Sha256Digest {
        &self.compiler_tree_sha256
    }

    /// Closed measured identity for the C driver role.
    #[must_use]
    pub const fn clang_identity(&self) -> &LlvmSdkDriverRoleIdentity {
        &self.clang
    }

    /// Closed measured identity for the C++ driver role.
    #[must_use]
    pub const fn clangxx_identity(&self) -> &LlvmSdkDriverRoleIdentity {
        &self.clangxx
    }

    /// Remeasure the bounded compiler tree and require the same root and every
    /// file, directory, mode and symbolic-link snapshot.
    ///
    /// This does not rerun the driver queries or authenticate the package.
    ///
    /// # Errors
    /// Rejects cancellation, a changed selected root or any tree mutation.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        check_cancelled(cancellation)?;
        let current_root = self.selected_root.canonicalize().map_err(|_| {
            ContractError::compatibility("LLVM compiler root is no longer accessible")
        })?;
        if current_root != self.compiler_root || !current_root.is_dir() {
            return Err(ContractError::compatibility(
                "LLVM compiler root identity changed after driver inspection",
            ));
        }
        let current = measure_compiler_tree(&self.compiler_root)?;
        check_cancelled(cancellation)?;
        if current != self.compiler_tree_snapshot {
            return Err(ContractError::compatibility(
                "LLVM compiler tree changed after driver inspection",
            ));
        }
        Ok(())
    }
}

/// Measure and query the historical LLVM SDK C/C++ driver paths without
/// building or linking anything.
///
/// The compiler identity and generic target descriptor are validated before
/// process creation. All four commands run with a cleared environment except
/// `PATH=/nonexistent` and `LC_ALL=C`; stdout must be complete UTF-8 and stderr
/// must be empty. The compiler package must be authenticated independently by
/// the caller. This routine does not prove link success, scalar ABI facts, or
/// release eligibility.
///
/// # Errors
/// Rejects non-LLVM or invalid compiler identities, invalid/unsupported target
/// triples, an invalid timeout, unsafe or missing drivers, query disagreement,
/// process failure, cancellation, or a changed compiler tree.
pub fn inspect_llvm_sdk_drivers(
    request: &LlvmSdkDriverRequest,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkDriverBinding, ContractError> {
    validate_request(request)?;
    check_cancelled(cancellation)?;

    if !request.compiler_root.is_absolute() {
        return Err(ContractError::compatibility(
            "LLVM compiler root must be an absolute path",
        ));
    }
    let selected_root = request.compiler_root.clone();
    let compiler_root = selected_root
        .canonicalize()
        .map_err(|_| ContractError::compatibility("LLVM compiler root is not accessible"))?;
    if !compiler_root.is_dir() {
        return Err(ContractError::compatibility(
            "LLVM compiler root is not a real directory",
        ));
    }

    let before = measure_compiler_tree(&compiler_root)?;
    check_cancelled(cancellation)?;
    let clang_file = inspect_driver_file(&compiler_root, CLANG_RELATIVE_PATH)?;
    let clangxx_file = inspect_driver_file(&compiler_root, CLANGXX_RELATIVE_PATH)?;
    let clang = inspect_driver_role(
        LlvmSdkDriverRole::Clang,
        clang_file,
        &compiler_root,
        request,
        cancellation,
    )?;
    let clangxx = inspect_driver_role(
        LlvmSdkDriverRole::Clangxx,
        clangxx_file,
        &compiler_root,
        request,
        cancellation,
    )?;

    check_cancelled(cancellation)?;
    let after = measure_compiler_tree(&compiler_root)?;
    check_cancelled(cancellation)?;
    if before != after {
        return Err(ContractError::compatibility(
            "LLVM compiler tree changed during driver inspection",
        ));
    }

    Ok(LlvmSdkDriverBinding {
        selected_root,
        compiler_root,
        compiler: request.compiler.clone(),
        target: request.target.clone(),
        compiler_tree_sha256: before.payload_digest_excluding(None),
        compiler_tree_snapshot: before,
        clang,
        clangxx,
    })
}

fn validate_request(request: &LlvmSdkDriverRequest) -> Result<(), ContractError> {
    let ArosCompilerIdentity::Llvm { version } = &request.compiler else {
        return Err(ContractError::compatibility(
            "LLVM SDK driver inspection requires an LLVM compiler identity",
        ));
    };
    request
        .compiler
        .validate_for_target(&request.target.target_triple)
        .map_err(|_| ContractError::compatibility("LLVM compiler identity is invalid"))?;
    request
        .target
        .validate()
        .map_err(|_| ContractError::compatibility("SDK target descriptor is invalid"))?;
    validate_aros_target_triple(&request.target.target_triple)?;
    if version.is_empty() {
        return Err(ContractError::compatibility(
            "LLVM compiler identity has an empty version",
        ));
    }
    if request.timeout.is_zero() || request.timeout > MAX_QUERY_TIMEOUT {
        return Err(ContractError::compatibility(
            "LLVM SDK driver query timeout must be positive and at most 60 seconds",
        ));
    }
    Ok(())
}

fn validate_aros_target_triple(triple: &str) -> Result<(), ContractError> {
    let components = triple.split('-').collect::<Vec<_>>();
    if components.len() < 2
        || components.iter().any(|component| component.is_empty())
        || components.last() != Some(&"aros")
    {
        return Err(ContractError::compatibility(
            "SDK target triple must have nonempty components and end in an exact -aros component",
        ));
    }
    Ok(())
}

struct DriverFile {
    invocation_path: PathBuf,
    resolved_executable: PathBuf,
    file_sha256: Sha256Digest,
    file_size: u64,
    file_mode: u32,
}

fn inspect_driver_file(root: &Path, relative: &str) -> Result<DriverFile, ContractError> {
    let invocation_path = root.join(relative);
    let resolved_executable = resolve_internal_symlinks(root, Path::new(relative))?;
    if !resolved_executable.starts_with(root) || resolved_executable == root {
        return Err(ContractError::compatibility(
            "LLVM SDK driver symlink escapes the selected compiler root",
        ));
    }

    let mut file = open_regular_file_nofollow(&resolved_executable).map_err(|_| {
        ContractError::compatibility("LLVM SDK driver does not resolve to a regular file")
    })?;
    let before = file
        .metadata()
        .map_err(|_| ContractError::compatibility("cannot inspect LLVM SDK driver metadata"))?;
    if !before.is_file() || before.mode() & 0o111 == 0 {
        return Err(ContractError::compatibility(
            "LLVM SDK driver is not an executable regular file",
        ));
    }
    let measured = sha256_reader(&mut file)
        .map_err(|_| ContractError::compatibility("cannot measure LLVM SDK driver bytes"))?;
    let after = file
        .metadata()
        .map_err(|_| ContractError::compatibility("cannot recheck LLVM SDK driver metadata"))?;
    if !same_file_snapshot(&before, &after) || measured.size != before.len() {
        return Err(ContractError::compatibility(
            "LLVM SDK driver changed while it was being measured",
        ));
    }

    Ok(DriverFile {
        invocation_path,
        resolved_executable,
        file_sha256: measured.digest,
        file_size: measured.size,
        file_mode: before.mode() & 0o7777,
    })
}

fn resolve_internal_symlinks(root: &Path, relative: &Path) -> Result<PathBuf, ContractError> {
    let mut pending = path_components(relative)?;
    let mut resolved = root.to_owned();
    let mut symlink_count = 0_usize;

    while let Some(component) = pending.pop_front() {
        if component == OsStr::new(".") {
            continue;
        }
        if component == OsStr::new("..") {
            if !resolved.pop() || !resolved.starts_with(root) {
                return Err(ContractError::compatibility(
                    "LLVM SDK driver path escapes the selected compiler root",
                ));
            }
            continue;
        }

        let candidate = resolved.join(&component);
        let metadata = fs::symlink_metadata(&candidate).map_err(|_| {
            ContractError::compatibility("required LLVM SDK driver is missing or unresolved")
        })?;
        if !metadata.file_type().is_symlink() {
            resolved = candidate;
            continue;
        }

        symlink_count += 1;
        if symlink_count > 40 {
            return Err(ContractError::compatibility(
                "LLVM SDK driver has too many symbolic-link hops",
            ));
        }
        let target = fs::read_link(&candidate).map_err(|_| {
            ContractError::compatibility("cannot read LLVM SDK driver symbolic link")
        })?;
        let target_components = if target.is_absolute() {
            let internal = target.strip_prefix(root).map_err(|_| {
                ContractError::compatibility(
                    "LLVM SDK driver symlink escapes the selected compiler root",
                )
            })?;
            root.clone_into(&mut resolved);
            path_components(internal)?
        } else {
            path_components(&target)?
        };
        for target_component in target_components.into_iter().rev() {
            pending.push_front(target_component);
        }
    }

    if !resolved.starts_with(root) || resolved == root {
        return Err(ContractError::compatibility(
            "LLVM SDK driver path escapes the selected compiler root",
        ));
    }
    Ok(resolved)
}

fn path_components(path: &Path) -> Result<VecDeque<OsString>, ContractError> {
    let mut components = VecDeque::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => components.push_back(value.to_owned()),
            std::path::Component::ParentDir => components.push_back(OsString::from("..")),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(ContractError::compatibility(
                    "LLVM SDK driver symlink target has an invalid root component",
                ));
            }
        }
    }
    Ok(components)
}

fn same_file_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.mode() == right.mode()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn inspect_driver_role(
    role: LlvmSdkDriverRole,
    file: DriverFile,
    root: &Path,
    request: &LlvmSdkDriverRequest,
    cancellation: &CancellationToken,
) -> Result<LlvmSdkDriverRoleIdentity, ContractError> {
    let ArosCompilerIdentity::Llvm { version } = &request.compiler else {
        return Err(ContractError::compatibility(
            "LLVM SDK driver inspection requires an LLVM compiler identity",
        ));
    };

    // Invoke the original path so a `clang++ -> clang` alias retains its role
    // spelling in argv[0]. The resolved path is evidence only.
    let (version_text, version_stdout) = run_query(
        &file.invocation_path,
        root,
        &["--version"],
        request.timeout,
        cancellation,
    )?;
    validate_clang_version(&version_text, version)?;

    let target_argument = format!("--target={}", request.target.target_triple);
    let (target_text, target_stdout) = run_query(
        &file.invocation_path,
        root,
        &[&target_argument, "-dumpmachine"],
        request.timeout,
        cancellation,
    )?;
    validate_dumpmachine(&target_text, &request.target.target_triple)?;

    Ok(LlvmSdkDriverRoleIdentity {
        role,
        invocation_path: file.invocation_path,
        resolved_executable: file.resolved_executable,
        file_sha256: file.file_sha256,
        file_size: file.file_size,
        file_mode: file.file_mode,
        version_stdout,
        target_stdout,
    })
}

fn run_query(
    program: &Path,
    root: &Path,
    arguments: &[&str],
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<(String, LlvmSdkDriverQueryIdentity), ContractError> {
    check_cancelled(cancellation)?;
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/nonexistent")
        .env("LC_ALL", "C")
        .args(arguments);
    let output =
        run_output_with_control(&mut command, MAX_QUERY_CAPTURE_BYTES, timeout, cancellation)
            .map_err(|_| {
                if cancellation.is_cancelled() {
                    ContractError::compatibility("LLVM SDK driver inspection was cancelled")
                } else {
                    ContractError::compatibility("LLVM SDK driver query could not be completed")
                }
            })?;
    if output.cancelled || cancellation.is_cancelled() {
        return Err(ContractError::compatibility(
            "LLVM SDK driver inspection was cancelled",
        ));
    }
    if output.timed_out {
        return Err(ContractError::compatibility(
            "LLVM SDK driver query exceeded its explicit timeout",
        ));
    }
    if !output.status.success() {
        return Err(ContractError::compatibility(
            "LLVM SDK driver query returned a nonzero status",
        ));
    }
    let stdout = output.stdout.exact_bytes().ok_or_else(|| {
        ContractError::compatibility("LLVM SDK driver stdout exceeded its capture bound")
    })?;
    let stderr = output.stderr.exact_bytes().ok_or_else(|| {
        ContractError::compatibility("LLVM SDK driver stderr exceeded its capture bound")
    })?;
    if !stderr.is_empty() {
        return Err(ContractError::compatibility(
            "LLVM SDK driver query wrote to stderr",
        ));
    }
    let text = std::str::from_utf8(stdout)
        .map_err(|_| ContractError::compatibility("LLVM SDK driver stdout is not UTF-8"))?
        .to_owned();
    let size = u64::try_from(stdout.len())
        .map_err(|_| ContractError::compatibility("LLVM SDK driver stdout size is invalid"))?;
    Ok((
        text,
        LlvmSdkDriverQueryIdentity {
            sha256: sha256_bytes(stdout),
            size,
            stdout: stdout.to_vec(),
        },
    ))
}

fn validate_clang_version(output: &str, expected: &str) -> Result<(), ContractError> {
    let prefix = format!("clang version {expected}");
    let first_line = output.split('\n').next().unwrap_or_default();
    let matches = first_line
        .strip_prefix(&prefix)
        .is_some_and(|suffix| suffix.chars().next().is_none_or(char::is_whitespace));
    if !matches {
        return Err(ContractError::compatibility(
            "LLVM SDK driver version does not match the selected compiler identity",
        ));
    }
    Ok(())
}

fn validate_dumpmachine(output: &str, expected: &str) -> Result<(), ContractError> {
    let with_lf = format!("{expected}\n");
    let with_crlf = format!("{expected}\r\n");
    if output != expected && output != with_lf && output != with_crlf {
        return Err(ContractError::compatibility(
            "LLVM SDK driver dumpmachine differs from the selected target triple",
        ));
    }
    Ok(())
}

fn measure_compiler_tree(root: &Path) -> Result<TreeContentCas, ContractError> {
    let limits = TreeTraversalLimits::new(MAX_COMPILER_TREE_ENTRIES, MAX_COMPILER_TREE_BYTES)
        .map_err(|_| ContractError::compatibility("invalid LLVM compiler tree limits"))?;
    measure_tree_content_cas_bounded(root, limits).map_err(|_| {
        ContractError::compatibility("LLVM compiler tree is unsafe or exceeds its bounds")
    })
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), ContractError> {
    if cancellation.is_cancelled() {
        Err(ContractError::compatibility(
            "LLVM SDK driver inspection was cancelled",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "llvm_sdk_driver_tests.rs"]
mod tests;
