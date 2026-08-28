//! Physical-board profiles and fail-closed local lab workflows for AROS-NG.
//!
//! Build targets remain in the checked-in `aros-targets.toml`. This crate owns
//! the separate local identity of concrete boards and the hardware-facing
//! operations behind the `aros board` frontend.

use aros_common::{bounded_output_detail, DiagnosticContext, LogLevel};
use miette::Result;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub mod config;
pub mod deploy;
pub mod dhcp;
mod disk_inventory;
pub mod scan;
#[cfg(target_os = "linux")]
mod scan_linux;
#[cfg(target_os = "macos")]
mod scan_macos;
pub mod sd;
pub mod sd_disk;
mod sd_manifest;
pub mod sd_unmount;
mod sd_write_plan;
pub mod serve;
pub mod tftp;

/// A USB CDC-ECM network function discovered on the local host.
///
/// The current interface name is diagnostic state, not stable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbEcmAdapter {
    pub interface: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub interface_mac: Option<String>,
    pub ipv4_addresses: Vec<Ipv4Addr>,
    pub cdc_ecm: bool,
}

/// Component-neutral sink for board-engine events.
pub trait EventSink: Send + Sync {
    /// Record one structured event.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected sink cannot persist the event.
    fn event(
        &self,
        level: LogLevel,
        event: &str,
        message: &str,
        context: &DiagnosticContext,
    ) -> Result<()>;
}

/// Event sink used by library callers that intentionally disable local logs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn event(
        &self,
        _level: LogLevel,
        _event: &str,
        _message: &str,
        _context: &DiagnosticContext,
    ) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn validate_profile_name(value: &str, label: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !valid {
        miette::bail!(
            "Invalid {label} '{value}'. Names may contain only ASCII letters, digits, '-' and '_'."
        );
    }
    Ok(())
}

pub(crate) fn sha256_file_with_size(path: &Path) -> Result<(String, u64)> {
    aros_common::sha256_file(path)
        .map(|result| (result.digest.to_string(), result.size))
        .map_err(|error| miette::miette!("failed to hash '{}': {error}", path.display()))
}

/// Require an existing directory and return its canonical absolute path.
///
/// Board workflows use this single boundary before resolving caller-supplied
/// children, preventing path-policy drift between deployment and SD artifacts.
pub(crate) fn canonical_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        miette::miette!("Could not access {label} '{}': {error}", path.display())
    })?;
    if !metadata.is_dir() {
        miette::bail!("{label} '{}' is not a directory.", path.display());
    }
    path.canonicalize()
        .map_err(|error| miette::miette!("Could not resolve {label} '{}': {error}", path.display()))
}

pub(crate) fn run_output(command: &mut Command, description: &str) -> Result<Output> {
    let observed = aros_common::run_output(command)
        .map_err(|error| miette::miette!("could not start {description}: {error}"))?;
    if observed.output.status.success() {
        return Ok(observed.output);
    }
    let detail = bounded_output_detail(&observed.output.stdout, &observed.output.stderr, 64 * 1024);
    miette::bail!(
        "{description} failed with {}{}",
        observed.output.status,
        if detail.is_empty() {
            String::new()
        } else {
            format!(":\n{detail}")
        }
    )
}
