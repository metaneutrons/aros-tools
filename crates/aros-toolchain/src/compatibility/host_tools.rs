//! Measured host-command closures for native compatibility phases that need
//! host command resolution.
//!
//! The AROS CMake engine and upstream `configure`/generated Makefiles
//! necessarily resolve a bounded POSIX tool set by name. This module never
//! inherits that search path into a build: it creates an owned directory with
//! measured symlinks to caller-selected absolute executables, plus two sealed
//! Darwin argv-zero normalization wrappers where upstream requires them.
//! Standalone C/C++ probes deliberately do not use this closure and retain
//! their poisoned `PATH`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aros_common::{open_regular_file_nofollow, Sha256Digest};

use super::{checked_directory, checked_executable, measure_executable};
use crate::filesystem::open_directory;
use crate::ContractError;

const MAX_HOST_TOOLS: usize = 72;

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
/// those Autotools programs during configuration. Its generated MetaMake
/// source later runs `autoconf` through its `autom4te` runner, so both programs
/// are part of the same closure.
/// It also requires the host `strip`, `uniq`, `gcc`, `bash`, the `libpng-config`
/// discovery program, and Netpbm conversion programs. Some checked upstream
/// helper rules retain the literal `gcc` and `/usr/bin/env bash` spellings;
/// they must resolve inside the same closure rather than through an ambient
/// runner path. Its generated MetaMake rules invoke `env` to bind their
/// explicit configuration variables before they build `archtool`.
/// The bzip2 port is unpacked by the selected upstream `fetch.sh`, so `tar`
/// is also a literal requirement of the same sealed child. On Darwin,
/// upstream configure explicitly selects GNU sed as `gsed`.
/// macOS has two SDK-discovery commands and two measured compatibility aliases;
/// see
/// [`native_compatibility_host_tools`].
pub const REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS: &[&str] = &[
    "aclocal",
    "ar",
    "as",
    "autoconf",
    "autom4te",
    "automake",
    "awk",
    "basename",
    "bash",
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
    "env",
    "expr",
    "false",
    "fgrep",
    "file",
    "find",
    "flex",
    "gawk",
    "gcc",
    "grep",
    "head",
    "id",
    "install",
    "ld",
    "libpng-config",
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
    "tar",
    "test",
    "touch",
    "tr",
    "true",
    "uname",
    "uniq",
    "wc",
    "xargs",
];

// The pinned upstream configure script appends the host compiler suffix `cc`
// to its LLVM binutils candidates on macOS.  For Apple Clang this probes the
// historical spellings `llvm-arcc` and `llvm-ranlibcc`.  They are not ambient
// commands: the workflow resolves the underlying Xcode tool paths through the
// separately measured `xcrun` role before this owned closure exposes the
// names to upstream. Pointing these aliases at the `/usr/bin` xcrun shims
// would leak their alias names through argv[0] and make them search for their
// own nonexistent names.
const MACOS_NATIVE_COMPATIBILITY_HOST_TOOLS: &[&str] = &[
    "gsed",
    "llvm-arcc",
    "llvm-ranlibcc",
    "xcode-select",
    "xcrun",
];

/// Return the exact closed native-compatibility command roles for one v1 host.
///
/// The AROS upstream `configure` invokes `xcode-select` and `xcrun` only on
/// Darwin. Requiring either program on Linux would make the measured closure
/// reject an otherwise valid Linux runner before compatibility begins. The
/// host selector is deliberately validated against the released v1 matrix so
/// an unknown platform cannot silently receive the wrong closure.
///
/// # Errors
///
/// Returns AX0703 when `host` is not one of the closed v1 build-host
/// selectors. It performs no filesystem, process, network, credential, tag,
/// or release operation.
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
    /// Original executable name restored by a sealed wrapper when a Darwin
    /// multi-call tool would otherwise inspect the compatibility alias name.
    pub invocation_name: Option<&'static str>,
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
            "compatibility host-tool closure exceeds its explicit 72-tool capacity",
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
                invocation_name: normalized_invocation_name(&tool.name),
            },
        );
    }
    if tools
        .values()
        .any(|identity| identity.invocation_name.is_some())
        && !tools.contains_key("bash")
    {
        return Err(ContractError::compatibility(
            "compatibility host-tool alias wrappers require an explicit bash entry",
        ));
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
        if let Some(invocation_name) = identity.invocation_name {
            write_normalized_alias_wrapper(&destination, &identity.program, invocation_name)?;
        } else {
            symlink(&identity.program, &destination).map_err(|_| {
                ContractError::compatibility(
                    "cannot materialize a measured compatibility host-tool closure entry",
                )
            })?;
        }
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
    /// Recheck every closure entry and executable identity before a child starts.
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
            if let Some(invocation_name) = expected.invocation_name {
                if !valid_normalized_alias_wrapper(
                    &destination,
                    &metadata,
                    &expected.program,
                    invocation_name,
                )? {
                    return Err(ContractError::compatibility(
                        "compatibility host-tool alias wrapper changed after preparation",
                    ));
                }
            } else if !metadata.file_type().is_symlink()
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

fn normalized_invocation_name(name: &str) -> Option<&'static str> {
    match name {
        "llvm-arcc" => Some("ar"),
        "llvm-ranlibcc" => Some("ranlib"),
        _ => None,
    }
}

fn write_normalized_alias_wrapper(
    destination: &Path,
    program: &Path,
    invocation_name: &str,
) -> Result<(), ContractError> {
    let bytes = normalized_alias_wrapper_bytes(program, invocation_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| {
            ContractError::compatibility(
                "cannot create a normalized compatibility host-tool alias wrapper",
            )
        })?;
    file.write_all(&bytes).map_err(|_| {
        ContractError::compatibility(
            "cannot write a normalized compatibility host-tool alias wrapper",
        )
    })?;
    file.sync_all().map_err(|_| {
        ContractError::compatibility(
            "cannot durably write a normalized compatibility host-tool alias wrapper",
        )
    })?;
    drop(file);
    fs::set_permissions(destination, fs::Permissions::from_mode(0o500)).map_err(|_| {
        ContractError::compatibility(
            "cannot seal a normalized compatibility host-tool alias wrapper",
        )
    })
}

fn valid_normalized_alias_wrapper(
    destination: &Path,
    metadata: &fs::Metadata,
    program: &Path,
    invocation_name: &str,
) -> Result<bool, ContractError> {
    let expected = normalized_alias_wrapper_bytes(program, invocation_name);
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o500
        || metadata.len() != expected.len() as u64
    {
        return Ok(false);
    }
    let mut file = open_regular_file_nofollow(destination).map_err(|_| {
        ContractError::compatibility(
            "cannot safely open a normalized compatibility host-tool alias wrapper",
        )
    })?;
    let opened = file.metadata().map_err(|_| {
        ContractError::compatibility(
            "cannot inspect a normalized compatibility host-tool alias wrapper",
        )
    })?;
    if !opened.is_file()
        || opened.permissions().mode() & 0o777 != 0o500
        || opened.len() != expected.len() as u64
    {
        return Ok(false);
    }
    let mut actual = Vec::with_capacity(expected.len());
    file.read_to_end(&mut actual).map_err(|_| {
        ContractError::compatibility(
            "cannot read a normalized compatibility host-tool alias wrapper",
        )
    })?;
    Ok(actual == expected)
}

fn normalized_alias_wrapper_bytes(program: &Path, invocation_name: &str) -> Vec<u8> {
    format!(
        "#!/usr/bin/env bash\nexec -a {} {} \"$@\"\n",
        shell_quote(invocation_name),
        shell_quote(&program.to_string_lossy()),
    )
    .into_bytes()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
        HostToolClosureRequest, MAX_HOST_TOOLS, REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS,
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
    fn darwin_aliases_are_owned_argv_zero_normalizing_wrappers() {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = temporary.path().join("inputs");
        fs::create_dir(&inputs).unwrap();
        let bash = executable(&inputs, "bash", b"#!/bin/sh\nexit 0\n");
        let ar = executable(&inputs, "ar", b"#!/bin/sh\nexit 0\n");
        let ranlib = executable(&inputs, "ranlib", b"#!/bin/sh\nexit 0\n");
        let closure = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("closure"),
            tools: vec![
                CompatibilityHostTool {
                    name: "bash".into(),
                    program: bash,
                },
                CompatibilityHostTool {
                    name: "llvm-arcc".into(),
                    program: ar,
                },
                CompatibilityHostTool {
                    name: "llvm-ranlibcc".into(),
                    program: ranlib,
                },
            ],
        })
        .unwrap();
        for (name, original_name) in [("llvm-arcc", "ar"), ("llvm-ranlibcc", "ranlib")] {
            let wrapper = closure.root.join(name);
            assert!(!wrapper.is_symlink());
            assert!(String::from_utf8(fs::read(&wrapper).unwrap())
                .unwrap()
                .contains(&format!("exec -a '{original_name}'")));
        }
        closure.revalidate().unwrap();
        let ranlib_wrapper = closure.root.join("llvm-ranlibcc");
        fs::remove_file(&ranlib_wrapper).unwrap();
        fs::write(ranlib_wrapper, b"modified\n").unwrap();
        assert!(closure.revalidate().is_err());
    }

    #[test]
    fn darwin_alias_wrappers_require_a_measured_bash_runner() {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = temporary.path().join("inputs");
        fs::create_dir(&inputs).unwrap();
        let ar = executable(&inputs, "ar", b"#!/bin/sh\nexit 0\n");
        assert!(prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("closure"),
            tools: vec![CompatibilityHostTool {
                name: "llvm-arcc".into(),
                program: ar,
            }],
        })
        .is_err());
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
    fn required_roles_include_autotools_required_by_upstream_configure_and_metamake() {
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"aclocal"));
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"autoconf"));
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"autom4te"));
        assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&"automake"));
    }

    #[test]
    fn every_supported_host_role_set_fits_the_closed_closure_capacity() {
        for host in crate::release_index::V1_HOSTS {
            assert!(native_compatibility_host_tools(host).unwrap().len() <= MAX_HOST_TOOLS);
        }
    }

    #[test]
    fn required_roles_include_unconditional_upstream_make_tools() {
        for role in [
            "bash",
            "env",
            "gcc",
            "strip",
            "uniq",
            "libpng-config",
            "pngtopnm",
            "ppmtoilbm",
            "tar",
        ] {
            assert!(REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS.contains(&role));
        }
    }

    #[test]
    fn required_roles_include_macos_sdk_tools_required_by_upstream_configure() {
        let macos = native_compatibility_host_tools("macos-aarch64").unwrap();
        assert!(macos.contains(&"gsed"));
        assert!(macos.contains(&"llvm-arcc"));
        assert!(macos.contains(&"llvm-ranlibcc"));
        assert!(macos.contains(&"xcode-select"));
        assert!(macos.contains(&"xcrun"));

        let linux = native_compatibility_host_tools("linux-x86_64").unwrap();
        assert!(!linux.contains(&"gsed"));
        assert!(!linux.contains(&"llvm-arcc"));
        assert!(!linux.contains(&"llvm-ranlibcc"));
        assert!(!linux.contains(&"xcode-select"));
        assert!(!linux.contains(&"xcrun"));
    }

    #[test]
    fn required_roles_reject_unknown_host_selectors() {
        assert!(native_compatibility_host_tools("freebsd-x86_64").is_err());
    }
}
