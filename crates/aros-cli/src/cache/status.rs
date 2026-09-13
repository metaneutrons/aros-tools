//! Passive cache-status rendering shared by the cache command families.

use aros_cache::{cache_status, CacheFamilyStatus, CacheStatus};
use miette::Result;

use crate::toolchain_management::ResultFormat;

/// Render the top-level passive cache overview.
pub fn render_status(format: ResultFormat) -> Result<()> {
    let report = cache_status().map_err(|error| miette::miette!(error))?;
    match format {
        ResultFormat::Human => print_status_human(&report),
        ResultFormat::Json => super::print_json(&report, "cache status")?,
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

pub(super) fn print_backend_details(family: &CacheFamilyStatus) {
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
