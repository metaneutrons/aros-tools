//! macOS USB CDC-ECM discovery through the IOKit registry.
//!
//! `enN` is deliberately not treated as a stable identity: macOS assigns it
//! anew as devices appear.  Instead, this module walks the `ioreg` tree from a
//! USB device to its CDC-ECM interface and its `IOEthernetInterface` child.
//! That preserves the USB descriptor identity alongside the currently assigned
//! BSD interface name without changing any network state.

use super::UsbEcmAdapter;
use miette::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

const IOREG_PATH: &str = "/usr/sbin/ioreg";

/// Read the IOKit tree rooted at USB devices and report CDC-ECM adapters.
///
/// No address lookup is performed here.  The service startup path must verify
/// the selected interface still owns the configured concrete address before it
/// binds any network socket.
pub fn scan() -> Result<Vec<UsbEcmAdapter>> {
    let output = crate::run_output(
        Command::new(IOREG_PATH).args(["-r", "-c", "IOUSBHostDevice", "-l", "-w", "0"]),
        "IOKit USB CDC-ECM inventory",
    )?;

    let output = String::from_utf8(output.stdout)
        .map_err(|error| miette::miette!("'{IOREG_PATH}' returned non-UTF-8 output: {error}"))?;
    Ok(parse_ioreg(&output))
}

#[derive(Debug)]
struct IokitNode {
    depth: usize,
    class: String,
    properties: BTreeMap<String, String>,
}

/// Parse the textual tree produced by `ioreg -r -c IOUSBHostDevice -l -w 0`.
///
/// The output is not a stable machine-format API, so this accepts only the
/// narrow, directly reported properties we need.  In particular, it never
/// guesses a USB parent from an unrelated `enN` interface.
fn parse_ioreg(output: &str) -> Vec<UsbEcmAdapter> {
    let nodes = parse_nodes(output);
    let parents = usb_device_parents(&nodes);
    let mut devices = BTreeMap::<usize, DeviceCandidate>::new();

    for (index, node) in nodes.iter().enumerate() {
        if node.class == "IOUSBHostDevice" {
            devices.insert(index, DeviceCandidate::from_usb_device(node));
            continue;
        }

        let Some(device_index) = parents[index] else {
            continue;
        };
        let Some(device) = devices.get_mut(&device_index) else {
            continue;
        };

        if node.class == "IOUSBHostInterface" && is_cdc_ecm(node) {
            device.cdc_ecm = true;
        }

        if matches!(
            node.class.as_str(),
            "IOEthernetInterface" | "IOSkywalkNetworkInterface"
        ) {
            if let Some(interface) = string_property(&node.properties, &["BSD Name"]) {
                device.interfaces.insert(interface);
            }
        }

        if device.interface_mac.is_none() {
            device.interface_mac = mac_property(&node.properties);
        }
    }

    let mut adapters = Vec::new();
    for device in devices.into_values() {
        if !device.cdc_ecm {
            continue;
        }
        let (Some(vendor_id), Some(product_id)) = (device.vendor_id, device.product_id) else {
            // A candidate without its USB descriptor IDs is not safe to pair.
            continue;
        };

        for interface in device.interfaces {
            adapters.push(UsbEcmAdapter {
                interface,
                vendor_id,
                product_id,
                serial: device.serial.clone(),
                manufacturer: device.manufacturer.clone(),
                product: device.product.clone(),
                interface_mac: device.interface_mac.clone(),
                ipv4_addresses: Vec::new(),
                cdc_ecm: true,
            });
        }
    }
    adapters
}

#[derive(Debug, Default)]
struct DeviceCandidate {
    vendor_id: Option<u16>,
    product_id: Option<u16>,
    serial: Option<String>,
    manufacturer: Option<String>,
    product: Option<String>,
    interface_mac: Option<String>,
    interfaces: BTreeSet<String>,
    cdc_ecm: bool,
}

impl DeviceCandidate {
    fn from_usb_device(node: &IokitNode) -> Self {
        Self {
            vendor_id: number_property(&node.properties, &["idVendor"]),
            product_id: number_property(&node.properties, &["idProduct"]),
            serial: string_property(
                &node.properties,
                &[
                    "USB Serial Number",
                    "kUSBSerialNumberString",
                    "USB Serial Number String",
                ],
            ),
            manufacturer: string_property(
                &node.properties,
                &["USB Vendor Name", "kUSBVendorString"],
            ),
            product: string_property(&node.properties, &["USB Product Name", "kUSBProductString"]),
            ..Self::default()
        }
    }
}

fn parse_nodes(output: &str) -> Vec<IokitNode> {
    let mut nodes = Vec::new();
    let mut current = None;

    for line in output.lines() {
        if let Some((depth, class)) = node_header(line) {
            nodes.push(IokitNode {
                depth,
                class,
                properties: BTreeMap::new(),
            });
            current = Some(nodes.len() - 1);
            continue;
        }

        let Some((key, value)) = property(line) else {
            continue;
        };
        if let Some(index) = current {
            nodes[index].properties.insert(key, value);
        }
    }

    nodes
}

fn node_header(line: &str) -> Option<(usize, String)> {
    let marker = "+-o ";
    let marker_index = line.find(marker)?;
    // In `ioreg`'s text renderer every tree level takes two character cells:
    // the immediate child is prefixed with two spaces (`  +-o`), while deeper
    // levels include the visual `|` bars (`  | +-o`, `  | | +-o`).  Counting
    // bars would incorrectly make an immediate child a second root.
    let depth = marker_index / 2;
    let after_marker = &line[marker_index + marker.len()..];
    let class_marker = "<class ";
    let class_index = after_marker.find(class_marker)?;
    let after_class = &after_marker[class_index + class_marker.len()..];
    let class = after_class.split([',', '>']).next()?.trim();
    (!class.is_empty()).then(|| (depth, class.to_string()))
}

fn property(line: &str) -> Option<(String, String)> {
    let line = line.trim_start_matches([' ', '|']);
    let rest = line.strip_prefix('"')?;
    let quote_index = rest.find('"')?;
    let key = &rest[..quote_index];
    let value = rest[quote_index + 1..]
        .trim_start()
        .strip_prefix('=')?
        .trim();
    Some((key.to_string(), value.to_string()))
}

/// Find the closest USB-device ancestor for every IOKit node.
///
/// A USB hub can expose another `IOUSBHostDevice` below it.  Maintaining a
/// depth stack ensures each CDC interface is associated with the innermost
/// physical device, rather than also being attributed to the hub.
fn usb_device_parents(nodes: &[IokitNode]) -> Vec<Option<usize>> {
    let mut parents = vec![None; nodes.len()];
    let mut stack = Vec::<(usize, usize)>::new();

    for (index, node) in nodes.iter().enumerate() {
        while stack.last().is_some_and(|(depth, _)| *depth >= node.depth) {
            stack.pop();
        }
        parents[index] = stack.last().map(|(_, index)| *index);
        if node.class == "IOUSBHostDevice" {
            stack.push((node.depth, index));
        }
    }

    parents
}

fn is_cdc_ecm(node: &IokitNode) -> bool {
    number_property(&node.properties, &["bInterfaceClass"]) == Some(0x02)
        && number_property(&node.properties, &["bInterfaceSubClass"]) == Some(0x06)
}

fn number_property(properties: &BTreeMap<String, String>, keys: &[&str]) -> Option<u16> {
    let raw = string_property(properties, keys)?;
    match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        Some(value) => u16::from_str_radix(value, 16).ok(),
        None if raw.chars().all(|character| character.is_ascii_hexdigit())
            && raw.chars().any(|character| character.is_ascii_alphabetic()) =>
        {
            u16::from_str_radix(&raw, 16).ok()
        }
        None => raw.parse().ok(),
    }
}

fn string_property(properties: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    let raw = keys.iter().find_map(|key| properties.get(*key))?.trim();
    let value = raw
        .strip_prefix('"')
        .and_then(|value| value.split_once('"').map(|(value, _)| value))
        .unwrap_or(raw)
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn mac_property(properties: &BTreeMap<String, String>) -> Option<String> {
    [
        "IOMACAddress",
        "IOMediaAddress",
        "MAC Address",
        "USB Ethernet Address",
        "ethernet-address",
    ]
    .iter()
    .find_map(|key| properties.get(*key))
    .and_then(|value| normalize_mac(value))
}

fn normalize_mac(value: &str) -> Option<String> {
    let value = value.trim().trim_matches(['<', '>', '"']);
    let mut digits = String::with_capacity(12);
    for character in value.chars() {
        if character.is_ascii_hexdigit() {
            digits.push(character.to_ascii_lowercase());
        } else if !matches!(character, ':' | '-' | '.') {
            return None;
        }
    }
    if digits.len() != 12 {
        return None;
    }

    Some(
        digits
            .as_bytes()
            .chunks_exact(2)
            .map(|chunk| std::str::from_utf8(chunk).expect("MAC digits are ASCII"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(test)]
mod tests {
    use super::parse_ioreg;
    use std::fmt::Write;

    const ECM_FIXTURE: &str = r#"
+-o AROS Pi USB ECM@14200000  <class IOUSBHostDevice, id 0x100000001, registered, matched, active, busy 0 (1 ms), retain 23>
  | {
  |   "idVendor" = 4617
  |   "idProduct" = 1040
  |   "USB Vendor Name" = "AROS"
  |   "USB Product Name" = "Pi USB ECM"
  |   "USB Serial Number" = "aros-rpi4-lab-01"
  | }
  |
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x100000002, registered, matched, active, busy 0 (0 ms), retain 10>
  | | {
  | |   "bInterfaceClass" = 2
  | |   "bInterfaceSubClass" = 6
  | | }
  | |
  | +-o AROSEthernet  <class AppleUSBEthernetDevice, id 0x100000003, registered, matched, active, busy 0 (0 ms), retain 10>
  |   | {
  |   |   "IOMACAddress" = <02aa00000002>
  |   | }
  |   |
  |   +-o en7  <class IOEthernetInterface, id 0x100000004, registered, matched, active, busy 0 (0 ms), retain 10>
  |     | {
  |     |   "BSD Name" = "en7"
  |     | }
"#;

    const NON_ECM_FIXTURE: &str = r#"
+-o Other USB Ethernet@14300000  <class IOUSBHostDevice, id 0x100000101, registered, matched, active, busy 0 (1 ms), retain 23>
  | {
  |   "idVendor" = 4660
  |   "idProduct" = 22136
  | }
  |
  +-o IOUSBHostInterface@0  <class IOUSBHostInterface, id 0x100000102, registered, matched, active, busy 0 (0 ms), retain 10>
  | | {
  | |   "bInterfaceClass" = 2
  | |   "bInterfaceSubClass" = 10
  | | }
  | +-o en8  <class IOEthernetInterface, id 0x100000103, registered, matched, active, busy 0 (0 ms), retain 10>
  |   | {
  |   |   "BSD Name" = "en8"
  |   | }
"#;

    #[test]
    fn resolves_usb_identity_through_the_iokit_tree() {
        let adapters = parse_ioreg(ECM_FIXTURE);

        assert_eq!(adapters.len(), 1);
        let adapter = &adapters[0];
        assert_eq!(adapter.interface, "en7");
        assert_eq!(adapter.vendor_id, 0x1209);
        assert_eq!(adapter.product_id, 0x0410);
        assert_eq!(adapter.serial.as_deref(), Some("aros-rpi4-lab-01"));
        assert_eq!(adapter.manufacturer.as_deref(), Some("AROS"));
        assert_eq!(adapter.product.as_deref(), Some("Pi USB ECM"));
        assert_eq!(adapter.interface_mac.as_deref(), Some("02:aa:00:00:00:02"));
        assert!(adapter.ipv4_addresses.is_empty());
        assert!(adapter.cdc_ecm);
    }

    #[test]
    fn ignores_usb_ethernet_that_is_not_cdc_ecm() {
        assert!(parse_ioreg(NON_ECM_FIXTURE).is_empty());
    }

    #[test]
    fn keeps_a_nested_usb_device_with_its_own_interface() {
        let fixture = format!(
            "+-o USB Hub  <class IOUSBHostDevice, id 0x1, registered, matched, active, busy 0, retain 1>\n  | {{\n  |   \"idVendor\" = 1\n  |   \"idProduct\" = 2\n  | }}\n  | +-o Downstream device  <class IOUSBHostDevice, id 0x2, registered, matched, active, busy 0, retain 1>\n{}",
            indent_fixture(ECM_FIXTURE, "  |   ")
        );

        let adapters = parse_ioreg(&fixture);
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].interface, "en7");
        assert_eq!(adapters[0].vendor_id, 0x1209);
    }

    fn indent_fixture(fixture: &str, prefix: &str) -> String {
        fixture
            .lines()
            .skip(1)
            .fold(String::new(), |mut output, line| {
                let _ = writeln!(output, "{prefix}{line}");
                output
            })
    }
}
