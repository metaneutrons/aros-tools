//! A bounded local observation of one configured Mesa source inventory.
//!
//! This records filesystem state for later consistency checking. It is not
//! evidence that the inventory or the configuration which selected it is
//! authentic.

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

const INVENTORY_FILE_NAME: &str = "Makefile.sources";
const MAX_INVENTORY_BYTES: u64 = 1024 * 1024;

/// A bounded observation of the exact configured Mesa inventory file.
///
/// The original lexical ports path is retained so [`Self::verify`] resolves
/// the same selection again. The canonical ports root and inventory parent
/// are retained alongside the file's exact contents for equality checking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryObservation {
    path: PathBuf,
    selected_ports: PathBuf,
    ports_root: PathBuf,
    parent: PathBuf,
    contents: Option<Vec<u8>>,
}

impl InventoryObservation {
    /// Capture one inventory file below the selected ports directory.
    ///
    /// Only a missing final file is represented as an absent inventory. The
    /// selected ports directory and the file's parent must already exist.
    ///
    /// # Errors
    /// Rejects unsafe paths, unavailable parents, non-regular files, read
    /// failures, oversized inputs or a changing filesystem observation.
    pub fn capture(path: &Path, selected_ports: &Path) -> Result<Self, String> {
        require_normalized_absolute(path, "inventory path")?;
        require_normalized_absolute(selected_ports, "selected ports path")?;

        if path.file_name() != Some(OsStr::new(INVENTORY_FILE_NAME)) {
            return Err(format!(
                "Mesa inventory path must name exactly {INVENTORY_FILE_NAME}"
            ));
        }

        let selected_ports = selected_ports.to_path_buf();
        let ports_root = fs::canonicalize(&selected_ports)
            .map_err(|error| format!("cannot resolve selected ports directory: {error}"))?;
        if !fs::metadata(&ports_root)
            .map_err(|error| format!("cannot inspect selected ports directory: {error}"))?
            .is_dir()
        {
            return Err("selected ports path is not a directory".into());
        }

        let parent = path
            .parent()
            .ok_or_else(|| "Mesa inventory path has no parent directory".to_owned())?;
        let canonical_parent = fs::canonicalize(parent)
            .map_err(|error| format!("cannot resolve Mesa inventory parent: {error}"))?;
        if !fs::metadata(&canonical_parent)
            .map_err(|error| format!("cannot inspect Mesa inventory parent: {error}"))?
            .is_dir()
        {
            return Err("Mesa inventory parent is not a directory".into());
        }
        if canonical_parent != parent {
            return Err("Mesa inventory parent is not a canonical lexical path".into());
        }
        if !canonical_parent.starts_with(&ports_root) {
            return Err("Mesa inventory parent is outside the selected ports directory".into());
        }

        let contents = match fs::symlink_metadata(path) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    return Err("Mesa inventory file must not be a symlink".into());
                }
                if !file_type.is_file() {
                    return Err("Mesa inventory path is not a regular file".into());
                }
                match aros_common::measure_regular_file_bounded(path, MAX_INVENTORY_BYTES)
                    .map_err(|error| format!("cannot read Mesa inventory file: {error}"))?
                {
                    Some((_, bytes)) => Some(bytes),
                    None => confirm_missing_final_file(path, parent, &canonical_parent)?,
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                confirm_missing_final_file(path, parent, &canonical_parent)?
            }
            Err(error) => {
                return Err(format!("cannot inspect Mesa inventory file: {error}"));
            }
        };

        Ok(Self {
            path: path.to_path_buf(),
            selected_ports,
            ports_root,
            parent: canonical_parent,
            contents,
        })
    }

    /// Return whether the inventory file existed when this observation was captured.
    #[must_use]
    pub const fn present(&self) -> bool {
        self.contents.is_some()
    }

    /// Re-capture the exact path and selected ports path and require equality.
    ///
    /// # Errors
    /// Rejects changed root selection, file presence/content or unsafe paths.
    pub fn verify(&self) -> Result<(), String> {
        let current = Self::capture(&self.path, &self.selected_ports)?;
        if current == *self {
            Ok(())
        } else {
            Err("Mesa inventory observation changed".into())
        }
    }
}

fn require_normalized_absolute(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be absolute"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(format!(
                    "{label} must contain only root and normal components"
                ));
            }
        }
    }
    if normalized.as_os_str() != path.as_os_str() {
        return Err(format!("{label} must be lexically normalized"));
    }
    Ok(())
}

fn confirm_missing_final_file(
    path: &Path,
    parent: &Path,
    expected_parent: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let current_parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot confirm Mesa inventory parent: {error}"))?;
    if current_parent != expected_parent {
        return Err("Mesa inventory parent changed while capturing observation".into());
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Ok(_) => Err("Mesa inventory file changed while capturing observation".into()),
        Err(error) => Err(format!(
            "cannot confirm missing Mesa inventory file: {error}"
        )),
    }
}

#[cfg(test)]
#[path = "configured_inventory_tests.rs"]
mod tests;
