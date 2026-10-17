//! Fixed, checked working directory for Cargo subprocesses.
//!
//! Cargo searches ancestor directories of its current working directory for
//! configuration. Producer Cargo commands therefore run from `/`, and this
//! guard excludes either supported root-level Cargo configuration filename.

use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::ContractError;

const ROOT_CARGO_DIRECTORY: &str = ".cargo";
const ROOT_CARGO_CONFIGURATIONS: [&str; 2] = ["config.toml", "config"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    owner: u32,
    group: u32,
    mode: u32,
    ctime_seconds: i64,
    ctime_nanoseconds: i64,
}

impl From<&Metadata> for DirectoryIdentity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner: metadata.uid(),
            group: metadata.gid(),
            mode: metadata.mode(),
            ctime_seconds: metadata.ctime(),
            ctime_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

/// A fixed Cargo cwd whose ancestor configuration state has been checked.
#[derive(Debug, Clone)]
pub struct CargoWorkingDirectory {
    root: PathBuf,
    root_identity: DirectoryIdentity,
    cargo_directory: Option<DirectoryIdentity>,
}

impl CargoWorkingDirectory {
    /// Select the fixed Unix filesystem root and reject root Cargo config.
    pub(crate) fn prepare() -> Result<Self, ContractError> {
        Self::inspect(PathBuf::from("/"))
    }

    /// Return the fixed cwd for a Cargo child.
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// Recheck the root and root-level Cargo configuration immediately before
    /// execution and successful output adoption.
    pub(crate) fn revalidate(&self) -> Result<(), ContractError> {
        let root_identity = real_directory_identity(&self.root)?;
        if root_identity != self.root_identity {
            return Err(ContractError::environment(
                "fixed Cargo working directory changed after validation",
            ));
        }
        let cargo_directory = checked_cargo_directory(&self.root, root_identity)?;
        reject_root_configurations(&self.root)?;
        if cargo_directory != self.cargo_directory {
            return Err(ContractError::environment(
                "root Cargo configuration directory changed after validation",
            ));
        }
        Ok(())
    }

    fn inspect(root: PathBuf) -> Result<Self, ContractError> {
        if !root.is_absolute() {
            return Err(ContractError::environment(
                "fixed Cargo working directory must be absolute",
            ));
        }
        let root_identity = real_directory_identity(&root)?;
        if root_identity.mode & 0o022 != 0 {
            return Err(ContractError::environment(
                "fixed Cargo working directory is writable by group or others",
            ));
        }
        let cargo_directory = checked_cargo_directory(&root, root_identity)?;
        reject_root_configurations(&root)?;
        Ok(Self {
            root,
            root_identity,
            cargo_directory,
        })
    }

    #[cfg(test)]
    fn prepare_at(root: &Path) -> Result<Self, ContractError> {
        let root = root
            .canonicalize()
            .map_err(|_| ContractError::environment("test Cargo root cannot be resolved"))?;
        Self::inspect(root)
    }
}

fn real_directory_identity(path: &Path) -> Result<DirectoryIdentity, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::environment("cannot inspect fixed Cargo working directory"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ContractError::environment(
            "fixed Cargo working directory is not a real directory",
        ));
    }
    Ok(DirectoryIdentity::from(&metadata))
}

fn checked_cargo_directory(
    root: &Path,
    root_identity: DirectoryIdentity,
) -> Result<Option<DirectoryIdentity>, ContractError> {
    let path = root.join(ROOT_CARGO_DIRECTORY);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(ContractError::environment(
                "cannot inspect root Cargo configuration directory",
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ContractError::environment(
            "root Cargo configuration parent is not a real directory",
        ));
    }
    let identity = DirectoryIdentity::from(&metadata);
    if identity.owner != root_identity.owner || identity.mode & 0o022 != 0 {
        return Err(ContractError::environment(
            "root Cargo configuration parent is untrusted or writable by group or others",
        ));
    }
    Ok(Some(identity))
}

fn reject_root_configurations(root: &Path) -> Result<(), ContractError> {
    for name in ROOT_CARGO_CONFIGURATIONS {
        let path = root.join(ROOT_CARGO_DIRECTORY).join(name);
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(ContractError::environment(
                    "root Cargo configuration is excluded by the native producer boundary",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(ContractError::environment(
                    "cannot inspect root Cargo configuration",
                ))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn accepts_absent_and_trusted_empty_configuration_parents() {
        let absent = tempfile::tempdir().unwrap();
        let absent_root = absent.path().join("root");
        fs::create_dir(&absent_root).unwrap();
        let absent_guard = CargoWorkingDirectory::prepare_at(&absent_root).unwrap();
        absent_guard.revalidate().unwrap();

        let empty = tempfile::tempdir().unwrap();
        let empty_root = empty.path().join("root");
        fs::create_dir(&empty_root).unwrap();
        let cargo = empty_root.join(ROOT_CARGO_DIRECTORY);
        fs::create_dir(&cargo).unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
        let empty_guard = CargoWorkingDirectory::prepare_at(&empty_root).unwrap();
        empty_guard.revalidate().unwrap();
    }

    #[test]
    fn rejects_modern_and_legacy_root_configuration_names() {
        for name in ROOT_CARGO_CONFIGURATIONS {
            let temporary = tempfile::tempdir().unwrap();
            let cargo = temporary.path().join(ROOT_CARGO_DIRECTORY);
            fs::create_dir(&cargo).unwrap();
            fs::write(cargo.join(name), b"not read").unwrap();
            assert!(CargoWorkingDirectory::prepare_at(temporary.path()).is_err());
        }
    }

    #[test]
    fn rejects_linked_and_non_directory_configuration_parents() {
        let linked = tempfile::tempdir().unwrap();
        let linked_target = linked.path().join("target");
        let linked_root = linked.path().join("root");
        fs::create_dir(&linked_target).unwrap();
        fs::create_dir(&linked_root).unwrap();
        symlink(&linked_target, linked_root.join(ROOT_CARGO_DIRECTORY)).unwrap();
        assert!(CargoWorkingDirectory::prepare_at(&linked_root).is_err());

        let non_directory = tempfile::tempdir().unwrap();
        let cargo = non_directory.path().join(ROOT_CARGO_DIRECTORY);
        fs::write(&cargo, b"not a directory").unwrap();
        assert!(CargoWorkingDirectory::prepare_at(non_directory.path()).is_err());
    }

    #[test]
    fn rejects_group_or_world_writable_configuration_parent() {
        let temporary = tempfile::tempdir().unwrap();
        let cargo = temporary.path().join(ROOT_CARGO_DIRECTORY);
        fs::create_dir(&cargo).unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(CargoWorkingDirectory::prepare_at(temporary.path()).is_err());
    }

    #[test]
    fn revalidation_rejects_parent_or_configuration_changes() {
        let absent = tempfile::tempdir().unwrap();
        let absent_root = absent.path().join("root");
        fs::create_dir(&absent_root).unwrap();
        let absent_guard = CargoWorkingDirectory::prepare_at(&absent_root).unwrap();
        let cargo = absent_root.join(ROOT_CARGO_DIRECTORY);
        fs::create_dir(&cargo).unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(absent_guard.revalidate().is_err());

        let empty = tempfile::tempdir().unwrap();
        let cargo = empty.path().join(ROOT_CARGO_DIRECTORY);
        fs::create_dir(&cargo).unwrap();
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
        let empty_guard = CargoWorkingDirectory::prepare_at(empty.path()).unwrap();
        fs::write(cargo.join("config.toml"), b"[build]").unwrap();
        assert!(empty_guard.revalidate().is_err());
    }
}
