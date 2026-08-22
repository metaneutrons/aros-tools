//! Linux USB CDC-ECM adapter discovery through sysfs.
//!
//! The scanner deliberately reads sysfs directly instead of shelling out to
//! `ip`, `udevadm`, or `nmcli`. That makes discovery available on minimal
//! developer hosts and keeps the selection data tied to the kernel device
//! hierarchy.

use super::UsbEcmAdapter;
use miette::Result;
use std::fs;
use std::path::{Path, PathBuf};

const SYS_CLASS_NET: &str = "/sys/class/net";

/// Finds USB network interfaces implemented by a CDC-ECM gadget.
///
/// IPv4 addresses are intentionally left empty here. Linux sysfs exposes the
/// USB and link-layer identity needed to select an adapter, but does not offer
/// a portable interface-to-IPv4 mapping. Address validation belongs to the
/// service startup path, where it can fail closed for the selected interface.
pub(super) fn scan() -> Result<Vec<UsbEcmAdapter>> {
    scan_root(Path::new(SYS_CLASS_NET))
}

fn scan_root(class_net_root: &Path) -> Result<Vec<UsbEcmAdapter>> {
    let entries = fs::read_dir(class_net_root).map_err(|error| {
        miette::miette!(
            "Could not read Linux network interfaces at '{}': {error}",
            class_net_root.display()
        )
    })?;

    let mut interfaces = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            miette::miette!(
                "Could not enumerate Linux network interfaces at '{}': {error}",
                class_net_root.display()
            )
        })?;
        interfaces.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    interfaces.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    Ok(interfaces
        .into_iter()
        .filter_map(|(interface, path)| scan_interface(&interface, &path))
        .collect())
}

fn scan_interface(interface: &str, class_entry: &Path) -> Option<UsbEcmAdapter> {
    // A class entry is a symlink to the real device hierarchy. Resolving it
    // lets us find both the USB interface and its USB-device parent.
    let device_path = fs::canonicalize(class_entry).ok()?;
    let (usb_device, cdc_ecm) = find_usb_device(&device_path)?;
    if !cdc_ecm {
        return None;
    }

    Some(UsbEcmAdapter {
        interface: interface.to_string(),
        vendor_id: read_hex_attribute(&usb_device, "idVendor")?,
        product_id: read_hex_attribute(&usb_device, "idProduct")?,
        serial: read_string_attribute(&usb_device, "serial"),
        manufacturer: read_string_attribute(&usb_device, "manufacturer"),
        product: read_string_attribute(&usb_device, "product"),
        interface_mac: read_string_attribute(&device_path, "address"),
        ipv4_addresses: Vec::new(),
        cdc_ecm,
    })
}

fn find_usb_device(device_path: &Path) -> Option<(PathBuf, bool)> {
    let mut cdc_ecm = false;

    for ancestor in device_path.ancestors() {
        cdc_ecm |= is_cdc_ecm_interface(ancestor);
        if has_usb_device_identity(ancestor) {
            return Some((ancestor.to_path_buf(), cdc_ecm));
        }
    }

    None
}

fn is_cdc_ecm_interface(path: &Path) -> bool {
    let class_matches = read_hex_attribute(path, "bInterfaceClass") == Some(0x02)
        && read_hex_attribute(path, "bInterfaceSubClass") == Some(0x06);
    let driver_matches = matches!(driver_name(path).as_deref(), Some("cdc_ether"));

    class_matches || driver_matches
}

fn has_usb_device_identity(path: &Path) -> bool {
    read_hex_attribute(path, "idVendor").is_some()
        && read_hex_attribute(path, "idProduct").is_some()
}

fn driver_name(path: &Path) -> Option<String> {
    fs::read_link(path.join("driver"))
        .ok()?
        .file_name()?
        .to_str()
        .map(ToOwned::to_owned)
}

fn read_hex_attribute(path: &Path, attribute: &str) -> Option<u16> {
    let value = read_string_attribute(path, attribute)?;
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(&value);
    u16::from_str_radix(value, 16).ok()
}

fn read_string_attribute(path: &Path, attribute: &str) -> Option<String> {
    let value = fs::read_to_string(path.join(attribute)).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::error::Error;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn finds_cdc_ecm_interface_from_usb_descriptors() -> std::result::Result<(), Box<dyn Error>> {
        let fixture = SysfsFixture::new("enxaros", "02", "06", None)?;

        let adapters = scan_root(&fixture.class_net)?;

        assert_eq!(adapters.len(), 1);
        let adapter = &adapters[0];
        assert_eq!(adapter.interface, "enxaros");
        assert_eq!(adapter.vendor_id, 0x1d6b);
        assert_eq!(adapter.product_id, 0x0104);
        assert_eq!(adapter.serial.as_deref(), Some("aros-rpi4-lab-01"));
        assert_eq!(adapter.manufacturer.as_deref(), Some("AROS"));
        assert_eq!(adapter.product.as_deref(), Some("Raspberry Pi USB ECM"));
        assert_eq!(adapter.interface_mac.as_deref(), Some("02:aa:00:00:00:01"));
        assert!(adapter.ipv4_addresses.is_empty());
        assert!(adapter.cdc_ecm);
        Ok(())
    }

    #[test]
    fn finds_cdc_ecm_interface_from_driver_name() -> std::result::Result<(), Box<dyn Error>> {
        let fixture = SysfsFixture::new("enxaros", "ff", "00", Some("cdc_ether"))?;

        let adapters = scan_root(&fixture.class_net)?;

        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].interface, "enxaros");
        assert!(adapters[0].cdc_ecm);
        Ok(())
    }

    #[test]
    fn ignores_non_ecm_usb_network_interfaces() -> std::result::Result<(), Box<dyn Error>> {
        let fixture = SysfsFixture::new("enxother", "02", "0a", None)?;

        assert!(scan_root(&fixture.class_net)?.is_empty());
        Ok(())
    }

    struct SysfsFixture {
        _temp_dir: TempDir,
        class_net: PathBuf,
    }

    impl SysfsFixture {
        fn new(
            interface: &str,
            interface_class: &str,
            interface_subclass: &str,
            driver: Option<&str>,
        ) -> std::result::Result<Self, Box<dyn Error>> {
            let temp_dir = tempfile::tempdir()?;
            let sys_root = temp_dir.path().join("sys");
            let class_net = sys_root.join("class/net");
            let usb_device = sys_root.join("devices/usb1/1-1");
            let usb_interface = usb_device.join("1-1:1.0");
            let network_device = usb_interface.join("net").join(interface);

            fs::create_dir_all(&class_net)?;
            fs::create_dir_all(&network_device)?;
            write_attribute(&usb_device, "idVendor", "1d6b")?;
            write_attribute(&usb_device, "idProduct", "0104")?;
            write_attribute(&usb_device, "serial", "aros-rpi4-lab-01")?;
            write_attribute(&usb_device, "manufacturer", "AROS")?;
            write_attribute(&usb_device, "product", "Raspberry Pi USB ECM")?;
            write_attribute(&usb_interface, "bInterfaceClass", interface_class)?;
            write_attribute(&usb_interface, "bInterfaceSubClass", interface_subclass)?;
            write_attribute(&network_device, "address", "02:aa:00:00:00:01")?;

            if let Some(driver) = driver {
                let driver_path = sys_root.join("bus/usb/drivers").join(driver);
                fs::create_dir_all(&driver_path)?;
                symlink(&driver_path, usb_interface.join("driver"))?;
            }

            symlink(&network_device, class_net.join(interface))?;
            Ok(Self {
                _temp_dir: temp_dir,
                class_net,
            })
        }
    }

    fn write_attribute(
        directory: &Path,
        name: &str,
        value: &str,
    ) -> std::result::Result<(), Box<dyn Error>> {
        fs::create_dir_all(directory)?;
        fs::write(directory.join(name), value)?;
        Ok(())
    }
}
