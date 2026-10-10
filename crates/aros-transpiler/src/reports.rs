//! Report files, inventory manifests and output path resolution.

use crate::publication::Publication;
use aros_common::DiagnosticSeverity;
use aros_transpiler::DependencyGraph;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// Resolve existing ancestors as well as the final entry. Reports may be new
// files, but a symlinked parent must not make them alias a protected output.
pub fn resolved_publication_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => continue,
            other => resolved.push(other.as_os_str()),
        }
        match std::fs::symlink_metadata(&resolved) {
            Ok(_) => resolved = std::fs::canonicalize(&resolved)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(resolved)
}

fn cmake_quoted_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn render_source_inventory_manifest(graph: &DependencyGraph) -> String {
    let mut fetches: Vec<_> = graph
        .source_inventory_fetches
        .iter()
        .filter_map(|name| graph.fetches.iter().find(|fetch| &fetch.name == name))
        .collect();
    fetches.sort_by(|left, right| left.name.cmp(&right.name));

    let mut body = format!("set(AROS_SOURCE_INVENTORY_FETCH_COUNT {})\n", fetches.len());
    for (index, fetch) in fetches.into_iter().enumerate() {
        let fields = [
            ("NAME", fetch.name.as_str()),
            ("ARCHIVE", fetch.archive.as_str()),
            ("SUFFIXES", fetch.suffixes.as_str()),
            ("ORIGINS", fetch.origins.as_str()),
            ("CHECKSUMS", fetch.checksums.as_str()),
            ("NORMALIZATION", fetch.normalization.as_str()),
            ("NORMALIZED_SIZE", fetch.normalized_size.as_str()),
            ("LOCATION", fetch.location.as_str()),
            ("DESTINATION", fetch.destination.as_str()),
            ("BASE", fetch.base.as_str()),
            ("PATCH_ORIGINS", fetch.patch_origins.as_str()),
            ("PATCHES", fetch.patches.as_str()),
        ];
        for (field, value) in fields {
            let _ = writeln!(
                body,
                "set(AROS_SOURCE_INVENTORY_FETCH_{index}_{field} \"{}\")",
                cmake_quoted_value(value)
            );
        }
    }
    body
}

/// Writes one skip report next to the generated CMake file.
///
/// Removes the file when there is nothing left to report. Every report used to
/// be written only in the non-empty case, so a file outlived the change that
/// emptied it and went on naming declarations that were no longer skipped. That
/// is worse than no report: the numbers are what the next step is chosen from.
///
/// Reports are part of the same publication transaction as the generated
/// graph. A stale report is removed only when the replacement generation
/// commits successfully.
pub fn write_report(
    publication: &mut Publication,
    output: &Path,
    extension: &str,
    mut lines: Vec<String>,
    what: &str,
) {
    let report = output.with_extension(extension);
    lines.sort_unstable();
    lines.dedup();
    let n = lines.len();
    let (code, severity) = report_metadata(extension);
    publication.record_coverage(code, severity, Some(&report), n, what);
    if lines.is_empty() {
        publication.absent(report);
        return;
    }
    let body = lines.join("\n");
    publication.present(report.clone(), format!("{body}\n"));
    let marker = if severity == DiagnosticSeverity::Info {
        "ℹ️ "
    } else {
        "⚠️ "
    };
    publication.notice(format!(
        "{marker} [{code}] {n} {what} -> {}",
        report.display()
    ));
}

fn report_metadata(extension: &str) -> (&'static str, DiagnosticSeverity) {
    use DiagnosticSeverity::{Error, Info, Warning};
    match extension {
        "skipped-script-outputs.txt" => ("AT1001", Warning),
        "skipped-hidd-stubs.txt" => ("AT1002", Warning),
        "skipped-host-generated-headers.txt" => ("AT1003", Warning),
        "kickstart-kobj-ldscript.txt" => ("AT1004", Warning),
        "skipped-binary-objects.txt" => ("AT1005", Warning),
        "arch-lane-attachments.txt" => ("AT1006", Info),
        "inherited-arch-sources.txt" => ("AT1007", Info),
        "unresolved-default-link-set.txt" => ("AT1008", Warning),
        "skipped-client-archives.txt" => ("AT1009", Warning),
        "skipped-make-opts.txt" => ("AT1010", Warning),
        "skipped-local-make-includes.txt" => ("AT1011", Warning),
        "skipped-fetches.txt" => ("AT1012", Warning),
        "unowned-port-sources.txt" => ("AT1013", Warning),
        "unresolved-generated-headers.txt" => ("AT1014", Warning),
        "skipped-arch-sources.txt" => ("AT1015", Warning),
        "skipped-icons.txt" => ("AT1016", Warning),
        "skipped-catalogs.txt" => ("AT1017", Warning),
        "skipped-flexcat-sources.txt" => ("AT1018", Warning),
        "skipped-meta-rules.txt" => ("AT1019", Warning),
        "meta-cycles.txt" => ("AT1020", Info),
        "skipped-header-staging.txt" => ("AT1021", Warning),
        "skipped-directory-staging.txt" => ("AT1022", Warning),
        "unresolved-uselibs.txt" => ("AT1023", Warning),
        "unresolved-package-members.txt" => ("AT1024", Warning),
        "unmodelled-declarations.txt" => ("AT1025", Warning),
        "partial-source-lists.txt" => ("AT1026", Warning),
        "unresolved-output-paths.txt" => ("AT1027", Warning),
        "generated-file-rules.txt" => ("AT1028", Warning),
        "skipped-flags.txt" => ("AT1029", Warning),
        "skipped-conditions.txt" => ("AT1030", Warning),
        "unresolved-includes.txt" => ("AT1031", Warning),
        "skipped-ilbm-sources.txt" => ("AT1033", Warning),
        // Adding a report without assigning a stable code is an internal
        // contract error. Publication rejects Error-severity coverage entries.
        _ => ("AT1099", Error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_inventory_preserves_explicit_archive_representation() {
        let mut graph = DependencyGraph::new();
        let (fetches, skipped) = aros_transpiler::fetch::collect_fetches(
            "%fetch mmake=fixture-fetch archive=fixture suffixes=tar.gz destination=$(PORTSDIR)/fixture normalization=canonical-tar-gzip-v1 normalized_size=42\n",
            Path::new("external/fixture"),
        );
        assert!(skipped.is_empty());
        graph.add_fetches(fetches);
        graph
            .source_inventory_fetches
            .push("fixture-fetch".to_owned());
        let manifest = render_source_inventory_manifest(&graph);
        assert!(manifest.contains(
            "set(AROS_SOURCE_INVENTORY_FETCH_0_NORMALIZATION \"canonical-tar-gzip-v1\")"
        ));
        assert!(manifest.contains("set(AROS_SOURCE_INVENTORY_FETCH_0_NORMALIZED_SIZE \"42\")"));

        graph.fetches[0].normalization.clear();
        graph.fetches[0].normalized_size.clear();
        let manifest = render_source_inventory_manifest(&graph);
        assert!(manifest.contains("set(AROS_SOURCE_INVENTORY_FETCH_0_NORMALIZATION \"\")"));
        assert!(manifest.contains("set(AROS_SOURCE_INVENTORY_FETCH_0_NORMALIZED_SIZE \"\")"));
        assert!(!manifest.contains("canonical-tar-gzip-v1"));
    }
}
