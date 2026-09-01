//! Transactional publication of a generated transpiler output set.

use aros_common::{publication_journal_path, DiagnosticSeverity, DurableFileSet};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

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
#[derive(Debug)]
pub struct Publication {
    commit_marker: PathBuf,
    artifacts: Vec<Artifact>,
    notices: Vec<String>,
    coverage: Vec<CoverageEntry>,
}

impl Publication {
    /// Create one generation whose graph file is installed after every sidecar.
    pub fn for_output(commit_marker: impl Into<PathBuf>) -> Self {
        Self {
            commit_marker: commit_marker.into(),
            artifacts: Vec::new(),
            notices: Vec::new(),
            coverage: Vec::new(),
        }
    }

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
        self.publish_impl(|| {})?;
        for notice in notices {
            aros_common::outputln!("{notice}");
        }
        Ok(())
    }

    fn publish_impl(self, after_lock: impl FnOnce()) -> io::Result<()> {
        validate_coverage(&self.coverage)?;
        validate_unique_destinations(&self.artifacts)?;
        let (artifacts, marker) = split_commit_marker(self.artifacts, &self.commit_marker)?;
        let journal = publication_journal_path(&self.commit_marker, "transpiler")?;
        let mut transaction = DurableFileSet::new(journal)?;
        after_lock();

        for artifact in artifacts {
            match artifact.content {
                ArtifactContent::Present(content) => {
                    transaction.stage_write(&artifact.destination, &content)?;
                }
                ArtifactContent::Absent => {
                    transaction.stage_remove(&artifact.destination)?;
                }
            }
        }
        transaction.stage_commit_marker(&self.commit_marker, &marker)?;
        transaction.commit()?;
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

fn split_commit_marker(
    artifacts: Vec<Artifact>,
    commit_marker: &Path,
) -> io::Result<(Vec<Artifact>, Vec<u8>)> {
    let mut ordinary = Vec::with_capacity(artifacts.len().saturating_sub(1));
    let mut marker = None;
    for artifact in artifacts {
        if artifact.destination == commit_marker {
            match artifact.content {
                ArtifactContent::Present(content) => marker = Some(content),
                ArtifactContent::Absent => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "transpiler graph commit marker '{}' cannot be absent",
                            commit_marker.display()
                        ),
                    ));
                }
            }
        } else {
            ordinary.push(artifact);
        }
    }
    marker.map_or_else(
        || {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "publication does not contain graph commit marker '{}'",
                    commit_marker.display()
                ),
            ))
        },
        |marker| Ok((ordinary, marker)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn generation(output: &Path, sidecar: &Path, name: &str) -> Publication {
        let mut publication = Publication::for_output(output);
        publication.present(sidecar.to_path_buf(), format!("{name} sidecar"));
        publication.present(output.to_path_buf(), format!("{name} graph"));
        publication
    }

    #[cfg(unix)]
    #[test]
    fn staging_failure_leaves_the_previous_generation_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");
        fs::write(&output, "old graph").unwrap();
        let invalid_parent = temp.path().join("not-a-directory");
        fs::write(&invalid_parent, "plain file").unwrap();

        let mut publication = Publication::for_output(&output);
        publication.present(invalid_parent.join("second.txt"), "new second");
        publication.present(output.clone(), "new graph");
        assert!(publication.publish().is_err());
        assert_eq!(fs::read_to_string(output).unwrap(), "old graph");
    }

    #[cfg(unix)]
    #[test]
    fn commit_failure_restores_every_previous_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");
        let sidecar = temp.path().join("generated.report.txt");
        fs::write(&output, "old graph").unwrap();
        fs::write(&sidecar, "old sidecar").unwrap();

        let publication = generation(&output, &sidecar, "new");
        let current = std::thread::current();
        let thread = current.name().expect("Rust test threads have stable names");
        std::env::set_var(
            "AROS_PUBLICATION_TEST_FAIL_AT",
            format!("before-committed-journal@{thread}"),
        );
        let result = publication.publish();
        std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(output).unwrap(), "old graph");
        assert_eq!(fs::read_to_string(sidecar).unwrap(), "old sidecar");
    }

    #[cfg(unix)]
    #[test]
    fn absent_artifact_removes_a_stale_report_on_commit() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");
        let report = temp.path().join("report.txt");
        fs::write(&output, "old graph").unwrap();
        fs::write(&report, "stale").unwrap();

        let mut publication = Publication::for_output(&output);
        publication.absent(report.clone());
        publication.present(output.clone(), "new graph");
        publication.publish().unwrap();
        assert!(!report.exists());
        assert_eq!(fs::read_to_string(output).unwrap(), "new graph");
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_generations_are_serialized_without_mixing() {
        use std::sync::{Arc, Barrier};

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");
        let sidecar = temp.path().join("generated.report.txt");
        fs::write(&output, "old graph").unwrap();
        fs::write(&sidecar, "old sidecar").unwrap();

        let first_locked = Arc::new(Barrier::new(2));
        let release_first = Arc::new(Barrier::new(2));
        let first = generation(&output, &sidecar, "first");
        let locked = Arc::clone(&first_locked);
        let release = Arc::clone(&release_first);
        let first_worker = std::thread::Builder::new()
            .name("transpiler-publisher-first".to_owned())
            .spawn(move || {
                first.publish_impl(|| {
                    locked.wait();
                    release.wait();
                })
            })
            .unwrap();

        // The first publisher holds the stable journal lock before the second
        // one starts. The second generation must therefore become authoritative
        // as one complete set after the first releases the namespace.
        first_locked.wait();
        let second = generation(&output, &sidecar, "second");
        let second_worker = std::thread::Builder::new()
            .name("transpiler-publisher-second".to_owned())
            .spawn(move || second.publish())
            .unwrap();
        release_first.wait();

        first_worker.join().unwrap().unwrap();
        second_worker.join().unwrap().unwrap();
        assert_eq!(fs::read_to_string(output).unwrap(), "second graph");
        assert_eq!(fs::read_to_string(sidecar).unwrap(), "second sidecar");
    }

    #[test]
    fn coverage_index_is_stable_and_does_not_expose_host_paths() {
        let mut publication = Publication::for_output("/private/build/generated.cmake");
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

        let mut publication = Publication::for_output(&output);
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

    #[test]
    fn graph_commit_marker_is_required_and_cannot_be_removed() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("generated.cmake");

        let mut missing = Publication::for_output(&output);
        missing.present(temp.path().join("sidecar.txt"), "sidecar");
        assert!(missing.publish().is_err());

        let mut absent = Publication::for_output(&output);
        absent.absent(output.clone());
        assert!(absent.publish().is_err());
        assert!(!output.exists());
    }
}
