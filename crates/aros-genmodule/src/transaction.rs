//! Generator-specific facade over the shared durable publication transaction.

use aros_common::{publication_journal_path, DurableFileSet, PublicationReceipt, RecoveryOutcome};
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct FileTransaction {
    inner: DurableFileSet,
}

impl FileTransaction {
    #[cfg(test)]
    pub fn for_output_root(output_root: &Path) -> std::io::Result<Self> {
        let absolute = if output_root.is_absolute() {
            output_root.to_path_buf()
        } else {
            std::env::current_dir()?.join(output_root)
        };
        Ok(Self {
            inner: DurableFileSet::new(publication_journal_path(&absolute, "genmodule")?)?,
        })
    }

    /// Create one transaction covering every explicitly selected output.
    ///
    /// AROS-NX places `SDK/include`, generated private headers, link-library
    /// sources, and the library-base inventory in different subdirectories of
    /// one build tree. The required include output defines that stable build
    /// root: `<build>/SDK/include` anchors at `<build>`, while a standalone
    /// `<build>/include` anchors at `<build>`. Optional output switches never
    /// change the journal/lock namespace. Call
    /// [`Self::for_output_paths_with_root`] when the SDK layout needs an
    /// explicitly declared transaction root.
    pub fn for_output_paths(output_inc: &Path, paths: &[&Path]) -> std::io::Result<Self> {
        let output_inc = normalized_absolute(output_inc)?;
        let include_parent = output_inc.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "include output '{}' has no transaction parent",
                    output_inc.display()
                ),
            )
        })?;
        let root = if include_parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("SDK"))
        {
            include_parent.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "SDK include output '{}' has no writable build root",
                        output_inc.display()
                    ),
                )
            })?
        } else {
            include_parent
        };
        Self::for_root(root, paths, Some(&output_inc))
    }

    /// Create a transaction using an explicitly declared stable output root.
    ///
    /// The root is normalized to an absolute path and must not be the
    /// filesystem root. Every output target must be normalized and strictly
    /// below it. Durable publication still validates the root and its parents
    /// without following symlinks before acquiring the journal lock.
    pub fn for_output_paths_with_root(
        output_root: &Path,
        paths: &[&Path],
    ) -> std::io::Result<Self> {
        Self::for_root(output_root, paths, None)
    }

    fn for_root(
        output_root: &Path,
        paths: &[&Path],
        required_output: Option<&Path>,
    ) -> std::io::Result<Self> {
        let root = normalized_absolute(output_root)?;
        if root.parent().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "generated outputs cannot use the filesystem root as their transaction namespace",
            ));
        }
        let absolute = paths
            .iter()
            .map(|path| normalized_absolute(path))
            .collect::<std::io::Result<Vec<_>>>()?;
        if absolute.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "generated-output transaction requires at least one target",
            ));
        }
        if let Some(required_output) = required_output {
            let required_output = normalized_absolute(required_output)?;
            validate_output_below_root(&required_output, &root)?;
        }
        for path in &absolute {
            validate_output_below_root(path, &root)?;
        }
        let anchor = root.join(".aros-genmodule-publication-root");
        Ok(Self {
            inner: DurableFileSet::new(publication_journal_path(&anchor, "genmodule")?)?,
        })
    }

    pub const fn recovery_outcome(&self) -> RecoveryOutcome {
        self.inner.recovery_outcome()
    }

    pub fn stage_write(&mut self, path: &Path, contents: &[u8]) -> std::io::Result<bool> {
        self.inner.stage_write(path, contents)
    }

    pub fn stage_remove(&mut self, path: &Path) -> std::io::Result<bool> {
        self.inner.stage_remove(path)
    }

    pub fn commit(self) -> std::io::Result<PublicationReceipt> {
        self.inner.commit()
    }
}

fn validate_output_below_root(path: &Path, root: &Path) -> std::io::Result<()> {
    let below_root = path.starts_with(root) && path != root;
    if !below_root {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "generated output '{}' must be below stable output root '{}'",
                path.display(),
                root.display()
            ),
        ));
    }
    if path.parent().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("output path '{}' has no transaction parent", path.display()),
        ));
    }
    Ok(())
}

fn normalized_absolute(path: &Path) -> std::io::Result<PathBuf> {
    let has_dot_component = path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || has_embedded_dot_component(path);
    if has_dot_component {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("output path '{}' is not normalized", path.display()),
        ));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir().map(|directory| directory.join(path))
    }
}

#[cfg(unix)]
fn has_embedded_dot_component(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str()
        .as_bytes()
        .split(|byte| *byte == b'/')
        .any(|component| component == b".")
}

#[cfg(not(unix))]
fn has_embedded_dot_component(path: &Path) -> bool {
    path.to_string_lossy()
        .split(|character| character == '/' || character == '\\')
        .any(|component| component == ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn optional_outputs_share_the_required_include_lock_namespace() {
        let root = tempfile::tempdir().unwrap();
        let include = root.path().join("SDK/include");
        let generated = root.path().join("generated/private");
        let first = FileTransaction::for_output_paths(&include, &[&include]).unwrap();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let include_worker = include;
        let generated_worker = generated;
        let worker = std::thread::spawn(move || {
            let transaction = FileTransaction::for_output_paths(
                &include_worker,
                &[&include_worker, &generated_worker],
            )
            .unwrap();
            acquired_tx.send(()).unwrap();
            drop(transaction);
        });

        assert!(acquired_rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());
        drop(first);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn optional_output_must_remain_below_include_selected_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let include = root.path().join("SDK/include");
        let error = FileTransaction::for_output_paths(
            &include,
            &[&include, &outside.path().join("generated")],
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn explicit_root_accepts_the_rv32_sdk_layout_and_separate_audit_outputs() {
        let root = tempfile::tempdir().unwrap();
        let include = root.path().join("SYS/Developer/include");
        let generated = root.path().join("gen");
        let symbol_audit = root.path().join("symbol-audit/libbases.txt");

        FileTransaction::for_output_paths_with_root(
            root.path(),
            &[&include, &generated, &symbol_audit],
        )
        .unwrap();
    }

    #[test]
    fn explicit_root_rejects_outside_and_traversing_outputs_before_creating_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("build");
        let outside = directory.path().join("outside/output");
        let traversing = root.join("../escaped/output");

        for output in [&outside, &traversing] {
            let error = FileTransaction::for_output_paths_with_root(&root, &[output]).unwrap_err();
            assert!(matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied
            ));
            assert!(!root.exists());
        }
        assert!(!outside.exists());
        assert!(!directory.path().join("escaped").exists());
    }

    #[test]
    fn explicit_root_rejects_the_filesystem_root_and_the_root_as_an_output() {
        let directory = tempfile::tempdir().unwrap();
        let error = FileTransaction::for_output_paths_with_root(
            Path::new("/"),
            &[&directory.path().join("output")],
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        let root = directory.path().join("build");
        let error = FileTransaction::for_output_paths_with_root(&root, &[&root]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!root.exists());

        let unnormalized_root = directory.path().join("build/./nested");
        let error = FileTransaction::for_output_paths_with_root(
            &unnormalized_root,
            &[&unnormalized_root.join("include")],
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!directory.path().join("build").exists());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_root_rejects_symlinked_root_and_parent_components() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let real_root = directory.path().join("real-root");
        std::fs::create_dir(&real_root).unwrap();
        let root_link = directory.path().join("root-link");
        symlink(&real_root, &root_link).unwrap();
        let output_below_root_link = root_link.join("include");
        assert!(FileTransaction::for_output_paths_with_root(
            &root_link,
            &[&output_below_root_link]
        )
        .is_err());

        let parent_link = directory.path().join("parent-link");
        symlink(directory.path(), &parent_link).unwrap();
        let root_below_parent_link = parent_link.join("build");
        let output_below_parent_link = root_below_parent_link.join("include");
        assert!(FileTransaction::for_output_paths_with_root(
            &root_below_parent_link,
            &[&output_below_parent_link]
        )
        .is_err());
        assert!(!directory.path().join("build").exists());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_root_serializes_writers_with_different_output_subsets() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("build");
        let include = root.join("SYS/Developer/include");
        let generated = root.join("gen");
        let first = FileTransaction::for_output_paths_with_root(&root, &[&include]).unwrap();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let worker_root = root;
        let worker_output = generated;
        let worker = std::thread::spawn(move || {
            let transaction =
                FileTransaction::for_output_paths_with_root(&worker_root, &[&worker_output])
                    .unwrap();
            acquired_tx.send(()).unwrap();
            drop(transaction);
        });

        assert!(acquired_rx
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());
        drop(first);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.join().unwrap();
    }
}
