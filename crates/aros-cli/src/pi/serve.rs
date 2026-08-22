//! Fail-closed service planning for the local Pi lab.
//!
//! The plan is deliberately resolved before any DHCP or TFTP socket is
//! opened.  In USB-ECM mode it is rooted in a full USB descriptor identity,
//! not a transient `enN`/`enx…` name; in native-RJ45 mode the interface name
//! is an explicit part of the local board profile.  Both paths then prove
//! that the configured concrete IPv4 address is currently assigned to that
//! exact interface.

use super::config::{parse_unicast_mac, Board, Transport};
use super::dhcp::{self, DhcpConfig};
use super::tftp;
use super::UsbEcmAdapter;
use if_addrs::{get_if_addrs, IfAddr};
use miette::Result;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use tftp_rs::server::ServerEvent;
use tokio::sync::{mpsc, watch};

const DEFAULT_SUBNET_MASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const DHCP_LEASE_SECONDS: u32 = 300;
const TFTP_PORT: u16 = 69;

/// All concrete, local information required before a foreground lab service
/// may bind any socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePlan {
    pub board_name: String,
    pub transport: Transport,
    pub interface: String,
    pub server_address: Ipv4Addr,
    pub target_address: Ipv4Addr,
    pub subnet_mask: Ipv4Addr,
    pub expected_target_mac: [u8; 6],
    /// The one board-specific, atomically published deployment that TFTP is
    /// allowed to expose.  The parent TFTP root is intentionally never used.
    pub tftp_root: PathBuf,
}

/// Resolve a service plan from a board and the already-discovered USB
/// adapters.  This is kept pure so it can be tested without a live network
/// device or privileged ports.
pub(super) fn resolve_from_adapters(
    board: &Board,
    adapters: &[UsbEcmAdapter],
) -> Result<ServicePlan> {
    match board.config.transport {
        Transport::UbootUsbEcm => resolve_usb_ecm(board, adapters),
        Transport::NativeTftp => resolve_native(board),
    }
}

/// Resolve a plan against the real host state.  This is read-only; service
/// startup remains a separate operation.
pub fn resolve(board: &Board) -> Result<ServicePlan> {
    let adapters = super::scan::adapters()?;
    resolve_from_adapters(board, &adapters)
}

/// Resolve and, unless `dry_run` is requested, run a board's narrow DHCP and
/// TFTP service until Ctrl-C.  The caller never supplies a bind address: it
/// comes solely from the board profile after identity verification.
pub async fn run(board: &Board, dry_run: bool) -> Result<()> {
    let plan = resolve(board)?;
    print_plan(&plan);
    if dry_run {
        println!("  Dry run: no DHCP or TFTP socket was opened.");
        return Ok(());
    }

    let dhcp_config = DhcpConfig {
        server_address: plan.server_address,
        target_address: plan.target_address,
        expected_client_mac: plan.expected_target_mac,
        subnet_mask: plan.subnet_mask,
        lease_seconds: DHCP_LEASE_SECONDS,
    };
    dhcp_config
        .validate()
        .map_err(|error| miette::miette!("Invalid restricted DHCP configuration: {error}"))?;

    let tftp_bind = SocketAddr::V4(SocketAddrV4::new(plan.server_address, TFTP_PORT));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let event_task = tokio::spawn(print_tftp_events(event_rx));

    println!("\n▶ Starting restricted Pi lab service. Press Ctrl-C to stop it.");
    println!(
        "  DHCP: {}:67 → {} ({})",
        plan.server_address,
        plan.target_address,
        format_mac(plan.expected_target_mac)
    );
    println!("  TFTP: {} (root {})", tftp_bind, plan.tftp_root.display());

    let mut tftp_task = Box::pin(tftp::serve_read_only(
        tftp_bind,
        &plan.interface,
        plan.tftp_root.clone(),
        event_tx,
        shutdown_rx.clone(),
    ));
    let mut dhcp_task = Box::pin(dhcp::serve_on_named_interface(
        dhcp_config,
        &plan.interface,
        shutdown_rx,
    ));

    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => match signal {
            Ok(()) => {
                println!("\nStopping Pi lab service…");
                Ok(())
            }
            Err(error) => Err(miette::miette!("Could not wait for Ctrl-C: {error}")),
        },
        result = &mut tftp_task => result,
        result = &mut dhcp_task => result.map_err(|error| miette::miette!("DHCP service failed: {error}")),
    };

    // The selected future may already have completed, so do not poll it a
    // second time during cleanup.  Signal the still-running peer, then drop
    // both exact-address futures as this foreground command returns.
    let _ = shutdown_tx.send(true);
    drop(tftp_task);
    drop(dhcp_task);
    event_task.abort();

    result
}

fn print_plan(plan: &ServicePlan) {
    println!("🧭 AROS Pi lab service plan");
    println!("  • Board:     {} ({})", plan.board_name, plan.transport);
    println!("  • Interface: {}", plan.interface);
    println!(
        "  • Address:   {} / {}",
        plan.server_address, plan.subnet_mask
    );
    println!(
        "  • Pi lease:  {} for {}",
        plan.target_address,
        format_mac(plan.expected_target_mac)
    );
    println!("  • TFTP root: {}", plan.tftp_root.display());
}

async fn print_tftp_events(mut events: mpsc::UnboundedReceiver<ServerEvent>) {
    while let Some(event) = events.recv().await {
        match event {
            ServerEvent::Log(message) => println!("  TFTP: {message}"),
            ServerEvent::TransferStarted(transfer) => println!(
                "  TFTP: {:?} '{}' for {}",
                transfer.kind, transfer.filename, transfer.peer
            ),
            ServerEvent::TransferProgress {
                id,
                transferred,
                total_bytes,
            } => println!("  TFTP: transfer {id}: {transferred}/{total_bytes} bytes"),
            ServerEvent::TransferComplete(id) => println!("  TFTP: transfer {id} complete"),
            ServerEvent::TransferFailed { id, error } => {
                println!("  TFTP: transfer {id} failed: {error}");
            }
        }
    }
}

fn format_mac(mac: [u8; 6]) -> String {
    mac.iter()
        .map(|octet| format!("{octet:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn resolve_usb_ecm(board: &Board, adapters: &[UsbEcmAdapter]) -> Result<ServicePlan> {
    let usb_ecm = board.config.usb_ecm.as_ref().ok_or_else(|| {
        miette::miette!(
            "Board '{}' uses uboot-usb-ecm but has no [boards.{}.usb_ecm] section.",
            board.name,
            board.name
        )
    })?;
    let identity = usb_ecm.identity.as_ref().ok_or_else(|| {
        miette::miette!(
            "Board '{}' has no usb_ecm.identity. `aros pi serve` requires vendor_id, product_id, serial and expected_target_mac before it can bind a USB adapter.",
            board.name
        )
    })?;

    let matches = adapters
        .iter()
        .filter(|adapter| {
            adapter.cdc_ecm
                && adapter.vendor_id == identity.vendor_id
                && adapter.product_id == identity.product_id
                && adapter.serial.as_deref() == Some(identity.serial.as_str())
        })
        .collect::<Vec<_>>();
    let adapter = match matches.as_slice() {
        [adapter] => *adapter,
        [] => {
            miette::bail!(
                "No USB CDC-ECM adapter matches board '{}' (USB {:04x}:{:04x}, serial '{}'). Run `aros pi scan`; no service was started.",
                board.name,
                identity.vendor_id,
                identity.product_id,
                identity.serial
            );
        }
        _ => {
            miette::bail!(
                "Multiple USB CDC-ECM adapters match board '{}' (USB {:04x}:{:04x}, serial '{}'). Disconnect duplicates or make the USB serial unique; no service was started.",
                board.name,
                identity.vendor_id,
                identity.product_id,
                identity.serial
            );
        }
    };

    let server_address = concrete_ipv4(usb_ecm.host_address, "usb_ecm.host_address")?;
    let target_address = concrete_ipv4(usb_ecm.target_address, "usb_ecm.target_address")?;
    ensure_address_on_adapter(server_address, adapter, "usb_ecm.host_address")?;
    let subnet_mask = validated_subnet_mask(usb_ecm.subnet_mask)?;
    ensure_same_subnet(server_address, target_address, subnet_mask)?;

    Ok(ServicePlan {
        board_name: board.name.clone(),
        transport: Transport::UbootUsbEcm,
        interface: adapter.interface.clone(),
        server_address,
        target_address,
        subnet_mask,
        expected_target_mac: parse_unicast_mac(&identity.expected_target_mac).ok_or_else(|| {
            miette::miette!(
                "Board '{}' has an invalid usb_ecm.identity.expected_target_mac.",
                board.name
            )
        })?,
        tftp_root: published_deployment_dir(board)?,
    })
}

fn resolve_native(board: &Board) -> Result<ServicePlan> {
    let network = board.config.network.as_ref().ok_or_else(|| {
        miette::miette!(
            "Board '{}' uses native-tftp but has no [boards.{}.network] section.",
            board.name,
            board.name
        )
    })?;
    let interface = network.interface.as_deref().ok_or_else(|| {
        miette::miette!(
            "Board '{}' has no network.interface. Name the intended physical Ethernet interface explicitly; `aros pi serve` never guesses an RJ45 port.",
            board.name
        )
    })?;
    if interface.trim().is_empty() {
        miette::bail!("Board '{}' has an empty network.interface.", board.name);
    }
    let expected_target_mac = network.expected_target_mac.as_deref().ok_or_else(|| {
        miette::miette!(
            "Board '{}' has no network.expected_target_mac. DHCP must be pinned to the Pi Ethernet MAC.",
            board.name
        )
    })?;
    let server_address = concrete_ipv4(network.server_address, "network.server_address")?;
    let target_address = concrete_ipv4(network.target_address, "network.target_address")?;
    let subnet_mask = validated_subnet_mask(network.subnet_mask)?;
    ensure_same_subnet(server_address, target_address, subnet_mask)?;
    ensure_address_on_named_interface(interface, server_address)?;

    Ok(ServicePlan {
        board_name: board.name.clone(),
        transport: Transport::NativeTftp,
        interface: interface.to_string(),
        server_address,
        target_address,
        subnet_mask,
        expected_target_mac: parse_unicast_mac(expected_target_mac).ok_or_else(|| {
            miette::miette!(
                "Board '{}' has an invalid network.expected_target_mac.",
                board.name
            )
        })?,
        tftp_root: published_deployment_dir(board)?,
    })
}

fn concrete_ipv4(address: IpAddr, field: &str) -> Result<Ipv4Addr> {
    let address = match address {
        IpAddr::V4(address) => address,
        IpAddr::V6(_) => miette::bail!("{field} must be a concrete IPv4 address for DHCP/TFTP."),
    };
    if address.is_unspecified()
        || address.is_broadcast()
        || address.is_multicast()
        || address.is_loopback()
        || address.is_documentation()
    {
        miette::bail!(
            "{field} '{}' is not a usable concrete Pi-lab IPv4 address.",
            address
        );
    }
    Ok(address)
}

fn validated_subnet_mask(mask: Option<Ipv4Addr>) -> Result<Ipv4Addr> {
    let mask = mask.unwrap_or(DEFAULT_SUBNET_MASK);
    let numeric = u32::from(mask);
    // A valid mask is one contiguous run of ones followed by zeros. /0, /31
    // and /32 cannot describe this directed-broadcast DHCP link.
    let prefix_length = numeric.leading_ones();
    if !(1..=30).contains(&prefix_length) || (numeric | (numeric - 1)) != u32::MAX {
        miette::bail!(
            "subnet_mask '{}' must be a contiguous mask between /1 and /30.",
            mask
        );
    }
    Ok(mask)
}

fn ensure_same_subnet(
    server_address: Ipv4Addr,
    target_address: Ipv4Addr,
    subnet_mask: Ipv4Addr,
) -> Result<()> {
    let mask = u32::from(subnet_mask);
    if u32::from(server_address) & mask != u32::from(target_address) & mask {
        miette::bail!(
            "server address '{}' and target address '{}' are not in subnet '{}'.",
            server_address,
            target_address,
            subnet_mask
        );
    }
    Ok(())
}

fn ensure_address_on_adapter(
    address: Ipv4Addr,
    adapter: &UsbEcmAdapter,
    field: &str,
) -> Result<()> {
    if !adapter.ipv4_addresses.contains(&address) {
        miette::bail!(
            "{field} '{}' is not assigned to selected USB CDC-ECM interface '{}' (current IPv4: {}). Configure that specific interface first; no service was started.",
            address,
            adapter.interface,
            displayed_addresses(&adapter.ipv4_addresses)
        );
    }
    Ok(())
}

fn ensure_address_on_named_interface(interface_name: &str, address: Ipv4Addr) -> Result<()> {
    let interfaces = get_if_addrs().map_err(|error| {
        miette::miette!("Could not enumerate local interface addresses: {error}")
    })?;
    let found = interfaces.iter().any(|interface| {
        interface.name == interface_name
            && match &interface.addr {
                IfAddr::V4(candidate) => candidate.ip == address,
                IfAddr::V6(_) => false,
            }
    });
    if !found {
        miette::bail!(
            "network.server_address '{}' is not assigned to explicitly configured interface '{}'; no service was started.",
            address,
            interface_name
        );
    }
    Ok(())
}

fn published_deployment_dir(board: &Board) -> Result<PathBuf> {
    let deployment = board.deployment_dir()?;
    let metadata = std::fs::symlink_metadata(&deployment).map_err(|error| {
        miette::miette!(
            "Board '{}' has no published deployment at '{}': {error}. Run `aros pi deploy --board {} --apply` first.",
            board.name,
            deployment.display(),
            board.name
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        miette::bail!(
            "Published deployment '{}' must be a real directory, not a symlink or file.",
            deployment.display()
        );
    }
    let marker = deployment.join(".aros-pi-deploy");
    let marker_contents = std::fs::read_to_string(&marker).map_err(|error| {
        miette::miette!(
            "Published deployment '{}' is not AROS-managed (missing '{}': {error}). Run `aros pi deploy --apply` first.",
            deployment.display(),
            marker.display()
        )
    })?;
    if marker_contents != "AROS PI deployment directory\n" {
        miette::bail!(
            "Published deployment '{}' has an invalid AROS deployment marker.",
            deployment.display()
        );
    }
    deployment.canonicalize().map_err(|error| {
        miette::miette!(
            "Could not resolve published deployment '{}': {error}",
            deployment.display()
        )
    })
}

fn displayed_addresses(addresses: &[Ipv4Addr]) -> String {
    if addresses.is_empty() {
        "none".to_string()
    } else {
        addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_from_adapters, validated_subnet_mask};
    use crate::pi::config::{Board, BoardConfig, Transport, UsbEcmConfig, UsbEcmIdentity};
    use crate::pi::UsbEcmAdapter;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    #[test]
    fn rejects_non_contiguous_masks() {
        assert!(validated_subnet_mask(Some(Ipv4Addr::new(255, 0, 255, 0))).is_err());
        assert_eq!(
            validated_subnet_mask(Some(Ipv4Addr::new(255, 255, 255, 0))).expect("mask"),
            Ipv4Addr::new(255, 255, 255, 0)
        );
    }

    #[test]
    fn usb_service_plan_requires_exact_descriptor_match() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let deployment = temporary.path().join("rpi4");
        std::fs::create_dir_all(&deployment).expect("deployment");
        std::fs::write(
            deployment.join(".aros-pi-deploy"),
            "AROS PI deployment directory\n",
        )
        .expect("marker");
        let board = usb_board(temporary.path());
        let adapters = vec![adapter("en7", "aros-rpi4-lab-01")];

        let plan = resolve_from_adapters(&board, &adapters).expect("service plan");
        assert_eq!(plan.interface, "en7");
        assert_eq!(plan.server_address, Ipv4Addr::new(192, 168, 50, 1));
        assert_eq!(plan.expected_target_mac, [0x02, 0xaa, 0, 0, 0, 1]);

        let wrong_serial = vec![adapter("en7", "another-board")];
        assert!(resolve_from_adapters(&board, &wrong_serial).is_err());
    }

    #[test]
    fn usb_service_plan_rejects_address_from_another_interface() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let deployment = temporary.path().join("rpi4");
        std::fs::create_dir_all(&deployment).expect("deployment");
        std::fs::write(
            deployment.join(".aros-pi-deploy"),
            "AROS PI deployment directory\n",
        )
        .expect("marker");
        let board = usb_board(temporary.path());
        let mut adapter = adapter("en7", "aros-rpi4-lab-01");
        adapter.ipv4_addresses.clear();

        assert!(resolve_from_adapters(&board, &[adapter]).is_err());
    }

    fn usb_board(tftp_root: &std::path::Path) -> Board {
        Board {
            name: "rpi4".to_string(),
            config: BoardConfig {
                model: "rpi4".to_string(),
                preset: "rpi4-aarch64-debug".to_string(),
                build_target: "rpi-artifacts".to_string(),
                transport: Transport::UbootUsbEcm,
                artifact_dir: None,
                dtb_path: None,
                core_kobj_dir: None,
                tftp_root: Some(tftp_root.to_path_buf()),
                tftp_prefix: None,
                serial_device: None,
                serial_baud: 115_200,
                debug_transport: None,
                power_control: None,
                network: None,
                usb_ecm: Some(UsbEcmConfig {
                    host_address: IpAddr::V4(Ipv4Addr::new(192, 168, 50, 1)),
                    target_address: IpAddr::V4(Ipv4Addr::new(192, 168, 50, 2)),
                    subnet_mask: None,
                    identity: Some(UsbEcmIdentity {
                        vendor_id: 0x1234,
                        product_id: 0x5678,
                        serial: "aros-rpi4-lab-01".to_string(),
                        expected_target_mac: "02:aa:00:00:00:01".to_string(),
                    }),
                    mac_interface_hint: None,
                }),
            },
            config_path: PathBuf::from("boards.toml"),
        }
    }

    fn adapter(interface: &str, serial: &str) -> UsbEcmAdapter {
        UsbEcmAdapter {
            interface: interface.to_string(),
            vendor_id: 0x1234,
            product_id: 0x5678,
            serial: Some(serial.to_string()),
            manufacturer: None,
            product: None,
            interface_mac: None,
            ipv4_addresses: vec![Ipv4Addr::new(192, 168, 50, 1)],
            cdc_ecm: true,
        }
    }
}
