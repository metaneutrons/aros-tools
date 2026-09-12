//! Read-only management inventory for the cross-toolchain store.
//!
//! This module deliberately observes only the fixed release-envelope layout.
//! It never resolves a project lock, downloads an archive, walks a payload, or
//! invokes a toolchain executable.  Management mutations and their ownership
//! records are separate M8 increments; a metadata observation is never a
//! release-trust, integrity, compatibility, or deletion decision.

use crate::artifact::require_absolute_state_path;
use crate::observability;
use crate::toolchain::default_store_root;
use aros_common::{
    copy_tree_from_snapshot_nofollow, ensure_directory_nofollow, measure_regular_file,
    measure_tree_content_cas_bounded, open_regular_file_nofollow, publication_failure_class,
    publish_atomic_file, publish_prepared_source_tree_noclobber, sha256_bytes,
    toolchain_tree_inventory, AdvisoryFileLock, ArosToolchainManifest, AtomicFilePolicy,
    CommitState, PublicationFailureClass, TreeContentCas, TreeTraversalLimits,
    AROS_TOOLCHAIN_MANIFEST_FILE,
};
use clap::{Args, ValueEnum};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const INVENTORY_SCHEMA: &str = "aros-toolchain-inventory-v1";
const DEFAULT_MAX_ENTRIES: usize = 10_000;
const MAX_MAX_ENTRIES: usize = 100_000;
const COMPLETE_MARKER: &str = ".complete";
const PAYLOAD_DIRECTORY: &str = "toolchain";
const MANAGEMENT_SCHEMA: &str = "aros-toolchain-management-v1";
const OWNERSHIP_RECEIPT_SCHEMA: &str = "aros-toolchain-ownership-v1";
const REGISTRATION_RECEIPT_SCHEMA: &str = "aros-toolchain-registration-v1";
pub const MANAGEMENT_DIRECTORY: &str = ".aros-management/v1";
const IMPORTS_DIRECTORY: &str = "imports/v1";
const OWNERSHIP_RECEIPT: &str = "ownership.json";
const REGISTRATIONS_DIRECTORY: &str = "registrations";
pub const STORE_LOCK: &str = "store.lock";
const IMPORT_MAX_ENTRIES: usize = 500_000;
const IMPORT_MAX_REGULAR_FILE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

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

/// Arguments for a verified local import.  Without `--apply` this command
/// produces a non-persistent preview token.
#[derive(Args)]
pub struct ImportArgs {
    /// Existing absolute self-describing toolchain prefix to copy
    #[arg(long, value_name = "DIR")]
    source: PathBuf,

    /// Explicit absolute store root; defaults to AROS_CROSS_TOOLCHAINS_DIR or AROS_HOME
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Apply only the exact preview identified by this token
    #[arg(long, value_name = "TOKEN")]
    apply: Option<String>,

    /// Result representation on stdout, independent of diagnostic format
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

/// Arguments for a non-owning external-prefix registration.  Without
/// `--apply` this command produces a non-persistent preview token.
#[derive(Args)]
pub struct RegisterArgs {
    /// Existing absolute self-describing toolchain prefix to record without copying
    #[arg(long, value_name = "DIR")]
    source: PathBuf,

    /// Explicit absolute store root; defaults to AROS_CROSS_TOOLCHAINS_DIR or AROS_HOME
    #[arg(long, value_name = "DIR")]
    store: Option<PathBuf>,

    /// Apply only the exact preview identified by this token
    #[arg(long, value_name = "TOKEN")]
    apply: Option<String>,

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
        if depth == 0 && name == "imports" {
            scan_import_layout(&path, &[], 0, budget, result)?;
            if budget.truncated {
                return Ok(());
            }
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

fn scan_import_layout(
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
                message: "import-layout entry name is not UTF-8".into(),
            });
            continue;
        };
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
        if !metadata.is_dir() || metadata.file_type().is_symlink() || !safe_segment(&name) {
            result.findings.push(InventoryFinding {
                location: path.display().to_string(),
                kind: "unexpected-entry",
                message: "expected a portable real directory in the managed import layout".into(),
            });
            continue;
        }
        let valid = match depth {
            0 => name == "v1",
            1 | 2 => true,
            3 => is_lower_sha256(&name),
            _ => false,
        };
        if !valid {
            result.findings.push(InventoryFinding {
                location: path.display().to_string(),
                kind: "invalid-import-layout",
                message: "managed import layout has an unsupported version or identity".into(),
            });
            continue;
        }
        let mut next = components.to_vec();
        next.push(name);
        if depth == 3 {
            inspect_import_envelope(&path, &next, result);
        } else {
            scan_import_layout(&path, &next, depth + 1, budget, result)?;
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
    let summary = manifest.as_ref().map(manifest_summary);
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

/// Observe one managed local-import envelope without walking its payload.
///
/// The ownership receipt is deliberately verified together with the manifest
/// identity, but the payload file inventory is not recomputed here.  Import
/// performs that expensive verification before publication; inventory remains
/// bounded metadata observation and never changes the candidate's trust tier.
fn inspect_import_envelope(envelope: &Path, components: &[String], result: &mut InventoryResult) {
    let [version, host, target_profile, managed_id] = components else {
        result.findings.push(InventoryFinding {
            location: envelope.display().to_string(),
            kind: "internal-layout-error",
            message: "inventory scanner produced an invalid managed import path".into(),
        });
        return;
    };
    debug_assert_eq!(version, "v1");

    let payload = envelope.join(PAYLOAD_DIRECTORY);
    let marker = marker_state(&envelope.join(COMPLETE_MARKER));
    let mut error = None;
    let manifest = match fs::symlink_metadata(&payload) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            match ArosToolchainManifest::load(&payload) {
                Ok(manifest) => {
                    if manifest.host != *host || manifest.target_profile != *target_profile {
                        error = Some(
                            "embedded manifest identity does not match its managed import path"
                                .into(),
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
    if !matches!(&marker, MarkerState::Complete) {
        error.get_or_insert_with(|| "installation completion marker is absent or invalid".into());
    }

    let ownership_valid = manifest.as_ref().is_some_and(|manifest| {
        match verify_import_ownership(envelope, manifest, managed_id) {
            Ok(()) => true,
            Err(receipt_error) => {
                error.get_or_insert(receipt_error);
                false
            }
        }
    });
    let summary = manifest.as_ref().map(manifest_summary);
    result.entries.push(InventoryEntry {
        location: envelope.display().to_string(),
        release_id: manifest
            .as_ref()
            .map_or_else(|| "unknown".into(), |manifest| manifest.release_id.clone()),
        host: host.clone(),
        target_profile: target_profile.clone(),
        archive_sha256: managed_id.clone(),
        marker,
        metadata: if error.is_some() {
            MetadataState::Invalid
        } else {
            MetadataState::Valid
        },
        integrity: "not-checked",
        compatibility: "not-checked",
        provenance: if ownership_valid {
            "imported-local-receipt"
        } else {
            "unverified-local-import"
        },
        qualification: "unknown",
        management: if ownership_valid {
            "owned-import"
        } else {
            "invalid-import-envelope"
        },
        manifest: summary,
        error,
    });
}

fn verify_import_ownership(
    envelope: &Path,
    manifest: &ArosToolchainManifest,
    managed_id: &str,
) -> std::result::Result<(), String> {
    let receipt_path = envelope.join(OWNERSHIP_RECEIPT);
    let Some((_, receipt_bytes)) = measure_regular_file(&receipt_path)
        .map_err(|error| format!("cannot safely read ownership receipt: {error}"))?
    else {
        return Err("ownership receipt is missing".into());
    };
    let receipt: OwnershipReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("ownership receipt is not valid JSON: {error}"))?;
    let manifest_path = envelope
        .join(PAYLOAD_DIRECTORY)
        .join(AROS_TOOLCHAIN_MANIFEST_FILE);
    let Some((_, manifest_bytes)) = measure_regular_file(&manifest_path)
        .map_err(|error| format!("cannot safely read embedded manifest: {error}"))?
    else {
        return Err("embedded manifest disappeared during receipt verification".into());
    };
    let manifest_sha256 = sha256_bytes(&manifest_bytes).to_string();
    if receipt.schema != OWNERSHIP_RECEIPT_SCHEMA
        || receipt.management != "owned-import"
        || receipt.managed_id != managed_id
        || receipt.host != manifest.host
        || receipt.target_profile != manifest.target_profile
        || receipt.target_triple != manifest.target_triple
        || receipt.release_id_claim != manifest.release_id
        || receipt.manifest_sha256 != manifest_sha256
        || receipt.tree_sha256 != manifest.tree_sha256
        || !is_lower_sha256(&receipt.source_snapshot_sha256)
    {
        return Err("ownership receipt does not bind the managed payload identity".into());
    }
    Ok(())
}

fn manifest_summary(manifest: &ArosToolchainManifest) -> ManifestSummary {
    ManifestSummary {
        target_triple: manifest.target_triple.clone(),
        tree_sha256: manifest.tree_sha256.clone(),
        llvm_version: manifest.llvm_version.clone(),
        source_commit: manifest.source_commit.clone(),
        producer_commit: manifest.producer_commit.clone(),
        tools_commit: manifest.tools_commit.clone(),
    }
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

#[derive(Debug, Serialize)]
struct ManagementResult {
    schema: &'static str,
    operation: &'static str,
    state: &'static str,
    source: String,
    store: String,
    source_snapshot_sha256: String,
    manifest_sha256: String,
    tree_sha256: String,
    host: String,
    target_profile: String,
    target_triple: String,
    management: &'static str,
    id: String,
    destination: String,
    apply_token: Option<String>,
    note: &'static str,
}

#[derive(Debug, Clone)]
struct Candidate {
    source: PathBuf,
    source_text: String,
    snapshot: TreeContentCas,
    source_snapshot_sha256: String,
    manifest: ArosToolchainManifest,
    manifest_sha256: String,
    tree_sha256: String,
    managed_id: String,
    registration_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct OwnershipReceipt {
    schema: String,
    management: String,
    managed_id: String,
    host: String,
    target_profile: String,
    target_triple: String,
    release_id_claim: String,
    manifest_sha256: String,
    tree_sha256: String,
    source_snapshot_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RegistrationReceipt {
    schema: String,
    management: String,
    registration_id: String,
    source: String,
    host: String,
    target_profile: String,
    target_triple: String,
    release_id_claim: String,
    manifest_sha256: String,
    tree_sha256: String,
    source_snapshot_sha256: String,
}

/// Preview or commit a verified local toolchain import.
pub fn import(args: ImportArgs) -> Result<()> {
    let store = management_store(args.store)?;
    let candidate = inspect_candidate(args.source)?;
    let destination = import_destination(&store, &candidate);
    let token = apply_token("import", &candidate, &destination);
    let result = if let Some(provided) = args.apply {
        if provided != token {
            return Err(miette::miette!(
                "import apply token does not match the current source snapshot; rerun the import preview"
            ));
        }
        apply_import(&store, &candidate, &destination)?
    } else {
        management_result(
            "import",
            "preview",
            &store,
            &candidate,
            &destination,
            Some(token),
            "validated candidate; no managed envelope or receipt was published",
        )
    };
    print_management_result(&result, args.format);
    Ok(())
}

/// Preview or commit a non-owning external-prefix registration.
pub fn register(args: RegisterArgs) -> Result<()> {
    let store = management_store(args.store)?;
    let candidate = inspect_candidate(args.source)?;
    let destination = registration_destination(&store, &candidate);
    let token = apply_token("register", &candidate, &destination);
    let result = if let Some(provided) = args.apply {
        if provided != token {
            return Err(miette::miette!(
                "registration apply token does not match the current source snapshot; rerun the registration preview"
            ));
        }
        apply_registration(&store, &candidate, &destination)?
    } else {
        management_result(
            "register",
            "preview",
            &store,
            &candidate,
            &destination,
            Some(token),
            "validated external candidate; no registration receipt was published",
        )
    };
    print_management_result(&result, args.format);
    Ok(())
}

pub fn management_store(configured: Option<PathBuf>) -> Result<PathBuf> {
    let store = configured.map_or_else(default_store_root, |path| {
        require_absolute_state_path("--store", path)
    })?;
    normalized_absolute_utf8(&store, "--store")?;
    Ok(store)
}

fn inspect_candidate(source: PathBuf) -> Result<Candidate> {
    let source = require_absolute_state_path("--source", source)?;
    let source_text = normalized_absolute_utf8(&source, "--source")?;
    let metadata = fs::symlink_metadata(&source)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot inspect import source '{}'", source.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(miette::miette!(
            "import source '{}' is not a real directory",
            source.display()
        ));
    }
    let limits = import_limits();
    let snapshot = measure_tree_content_cas_bounded(&source, limits)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot safely measure import source '{}'", source.display()))?;
    let scratch = tempfile::Builder::new()
        .prefix(".aros-toolchain-import-inspect-")
        .tempdir()
        .into_diagnostic()
        .wrap_err("cannot create private import inspection staging directory")?;
    copy_tree_from_snapshot_nofollow(&source, scratch.path(), &snapshot, limits)
        .into_diagnostic()
        .wrap_err("import source changed or could not be copied safely for validation")?;
    let (manifest, manifest_sha256, tree_sha256) = validate_candidate_payload(scratch.path())?;
    let source_snapshot_sha256 = snapshot.snapshot_digest().to_string();
    let managed_id = stable_token(
        "aros-toolchain-managed-import-v1",
        &[&manifest_sha256, &tree_sha256],
    );
    let registration_id = stable_token(
        "aros-toolchain-external-registration-v1",
        &[
            &source_text,
            &manifest_sha256,
            &tree_sha256,
            &source_snapshot_sha256,
        ],
    );
    Ok(Candidate {
        source,
        source_text,
        snapshot,
        source_snapshot_sha256,
        manifest,
        manifest_sha256,
        tree_sha256,
        managed_id,
        registration_id,
    })
}

fn validate_candidate_payload(root: &Path) -> Result<(ArosToolchainManifest, String, String)> {
    let manifest_path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    let Some((_, manifest_bytes)) = measure_regular_file(&manifest_path)
        .into_diagnostic()
        .wrap_err("cannot safely read embedded toolchain manifest")?
    else {
        return Err(miette::miette!(
            "embedded toolchain manifest disappeared during validation"
        ));
    };
    let manifest = ArosToolchainManifest::load(root).into_diagnostic()?;
    let (tree_sha256, files) = toolchain_tree_inventory(root)
        .into_diagnostic()
        .wrap_err("cannot inventory copied toolchain candidate")?;
    if tree_sha256 != manifest.tree_sha256 {
        return Err(miette::miette!(
            "toolchain candidate tree SHA256 mismatch: manifest declares {}, measured {}",
            manifest.tree_sha256,
            tree_sha256
        ));
    }
    if files != manifest.files {
        return Err(miette::miette!(
            "toolchain candidate file inventory does not match its embedded manifest"
        ));
    }
    Ok((
        manifest,
        sha256_bytes(&manifest_bytes).to_string(),
        tree_sha256,
    ))
}

fn import_limits() -> TreeTraversalLimits {
    TreeTraversalLimits::new(IMPORT_MAX_ENTRIES, IMPORT_MAX_REGULAR_FILE_BYTES)
        .expect("compile-time import limits must be non-zero")
}

pub fn normalized_absolute_utf8(path: &Path, label: &str) -> Result<String> {
    if !path.is_absolute() {
        return Err(miette::miette!("{label} must be an absolute path"));
    }
    let raw = path
        .to_str()
        .ok_or_else(|| miette::miette!("{label} must be valid UTF-8"))?;
    if raw
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err(miette::miette!(
            "{label} must not contain '.' or '..' components"
        ));
    }
    let mut normalized = String::from("/");
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(component) => {
                let component = component
                    .to_str()
                    .ok_or_else(|| miette::miette!("{label} must be valid UTF-8"))?;
                if normalized.len() > 1 {
                    normalized.push('/');
                }
                normalized.push_str(component);
            }
            std::path::Component::CurDir | std::path::Component::ParentDir => {
                return Err(miette::miette!(
                    "{label} must not contain '.' or '..' components"
                ));
            }
            std::path::Component::Prefix(_) => {
                return Err(miette::miette!("{label} must use a Unix absolute path"));
            }
        }
    }
    Ok(normalized)
}

fn import_destination(store: &Path, candidate: &Candidate) -> PathBuf {
    store
        .join(IMPORTS_DIRECTORY)
        .join(&candidate.manifest.host)
        .join(&candidate.manifest.target_profile)
        .join(&candidate.managed_id)
}

fn registration_destination(store: &Path, candidate: &Candidate) -> PathBuf {
    store
        .join(MANAGEMENT_DIRECTORY)
        .join(REGISTRATIONS_DIRECTORY)
        .join(format!("{}.json", candidate.registration_id))
}

fn apply_token(operation: &str, candidate: &Candidate, destination: &Path) -> String {
    let destination = destination.display().to_string();
    stable_token(
        "aros-toolchain-management-apply-v1",
        &[
            operation,
            &candidate.source_text,
            &candidate.source_snapshot_sha256,
            &candidate.manifest_sha256,
            &candidate.tree_sha256,
            &candidate.managed_id,
            &candidate.registration_id,
            &destination,
        ],
    )
}

pub fn stable_token(namespace: &str, fields: &[&str]) -> String {
    let mut bytes = Vec::new();
    for field in std::iter::once(&namespace).chain(fields.iter()) {
        let field = field.as_bytes();
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    sha256_bytes(&bytes).to_string()
}

fn apply_import(
    store: &Path,
    candidate: &Candidate,
    destination: &Path,
) -> Result<ManagementResult> {
    let lock = acquire_store_lock(store)?;
    lock.revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lock could not be revalidated")?;
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("managed import destination has no parent"))?;
    ensure_directory_nofollow(parent)
        .into_diagnostic()
        .wrap_err("cannot create or validate managed import parent")?;
    let staging = tempfile::Builder::new()
        .prefix(".aros-toolchain-import-")
        .tempdir_in(parent)
        .into_diagnostic()
        .wrap_err("cannot create managed import staging directory")?;
    let payload = staging.path().join(PAYLOAD_DIRECTORY);
    fs::create_dir(&payload)
        .into_diagnostic()
        .wrap_err("cannot create managed import payload staging directory")?;
    copy_tree_from_snapshot_nofollow(
        &candidate.source,
        &payload,
        &candidate.snapshot,
        import_limits(),
    )
    .into_diagnostic()
    .wrap_err("import source changed or could not be copied safely")?;
    let (manifest, manifest_sha256, tree_sha256) = validate_candidate_payload(&payload)?;
    if manifest != candidate.manifest
        || manifest_sha256 != candidate.manifest_sha256
        || tree_sha256 != candidate.tree_sha256
    {
        return Err(miette::miette!(
            "import source no longer matches the approved preview; no envelope was published"
        ));
    }
    let receipt = OwnershipReceipt {
        schema: OWNERSHIP_RECEIPT_SCHEMA.into(),
        management: "owned-import".into(),
        managed_id: candidate.managed_id.clone(),
        host: candidate.manifest.host.clone(),
        target_profile: candidate.manifest.target_profile.clone(),
        target_triple: candidate.manifest.target_triple.clone(),
        release_id_claim: candidate.manifest.release_id.clone(),
        manifest_sha256: candidate.manifest_sha256.clone(),
        tree_sha256: candidate.tree_sha256.clone(),
        source_snapshot_sha256: candidate.source_snapshot_sha256.clone(),
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt)
        .into_diagnostic()
        .wrap_err("cannot serialize import ownership receipt")?;
    fs::write(staging.path().join(OWNERSHIP_RECEIPT), &receipt_bytes)
        .into_diagnostic()
        .wrap_err("cannot stage import ownership receipt")?;
    fs::write(staging.path().join(COMPLETE_MARKER), b"complete\n")
        .into_diagnostic()
        .wrap_err("cannot stage import completion marker")?;
    verify_exact_receipt(
        &staging.path().join(OWNERSHIP_RECEIPT),
        &receipt_bytes,
        &receipt,
    )?;
    lock.revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lock was lost before import publication")?;
    if let Err(error) = publish_prepared_source_tree_noclobber(staging.path(), destination) {
        return publication_error(
            error,
            "managed import envelope publication failed",
            "managed import may have crossed its atomic publication boundary; destination was retained for inspection",
            "managed import did not cross its atomic publication boundary",
        );
    }
    let published_receipt = destination.join(OWNERSHIP_RECEIPT);
    verify_exact_receipt(&published_receipt, &receipt_bytes, &receipt).or_else(|error| {
        observability::commit_state(
            Err(error),
            CommitState::Committed,
            "managed import was published, but receipt readback could not be proven",
        )
    })?;
    validate_candidate_payload(&destination.join(PAYLOAD_DIRECTORY)).or_else(|error| {
        observability::commit_state(
            Err(error),
            CommitState::Committed,
            "managed import was published, but post-publication payload validation could not be proven",
        )
    })?;
    lock.revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lock was lost after import publication")?;
    Ok(management_result(
        "import",
        "committed",
        store,
        candidate,
        destination,
        None,
        "managed envelope and in-envelope ownership receipt were published atomically; imported is not a released or attested toolchain",
    ))
}

fn apply_registration(
    store: &Path,
    candidate: &Candidate,
    destination: &Path,
) -> Result<ManagementResult> {
    let lock = acquire_store_lock(store)?;
    let parent = destination
        .parent()
        .ok_or_else(|| miette::miette!("registration destination has no parent"))?;
    ensure_directory_nofollow(parent)
        .into_diagnostic()
        .wrap_err("cannot create or validate registration receipt directory")?;
    let receipt = RegistrationReceipt {
        schema: REGISTRATION_RECEIPT_SCHEMA.into(),
        management: "non-owning-external".into(),
        registration_id: candidate.registration_id.clone(),
        source: candidate.source_text.clone(),
        host: candidate.manifest.host.clone(),
        target_profile: candidate.manifest.target_profile.clone(),
        target_triple: candidate.manifest.target_triple.clone(),
        release_id_claim: candidate.manifest.release_id.clone(),
        manifest_sha256: candidate.manifest_sha256.clone(),
        tree_sha256: candidate.tree_sha256.clone(),
        source_snapshot_sha256: candidate.source_snapshot_sha256.clone(),
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt)
        .into_diagnostic()
        .wrap_err("cannot serialize external registration receipt")?;
    lock.revalidate()
        .into_diagnostic()
        .wrap_err("toolchain store lock was lost before registration publication")?;
    if let Err(error) =
        publish_atomic_file(destination, &receipt_bytes, AtomicFilePolicy::NoClobber)
    {
        return publication_error(
            error,
            "external registration publication failed",
            "external registration may have crossed its publication boundary; receipt was retained for inspection",
            "external registration did not cross its publication boundary",
        );
    }
    verify_exact_receipt(destination, &receipt_bytes, &receipt).or_else(|error| {
        observability::commit_state(
            Err(error),
            CommitState::Committed,
            "external registration was published, but receipt readback could not be proven",
        )
    })?;
    Ok(management_result(
        "register",
        "committed",
        store,
        candidate,
        destination,
        None,
        "external prefix was recorded as non-owning; it was not copied, selected, or made removable",
    ))
}

pub fn acquire_store_lock(store: &Path) -> Result<AdvisoryFileLock> {
    AdvisoryFileLock::acquire(&store.join(MANAGEMENT_DIRECTORY).join(STORE_LOCK))
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot acquire toolchain store lock below '{}'",
                store.display()
            )
        })
}

pub fn publication_error<T>(
    error: std::io::Error,
    context: &'static str,
    indeterminate: &'static str,
    rolled_back: &'static str,
) -> Result<T> {
    let class = publication_failure_class(&error);
    let report = Err(error).into_diagnostic().wrap_err(context);
    if matches!(
        class,
        PublicationFailureClass::CommitStateUncertain | PublicationFailureClass::RecoveryIncomplete
    ) {
        observability::commit_state(report, CommitState::Indeterminate, indeterminate)
    } else {
        observability::commit_state(report, CommitState::RolledBack, rolled_back)
    }
}

fn verify_exact_receipt<T>(path: &Path, expected_bytes: &[u8], expected: &T) -> Result<()>
where
    T: for<'de> Deserialize<'de> + PartialEq,
{
    let Some((_, actual)) = measure_regular_file(path)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "cannot safely reread management receipt '{}'",
                path.display()
            )
        })?
    else {
        return Err(miette::miette!(
            "management receipt '{}' disappeared before readback",
            path.display()
        ));
    };
    if actual != expected_bytes {
        return Err(miette::miette!(
            "management receipt '{}' changed during readback",
            path.display()
        ));
    }
    let parsed: T = serde_json::from_slice(&actual)
        .into_diagnostic()
        .wrap_err_with(|| format!("management receipt '{}' is not valid JSON", path.display()))?;
    if &parsed != expected {
        return Err(miette::miette!(
            "management receipt '{}' does not match its planned identity",
            path.display()
        ));
    }
    Ok(())
}

fn management_result(
    operation: &'static str,
    state: &'static str,
    store: &Path,
    candidate: &Candidate,
    destination: &Path,
    apply_token: Option<String>,
    note: &'static str,
) -> ManagementResult {
    ManagementResult {
        schema: MANAGEMENT_SCHEMA,
        operation,
        state,
        source: candidate.source_text.clone(),
        store: store.display().to_string(),
        source_snapshot_sha256: candidate.source_snapshot_sha256.clone(),
        manifest_sha256: candidate.manifest_sha256.clone(),
        tree_sha256: candidate.tree_sha256.clone(),
        host: candidate.manifest.host.clone(),
        target_profile: candidate.manifest.target_profile.clone(),
        target_triple: candidate.manifest.target_triple.clone(),
        management: if operation == "import" {
            "owned-import"
        } else {
            "non-owning-external"
        },
        id: if operation == "import" {
            candidate.managed_id.clone()
        } else {
            candidate.registration_id.clone()
        },
        destination: destination.display().to_string(),
        apply_token,
        note,
    }
}

fn print_management_result(result: &ManagementResult, format: ResultFormat) {
    match format {
        ResultFormat::Human => {
            aros_common::outputln!(
                "Toolchain {} {}: {} / {} [{}]",
                result.operation,
                result.state,
                result.host,
                result.target_profile,
                result.id
            );
            aros_common::outputln!("  Source:      {}", result.source);
            aros_common::outputln!("  Destination: {}", result.destination);
            aros_common::outputln!("  Tree SHA256: {}", result.tree_sha256);
            aros_common::outputln!("  {}", result.note);
            if let Some(token) = &result.apply_token {
                aros_common::outputln!("  Apply token: {token}");
                aros_common::outputln!(
                    "  No persistent state was changed. Re-run with --apply {token} to commit this exact preview."
                );
            }
        }
        ResultFormat::Json => {
            let document = serde_json::to_string_pretty(result)
                .expect("management result serialization is infallible");
            aros_common::outputln!("{document}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_import, apply_registration, import_destination, inspect_candidate, inspect_store,
        normalized_absolute_utf8, registration_destination, validate_candidate_payload,
        MarkerState, MetadataState,
    };
    use aros_common::{
        toolchain_tree_inventory, ArosToolchainManifest, ArosToolchainManifestEntry,
        AROS_TOOLCHAIN_MANIFEST_FILE,
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

    fn write_candidate(root: &Path) {
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/aros-collect"), b"fixture collector").unwrap();
        let (tree_sha256, files) = toolchain_tree_inventory(root).unwrap();
        let mut candidate = manifest();
        candidate.tree_sha256 = tree_sha256;
        candidate.files = files;
        fs::write(
            root.join(AROS_TOOLCHAIN_MANIFEST_FILE),
            serde_json::to_vec(&candidate).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn source_identity_path_is_lexically_normalized_without_resolving_links() {
        assert_eq!(
            normalized_absolute_utf8(Path::new("/tmp//candidate"), "--source").unwrap(),
            "/tmp/candidate"
        );
        assert!(normalized_absolute_utf8(Path::new("/tmp/./candidate"), "--source").is_err());
        assert!(normalized_absolute_utf8(Path::new("/tmp/../candidate"), "--source").is_err());
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

    #[test]
    fn import_publishes_an_owned_envelope_with_an_in_envelope_receipt() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("candidate");
        let store = temporary.path().join("store");
        write_candidate(&source);

        let candidate = inspect_candidate(source).unwrap();
        let destination = import_destination(&store, &candidate);
        let result = apply_import(&store, &candidate, &destination).unwrap();

        assert_eq!(result.state, "committed");
        assert_eq!(result.management, "owned-import");
        assert_eq!(
            fs::read(destination.join(".complete")).unwrap(),
            b"complete\n"
        );
        assert!(destination.join("ownership.json").is_file());
        let (manifest, _, _) = validate_candidate_payload(&destination.join("toolchain")).unwrap();
        assert_eq!(manifest.tree_sha256, candidate.tree_sha256);
        let receipt = fs::read_to_string(destination.join("ownership.json")).unwrap();
        assert!(!receipt.contains(temporary.path().to_str().unwrap()));
        let inventory = inspect_store(&store, 100).unwrap();
        assert_eq!(inventory.entries.len(), 1);
        let entry = &inventory.entries[0];
        assert_eq!(entry.management, "owned-import");
        assert_eq!(entry.provenance, "imported-local-receipt");
        assert_eq!(entry.integrity, "not-checked");
        assert_eq!(entry.compatibility, "not-checked");
        assert!(matches!(entry.metadata, MetadataState::Valid));
        assert!(apply_import(&store, &candidate, &destination).is_err());
    }

    #[test]
    fn inventory_marks_a_tampered_import_receipt_invalid_without_reading_payload_files() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("candidate");
        let store = temporary.path().join("store");
        write_candidate(&source);

        let candidate = inspect_candidate(source).unwrap();
        let destination = import_destination(&store, &candidate);
        apply_import(&store, &candidate, &destination).unwrap();
        fs::write(destination.join("ownership.json"), b"{}\n").unwrap();

        let inventory = inspect_store(&store, 100).unwrap();
        assert_eq!(inventory.entries.len(), 1);
        let entry = &inventory.entries[0];
        assert!(matches!(entry.metadata, MetadataState::Invalid));
        assert_eq!(entry.management, "invalid-import-envelope");
        assert_eq!(entry.provenance, "unverified-local-import");
        assert_eq!(entry.integrity, "not-checked");
        assert!(entry
            .error
            .as_deref()
            .unwrap()
            .contains("ownership receipt"));
    }

    #[test]
    fn external_registration_is_non_owning_and_does_not_copy_the_source() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("candidate");
        let store = temporary.path().join("store");
        write_candidate(&source);

        let candidate = inspect_candidate(source.clone()).unwrap();
        let destination = registration_destination(&store, &candidate);
        let result = apply_registration(&store, &candidate, &destination).unwrap();

        assert_eq!(result.state, "committed");
        assert_eq!(result.management, "non-owning-external");
        assert!(source.join(AROS_TOOLCHAIN_MANIFEST_FILE).is_file());
        assert!(destination.is_file());
        assert!(!store.join("imports").exists());
        let receipt = fs::read_to_string(destination).unwrap();
        assert!(receipt.contains(source.to_str().unwrap()));
        assert!(apply_registration(
            &store,
            &candidate,
            &registration_destination(&store, &candidate)
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_a_symlinked_source_root() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("candidate");
        let link = temporary.path().join("candidate-link");
        write_candidate(&source);
        symlink(&source, &link).unwrap();

        assert!(inspect_candidate(link).is_err());
    }
}
