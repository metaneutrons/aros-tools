//! Rendering adapter for the bounded cache-status commands.

use std::path::Path;

use crate::toolchain_management::ResultFormat;
use crate::CacheSourceSelector;
use aros_cache::{
    cache_status, compiler_cache_status, CacheFamilyStatus, CacheStatus, CompilerBackendChoice,
    CompilerCacheStatus,
};
use aros_toolchain::{
    source_cache::{
        fetch_request, list as list_source_cache, status as source_cache_status, verify_request,
        SourceCacheFetch, SourceCacheList, SourceCacheStatus, SourceCacheVerification,
    },
    source_cache_request::{read_selector, SourceCacheRequest},
    ContractError,
};
use miette::Result;

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

fn print_json<T: serde::Serialize>(value: &T, operation: &str) -> Result<()> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| miette::miette!("could not serialize {operation}: {error}"))?;
    aros_common::outputln!("{document}");
    Ok(())
}
