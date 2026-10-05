//! Explicit qualification against fresh locked external producer outputs.
//! Never discovers a checkout or substitutes a retained Make payload.

use std::io::Read;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, Sha256Digest, TargetProfile};
use aros_verify::esp_image::{verify_esp_image, EspImagePolicy};
use aros_verify::esp_partition::verify_esp_partition_table;

fn explicit_path(variable: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| panic!("set {variable}"), PathBuf::from)
}

fn bounded_read(path: &Path, maximum: u64) -> Vec<u8> {
    let mut file = open_regular_file_nofollow(path).unwrap();
    assert!(file.metadata().unwrap().len() <= maximum);
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() as u64 <= maximum);
    bytes
}

#[test]
#[ignore = "requires explicit P4 source, pinned bootloader SHA-256 and fresh locked IDF bootloader/table outputs"]
fn source_resolved_geometry_verifies_actual_external_bootloader_and_both_tables() {
    let root = explicit_path("AROS_TEST_NATIVE_MEDIA_SOURCE");
    let profiles = TargetProfile::load_from_file(&root.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let media = aros_common::native_media::resolve_bound_native_media(
        &root,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
        &Sha256Digest::parse("201bcf7de54ea000da99e38aa4d13225bd695cee00ebb1745465de0c3fc618eb")
            .unwrap(),
    )
    .unwrap();
    let geometry = &media.binding.geometry;
    let slot = media
        .binding
        .layout
        .slots
        .iter()
        .find(|slot| slot.role == "bootloader")
        .unwrap();
    let bytes = bounded_read(
        &explicit_path("AROS_TEST_NATIVE_MEDIA_BOOTLOADER"),
        slot.range.end - slot.range.start,
    );
    let policy = EspImagePolicy {
        chip_id: geometry.chip_id,
        revision_min: geometry.revision_min,
        revision_max: geometry.revision_max,
        flash_bytes: geometry.flash_bytes,
        maximum_image_bytes: slot.range.end - slot.range.start,
    };
    let image = verify_esp_image(&bytes, policy).unwrap();
    // IDF embeds source-owned compilation timestamps. Bind this exact run's
    // reviewed artifact rather than requiring a different historical image.
    // Never infer the expected digest from the bytes under test.
    let expected_image = Sha256Digest::parse(
        &std::env::var("AROS_TEST_NATIVE_MEDIA_BOOTLOADER_SHA256")
            .expect("set the independently measured bootloader SHA-256"),
    )
    .expect("explicit bootloader pin is a valid SHA-256");
    assert_eq!(image.sha256, expected_image);
    assert_eq!(image.size_bytes, 22544);
    for changed_policy in [
        EspImagePolicy {
            chip_id: policy.chip_id + 1,
            ..policy
        },
        EspImagePolicy {
            revision_min: policy.revision_min + 1,
            ..policy
        },
        EspImagePolicy {
            revision_max: policy.revision_max + 1,
            ..policy
        },
        EspImagePolicy {
            flash_bytes: policy.flash_bytes / 2,
            ..policy
        },
        EspImagePolicy {
            maximum_image_bytes: image.size_bytes as u64 - 1,
            ..policy
        },
    ] {
        assert!(verify_esp_image(&bytes, changed_policy).is_err());
    }
    let vendor_table = bounded_read(&explicit_path("AROS_TEST_NATIVE_MEDIA_VENDOR_TABLE"), 3072);
    assert_eq!(vendor_table, media.partition.artifact.bytes);
    let verify = |bytes: &[u8]| {
        verify_esp_partition_table(
            bytes,
            geometry.partition_table_offset,
            geometry.flash_bytes,
            &media.partition.artifact.source.partitions,
        )
    };
    let table = verify(&media.partition.artifact.bytes).unwrap();
    assert_eq!(verify(&vendor_table).unwrap(), table);
    assert_eq!(
        table.sha256.as_str(),
        "1011273e9c756cbbee22c1ba09794acb3bdc04f088d3a3154e61df0330b91b96"
    );
    for index in [0, 2, 4, 12, 28, 224, 225, 240, 256, 3071] {
        let mut changed = vendor_table.clone();
        changed[index] ^= 1;
        assert!(
            verify(&changed).is_err(),
            "accepted table mutation at {index}"
        );
    }
    // This verifies external containers and source-consistent layout only.
    // There are intentionally no fake native core, BSP or Developer artifacts.
}
