//! Canonical inventory for a toolchain payload tree.
//!
//! The inventory is a deliberately small shared primitive: it defines the
//! payload tree digest used by both a release producer and a release consumer.
//! It does not read archives, install files, or publish assets.  Those
//! operations must use this module rather than each serializing an equivalent
//! inventory independently.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::toolchain_manifest::{ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_FILE};
use crate::{finish_sha256, measure_regular_file, sha256_bytes};

/// Failure while measuring a canonical toolchain payload inventory.
#[derive(Debug, Error)]
pub enum ToolchainInventoryError {
    /// The filesystem could not be read without following an unsafe object.
    #[error("cannot read toolchain tree '{path}': {source}")]
    Read {
        /// Path that could not be inspected.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },

    /// A source pathname cannot be represented by the portable payload
    /// contract.
    #[error("toolchain inventory path is not portable: '{path}' ({message})")]
    Path {
        /// Offending relative path.
        path: PathBuf,
        /// Reason the path is not portable.
        message: String,
    },

    /// A symbolic-link target cannot be represented in the UTF-8 manifest.
    #[error("toolchain symbolic-link target is not UTF-8: '{path}'")]
    SymlinkTarget {
        /// Link target as supplied by the filesystem.
        path: PathBuf,
    },

    /// The tree changed between its no-follow measurement and metadata check.
    #[error("toolchain payload file changed while it was inventoried: '{path}'")]
    Changed {
        /// Relative payload path.
        path: PathBuf,
    },

    /// A special filesystem object is not a portable toolchain payload entry.
    #[error("toolchain payload contains unsupported entry '{path}'")]
    Unsupported {
        /// Relative payload path.
        path: PathBuf,
    },

    /// The canonical JSON representation could not be created.
    #[error("cannot serialize canonical toolchain inventory: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Compute the canonical digest and sorted inventory for a toolchain payload.
///
/// The embedded [`AROS_TOOLCHAIN_MANIFEST_FILE`] is omitted to avoid a
/// self-referential digest cycle.
///
/// # Errors
///
/// Returns an error when the tree is missing, contains unsupported entries,
/// changes while being measured, or cannot be represented portably.
pub fn toolchain_tree_inventory(
    root: &Path,
) -> Result<(String, Vec<ArosToolchainManifestEntry>), ToolchainInventoryError> {
    toolchain_tree_inventory_excluding(root, &[AROS_TOOLCHAIN_MANIFEST_FILE])
}

/// Compute the canonical digest and sorted inventory while omitting explicit
/// payload subtrees.
///
/// Exclusions are portable relative paths.  Each exclusion removes that node
/// and all descendants.  This supports self-describing payloads without
/// letting their receipt hash itself.
///
/// # Errors
///
/// Returns an error when an exclusion is unsafe, the tree is unreadable, or a
/// payload object cannot be represented by the canonical inventory contract.
pub fn toolchain_tree_inventory_excluding(
    root: &Path,
    exclusions: &[&str],
) -> Result<(String, Vec<ArosToolchainManifestEntry>), ToolchainInventoryError> {
    let exclusions = exclusions
        .iter()
        .map(|value| {
            let path = PathBuf::from(value);
            portable_relative_path(&path)?;
            Ok::<PathBuf, ToolchainInventoryError>(path)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut entries = Vec::new();
    collect_tree_entries(root, Path::new(""), &exclusions, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((toolchain_inventory_sha256(&entries)?, entries))
}

/// Hash already-normalized toolchain inventory entries using canonical JSON
/// lines.
///
/// This is public for producer read-back checks and golden vectors.  Callers
/// must supply the same path-sorted, validated inventory emitted by
/// [`toolchain_tree_inventory`].
///
/// # Errors
///
/// Returns an error only when canonical JSON serialization fails.
pub fn toolchain_inventory_sha256(
    entries: &[ArosToolchainManifestEntry],
) -> Result<String, ToolchainInventoryError> {
    let mut tree = Sha256::new();
    for entry in entries {
        tree.update(serde_json::to_vec(&canonical_entry(entry))?);
        tree.update(b"\n");
    }
    Ok(finish_sha256(tree).to_string())
}

fn collect_tree_entries(
    root: &Path,
    relative: &Path,
    exclusions: &[PathBuf],
    output: &mut Vec<ArosToolchainManifestEntry>,
) -> Result<(), ToolchainInventoryError> {
    let directory = root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .map_err(|source| ToolchainInventoryError::Read {
            path: directory.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ToolchainInventoryError::Read {
            path: directory.clone(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let entry_path = entry.path();
        let child_relative = relative.join(entry.file_name());
        if exclusions
            .iter()
            .any(|excluded| child_relative == *excluded || child_relative.starts_with(excluded))
        {
            continue;
        }
        let metadata =
            fs::symlink_metadata(&entry_path).map_err(|source| ToolchainInventoryError::Read {
                path: entry_path.clone(),
                source,
            })?;
        let path = portable_relative_path(&child_relative)?;
        if metadata.file_type().is_symlink() {
            let target =
                fs::read_link(&entry_path).map_err(|source| ToolchainInventoryError::Read {
                    path: entry_path,
                    source,
                })?;
            let target = target
                .to_str()
                .ok_or_else(|| ToolchainInventoryError::SymlinkTarget {
                    path: target.clone(),
                })?;
            output.push(ArosToolchainManifestEntry {
                path,
                mode: "0777".into(),
                kind: "symlink".into(),
                sha256: None,
                size: None,
                target: Some(target.into()),
            });
        } else if metadata.is_dir() {
            output.push(ArosToolchainManifestEntry {
                path,
                mode: "0755".into(),
                kind: "directory".into(),
                sha256: None,
                size: None,
                target: None,
            });
            collect_tree_entries(root, &child_relative, exclusions, output)?;
        } else if metadata.is_file() {
            let Some((_, contents)) = measure_regular_file(&entry_path).map_err(|source| {
                ToolchainInventoryError::Read {
                    path: entry_path.clone(),
                    source,
                }
            })?
            else {
                return Err(ToolchainInventoryError::Changed {
                    path: child_relative,
                });
            };
            let measured = fs::symlink_metadata(&entry_path).map_err(|source| {
                ToolchainInventoryError::Read {
                    path: entry_path.clone(),
                    source,
                }
            })?;
            if !measured.is_file()
                || measured.file_type().is_symlink()
                || measured.len()
                    != u64::try_from(contents.len())
                        .map_err(io::Error::other)
                        .map_err(|source| ToolchainInventoryError::Read {
                            path: entry_path.clone(),
                            source,
                        })?
            {
                return Err(ToolchainInventoryError::Changed {
                    path: child_relative,
                });
            }
            output.push(ArosToolchainManifestEntry {
                path,
                mode: format!("{:04o}", normalized_toolchain_file_mode(&measured)),
                kind: "file".into(),
                sha256: Some(sha256_bytes(&contents).to_string()),
                size: Some(measured.len()),
                target: None,
            });
        } else {
            return Err(ToolchainInventoryError::Unsupported {
                path: child_relative,
            });
        }
    }
    Ok(())
}

fn canonical_entry(
    entry: &ArosToolchainManifestEntry,
) -> BTreeMap<&'static str, serde_json::Value> {
    let mut object = BTreeMap::new();
    object.insert("mode", serde_json::Value::String(entry.mode.clone()));
    object.insert("path", serde_json::Value::String(entry.path.clone()));
    if let Some(sha256) = &entry.sha256 {
        object.insert("sha256", serde_json::Value::String(sha256.clone()));
    }
    if let Some(size) = entry.size {
        object.insert("size", serde_json::Value::Number(size.into()));
    }
    if let Some(target) = &entry.target {
        object.insert("target", serde_json::Value::String(target.clone()));
    }
    object.insert("type", serde_json::Value::String(entry.kind.clone()));
    object
}

fn portable_relative_path(path: &Path) -> Result<String, ToolchainInventoryError> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => {
                value
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ToolchainInventoryError::Path {
                        path: path.to_path_buf(),
                        message: "path is not valid UTF-8".into(),
                    })
            }
            _ => Err(ToolchainInventoryError::Path {
                path: path.to_path_buf(),
                message: "path is not relative".into(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(components.join("/"))
}

/// Return the portable mode encoded in a toolchain inventory or restored
/// during extraction.
///
/// Regular files have only two portable states: non-executable (`0644`) and
/// executable (`0755`). Directories and symbolic links have fixed contract
/// modes in the inventory itself.
#[cfg(unix)]
#[must_use]
pub fn normalized_toolchain_file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode() & 0o111 == 0 {
        0o644
    } else {
        0o755
    }
}

/// Return the portable mode encoded in a toolchain inventory or restored
/// during extraction.
#[cfg(not(unix))]
#[must_use]
pub fn normalized_toolchain_file_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o644
    } else {
        0o755
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_language_neutral_tree_digest_vector() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/tree-digest-v1.fixture.json"
        ))
        .unwrap();
        assert_eq!(fixture["schema"], "aros-toolchain-tree-digest-fixture-v1");
        let entries: Vec<ArosToolchainManifestEntry> =
            serde_json::from_value(fixture["entries"].clone()).unwrap();
        let expected = fixture["tree_sha256"].as_str().unwrap();
        assert_eq!(
            toolchain_inventory_sha256(&entries).unwrap(),
            "11cbd45962f89c54c02fc9c1ae55eb283774b76425c08564da060bd5ca9c840b"
        );
        assert_eq!(toolchain_inventory_sha256(&entries).unwrap(), expected);
    }

    #[test]
    fn exclusion_removes_a_complete_metadata_subtree() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".metadata/nested")).unwrap();
        fs::write(root.path().join("payload"), b"payload").unwrap();
        fs::write(root.path().join(".metadata/nested/receipt"), b"receipt").unwrap();

        let (digest, entries) =
            toolchain_tree_inventory_excluding(root.path(), &[".metadata"]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "payload");
        assert_eq!(digest, toolchain_inventory_sha256(&entries).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn matches_producer_known_answer_with_unicode_and_symlink() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in [
            "bin",
            "include/c++/v1",
            "lib/clang/11.0.0/lib/aros",
            "share/Größe",
        ] {
            fs::create_dir_all(root.join(path)).unwrap();
        }

        let mock_tool = include_bytes!("../tests/fixtures/mock-tool.sh");
        for tool in [
            "clang",
            "clang++",
            "ld.lld",
            "llvm-ar",
            "llvm-ranlib",
            "llvm-nm",
            "llvm-strip",
            "llvm-objcopy",
            "llvm-objdump",
        ] {
            let path = root.join("bin").join(tool);
            fs::write(&path, mock_tool).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fs::write(
            root.join("include/c++/v1/vector"),
            b"// deterministic producer fixture\n",
        )
        .unwrap();
        for library in ["libc++.a", "libc++abi.a", "libunwind.a"] {
            fs::write(
                root.join("lib").join(library),
                format!("fixture {library}\n"),
            )
            .unwrap();
        }
        fs::write(
            root.join("lib/clang/11.0.0/lib/aros/libclang_rt.builtins-x86_64.a"),
            b"fixture x86_64 builtins\n",
        )
        .unwrap();
        fs::write(
            root.join("lib/clang/11.0.0/lib/aros/libclang_rt.builtins-i386.a"),
            b"fixture i386 builtins\n",
        )
        .unwrap();
        fs::write(
            root.join("share/Größe/marker-ä.txt"),
            b"UTF-8 inventory fixture\n",
        )
        .unwrap();
        symlink("../include/c++/v1/vector", root.join("share/vector-link")).unwrap();

        assert_eq!(
            toolchain_tree_inventory(root).unwrap().0,
            "4f78bdbc52ffbab2c6b337bb47d8c40b716574a82a03c2b0ac031ecca16fecef"
        );
    }
}
