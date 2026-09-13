//! Rendering adapter for the bounded cache-status commands.

use std::fs;
use std::path::{Path, PathBuf};

use crate::artifact::{
    archive_cache_path, archive_cache_request, obtain_archive, open_verified_archive,
    require_sha256,
};
use crate::host_compiler;
use crate::toolchain;
use crate::toolchain_management::ResultFormat;
use crate::{CacheArchiveSelector, CacheCargoSelector, CacheGenmfSelector, CacheSourceSelector};
use aros_cache::{
    apply_removal, cache_status, compiler_cache_status, keep_validated, preview_removal, release,
    CacheCapability, CacheFamily, CacheFamilyStatus, CacheRemovalPreview, CacheRemovalResult,
    CacheRetentionRecord, CacheRetentionRelease, CacheSideEffects, CacheStatus,
    CompilerBackendChoice, CompilerCacheStatus, RootObservation,
};
use aros_toolchain::{
    cargo_vendor::{
        cargo_vendor_status, fetch_vendor_generation, list_vendor_generation,
        retain_vendor_generation, select_vendor_generation, select_vendor_lifecycle_object,
        verify_vendor_generation, CargoVendorGeneration, CargoVendorRequest, CargoVendorSelection,
        CargoVendorStatus, CARGO_VENDOR_FETCH_SCHEMA, CARGO_VENDOR_LIST_SCHEMA,
        CARGO_VENDOR_VERIFY_SCHEMA,
    },
    source_cache::{
        fetch_request, list as list_source_cache, retain_request, select_lifecycle_object,
        status as source_cache_status, verify_request, SourceCacheFetch, SourceCacheList,
        SourceCacheStatus, SourceCacheVerification, SOURCE_CACHE_KEEP_SCHEMA,
    },
    source_cache_request::{read_selector, SourceCacheRequest},
    ContractError,
};
use aros_verify::genmf_cache::{
    list as list_genmf_cache, refresh as refresh_genmf_cache, retain as retain_genmf_cache,
    select_lifecycle_object as select_genmf_lifecycle_object, status as genmf_cache_status,
    verify as verify_genmf_cache, GenmfCacheEntrySelection, GenmfCacheError, GenmfCacheList,
    GenmfCacheRequest, GenmfCacheSelection, GenmfCacheStatus, GenmfCacheVerification,
    GENMF_KEEP_SCHEMA,
};
use miette::Result;
use serde::Serialize;

const ARCHIVE_STATUS_SCHEMA: &str = "aros-cache-archives-status-v1";
const ARCHIVE_LIST_SCHEMA: &str = "aros-cache-archives-list-v1";
const ARCHIVE_FETCH_SCHEMA: &str = "aros-cache-archives-fetch-v1";
const ARCHIVE_VERIFY_SCHEMA: &str = "aros-cache-archives-verify-v1";
const ARCHIVE_KEEP_SCHEMA: &str = "aros-cache-archives-keep-v1";
const ARCHIVE_RELEASE_SCHEMA: &str = "aros-cache-archives-release-v1";
const ARCHIVE_REMOVE_SCHEMA: &str = "aros-cache-archives-remove-v1";
const CARGO_KEEP_SCHEMA: &str = "aros-cache-cargo-keep-v1";
const CARGO_RELEASE_SCHEMA: &str = "aros-cache-cargo-release-v1";
const CARGO_REMOVE_SCHEMA: &str = "aros-cache-cargo-remove-v1";
const SOURCE_RELEASE_SCHEMA: &str = "aros-cache-sources-release-v1";
const SOURCE_REMOVE_SCHEMA: &str = "aros-cache-sources-remove-v1";
const GENMF_RELEASE_SCHEMA: &str = "aros-cache-genmf-release-v1";
const GENMF_REMOVE_SCHEMA: &str = "aros-cache-genmf-remove-v1";
const ARCHIVE_REMOVAL_RECOVERABILITY: &str = "restore only through an explicit cache archives fetch for the same declared host or toolchain identity; the command never redownloads or reconstructs archive bytes";
const ARCHIVE_REMOVAL_OFFLINE_IMPACT: &str = "offline archive fetch and any consumer requiring these exact bytes will fail until the declared archive is restored and verified";
const CARGO_REMOVAL_RECOVERABILITY: &str = "restore only through an explicit online cache cargo fetch with the same producer, tools, Cargo and cache selection; the command never uses global Cargo state";
const CARGO_REMOVAL_OFFLINE_IMPACT: &str = "offline cache cargo fetch and native producer execution requiring this generation will fail until an exact verified generation is restored";
const SOURCE_REMOVAL_RECOVERABILITY: &str = "restore only through an explicit cache sources fetch with the same reviewed selector; the command never guesses an origin, redownloads automatically, or reconstructs source bytes";
const SOURCE_REMOVAL_OFFLINE_IMPACT: &str = "offline source fetch and any producer or compatibility consumer requiring this exact role will fail until the reviewed closure is restored and verified";
const GENMF_REMOVAL_RECOVERABILITY: &str = "restore only through an explicit cache genmf refresh with the same source, template, generator and Python selection; the command never regenerates automatically or reconstructs an unselected generation";
const GENMF_REMOVAL_OFFLINE_IMPACT: &str = "verification and reference-shape comparison requiring this exact generation will fail until the matching immutable expansion is refreshed and verified";

#[derive(Serialize)]
struct ArchiveCacheStatus {
    schema: &'static str,
    operation: &'static str,
    observation: &'static str,
    side_effects: CacheSideEffects,
    capabilities: [CacheCapability; 7],
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
struct ArchiveCacheRetention {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    retention: CacheRetentionRecord,
    boundary: &'static str,
}

#[derive(Serialize)]
struct ArchiveCacheRelease {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    release: CacheRemovalResult,
    boundary: &'static str,
}

#[derive(Serialize)]
struct ArchiveCacheRemovalPreview {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    preview: CacheRemovalPreview,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct ArchiveCacheRemovalApplied {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: ArchiveSelection,
    removal: CacheRemovalResult,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
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

#[derive(Serialize)]
struct CargoVendorRetention {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: CargoVendorSelection,
    retention: CacheRetentionRecord,
    boundary: &'static str,
}

#[derive(Serialize)]
struct CargoVendorRelease {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    release: CacheRemovalResult,
    boundary: &'static str,
}

#[derive(Serialize)]
struct CargoVendorRemovalPreview {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: CargoVendorSelection,
    preview: CacheRemovalPreview,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct CargoVendorRemovalApplied {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: CargoVendorSelection,
    removal: CacheRemovalResult,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct SourceCacheRetention {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: SourceCacheRequest,
    retention: CacheRetentionRecord,
    boundary: &'static str,
}

#[derive(Serialize)]
struct SourceCacheRelease {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    release: CacheRemovalResult,
    boundary: &'static str,
}

#[derive(Serialize)]
struct SourceCacheRemovalPreview {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: SourceCacheRequest,
    role: String,
    preview: CacheRemovalPreview,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct GenmfCacheRetention {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: GenmfCacheSelection,
    retention: CacheRetentionRecord,
    boundary: &'static str,
}

#[derive(Serialize)]
struct GenmfCacheRelease {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    release: CacheRemovalResult,
    boundary: &'static str,
}

#[derive(Serialize)]
struct GenmfCacheRemovalPreview {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: GenmfCacheEntrySelection,
    preview: CacheRemovalPreview,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct GenmfCacheRemovalApplied {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: GenmfCacheEntrySelection,
    removal: CacheRemovalResult,
    recoverability: &'static str,
    offline_impact: &'static str,
    boundary: &'static str,
}

#[derive(Serialize)]
struct SourceCacheRemovalApplied {
    schema: &'static str,
    operation: &'static str,
    side_effects: CacheSideEffects,
    selection: SourceCacheRequest,
    role: String,
    removal: CacheRemovalResult,
    recoverability: &'static str,
    offline_impact: &'static str,
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

/// Retain a fully verified reviewed source closure under one named reference.
///
/// # Errors
///
/// Returns a structured lifecycle diagnostic when a selected object is
/// missing, unsafe, changed, actively consumed, or cannot be retained under
/// the supplied portable name. The command never downloads, replaces or
/// deletes source bytes.
pub fn source_keep(
    selector: CacheSourceSelector,
    dir: &Path,
    name: &str,
    format: ResultFormat,
) -> Result<()> {
    let request = source_request(selector)?;
    let retention = retain_request(dir, &request, name).map_err(|error| contract_error(&error))?;
    let report = SourceCacheRetention {
        schema: SOURCE_CACHE_KEEP_SCHEMA,
        operation: "sources.keep",
        side_effects: lifecycle_keep_side_effects(),
        selection: request,
        retention,
        boundary: "keep verifies and retains every exact role in one reviewed source closure under a no-clobber named reference; it neither downloads, replaces, nor deletes source bytes",
    };
    match format {
        ResultFormat::Human => print_source_retention_human(&report),
        ResultFormat::Json => print_json(&report, "source cache keep")?,
    }
    Ok(())
}

/// Release one named source-cache retention reference without deleting bytes.
///
/// # Errors
///
/// Returns a structured lifecycle diagnostic when the root or receipt is
/// missing, unsafe, substituted or malformed.
pub fn source_release(dir: &Path, name: &str, format: ResultFormat) -> Result<()> {
    let release = release(&CacheRetentionRelease {
        family: CacheFamily::Sources,
        cache_root: dir.to_path_buf(),
        name: name.to_owned(),
    })
    .map_err(|error| miette::miette!(error))?;
    let report = SourceCacheRelease {
        schema: SOURCE_RELEASE_SCHEMA,
        operation: "sources.release",
        side_effects: lifecycle_release_side_effects(),
        release,
        boundary: "release removes one named source retention receipt only; it never enumerates or deletes source-cache bytes",
    };
    match format {
        ResultFormat::Human => print_source_release_human(&report),
        ResultFormat::Json => print_json(&report, "source cache release")?,
    }
    Ok(())
}

/// Preview or token-confirm removal of one role-selected source object.
///
/// # Errors
///
/// Returns a structured lifecycle diagnostic when the role is not selected by
/// the reviewed closure, the object is unsafe/changed, retention blocks it, or
/// an active reader or writer lease prevents removal.
pub fn source_remove(
    selector: CacheSourceSelector,
    dir: &Path,
    role: &str,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    let request = source_request(selector)?;
    let object =
        select_lifecycle_object(dir, &request, role).map_err(|error| contract_error(&error))?;
    if let Some(apply_token) = apply_token {
        let removal =
            apply_removal(&object, apply_token).map_err(|error| miette::miette!(error))?;
        let report = SourceCacheRemovalApplied {
            schema: SOURCE_REMOVE_SCHEMA,
            operation: "sources.remove.apply",
            side_effects: lifecycle_remove_apply_side_effects(),
            selection: request,
            role: role.to_owned(),
            removal,
            recoverability: SOURCE_REMOVAL_RECOVERABILITY,
            offline_impact: SOURCE_REMOVAL_OFFLINE_IMPACT,
            boundary: "apply removes only the direct object selected by one reviewed semantic role after token, retention, reader/writer lease, identity and content bindings still match; no source-cache root scan occurs",
        };
        match format {
            ResultFormat::Human => print_source_removal_applied_human(&report),
            ResultFormat::Json => print_json(&report, "source cache remove apply")?,
        }
    } else {
        let preview = preview_removal(&object).map_err(|error| miette::miette!(error))?;
        let report = SourceCacheRemovalPreview {
            schema: SOURCE_REMOVE_SCHEMA,
            operation: "sources.remove.preview",
            side_effects: lifecycle_remove_preview_side_effects(),
            selection: request,
            role: role.to_owned(),
            preview,
            recoverability: SOURCE_REMOVAL_RECOVERABILITY,
            offline_impact: SOURCE_REMOVAL_OFFLINE_IMPACT,
            boundary: "preview measures only the direct object selected by one reviewed semantic role and reports retention blockers without creating state, taking a lease, or deleting data; pass its apply_token back with --apply to request removal",
        };
        match format {
            ResultFormat::Human => print_source_removal_preview_human(&report),
            ResultFormat::Json => print_json(&report, "source cache remove preview")?,
        }
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

/// Retain one verified Cargo vendor generation under a named no-clobber
/// reference.
///
/// # Errors
///
/// Returns a structured selection, vendor-integrity or lifecycle diagnostic.
/// The selected generation is revalidated while its exclusive lifecycle lock
/// is held; this command neither resolves dependencies nor changes bytes.
pub fn cargo_keep(selector: CacheCargoSelector, name: &str, format: ResultFormat) -> Result<()> {
    let request = cargo_request(selector)?;
    let (selection, retention) =
        retain_vendor_generation(&request, name).map_err(|error| contract_error(&error))?;
    let report = CargoVendorRetention {
        schema: CARGO_KEEP_SCHEMA,
        operation: "cargo.keep",
        side_effects: lifecycle_keep_side_effects(),
        selection,
        retention,
        boundary: "keep retains one fully verified immutable Cargo vendor generation under a named reference; it never resolves dependencies, rewrites Cargo inputs, replaces data, or deletes bytes",
    };
    match format {
        ResultFormat::Human => print_cargo_retention_human(&report),
        ResultFormat::Json => print_json(&report, "cargo cache keep")?,
    }
    Ok(())
}

/// Release one named Cargo vendor retention reference without deleting data.
///
/// # Errors
///
/// Returns a structured lifecycle diagnostic for an invalid root, name or
/// receipt. It cannot enumerate or remove Cargo generations.
pub fn cargo_release(dir: &Path, name: &str, format: ResultFormat) -> Result<()> {
    let release = release(&CacheRetentionRelease {
        family: CacheFamily::Cargo,
        cache_root: dir.to_owned(),
        name: name.to_owned(),
    })
    .map_err(|error| miette::miette!(error))?;
    let report = CargoVendorRelease {
        schema: CARGO_RELEASE_SCHEMA,
        operation: "cargo.release",
        side_effects: lifecycle_release_side_effects(),
        release,
        boundary: "release removes one named retention receipt only; it neither inspects nor deletes Cargo vendor generations",
    };
    match format {
        ResultFormat::Human => print_cargo_release_human(&report),
        ResultFormat::Json => print_json(&report, "cargo cache release")?,
    }
    Ok(())
}

/// Preview or token-confirm removal of one exact Cargo vendor generation.
///
/// # Errors
///
/// Returns a structured selection or lifecycle diagnostic. Without `apply`,
/// it only emits a five-minute preview. With the exact token, removal
/// remeasures the selected tree under an exclusive lifecycle lock.
pub fn cargo_remove(
    selector: CacheCargoSelector,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    let request = cargo_request(selector)?;
    let (selection, object) =
        select_vendor_lifecycle_object(&request).map_err(|error| contract_error(&error))?;
    if let Some(apply_token) = apply_token {
        let removal =
            apply_removal(&object, apply_token).map_err(|error| miette::miette!(error))?;
        let report = CargoVendorRemovalApplied {
            schema: CARGO_REMOVE_SCHEMA,
            operation: "cargo.remove.apply",
            side_effects: lifecycle_remove_apply_side_effects(),
            selection,
            removal,
            recoverability: CARGO_REMOVAL_RECOVERABILITY,
            offline_impact: CARGO_REMOVAL_OFFLINE_IMPACT,
            boundary: "apply removes only the preview-bound immutable Cargo generation; it never clears a parent root, global CARGO_HOME, user Cargo credentials, or an unselected generation",
        };
        match format {
            ResultFormat::Human => print_cargo_removal_applied_human(&report),
            ResultFormat::Json => print_json(&report, "cargo cache remove apply")?,
        }
    } else {
        let preview = preview_removal(&object).map_err(|error| miette::miette!(error))?;
        let report = CargoVendorRemovalPreview {
            schema: CARGO_REMOVE_SCHEMA,
            operation: "cargo.remove.preview",
            side_effects: lifecycle_remove_preview_side_effects(),
            selection,
            preview,
            recoverability: CARGO_REMOVAL_RECOVERABILITY,
            offline_impact: CARGO_REMOVAL_OFFLINE_IMPACT,
            boundary: "preview measures one exact immutable Cargo generation and reports retained or active-use blockers; it never removes data or scans a cache root",
        };
        match format {
            ResultFormat::Human => print_cargo_removal_preview_human(&report),
            ResultFormat::Json => print_json(&report, "cargo cache remove preview")?,
        }
    }
    Ok(())
}

/// Render a passive observation of one explicit GenMF cache root.
///
/// # Errors
///
/// Returns a configuration diagnostic only for an invalid absolute root or
/// output serialization failure. It never selects source inputs, invokes
/// Python, scans generations, or creates cache state.
pub fn genmf_status(dir: &Path, format: ResultFormat) -> Result<()> {
    let report = genmf_cache_status(dir).map_err(|error| genmf_error(&error))?;
    match format {
        ResultFormat::Human => print_genmf_status_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache status")?,
    }
    Ok(())
}

/// Render metadata-only state for all current GenMF source selections.
///
/// # Errors
///
/// Returns a source/interpreter/cache configuration diagnostic. This operation
/// hashes selected inputs and performs a bounded interpreter-version probe, but
/// never reads an expansion payload, invokes GenMF, locks, or creates cache state.
pub fn genmf_list(selector: CacheGenmfSelector, format: ResultFormat) -> Result<()> {
    let request = genmf_request(selector)?;
    let report = list_genmf_cache(&request).map_err(|error| genmf_error(&error))?;
    match format {
        ResultFormat::Human => print_genmf_list_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache list")?,
    }
    Ok(())
}

/// Fully verify every immutable GenMF generation selected by current inputs.
///
/// # Errors
///
/// Returns a source/interpreter/cache-integrity diagnostic. It hashes the
/// selected source and final expansion generations. Selection performs only a
/// bounded Python-version probe; this command never invokes GenMF, takes a
/// generation lock, repairs, or replaces state.
pub fn genmf_verify(selector: CacheGenmfSelector, format: ResultFormat) -> Result<()> {
    let request = genmf_request(selector)?;
    let report = verify_genmf_cache(&request).map_err(|error| genmf_error(&error))?;
    match format {
        ResultFormat::Human => print_genmf_verification_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache verify")?,
    }
    Ok(())
}

/// Regenerate every selected GenMF reference with cooperative Ctrl-C handling.
///
/// # Errors
///
/// Returns a source/interpreter/cache or upstream-GenMF diagnostic. Refresh
/// runs only the selected resolved Python and immutable inputs; it publishes a
/// missing complete generation or proves an existing generation is identical.
pub async fn genmf_refresh(selector: CacheGenmfSelector, format: ResultFormat) -> Result<()> {
    let request = genmf_request(selector)?;
    let cancellation = aros_common::CancellationToken::default();
    let worker_token = cancellation.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || refresh_genmf_cache(&request, &worker_token));
    let report = tokio::select! {
        result = &mut worker => result.map_err(|_| miette::miette!("GenMF cache refresh worker terminated unexpectedly"))?,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                cancellation.cancel();
            }
            (&mut worker).await.map_err(|_| miette::miette!("GenMF cache refresh worker terminated unexpectedly"))?
        }
    }
    .map_err(|error| genmf_error(&error))?;
    match format {
        ResultFormat::Human => print_genmf_verification_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache refresh")?,
    }
    Ok(())
}

/// Retain the complete current verified GenMF selection under one name.
///
/// # Errors
///
/// Returns a source, interpreter, cache-integrity or lifecycle diagnostic. The
/// full source/template/generator/interpreter closure is reselected and every
/// immutable expansion reverified while exclusive lifecycle locks are held.
/// It does not invoke GenMF or change expansion bytes.
pub fn genmf_keep(selector: CacheGenmfSelector, name: &str, format: ResultFormat) -> Result<()> {
    let request = genmf_request(selector)?;
    let (selection, retention) =
        retain_genmf_cache(&request, name).map_err(|error| genmf_error(&error))?;
    let report = GenmfCacheRetention {
        schema: GENMF_KEEP_SCHEMA,
        operation: "genmf.keep",
        side_effects: lifecycle_keep_side_effects(),
        selection,
        retention,
        boundary: "keep retains every fully verified immutable expansion in the exact current GenMF source/template/generator/interpreter closure under one named reference; it never invokes GenMF, replaces data, or deletes bytes",
    };
    match format {
        ResultFormat::Human => print_genmf_retention_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache keep")?,
    }
    Ok(())
}

/// Release one named GenMF retention reference without removing expansions.
///
/// # Errors
///
/// Returns a lifecycle diagnostic when the root or named receipt is absent,
/// unsafe, malformed, or bound to a different cache family.
pub fn genmf_release(dir: &Path, name: &str, format: ResultFormat) -> Result<()> {
    let release = release(&CacheRetentionRelease {
        family: CacheFamily::Genmf,
        cache_root: dir.to_path_buf(),
        name: name.to_owned(),
    })
    .map_err(|error| miette::miette!(error))?;
    let report = GenmfCacheRelease {
        schema: GENMF_RELEASE_SCHEMA,
        operation: "genmf.release",
        side_effects: lifecycle_release_side_effects(),
        release,
        boundary: "release removes one named GenMF retention receipt only; it never enumerates, regenerates, or removes expansion bytes",
    };
    match format {
        ResultFormat::Human => print_genmf_release_human(&report),
        ResultFormat::Json => print_json(&report, "GenMF cache release")?,
    }
    Ok(())
}

/// Preview or token-confirm removal of one source-selected GenMF generation.
///
/// # Errors
///
/// Returns a source, interpreter, selection or lifecycle diagnostic. The
/// requested input must be part of the current exact selection; the command
/// never enumerates a cache root or removes an unselected generation.
pub fn genmf_remove(
    selector: CacheGenmfSelector,
    source_relative_path: &str,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    let request = genmf_request(selector)?;
    let (selection, object) = select_genmf_lifecycle_object(&request, source_relative_path)
        .map_err(|error| genmf_error(&error))?;
    if let Some(apply_token) = apply_token {
        let removal =
            apply_removal(&object, apply_token).map_err(|error| miette::miette!(error))?;
        let report = GenmfCacheRemovalApplied {
            schema: GENMF_REMOVE_SCHEMA,
            operation: "genmf.remove.apply",
            side_effects: lifecycle_remove_apply_side_effects(),
            selection,
            removal,
            recoverability: GENMF_REMOVAL_RECOVERABILITY,
            offline_impact: GENMF_REMOVAL_OFFLINE_IMPACT,
            boundary: "apply removes only the immutable generation selected by one current source-root-relative MMake input after token, retention, reader/writer lease, snapshot and payload bindings still match; it never scans, clears or prunes a GenMF cache root",
        };
        match format {
            ResultFormat::Human => print_genmf_removal_applied_human(&report),
            ResultFormat::Json => print_json(&report, "GenMF cache remove apply")?,
        }
    } else {
        let preview = preview_removal(&object).map_err(|error| miette::miette!(error))?;
        let report = GenmfCacheRemovalPreview {
            schema: GENMF_REMOVE_SCHEMA,
            operation: "genmf.remove.preview",
            side_effects: lifecycle_remove_preview_side_effects(),
            selection,
            preview,
            recoverability: GENMF_REMOVAL_RECOVERABILITY,
            offline_impact: GENMF_REMOVAL_OFFLINE_IMPACT,
            boundary: "preview measures only the immutable generation selected by one current source-root-relative MMake input and reports retention blockers without creating state, taking a lease, or deleting data; pass its apply_token back with --apply to request removal",
        };
        match format {
            ResultFormat::Human => print_genmf_removal_preview_human(&report),
            ResultFormat::Json => print_json(&report, "GenMF cache remove preview")?,
        }
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
            CacheCapability::Keep,
            CacheCapability::Release,
            CacheCapability::Remove,
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
    let archive = open_verified_archive(&selection.sha256, selection.expected_size)?;
    if archive.path() != selection.cache_path {
        return Err(miette::miette!(
            "selected archive cache path changed while opening its lifecycle lease"
        ));
    }
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

/// Create a named no-clobber retention reference for one selected archive.
///
/// # Errors
///
/// Returns an error for an invalid selection/reference name, an unsafe cache
/// root, an unavailable archive, or an active lifecycle writer.
pub fn archive_keep(
    selector: CacheArchiveSelector,
    name: &str,
    format: ResultFormat,
) -> Result<()> {
    let selection = archive_selection(selector)?;
    let request = archive_cache_request(&selection.sha256, selection.expected_size)?;
    let expected_path = request.cache_root.join(&request.relative_path);
    if expected_path != selection.cache_path {
        return Err(miette::miette!(
            "selected archive cache path does not match its declared lifecycle identity"
        ));
    }
    let retention = keep_validated(&request, name, || {
        crate::artifact::verify_archive(&expected_path, &selection.sha256, selection.expected_size)
            .map_err(|error| aros_cache::CacheLifecycleError::validation(error.to_string()))
    })
    .map_err(|error| miette::miette!(error))?;
    let report = ArchiveCacheRetention {
        schema: ARCHIVE_KEEP_SCHEMA,
        operation: "archives.keep",
        side_effects: lifecycle_keep_side_effects(),
        selection,
        retention,
        boundary: "keep binds one declared content-addressed archive under an immutable named reference; it neither downloads, installs, replaces, nor deletes archive bytes",
    };
    match format {
        ResultFormat::Human => print_archive_retention_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache keep")?,
    }
    Ok(())
}

/// Release one named archive retention reference without deleting archive bytes.
///
/// # Errors
///
/// Returns an error for an invalid name, unsafe root, or a missing, malformed
/// or substituted retention receipt.
pub fn archive_release(name: &str, format: ResultFormat) -> Result<()> {
    let cache_root = aros_cache::archive_cache_root().map_err(|error| miette::miette!(error))?;
    let release = release(&CacheRetentionRelease {
        family: CacheFamily::Archives,
        cache_root,
        name: name.to_owned(),
    })
    .map_err(|error| miette::miette!(error))?;
    let report = ArchiveCacheRelease {
        schema: ARCHIVE_RELEASE_SCHEMA,
        operation: "archives.release",
        side_effects: lifecycle_release_side_effects(),
        release,
        boundary: "release removes one named retention receipt only; it never enumerates or deletes archive bytes",
    };
    match format {
        ResultFormat::Human => print_archive_release_human(&report),
        ResultFormat::Json => print_json(&report, "archive cache release")?,
    }
    Ok(())
}

/// Preview or token-confirm removal of one exact selected archive.
///
/// Without `apply_token`, the command hashes the selected archive and emits a
/// five-minute token-bound preview. Supplying that exact token repeats every
/// binding check under an exclusive lifecycle lock before removal.
///
/// # Errors
///
/// Returns an error for invalid selection/token/root state, active readers or
/// writers, changed archive bytes, or named retention blockers.
pub fn archive_remove(
    selector: CacheArchiveSelector,
    apply_token: Option<&str>,
    format: ResultFormat,
) -> Result<()> {
    let selection = archive_selection(selector)?;
    let request = archive_cache_request(&selection.sha256, selection.expected_size)?;
    if let Some(apply_token) = apply_token {
        let removal =
            apply_removal(&request, apply_token).map_err(|error| miette::miette!(error))?;
        let report = ArchiveCacheRemovalApplied {
            schema: ARCHIVE_REMOVE_SCHEMA,
            operation: "archives.remove.apply",
            side_effects: lifecycle_remove_apply_side_effects(),
            selection,
            removal,
            recoverability: ARCHIVE_REMOVAL_RECOVERABILITY,
            offline_impact: ARCHIVE_REMOVAL_OFFLINE_IMPACT,
            boundary: "apply removes only the exact previewed archive after token, retention, reader/writer lease, identity and SHA-256 bindings still match; no root-wide scan occurs",
        };
        match format {
            ResultFormat::Human => print_archive_removal_applied_human(&report),
            ResultFormat::Json => print_json(&report, "archive cache remove apply")?,
        }
    } else {
        let preview = preview_removal(&request).map_err(|error| miette::miette!(error))?;
        let report = ArchiveCacheRemovalPreview {
            schema: ARCHIVE_REMOVE_SCHEMA,
            operation: "archives.remove.preview",
            side_effects: lifecycle_remove_preview_side_effects(),
            selection,
            preview,
            recoverability: ARCHIVE_REMOVAL_RECOVERABILITY,
            offline_impact: ARCHIVE_REMOVAL_OFFLINE_IMPACT,
            boundary: "preview hashes one exact selected archive and reports retention blockers without creating state, taking a lease, or deleting data; pass its apply_token back with --apply to request removal",
        };
        match format {
            ResultFormat::Human => print_archive_removal_preview_human(&report),
            ResultFormat::Json => print_json(&report, "archive cache remove preview")?,
        }
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

const fn lifecycle_keep_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: true,
        mutates_state: true,
        network: false,
        backend_process: false,
        locks: true,
        hashes_payloads: true,
    }
}

const fn lifecycle_release_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: true,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: false,
    }
}

const fn lifecycle_remove_preview_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: false,
        network: false,
        backend_process: false,
        locks: false,
        hashes_payloads: true,
    }
}

const fn lifecycle_remove_apply_side_effects() -> CacheSideEffects {
    CacheSideEffects {
        creates_state: false,
        mutates_state: true,
        network: false,
        backend_process: false,
        locks: true,
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

fn genmf_request(selector: CacheGenmfSelector) -> Result<GenmfCacheRequest> {
    let python = match selector.python {
        Some(path) => path,
        None => which::which("python3").map_err(|_| {
            miette::miette!("could not resolve Python from PATH; pass --python FILE")
        })?,
    };
    Ok(GenmfCacheRequest {
        source_dir: selector.source_dir,
        cache_dir: selector.dir,
        python,
        timeout: std::time::Duration::from_secs(selector.timeout_seconds),
    })
}

fn contract_error(error: &ContractError) -> miette::Report {
    crate::observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
}

fn genmf_error(error: &GenmfCacheError) -> miette::Report {
    miette::miette!(error.to_string())
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
    aros_common::outputln!("  operations: status, list, fetch, verify, keep, release, remove");
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

fn print_source_retention_human(report: &SourceCacheRetention) {
    print_source_selection_human(&report.selection);
    aros_common::outputln!("  retention reference: {}", report.retention.name);
    aros_common::outputln!("  retained objects: {}", report.retention.objects.len());
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_source_release_human(report: &SourceCacheRelease) {
    aros_common::outputln!("Source cache retention reference released:");
    aros_common::outputln!("  root: {}", report.release.cache_root.display());
    aros_common::outputln!("  reference: {}", report.release.relative_path);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_source_removal_preview_human(report: &SourceCacheRemovalPreview) {
    print_source_selection_human(&report.selection);
    aros_common::outputln!("  removal role: {}", report.role);
    aros_common::outputln!("  removal eligible: {}", report.preview.eligible);
    if report.preview.blockers.is_empty() {
        aros_common::outputln!("  blockers: none");
    } else {
        aros_common::outputln!(
            "  blockers: {}",
            report
                .preview
                .blockers
                .iter()
                .map(|blocker| blocker.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    aros_common::outputln!("  apply token: {}", report.preview.apply_token);
    aros_common::outputln!("  recovery: {}", report.preview.recovery);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_source_removal_applied_human(report: &SourceCacheRemovalApplied) {
    print_source_selection_human(&report.selection);
    aros_common::outputln!("  removal role: {}", report.role);
    aros_common::outputln!("  removal: {}", report.removal.outcome);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_source_selection_human(request: &SourceCacheRequest) {
    aros_common::outputln!(
        "Source cache selection: {} (request {})",
        request.kind.as_str(),
        request.request_sha256
    );
    for entry in &request.entries {
        aros_common::outputln!("  {}: {}", entry.role, entry.filename);
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
    aros_common::outputln!("  operations: status, list, fetch, verify, keep, release, remove");
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
    aros_common::outputln!("  operations: status, list, fetch, verify, keep, release, remove");
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

fn print_cargo_retention_human(report: &CargoVendorRetention) {
    print_cargo_selection_human(&report.selection);
    aros_common::outputln!("  retention reference: {}", report.retention.name);
    aros_common::outputln!("  retained objects: {}", report.retention.objects.len());
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_release_human(report: &CargoVendorRelease) {
    aros_common::outputln!("Cargo vendor retention reference released:");
    aros_common::outputln!("  root: {}", report.release.cache_root.display());
    aros_common::outputln!("  reference: {}", report.release.relative_path);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_removal_preview_human(report: &CargoVendorRemovalPreview) {
    print_cargo_selection_human(&report.selection);
    aros_common::outputln!("  removal eligible: {}", report.preview.eligible);
    if report.preview.blockers.is_empty() {
        aros_common::outputln!("  blockers: none");
    } else {
        aros_common::outputln!(
            "  blockers: {}",
            report
                .preview
                .blockers
                .iter()
                .map(|blocker| blocker.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    aros_common::outputln!("  apply token: {}", report.preview.apply_token);
    aros_common::outputln!("  recovery: {}", report.preview.recovery);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_cargo_removal_applied_human(report: &CargoVendorRemovalApplied) {
    print_cargo_selection_human(&report.selection);
    aros_common::outputln!("  removal: {}", report.removal.outcome);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_status_human(report: &GenmfCacheStatus) {
    aros_common::outputln!(
        "GenMF cache status (passive): {} ({}, {})",
        report.root.root.path.display(),
        report.root.root.origin.as_str(),
        report.root.state.as_str()
    );
    aros_common::outputln!("  object layout: {}", report.object_layout);
    aros_common::outputln!("  operations: status, list, verify, refresh, keep, release, remove");
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_list_human(report: &GenmfCacheList) {
    aros_common::outputln!(
        "GenMF cache list: {} selected expansion(s)",
        report.entries.len()
    );
    for entry in &report.entries {
        aros_common::outputln!(
            "  {}: {:?} ({})",
            entry.selection.source_relative_path,
            entry.state,
            entry.selection.generation
        );
    }
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_verification_human(report: &GenmfCacheVerification) {
    aros_common::outputln!(
        "GenMF cache {}: {} verified immutable expansion(s)",
        report.operation,
        report.entries.len()
    );
    for entry in &report.entries {
        aros_common::outputln!(
            "  {}: {} bytes, SHA-256 {}",
            entry.selection.source_relative_path,
            entry.output_size,
            entry.output_sha256
        );
    }
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_retention_human(report: &GenmfCacheRetention) {
    aros_common::outputln!(
        "GenMF cache retention: {} selected immutable expansion(s)",
        report.selection.entries.len()
    );
    aros_common::outputln!("  retention reference: {}", report.retention.name);
    aros_common::outputln!("  retained objects: {}", report.retention.objects.len());
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_release_human(report: &GenmfCacheRelease) {
    aros_common::outputln!("GenMF cache retention reference released:");
    aros_common::outputln!("  root: {}", report.release.cache_root.display());
    aros_common::outputln!("  reference: {}", report.release.relative_path);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_removal_preview_human(report: &GenmfCacheRemovalPreview) {
    aros_common::outputln!("  removal input: {}", report.selection.source_relative_path);
    aros_common::outputln!("  generation: {}", report.selection.generation);
    aros_common::outputln!("  removal eligible: {}", report.preview.eligible);
    if report.preview.blockers.is_empty() {
        aros_common::outputln!("  blockers: none");
    } else {
        aros_common::outputln!(
            "  blockers: {}",
            report
                .preview
                .blockers
                .iter()
                .map(|blocker| blocker.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    aros_common::outputln!("  apply token: {}", report.preview.apply_token);
    aros_common::outputln!("  recovery: {}", report.preview.recovery);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_genmf_removal_applied_human(report: &GenmfCacheRemovalApplied) {
    aros_common::outputln!("  removal input: {}", report.selection.source_relative_path);
    aros_common::outputln!("  generation: {}", report.selection.generation);
    aros_common::outputln!("  removal: {}", report.removal.outcome);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
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

fn print_archive_retention_human(report: &ArchiveCacheRetention) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!("  retention reference: {}", report.retention.name);
    aros_common::outputln!("  retained objects: {}", report.retention.objects.len());
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_archive_release_human(report: &ArchiveCacheRelease) {
    aros_common::outputln!("Archive retention reference released:");
    aros_common::outputln!("  root: {}", report.release.cache_root.display());
    aros_common::outputln!("  reference: {}", report.release.relative_path);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_archive_removal_preview_human(report: &ArchiveCacheRemovalPreview) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!("  removal eligible: {}", report.preview.eligible);
    if report.preview.blockers.is_empty() {
        aros_common::outputln!("  blockers: none");
    } else {
        aros_common::outputln!(
            "  blockers: {}",
            report
                .preview
                .blockers
                .iter()
                .map(|blocker| blocker.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    aros_common::outputln!("  expires: {}", report.preview.expires_unix_seconds);
    aros_common::outputln!("  apply token: {}", report.preview.apply_token);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
}

fn print_archive_removal_applied_human(report: &ArchiveCacheRemovalApplied) {
    print_archive_selection_human(&report.selection);
    aros_common::outputln!("  removal: {}", report.removal.outcome);
    aros_common::outputln!("  recoverability: {}", report.recoverability);
    aros_common::outputln!("  offline impact: {}", report.offline_impact);
    aros_common::outputln!("  boundary: {}", report.boundary);
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
