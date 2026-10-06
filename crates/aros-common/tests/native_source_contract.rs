//! Explicit local qualification of the selected, checkout-owned P4 export.
//! Ordinary CI never silently imports a maintainer checkout.

use aros_common::native_build_contract::load_bound_native_build_contract;
use aros_common::TargetProfile;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
#[ignore = "requires the explicitly selected isolated P4 source export"]
fn selected_p4_export_binds_every_actual_source_input() {
    let root = PathBuf::from(
        std::env::var_os("AROS_TEST_P4_SOURCE").expect("set isolated P4 source root"),
    );
    let profiles = TargetProfile::load_from_file(&root.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let relative = Path::new(profile.native_build_contract.as_deref().unwrap());
    let loaded = load_bound_native_build_contract(&root, relative, profile).unwrap();
    // The caller pins the actual selected export, rather than silently
    // reusing an earlier qualification's contract digest.
    let expected = aros_common::Sha256Digest::parse(
        &std::env::var("AROS_TEST_NATIVE_CONTRACT_SHA256")
            .expect("pin the selected isolated source contract bytes"),
    )
    .unwrap();
    let generate = |root: &Path| {
        aros_common::esp_partition::encode_bound_native_partition_table(
            root,
            relative,
            profile,
            &expected,
            0x8000,
            32 << 20,
        )
    };
    let artifact = generate(&root).unwrap();
    let resolve = |root: &Path| {
        aros_common::native_media::resolve_bound_native_media(root, relative, profile, &expected)
    };
    let media = resolve(&root).unwrap();
    assert_eq!(media.partition.artifact, artifact.artifact);
    assert_eq!(media.binding.layout.source_contract_sha256, expected);
    assert_eq!(media.binding.geometry.chip_id, 18);
    assert_eq!(media.binding.geometry.revision_min, 100);
    assert_eq!(media.binding.geometry.revision_max, 199);
    assert_eq!(media.binding.geometry.flash_bytes, 32 << 20);
    assert_eq!(media.binding.geometry.bootloader_offset, 0x2000);
    assert_eq!(media.binding.geometry.partition_table_offset, 0x8000);
    assert_eq!(media.binding.geometry.core_partition_kind, 0);
    assert_eq!(media.binding.geometry.core_partition_subtype, 0x10);
    assert_eq!(media.binding.geometry.package_partition_kind, 0x40);
    assert_eq!(media.binding.geometry.package_partition_subtype, 0);
    assert_eq!(media.binding.geometry.package_limit_bytes, 0x003e_0000);
    assert_eq!(
        media.binding.geometry.development_volume_size_bytes,
        4 << 20
    );
    assert_eq!(loaded.contract.inputs.len(), 76);
    assert!(loaded
        .contract
        .optional_meta_dependencies
        .iter()
        .any(|edge| edge.recipe == "workbench/libs/tiff/mmakefile.src"
            && edge.target == "linklibs-tiff"
            && edge.dependency == "workbench-libs-tiff-pkgconfig"
            && edge.absence
                == aros_common::native_build_contract::NativeMetaAbsence::DisabledOwner));
    let placements = [
        ("bootloader", 0x2000, 0x8000),
        ("partition-table", 0x8000, 0x9000),
        ("core", 0x0002_0000, 0x0082_0000),
        ("bsp", 0x0082_0000, 0x00c0_0000),
        ("developer", 0x00c0_0000, 0x0100_0000),
    ];
    for (slot, (role, start, end)) in media.binding.layout.slots.iter().zip(placements) {
        assert_eq!(slot.role, role);
        assert_eq!(
            slot.range,
            aros_common::flash_plan::FlashRange { start, end }
        );
    }
    assert_eq!(artifact.binding.source_contract_sha256, expected);
    assert_eq!(artifact.binding.contract_inputs, loaded.contract.inputs);
    assert_eq!(
        artifact.artifact.sha256.as_str(),
        "1011273e9c756cbbee22c1ba09794acb3bdc04f088d3a3154e61df0330b91b96"
    );
    assert_eq!(artifact.artifact.bytes.len(), 3072);
    assert_eq!(
        loaded.contract.source_baseline,
        "763a536f26a5e94176c821d48cedbcbff03b4189"
    );
    for required in [
        "arch/riscv-esp32p4/bootloader/project/CMakeLists.txt",
        "arch/riscv-esp32p4/bootloader/project/main/CMakeLists.txt",
        "arch/riscv-esp32p4/bootloader/project/main/main.c",
        "compiler/libinit/mmakefile.src",
        "compiler/libinit/libentry.c",
        "compiler/libinit/kickentry.c",
        "config/features.in",
        "rom/utility/mmakefile.src",
        "rom/aros/mmakefile.src",
        "rom/aros/arosinquirea.c",
    ] {
        assert!(loaded
            .contract
            .inputs
            .iter()
            .any(|input| input.path == required));
    }
    // Every counterprobe changes a private file, never the selected checkout.
    let isolated = tempfile::tempdir().unwrap();
    let contract_copy = isolated.path().join(relative);
    fs::create_dir_all(contract_copy.parent().unwrap()).unwrap();
    fs::copy(&loaded.path, &contract_copy).unwrap();
    for input in &loaded.contract.inputs {
        let destination = isolated.path().join(&input.path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root.join(&input.path), destination).unwrap();
    }
    assert_eq!(
        load_bound_native_build_contract(isolated.path(), relative, profile)
            .unwrap()
            .sha256,
        loaded.sha256
    );
    for input in &loaded.contract.inputs {
        let path = isolated.path().join(&input.path);
        let original = fs::read(&path).unwrap();
        let mut altered = original.clone();
        altered.push(0);
        fs::write(&path, altered).unwrap();
        let error =
            load_bound_native_build_contract(isolated.path(), relative, profile).unwrap_err();
        assert!(
            error.to_string().contains("SHA-256 differs"),
            "{}: {error}",
            input.path
        );
        assert!(
            generate(isolated.path())
                .unwrap_err()
                .contains("SHA-256 differs"),
            "partition generation accepted altered {}",
            input.path
        );
        assert!(resolve(isolated.path())
            .unwrap_err()
            .contains("SHA-256 differs"));
        fs::write(path, original).unwrap();
    }
    assert_eq!(generate(isolated.path()).unwrap(), artifact);
    assert_eq!(resolve(isolated.path()).unwrap(), media);
    if let Some(parent) = std::env::var_os("AROS_TEST_NATIVE_PARTITION_EVIDENCE_PARENT") {
        let evidence = tempfile::Builder::new()
            .prefix("rv3-bound-native-partition.")
            .tempdir_in(parent)
            .unwrap()
            .keep();
        fs::write(
            evidence.join("partition-table.bin"),
            &artifact.artifact.bytes,
        )
        .unwrap();
        fs::write(
            evidence.join("source-binding.json"),
            serde_json::to_vec_pretty(&artifact.binding).unwrap(),
        )
        .unwrap();
        fs::write(
            evidence.join("media-binding.json"),
            serde_json::to_vec_pretty(&media.binding).unwrap(),
        )
        .unwrap();
        eprintln!(
            "Bound native partition proof retained at {}",
            evidence.display()
        );
    }
}
