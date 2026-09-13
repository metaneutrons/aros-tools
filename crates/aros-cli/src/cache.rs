//! Rendering adapter for the bounded cache-status commands.

use crate::toolchain_management::ResultFormat;
use aros_cache::{
    cache_status, compiler_cache_status, CacheFamilyStatus, CacheStatus, CompilerBackendChoice,
    CompilerCacheStatus,
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

fn print_json<T: serde::Serialize>(value: &T, operation: &str) -> Result<()> {
    let document = serde_json::to_string_pretty(value)
        .map_err(|error| miette::miette!("could not serialize {operation}: {error}"))?;
    aros_common::outputln!("{document}");
    Ok(())
}
