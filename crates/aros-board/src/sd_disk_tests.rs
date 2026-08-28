use super::{
    confirmation_token, macos_candidate_from_info, make_candidate, parse_linux_inventory,
    validate_artifact_against_expectation, validate_artifact_for_board, verify_image_artifact,
    write_verified_image_with_backend, write_verified_image_with_backend_and_expectation,
    BoardImageExpectation, DiskBackend, DiskCandidate, DiskPlatform, OpenedTarget, TestTargetGuard,
    UsbEcmArtifactIdentity,
};
use miette::Result;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct FakeBackend {
    candidates: Vec<DiskCandidate>,
    target: PathBuf,
    after_open_safe: bool,
}

impl DiskBackend for FakeBackend {
    fn scan(&self) -> Result<Vec<DiskCandidate>> {
        Ok(self.candidates.clone())
    }

    fn open_verified_target(&self, _candidate: &DiskCandidate) -> Result<OpenedTarget> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.target)
            .map_err(|error| miette::miette!("Could not open injected test target: {error}"))?;
        Ok(OpenedTarget::unclaimed(file))
    }

    fn verify_target_safe_after_open(
        &self,
        _candidate: &DiskCandidate,
        _target: &OpenedTarget,
    ) -> Result<()> {
        if self.after_open_safe {
            Ok(())
        } else {
            miette::bail!("Injected post-open mount revalidation failure.");
        }
    }
}

struct GuardedFakeBackend {
    candidate: DiskCandidate,
    target: PathBuf,
    guard_drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    after_open_safe: bool,
}

impl DiskBackend for GuardedFakeBackend {
    fn scan(&self) -> Result<Vec<DiskCandidate>> {
        Ok(vec![self.candidate.clone()])
    }

    fn open_verified_target(&self, _candidate: &DiskCandidate) -> Result<OpenedTarget> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.target)
            .map_err(|error| {
                miette::miette!("Could not open injected guarded test target: {error}")
            })?;
        Ok(OpenedTarget::unclaimed_with_test_guard(
            file,
            TestTargetGuard(std::sync::Arc::clone(&self.guard_drops)),
        ))
    }

    fn verify_target_safe_after_open(
        &self,
        _candidate: &DiskCandidate,
        target: &OpenedTarget,
    ) -> Result<()> {
        assert!(target.file().is_ok(), "raw target must still be open");
        assert_eq!(
            self.guard_drops.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "claim-like ownership must still be retained during post-open verification"
        );
        if self.after_open_safe {
            Ok(())
        } else {
            miette::bail!("Injected post-open guarded revalidation failure.");
        }
    }
}

fn write_image_artifact(artifact_dir: &Path, image_name: &str, image: &[u8]) {
    fs::create_dir_all(artifact_dir).expect("artifact directory");
    fs::write(artifact_dir.join(image_name), image).expect("image");
    let manifest = json!({
        "format_version": 1,
        "kind": "aros-board-sd-image",
        "board": {
            "name": "rpi4-lab",
            "model": "rpi4",
            "transport": "uboot-usb-ecm",
        },
        "usb_ecm": {
            "vendor_id": 0x1d6b,
            "product_id": 0x0104,
            "serial": "aros-rpi4-lab-01",
            "expected_target_mac": "02:aa:00:00:00:01",
        },
        "partition": {
            "scheme": "mbr",
            "filesystem": "fat32",
            "start_lba": 2048,
            "size_bytes": 67_108_864_u64,
            "label": "AROSBOOT",
            "layout_sha256": "0".repeat(64),
        },
        "source_manifest": {
            "filename": "boot-bundle.toml",
            "sha256": "1".repeat(64),
        },
        "image": {
            "filename": image_name,
            "sha256": super::sha256_hex(image),
            "size_bytes": image.len(),
        },
        "minimum_device_bytes": 4096,
        "payload": [],
    });
    fs::write(
        artifact_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");
}

fn matching_board_expectation() -> BoardImageExpectation {
    BoardImageExpectation::new("rpi4-lab", "rpi4", "uboot-usb-ecm").with_usb_ecm_identity(
        UsbEcmArtifactIdentity {
            vendor_id: 0x1d6b,
            product_id: 0x0104,
            serial: "aros-rpi4-lab-01".to_string(),
            // The comparison intentionally accepts the normal equivalent
            // configured spelling while retaining one canonical value.
            expected_target_mac: "02:AA:00:00:00:01".to_string(),
        },
    )
}

fn fake_candidate() -> DiskCandidate {
    make_candidate(
        DiskPlatform::Linux,
        PathBuf::from("/dev/sdb"),
        PathBuf::from("/dev/sdb"),
        8192,
        "serial:test-card-01".to_string(),
        "Test USB Reader".to_string(),
        "usb".to_string(),
    )
    .expect("test candidate")
}

fn safe_linux_inventory_node() -> Value {
    json!({
        "name": "/dev/sdb",
        "kname": "/dev/sdb",
        "pkname": null,
        "path": "/dev/sdb",
        "type": "disk",
        "size": 33_554_432_u64,
        "rm": true,
        "hotplug": true,
        "ro": false,
        "mountpoints": [null],
        "tran": "usb",
        "serial": "CARD-STRICT-01",
        "wwn": null,
        "model": "SD Reader",
        "vendor": "ACME",
        "children": [{
            "name": "/dev/sdb1",
            "kname": "/dev/sdb1",
            "pkname": "/dev/sdb",
            "path": "/dev/sdb1",
            "type": "part",
            "size": 33_552_384_u64,
            "rm": true,
            "hotplug": true,
            "ro": false,
            "mountpoints": [null]
        }]
    })
}

fn safe_mmc_inventory_node() -> Value {
    json!({
        "name": "/dev/mmcblk0",
        "kname": "mmcblk0",
        "pkname": null,
        "path": "/dev/mmcblk0",
        "type": "disk",
        "size": 33_554_432_u64,
        "rm": true,
        "hotplug": true,
        "ro": false,
        "mountpoints": [null],
        "tran": "mmc",
        "serial": "CARD-MMC-01",
        "wwn": null,
        "model": "SD Card",
        "vendor": "MMC",
        "children": [{
            "name": "/dev/mmcblk0p1",
            "kname": "mmcblk0p1",
            "pkname": "mmcblk0",
            "path": "/dev/mmcblk0p1",
            "type": "part",
            "size": 33_552_384_u64,
            "rm": true,
            "hotplug": true,
            "ro": false,
            "mountpoints": [null]
        }]
    })
}

fn renamed_linux_usb_inventory_node(mut value: Value, root_name: &str) -> Value {
    let root_path = format!("/dev/{root_name}");
    let partition_path = format!("{root_path}1");
    let object = value.as_object_mut().expect("root fixture object");
    for field in ["name", "kname", "path"] {
        object.insert(field.to_string(), json!(root_path));
    }
    let child = object["children"][0]
        .as_object_mut()
        .expect("child fixture object");
    for field in ["name", "kname", "path"] {
        child.insert(field.to_string(), json!(partition_path));
    }
    child.insert("pkname".to_string(), json!(root_path));
    value
}

fn renamed_macos_disk_info(mut value: Value, identifier: &str) -> Value {
    let object = value.as_object_mut().expect("macOS fixture object");
    object.insert("DeviceIdentifier".to_string(), json!(identifier));
    object.insert(
        "DeviceNode".to_string(),
        json!(format!("/dev/{identifier}")),
    );
    object.insert("ParentWholeDisk".to_string(), json!(identifier));
    value
}

fn safe_macos_disk_info() -> Value {
    json!({
        "DeviceIdentifier": "disk7",
        "DeviceNode": "/dev/disk7",
        "ParentWholeDisk": "disk7",
        "WholeDisk": true,
        "Internal": false,
        "Writable": true,
        "Ejectable": true,
        "Removable": true,
        "RemovableMedia": true,
        "MountPoint": null,
        "VirtualOrPhysical": "Physical",
        "Protocol": "USB",
        "SerialNumber": "CARD-STRICT-01",
        "MediaName": "USB SD Reader",
        "Size": 33_554_432_u64,
    })
}

fn safe_macos_topology() -> (Vec<String>, Vec<Value>) {
    (
        vec!["disk7".to_string(), "disk7s1".to_string()],
        vec![
            safe_macos_disk_info(),
            json!({
                "DeviceIdentifier": "disk7s1",
                "DeviceNode": "/dev/disk7s1",
                "ParentWholeDisk": "disk7",
                "WholeDisk": false,
                "MountPoint": null,
            }),
        ],
    )
}

fn without_json_field(mut value: Value, field: &str) -> Value {
    value.as_object_mut().expect("fixture object").remove(field);
    value
}

fn with_json_field(mut value: Value, field: &str, replacement: Value) -> Value {
    value
        .as_object_mut()
        .expect("fixture object")
        .insert(field.to_string(), replacement);
    value
}

fn without_linux_child_field(mut value: Value, field: &str) -> Value {
    value["children"][0]
        .as_object_mut()
        .expect("child fixture object")
        .remove(field);
    value
}

fn with_linux_child_field(mut value: Value, field: &str, replacement: Value) -> Value {
    value["children"][0]
        .as_object_mut()
        .expect("child fixture object")
        .insert(field.to_string(), replacement);
    value
}

#[test]
fn linux_structured_scan_rejects_mounted_and_nonremovable_nodes() {
    let inventory: Value = json!({
        "blockdevices": [
            {
                "name": "/dev/sdb", "kname": "/dev/sdb", "pkname": null,
                "path": "/dev/sdb",
                "type": "disk", "size": 33_554_432_u64, "rm": true, "ro": false,
                "mountpoints": [null], "tran": "usb", "serial": "CARD-01",
                "wwn": null, "model": "SD Reader", "vendor": "ACME", "hotplug": true,
                "children": [{
                    "name": "/dev/sdb1", "kname": "/dev/sdb1", "pkname": "/dev/sdb",
                    "path": "/dev/sdb1",
                    "type": "part", "size": 33_552_384_u64, "rm": true, "ro": false,
                    "mountpoints": [null]
                }]
            },
            {
                "name": "/dev/sdc", "kname": "/dev/sdc", "pkname": null,
                "path": "/dev/sdc",
                "type": "disk", "size": 33_554_432_u64, "rm": true, "ro": false,
                "mountpoints": [null], "tran": "usb", "serial": "CARD-02",
                "wwn": null, "model": "SD Reader", "vendor": "ACME", "hotplug": true,
                "children": [{
                    "name": "/dev/sdc1", "kname": "/dev/sdc1", "pkname": "/dev/sdc",
                    "path": "/dev/sdc1",
                    "type": "part", "size": 33_552_384_u64, "rm": true, "ro": false,
                    "mountpoints": ["/media/card"]
                }]
            },
            {
                "name": "/dev/sda", "kname": "/dev/sda", "pkname": null,
                "path": "/dev/sda",
                "type": "disk", "size": 33_554_432_u64, "rm": false, "ro": false,
                "mountpoints": [null], "tran": "usb", "serial": "SYSTEM-01",
                "wwn": null, "model": "System", "vendor": "ACME", "hotplug": true
            },
            {
                "name": "/dev/loop0", "kname": "/dev/loop0", "pkname": null,
                "path": "/dev/loop0",
                "type": "loop", "size": 33_554_432_u64, "rm": true, "ro": false,
                "mountpoints": [null], "tran": "usb", "serial": "LOOP-01",
                "wwn": null, "model": "Loop", "vendor": "ACME", "hotplug": true
            }
        ]
    });

    let candidates = parse_linux_inventory(&inventory).expect("parse inventory");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].device_path, Path::new("/dev/sdb"));
    assert_eq!(candidates[0].identity, "serial:CARD-01");
    assert!(!candidates[0].mounted);
}

#[test]
fn linux_candidate_requires_every_explicit_removable_safety_signal() {
    let safe = safe_linux_inventory_node();
    let candidate = super::linux_candidate_from_node(&safe).expect("strict safe candidate");
    assert_eq!(candidate.device_path, Path::new("/dev/sdb"));
    let mmc_candidate = super::linux_candidate_from_node(&safe_mmc_inventory_node())
        .expect("strict native MMC candidate");
    assert_eq!(mmc_candidate.device_path, Path::new("/dev/mmcblk0"));
    assert_eq!(mmc_candidate.transport, "mmc");

    let bare_kernel_names = with_linux_child_field(
        with_linux_child_field(
            with_json_field(safe.clone(), "kname", json!("sdb")),
            "kname",
            json!("sdb1"),
        ),
        "pkname",
        json!("sdb"),
    );
    super::linux_candidate_from_node(&bare_kernel_names)
        .expect("lsblk bare kernel-name spelling is accepted and cross-checked");

    let mut rejected = [
        "type",
        "rm",
        "hotplug",
        "ro",
        "mountpoints",
        "tran",
        "name",
        "kname",
        "pkname",
        "path",
        "size",
        "model",
    ]
    .into_iter()
    .map(|field| {
        (
            format!("missing root {field}"),
            without_json_field(safe.clone(), field),
        )
    })
    .collect::<Vec<_>>();
    rejected.extend([
        (
            "non-disk type".to_string(),
            with_json_field(safe.clone(), "type", json!("part")),
        ),
        (
            "rm false".to_string(),
            with_json_field(safe.clone(), "rm", json!(false)),
        ),
        (
            "rm unknown".to_string(),
            with_json_field(safe.clone(), "rm", json!("yes")),
        ),
        (
            "hotplug false".to_string(),
            with_json_field(safe.clone(), "hotplug", json!(false)),
        ),
        (
            "hotplug unknown".to_string(),
            with_json_field(safe.clone(), "hotplug", json!("yes")),
        ),
        (
            "read-only".to_string(),
            with_json_field(safe.clone(), "ro", json!(true)),
        ),
        (
            "unknown read-only state".to_string(),
            with_json_field(safe.clone(), "ro", json!("unknown")),
        ),
        (
            "unsupported transport".to_string(),
            with_json_field(safe.clone(), "tran", json!("sata")),
        ),
        (
            "MMC transport on a SCSI-style path".to_string(),
            with_json_field(safe.clone(), "tran", json!("mmc")),
        ),
        (
            "USB transport on an MMC-style path".to_string(),
            with_json_field(safe_mmc_inventory_node(), "tran", json!("usb")),
        ),
        (
            "whole root has a parent".to_string(),
            with_json_field(safe.clone(), "pkname", json!("sda")),
        ),
        (
            "unknown root parent".to_string(),
            with_json_field(safe.clone(), "pkname", json!(7)),
        ),
        (
            "mounted root".to_string(),
            with_json_field(safe.clone(), "mountpoints", json!(["/media/card"])),
        ),
    ]);

    let mut inconsistent_name = safe.clone();
    inconsistent_name
        .as_object_mut()
        .expect("fixture object")
        .insert("kname".to_string(), json!("/dev/sdc"));
    rejected.push(("inconsistent lsblk names".to_string(), inconsistent_name));

    let mut partition_path = safe.clone();
    for field in ["name", "kname", "path"] {
        partition_path
            .as_object_mut()
            .expect("fixture object")
            .insert(field.to_string(), json!("/dev/sdb1"));
    }
    rejected.push(("partition path".to_string(), partition_path));

    let mut noncanonical_mmc = with_json_field(safe.clone(), "tran", json!("mmc"));
    for field in ["name", "kname", "path"] {
        noncanonical_mmc
            .as_object_mut()
            .expect("fixture object")
            .insert(field.to_string(), json!("/dev/mmcblk01"));
    }
    rejected.push(("non-canonical mmc whole path".to_string(), noncanonical_mmc));

    let mut missing_identity = safe.clone();
    let identity_object = missing_identity.as_object_mut().expect("fixture object");
    identity_object.insert("serial".to_string(), Value::Null);
    identity_object.insert("wwn".to_string(), Value::Null);
    rejected.push(("missing serial and WWN".to_string(), missing_identity));

    for field in ["type", "name", "kname", "pkname", "path", "mountpoints"] {
        rejected.push((
            format!("missing child {field}"),
            without_linux_child_field(safe.clone(), field),
        ));
    }
    rejected.extend([
        (
            "unknown child type".to_string(),
            with_linux_child_field(safe.clone(), "type", json!(7)),
        ),
        (
            "unsupported child type".to_string(),
            with_linux_child_field(safe.clone(), "type", json!("crypt")),
        ),
        (
            "foreign child path".to_string(),
            with_linux_child_field(safe.clone(), "path", json!("/dev/sdc1")),
        ),
        (
            "foreign child name".to_string(),
            with_linux_child_field(safe.clone(), "name", json!("/dev/sdc1")),
        ),
        (
            "foreign child kernel name".to_string(),
            with_linux_child_field(safe.clone(), "kname", json!("/dev/sdc1")),
        ),
        (
            "wrong child parent".to_string(),
            with_linux_child_field(safe.clone(), "pkname", json!("/dev/sdc")),
        ),
        (
            "unknown child parent".to_string(),
            with_linux_child_field(safe.clone(), "pkname", json!(7)),
        ),
        (
            "mounted child".to_string(),
            with_linux_child_field(safe.clone(), "mountpoints", json!(["/media/card"])),
        ),
        (
            "unknown child mount state".to_string(),
            with_linux_child_field(safe.clone(), "mountpoints", json!(7)),
        ),
        (
            "nested child topology".to_string(),
            with_linux_child_field(
                safe.clone(),
                "children",
                json!([{
                    "name": "/dev/sdb2",
                    "kname": "/dev/sdb2",
                    "pkname": "/dev/sdb1",
                    "path": "/dev/sdb2",
                    "type": "part",
                    "mountpoints": [null]
                }]),
            ),
        ),
    ]);

    let mut noncanonical_partition = safe.clone();
    for field in ["name", "kname", "path"] {
        noncanonical_partition["children"][0]
            .as_object_mut()
            .expect("child fixture object")
            .insert(field.to_string(), json!("/dev/sdb01"));
    }
    rejected.push((
        "non-canonical child partition".to_string(),
        noncanonical_partition,
    ));

    let mut duplicate_child = safe.clone();
    let repeated = duplicate_child["children"][0].clone();
    duplicate_child["children"]
        .as_array_mut()
        .expect("children fixture array")
        .push(repeated);
    rejected.push(("duplicate child".to_string(), duplicate_child));

    let invalid_children = with_json_field(safe.clone(), "children", json!("unknown"));
    rejected.push(("invalid child topology".to_string(), invalid_children));

    for (label, node) in rejected {
        assert!(
            super::linux_candidate_from_node(&node).is_none(),
            "unsafe Linux fixture was accepted: {label}"
        );
    }

    let wwn_only = with_json_field(
        with_json_field(safe, "serial", Value::Null),
        "wwn",
        json!("0x5000000000000001"),
    );
    assert_eq!(
        super::linux_candidate_from_node(&wwn_only)
            .expect("WWN is an accepted persistent identity")
            .identity,
        "wwn:0x5000000000000001"
    );
}

#[test]
fn linux_inventory_hides_every_candidate_with_ambiguous_identity_or_path() {
    let first = safe_linux_inventory_node();
    let second = renamed_linux_usb_inventory_node(first.clone(), "sdc");
    let unique = with_json_field(
        renamed_linux_usb_inventory_node(first.clone(), "sdd"),
        "serial",
        json!("CARD-UNIQUE-03"),
    );
    let candidates = parse_linux_inventory(&json!({
        "blockdevices": [first, second, unique]
    }))
    .expect("ambiguous identity inventory");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].device_path, Path::new("/dev/sdd"));

    let same_path_different_identity =
        with_json_field(first.clone(), "serial", json!("CARD-DIFFERENT-02"));
    assert!(
        parse_linux_inventory(&json!({
            "blockdevices": [first, same_path_different_identity]
        }))
        .expect("ambiguous device path inventory")
        .is_empty(),
        "all candidates sharing a device/raw path must be hidden"
    );
}

#[test]
fn physical_candidate_filter_is_platform_scoped_and_rejects_macos_collisions() {
    let first =
        macos_candidate_from_info(&safe_macos_disk_info(), true).expect("first macOS candidate");
    let second_info = renamed_macos_disk_info(safe_macos_disk_info(), "disk8");
    let second = macos_candidate_from_info(&second_info, true).expect("second macOS candidate");
    assert!(
        super::retain_unambiguous_physical_candidates(vec![first.clone(), second]).is_empty(),
        "the same persistent identity on two macOS disks must hide both"
    );

    let different_identity_same_path = macos_candidate_from_info(
        &with_json_field(
            safe_macos_disk_info(),
            "SerialNumber",
            json!("CARD-DIFFERENT-02"),
        ),
        true,
    )
    .expect("same path with a different identity");
    assert!(
        super::retain_unambiguous_physical_candidates(vec![
            first.clone(),
            different_identity_same_path,
        ])
        .is_empty(),
        "the same device path must hide every colliding candidate"
    );

    let raw_path_collision = make_candidate(
        DiskPlatform::Macos,
        PathBuf::from("/dev/disk8"),
        first.raw_device_path.clone(),
        first.size_bytes,
        "serial:CARD-RAW-COLLISION".to_string(),
        first.model.clone(),
        first.transport.clone(),
    )
    .expect("synthetic raw-path collision");
    assert!(
        super::retain_unambiguous_physical_candidates(vec![first.clone(), raw_path_collision,])
            .is_empty(),
        "the same raw path must hide every colliding candidate"
    );

    let linux_same_identity = make_candidate(
        DiskPlatform::Linux,
        PathBuf::from("/dev/sdz"),
        PathBuf::from("/dev/sdz"),
        first.size_bytes,
        first.identity.clone(),
        "USB SD Reader".to_string(),
        "usb".to_string(),
    )
    .expect("cross-platform fixture");
    assert_eq!(
        super::retain_unambiguous_physical_candidates(vec![first, linux_same_identity]).len(),
        2,
        "identity uniqueness is scoped by platform"
    );
}

#[test]
fn macos_structured_scan_requires_physical_unmounted_removable_whole_disk() {
    let safe_info = json!({
        "DeviceIdentifier": "disk7",
        "DeviceNode": "/dev/disk7",
        "ParentWholeDisk": "disk7",
        "WholeDisk": true,
        "Internal": false,
        "Writable": true,
        "Ejectable": true,
        "Removable": true,
        "RemovableMedia": true,
        "MountPoint": "",
        "VirtualOrPhysical": "Physical",
        "Protocol": "USB",
        "SerialNumber": "CARD-01",
        "MediaName": "USB SD Reader",
        "Size": 33_554_432_u64,
    });
    let candidate = macos_candidate_from_info(&safe_info, true).expect("safe candidate");
    assert_eq!(candidate.device_path, Path::new("/dev/disk7"));
    assert_eq!(candidate.raw_device_path, Path::new("/dev/rdisk7"));

    let mounted_info = json!({
        "DeviceIdentifier": "disk7",
        "DeviceNode": "/dev/disk7",
        "ParentWholeDisk": "disk7",
        "WholeDisk": true,
        "Internal": false,
        "Writable": true,
        "Ejectable": true,
        "Removable": true,
        "RemovableMedia": true,
        "MountPoint": "",
        "VirtualOrPhysical": "Physical",
        "Protocol": "USB",
        "SerialNumber": "CARD-01",
        "MediaName": "USB SD Reader",
        "Size": 33_554_432_u64,
    });
    assert!(macos_candidate_from_info(&mounted_info, false).is_none());
}

#[test]
fn macos_candidate_rejects_missing_or_unknown_removability_evidence() {
    let safe = safe_macos_disk_info();
    macos_candidate_from_info(&safe, true).expect("strict safe macOS candidate");

    let only_removable = without_json_field(safe.clone(), "RemovableMedia");
    macos_candidate_from_info(&only_removable, true)
        .expect("explicit Removable=true is sufficient");
    let only_removable_media = without_json_field(safe.clone(), "Removable");
    macos_candidate_from_info(&only_removable_media, true)
        .expect("explicit RemovableMedia=true is sufficient");
    let only_bus_protocol = with_json_field(
        without_json_field(safe.clone(), "Protocol"),
        "BusProtocol",
        json!("USB"),
    );
    macos_candidate_from_info(&only_bus_protocol, true)
        .expect("one explicit supported transport field is sufficient");
    let consistent_usb = with_json_field(safe.clone(), "BusProtocol", json!("USB"));
    macos_candidate_from_info(&consistent_usb, true)
        .expect("matching transport fields are accepted");
    let consistent_sd = with_json_field(
        with_json_field(safe.clone(), "Protocol", json!("Secure Digital")),
        "BusProtocol",
        json!("SD"),
    );
    assert_eq!(
        macos_candidate_from_info(&consistent_sd, true)
            .expect("equivalent SD transport spellings are accepted")
            .transport,
        "sd"
    );

    let mut rejected = Vec::new();
    for field in [
        "DeviceIdentifier",
        "DeviceNode",
        "ParentWholeDisk",
        "WholeDisk",
        "Internal",
        "Writable",
        "Ejectable",
        "MountPoint",
        "VirtualOrPhysical",
        "Protocol",
        "SerialNumber",
        "MediaName",
        "Size",
    ] {
        rejected.push((
            format!("missing {field}"),
            without_json_field(safe.clone(), field),
        ));
    }
    for field in ["WholeDisk", "Internal", "Writable", "Ejectable"] {
        rejected.push((
            format!("unknown {field}"),
            with_json_field(safe.clone(), field, json!("unknown")),
        ));
    }

    rejected.extend([
        (
            "not whole".to_string(),
            with_json_field(safe.clone(), "WholeDisk", json!(false)),
        ),
        (
            "wrong parent".to_string(),
            with_json_field(safe.clone(), "ParentWholeDisk", json!("disk8")),
        ),
        (
            "internal".to_string(),
            with_json_field(safe.clone(), "Internal", json!(true)),
        ),
        (
            "not writable".to_string(),
            with_json_field(safe.clone(), "Writable", json!(false)),
        ),
        (
            "not ejectable".to_string(),
            with_json_field(safe.clone(), "Ejectable", json!(false)),
        ),
        (
            "virtual".to_string(),
            with_json_field(safe.clone(), "VirtualOrPhysical", json!("Virtual")),
        ),
        (
            "unsupported protocol".to_string(),
            with_json_field(safe.clone(), "Protocol", json!("SATA")),
        ),
        (
            "mounted root".to_string(),
            with_json_field(safe.clone(), "MountPoint", json!("/Volumes/CARD")),
        ),
        (
            "wrong device node".to_string(),
            with_json_field(safe.clone(), "DeviceNode", json!("/dev/disk8")),
        ),
        (
            "empty serial".to_string(),
            with_json_field(safe.clone(), "SerialNumber", json!("")),
        ),
        (
            "zero size".to_string(),
            with_json_field(safe.clone(), "Size", json!(0)),
        ),
    ]);

    let neither_removable = without_json_field(
        without_json_field(safe.clone(), "Removable"),
        "RemovableMedia",
    );
    rejected.push(("missing removable evidence".to_string(), neither_removable));

    let false_removable = with_json_field(
        with_json_field(safe.clone(), "Removable", json!(false)),
        "RemovableMedia",
        json!(false),
    );
    rejected.push(("explicitly non-removable".to_string(), false_removable));

    let conflicting_removable = with_json_field(
        with_json_field(safe.clone(), "Removable", json!(false)),
        "RemovableMedia",
        json!(true),
    );
    rejected.push((
        "conflicting removable fields".to_string(),
        conflicting_removable,
    ));
    let conflicting_removable_reverse = with_json_field(
        with_json_field(safe.clone(), "Removable", json!(true)),
        "RemovableMedia",
        json!(false),
    );
    rejected.push((
        "reverse conflicting removable fields".to_string(),
        conflicting_removable_reverse,
    ));
    let unknown_with_true_removable = with_json_field(
        with_json_field(safe.clone(), "Removable", json!("unknown")),
        "RemovableMedia",
        json!(true),
    );
    rejected.push((
        "unknown removable field beside true".to_string(),
        unknown_with_true_removable,
    ));

    let unsupported_bus = with_json_field(safe.clone(), "BusProtocol", json!("PCI"));
    rejected.push((
        "unsupported secondary transport".to_string(),
        unsupported_bus,
    ));
    let conflicting_transport = with_json_field(safe.clone(), "BusProtocol", json!("SD"));
    rejected.push((
        "conflicting supported transport fields".to_string(),
        conflicting_transport,
    ));
    let unknown_bus = with_json_field(safe.clone(), "BusProtocol", json!(1));
    rejected.push(("unknown secondary transport".to_string(), unknown_bus));

    let unknown_removable = without_json_field(
        with_json_field(safe, "Removable", json!("unknown")),
        "RemovableMedia",
    );
    rejected.push(("unknown removable state".to_string(), unknown_removable));

    let noncanonical_identifier = with_json_field(
        with_json_field(
            with_json_field(safe_macos_disk_info(), "DeviceIdentifier", json!("disk07")),
            "DeviceNode",
            json!("/dev/disk07"),
        ),
        "ParentWholeDisk",
        json!("disk07"),
    );
    rejected.push((
        "non-canonical whole-disk identifier".to_string(),
        noncanonical_identifier,
    ));

    for (label, info) in rejected {
        assert!(
            macos_candidate_from_info(&info, true).is_none(),
            "unsafe macOS fixture was accepted: {label}"
        );
    }

    assert!(
        macos_candidate_from_info(&safe_macos_disk_info(), false).is_none(),
        "incomplete or mounted descendant topology must reject the whole disk"
    );
}

#[test]
fn macos_descendant_topology_must_be_complete_unique_and_consistent() {
    let root = safe_macos_disk_info();
    let (identifiers, infos) = safe_macos_topology();
    super::macos_candidate_from_inventory(&root, &identifiers, &infos)
        .expect("complete consistent topology");

    let parsed =
        super::macos_descendant_identifiers(&json!({ "AllDisks": ["disk7s1", "disk7"] }), "disk7")
            .expect("valid descendant list");
    assert_eq!(parsed, identifiers);

    for (label, list) in [
        ("rootless", json!({ "AllDisks": ["disk7s1"] })),
        (
            "duplicate",
            json!({ "AllDisks": ["disk7", "disk7s1", "disk7s1"] }),
        ),
        ("foreign", json!({ "AllDisks": ["disk7", "disk8s1"] })),
        ("unparseable", json!({ "AllDisks": ["disk7", "disk7foo"] })),
        (
            "non-canonical",
            json!({ "AllDisks": ["disk7", "disk7s01"] }),
        ),
        ("non-string", json!({ "AllDisks": ["disk7", 7] })),
    ] {
        assert!(
            super::macos_descendant_identifiers(&list, "disk7").is_err(),
            "unsafe descendant list was accepted: {label}"
        );
    }

    let mut rejected = vec![
        (
            "missing descendant info".to_string(),
            identifiers.clone(),
            vec![infos[0].clone()],
        ),
        (
            "duplicate descendant info".to_string(),
            identifiers.clone(),
            vec![infos[1].clone(), infos[1].clone()],
        ),
        (
            "rootless identifier set".to_string(),
            vec!["disk7s1".to_string()],
            vec![infos[1].clone()],
        ),
        (
            "duplicate identifier set".to_string(),
            vec!["disk7".to_string(), "disk7".to_string()],
            vec![infos[0].clone(), infos[0].clone()],
        ),
        (
            "foreign identifier set".to_string(),
            vec!["disk7".to_string(), "disk8s1".to_string()],
            infos.clone(),
        ),
    ];

    for (label, field, replacement) in [
        ("wrong parent", "ParentWholeDisk", json!("disk8")),
        ("wrong device node", "DeviceNode", json!("/dev/disk8s1")),
        ("child claims whole-disk status", "WholeDisk", json!(true)),
        ("unknown child whole-disk status", "WholeDisk", json!(7)),
        ("mounted descendant", "MountPoint", json!("/Volumes/CARD")),
        ("unknown mount state", "MountPoint", json!(7)),
        ("foreign info", "DeviceIdentifier", json!("disk8s1")),
    ] {
        let mut changed_infos = infos.clone();
        changed_infos[1] = with_json_field(changed_infos[1].clone(), field, replacement);
        rejected.push((label.to_string(), identifiers.clone(), changed_infos));
    }
    let mut missing_mount = infos.clone();
    missing_mount[1] = without_json_field(missing_mount[1].clone(), "MountPoint");
    rejected.push((
        "missing descendant mount state".to_string(),
        identifiers.clone(),
        missing_mount,
    ));
    let mut missing_whole_disk = infos.clone();
    missing_whole_disk[1] = without_json_field(missing_whole_disk[1].clone(), "WholeDisk");
    rejected.push((
        "missing child whole-disk status".to_string(),
        identifiers.clone(),
        missing_whole_disk,
    ));

    let mut false_root_whole_disk = infos.clone();
    false_root_whole_disk[0] =
        with_json_field(false_root_whole_disk[0].clone(), "WholeDisk", json!(false));
    rejected.push((
        "root denies whole-disk status".to_string(),
        identifiers.clone(),
        false_root_whole_disk,
    ));

    let mut changed_root_infos = infos;
    changed_root_infos[0] = with_json_field(
        changed_root_infos[0].clone(),
        "SerialNumber",
        json!("DIFFERENT-CARD"),
    );
    rejected.push((
        "root changed between inventory reads".to_string(),
        identifiers,
        changed_root_infos,
    ));

    for (label, candidate_identifiers, candidate_infos) in rejected {
        assert!(
            super::macos_candidate_from_inventory(&root, &candidate_identifiers, &candidate_infos,)
                .is_none(),
            "unsafe macOS topology was accepted: {label}"
        );
    }
}

#[test]
fn artifact_board_validation_requires_the_exact_usb_ecm_identity() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");

    validate_artifact_against_expectation(&artifact, &matching_board_expectation())
        .expect("matching board expectation");

    let mismatched = BoardImageExpectation::new("rpi4-lab", "rpi4", "uboot-usb-ecm")
        .with_usb_ecm_identity(UsbEcmArtifactIdentity {
            vendor_id: 0x1d6b,
            product_id: 0x0104,
            serial: "a-different-pi".to_string(),
            expected_target_mac: "02:aa:00:00:00:01".to_string(),
        });
    let error = validate_artifact_against_expectation(&artifact, &mismatched)
        .expect_err("identity mismatch must fail");
    assert!(error.to_string().contains("USB-ECM identity"));
}

#[test]
fn local_board_profile_is_converted_and_bound_to_the_artifact() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let boards_path = temporary.path().join("boards.toml");
    fs::write(
        &boards_path,
        r#"
format_version = 2

[boards.rpi4-lab]
backend = "raspberry-pi"
model = "rpi4"
transport = "uboot-usb-ecm"
preset = "arm-raspi"
toolchain_preset = "arm-raspi"
build_target = "rpi-artifacts"

[boards.rpi4-lab.raspberry_pi]
dtb_path = "firmware/bcm2711-rpi-4-b.dtb"
core_kobj_dir = "legacy-kobjs"

[boards.rpi4-lab.usb_ecm]
host_address = "192.168.101.1"
target_address = "192.168.101.2"

[boards.rpi4-lab.usb_ecm.identity]
vendor_id = 0x1d6b
product_id = 0x0104
serial = "aros-rpi4-lab-01"
expected_target_mac = "02:AA:00:00:00:01"
"#,
    )
    .expect("board profile");
    let board =
        crate::config::load_board(Some(&boards_path), "rpi4-lab").expect("local board profile");

    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");

    validate_artifact_for_board(&artifact, &board).expect("matching local board profile");
}

#[test]
fn production_writer_requires_a_matching_local_board_before_any_disk_scan() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let boards_path = temporary.path().join("boards.toml");
    fs::write(
        &boards_path,
        r#"
format_version = 2

[boards.rpi4-lab]
backend = "raspberry-pi"
model = "rpi4"
transport = "uboot-usb-ecm"
preset = "arm-raspi"
toolchain_preset = "arm-raspi"
build_target = "rpi-artifacts"

[boards.rpi4-lab.raspberry_pi]
dtb_path = "firmware/bcm2711-rpi-4-b.dtb"
core_kobj_dir = "legacy-kobjs"

[boards.rpi4-lab.usb_ecm]
host_address = "192.168.101.1"
target_address = "192.168.101.2"

[boards.rpi4-lab.usb_ecm.identity]
vendor_id = 0x1d6b
product_id = 0x0104
serial = "aros-rpi4-lab-01"
expected_target_mac = "02:AA:00:00:00:01"
"#,
    )
    .expect("board profile");
    let mut wrong_board =
        crate::config::load_board(Some(&boards_path), "rpi4-lab").expect("local board profile");
    wrong_board.name = "rpi5-lab".to_string();

    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");

    let error = super::write_verified_image_for_board(
        &artifact,
        &wrong_board,
        "a-deliberate-nonempty-selection",
        "aros-sd-write-v1:test-token",
    )
    .expect_err("production writer must reject a mismatched board before scanning");
    assert!(error.to_string().contains("board.name"));
}

#[test]
fn board_mismatch_stops_before_the_injected_disk_backend_is_scanned() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");
    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, b"still intact").expect("fake target");
    let candidate = fake_candidate();
    let token = confirmation_token(&artifact, &candidate);
    let backend = FakeBackend {
        candidates: vec![candidate.clone()],
        target: target.clone(),
        after_open_safe: true,
    };
    let wrong_board = BoardImageExpectation::new("rpi5-lab", "rpi5", "uboot-usb-ecm")
        .with_usb_ecm_identity(UsbEcmArtifactIdentity {
            vendor_id: 0x1d6b,
            product_id: 0x0104,
            serial: "aros-rpi4-lab-01".to_string(),
            expected_target_mac: "02:aa:00:00:00:01".to_string(),
        });

    let error = write_verified_image_with_backend_and_expectation(
        &artifact,
        &candidate.scan_id,
        &token,
        &wrong_board,
        &backend,
    )
    .expect_err("wrong board must fail before a disk is opened");
    assert!(error.to_string().contains("board.name"));
    assert_eq!(fs::read_to_string(&target).expect("target"), "still intact");
}

#[test]
fn uboot_usb_ecm_image_without_identity_is_not_a_verified_artifact() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let manifest_path = artifact_dir.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest bytes"))
            .expect("manifest JSON");
    manifest
        .as_object_mut()
        .expect("manifest object")
        .insert("usb_ecm".to_string(), Value::Null);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("updated manifest"),
    )
    .expect("updated manifest");

    let error = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect_err("uboot USB-ECM image requires identity");
    assert!(error
        .to_string()
        .contains("lacks a complete USB-ECM identity"));
}

#[test]
fn verified_artifact_and_fake_target_receive_a_checked_write_and_readback() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    let image = b"AROS SD image payload";
    write_image_artifact(&artifact_dir, "aros-rpi4-usb.img", image);
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("aros-rpi4-usb.img"),
    )
    .expect("verified artifact");

    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, vec![0_u8; 8192]).expect("fake target");
    let candidate = fake_candidate();
    let token = confirmation_token(&artifact, &candidate);
    let backend = FakeBackend {
        candidates: vec![candidate.clone()],
        target: target.clone(),
        after_open_safe: true,
    };

    let report = write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
        .expect("fake write");
    assert_eq!(
        report.bytes_written,
        u64::try_from(image.len()).expect("image length")
    );
    assert_eq!(report.readback_sha256, super::sha256_hex(image));
    assert_eq!(
        &fs::read(&target).expect("target bytes")[..image.len()],
        image
    );
}

#[test]
fn opened_target_retains_its_guard_through_write_sync_and_readback() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    let image = b"AROS guarded SD image payload";
    write_image_artifact(&artifact_dir, "image.img", image);
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");

    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, vec![0_u8; 8192]).expect("fake target");
    let candidate = fake_candidate();
    let token = confirmation_token(&artifact, &candidate);
    let guard_drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = GuardedFakeBackend {
        candidate: candidate.clone(),
        target: target.clone(),
        guard_drops: std::sync::Arc::clone(&guard_drops),
        after_open_safe: true,
    };

    let report = write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
        .expect("guarded fake write");
    assert_eq!(report.readback_sha256, super::sha256_hex(image));
    assert_eq!(
        &fs::read(&target).expect("target bytes")[..image.len()],
        image
    );
    assert_eq!(
        guard_drops.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "claim-like ownership must be released exactly once after readback"
    );
}

#[test]
fn post_open_failure_closes_target_and_releases_guard_without_writing() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");

    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, b"leave this target alone").expect("fake target");
    let candidate = fake_candidate();
    let token = confirmation_token(&artifact, &candidate);
    let guard_drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = GuardedFakeBackend {
        candidate: candidate.clone(),
        target: target.clone(),
        guard_drops: std::sync::Arc::clone(&guard_drops),
        after_open_safe: false,
    };

    let error = write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
        .expect_err("guarded post-open check must fail");
    assert!(error.to_string().contains("guarded revalidation"));
    assert_eq!(
        fs::read_to_string(&target).expect("untouched target"),
        "leave this target alone"
    );
    assert_eq!(
        guard_drops.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "claim-like ownership must be released exactly once on failure"
    );
}

#[test]
fn post_open_mount_revalidation_failure_happens_before_the_first_write() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");
    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, b"leave this target alone").expect("fake target");
    let candidate = fake_candidate();
    let token = confirmation_token(&artifact, &candidate);
    let backend = FakeBackend {
        candidates: vec![candidate.clone()],
        target: target.clone(),
        after_open_safe: false,
    };

    let error = write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
        .expect_err("post-open mount check must stop before writes");
    assert!(error.to_string().contains("post-open mount revalidation"));
    assert_eq!(
        fs::read_to_string(&target).expect("untouched target"),
        "leave this target alone"
    );
}

#[test]
fn linux_raw_open_contract_uses_exclusive_nofollow_readwrite_flags() {
    let flags = super::linux_raw_open_flags();
    assert!(flags.contains(rustix::fs::OFlags::RDWR));
    assert!(flags.contains(rustix::fs::OFlags::EXCL));
    assert!(flags.contains(rustix::fs::OFlags::NOFOLLOW));
    assert!(flags.contains(rustix::fs::OFlags::CLOEXEC));
}

#[test]
fn macos_raw_open_contract_uses_nofollow_readwrite_flags() {
    let flags = super::macos_raw_open_flags();
    assert!(flags.contains(rustix::fs::OFlags::RDWR));
    assert!(flags.contains(rustix::fs::OFlags::NOFOLLOW));
    assert!(flags.contains(rustix::fs::OFlags::CLOEXEC));
    assert!(!flags.contains(rustix::fs::OFlags::EXCL));
}

#[test]
fn raw_device_node_type_must_match_the_selected_platform_exactly() {
    assert!(super::raw_device_type_matches_platform(
        DiskPlatform::Linux,
        true,
        false
    ));
    assert!(super::raw_device_type_matches_platform(
        DiskPlatform::Macos,
        false,
        true
    ));

    for platform in [DiskPlatform::Linux, DiskPlatform::Macos] {
        assert!(!super::raw_device_type_matches_platform(
            platform, false, false
        ));
        assert!(!super::raw_device_type_matches_platform(
            platform, true, true
        ));
    }
    assert!(!super::raw_device_type_matches_platform(
        DiskPlatform::Linux,
        false,
        true
    ));
    assert!(!super::raw_device_type_matches_platform(
        DiskPlatform::Macos,
        true,
        false
    ));
}

#[test]
fn mismatched_token_never_opens_the_injected_target() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");
    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, b"leave this target alone").expect("fake target");
    let candidate = fake_candidate();
    let backend = FakeBackend {
        candidates: vec![candidate.clone()],
        target: target.clone(),
        after_open_safe: true,
    };

    let error = write_verified_image_with_backend(
        &artifact,
        &candidate.scan_id,
        "aros-sd-write-v1:not-a-token",
        &backend,
    )
    .expect_err("token must fail");
    assert!(error.to_string().contains("Confirmation token"));
    assert_eq!(
        fs::read_to_string(&target).expect("untouched target"),
        "leave this target alone"
    );
}

#[test]
fn a_mounted_candidate_is_rejected_before_the_injected_target_is_opened() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let artifact_dir = temporary.path().join("artifact");
    write_image_artifact(&artifact_dir, "image.img", b"payload");
    let artifact = verify_image_artifact(
        &artifact_dir,
        Path::new("manifest.json"),
        Path::new("image.img"),
    )
    .expect("verified artifact");
    let target = temporary.path().join("fake-whole-disk");
    fs::write(&target, b"still intact").expect("fake target");
    let mut candidate = fake_candidate();
    candidate.mounted = true;
    let token = confirmation_token(&artifact, &candidate);
    let backend = FakeBackend {
        candidates: vec![candidate.clone()],
        target: target.clone(),
        after_open_safe: true,
    };

    let error = write_verified_image_with_backend(&artifact, &candidate.scan_id, &token, &backend)
        .expect_err("mounted target must fail");
    assert!(error.to_string().contains("safe, whole, removable"));
    assert_eq!(fs::read_to_string(&target).expect("target"), "still intact");
}
