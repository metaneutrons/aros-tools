//! Rendering adapter for the bounded cache-status commands.

use std::fs;
use std::path::{Path, PathBuf};

use crate::artifact::{archive_cache_path, obtain_archive, require_sha256, verify_archive};
use crate::host_compiler;
use crate::toolchain;
use crate::toolchain_management::ResultFormat;
use crate::{CacheArchiveSelector, CacheCargoSelector, CacheSourceSelector};
use aros_cache::{
    cache_status, compiler_cache_status, CacheCapability, CacheFamily, CacheFamilyStatus,
    CacheSideEffects, CacheStatus, CompilerBackendChoice, CompilerCacheStatus, RootObservation,
};
use aros_toolchain::{
    cargo_vendor::{
        cargo_vendor_status, fetch_vendor_generation, list_vendor_generation,
        select_vendor_generation, verify_vendor_generation, CargoVendorGeneration,
        CargoVendorRequest, CargoVendorStatus, CARGO_VENDOR_FETCH_SCHEMA, CARGO_VENDOR_LIST_SCHEMA,
        CARGO_VENDOR_VERIFY_SCHEMA,
    },
    source_cache::{
        fetch_request, list as list_source_cache, status as source_cache_status, verify_request,
        SourceCacheFetch, SourceCacheList, SourceCacheStatus, SourceCacheVerification,
    },
    source_cache_request::{read_selector, SourceCacheRequest},
    ContractError,
};
use miette::Result;
use serde::Serialize;

const ARCHIVE_STATUS_SCHEMA: &str = "aros-cache-archives-status-v1";
const ARCHIVE_LIST_SCHEMA: &str = "aros-cache-archives-list-v1";
const ARCHIVE_FETCH_SCHEMA: &str = "aros-cache-archives-fetch-v1";
const ARCHIVE_VERIFY_SCHEMA: &str = "aros-cache-archives-verify-v1";

#[derive(Serialize)]
struct ArchiveCacheStatus {
    schema: &'static str,
    operation: &'static str,
    observation: &'static str,
    side_effects: CacheSideEffects,
    capabilities: [CacheCapability; 4],
    root: RootObservation,
    object_layout: &'static str,
    boundary: &'static str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ArchiveKind {
    HostCompiler,
    CrossToolchain,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ArchiveHostSelection {
    Explicit,
    RunningHost,
}

#[derive(Serialize)]
struct ArchiveSelection {
    kind: ArchiveKind,
    project: PathBuf,
    configuration_source: String,
    configuration_kind: &'static str,
    transport_source: &'static str,
    host: String,
    host_selection: ArchiveHostSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_triple: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    llvm_version: Option<String>,
    url: String,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_size: Option<u64>,
    cache_path: PathBuf,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ArchiveCacheEntryState {
    Missing,
    PresentUnverified,
    Unsafe,
    Inaccessible,
}

#[derive(Serialize)]
struct ArchiveCacheEntry {
    state: ArchiveCacheEntryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata_size: Option<u64>,
}

#[derive(Serialize)]
struct ArchiveCacheList {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    entry: ArchiveCacheEntry,
    boundary: &'static str,
}

#[derive(Serialize)]
struct ArchiveCacheFetch {
    schema: &'static str,
    operation: &'static str,
    offline: bool,
    refresh_requested: bool,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    prior_entry: ArchiveCacheEntry,
    verification_scope: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct ArchiveCacheVerification {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    verification_scope: &'static str,
    not_verified: [&'static str; 4],
}

#[derive(Serialize)]
struct CargoVendorList {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: aros_toolchain::cargo_vendor::CargoVendorSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<CargoVendorGeneration>,
    boundary: &'static str,
}

/// Render the top-level passive cache overview.
///
/// # Errors
///
/// Returns an error only when an explicit cache root is invalid or result
/// serialization fails. The observation never creates, scans or mutates cache
/// state.
pub fn status(format: ResultFormat) -> Result<()> {
    let report = cache_status().map_err(|error| miette::miette!(error))?;
    match format {
        ResultFormat::Human => print_status_human(&report),
        ResultFormat::Json => print_json(&report, "cache status")?,
    }
    Ok(())
}

/// Render the passive compiler-cache backend projection.
///
/// # Errors
///
/// Returns an error only when result serialization fails. An unavailable
/// backend is a successful status observation, never an implicit fallback.
pub fn compiler_status(
    backend: CompilerBackendChoice,
    dir: Option<std::path::PathBuf>,
    format: ResultFormat,
) -> Result<()> {
    let report = compiler_cache_status(backend, dir).map_err(|error| miette::miette!(error))?;
    match format {
        ResultFormat::Human => print_compiler_status_human(&report),
        ResultFormat::Json => print_json(&report, "compiler cache status")?,
    }
    Ok(())
}

/// Render the passive observation of one explicit source-cache root.
///
/// # Errors
///
/// Returns a structured configuration diagnostic only for an invalid root or
/// result-serialization failure. No selector, payload, lock, network or
/// backend process is read or started.
pub fn source_status(dir: &Path, format: ResultFormat) -> Result<()> {
    let report = source_cache_status(dir).map_err(|error| contract_error(&error))?;
    match format {
        ResultFormat::Human => print_source_status_human(&report),
        ResultFormat::Json => print_json(&report, "source cache status")?,
    }
    Ok(())
}

/// Render a metadata-only projection of one reviewed source selector.
///
/// # Errors
///
/// Returns a structured selector/cache diagnostic. This operation never hashes
/// or acquires payloads.
pub fn source_list(selector: CacheSourceSelector, dir: &Path, format: ResultFormat) -> Result<()> {
    let request = source_request(selector)?;
    let report = list_source_cache(dir, &request).map_err(|error| contract_error(&error))?;
    match format {
        ResultFormat::Human => print_source_list_human(&report),
        ResultFormat::Json => print_json(&report, "source cache list")?,
    }
    Ok(())
}

/// Fetch one reviewed source selector, then report its measured closure.
///
/// # Errors
///
/// Returns a structured selector/cache/transport diagnostic. `offline` is a
/// hard no-network boundary. An unverified product plan is accepted only when
/// the caller supplied the explicit, parser-bound `--allow-unverified` flag.
pub async fn source_fetch(
    selector: CacheSourceSelector,
    dir: &Path,
    offline: bool,
    allow_unverified: bool,
    format: ResultFormat,
) -> Result<()> {
    let request = source_request(selector)?;
    if allow_unverified
        && !request.entries.iter().any(|entry| {
            matches!(
                entry.integrity,
                aros_toolchain::source_cache_request::SourceCacheIntegrity::Unverified { .. }
            )
        })
    {
        return Err(miette::miette!(
            "--allow-unverified is valid only for a product source-fetch plan that declares an unverified entry"
        ));
    }
    let report = fetch_request(dir, &request, offline, allow_unverified)
        .await
        .map_err(|error| contract_error(&error))?;
    match format {
        ResultFormat::Human => print_source_fetch_human(&report),
        ResultFormat::Json => print_json(&report, "source cache fetch")?,
    }
    Ok(())
}

/// Verify one strict reviewed source selector.
///
/// # Errors
///
/// Returns a structured selector/cache-integrity diagnostic. This operation
/// hashes selected objects but neither acquires nor changes them.
pub fn source_verify(
    selector: CacheSourceSelector,
    dir: &Path,
    format: ResultFormat,
) -> Result<()> {
    let request = source_request(selector)?;
    let report = verify_request(dir, &request).map_err(|error| contract_error(&error))?;
    match format {
        ResultFormat::Human => print_source_verify_human(&report),
        ResultFormat::Json => print_json(&report, "source cache verify")?,
    }
    Ok(())
}

/// Render a passive observation of the caller-selected Cargo cache root.
///
/// # Errors
///
/// Returns a structured configuration diagnostic only for an invalid root or
/// result serialization failure. It never resolves Cargo, reads a checkout,
/// scans generations or creates state.
pub fn cargo_status(dir: &Path, format: ResultFormat) -> Result<()> {
    let report = cargo_vendor_status(dir).map_err(|error| contract_error(&error))?;
    match format {
        ResultFormat::Human => print_cargo_status_human(&report),
        ResultFormat::Json => print_json(&report, "cargo cache status")?,
    }
    Ok(())
}

/// Render metadata from one selected immutable Cargo generation without
/// hashing the vendor payload.
pub fn cargo_list(selector: CacheCargoSelector, format: ResultFormat) -> Result<()> {
    let request = cargo_request(selector)?;
    let selection = select_vendor_generation(&request).map_err(|error| contract_error(&error))?;
    let generation = list_vendor_generation(&request).map_err(|error| contract_error(&error))?;
    let report = CargoVendorList {
        schema: CARGO_VENDOR_LIST_SCHEMA,
        operation: "cargo.list",
        side_effects: cargo_selection_side_effects(),
        selection,
        generation,
        boundary: "list proves explicit producer/tools/Cargo selection inputs through bounded Git and Cargo version probes, then reads only a generation receipt; it never reads vendor payloads, resolves dependencies or creates cache state",
    };
    match format {
        ResultFormat::Human => print_cargo_list_human(&report),
        ResultFormat::Json => print_json(&report, "cargo cache list")?,
    }
    Ok(())
}

/// Populate or strictly reuse one selected immutable Cargo vendor generation.
pub fn cargo_fetch(
    selector: CacheCargoSelector,
    offline: bool,
    format: ResultFormat,
) -> Result<()> {
    let request = cargo_request(selector)?;
    let cancellation = aros_common::CancellationToken::default();
    let mut report = fetch_vendor_generation(&request, offline, &cancellation)
        .map_err(|error| contract_error(&error))?;
    report.schema = CARGO_VENDOR_FETCH_SCHEMA;
    report.operation = "cargo.fetch";
    match format {
        ResultFormat::Human => print_cargo_generation_human(&report, offline),
        ResultFormat::Json => print_json(&report, "cargo cache fetch")?,
    }
    Ok(())
}

/// Fully validate one selected Cargo vendor generation.
pub fn cargo_verify(selector: CacheCargoSelector, format: ResultFormat) -> Result<()> {
    let request = cargo_request(selector)?;
    let mut report = verify_vendor_generation(&request).map_err(|error| contract_error(&error))?;
    report.schema = CARGO_VENDOR_VERIFY_SCHEMA;
    report.operation = "cargo.verify";
    match format {
        ResultFormat::Human => print_cargo_generation_human(&report, false),
        ResultFormat::Json => print_json(&report, "cargo cache verify")?,
    }
    Ok(())
}

/// Render the passive archive-cache root observation.
///
/// # Errors
///
/// Returns an error when the AROS archive root cannot be resolved safely or
/// result serialization fails. This operation neither reads an archive nor
/// creates the archive directory.
pub fn archive_status(format: ResultFormat) -> Result<()> {
    let report = ArchiveCacheStatus {
        schema: ARCHIVE_STATUS_SCHEMA,
        operation: "archives.status",
        observation: "passive",
        side_effects: passive_side_effects(),
        capabilities: [
            CacheCapability::Status,
            CacheCapability::List,
            CacheCapability::Fetch,
            CacheCapability::Verify,
        ],
        root: archive_root_observation()?,
        object_layout: "downloads/sha256/<archive-sha256>.tar.xz",
        boundary: "installed host compilers and cross-toolchain stores are outside this cache; status does not enumerate or hash archive objects",
    };
    match format {
        ResultFormat::Human => print_archive_status_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache status")?,
    }
    Ok(())
}

/// List metadata for one configured compiler archive without hashing it.
///
/// # Errors
///
/// Returns an error for an invalid selected project/configuration or result
/// serialization. A missing or unsafe cache object is a successful list result.
pub fn archive_list(selector: CacheArchiveSelector, format: ResultFormat) -> Result<()> {
    let selection = archive_selection(selector)?;
    let report = ArchiveCacheList {
        schema: ARCHIVE_LIST_SCHEMA,
        operation: "archives.list",
        side_effects: passive_side_effects(),
        entry: observe_archive_entry(&selection.cache_path),
        selection,
        boundary: "list reads final-path metadata only; it does not hash, download, extract, install, or validate a payload tree",
    };
    match format {
        ResultFormat::Human => print_archive_list_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache list")?,
    }
    Ok(())
}

/// Acquire and verify one configured archive without extracting it.
///
/// # Errors
///
/// Returns an error for invalid selection, offline cache misses, transfer
/// failures, or archive identity mismatches. The shared acquisition primitive
/// publishes only verified content-addressed bytes; it never installs a tree.
pub async fn archive_fetch(
    selector: CacheArchiveSelector,
    offline: bool,
    refresh: bool,
    format: ResultFormat,
) -> Result<()> {
    let selection = archive_selection(selector)?;
    let prior_entry = observe_archive_entry(&selection.cache_path);
    obtain_archive(
        &selection.url,
        &selection.sha256,
        selection.expected_size,
        offline,
        refresh,
    )
    .await?;
    let report = ArchiveCacheFetch {
        schema: ARCHIVE_FETCH_SCHEMA,
        operation: "archives.fetch",
        offline,
        refresh_requested: refresh,
        side_effects: archive_fetch_side_effects(offline, refresh, prior_entry.state),
        selection,
        prior_entry,
        verification_scope: "archive_bytes_exact_size_and_sha256",
        boundary: "fetch never extracts, installs, replaces a content-addressed cache object, or validates a payload tree, manifest, provenance, or attestation",
    };
    match format {
        ResultFormat::Human => print_archive_fetch_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache fetch")?,
    }
    Ok(())
}

/// Hash and verify one selected archive's declared byte identity.
///
/// # Errors
///
/// Returns an error for a missing, unsafe, inaccessible, or mismatched cache
/// object. Verification does not extract or install an archive.
pub fn archive_verify(selector: CacheArchiveSelector, format: ResultFormat) -> Result<()> {
    let selection = archive_selection(selector)?;
    verify_archive(
        &selection.cache_path,
        &selection.sha256,
        selection.expected_size,
    )?;
    let report = ArchiveCacheVerification {
        schema: ARCHIVE_VERIFY_SCHEMA,
        operation: "archives.verify",
        side_effects: archive_verify_side_effects(),
        selection,
        verification_scope: "archive_bytes_exact_size_and_sha256",
        not_verified: [
            "archive extraction safety",
            "payload tree identity",
            "installed toolchain or host-compiler receipt",
            "release provenance or attestation",
        ],
    };
    match format {
        ResultFormat::Human => print_archive_verify_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache verify")?,
    }
    Ok(())
}

const fn passive_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: false,
    }
}

const fn archive_fetch_side_effects(
    offline: bool,
    refresh: bool,
    prior_state: ArchiveCacheEntryState,
) -> CacheSideEffects {
    let transfers_or_stages =
        !offline && (refresh || matches!(prior_state, ArchiveCacheEntryState::Missing));
    CacheSideEffects {
        creates_state: transfers_or_stages,
        mutates_state: transfers_or_stages,
        network: transfers_or_stages,
        backend_process: false,
        locks: false,
        hashes_payloads: true,
    }
}

const fn archive_verify_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: true,
    }
}

const fn cargo_selection_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: true,
        locks: false,
        hashes_payloads: false,
    }
}

fn archive_root_observation() -> Result<RootObservation> {
    cache_status()
        .map_err(|error| miette::miette!(error))?
        .families
        .into_iter()
        .find(|family| family.family == CacheFamily::Archives)
        .and_then(|family| family.root)
        .ok_or_else(|| miette::miette!("archive cache status did not return its configured root"))
}

fn archive_selection(selector: CacheArchiveSelector) -> Result<ArchiveSelection> {
    let project = canonical_project(&selector.project)?;
    let (host, host_selection) = match selector.host {
        Some(host) => (host, ArchiveHostSelection::Explicit),
        None => (
            host_compiler::host_platform_key()?.to_owned(),
            ArchiveHostSelection::RunningHost,
        ),
    };

    if selector.host_compiler {
        let config = host_compiler::load_host_compiler_config(&project)?;
        let selected = host_compiler::select_host_compiler_for_host(&config, &host)?;
        let sha256 = require_sha256(
            selected.sha256.as_deref(),
            &format!("host compiler asset for {}", selected.host_key),
        )?;
        return Ok(ArchiveSelection {
            kind: ArchiveKind::HostCompiler,
            configuration_source: target_configuration_source(&project),
            configuration_kind: target_configuration_kind(&project),
            transport_source: if std::env::var("AROS_HOST_COMPILER_URL").is_ok() {
                "AROS_HOST_COMPILER_URL"
            } else {
                "aros-targets.toml host_compiler.base_url"
            },
            project,
            host: selected.host_key,
            host_selection,
            release_id: None,
            target_profile: None,
            target_triple: None,
            llvm_version: Some(selected.version),
            url: selected.url,
            cache_path: archive_cache_path(&sha256)?,
            sha256,
            expected_size: None,
        });
    }

    let preset = selector
        .preset
        .ok_or_else(|| miette::miette!("--toolchain requires --preset NAME"))?;
    let lock = toolchain::load_lock(&project)?;
    let artifact = toolchain::select_locked_artifact(&project, &lock, &host, &preset)?;
    let sha256 = artifact.sha256.to_ascii_lowercase();
    Ok(ArchiveSelection {
        kind: ArchiveKind::CrossToolchain,
        configuration_source: toolchain::lock_file_path(&project).display().to_string(),
        configuration_kind: "aros-toolchains.lock.toml",
        transport_source: "aros-toolchains.lock.toml",
        project,
        host,
        host_selection,
        release_id: Some(lock.release_id.clone()),
        target_profile: Some(artifact.target_profile.clone()),
        target_triple: Some(artifact.target_triple.clone()),
        llvm_version: artifact.llvm_version.clone(),
        url: lock
            .asset_url(artifact)
            .map_err(|error| miette::miette!("invalid locked archive URL: {error}"))?,
        cache_path: archive_cache_path(&sha256)?,
        sha256,
        expected_size: artifact.size,
    })
}

fn canonical_project(project: &Path) -> Result<PathBuf> {
    let canonical = project.canonicalize().map_err(|error| {
        miette::miette!(
            "failed to resolve --project '{}': {error}",
            project.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(miette::miette!(
            "--project '{}' is not a directory",
            canonical.display()
        ));
    }
    if !crate::repo::is_repo_root(&canonical) {
        return Err(miette::miette!(
            "--project '{}' is not an AROS source checkout",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn target_configuration_source(project: &Path) -> String {
    let path = crate::repo::targets_file(project);
    if path.is_file() {
        path.display().to_string()
    } else {
        "<built-in aros-targets.toml>".to_owned()
    }
}

fn target_configuration_kind(project: &Path) -> &'static str {
    if project.join(crate::repo::TARGETS_FILE).is_file() {
        "aros-targets.toml host_compiler"
    } else {
        "embedded aros-tools host_compiler contract"
    }
}

fn observe_archive_entry(path: &Path) -> ArchiveCacheEntry {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            ArchiveCacheEntry {
                state: ArchiveCacheEntryState::PresentUnverified,
                metadata_size: Some(metadata.len()),
            }
        }
        Ok(_) => ArchiveCacheEntry {
            state: ArchiveCacheEntryState::Unsafe,
            metadata_size: None,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ArchiveCacheEntry {
            state: ArchiveCacheEntryState::Missing,
            metadata_size: None,
        },
        Err(_) => ArchiveCacheEntry {
            state: ArchiveCacheEntryState::Inaccessible,
            metadata_size: None,
        },
    }
}

fn source_request(selector: CacheSourceSelector) -> Result<SourceCacheRequest> {
    match selector {
        CacheSourceSelector {
            source_lock: Some(path),
            compatibility_ports_lock: None,
            source_fetch_plan: None,
        } => SourceCacheRequest::from_source_lock(
            &read_selector(&path, "source lock").map_err(|error| contract_error(&error))?,
        )
        .map_err(|error| contract_error(&error)),
        CacheSourceSelector {
            source_lock: None,
            compatibility_ports_lock: Some(path),
            source_fetch_plan: None,
        } => SourceCacheRequest::from_compatibility_ports_lock(
            &read_selector(&path, "compatibility ports lock")
                .map_err(|error| contract_error(&error))?,
        )
        .map_err(|error| contract_error(&error)),
        CacheSourceSelector {
            source_lock: None,
            compatibility_ports_lock: None,
            source_fetch_plan: Some(path),
        } => SourceCacheRequest::from_product_source_fetch_plan(
            &read_selector(&path, "product source-fetch plan")
                .map_err(|error| contract_error(&error))?,
        )
        .map_err(|error| contract_error(&error)),
        _ => Err(miette::miette!(
            "exactly one source-cache selector is required"
        )),
    }
}

fn cargo_request(selector: CacheCargoSelector) -> Result<CargoVendorRequest> {
    let cargo = match selector.cargo {
        Some(path) => path,
        None => which::which("cargo")
            .map_err(|_| miette::miette!("could not resolve Cargo from PATH; pass --cargo FILE"))?,
    };
    Ok(CargoVendorRequest {
        producer_dir: selector.producer_dir,
        tools_dir: selector.tools_dir,
        tools_tree: None,
        cargo,
        cache_dir: selector.dir,
    })
}

fn contract_error(error: &ContractError) -> miette::Report {
    crate::observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
}

fn print_status_human(report: &CacheStatus) {
    aros_common::outputln!(
        "Cache status (passive; no directories, locks, network, or backend processes are created):"
    );
    for family in &report.families {
        aros_common::outputln!("  {}: {}", family.family.as_str(), family.status.as_str());
        if let Some(root) = &family.root {
            aros_common::outputln!(
                "    root: {} ({}, {})",
                root.root.path.display(),
                root.root.origin.as_str(),
                root.state.as_str()
            );
        }
        print_backend_details(family);
        if let Some(detail) = family.detail {
            aros_common::outputln!("    boundary: {detail}");
        }
    }
}

fn print_backend_details(family: &CacheFamilyStatus) {
    for backend in &family.backends {
        let executable = backend.executable.as_ref().map_or_else(
            || "not on PATH".to_owned(),
            |path| path.display().to_string(),
        );
        aros_common::outputln!(
            "    {}: {}; {}; configuration {}",
            backend.backend.program(),
            backend.state.as_str(),
            executable,
            backend.configuration_scope.as_str()
        );
        if !backend.configuration_sources.is_empty() {
            let sources = backend
                .configuration_sources
                .iter()
                .map(|source| source.variable)
                .collect::<Vec<_>>()
                .join(", ");
            aros_common::outputln!("      environment provenance: {sources} (values redacted)");
        }
    }
}

fn print_compiler_status_human(report: &CompilerCacheStatus) {
    let selected = report
        .selected_backend
        .map_or_else(|| "none".to_owned(), |backend| backend.program().to_owned());
    aros_common::outputln!(
        "Compiler cache status (passive; requested {}, selected {}):",
        match report.requested_backend {
            CompilerBackendChoice::Auto => "auto",
            CompilerBackendChoice::Off => "off",
            CompilerBackendChoice::Sccache => "sccache",
            CompilerBackendChoice::Ccache => "ccache",
        },
        selected
    );
    if let Some(root) = &report.root {
        aros_common::outputln!(
            "  candidate root: {} ({}, {}; status only, not applied)",
            root.root.path.display(),
            root.root.origin.as_str(),
            root.state.as_str(),
        );
    }
    let family = CacheFamilyStatus {
        family: aros_cache::CacheFamily::Compiler,
        status: aros_cache::CacheFamilyStatusKind::Configured,
        capabilities: vec![aros_cache::CacheCapability::Status],
        root: None,
        backends: report.backends.clone(),
        detail: Some(
            "no backend process or configuration file was queried; storage scope remains uninspected",
        ),
    };
    print_backend_details(&family);
    aros_common::outputln!("  boundary: {}", family.detail.unwrap());
}

fn print_source_status_human(report: &SourceCacheStatus) {
    aros_common::outputln!(
        "Source cache status (passive): {} ({}, {})",
        report.root.root.path.display(),
        report.root.root.origin.as_str(),
        report.root.state.as_str()
    );
}

fn print_source_list_human(report: &SourceCacheList) {
    aros_common::outputln!(
        "Source cache list ({}, request {}):",
        report.request_kind.as_str(),
        report.request_sha256
    );
    for entry in &report.entries {
        aros_common::outputln!(
            "  {}: {} ({:?}; {}; {} candidate(s))",
            entry.role,
            entry.filename,
            entry.state,
            entry.representation.as_str(),
            entry.candidates.len(),
        );
    }
}

fn print_source_fetch_human(report: &SourceCacheFetch) {
    aros_common::outputln!(
        "Source cache fetch ({}, request {}): {} measured object(s)",
        report.request_kind.as_str(),
        report.request_sha256,
        report.entries.len()
    );
    for entry in &report.entries {
        aros_common::outputln!(
            "  {} {} {} {} ({})",
            entry.role,
            entry.integrity.as_str(),
            entry.representation.as_str(),
            entry.sha256,
            entry.filename
        );
    }
}

fn print_source_verify_human(report: &SourceCacheVerification) {
    aros_common::outputln!(
        "Source cache verify ({}, request {}): {} measured object(s)",
        report.request_kind.as_str(),
        report.request_sha256,
        report.entries.len()
    );
    for entry in &report.entries {
        aros_common::outputln!(
            "  {} {} {} {} ({})",
            entry.role,
            entry.integrity.as_str(),
            entry.representation.as_str(),
            entry.sha256,
            entry.filename
        );
    }
}

fn print_archive_status_human(report: &ArchiveCacheStatus) {
    aros_common::outputln!(
        "Compiler archive cache status (passive): {} ({}, {})",
        report.root.root.path.display(),
        report.root.root.origin.as_str(),
        report.root.state.as_str()
    );
    aros_common::outputln!("  object layout: {}", report.object_layout);
    aros_common::outputln!("  operations: status, list, fetch, verify");
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_status_human(report: &CargoVendorStatus) {
    aros_common::outputln!(
        "Cargo vendor cache status (passive): {} ({}, {})",
        report.root.root.path.display(),
        report.root.root.origin.as_str(),
        report.root.state.as_str()
    );
    aros_common::outputln!("  object layout: {}", report.object_layout);
    aros_common::outputln!("  operations: status, list, fetch, verify");
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_list_human(report: &CargoVendorList) {
    aros_common::outputln!("Cargo vendor cache selection:");
    print_cargo_selection_human(&report.selection);
    match &report.generation {
        Some(generation) => aros_common::outputln!(
            "  generation receipt: present ({} packages; vendor {})",
            generation.package_count,
            generation.vendor_tree_sha256
        ),
        None => aros_common::outputln!("  generation receipt: missing"),
    }
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_generation_human(report: &CargoVendorGeneration, offline: bool) {
    aros_common::outputln!("Cargo vendor {}:", report.operation);
    print_cargo_selection_human(&report.selection);
    aros_common::outputln!("  generation: {}", report.generation_dir.display());
    aros_common::outputln!("  packages: {}", report.package_count);
    aros_common::outputln!("  vendor tree SHA-256: {}", report.vendor_tree_sha256);
    aros_common::outputln!(
        "  configuration template SHA-256: {}",
        report.configuration_template_sha256
    );
    if report.operation == "cargo.fetch" {
        aros_common::outputln!("  offline: {offline}");
    }
}

fn print_cargo_selection_human(selection: &aros_toolchain::cargo_vendor::CargoVendorSelection) {
    aros_common::outputln!("  producer: {}", selection.producer_dir.display());
    aros_common::outputln!("  tools: {}", selection.tools_dir.display());
    aros_common::outputln!("  tools Git tree: {}", selection.tools_tree);
    aros_common::outputln!("  Cargo.lock SHA-256: {}", selection.cargo_lock_sha256);
    aros_common::outputln!("  Rust/Cargo channel: {}", selection.rust_channel);
    aros_common::outputln!(
        "  Cargo: {} ({})",
        selection.cargo_invocation_path.display(),
        selection.cargo_version
    );
    aros_common::outputln!("  selection SHA-256: {}", selection.generation);
}

fn print_archive_list_human(report: &ArchiveCacheList) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!(
        "  cache entry: {}{}",
        archive_entry_state_label(report.entry.state),
        report
            .entry
            .metadata_size
            .map_or_else(String::new, |size| format!(" ({size} metadata bytes)"))
    );
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_archive_fetch_human(report: &ArchiveCacheFetch) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!(
        "  prior cache entry: {}; offline: {}; refresh requested: {}",
        archive_entry_state_label(report.prior_entry.state),
        report.offline,
        report.refresh_requested
    );
    aros_common::outputln!("  verified: {}", report.verification_scope);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_archive_verify_human(report: &ArchiveCacheVerification) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!("  verified: {}", report.verification_scope);
    aros_common::outputln!("  not verified: {}", report.not_verified.join("; "));
}

fn print_archive_selection_human(selection: &ArchiveSelection) {
    let kind = match selection.kind {
        ArchiveKind::HostCompiler => "host compiler",
        ArchiveKind::CrossToolchain => "cross toolchain",
    };
    let host_origin = match selection.host_selection {
        ArchiveHostSelection::Explicit => "explicit host",
        ArchiveHostSelection::RunningHost => "running host",
    };
    aros_common::outputln!("Compiler archive ({kind}; {}):", selection.host);
    aros_common::outputln!("  host selection: {host_origin}");
    aros_common::outputln!(
        "  configuration: {} ({})",
        selection.configuration_source,
        selection.configuration_kind
    );
    aros_common::outputln!("  archive transport: {}", selection.transport_source);
    if let Some(release_id) = &selection.release_id {
        aros_common::outputln!("  release: {release_id}");
    }
    if let Some(profile) = &selection.target_profile {
        aros_common::outputln!("  target preset: {profile}");
    }
    if let Some(triple) = &selection.target_triple {
        aros_common::outputln!("  target triple: {triple}");
    }
    if let Some(version) = &selection.llvm_version {
        aros_common::outputln!("  LLVM version: {version}");
    }
    aros_common::outputln!("  archive SHA-256: {}", selection.sha256);
    match selection.expected_size {
        Some(size) => aros_common::outputln!("  expected size: {size} bytes"),
        None => aros_common::outputln!("  expected size: unknown (bounded during download)"),
    }
    aros_common::outputln!("  cache path: {}", selection.cache_path.display());
}

const fn archive_entry_state_label(state: ArchiveCacheEntryState) -> &'static str {
    match state {
        ArchiveCacheEntryState::Missing => "missing",
        ArchiveCacheEntryState::PresentUnverified => "present, unverified",
        ArchiveCacheEntryState::Unsafe => "unsafe",
        ArchiveCacheEntryState::Inaccessible => "inaccessible",
    }
}

fn print_json<T: serde::Serialize>(value: &T, operation: &str) -> Result<()> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| miette::miette!("could not serialize {operation}: {error}"))?;
    aros_common::outputln!("{document}");
    Ok(())
}
