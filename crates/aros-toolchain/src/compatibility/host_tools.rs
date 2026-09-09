//! Measured host-command closures for native compatibility phases that need
//! host command resolution.
//!
//! The AROS CMake engine and upstream `configure`/generated Makefiles
//! necessarily resolve a bounded POSIX tool set by name. This module never
//! inherits that search path into a build: it creates an owned directory
//! containing only measured symlinks to caller-selected absolute executables.
//! Standalone C/C++ probes deliberately do not use this closure and retain
//! their poisoned `PATH`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aros_common::Sha256Digest;

use super::{checked_directory, checked_executable, measure_executable};
use crate::filesystem::open_directory;
use crate::ContractError;

const MAX_HOST_TOOLS: usize = 64;

/// Exact command roles admitted to the sealed native-compatibility closure.
///
/// The CMake consumer compiles source-owned host utilities, applies declared
/// source patches, and configures an unmodified upstream tree.  The latter
/// invokes the standard Autoconf and Make utility set by basename.  Keep this
/// list explicit and platform-neutral: the caller measures one absolute
/// executable for every role before a child process starts; the closure never
/// inherits a directory from the runner's ambient `PATH`.
///
/// `make` deliberately names the role rather than the selected executable, so
/// macOS may expose Homebrew `gmake` as `make` without changing the child
/// contract.  Likewise, `cc` remains the source-owned host-compiler spelling
/// while `as`, `ld`, `ar`, and `ranlib` provide its explicitly measured
/// binutils dependencies.  The current upstream `configure` also rejects a
/// closure without `aclocal` and `automake`, even though it merely discovers
/// those Autotools programs during configuration. It also requires the host
/// `strip`, `uniq`, and Netpbm conversion programs. macOS has two additional
/// SDK-discovery commands; see [`native_compatibility_host_tools`].
pub const REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS: &[&str] = &[
    "aclocal",
    "ar",
    "as",
    "automake",
    "awk",
    "basename",
    "bison",
    "c++",
    "cat",
    "cc",
    "chmod",
    "cmp",
    "cp",
    "cut",
    "date",
    "diff",
    "dirname",
    "echo",
    "egrep",
    "expr",
    "false",
    "fgrep",
    "file",
    "find",
    "flex",
    "gawk",
    "grep",
    "head",
    "id",
    "install",
    "ld",
    "ln",
    "ls",
    "m4",
    "make",
    "mkdir",
    "mv",
    "patch",
    "perl",
    "pngtopnm",
    "ppmtoilbm",
    "printf",
    "pwd",
    "python3",
    "ranlib",
    "rm",
    "sed",
    "sh",
    "sleep",
    "sort",
    "strip",
    "tail",
    "test",
    "touch",
    "tr",
    "true",
    "uname",
    "uniq",
    "wc",
    "xargs",
];

const MACOS_NATIVE_COMPATIBILITY_HOST_TOOLS: &[&str] = &["xcode-select", "xcrun"];

/// Return the exact closed native-compatibility command roles for one v1 host.
///
/// The AROS upstream `configure` invokes `xcode-select` and `xcrun` only on
/// Darwin. Requiring either program on Linux would make the measured closure
/// reject an otherwise valid Linux runner before compatibility begins. The
/// host selector is deliberately validated against the released v1 matrix so
/// an unknown platform cannot silently receive the wrong closure.
pub fn native_compatibility_host_tools(host: &str) -> Result<Vec<&'static str>, ContractError> {
    if !crate::release_index::V1_HOSTS.contains(&host) {
        return Err(ContractError::compatibility(format!(
            "native compatibility host-tool closure does not support host '{host}'"
        )));
    }

    let mut roles = REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.to_vec();
    if host.starts_with("macos-") {
        roles.extend_from_slice(MACOS_NATIVE_COMPATIBILITY_HOST_TOOLS);
    }
    Ok(roles)
}

/// One explicitly selected upstream host-command role and executable.
#[derive(Debug, Clone)]
pub struct CompatibilityHostTool {
    /// Name exposed below the owned closure directory, such as `sed` or `make`.
    pub name: String,
    /// Absolute selected host executable. It is canonicalized and measured.
    pub program: PathBuf,
}

/// Inputs for a fresh measured host-command closure.
#[derive(Debug, Clone)]
pub struct HostToolClosureRequest {
    /// Absent owned directory to create; it is never adopted or repaired.
    pub output_root: PathBuf,
    /// Exact command names allowed through the closure.
    pub tools: Vec<CompatibilityHostTool>,
}

/// Measured identity of one executable admitted to an owned closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostToolIdentity {
    /// Canonical regular executable target.
    pub program: PathBuf,
    /// SHA-256 of the selected executable bytes.
    pub sha256: Sha256Digest,
    /// Exact selected executable size.
    pub size: u64,
}

/// Owned, revalidatable host-command closure for CMake, configure, and Make
/// phases.
#[derive(Debug, Clone)]
pub struct HostToolClosure {
    /// Canonical owned directory to use as the complete child `PATH`.
    pub root: PathBuf,
    /// Measured executable identities keyed by their exposed command names.
    pub tools: BTreeMap<String, HostToolIdentity>,
    /// Open no-follow directory description that prevents inode reuse after
    /// replacement and binds subsequent path checks to the originally created
    /// closure directory. The descriptor is CLOEXEC and never reaches a child.
    root_handle: Arc<File>,
    directory_identity: DirectoryIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

/// Create a fresh exact host-command closure without executing a command.
///
/// The caller supplies the roles reachable by a particular upstream phase;
/// arbitrary ambient `PATH` directories are neither copied nor appended. Each
/// destination is a symlink below `output_root` to the canonical regular
/// executable whose digest is recorded. A later phase must call
/// [`HostToolClosure::revalidate`] before it executes configure or Make.
///
/// # Errors
///
/// Returns AX0703 for unsafe names, duplicate roles, absent/reused roots or
/// mutable executables. It never executes a host tool, runs a shell, accesses
/// the network, modifies a source tree, or performs release work.
pub fn prepare_host_tool_closure(
    request: &HostToolClosureRequest,
) -> Result<HostToolClosure, ContractError> {
    if request.tools.is_empty() || request.tools.len() > MAX_HOST_TOOLS {
        return Err(ContractError::compatibility(
            "compatibility host-tool closure must contain one to 64 explicit tools",
        ));
    }
    let output_root = checked_absent_root(&request.output_root)?;
    let mut names = BTreeSet::new();
    let mut tools = BTreeMap::new();
    for tool in &request.tools {
        if !valid_tool_name(&tool.name) || !names.insert(tool.name.clone()) {
            return Err(ContractError::compatibility(
                "compatibility host-tool closure contains an unsafe or duplicate command name",
            ));
        }
        let program = checked_executable(&tool.program)?;
        let (sha256, size) = measure_executable(&program)?;
        tools.insert(
            tool.name.clone(),
            HostToolIdentity {
                program,
                sha256,
                size,
            },
        );
    }

    fs::create_dir(&output_root).map_err(|_| {
        ContractError::compatibility(
            "cannot create the fresh compatibility host-tool closure directory",
        )
    })?;
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700)).map_err(|_| {
        ContractError::compatibility(
            "cannot restrict the compatibility host-tool closure directory to its owner",
        )
    })?;
    let output_root = checked_directory(&output_root, "compatibility host-tool closure root")?;
    let root_handle = Arc::new(open_directory(&output_root).map_err(|_| {
        ContractError::compatibility(
            "cannot retain a no-follow directory description for the compatibility host-tool closure",
        )
    })?);
    let directory_identity = private_directory_identity(&output_root, &root_handle)?;
    for (name, identity) in &tools {
        let destination = output_root.join(name);
        symlink(&identity.program, &destination).map_err(|_| {
            ContractError::compatibility(
                "cannot materialize a measured compatibility host-tool closure entry",
            )
        })?;
    }
    let closure = HostToolClosure {
        root: output_root,
        tools,
        root_handle,
        directory_identity,
    };
    closure.revalidate()?;
    Ok(closure)
}

impl HostToolClosure {
    /// Recheck every closure symlink and executable identity before a child starts.
    ///
    /// # Errors
    ///
    /// Returns AX0703 when a path, link or executable changed. It does not
    /// execute a command or alter closure material.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        let root = checked_directory(&self.root, "compatibility host-tool closure root")?;
        if root != self.root || self.tools.is_empty() || self.tools.len() > MAX_HOST_TOOLS {
            return Err(ContractError::compatibility(
                "compatibility host-tool closure root or entry count changed after preparation",
            ));
        }
        if private_directory_identity(&root, &self.root_handle)? != self.directory_identity {
            return Err(ContractError::compatibility(
                "compatibility host-tool closure directory changed after preparation",
            ));
        }
        let actual = fs::read_dir(&root)
            .map_err(|_| {
                ContractError::compatibility(
                    "compatibility host-tool closure entries cannot be enumerated",
                )
            })?
            .map(|entry| {
                entry
                    .map_err(|_| {
                        ContractError::compatibility(
                            "compatibility host-tool closure entry cannot be read",
                        )
                    })?
                    .file_name()
                    .into_string()
                    .map_err(|_| {
                        ContractError::compatibility(
                            "compatibility host-tool closure contains a non-UTF-8 entry name",
                        )
                    })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if actual != self.tools.keys().cloned().collect() {
            return Err(ContractError::compatibility(
                "compatibility host-tool closure contains unmeasured or missing entries",
            ));
        }
        for (name, expected) in &self.tools {
            let destination = root.join(name);
            let metadata = fs::symlink_metadata(&destination).map_err(|_| {
                ContractError::compatibility(
                    "compatibility host-tool closure entry cannot be inspected",
                )
            })?;
            if !metadata.file_type().is_symlink()
                || fs::read_link(&destination).ok().as_deref() != Some(expected.program.as_path())
            {
                return Err(ContractError::compatibility(
                    "compatibility host-tool closure entry changed after preparation",
                ));
            }
            let program = checked_executable(&expected.program)?;
            let (sha256, size) = measure_executable(&program)?;
            if program != expected.program || sha256 != expected.sha256 || size != expected.size {
                return Err(ContractError::compatibility(
                    "compatibility host-tool executable changed after preparation",
                ));
            }
        }
        Ok(())
    }
}

fn checked_absent_root(path: &Path) -> Result<PathBuf, ContractError> {
    let absent = matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if !path.is_absolute() || path.file_name().is_none() || !absent {
        return Err(ContractError::compatibility(
            "compatibility host-tool closure root must be an absent absolute leaf directory",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ContractError::compatibility("compatibility host-tool closure root has no parent")
    })?;
    let parent = checked_directory(parent, "compatibility host-tool closure parent")?;
    let name = path.file_name().ok_or_else(|| {
        ContractError::compatibility("compatibility host-tool closure root has no leaf name")
    })?;
    Ok(parent.join(name))
}

fn private_directory_identity(
    path: &Path,
    handle: &File,
) -> Result<DirectoryIdentity, ContractError> {
    let path_metadata = fs::metadata(path).map_err(|_| {
        ContractError::compatibility("cannot inspect the compatibility host-tool closure root")
    })?;
    let handle_metadata = handle.metadata().map_err(|_| {
        ContractError::compatibility(
            "cannot inspect the retained compatibility host-tool closure directory",
        )
    })?;
    if !path_metadata.is_dir()
        || !handle_metadata.is_dir()
        || path_metadata.permissions().mode() & 0o077 != 0
        || handle_metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ContractError::compatibility(
            "compatibility host-tool closure root is not a private directory",
        ));
    }
    if path_metadata.dev() != handle_metadata.dev() || path_metadata.ino() != handle_metadata.ino()
    {
        return Err(ContractError::compatibility(
            "compatibility host-tool closure directory was replaced after preparation",
        ));
    }
    Ok(DirectoryIdentity {
        device: handle_metadata.dev(),
        inode: handle_metadata.ino(),
    })
}

pub(super) fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    use super::{
        native_compatibility_host_tools, prepare_host_tool_closure, CompatibilityHostTool,
        HostToolClosureRequest, REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS,
    };

    fn executable(root: &std::path::Path, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = root.join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn closure_measures_exact_tools_and_rejects_changed_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = temporary.path().join("inputs");
        fs::create_dir(&inputs).unwrap();
        let shell = executable(&inputs, "shell", b"#!/bin/sh\nexit 0\n");
        let make = executable(&inputs, "make", b"#!/bin/sh\nexit 0\n");
        let closure = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("closure"),
            tools: vec![
                CompatibilityHostTool {
                    name: "sh".into(),
                    program: shell,
                },
                CompatibilityHostTool {
                    name: "make".into(),
                    program: make,
                },
            ],
        })
        .unwrap();
        assert_eq!(closure.tools.len(), 2);
        assert!(closure.root.join("sh").is_symlink());
        closure.revalidate().unwrap();

        fs::remove_file(closure.root.join("make")).unwrap();
        symlink("/nonexistent", closure.root.join("make")).unwrap();
        assert!(closure.revalidate().is_err());
    }

    #[test]
    fn closure_rejects_unsafe_names_duplicate_roles_and_reused_roots() {
        let temporary = tempfile::tempdir().unwrap();
        let program = executable(temporary.path(), "tool", b"#!/bin/sh\nexit 0\n");
        let request = HostToolClosureRequest {
            output_root: temporary.path().join("closure"),
            tools: vec![
                CompatibilityHostTool {
                    name: "../bad".into(),
                    program: program.clone(),
                },
                CompatibilityHostTool {
                    name: "../bad".into(),
                    program,
                },
            ],
        };
        assert!(prepare_host_tool_closure(&request).is_err());
        fs::create_dir(temporary.path().join("reused")).unwrap();
        assert!(prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("reused"),
            tools: vec![CompatibilityHostTool {
                name: "sh".into(),
                program: temporary.path().join("tool"),
            }],
        })
        .is_err());
    }

    #[test]
    fn closure_rejects_extra_entries_permissive_permissions_and_root_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let program = executable(temporary.path(), "tool", b"#!/bin/sh\nexit 0\n");
        let closure = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("closure"),
            tools: vec![CompatibilityHostTool {
                name: "tool".into(),
                program,
            }],
        })
        .unwrap();
        fs::write(closure.root.join("injected"), b"unexpected\n").unwrap();
        assert!(closure.revalidate().is_err());
        fs::remove_file(closure.root.join("injected")).unwrap();
        fs::set_permissions(&closure.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(closure.revalidate().is_err());
        fs::set_permissions(&closure.root, fs::Permissions::from_mode(0o700)).unwrap();
        closure.revalidate().unwrap();

        let selected = closure.tools["tool"].program.clone();
        fs::remove_file(closure.root.join("tool")).unwrap();
        fs::remove_dir(&closure.root).unwrap();
        fs::create_dir(&closure.root).unwrap();
        fs::set_permissions(&closure.root, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(selected, closure.root.join("tool")).unwrap();
        assert!(closure.revalidate().is_err());
    }

    #[test]
    fn required_roles_include_gnu_awk_for_upstream_configure() {
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"gawk"));
    }

    #[test]
    fn required_roles_include_autotools_required_by_upstream_configure() {
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"aclocal"));
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"automake"));
    }

    #[test]
    fn required_roles_include_unconditional_upstream_configure_tools() {
        for role in ["strip", "uniq", "pngtopnm", "ppmtoilbm"] {
            assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&role));
        }
    }

    #[test]
    fn required_roles_include_macos_sdk_tools_required_by_upstream_configure() {
        let macos = native_compatibility_host_tools("macos-aarch64").unwrap();
        assert!(macos.contains(&"xcode-select"));
        assert!(macos.contains(&"xcrun"));

        let linux = native_compatibility_host_tools("linux-x86_64").unwrap();
        assert!(!linux.contains(&"xcode-select"));
        assert!(!linux.contains(&"xcrun"));
    }

    #[test]
    fn required_roles_reject_unknown_host_selectors() {
        assert!(native_compatibility_host_tools("freebsd-x86_64").is_err());
    }
}
