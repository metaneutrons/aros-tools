//! Read-only management inventory for the cross-toolchain store.
//!
//! This module deliberately observes only the fixed release-envelope layout.
//! It never resolves a project lock, downloads an archive, walks a payload, or
//! invokes a toolchain executable.  Management mutations and their ownership
//! records are separate M8 increments; a metadata observation is never a
//! release-trust, integrity, compatibility, or deletion decision.

use crate::artifact::require_absolute_state_path;
use crate::toolchain::default_store_root;
use aros_common::{open_regular_file_nofollow, ArosToolchainManifest};
use clap::{Args, ValueEnum};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const INVENTORY_SCHEMA: &str = "aros-toolchain-inventory-v1";
const DEFAULT_MAX_ENTRIES: usize = 10_000;
const MAX_MAX_ENTRIES: usize = 100_000;
const COMPLETE_MARKER: &str = ".complete";
const PAYLOAD_DIRECTORY: &str = "toolchain";

/// Output representation for a read-only store inventory.
#[derive(Clone, Copy, ValueEnum)]
pub enum ResultFormat {
    /// Compact human-oriented output.
    Human,
    /// Versioned JSON document on stdout.
    Json,
}

/// Arguments for `aros toolchain inventory`.
#[derive(Args)]
pub struct InventoryArgs {
    /// Explicit absolute store root; defaults to AROS_CROSS_TOOLCHAINS_DIR or AROS_HOME
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Bound all fixed-layout directory entries inspected in this invocation
    #[arg(
        long,
        default_value_t = DEFAULT_MAX_ENTRIES,
        value_parser = parse_max_entries
    )]
    max_entries: usize,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

fn parse_max_entries(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| "must be a positive integer".to_owned())?;
    if value == 0 || value > MAX_MAX_ENTRIES {
        return Err(format!(
            "must be between 1 and {MAX_MAX_ENTRIES} fixed-layout entries"
        ));
    }
    Ok(value)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum StoreState {
    Absent,
    Present,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MetadataState {
    Valid,
    Invalid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MarkerState {
    Complete,
    Missing,
    Invalid,
}

#[derive(Debug, Serialize)]
struct InventoryResult {
    schema: &'static str,
    operation: &'static str,
    observation: &'static str,
    store: String,
    store_state: StoreState,
    max_entries: usize,
    inspected_entries: usize,
    truncated: bool,
    coverage: &'static str,
    entries: Vec<InventoryEntry>,
    findings: Vec<InventoryFinding>,
}

#[derive(Debug, Serialize)]
struct InventoryEntry {
    location: String,
    release_id: String,
    host: String,
    target_profile: String,
    archive_sha256: String,
    marker: MarkerState,
    metadata: MetadataState,
    integrity: &'static str,
    compatibility: &'static str,
    provenance: &'static str,
    qualification: &'static str,
    management: &'static str,
    manifest: Option<ManifestSummary>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ManifestSummary {
    target_triple: String,
    tree_sha256: String,
    llvm_version: Option<String>,
    source_commit: String,
    producer_commit: String,
    tools_commit: String,
}

#[derive(Debug, Serialize)]
struct InventoryFinding {
    location: String,
    kind: &'static str,
    message: String,
}

struct ScanBudget {
    limit: usize,
    inspected: usize,
    truncated: bool,
}

impl ScanBudget {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            inspected: 0,
            truncated: false,
        }
    }

    const fn consume(&mut self) -> bool {
        if self.inspected == self.limit {
            self.truncated = true;
            return false;
        }
        self.inspected += 1;
        true
    }
}

/// Inspect the selected store without mutating it or executing a payload.
///
/// # Errors
///
/// Returns an error when the configured root is unsafe or cannot be listed.
/// Malformed candidate envelopes are reported in the successful inventory
/// result, because their presence is an observation rather than a reason to
/// hide other entries.
pub fn inventory(args: InventoryArgs) -> Result<()> {
    let store = match args.store {
        Some(path) => require_absolute_state_path("--store", path)?,
        None => default_store_root()?,
    };
    let result = inspect_store(&store, args.max_entries)?;
    match args.format {
        ResultFormat::Human => print_human(&result),
        ResultFormat::Json => {
            let document = serde_json::to_string_pretty(&result)
                .into_diagnostic()
                .wrap_err("cannot serialize toolchain inventory")?;
            aros_common::outputln!("{document}");
        }
    }
    Ok(())
}

fn inspect_store(store: &Path, max_entries: usize) -> Result<InventoryResult> {
    let metadata = match fs::symlink_metadata(store) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InventoryResult {
                schema: INVENTORY_SCHEMA,
                operation: "inventory",
                observation: "metadata-only",
                store: store.display().to_string(),
                store_state: StoreState::Absent,
                max_entries,
                inspected_entries: 0,
                truncated: false,
                coverage: "complete",
                entries: Vec::new(),
                findings: Vec::new(),
            });
        }
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to inspect store '{}'", store.display()));
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "toolchain store '{}' is not a real directory",
            store.display()
        ));
    }

    let mut result = InventoryResult {
        schema: INVENTORY_SCHEMA,
        operation: "inventory",
        observation: "metadata-only",
        store: store.display().to_string(),
        store_state: StoreState::Present,
        max_entries,
        inspected_entries: 0,
        truncated: false,
        coverage: "complete",
        entries: Vec::new(),
        findings: Vec::new(),
    };
    let mut budget = ScanBudget::new(max_entries);
    scan_level(store, &[], 0, &mut budget, &mut result)?;
    result.inspected_entries = budget.inspected;
    result.truncated = budget.truncated;
    result.coverage = if result.truncated {
        "truncated"
    } else if result
        .findings
        .iter()
        .any(|finding| finding.kind == "unreadable-directory")
    {
        "incomplete"
    } else {
        "complete"
    };
    if result.truncated {
        result.findings.push(InventoryFinding {
            location: store.display().to_string(),
            kind: "scan-truncated",
            message: format!(
                "stopped after {max_entries} fixed-layout entries; rerun with --max-entries above {max_entries} to inspect more"
            ),
        });
    }
    Ok(result)
}

fn scan_level(
    directory: &Path,
    components: &[String],
    depth: usize,
    budget: &mut ScanBudget,
    result: &mut InventoryResult,
) -> Result<()> {
    let Some(entries) = bounded_directory_entries(directory, budget, result) else {
        return Ok(());
    };

    for entry in entries {
        if !budget.consume() {
            return Ok(());
        }
        let Ok(name) = entry.file_name().into_string() else {
            result.findings.push(InventoryFinding {
                location: entry.path().display().to_string(),
                kind: "non-utf8-name",
                message: "store entry name is not UTF-8".into(),
            });
            continue;
        };
        if depth == 0 && name == ".aros-management" {
            // Reserved for later M8 receipts. It is intentionally not a second
            // source of truth for this metadata-only scan.
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                result.findings.push(InventoryFinding {
                    location: path.display().to_string(),
                    kind: "unreadable-entry",
                    message: error.to_string(),
                });
                continue;
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            result.findings.push(InventoryFinding {
                location: path.display().to_string(),
                kind: "unexpected-entry",
                message: "expected a real directory in the fixed release-envelope layout".into(),
            });
            continue;
        }
        if !safe_segment(&name) {
            result.findings.push(InventoryFinding {
                location: path.display().to_string(),
                kind: "unsafe-segment",
                message: "store layout segment is not portable".into(),
            });
            continue;
        }
        let mut next = components.to_vec();
        next.push(name);
        if depth == 3 {
            inspect_envelope(&path, &next, result);
        } else {
            scan_level(&path, &next, depth + 1, budget, result)?;
            if budget.truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Read no more directory records than the remaining global scan budget.
///
/// Sorting is deterministic whenever the directory fit in the budget. A
/// truncated observation makes no claim about omitted sibling names, which is
/// preferable to reading an attacker-controlled directory without a bound.
fn bounded_directory_entries(
    directory: &Path,
    budget: &mut ScanBudget,
    result: &mut InventoryResult,
) -> Option<Vec<fs::DirEntry>> {
    if budget.inspected == budget.limit {
        budget.truncated = true;
        return None;
    }
    let reader = match fs::read_dir(directory) {
        Ok(reader) => reader,
        Err(error) => {
            result.findings.push(InventoryFinding {
                location: directory.display().to_string(),
                kind: "unreadable-directory",
                message: error.to_string(),
            });
            return None;
        }
    };
    let remaining = budget.limit - budget.inspected;
    let mut entries = Vec::with_capacity(remaining.min(1024));
    for candidate in reader {
        let entry = match candidate {
            Ok(entry) => entry,
            Err(error) => {
                result.findings.push(InventoryFinding {
                    location: directory.display().to_string(),
                    kind: "unreadable-directory",
                    message: error.to_string(),
                });
                return None;
            }
        };
        if entries.len() == remaining {
            budget.truncated = true;
            break;
        }
        entries.push(entry);
    }
    entries.sort_by_key(fs::DirEntry::file_name);
    Some(entries)
}

fn inspect_envelope(envelope: &Path, components: &[String], result: &mut InventoryResult) {
    let [release_id, host, target_profile, archive_sha256] = components else {
        result.findings.push(InventoryFinding {
            location: envelope.display().to_string(),
            kind: "internal-layout-error",
            message: "inventory scanner produced an invalid envelope path".into(),
        });
        return;
    };
    let payload = envelope.join(PAYLOAD_DIRECTORY);
    let marker = marker_state(&envelope.join(COMPLETE_MARKER));
    let mut error = None;
    let manifest = match fs::symlink_metadata(&payload) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            match ArosToolchainManifest::load(&payload) {
                Ok(manifest) => {
                    if manifest.release_id != *release_id
                        || manifest.host != *host
                        || manifest.target_profile != *target_profile
                    {
                        error = Some(
                            "embedded manifest identity does not match its envelope path".into(),
                        );
                        None
                    } else {
                        Some(manifest)
                    }
                }
                Err(load_error) => {
                    error = Some(format!("invalid embedded manifest: {load_error}"));
                    None
                }
            }
        }
        Ok(_) => {
            error = Some("payload is not a real directory".into());
            None
        }
        Err(load_error) => {
            error = Some(format!("payload is unavailable: {load_error}"));
            None
        }
    };
    if !is_lower_sha256(archive_sha256) {
        error.get_or_insert_with(|| {
            "archive identity path segment is not a lowercase SHA-256".into()
        });
    }
    if !matches!(marker, MarkerState::Complete) {
        error.get_or_insert_with(|| "installation completion marker is absent or invalid".into());
    }
    let summary = manifest.as_ref().map(|manifest| ManifestSummary {
        target_triple: manifest.target_triple.clone(),
        tree_sha256: manifest.tree_sha256.clone(),
        llvm_version: manifest.llvm_version.clone(),
        source_commit: manifest.source_commit.clone(),
        producer_commit: manifest.producer_commit.clone(),
        tools_commit: manifest.tools_commit.clone(),
    });
    result.entries.push(InventoryEntry {
        location: envelope.display().to_string(),
        release_id: release_id.clone(),
        host: host.clone(),
        target_profile: target_profile.clone(),
        archive_sha256: archive_sha256.clone(),
        marker,
        metadata: if error.is_some() {
            MetadataState::Invalid
        } else {
            MetadataState::Valid
        },
        integrity: "not-checked",
        compatibility: "not-checked",
        provenance: "embedded-manifest-claim",
        qualification: "unknown",
        management: "unowned-legacy",
        manifest: summary,
        error,
    });
}

fn marker_state(path: &Path) -> MarkerState {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return MarkerState::Missing,
        Err(_) => return MarkerState::Invalid,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return MarkerState::Invalid;
    }
    let Ok(file) = open_regular_file_nofollow(path) else {
        return MarkerState::Invalid;
    };
    let mut contents = Vec::new();
    if file.take(10).read_to_end(&mut contents).is_err() || contents != b"complete\n" {
        MarkerState::Invalid
    } else {
        MarkerState::Complete
    }
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\'])
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn print_human(result: &InventoryResult) {
    aros_common::outputln!("Toolchain inventory (metadata-only): {}", result.store);
    match result.store_state {
        StoreState::Absent => {
            aros_common::outputln!("  Store is absent; no toolchain envelopes were inspected.");
        }
        StoreState::Present => {
            aros_common::outputln!(
                "  {} envelope(s), {} fixed-layout entry/entries inspected; coverage: {}{}.",
                result.entries.len(),
                result.inspected_entries,
                result.coverage,
                if result.truncated {
                    "; scan truncated"
                } else {
                    ""
                }
            );
            for entry in &result.entries {
                aros_common::outputln!(
                    "  {} / {} / {} [{}] {}",
                    entry.host,
                    entry.target_profile,
                    entry.release_id,
                    entry.metadata_label(),
                    entry.location
                );
                if let Some(error) = &entry.error {
                    aros_common::outputln!("    {error}");
                }
            }
            for finding in &result.findings {
                aros_common::outputln!(
                    "  {}: {} ({})",
                    finding.kind,
                    finding.message,
                    finding.location
                );
            }
        }
    }
    aros_common::outputln!(
        "  Payload integrity and executable compatibility were not checked; no toolchain was executed."
    );
}

impl InventoryEntry {
    const fn metadata_label(&self) -> &'static str {
        match self.metadata {
            MetadataState::Valid => "valid metadata",
            MetadataState::Invalid => "invalid metadata",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect_store, MarkerState, MetadataState};
    use aros_common::{
        ArosToolchainManifest, ArosToolchainManifestEntry, AROS_TOOLCHAIN_MANIFEST_FILE,
    };
    use std::fs;
    use std::path::Path;

    fn manifest() -> ArosToolchainManifest {
        ArosToolchainManifest {
            schema: 1,
            release_id: "toolchain-v1".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: "a".repeat(64),
            llvm_version: Some("11.0.0".into()),
            recipe_sha256: "b".repeat(64),
            source_lock_sha256: "c".repeat(64),
            profiles_sha256: "d".repeat(64),
            source_commit: "e".repeat(40),
            producer_commit: "f".repeat(40),
            tools_commit: "0".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["collect-aros".into()],
            build_environment: serde_json::Map::new(),
            files: vec![ArosToolchainManifestEntry {
                path: "bin".into(),
                mode: "0755".into(),
                kind: "directory".into(),
                sha256: None,
                size: None,
                target: None,
            }],
        }
    }

    fn write_envelope(store: &Path) -> std::path::PathBuf {
        let envelope = store
            .join("toolchain-v1")
            .join("linux-x86_64")
            .join("pc-x86_64")
            .join("1".repeat(64));
        let payload = envelope.join("toolchain");
        fs::create_dir_all(&payload).unwrap();
        fs::write(envelope.join(".complete"), b"complete\n").unwrap();
        fs::write(
            payload.join(AROS_TOOLCHAIN_MANIFEST_FILE),
            serde_json::to_vec(&manifest()).unwrap(),
        )
        .unwrap();
        envelope
    }

    #[test]
    fn inventory_observes_valid_metadata_without_measuring_or_executing_payload() {
        let temporary = tempfile::tempdir().unwrap();
        let envelope = write_envelope(temporary.path());

        let result = inspect_store(temporary.path(), 100).unwrap();

        assert!(!result.truncated);
        assert_eq!(result.entries.len(), 1);
        let entry = &result.entries[0];
        assert_eq!(entry.location, envelope.display().to_string());
        assert!(matches!(entry.marker, MarkerState::Complete));
        assert!(matches!(entry.metadata, MetadataState::Valid));
        assert_eq!(entry.integrity, "not-checked");
        assert_eq!(entry.compatibility, "not-checked");
        assert!(entry.error.is_none());
        assert_eq!(result.coverage, "complete");
    }

    #[test]
    fn inventory_reports_bad_marker_without_hiding_the_envelope() {
        let temporary = tempfile::tempdir().unwrap();
        let envelope = write_envelope(temporary.path());
        fs::write(envelope.join(".complete"), b"partial\n").unwrap();

        let result = inspect_store(temporary.path(), 100).unwrap();

        assert_eq!(result.entries.len(), 1);
        let entry = &result.entries[0];
        assert!(matches!(entry.marker, MarkerState::Invalid));
        assert!(matches!(entry.metadata, MetadataState::Invalid));
        assert!(entry
            .error
            .as_deref()
            .unwrap()
            .contains("completion marker"));
    }

    #[test]
    fn inventory_stops_at_the_declared_fixed_layout_budget() {
        let temporary = tempfile::tempdir().unwrap();
        write_envelope(temporary.path());

        let result = inspect_store(temporary.path(), 1).unwrap();

        assert!(result.truncated);
        assert_eq!(result.coverage, "truncated");
        assert_eq!(result.inspected_entries, 1);
        assert!(result.entries.is_empty());
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.kind == "scan-truncated"));
    }
}
