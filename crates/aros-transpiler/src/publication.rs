//! Transactional publication of a generated transpiler output set.

use aros_common::DiagnosticSeverity;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
enum ArtifactContent {
    Present(Vec<u8>),
    Absent,
}

#[derive(Debug)]
struct Artifact {
    destination: PathBuf,
    content: ArtifactContent,
}

#[derive(Debug)]
struct StagedArtifact {
    destination: PathBuf,
    staged: Option<PathBuf>,
}

#[derive(Debug)]
struct PublishedArtifact {
    destination: PathBuf,
    backup: Option<PathBuf>,
    installed: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageEntry {
    code: String,
    severity: DiagnosticSeverity,
    report: String,
    count: usize,
    summary: String,
}

#[derive(Serialize)]
struct CoverageDocument<'a> {
    schema: &'static str,
    reports: &'a [CoverageEntry],
}

/// A complete output generation which becomes visible only after every file
/// has been rendered and staged successfully.
#[derive(Debug, Default)]
pub struct Publication {
    artifacts: Vec<Artifact>,
    notices: Vec<String>,
    coverage: Vec<CoverageEntry>,
}

impl Publication {
    pub fn present(&mut self, destination: PathBuf, content: impl Into<Vec<u8>>) {
        self.artifacts.push(Artifact {
            destination,
            content: ArtifactContent::Present(content.into()),
        });
    }

    pub fn absent(&mut self, destination: PathBuf) {
        self.artifacts.push(Artifact {
            destination,
            content: ArtifactContent::Absent,
        });
    }

    pub fn notice(&mut self, notice: impl Into<String>) {
        self.notices.push(notice.into());
    }

    pub fn record_coverage(
        &mut self,
        code: &str,
        severity: DiagnosticSeverity,
        report: Option<&Path>,
        count: usize,
        summary: &str,
    ) {
        self.coverage.push(CoverageEntry {
            code: code.to_owned(),
            severity,
            report: report
                .and_then(Path::file_name)
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            count,
            summary: summary.to_owned(),
        });
    }

    pub fn coverage_json(&mut self) -> serde_json::Result<String> {
        self.coverage
            .sort_by(|left, right| left.code.cmp(&right.code));
        serde_json::to_string_pretty(&CoverageDocument {
            schema: "aros-transpiler-coverage-v1",
            reports: &self.coverage,
        })
        .map(|mut json| {
            json.push('\n');
            json
        })
    }

    pub fn publish(self) -> io::Result<()> {
        let notices = self.notices.clone();
        self.publish_impl(None)?;
        for notice in notices {
            println!("{notice}");
        }
        Ok(())
    }

    fn publish_impl(self, fail_after: Option<usize>) -> io::Result<()> {
        validate_coverage(&self.coverage)?;
        validate_unique_destinations(&self.artifacts)?;
        let mut staged = stage_all(self.artifacts)?;
        let mut published = Vec::with_capacity(staged.len());

        for (index, artifact) in staged.iter_mut().enumerate() {
            let backup = match move_existing_to_backup(&artifact.destination) {
                Ok(backup) => backup,
                Err(error) => return Err(rollback_error(error, &published, &staged)),
            };
            published.push(PublishedArtifact {
                destination: artifact.destination.clone(),
                backup,
                installed: false,
            });

            let result = if fail_after == Some(index) {
                Err(io::Error::other("injected publication failure"))
            } else if let Some(staged_path) = artifact.staged.take() {
                fs::rename(staged_path, &artifact.destination)
            } else {
                Ok(())
            };

            if let Err(error) = result {
                return Err(rollback_error(error, &published, &staged));
            }
            if let Some(last) = published.last_mut() {
                last.installed = true;
            }
        }

        if let Err(error) = sync_parent_directories(&published) {
            return Err(rollback_error(error, &published, &staged));
        }
        for artifact in &published {
            if let Some(backup) = &artifact.backup {
                // The generation is already durable and complete. A leftover
                // hidden backup is recoverable housekeeping, not a reason to
                // report that publication failed after the commit succeeded.
                let _ = remove_any(backup);
            }
        }
        Ok(())
    }
}

fn validate_coverage(coverage: &[CoverageEntry]) -> io::Result<()> {
    let mut codes = BTreeSet::new();
    for entry in coverage {
        if !codes.insert(&entry.code) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate coverage diagnostic code {}", entry.code),
            ));
        }
        if entry.severity == DiagnosticSeverity::Error {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unregistered coverage report {} reached publication as an internal error",
                    entry.report
                ),
            ));
        }
    }
    Ok(())
}

fn validate_unique_destinations(artifacts: &[Artifact]) -> io::Result<()> {
    let mut destinations = BTreeSet::new();
    for artifact in artifacts {
        if !destinations.insert(&artifact.destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "publication contains duplicate destination {}",
                    artifact.destination.display()
                ),
            ));
        }
    }
    Ok(())
}

fn stage_all(artifacts: Vec<Artifact>) -> io::Result<Vec<StagedArtifact>> {
    let mut staged = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let stage_result = match artifact.content {
            ArtifactContent::Present(content) => {
                stage_file(&artifact.destination, &content).map(Some)
            }
            ArtifactContent::Absent => Ok(None),
        };
        match stage_result {
            Ok(path) => staged.push(StagedArtifact {
                destination: artifact.destination,
                staged: path,
            }),
            Err(error) => {
                cleanup_staged(&staged);
                return Err(error);
            }
        }
    }
    Ok(staged)
}

fn stage_file(destination: &Path, content: &[u8]) -> io::Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let staged = unused_sibling(destination, "stage")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok(staged)
}

fn move_existing_to_backup(destination: &Path) -> io::Result<Option<PathBuf>> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            let backup = unused_sibling(destination, "backup")?;
            fs::rename(destination, &backup)?;
            Ok(Some(backup))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn unused_sibling(destination: &Path, purpose: &str) -> io::Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("output path {} has no file name", destination.display()),
        )
    })?;
    for _ in 0..100 {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{}.aros-transpiler-{purpose}-{}-{id}",
            name.to_string_lossy(),
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "cannot reserve a temporary sibling for {}",
            destination.display()
        ),
    ))
}

fn rollback_error(
    publication_error: io::Error,
    published: &[PublishedArtifact],
    staged: &[StagedArtifact],
) -> io::Error {
    let mut rollback_errors = Vec::new();
    for artifact in published.iter().rev() {
        if artifact.installed {
            if let Err(error) = remove_any(&artifact.destination) {
                rollback_errors.push(format!(
                    "cannot remove replacement {}: {error}",
                    artifact.destination.display()
                ));
            }
        }
        if let Some(backup) = &artifact.backup {
            if let Err(error) = fs::rename(backup, &artifact.destination) {
                rollback_errors.push(format!(
                    "cannot restore {}: {error}",
                    artifact.destination.display()
                ));
            }
        }
    }
    cleanup_staged(staged);

    if rollback_errors.is_empty() {
        publication_error
    } else {
        io::Error::other(format!(
            "{publication_error}; publication rollback also failed: {}",
            rollback_errors.join("; ")
        ))
    }
}

fn cleanup_staged(staged: &[StagedArtifact]) {
    for artifact in staged {
        if let Some(path) = &artifact.staged {
            let _ = fs::remove_file(path);
        }
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_parent_directories(published: &[PublishedArtifact]) -> io::Result<()> {
    let parents: BTreeSet<_> = published
        .iter()
        .filter_map(|artifact| artifact.destination.parent())
        .collect();
    for parent in parents {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_failure_leaves_the_previous_generation_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.txt");
        fs::write(&first, "old first").unwrap();
        let invalid_parent = temp.path().join("not-a-directory");
        fs::write(&invalid_parent, "plain file").unwrap();

        let mut publication = Publication::default();
        publication.present(first.clone(), "new first");
        publication.present(invalid_parent.join("second.txt"), "new second");
        assert!(publication.publish().is_err());
        assert_eq!(fs::read_to_string(first).unwrap(), "old first");
    }

    #[test]
    fn commit_failure_restores_every_previous_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        fs::write(&first, "old first").unwrap();
        fs::write(&second, "old second").unwrap();

        let mut publication = Publication::default();
        publication.present(first.clone(), "new first");
        publication.present(second.clone(), "new second");
        assert!(publication.publish_impl(Some(1)).is_err());
        assert_eq!(fs::read_to_string(first).unwrap(), "old first");
        assert_eq!(fs::read_to_string(second).unwrap(), "old second");
    }

    #[test]
    fn absent_artifact_removes_a_stale_report_on_commit() {
        let temp = tempfile::tempdir().unwrap();
        let report = temp.path().join("report.txt");
        fs::write(&report, "stale").unwrap();

        let mut publication = Publication::default();
        publication.absent(report.clone());
        publication.publish().unwrap();
        assert!(!report.exists());
    }

    #[test]
    fn coverage_index_is_stable_and_does_not_expose_host_paths() {
        let mut publication = Publication::default();
        publication.record_coverage(
            "AT1002",
            DiagnosticSeverity::Warning,
            Some(Path::new("/private/build/generated.second.txt")),
            2,
            "second report",
        );
        publication.record_coverage(
            "AT1001",
            DiagnosticSeverity::Info,
            Some(Path::new("/private/build/generated.first.txt")),
            0,
            "first report",
        );
        let value: serde_json::Value =
            serde_json::from_str(&publication.coverage_json().unwrap()).unwrap();
        assert_eq!(value["schema"], "aros-transpiler-coverage-v1");
        assert_eq!(value["reports"][0]["code"], "AT1001");
        assert_eq!(value["reports"][1]["code"], "AT1002");
        assert_eq!(value["reports"][1]["report"], "generated.second.txt");
        assert!(!value.to_string().contains("/private/build"));
    }

    #[test]
    fn unregistered_report_category_fails_before_replacing_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");
        fs::write(&output, "old generation").unwrap();

        let mut publication = Publication::default();
        publication.present(output.clone(), "new generation");
        publication.record_coverage(
            "AT1099",
            DiagnosticSeverity::Error,
            Some(Path::new("unknown-report.txt")),
            1,
            "unregistered report",
        );
        assert!(publication.publish().is_err());
        assert_eq!(fs::read_to_string(output).unwrap(), "old generation");
    }
}
