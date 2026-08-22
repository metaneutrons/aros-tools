//! Read-only discovery of USB CDC-ECM adapters.
//!
//! Discovery deliberately stops at reporting a candidate.  It neither changes
//! an interface address nor opens DHCP/TFTP sockets; a future `serve` command
//! must resolve a paired USB identity again before binding a service.

use super::UsbEcmAdapter;
use if_addrs::{get_if_addrs, IfAddr};
use miette::Result;
use std::fmt::Write;
use std::net::Ipv4Addr;

pub(super) fn print() -> Result<()> {
    let adapters = adapters()?;
    if adapters.is_empty() {
        println!("No USB CDC-ECM adapters found.");
        println!(
            "Connect and boot the Pi's U-Boot USB-ECM profile, then run `aros pi scan` again."
        );
        return Ok(());
    }

    print!("{}", format_adapters(&adapters));
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn adapters() -> Result<Vec<UsbEcmAdapter>> {
    enrich_ipv4_addresses(super::scan_linux::scan()?)
}

#[cfg(target_os = "macos")]
pub(super) fn adapters() -> Result<Vec<UsbEcmAdapter>> {
    enrich_ipv4_addresses(super::scan_macos::scan()?)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn adapters() -> Result<Vec<UsbEcmAdapter>> {
    miette::bail!(
        "`aros pi scan` currently supports macOS and Linux only; no USB adapter was selected."
    )
}

/// Attach the current IPv4 addresses after the platform scanner has paired an
/// interface with its USB descriptor.  The USB scanner owns identity; this
/// cross-platform query only answers whether the configured concrete address
/// is currently assigned to that exact resulting interface.
fn enrich_ipv4_addresses(mut adapters: Vec<UsbEcmAdapter>) -> Result<Vec<UsbEcmAdapter>> {
    let interfaces = get_if_addrs().map_err(|error| {
        miette::miette!("Could not enumerate local interface addresses: {error}")
    })?;

    for adapter in &mut adapters {
        adapter.ipv4_addresses = interfaces
            .iter()
            .filter(|interface| interface.name == adapter.interface)
            .filter_map(|interface| match &interface.addr {
                IfAddr::V4(address) => Some(address.ip),
                IfAddr::V6(_) => None,
            })
            .collect::<Vec<Ipv4Addr>>();
        adapter.ipv4_addresses.sort_unstable();
        adapter.ipv4_addresses.dedup();
    }

    Ok(adapters)
}

fn format_adapters(adapters: &[UsbEcmAdapter]) -> String {
    let mut output = String::from("USB CDC-ECM adapters:\n");
    for (index, adapter) in adapters.iter().enumerate() {
        let _ = writeln!(
            output,
            "\n  {}. {}  USB {:04x}:{:04x}",
            index + 1,
            adapter.interface,
            adapter.vendor_id,
            adapter.product_id
        );
        if let Some(product) = &adapter.product {
            let _ = writeln!(output, "     product: {product}");
        }
        if let Some(manufacturer) = &adapter.manufacturer {
            let _ = writeln!(output, "     manufacturer: {manufacturer}");
        }
        if let Some(serial) = &adapter.serial {
            let _ = writeln!(output, "     USB serial: {serial}");
        } else {
            let _ = writeln!(
                output,
                "     USB serial: unavailable (do not pair this device yet)"
            );
        }
        if let Some(mac) = &adapter.interface_mac {
            let _ = writeln!(output, "     host interface MAC: {mac}");
        }
        if !adapter.ipv4_addresses.is_empty() {
            let addresses = adapter
                .ipv4_addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(output, "     IPv4: {addresses}");
        }
        let _ = writeln!(
            output,
            "     state: {}",
            if adapter.cdc_ecm {
                "CDC-ECM confirmed"
            } else {
                "USB Ethernet (CDC-ECM not confirmed)"
            }
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{enrich_ipv4_addresses, format_adapters};
    use crate::pi::UsbEcmAdapter;
    use std::net::Ipv4Addr;

    #[test]
    fn output_preserves_usb_identity_and_ephemeral_interface_name() {
        let output = format_adapters(&[UsbEcmAdapter {
            interface: "en7".to_string(),
            vendor_id: 0x1234,
            product_id: 0x5678,
            serial: Some("aros-rpi4-lab-01".to_string()),
            manufacturer: Some("AROS".to_string()),
            product: Some("Pi USB ECM".to_string()),
            interface_mac: Some("02:aa:00:00:00:02".to_string()),
            ipv4_addresses: vec![Ipv4Addr::new(192, 168, 9, 1)],
            cdc_ecm: true,
        }]);

        assert!(output.contains("en7"));
        assert!(output.contains("1234:5678"));
        assert!(output.contains("aros-rpi4-lab-01"));
        assert!(output.contains("CDC-ECM confirmed"));
    }

    #[test]
    fn address_enrichment_preserves_usb_identity_when_an_interface_is_absent() {
        let adapters = enrich_ipv4_addresses(vec![UsbEcmAdapter {
            interface: "aros-test-interface-that-does-not-exist".to_string(),
            vendor_id: 0x1234,
            product_id: 0x5678,
            serial: Some("serial".to_string()),
            manufacturer: None,
            product: None,
            interface_mac: None,
            ipv4_addresses: vec![Ipv4Addr::LOCALHOST],
            cdc_ecm: true,
        }])
        .expect("address lookup");

        assert_eq!(adapters.len(), 1);
        assert!(adapters[0].ipv4_addresses.is_empty());
        assert_eq!(adapters[0].vendor_id, 0x1234);
    }
}
