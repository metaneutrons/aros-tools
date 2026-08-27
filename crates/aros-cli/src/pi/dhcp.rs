//! A deliberately narrow DHCPv4 service for an isolated AROS Pi lab link.
//!
//! The caller must resolve and validate the selected network interface before
//! constructing [`DhcpConfig`]. This module never binds a wildcard address.
//! Before it opens a socket it proves that the configured IPv4 address still
//! belongs to the selected interface, then asks the OS to bind the socket to
//! that interface too. It only answers DHCP DISCOVER and REQUEST packets from
//! the one configured Ethernet MAC address.
//!
//! A general-purpose DHCP server is deliberately not embedded here: the Pi
//! lab has one client, one lease and one exact host address.  Keeping that
//! contract in this small module prevents a default pool, lease database or
//! wildcard listener from accidentally escaping the selected USB/RJ45 link.

use anyhow::{bail, Context, Result};
use aros_common::{DiagnosticContext, LogLevel};
use if_addrs::{get_if_addrs, IfAddr};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::num::NonZeroU32;
use tokio::net::UdpSocket;
use tokio::sync::watch;

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const BOOTP_HEADER_LEN: usize = 236;
const DHCP_FIXED_LEN: usize = BOOTP_HEADER_LEN + 4;
const MAX_DHCP_PACKET_LEN: usize = 4096;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const OPTION_SUBNET_MASK: u8 = 1;
const OPTION_REQUESTED_IP: u8 = 50;
const OPTION_LEASE_TIME: u8 = 51;
const OPTION_MESSAGE_TYPE: u8 = 53;
const OPTION_SERVER_IDENTIFIER: u8 = 54;
const OPTION_PAD: u8 = 0;
const OPTION_END: u8 = 255;

/// A currently resolved, concrete local interface for the Pi lab service.
///
/// The name is required by Linux's `SO_BINDTODEVICE`; the index is required by
/// macOS's IPv4 `IP_BOUND_IF` socket option. Keeping both prevents a caller
/// from accidentally reducing an interface-bound DHCP socket to an
/// address-bound one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpInterface {
    name: String,
    index: NonZeroU32,
}

impl DhcpInterface {
    /// Resolve a local interface name to its current non-zero OS index.
    ///
    /// This is deliberately a live lookup: interface indices can change when
    /// a USB CDC-ECM device is unplugged and reattached.
    pub fn resolve(name: impl AsRef<str>) -> Result<Self> {
        let name = name.as_ref();
        validate_interface_name(name)?;
        let interfaces = local_interfaces()?;
        let index = index_for_name(&interfaces, name)?;
        Self::new(name, index)
    }

    /// Construct a checked interface identity from a caller-provided name and
    /// index. [`serve_on_interface`] revalidates it against current host state
    /// immediately before opening the socket.
    pub fn new(name: impl Into<String>, index: NonZeroU32) -> Result<Self> {
        let name = name.into();
        validate_interface_name(&name)?;
        Ok(Self { name, index })
    }

    /// Current OS interface name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Current OS interface index.
    #[must_use]
    pub const fn index(&self) -> NonZeroU32 {
        self.index
    }
}

/// Static, fail-closed configuration for the Pi lab DHCP service.
///
/// `server_address` is both the exact local bind address and the server
/// identifier advertised to the Pi. `target_address` is the sole lease this
/// server can offer. The caller must have verified that `server_address`
/// belongs to the currently selected USB-ECM interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpConfig {
    /// Concrete IPv4 address of the selected host interface.
    pub server_address: Ipv4Addr,
    /// The one IPv4 lease reserved for the selected Pi.
    pub target_address: Ipv4Addr,
    /// Pi-side USB gadget MAC address allowed to receive a response.
    pub expected_client_mac: [u8; 6],
    /// IPv4 subnet mask for the isolated lab link.
    pub subnet_mask: Ipv4Addr,
    /// Lease lifetime advertised in OFFER and ACK packets.
    pub lease_seconds: u32,
}

impl DhcpConfig {
    /// Validate that this configuration is narrow enough for an isolated link.
    pub fn validate(&self) -> Result<()> {
        validate_concrete_unicast(self.server_address, "DHCP server address")?;
        validate_concrete_unicast(self.target_address, "DHCP target address")?;

        if self.server_address == self.target_address {
            bail!("DHCP server and target addresses must be different");
        }
        if self.expected_client_mac == [0; 6] {
            bail!("DHCP client MAC must not be all zeroes");
        }
        if self.expected_client_mac[0] & 1 != 0 {
            bail!("DHCP client MAC must be a unicast Ethernet address");
        }
        if self.lease_seconds == 0 {
            bail!("DHCP lease duration must be greater than zero");
        }

        let subnet_mask = ipv4_to_u32(self.subnet_mask);
        let prefix_len = subnet_mask.leading_ones();
        let canonical_mask = if prefix_len == 0 {
            0
        } else {
            u32::MAX << (32 - prefix_len)
        };
        if subnet_mask != canonical_mask || !(1..=30).contains(&prefix_len) {
            bail!(
                "DHCP subnet mask '{}' must be a contiguous /1 through /30 mask",
                self.subnet_mask
            );
        }

        let server = ipv4_to_u32(self.server_address);
        let target = ipv4_to_u32(self.target_address);
        let network = server & subnet_mask;
        let broadcast = network | !subnet_mask;
        if target & subnet_mask != network {
            bail!(
                "DHCP target address '{}' is not in the server subnet",
                self.target_address
            );
        }
        if server == network || server == broadcast {
            bail!(
                "DHCP server address '{}' is not a usable host address",
                self.server_address
            );
        }
        if target == network || target == broadcast {
            bail!(
                "DHCP target address '{}' is not a usable host address",
                self.target_address
            );
        }

        Ok(())
    }

    fn broadcast_destination(&self) -> SocketAddrV4 {
        let server = ipv4_to_u32(self.server_address);
        let subnet_mask = ipv4_to_u32(self.subnet_mask);
        let broadcast = (server & subnet_mask) | !subnet_mask;
        SocketAddrV4::new(Ipv4Addr::from(broadcast.to_be_bytes()), DHCP_CLIENT_PORT)
    }
}

/// Resolve `interface_name` again, then serve DHCPv4 strictly on it.
///
/// This is the intended entry point for the Pi service planner: it accepts
/// the selected interface by name, derives its current index, confirms that
/// the configured server address belongs to it, and never chooses another
/// interface as a fallback.
pub async fn serve_on_named_interface(
    config: DhcpConfig,
    interface_name: impl AsRef<str>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let interface = DhcpInterface::resolve(interface_name)?;
    serve_on_interface(config, interface, shutdown).await
}

/// Serve DHCPv4 strictly on an already resolved interface identity.
///
/// The identity is checked against the current interface table and the
/// configured address immediately before the socket is created. This prevents
/// a stale USB-ECM interface index from silently targeting a new interface.
pub async fn serve_on_interface(
    config: DhcpConfig,
    interface: DhcpInterface,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    config.validate()?;
    serve_with_interface(config, interface, &mut shutdown).await
}

async fn serve_with_interface(
    config: DhcpConfig,
    interface: DhcpInterface,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    validate_current_interface(&interface, config.server_address)?;

    let bind_address = SocketAddrV4::new(config.server_address, DHCP_SERVER_PORT);
    let socket = bind_interface_socket(bind_address, &interface)?;
    socket
        .set_broadcast(true)
        .context("could not enable directed DHCP broadcasts")?;

    crate::observability::log_event(
        LogLevel::Info,
        "pi.dhcp.started",
        "started restricted Pi lab DHCPv4 service",
        &DiagnosticContext {
            tool: Some("dhcpv4".into()),
            mode: Some(interface.name().into()),
            target: Some(config.target_address.to_string()),
            output: Some(bind_address.to_string()),
            ..DiagnosticContext::default()
        },
    )?;

    if *shutdown.borrow() {
        return Ok(());
    }

    let destination = config.broadcast_destination();
    let mut receive_buffer = [0_u8; MAX_DHCP_PACKET_LEN];

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            received = socket.recv_from(&mut receive_buffer) => {
                let (received_len, peer) = received.context("DHCP receive failed")?;
                if peer.port() != DHCP_CLIENT_PORT {
                    continue;
                }

                let Some(request) = DhcpRequest::parse(&receive_buffer[..received_len]) else {
                    continue;
                };
                let Some(response) = response_for(&request, &config) else {
                    continue;
                };

                if let Err(error) = socket.send_to(&response, destination).await {
                    crate::observability::log_event(
                        LogLevel::Warn,
                        "pi.dhcp.response_failed",
                        &format!("could not send restricted DHCP response: {error}"),
                        &DiagnosticContext {
                            tool: Some("dhcpv4".into()),
                            mode: Some(interface.name().into()),
                            target: Some(destination.to_string()),
                            output: Some(bind_address.to_string()),
                            ..DiagnosticContext::default()
                        },
                    )?;
                }
            }
        }
    }
}

/// A local interface record kept deliberately small so the selection logic is
/// pure and unit-testable without opening a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalInterface {
    name: String,
    index: Option<u32>,
    ipv4_address: Option<Ipv4Addr>,
}

fn local_interfaces() -> Result<Vec<LocalInterface>> {
    get_if_addrs()
        .context("could not enumerate local interfaces for DHCP binding")
        .map(|interfaces| {
            interfaces
                .into_iter()
                .map(|interface| LocalInterface {
                    name: interface.name,
                    index: interface.index,
                    ipv4_address: match interface.addr {
                        IfAddr::V4(address) => Some(address.ip),
                        IfAddr::V6(_) => None,
                    },
                })
                .collect()
        })
}

fn validate_interface_name(name: &str) -> Result<()> {
    if name.is_empty() || name.as_bytes().contains(&0) {
        bail!("DHCP interface name must be non-empty and must not contain NUL bytes");
    }
    Ok(())
}

fn index_for_name(interfaces: &[LocalInterface], name: &str) -> Result<NonZeroU32> {
    let mut indexes = BTreeSet::new();
    for interface in interfaces.iter().filter(|interface| interface.name == name) {
        let index = interface.index.and_then(NonZeroU32::new).ok_or_else(|| {
            anyhow::anyhow!(
                "local interface '{name}' has no usable OS interface index; DHCP will not bind without one"
            )
        })?;
        indexes.insert(index);
    }

    match indexes.len() {
        0 => bail!("selected DHCP interface '{name}' is not present"),
        1 => Ok(*indexes.first().expect("one interface index")),
        _ => bail!(
            "selected DHCP interface '{name}' resolves to multiple OS interface indices; DHCP will not choose one"
        ),
    }
}

fn validate_current_interface(interface: &DhcpInterface, address: Ipv4Addr) -> Result<()> {
    let interfaces = local_interfaces()?;
    validate_interface_in_records(&interfaces, interface, address)
}

fn validate_interface_in_records(
    interfaces: &[LocalInterface],
    interface: &DhcpInterface,
    address: Ipv4Addr,
) -> Result<()> {
    let current_index = index_for_name(interfaces, interface.name())?;
    if current_index != interface.index() {
        bail!(
            "DHCP interface '{}' changed from index {} to {}; resolve the selected interface again before serving",
            interface.name(),
            interface.index(),
            current_index
        );
    }

    let mut owners = BTreeSet::new();
    for observed in interfaces
        .iter()
        .filter(|observed| observed.ipv4_address == Some(address))
    {
        let index = observed.index.and_then(NonZeroU32::new).ok_or_else(|| {
            anyhow::anyhow!(
                "local interface '{}' owns DHCP address '{}' but has no usable OS interface index",
                observed.name,
                address
            )
        })?;
        owners.insert((observed.name.as_str(), index));
    }

    let selected = (interface.name(), interface.index());
    if !owners.contains(&selected) {
        bail!(
            "DHCP server address '{}' is not currently assigned to selected interface '{}' (index {})",
            address,
            interface.name(),
            interface.index()
        );
    }
    if owners.len() != 1 {
        bail!(
            "DHCP server address '{address}' is also assigned to another local interface; refusing an ambiguous DHCP bind"
        );
    }

    Ok(())
}

fn bind_interface_socket(
    bind_address: SocketAddrV4,
    interface: &DhcpInterface,
) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .with_context(|| format!("could not create restricted DHCP socket for {bind_address}"))?;
    apply_interface_binding(&socket, interface)?;
    socket
        .bind(&SocketAddr::V4(bind_address).into())
        .with_context(|| {
            format!(
                "could not bind restricted DHCP socket at {bind_address} on interface '{}'",
                interface.name()
            )
        })?;
    socket
        .set_nonblocking(true)
        .context("could not make restricted DHCP socket nonblocking")?;
    let socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(socket).context("could not register restricted DHCP socket with Tokio")
}

#[cfg(target_os = "linux")]
fn apply_interface_binding(socket: &Socket, interface: &DhcpInterface) -> Result<()> {
    socket
        .bind_device(Some(interface.name().as_bytes()))
        .with_context(|| {
            format!(
                "could not apply Linux SO_BINDTODEVICE for DHCP interface '{}'",
                interface.name()
            )
        })
}

#[cfg(target_os = "macos")]
fn apply_interface_binding(socket: &Socket, interface: &DhcpInterface) -> Result<()> {
    socket
        .bind_device_by_index_v4(Some(interface.index()))
        .with_context(|| {
            format!(
                "could not apply macOS IP_BOUND_IF for DHCP interface '{}' (index {})",
                interface.name(),
                interface.index()
            )
        })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn apply_interface_binding(_socket: &Socket, interface: &DhcpInterface) -> Result<()> {
    bail!(
        "restricted DHCP interface binding is supported only on macOS and Linux; refusing to serve on '{}'",
        interface.name()
    )
}

fn validate_concrete_unicast(address: Ipv4Addr, label: &str) -> Result<()> {
    if address.is_unspecified() || address.is_multicast() || address.octets() == [255; 4] {
        bail!("{label} '{address}' must be a concrete unicast IPv4 address");
    }
    Ok(())
}

const fn ipv4_to_u32(address: Ipv4Addr) -> u32 {
    u32::from_be_bytes(address.octets())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Ack = 5,
}

impl DhcpMessageType {
    const fn from_option(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            5 => Some(Self::Ack),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DhcpRequest {
    transaction_id: [u8; 4],
    seconds_elapsed: [u8; 2],
    flags: [u8; 2],
    client_address: Ipv4Addr,
    relay_address: Ipv4Addr,
    client_mac: [u8; 6],
    message_type: DhcpMessageType,
    requested_address: Option<Ipv4Addr>,
    server_identifier: Option<Ipv4Addr>,
}

impl DhcpRequest {
    fn parse(packet: &[u8]) -> Option<Self> {
        if packet.len() < DHCP_FIXED_LEN
            || packet[0] != 1
            || packet[1] != 1
            || packet[2] != 6
            || packet[BOOTP_HEADER_LEN..DHCP_FIXED_LEN] != DHCP_MAGIC_COOKIE
        {
            return None;
        }

        let options = parse_options(&packet[DHCP_FIXED_LEN..])?;
        let message_type = options.message_type?;
        let transaction_id = packet[4..8].try_into().ok()?;
        let seconds_elapsed = packet[8..10].try_into().ok()?;
        let flags = packet[10..12].try_into().ok()?;
        let client_mac = packet[28..34].try_into().ok()?;

        Some(Self {
            transaction_id,
            seconds_elapsed,
            flags,
            client_address: ipv4_from_packet(packet, 12),
            relay_address: ipv4_from_packet(packet, 24),
            client_mac,
            message_type,
            requested_address: options.requested_address,
            server_identifier: options.server_identifier,
        })
    }
}

#[derive(Debug, Default)]
struct DhcpOptions {
    message_type: Option<DhcpMessageType>,
    requested_address: Option<Ipv4Addr>,
    server_identifier: Option<Ipv4Addr>,
}

fn parse_options(packet: &[u8]) -> Option<DhcpOptions> {
    let mut options = DhcpOptions::default();
    let mut offset = 0;

    while offset < packet.len() {
        let code = packet[offset];
        offset += 1;
        match code {
            OPTION_PAD => continue,
            OPTION_END => return Some(options),
            _ => {}
        }

        let value_len = usize::from(*packet.get(offset)?);
        offset += 1;
        let value_end = offset.checked_add(value_len)?;
        let value = packet.get(offset..value_end)?;
        offset = value_end;

        match code {
            OPTION_MESSAGE_TYPE => {
                if value.len() != 1 || options.message_type.is_some() {
                    return None;
                }
                options.message_type = Some(DhcpMessageType::from_option(value[0])?);
            }
            OPTION_REQUESTED_IP => {
                if options.requested_address.is_some() {
                    return None;
                }
                options.requested_address = Some(ipv4_from_option(value)?);
            }
            OPTION_SERVER_IDENTIFIER => {
                if options.server_identifier.is_some() {
                    return None;
                }
                options.server_identifier = Some(ipv4_from_option(value)?);
            }
            _ => {}
        }
    }

    None
}

fn ipv4_from_packet(packet: &[u8], offset: usize) -> Ipv4Addr {
    Ipv4Addr::new(
        packet[offset],
        packet[offset + 1],
        packet[offset + 2],
        packet[offset + 3],
    )
}

fn ipv4_from_option(value: &[u8]) -> Option<Ipv4Addr> {
    let bytes: [u8; 4] = value.try_into().ok()?;
    Some(Ipv4Addr::from(bytes))
}

fn response_type_for(request: &DhcpRequest, config: &DhcpConfig) -> Option<DhcpMessageType> {
    if !request.relay_address.is_unspecified() {
        return None;
    }
    if request
        .server_identifier
        .is_some_and(|identifier| identifier != config.server_address)
    {
        return None;
    }

    match request.message_type {
        DhcpMessageType::Discover => Some(DhcpMessageType::Offer),
        DhcpMessageType::Request => {
            let requested_address = request.requested_address.or_else(|| {
                (!request.client_address.is_unspecified()).then_some(request.client_address)
            })?;
            (requested_address == config.target_address).then_some(DhcpMessageType::Ack)
        }
        DhcpMessageType::Offer | DhcpMessageType::Ack => None,
    }
}

fn response_for(request: &DhcpRequest, config: &DhcpConfig) -> Option<Vec<u8>> {
    if request.client_mac != config.expected_client_mac {
        return None;
    }
    let response_type = response_type_for(request, config)?;
    Some(build_response(request, config, response_type))
}

fn build_response(
    request: &DhcpRequest,
    config: &DhcpConfig,
    response_type: DhcpMessageType,
) -> Vec<u8> {
    debug_assert!(matches!(
        response_type,
        DhcpMessageType::Offer | DhcpMessageType::Ack
    ));

    let mut response = vec![0_u8; DHCP_FIXED_LEN];
    response[0] = 2;
    response[1] = 1;
    response[2] = 6;
    response[4..8].copy_from_slice(&request.transaction_id);
    response[8..10].copy_from_slice(&request.seconds_elapsed);
    response[10..12].copy_from_slice(&request.flags);
    response[16..20].copy_from_slice(&config.target_address.octets());
    response[20..24].copy_from_slice(&config.server_address.octets());
    response[28..34].copy_from_slice(&request.client_mac);
    response[BOOTP_HEADER_LEN..DHCP_FIXED_LEN].copy_from_slice(&DHCP_MAGIC_COOKIE);

    response.extend_from_slice(&[OPTION_MESSAGE_TYPE, 1, response_type as u8]);
    response.extend_from_slice(&[OPTION_SERVER_IDENTIFIER, 4]);
    response.extend_from_slice(&config.server_address.octets());
    response.extend_from_slice(&[OPTION_SUBNET_MASK, 4]);
    response.extend_from_slice(&config.subnet_mask.octets());
    response.extend_from_slice(&[OPTION_LEASE_TIME, 4]);
    response.extend_from_slice(&config.lease_seconds.to_be_bytes());
    response.push(OPTION_END);
    response
}

#[cfg(test)]
mod tests {
    use super::{
        build_response, parse_options, response_for, response_type_for,
        validate_interface_in_records, DhcpConfig, DhcpInterface, DhcpMessageType, DhcpRequest,
        LocalInterface, DHCP_FIXED_LEN, DHCP_MAGIC_COOKIE, OPTION_END, OPTION_MESSAGE_TYPE,
        OPTION_REQUESTED_IP, OPTION_SERVER_IDENTIFIER,
    };
    use std::net::Ipv4Addr;
    use std::num::NonZeroU32;

    fn config() -> DhcpConfig {
        DhcpConfig {
            server_address: Ipv4Addr::new(192, 0, 2, 1),
            target_address: Ipv4Addr::new(192, 0, 2, 2),
            expected_client_mac: [0x02, 0xaa, 0, 0, 0, 1],
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
            lease_seconds: 3600,
        }
    }

    fn observed_interface(
        name: &str,
        index: Option<u32>,
        ipv4_address: Option<Ipv4Addr>,
    ) -> LocalInterface {
        LocalInterface {
            name: name.to_string(),
            index,
            ipv4_address,
        }
    }

    fn request_packet(
        message_type: DhcpMessageType,
        client_mac: [u8; 6],
        requested_address: Option<Ipv4Addr>,
        server_identifier: Option<Ipv4Addr>,
        client_address: Ipv4Addr,
    ) -> Vec<u8> {
        let mut packet = vec![0_u8; DHCP_FIXED_LEN];
        packet[0] = 1;
        packet[1] = 1;
        packet[2] = 6;
        packet[4..8].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        packet[8..10].copy_from_slice(&[0, 42]);
        packet[10..12].copy_from_slice(&[0x80, 0]);
        packet[12..16].copy_from_slice(&client_address.octets());
        packet[28..34].copy_from_slice(&client_mac);
        packet[236..DHCP_FIXED_LEN].copy_from_slice(&DHCP_MAGIC_COOKIE);
        packet.extend_from_slice(&[OPTION_MESSAGE_TYPE, 1, message_type as u8]);
        if let Some(requested_address) = requested_address {
            packet.extend_from_slice(&[OPTION_REQUESTED_IP, 4]);
            packet.extend_from_slice(&requested_address.octets());
        }
        if let Some(server_identifier) = server_identifier {
            packet.extend_from_slice(&[OPTION_SERVER_IDENTIFIER, 4]);
            packet.extend_from_slice(&server_identifier.octets());
        }
        packet.push(OPTION_END);
        packet
    }

    #[test]
    fn configuration_rejects_wildcard_multicast_and_broadcast_addresses() {
        let mut wildcard = config();
        wildcard.server_address = Ipv4Addr::UNSPECIFIED;
        assert!(wildcard.validate().is_err());

        let mut multicast = config();
        multicast.target_address = Ipv4Addr::new(239, 1, 2, 3);
        assert!(multicast.validate().is_err());

        let mut broadcast = config();
        broadcast.server_address = Ipv4Addr::BROADCAST;
        assert!(broadcast.validate().is_err());
    }

    #[test]
    fn configuration_rejects_an_unsafe_mac_or_subnet() {
        let mut zero_mac = config();
        zero_mac.expected_client_mac = [0; 6];
        assert!(zero_mac.validate().is_err());

        let mut multicast_mac = config();
        multicast_mac.expected_client_mac[0] = 1;
        assert!(multicast_mac.validate().is_err());

        let mut non_contiguous_mask = config();
        non_contiguous_mask.subnet_mask = Ipv4Addr::new(255, 0, 255, 0);
        assert!(non_contiguous_mask.validate().is_err());

        let mut different_subnet = config();
        different_subnet.target_address = Ipv4Addr::new(198, 51, 100, 2);
        assert!(different_subnet.validate().is_err());
    }

    #[test]
    fn discover_from_configured_client_builds_a_matching_offer() {
        let config = config();
        let packet = request_packet(
            DhcpMessageType::Discover,
            config.expected_client_mac,
            None,
            None,
            Ipv4Addr::UNSPECIFIED,
        );
        let request = DhcpRequest::parse(&packet).expect("valid DHCP discover");

        assert_eq!(
            response_type_for(&request, &config),
            Some(DhcpMessageType::Offer)
        );

        let response = build_response(&request, &config, DhcpMessageType::Offer);
        assert_eq!(response[0..3], [2, 1, 6]);
        assert_eq!(response[4..8], [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(response[8..10], [0, 42]);
        assert_eq!(response[10..12], [0x80, 0]);
        assert_eq!(response[16..20], config.target_address.octets());
        assert_eq!(response[20..24], config.server_address.octets());
        assert_eq!(response[28..34], config.expected_client_mac);
        assert_eq!(response[236..DHCP_FIXED_LEN], DHCP_MAGIC_COOKIE);
        assert_eq!(
            &response[DHCP_FIXED_LEN..],
            &[53, 1, 2, 54, 4, 192, 0, 2, 1, 1, 4, 255, 255, 255, 0, 51, 4, 0, 0, 14, 16, 255,]
        );
    }

    #[test]
    fn request_for_the_reserved_lease_builds_an_ack() {
        let config = config();
        let packet = request_packet(
            DhcpMessageType::Request,
            config.expected_client_mac,
            Some(config.target_address),
            Some(config.server_address),
            Ipv4Addr::UNSPECIFIED,
        );
        let request = DhcpRequest::parse(&packet).expect("valid DHCP request");

        assert_eq!(
            response_type_for(&request, &config),
            Some(DhcpMessageType::Ack)
        );
        let response = build_response(&request, &config, DhcpMessageType::Ack);
        assert_eq!(response[DHCP_FIXED_LEN + 2], DhcpMessageType::Ack as u8);
    }

    #[test]
    fn packets_for_another_client_or_lease_never_receive_a_response() {
        let config = config();
        let wrong_mac = request_packet(
            DhcpMessageType::Discover,
            [0x02, 0xaa, 0, 0, 0, 2],
            None,
            None,
            Ipv4Addr::UNSPECIFIED,
        );
        let wrong_mac_request = DhcpRequest::parse(&wrong_mac).expect("valid DHCP discover");
        assert_ne!(wrong_mac_request.client_mac, config.expected_client_mac);
        assert!(response_for(&wrong_mac_request, &config).is_none());

        let wrong_lease = request_packet(
            DhcpMessageType::Request,
            config.expected_client_mac,
            Some(Ipv4Addr::new(192, 0, 2, 99)),
            Some(config.server_address),
            Ipv4Addr::UNSPECIFIED,
        );
        let wrong_lease_request = DhcpRequest::parse(&wrong_lease).expect("valid DHCP request");
        assert_eq!(response_type_for(&wrong_lease_request, &config), None);

        let other_server = request_packet(
            DhcpMessageType::Request,
            config.expected_client_mac,
            Some(config.target_address),
            Some(Ipv4Addr::new(192, 0, 2, 254)),
            Ipv4Addr::UNSPECIFIED,
        );
        let other_server_request = DhcpRequest::parse(&other_server).expect("valid DHCP request");
        assert_eq!(response_type_for(&other_server_request, &config), None);
    }

    #[test]
    fn parser_rejects_malformed_or_ambiguous_critical_options() {
        assert!(DhcpRequest::parse(&[]).is_none());

        let config = config();
        let mut bad_cookie = request_packet(
            DhcpMessageType::Discover,
            config.expected_client_mac,
            None,
            None,
            Ipv4Addr::UNSPECIFIED,
        );
        bad_cookie[236] = 0;
        assert!(DhcpRequest::parse(&bad_cookie).is_none());

        let mut duplicate_type = request_packet(
            DhcpMessageType::Discover,
            config.expected_client_mac,
            None,
            None,
            Ipv4Addr::UNSPECIFIED,
        );
        duplicate_type.pop();
        duplicate_type.extend_from_slice(&[OPTION_MESSAGE_TYPE, 1, 1, OPTION_END]);
        assert!(DhcpRequest::parse(&duplicate_type).is_none());

        assert!(parse_options(&[OPTION_MESSAGE_TYPE, 2, 1, 0, OPTION_END]).is_none());
    }

    #[test]
    fn directed_broadcast_stays_inside_the_configured_subnet() {
        let config = config();
        assert_eq!(
            config.broadcast_destination(),
            "192.0.2.255:68".parse().expect("valid socket address")
        );
    }

    #[test]
    fn explicit_interface_binding_requires_the_current_index_and_address() {
        let address = Ipv4Addr::new(192, 0, 2, 1);
        let interface =
            DhcpInterface::new("en7", NonZeroU32::new(7).expect("non-zero interface index"))
                .expect("valid DHCP interface");
        let current = [observed_interface("en7", Some(7), Some(address))];
        assert!(validate_interface_in_records(&current, &interface, address).is_ok());

        let rebound = [observed_interface("en7", Some(8), Some(address))];
        assert!(validate_interface_in_records(&rebound, &interface, address).is_err());

        let without_address = [observed_interface("en7", Some(7), None)];
        assert!(validate_interface_in_records(&without_address, &interface, address).is_err());

        let duplicate_address = [
            observed_interface("en7", Some(7), Some(address)),
            observed_interface("en8", Some(8), Some(address)),
        ];
        assert!(validate_interface_in_records(&duplicate_address, &interface, address).is_err());
    }

    #[test]
    fn interface_identity_rejects_empty_names_and_zero_indices() {
        let valid_index = NonZeroU32::new(7).expect("non-zero interface index");
        assert!(DhcpInterface::new("", valid_index).is_err());
        assert!(DhcpInterface::new("en7\0", valid_index).is_err());
        assert!(NonZeroU32::new(0).is_none());
    }
}
