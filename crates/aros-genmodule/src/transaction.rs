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
    /// change the journal/lock namespace.
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
        for path in &absolute {
            if !path.starts_with(root) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "generated output '{}' escapes stable build root '{}' selected by --output-inc",
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
        }
        if !output_inc.starts_with(root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "include output '{}' escapes stable build root '{}'",
                    output_inc.display(),
                    root.display()
                ),
            ));
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

fn normalized_absolute(path: &Path) -> std::io::Result<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
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
}
