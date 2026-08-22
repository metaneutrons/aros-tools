//! Read-only TFTP adapter for the opt-in Pi lab service.
//!
//! This module deliberately accepts a concrete bind address and an already
//! resolved interface from the caller. It never discovers an interface,
//! chooses a wildcard address, or starts as part of deployment.

use miette::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use tftp_rs::server::{self, ServerConfig, ServerEvent};
use tokio::sync::{mpsc, watch};

/// Start a read-only TFTP service for an already-built, local boot bundle.
pub async fn serve_read_only(
    bind_addr: SocketAddr,
    interface_name: &str,
    artifact_dir: PathBuf,
    events: mpsc::UnboundedSender<ServerEvent>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if !artifact_dir.is_dir() {
        miette::bail!(
            "TFTP artifact directory '{}' is not an existing directory.",
            artifact_dir.display()
        );
    }
    server::validate_bind_addr(bind_addr).map_err(|error| miette::miette!("{error}"))?;
    server::run_on_interface(
        bind_addr,
        interface_name,
        artifact_dir,
        events,
        shutdown,
        read_only_config(),
    )
    .await
    .map_err(|error| miette::miette!("TFTP service failed: {error}"))
}

#[must_use]
pub fn read_only_config() -> ServerConfig {
    ServerConfig {
        allow_overwrite: false,
        enable_read: true,
        enable_write: false,
        ..ServerConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::read_only_config;

    #[test]
    fn aros_tftp_adapter_is_read_only() {
        let config = read_only_config();

        assert!(config.enable_read);
        assert!(!config.enable_write);
        assert!(!config.allow_overwrite);
    }
}
