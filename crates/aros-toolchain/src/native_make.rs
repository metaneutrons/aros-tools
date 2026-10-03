//! Bind source-owned recursive `make` calls to the observed GNU Make.
//!
//! MetaMake reads HOST_MAKE from generated source configuration, not the
//! top-level make command. A private PATH alias preserves that source contract
//! without admitting a second, unqualified make executable.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::preflight::HostPreflight;
use crate::source_lock::CompilerFamily;
use crate::ContractError;

/// Select host directories, with an owned GNU-only recursive make alias.
///
/// # Errors
///
/// Rejects missing observations or an existing/unwritable private alias root.
pub fn tool_directories(
    family: CompilerFamily,
    host: &HostPreflight,
    lifecycle_root: &Path,
) -> Result<Vec<PathBuf>, ContractError> {
    let mut directories = host.tool_directories();
    if family == CompilerFamily::Gnu {
        let make = host
            .tools
            .iter()
            .find(|tool| tool.name == "gmake")
            .or_else(|| host.tools.iter().find(|tool| tool.name == "make"))
            .ok_or_else(|| ContractError::prerequisite("native preflight omitted GNU Make"))?;
        let directory = lifecycle_root.join("host-tools");
        fs::create_dir(&directory).map_err(|_| {
            ContractError::environment("cannot reserve private native make directory")
        })?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|_| {
            ContractError::environment("cannot protect private native make directory")
        })?;
        symlink(&make.invocation_path, directory.join("make"))
            .map_err(|_| ContractError::environment("cannot bind private recursive GNU Make"))?;
        directories.insert(0, directory);
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preflight::HostTool;

    fn host(root: &Path) -> HostPreflight {
        HostPreflight {
            host: "macos-aarch64",
            profile: "fixture".into(),
            capabilities: vec![],
            tools: vec![
                HostTool {
                    name: "make",
                    path: root.join("old-make"),
                    invocation_path: root.join("old-make"),
                    version: "GNU Make 3.81".into(),
                },
                HostTool {
                    name: "gmake",
                    path: root.join("selected-gmake"),
                    invocation_path: root.join("selected-gmake"),
                    version: "GNU Make 4.4.1".into(),
                },
            ],
        }
    }

    #[test]
    fn gnu_recursive_alias_uses_selected_make_and_never_replaces_a_leaf() {
        let temp = tempfile::tempdir().unwrap();
        let selected = host(temp.path());
        let directories = tool_directories(CompilerFamily::Gnu, &selected, temp.path()).unwrap();
        assert_eq!(directories[0], temp.path().join("host-tools"));
        assert_eq!(
            fs::read_link(directories[0].join("make")).unwrap(),
            temp.path().join("selected-gmake")
        );
        assert!(tool_directories(CompilerFamily::Gnu, &selected, temp.path()).is_err());
        assert_eq!(
            fs::read_link(directories[0].join("make")).unwrap(),
            temp.path().join("selected-gmake")
        );
    }

    #[test]
    fn llvm_does_not_create_an_alias_and_gnu_requires_an_observed_make() {
        let temp = tempfile::tempdir().unwrap();
        let mut selected = host(temp.path());
        assert_eq!(
            tool_directories(CompilerFamily::Llvm, &selected, temp.path()).unwrap(),
            selected.tool_directories()
        );
        assert!(!temp.path().join("host-tools").exists());
        selected.tools.clear();
        assert!(tool_directories(CompilerFamily::Gnu, &selected, temp.path()).is_err());
        assert!(!temp.path().join("host-tools").exists());
    }
}
