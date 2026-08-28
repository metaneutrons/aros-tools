//! Regression tests for boot-bundle validation and deterministic image staging.

use super::{
    sha256_file, stage_boot_bundle, validate_boot_bundle, BundleExpectation, PartitionLayout,
    UsbEcmIdentity, ARTIFACT_CHECKSUMS, ARTIFACT_MANIFEST, BOOT_BUNDLE_MANIFEST,
    BOOT_PAYLOAD_DIRECTORY, RAW_IMAGE_FILENAME, UBOOT_USB_ECM_TRANSPORT, UEFI_ESP_TRANSPORT,
};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::Path;

const RPI4_FILES: [(&str, &str, &str); 6] = [
    ("config", "config.txt", "config.txt"),
    ("firmware-start", "start4.elf", "start4.elf"),
    ("firmware-fixup", "fixup4.dat", "fixup4.dat"),
    ("device-tree", "bcm2711-rpi-4-b.dtb", "bcm2711-rpi-4-b.dtb"),
    ("u-boot", "u-boot.bin", "u-boot.bin"),
    ("boot-script", "boot.scr", "boot.scr"),
];

const TITAN_FILES: [(&str, &str, &str); 5] = [
    (
        "uefi-loader",
        "EFI/BOOT/BOOTRISCV64.EFI",
        "EFI/BOOT/BOOTRISCV64.EFI",
    ),
    ("kernel-image", "EFI/AROS/Image", "EFI/AROS/Image"),
    ("bsp-package", "aros-bsp.pkg", "aros-bsp.pkg"),
    ("command-line", "aros.cmd", "aros.cmd"),
    ("startup-script", "startup.nsh", "startup.nsh"),
];

fn expectation() -> BundleExpectation {
    BundleExpectation::new("rpi4-lab", "rpi4", UBOOT_USB_ECM_TRANSPORT).with_usb_ecm_identity(
        UsbEcmIdentity {
            vendor_id: 0x1d6b,
            product_id: 0x0104,
            serial: "aros-rpi4-lab-01".to_string(),
            expected_target_mac: "02:aa:00:00:00:01".to_string(),
        },
    )
}

fn partition() -> PartitionLayout {
    PartitionLayout {
        scheme: "mbr".to_string(),
        filesystem: "fat32".to_string(),
        start_lba: 2048,
        size_bytes: 64 * 1024 * 1024,
        label: "AROSBOOT".to_string(),
    }
}

fn write_valid_bundle(bundle_dir: &Path) {
    fs::create_dir_all(bundle_dir).expect("bundle directory");
    for (role, source, _) in RPI4_FILES {
        fs::write(bundle_dir.join(source), format!("{role} payload\n")).expect("boot input");
    }
    let partition = partition();
    let mut manifest = format!(
            "format_version = 1\n\n[board]\nname = \"rpi4-lab\"\nmodel = \"rpi4\"\ntransport = \"uboot-usb-ecm\"\n\n[usb_ecm]\nvendor_id = 0x1d6b\nproduct_id = 0x0104\nserial = \"aros-rpi4-lab-01\"\nexpected_target_mac = \"02:AA:00:00:00:01\"\n\n[partition]\nscheme = \"{}\"\nfilesystem = \"{}\"\nstart_lba = {}\nsize_bytes = {}\nlabel = \"{}\"\nlayout_sha256 = \"{}\"\n",
            partition.scheme,
            partition.filesystem,
            partition.start_lba,
            partition.size_bytes,
            partition.label,
            partition.layout_sha256(),
        );
    for (role, source, destination) in RPI4_FILES {
        let (sha256, _) = sha256_file(&bundle_dir.join(source)).expect("input checksum");
        write!(
                &mut manifest,
                "\n[[files]]\nrole = \"{role}\"\nsource = \"{source}\"\ndestination = \"{destination}\"\nsha256 = \"{sha256}\"\n"
            )
            .expect("write TOML file entry");
    }
    fs::write(bundle_dir.join(BOOT_BUNDLE_MANIFEST), manifest).expect("bundle manifest");
}

fn write_valid_titan_bundle(bundle_dir: &Path) {
    fs::create_dir_all(bundle_dir).expect("bundle directory");
    for (role, source, _) in TITAN_FILES {
        let path = bundle_dir.join(source);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("bundle input parent");
        }
        let contents = if role == "command-line" {
            b"\n".to_vec()
        } else {
            format!("{role} payload\n").into_bytes()
        };
        fs::write(path, contents).expect("boot input");
    }
    let partition = partition();
    let mut manifest = format!(
            "format_version = 1\n\n[board]\nname = \"milk-v-titan\"\nmodel = \"milk-v-titan\"\ntransport = \"uefi-esp\"\n\n[partition]\nscheme = \"{}\"\nfilesystem = \"{}\"\nstart_lba = {}\nsize_bytes = {}\nlabel = \"{}\"\nlayout_sha256 = \"{}\"\n",
            partition.scheme,
            partition.filesystem,
            partition.start_lba,
            partition.size_bytes,
            partition.label,
            partition.layout_sha256(),
        );
    for (role, source, destination) in TITAN_FILES {
        let (sha256, _) = sha256_file(&bundle_dir.join(source)).expect("input checksum");
        write!(
                &mut manifest,
                "\n[[files]]\nrole = \"{role}\"\nsource = \"{source}\"\ndestination = \"{destination}\"\nsha256 = \"{sha256}\"\n"
            )
            .expect("write TOML file entry");
    }
    fs::write(bundle_dir.join(BOOT_BUNDLE_MANIFEST), manifest).expect("bundle manifest");
}

fn validate_and_stage_for_test(bundle_dir: &Path, output_dir: &Path) -> super::StagedArtifact {
    let bundle = validate_boot_bundle(bundle_dir, &expectation()).expect("validated bundle");
    stage_boot_bundle(&bundle, output_dir).expect("staged bundle")
}

#[test]
fn stages_a_verified_bundle_with_deterministic_metadata() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let bundle_dir = temporary.path().join("bundle");
    write_valid_bundle(&bundle_dir);

    let first = validate_and_stage_for_test(&bundle_dir, &temporary.path().join("first"));
    let second = validate_and_stage_for_test(&bundle_dir, &temporary.path().join("second"));

    assert!(first
        .artifact_dir()
        .join(BOOT_PAYLOAD_DIRECTORY)
        .join("u-boot.bin")
        .is_file());
    assert!(first.image().path().is_file());
    assert_eq!(
        fs::metadata(first.image().path())
            .expect("raw image metadata")
            .len(),
        64 * 1024 * 1024 + 2048 * 512
    );
    assert_eq!(
        sha256_file(first.image().path())
            .expect("first image checksum")
            .0,
        first.image().sha256()
    );
    assert_eq!(first.image().sha256(), second.image().sha256());
    assert!(first.manifest_path().is_file());
    assert!(first.artifact_dir().join(ARTIFACT_CHECKSUMS).is_file());
    assert_eq!(
        fs::read(first.manifest_path()).expect("first manifest"),
        fs::read(second.manifest_path()).expect("second manifest")
    );
    assert_eq!(
        fs::read(first.artifact_dir().join(ARTIFACT_CHECKSUMS)).expect("first checksums"),
        fs::read(second.artifact_dir().join(ARTIFACT_CHECKSUMS)).expect("second checksums")
    );
    let manifest = fs::read_to_string(first.manifest_path()).expect("artifact manifest");
    assert!(manifest.contains("\"kind\": \"aros-board-sd-image\""));
    assert!(manifest.contains("\"minimum_device_bytes\""));
    assert!(manifest.contains(RAW_IMAGE_FILENAME));
    let disk_artifact = crate::sd_disk::verify_image_artifact(
        first.artifact_dir(),
        Path::new(ARTIFACT_MANIFEST),
        Path::new(RAW_IMAGE_FILENAME),
    )
    .expect("artifact must satisfy the disk-writer contract");
    assert_eq!(disk_artifact.image_sha256(), first.image().sha256());
    let mut mbr = [0_u8; 512];
    std::fs::File::open(first.image().path())
        .expect("raw image")
        .read_exact(&mut mbr)
        .expect("MBR");
    assert_eq!(&mbr[510..512], &[0x55, 0xaa]);
    assert_eq!(mbr[450], 0x0c);
    let checksums =
        fs::read_to_string(first.artifact_dir().join(ARTIFACT_CHECKSUMS)).expect("checksum file");
    assert!(checksums.contains(&format!("{BOOT_PAYLOAD_DIRECTORY}/u-boot.bin")));
    assert!(checksums.contains(RAW_IMAGE_FILENAME));
    assert!(checksums.contains(ARTIFACT_MANIFEST));
}

#[test]
fn stages_a_milk_v_titan_uefi_bundle_with_nested_esp_paths() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let bundle_dir = temporary.path().join("bundle");
    let output_dir = temporary.path().join("artifact");
    write_valid_titan_bundle(&bundle_dir);

    let expectation = BundleExpectation::new("milk-v-titan", "milk-v-titan", UEFI_ESP_TRANSPORT);
    let bundle = validate_boot_bundle(&bundle_dir, &expectation).expect("validated bundle");
    let artifact = stage_boot_bundle(&bundle, &output_dir).expect("staged bundle");

    assert!(artifact
        .artifact_dir()
        .join(BOOT_PAYLOAD_DIRECTORY)
        .join("EFI/BOOT/BOOTRISCV64.EFI")
        .is_file());
    assert!(artifact
        .artifact_dir()
        .join(BOOT_PAYLOAD_DIRECTORY)
        .join("EFI/AROS/Image")
        .is_file());
    assert_eq!(
        fs::read(
            artifact
                .artifact_dir()
                .join(BOOT_PAYLOAD_DIRECTORY)
                .join("aros.cmd")
        )
        .expect("empty command line"),
        b"\n"
    );
    let manifest = fs::read_to_string(artifact.manifest_path()).expect("artifact manifest");
    assert!(manifest.contains("\"model\": \"milk-v-titan\""));
    assert!(manifest.contains("\"transport\": \"uefi-esp\""));
    assert!(manifest.contains("EFI/BOOT/BOOTRISCV64.EFI"));
}

#[test]
fn reports_all_core_inputs_when_the_external_uboot_bundle_is_absent() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let error = validate_boot_bundle(temporary.path(), &expectation())
        .expect_err("empty directory must not be a boot bundle");
    let message = error.to_string();

    assert!(message.contains(BOOT_BUNDLE_MANIFEST));
    assert!(message.contains("start4.elf"));
    assert!(message.contains("u-boot.bin"));
    assert!(message.contains("boot.scr"));
}

#[test]
fn refuses_a_tampered_external_input_without_creating_an_artifact() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let bundle_dir = temporary.path().join("bundle");
    let output_dir = temporary.path().join("artifact");
    write_valid_bundle(&bundle_dir);
    fs::write(bundle_dir.join("u-boot.bin"), "tampered\n").expect("tamper input");

    let error = validate_boot_bundle(&bundle_dir, &expectation())
        .expect_err("tampered input must fail validation");
    assert!(error.to_string().contains("SHA-256 mismatch"));
    assert!(!output_dir.exists());
}

#[test]
fn refuses_to_replace_an_existing_output_directory() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let bundle_dir = temporary.path().join("bundle");
    let output_dir = temporary.path().join("artifact");
    write_valid_bundle(&bundle_dir);
    fs::create_dir_all(&output_dir).expect("existing output");
    fs::write(output_dir.join("keep.txt"), "do not replace").expect("sentinel");

    let bundle = validate_boot_bundle(&bundle_dir, &expectation()).expect("validated bundle");
    let error = super::stage_boot_bundle(&bundle, &output_dir)
        .expect_err("existing output must not be replaced");
    assert!(error.to_string().contains("Refusing to replace"));
    assert_eq!(
        fs::read_to_string(output_dir.join("keep.txt")).expect("sentinel"),
        "do not replace"
    );
}
