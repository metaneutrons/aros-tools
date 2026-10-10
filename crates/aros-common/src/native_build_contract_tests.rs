use super::*;
use crate::{arch::Architecture, BootstrapAbiProfile, TranspilerProfile};
use serde_json::{json, Value};

const CONTRACT_PATH: &str = "native-build-v1.json";
const INPUT_FILES: &[(&str, &str)] = &[
    ("configure.in", "synthetic configure source\n"),
    ("board/board.mk", "synthetic board rules\n"),
    ("kernel/makefile.src", "synthetic core recipe\n"),
    ("kernel/mmakefile", "synthetic MetaMake recipe\n"),
    ("kernel/linker.lds", "synthetic linker script\n"),
    ("kernel/check.sh", "synthetic residency check\n"),
    ("package/makefile.src", "synthetic package recipe\n"),
    ("boot/partitions.csv", "synthetic partition table\n"),
    (
        "boot/sdkconfig.defaults",
        "synthetic bootloader configuration\n",
    ),
    ("boot/standalone.diff", "synthetic bootloader patch\n"),
];

struct Fixture {
    directory: tempfile::TempDir,
    profile: TargetProfile,
    document: Value,
}

impl Fixture {
    fn root(&self) -> PathBuf {
        self.directory.path().join("source")
    }

    fn write_contract(&self, document: &Value) {
        fs::write(
            self.directory.path().join("source").join(CONTRACT_PATH),
            serde_json::to_vec_pretty(document).unwrap(),
        )
        .unwrap();
    }

    fn load(&self) -> Result<NativeBuildContract> {
        load_native_build_contract(
            &self.directory.path().join("source"),
            Path::new(CONTRACT_PATH),
            &self.profile,
        )
    }
}

#[test]
fn kernel_compiler_role_admits_only_the_target_role() {
    let fixture = new_fixture();
    let mut document = fixture.document.clone();
    document["kernel_compiler_role"] = json!("target");
    fixture.write_contract(&document);
    assert_eq!(
        fixture.load().unwrap().kernel_compiler_role.as_deref(),
        Some("target")
    );
    for role in ["kernel", "host", ""] {
        document["kernel_compiler_role"] = json!(role);
        fixture.write_contract(&document);
        assert!(
            error_text(fixture.load()).contains("kernel_compiler_role"),
            "{role}"
        );
    }
}

fn new_fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("source");
    fs::create_dir_all(&root).unwrap();
    let profile = TargetProfile {
        name: "synthetic-esp32p4-profile".into(),
        arch: Architecture::Riscv32,
        platform: "synthetic-platform".into(),
        bsp: "invented-board".into(),
        features: Vec::new(),
        float_abi: Some("ilp32f".into()),
        bootloader: None,
        transpiler: Some(TranspilerProfile {
            family: String::new(),
            variant: String::new(),
            toolchain: "gnu".into(),
            cpu32: String::new(),
            use_mmu: false,
            mesa_version: None,
        }),
        bootstrap_abi: Some(BootstrapAbiProfile {
            flavour: "standalone".into(),
            platform_smp: false,
        }),
        native_build_contract: None,
        native_consumer_contract: None,
        toolchain_profile: None,
    };
    let inputs = INPUT_FILES
        .iter()
        .map(|(path, contents)| {
            let full_path = root.join(path);
            fs::create_dir_all(full_path.parent().unwrap()).unwrap();
            fs::write(&full_path, contents).unwrap();
            json!({
                "path": path,
                "sha256": crate::sha256_bytes(contents.as_bytes()).to_string(),
            })
        })
        .collect::<Vec<_>>();
    let document = json!({
        "schema_version": 1,
        "profile": profile.name,
        "board": profile.bsp,
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "qualification": "experimental-unqualified",
        "inputs": inputs,
        "abi": {
            "source_cpu": "riscv",
            "target_triple": "riscv-aros",
            "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
            "abi": "ilp32f",
            "code_model": "medany",
            "flavour": "standalone",
            "platform_smp": false,
            "use_mmu": false
        },
        "core": {
            "recipe": "kernel/makefile.src",
            "linker_script": "kernel/linker.lds",
            "resources": ["kernel", "task"],
            "libraries": ["exec", "debug"],
            "devices": ["timer", "flashdisk"],
            "link_libraries": ["exec", "arossupport", "autoinit"],
            "compiler_runtime_role": "libgcc",
            "residency_check": "kernel/check.sh",
            "residency_policy": {
                "algorithm": "riscv32-xip-v1", "section": ".sramtext",
                "flash_start": 1_073_741_824, "flash_end": 1_140_850_688,
                "sram_start": 1_341_128_704, "sram_end": 1_341_652_992
            }
        },
        "package": {
            "recipe": "package/makefile.src",
            "format": "aros-pkg-v1",
            "target": "kernel-package-synthetic-riscv",
            "limit_from_board": "SYNTHETIC_BOARD_PACKAGE_LIMIT"
        },
        "media": {
            "chip": "synthetic-chip",
            "board_rules": "board/board.mk",
            "partition_table": "boot/partitions.csv",
            "core_partition": "core_partition",
            "package_partition": "package_partition",
            "development_volume_offset_from_board": "SYNTHETIC_BOARD_VOLUME_OFFSET",
            "bootloader_configuration": "boot/sdkconfig.defaults",
            "bootloader_patch": "boot/standalone.diff",
            "idf_version": "6.0.1"
        }
    });
    let fixture = Fixture {
        directory,
        profile,
        document,
    };
    fixture.write_contract(&fixture.document);
    fixture
}

fn make_include_fixture() -> Fixture {
    let mut fixture = new_fixture();
    for (path, contents) in [
        ("config/aros.cfg", "legacy configuration\n"),
        ("config/secondary.cfg", "secondary configuration\n"),
        ("arch/native-config.mk", "NATIVE_PROJECTION := 1\n"),
    ] {
        let full_path = fixture.root().join(path);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(&full_path, contents).unwrap();
        fixture.document["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": path,
                "sha256": crate::sha256_bytes(contents.as_bytes()).to_string(),
            }));
    }
    fs::write(fixture.root().join("arch/foreign.mk"), "FOREIGN := 1\n").unwrap();
    fixture.document["make_include_bindings"] = json!({"config/aros.cfg": "arch/native-config.mk"});
    fixture.write_contract(&fixture.document);
    fixture
}

fn error_text(result: Result<NativeBuildContract>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn source_input_inventory_keeps_an_explicit_bounded_limit() {
    let mut fixture = new_fixture();
    let initial_count = fixture.document["inputs"].as_array().unwrap().len();
    for index in initial_count..=MAX_NATIVE_BUILD_INPUTS {
        let path = format!("additional-{index}.src");
        let bytes = format!("source input {index}\n");
        fs::write(fixture.root().join(&path), &bytes).unwrap();
        fixture.document["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": path, "sha256": crate::sha256_bytes(bytes.as_bytes()),
            }));
        if index == MAX_NATIVE_BUILD_INPUTS - 1 {
            fixture.write_contract(&fixture.document);
            assert_eq!(
                fixture.load().unwrap().inputs.len(),
                MAX_NATIVE_BUILD_INPUTS
            );
        }
    }
    fixture.write_contract(&fixture.document);
    assert!(error_text(fixture.load()).contains("between 1 and 128 entries"));
}

#[test]
fn loads_source_facts_for_an_arbitrary_synthetic_board() {
    let fixture = new_fixture();
    let contract = fixture.load().unwrap();

    assert_eq!(contract.profile, "synthetic-esp32p4-profile");
    assert_eq!(contract.board, "invented-board");
    assert_eq!(contract.qualification, NATIVE_BUILD_QUALIFICATION);
    assert_eq!(contract.inputs.len(), INPUT_FILES.len());
    assert_eq!(contract.package.format, NATIVE_PACKAGE_FORMAT);
    assert_eq!(
        contract.core.compiler_runtime_role,
        NATIVE_COMPILER_RUNTIME_ROLE
    );
    assert_eq!(
        contract.media.development_volume_offset_from_board,
        "SYNTHETIC_BOARD_VOLUME_OFFSET"
    );
    let bound = load_bound_native_build_contract(
        &fixture.root(),
        Path::new(CONTRACT_PATH),
        &fixture.profile,
    )
    .unwrap();
    assert_eq!(bound.contract, contract);
    assert_eq!(
        bound.path,
        fixture.root().join(CONTRACT_PATH).canonicalize().unwrap()
    );
    assert_eq!(
        bound.sha256,
        crate::sha256_file(&bound.path).unwrap().digest
    );
}

#[test]
fn native_partition_generation_binds_contract_profile_and_every_input() {
    let fixture = new_fixture();
    let csv = b"storage,0x40,0,0x9000,4K,\n";
    let path = fixture.root().join("boot/partitions.csv");
    fs::write(&path, csv).unwrap();
    let mut document = fixture.document.clone();
    for input in document["inputs"].as_array_mut().unwrap() {
        if input["path"] == "boot/partitions.csv" {
            input["sha256"] = json!(crate::sha256_bytes(csv));
        }
    }
    fixture.write_contract(&document);
    let contract_hash = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
        .unwrap()
        .digest;
    let generate = |profile: &TargetProfile, expected: &Sha256Digest| {
        crate::esp_partition::encode_bound_native_partition_table(
            &fixture.root(),
            Path::new(CONTRACT_PATH),
            profile,
            expected,
            0x8000,
            1 << 20,
        )
    };
    let generated = generate(&fixture.profile, &contract_hash).unwrap();
    assert_eq!(generated.binding.board, "invented-board");
    assert_eq!(generated.binding.source_contract_sha256, contract_hash);
    assert_eq!(generated.binding.contract_inputs.len(), INPUT_FILES.len());
    assert_eq!(
        generated.binding.partition_source_sha256,
        crate::sha256_bytes(csv)
    );
    assert_eq!(
        generated.binding.artifact_sha256,
        crate::sha256_bytes(&generated.artifact.bytes)
    );
    assert_eq!(generated.binding.artifact_size, 3072);
    assert_eq!(
        generated.binding.geometry_origin,
        "explicit-caller-input-unqualified"
    );
    assert_eq!(
        generated.binding.qualification,
        "experimental-source-consistency-only"
    );
    assert!(
        generate(&fixture.profile, &crate::sha256_bytes(b"another contract"))
            .unwrap_err()
            .contains("contract SHA-256")
    );
    let mut wrong_profile = fixture.profile.clone();
    wrong_profile.bsp = "other-board".into();
    assert!(generate(&wrong_profile, &contract_hash).is_err());
    for (relative, _) in INPUT_FILES {
        let path = fixture.root().join(relative);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(
            generate(&fixture.profile, &contract_hash)
                .unwrap_err()
                .contains("SHA-256 differs"),
            "accepted altered {relative}"
        );
        fs::write(path, original).unwrap();
    }
    assert_eq!(
        generate(&fixture.profile, &contract_hash).unwrap(),
        generated
    );
}

fn media_fixture() -> (Fixture, Value) {
    let mut fixture = new_fixture();
    let csv = b"core_partition,app,ota_0,0x10000,0x20000,\npkg_partition,0x47,3,0x30000,0x20000,\n";
    fs::write(fixture.root().join("boot/partitions.csv"), csv).unwrap();
    for input in fixture.document["inputs"].as_array_mut().unwrap() {
        if input["path"] == "boot/partitions.csv" {
            input["sha256"] = json!(crate::sha256_bytes(csv));
        }
    }
    fixture.document["media"]["geometry_contract"] = json!("boot/geometry.json");
    fixture.document["media"]["package_partition"] = json!("pkg_partition");
    let geometry = json!({
        "schema_version": 1, "format": "esp-unsigned-flash-v1",
        "profile": fixture.profile.name, "board": fixture.profile.bsp,
        "chip": "synthetic-chip", "chip_id": 21,
        "revision_min": 50, "revision_max": 60, "idf_version": "6.0.1",
        "flash_bytes": 1 << 20, "erase_sector_bytes": 0x1000,
        "bootloader_offset": 0x2000, "partition_table_offset": 0x8000,
        "core_partition_kind": 0, "core_partition_subtype": 0x10,
        "package_partition_kind": 0x47, "package_partition_subtype": 3,
        "package_limit_bytes": 0x10000,
        "development_volume_offset": 0x40000,
        "development_volume_size_bytes": 0x10000
    });
    fixture.document["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": "boot/geometry.json", "sha256": crate::sha256_bytes(b"placeholder")
        }));
    write_media_geometry(&fixture, &geometry);
    (fixture, geometry)
}

fn write_media_geometry(fixture: &Fixture, geometry: &Value) {
    let bytes = serde_json::to_vec_pretty(geometry).unwrap();
    fs::write(fixture.root().join("boot/geometry.json"), &bytes).unwrap();
    let mut document = fixture.document.clone();
    for input in document["inputs"].as_array_mut().unwrap() {
        if input["path"] == "boot/geometry.json" {
            input["sha256"] = json!(crate::sha256_bytes(&bytes));
        }
    }
    fixture.write_contract(&document);
}

fn resolve_media(
    fixture: &Fixture,
) -> std::result::Result<crate::native_media::BoundNativeMedia, String> {
    let expected = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
        .unwrap()
        .digest;
    crate::native_media::resolve_bound_native_media(
        &fixture.root(),
        Path::new(CONTRACT_PATH),
        &fixture.profile,
        &expected,
    )
}

#[test]
fn native_media_geometry_uses_only_source_numbers_and_revalidates_every_input() {
    let (fixture, _) = media_fixture();
    let resolved = resolve_media(&fixture).unwrap();
    let loaded = load_bound_native_build_contract(
        &fixture.root(),
        Path::new(CONTRACT_PATH),
        &fixture.profile,
    )
    .unwrap();
    assert_eq!(
        resolved.binding.layout.source_contract_sha256,
        loaded.sha256
    );
    assert_eq!(resolved.binding.geometry.chip_id, 21);
    assert_eq!(resolved.binding.geometry.flash_bytes, 1 << 20);
    assert_eq!(resolved.binding.geometry.revision_min, 50);
    assert_eq!(resolved.binding.geometry.package_partition_kind, 0x47);
    assert_eq!(resolved.binding.geometry.package_partition_subtype, 3);
    assert_eq!(
        resolved.binding.partition_binding.geometry_origin,
        "source-owned-media-geometry"
    );
    assert_eq!(resolved.binding.layout.slots.len(), 5);
    let expected = [
        ("bootloader", 0x2000, 0x8000, false),
        ("partition-table", 0x8000, 0x9000, false),
        ("core", 0x10000, 0x30000, true),
        ("bsp", 0x30000, 0x40000, true),
        ("developer", 0x40000, 0x50000, true),
    ];
    for (slot, (role, start, end, native)) in resolved.binding.layout.slots.iter().zip(expected) {
        assert_eq!(slot.role, role);
        assert_eq!(slot.range, crate::flash_plan::FlashRange { start, end });
        assert_eq!(slot.native_build, native);
    }
    for input in loaded.contract.inputs {
        let path = fixture.root().join(&input.path);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(
            resolve_media(&fixture)
                .unwrap_err()
                .contains("SHA-256 differs"),
            "accepted altered {}",
            input.path
        );
        fs::write(path, original).unwrap();
    }
    assert_eq!(resolve_media(&fixture).unwrap(), resolved);
    assert!(crate::native_media::resolve_bound_native_media(
        &fixture.root(),
        Path::new(CONTRACT_PATH),
        &fixture.profile,
        &crate::sha256_bytes(b"different contract"),
    )
    .unwrap_err()
    .contains("contract SHA-256 differs"));
}

#[test]
fn native_media_geometry_refuses_semantic_mutations_even_with_matching_input_hashes() {
    let (fixture, geometry) = media_fixture();
    for (field, bad_value) in [
        ("schema_version", json!(2)),
        ("format", json!("run-command")),
        ("profile", json!("other-profile")),
        ("board", json!("other-board")),
        ("chip", json!("other-chip")),
        ("idf_version", json!("0.0.0")),
        ("flash_bytes", json!(0)),
        ("flash_bytes", json!(3 << 20)),
        ("flash_bytes", json!(1_u64 << 32)),
        ("erase_sector_bytes", json!(0x800)),
        ("revision_max", json!(49)),
        ("bootloader_offset", json!(0x8000)),
        ("bootloader_offset", json!(0x2001)),
        ("partition_table_offset", json!(0x8001)),
        ("partition_table_offset", json!(0x10000)),
        ("core_partition_kind", json!(1)),
        ("core_partition_subtype", json!(0x11)),
        ("package_partition_kind", json!(0x40)),
        ("package_partition_kind", json!(0xff)),
        ("package_partition_subtype", json!(0)),
        ("package_partition_subtype", json!(256)),
        ("package_limit_bytes", json!(0)),
        ("package_limit_bytes", json!(0x10001)),
        ("package_limit_bytes", json!(u64::MAX)),
        ("development_volume_offset", json!(0x30000)),
        ("development_volume_size_bytes", json!(0)),
        ("development_volume_size_bytes", json!(0x10001)),
        ("development_volume_size_bytes", json!(u64::MAX)),
        ("command", json!("false")),
    ] {
        let mut altered = geometry.clone();
        altered[field] = bad_value;
        write_media_geometry(&fixture, &altered);
        assert!(
            resolve_media(&fixture).is_err(),
            "accepted {field}: {altered}"
        );
    }
    write_media_geometry(&fixture, &geometry);
    assert!(resolve_media(&fixture).is_ok());
}

#[test]
fn native_media_requires_explicit_inventoried_geometry_and_matching_partition_roles() {
    let fixture = new_fixture();
    assert!(fixture.load().is_ok());
    assert!(resolve_media(&fixture)
        .unwrap_err()
        .contains("no explicit media geometry"));

    let (fixture, _) = media_fixture();
    let original_document: Value =
        serde_json::from_slice(&fs::read(fixture.root().join(CONTRACT_PATH)).unwrap()).unwrap();
    for wrong_path in ["boot/missing.json", "../geometry.json"] {
        let mut document = original_document.clone();
        document["media"]["geometry_contract"] = json!(wrong_path);
        fixture.write_contract(&document);
        assert!(resolve_media(&fixture).is_err());
    }
    for invalid_type in [Value::Null, json!(true), json!([]), json!({})] {
        let mut document = original_document.clone();
        document["media"]["geometry_contract"] = invalid_type;
        fixture.write_contract(&document);
        assert!(fixture.load().is_err());
    }
    for (field, value) in [
        ("core_partition", "missing"),
        ("core_partition", "pkg_partition"),
        ("package_partition", "core_partition"),
    ] {
        let mut document = original_document.clone();
        document["media"][field] = json!(value);
        fixture.write_contract(&document);
        assert!(resolve_media(&fixture).is_err(), "accepted {field}={value}");
    }
    for flags in ["encrypted", "readonly", "encrypted:readonly"] {
        let csv = format!(
            "core_partition,app,ota_0,0x10000,0x20000,{flags}\npkg_partition,0x47,3,0x30000,0x20000,\n"
        );
        fs::write(fixture.root().join("boot/partitions.csv"), &csv).unwrap();
        let mut document = original_document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/partitions.csv" {
                input["sha256"] = json!(crate::sha256_bytes(csv.as_bytes()));
            }
        }
        fixture.write_contract(&document);
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("unsupported flags"));
    }
}

#[cfg(unix)]
#[test]
fn native_media_geometry_refuses_a_symlink_even_when_it_matches_the_inventory() {
    let (fixture, _) = media_fixture();
    let path = fixture.root().join("boot/geometry.json");
    fs::rename(&path, fixture.root().join("boot/original.json")).unwrap();
    std::os::unix::fs::symlink("original.json", &path).unwrap();
    assert!(resolve_media(&fixture)
        .unwrap_err()
        .contains("cannot open media geometry"));
}

#[test]
fn native_media_refuses_reinventoried_partition_kind_and_subtype_drift() {
    let (fixture, geometry) = media_fixture();
    let original_csv = fs::read(fixture.root().join("boot/partitions.csv")).unwrap();
    let original_document: Value =
        serde_json::from_slice(&fs::read(fixture.root().join(CONTRACT_PATH)).unwrap()).unwrap();
    for (core_kind, core_subtype, package_kind, package_subtype) in [
        ("data", "0x10", "0x47", "3"),
        ("app", "ota_1", "0x47", "3"),
        ("app", "ota_0", "0x48", "3"),
        ("app", "ota_0", "0x47", "4"),
    ] {
        let csv = format!(
            "core_partition,{core_kind},{core_subtype},0x10000,0x20000,\npkg_partition,{package_kind},{package_subtype},0x30000,0x20000,\n"
        );
        fs::write(fixture.root().join("boot/partitions.csv"), &csv).unwrap();
        let mut document = original_document.clone();
        for input in document["inputs"].as_array_mut().unwrap() {
            if input["path"] == "boot/partitions.csv" {
                input["sha256"] = json!(crate::sha256_bytes(csv.as_bytes()));
            }
        }
        fixture.write_contract(&document);
        assert!(fixture.load().is_ok());
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("partition formats differ"));
    }
    fs::write(fixture.root().join("boot/partitions.csv"), &original_csv).unwrap();
    // A field omitted or explicitly null is not a board-specific default.
    for field in [
        "core_partition_kind",
        "core_partition_subtype",
        "package_partition_kind",
        "package_partition_subtype",
    ] {
        let mut absent = geometry.clone();
        absent.as_object_mut().unwrap().remove(field);
        write_media_geometry(&fixture, &absent);
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("invalid media geometry"));
        absent[field] = Value::Null;
        write_media_geometry(&fixture, &absent);
        assert!(resolve_media(&fixture)
            .unwrap_err()
            .contains("invalid media geometry"));
    }
}

#[cfg(unix)]
#[test]
fn native_partition_generation_refuses_an_in_tree_csv_symlink() {
    use std::os::unix::fs::symlink;
    let fixture = new_fixture();
    let csv = b"storage,0x40,0,0x9000,4K,\n";
    let path = fixture.root().join("boot/partitions.csv");
    fs::write(&path, csv).unwrap();
    let mut document = fixture.document.clone();
    for input in document["inputs"].as_array_mut().unwrap() {
        if input["path"] == "boot/partitions.csv" {
            input["sha256"] = json!(crate::sha256_bytes(csv));
        }
    }
    fixture.write_contract(&document);
    let contract_hash = crate::sha256_file(&fixture.root().join(CONTRACT_PATH))
        .unwrap()
        .digest;
    let original = fixture.root().join("boot/original.csv");
    fs::rename(&path, &original).unwrap();
    symlink("original.csv", &path).unwrap();
    assert!(crate::esp_partition::encode_bound_native_partition_table(
        &fixture.root(),
        Path::new(CONTRACT_PATH),
        &fixture.profile,
        &contract_hash,
        0x8000,
        1 << 20,
    )
    .is_err());
    assert_eq!(fs::read(original).unwrap(), csv);
}

#[test]
fn residency_policy_rejects_unknown_empty_overlapping_and_non_rv32_ranges() {
    for (field, value) in [
        ("algorithm", json!("run-source-script")),
        ("section", json!(".text")),
        ("flash_end", json!(1_073_741_824)),
        ("flash_end", json!(4_294_967_297_u64)),
        ("sram_start", json!(1_073_741_824)),
        ("flash_start", json!(-1)),
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["core"]["residency_policy"][field] = value;
        fixture.write_contract(&document);
        assert!(fixture.load().is_err(), "{field}");
    }
    let fixture = new_fixture();
    let mut document = fixture.document.clone();
    document["core"]["residency_policy"]["command"] = json!("evil");
    fixture.write_contract(&document);
    assert!(error_text(fixture.load()).contains("unknown field"));
}

#[test]
fn binds_exact_native_selectors_to_the_verified_compiler_not_just_width() {
    let fixture = new_fixture();
    let contract = fixture.load().unwrap();
    let compiler: crate::ArosCompilerIdentity = serde_json::from_value(json!({
        "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
        "target": {
            "schema": "aros-riscv-target-v1", "isa": contract.abi.isa,
            "abi": "ilp32f", "code_model": "medany",
            "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
            "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
        }
    }))
    .unwrap();
    validate_native_build_compiler(&contract, &compiler, "riscv-aros").unwrap();
    for field in ["isa", "abi", "code_model"] {
        let mut changed = serde_json::to_value(&contract).unwrap();
        changed["abi"][field] = match field {
            "isa" => json!("rv32imafc"),
            "abi" => json!("ilp32"),
            _ => json!("medlow"),
        };
        let changed = serde_json::from_value(changed).unwrap();
        assert!(validate_native_build_compiler(&changed, &compiler, "riscv-aros").is_err());
    }
    let llvm = crate::ArosCompilerIdentity::Llvm {
        version: "11.0.1".into(),
    };
    assert!(validate_native_build_compiler(&contract, &llvm, "riscv-aros").is_err());
    assert!(validate_native_build_compiler(&contract, &compiler, "riscv64-aros").is_err());
}

#[test]
fn rejects_modified_or_missing_inventoried_inputs() {
    let fixture = new_fixture();
    fs::write(
        fixture.root().join("kernel/linker.lds"),
        "mutated linker script\n",
    )
    .unwrap();
    assert!(error_text(fixture.load()).contains("SHA-256 differs"));

    let fixture = new_fixture();
    fs::remove_file(fixture.root().join("kernel/linker.lds")).unwrap();
    assert!(error_text(fixture.load()).contains("kernel/linker.lds"));
}

#[test]
fn rejects_absolute_parent_and_command_like_input_paths() {
    for path in ["/outside", "../outside", "kernel/evil;make", "C:/outside"] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["inputs"][0]["path"] = json!(path);
        fixture.write_contract(&document);
        let message = error_text(fixture.load());
        assert!(message.contains("unsafe"), "{path}: {message}");
    }
}

#[test]
fn rejects_contracts_larger_than_one_mebibyte_before_parsing() {
    let fixture = new_fixture();
    fs::write(
        fixture.root().join(CONTRACT_PATH),
        vec![b' '; usize::try_from(MAX_NATIVE_BUILD_CONTRACT_BYTES).unwrap() + 1],
    )
    .unwrap();

    assert!(error_text(fixture.load()).contains("1048576-byte size limit"));
}

#[cfg(unix)]
#[test]
fn rejects_input_symlink_that_escapes_source_root() {
    use std::os::unix::fs::symlink;

    let fixture = new_fixture();
    let outside = fixture.directory.path().join("outside.txt");
    fs::write(&outside, "outside source\n").unwrap();
    symlink(&outside, fixture.root().join("escape.txt")).unwrap();
    let mut document = fixture.document.clone();
    document["inputs"][0]["path"] = json!("escape.txt");
    fixture.write_contract(&document);

    let message = error_text(fixture.load());
    assert!(message.contains("escapes canonical scan root"));
}

#[test]
fn rejects_duplicate_input_hash_entries_and_unknown_fields() {
    let fixture = new_fixture();
    let mut duplicate = fixture.document.clone();
    let first_input = duplicate["inputs"][0].clone();
    duplicate["inputs"]
        .as_array_mut()
        .unwrap()
        .push(first_input);
    fixture.write_contract(&duplicate);
    assert!(error_text(fixture.load()).contains("duplicates another declared path"));

    for (location, field) in [
        ("root", "unexpected"),
        ("abi", "command"),
        ("core", "command"),
        ("package", "command"),
        ("media", "command"),
        ("input", "command"),
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        if location == "root" {
            document[field] = json!("echo unsafe");
        } else if location == "input" {
            document["inputs"][0][field] = json!("echo unsafe");
        } else {
            document[location][field] = json!("echo unsafe");
        }
        fixture.write_contract(&document);
        assert!(error_text(fixture.load()).contains("unknown field"));
    }
}

#[test]
fn rejects_incomplete_or_mismatched_profile_abi() {
    let fixture = new_fixture();
    let mut profile = fixture.profile.clone();
    profile.transpiler = None;
    assert!(error_text(load_native_build_contract(
        &fixture.directory.path().join("source"),
        Path::new(CONTRACT_PATH),
        &profile,
    ))
    .contains("no complete transpiler"));

    let mut profile = fixture.profile.clone();
    profile.bootstrap_abi = None;
    assert!(error_text(load_native_build_contract(
        &fixture.directory.path().join("source"),
        Path::new(CONTRACT_PATH),
        &profile,
    ))
    .contains("no complete bootstrap ABI"));

    let mutations: [fn(&mut Value); 10] = [
        |document: &mut Value| document["profile"] = json!("other-profile"),
        |document: &mut Value| document["board"] = json!("other-board"),
        |document: &mut Value| document["abi"]["source_cpu"] = json!("riscv32"),
        |document: &mut Value| document["abi"]["target_triple"] = json!("riscv64-aros"),
        |document: &mut Value| document["abi"]["isa"] = json!("rv64gc"),
        |document: &mut Value| document["abi"]["flavour"] = json!("native"),
        |document: &mut Value| document["abi"]["platform_smp"] = json!(true),
        |document: &mut Value| document["abi"]["use_mmu"] = json!(true),
        |document: &mut Value| document["abi"]["abi"] = json!("ilp32d"),
        |document: &mut Value| document["abi"]["code_model"] = json!("unknown"),
    ];
    for mutation in mutations {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        mutation(&mut document);
        fixture.write_contract(&document);
        assert!(matches!(
            fixture.load(),
            Err(ArosError::Configuration { .. })
        ));
    }

    let fixture = new_fixture();
    let mut profile = fixture.profile.clone();
    profile.transpiler.as_mut().unwrap().toolchain = "llvm".into();
    let message = error_text(load_native_build_contract(
        &fixture.directory.path().join("source"),
        Path::new(CONTRACT_PATH),
        &profile,
    ));
    assert!(message.contains("requires the GNU transpiler"));
}

#[test]
fn rejects_referenced_source_paths_outside_the_measured_inventory() {
    let fixture = new_fixture();
    fs::write(
        fixture.root().join("kernel/uninventoried.lds"),
        "unlocked\n",
    )
    .unwrap();
    let mut document = fixture.document.clone();
    document["core"]["linker_script"] = json!("kernel/uninventoried.lds");
    fixture.write_contract(&document);

    let message = error_text(fixture.load());
    assert!(message.contains("core.linker_script"));
    assert!(message.contains("not declared in inputs"));
}

#[test]
fn rejects_unsafe_or_duplicate_core_code_tokens() {
    for values in [json!(["kernel", "kernel"]), json!(["kernel;make"])] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["core"]["resources"] = values;
        fixture.write_contract(&document);
        assert!(error_text(fixture.load()).contains("core.resources"));
    }
}

#[test]
fn source_make_configuration_preserves_known_empty_and_literal_values() {
    let fixture = new_fixture();
    let mut document = fixture.document.clone();
    document["make_variables"] = json!({"FEATURE_MODE": "", "SDK_STYLE": "native-v1"});
    fixture.write_contract(&document);
    let contract = fixture.load().unwrap();
    assert_eq!(contract.make_variables.get("FEATURE_MODE").unwrap(), "");
    assert_eq!(
        contract.make_variables.get("SDK_STYLE").unwrap(),
        "native-v1"
    );
    assert!(!contract.make_variables.contains_key("UNDECLARED"));
}

#[test]
fn source_host_configuration_is_selected_without_guessing_missing_hosts() {
    let fixture = new_fixture();
    let mut document = fixture.document.clone();
    document["make_variables"] = json!({"FEATURE_MODE": ""});
    document["host_make_variables"] = json!({
        "linux-x86_64": {"HOST_STYLE": "elf"},
        "macos-aarch64": {"HOST_STYLE": "macho"}
    });
    fixture.write_contract(&document);
    let contract = fixture.load().unwrap();
    for (host, expected) in [("linux-x86_64", "elf"), ("macos-aarch64", "macho")] {
        let variables = contract.make_variables_for_host(host).unwrap();
        assert_eq!(variables["HOST_STYLE"], expected);
        assert_eq!(variables["FEATURE_MODE"], "");
        assert!(!variables.contains_key("UNDECLARED"));
    }
    assert!(contract.make_variables_for_host("linux-aarch64").is_err());
    assert!(contract.make_variables_for_host("").is_err());
    assert!(!fixture
        .load()
        .unwrap()
        .make_variables
        .contains_key("HOST_STYLE"));
}

#[test]
fn source_host_configuration_rejects_unsafe_values_and_ambiguous_defaults() {
    for table in [
        json!({"macos-aarch64": {"HOST_STYLE": "$(eval injected: owner)"}}),
        json!({"macos-aarch64": {"CPU": "injected"}}),
        json!({"macos-aarch64": {"HOST_STYLE": "two words"}}),
        json!({"macos-aarch64": {"SHARED": "collision"}}),
        json!({"bad/host": {"HOST_STYLE": "elf"}}),
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["make_variables"] = json!({"SHARED": "known"});
        document["host_make_variables"] = table;
        fixture.write_contract(&document);
        assert!(fixture.load().is_err(), "{document}");
    }
    let fixture = new_fixture();
    assert!(fixture.load().unwrap().make_variables_for_host("").is_ok());
}

#[test]
fn source_host_identity_cannot_be_a_shared_default() {
    for name in NATIVE_HOST_MAKE_IDENTITIES {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["make_variables"] = json!({name.to_string(): "linux"});
        fixture.write_contract(&document);
        assert!(fixture.load().is_err(), "{name}");

        let mut unvalidated =
            serde_json::from_value::<NativeBuildContract>(document.clone()).unwrap();
        assert!(unvalidated
            .make_variables_for_host("macos-aarch64")
            .is_err());
        unvalidated.make_variables.clear();
        unvalidated.host_make_variables.insert(
            "macos-aarch64".into(),
            BTreeMap::from([(name.to_string(), "darwin".into())]),
        );
        assert_eq!(
            unvalidated
                .make_variables_for_host("macos-aarch64")
                .unwrap()[*name],
            "darwin"
        );

        document["make_variables"] = json!({});
        document["host_make_variables"] = json!({"macos-aarch64": {name.to_string(): "darwin"}});
        fixture.write_contract(&document);
        assert_eq!(
            fixture
                .load()
                .unwrap()
                .make_variables_for_host("macos-aarch64")
                .unwrap()[*name],
            "darwin"
        );
    }
}

#[test]
fn source_host_configuration_rejects_duplicate_and_unbounded_maps() {
    for text in [
        r#"{"macos-aarch64":{"FLAG":"0"},"macos-aarch64":{"FLAG":"1"}}"#,
        r#"{"macos-aarch64":{"FLAG":"0","FLAG":"1"}}"#,
    ] {
        assert!(
            deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(text)).is_err()
        );
    }
    let hosts = (0..17)
        .map(|index| (format!("host-{index}"), json!({})))
        .collect::<BTreeMap<_, _>>();
    let text = serde_json::to_string(&hosts).unwrap();
    assert!(
        deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(&text)).is_err()
    );
    let variables = (0..=MAX_MAKE_VARIABLES)
        .map(|index| (format!("FLAG_{index}"), "0"))
        .collect::<BTreeMap<_, _>>();
    let text = serde_json::to_string(&json!({"macos-aarch64": variables})).unwrap();
    assert!(
        deserialize_host_make_variables(&mut serde_json::Deserializer::from_str(&text)).is_err()
    );
}

#[test]
fn source_make_configuration_rejects_syntax_shadowing_and_unbounded_data() {
    for value in [
        json!({"CPU": "riscv"}),
        json!({"AROS_TOOLCHAIN": "gnu"}),
        json!({"lowercase": "1"}),
        json!({"1BAD": "1"}),
        json!({"FEATURE": "$(shell touch bad)"}),
        json!({"FEATURE": "a;b"}),
        json!({"FEATURE": "a b"}),
        json!({"FEATURE": "a\nb"}),
        json!({"FEATURE": 0}),
        json!({"FEATURE": "a".repeat(129)}),
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["make_variables"] = value;
        fixture.write_contract(&document);
        assert!(fixture.load().is_err(), "{document}");
    }
    let duplicate = r#"{"FLAG":"0","FLAG":"1"}"#;
    let mut deserializer = serde_json::Deserializer::from_str(duplicate);
    assert!(deserialize_make_variables(&mut deserializer)
        .unwrap_err()
        .to_string()
        .contains("duplicate Make variable"));
    let too_many: BTreeMap<_, _> = (0..=MAX_MAKE_VARIABLES)
        .map(|index| (format!("FLAG_{index}"), "0"))
        .collect();
    let text = serde_json::to_string(&too_many).unwrap();
    assert!(deserialize_make_variables(&mut serde_json::Deserializer::from_str(&text)).is_err());
}

#[test]
fn source_make_include_binding_maps_inventoried_source_to_inventoried_projection() {
    let fixture = make_include_fixture();
    let contract = fixture.load().unwrap();
    assert_eq!(
        contract
            .make_include_bindings
            .get("config/aros.cfg")
            .unwrap(),
        "arch/native-config.mk"
    );

    let mut empty = fixture.document.clone();
    empty["make_include_bindings"] = json!({});
    fixture.write_contract(&empty);
    assert!(fixture.load().unwrap().make_include_bindings.is_empty());

    let mut original = fixture.document.clone();
    original["make_include_bindings"] = json!({"arch/native-config.mk": "arch/native-config.mk"});
    fixture.write_contract(&original);
    assert_eq!(
        fixture.load().unwrap().make_include_bindings["arch/native-config.mk"],
        "arch/native-config.mk"
    );
}

#[test]
fn generated_make_templates_require_sealed_sources_and_no_competing_binding() {
    let mut fixture = make_include_fixture();
    for (path, contents) in [
        (
            "configure.ac",
            "AC_CONFIG_FILES([gen/include.cfg:config/include.cfg.in])\n",
        ),
        (
            "config/include.cfg.in",
            "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
        ),
    ] {
        fs::write(fixture.root().join(path), contents).unwrap();
        fixture.document["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "path": path, "sha256": crate::sha256_bytes(contents.as_bytes()),
            }));
    }
    fixture.document["generated_make_templates"] = json!({
        "gen/include.cfg": {
            "template": "config/include.cfg.in",
            "configure_source": "configure.ac",
            "substitutions": {"@ENABLE_EXECSMP@": ""}
        }
    });
    fixture.write_contract(&fixture.document);
    assert!(!fixture.root().join("gen/include.cfg").exists());
    assert_eq!(fixture.load().unwrap().generated_make_templates.len(), 1);
    let valid = fixture.document.clone();
    fixture.document["make_include_bindings"]["GEN/include.cfg"] = json!("arch/native-config.mk");
    fixture.write_contract(&fixture.document);
    assert!(fixture.load().is_err());
    fixture.document = valid;
    fixture.document["inputs"]
        .as_array_mut()
        .unwrap()
        .retain(|input| input["path"] != "configure.ac");
    fixture.write_contract(&fixture.document);
    assert!(fixture.load().is_err());
}

#[test]
fn source_make_include_bindings_reject_uninventoried_paths_and_unsafe_mappings() {
    let fixture = make_include_fixture();

    for (path, is_replacement) in [("config/aros.cfg", false), ("arch/native-config.mk", true)] {
        let mut document = fixture.document.clone();
        document["inputs"]
            .as_array_mut()
            .unwrap()
            .retain(|input| input["path"] != path);
        fixture.write_contract(&document);
        let error = error_text(fixture.load());
        let expected = if is_replacement {
            "make_include_bindings replacement"
        } else {
            "make_include_bindings key"
        };
        assert!(error.contains(expected), "{path}: {error}");
        assert!(error.contains("not declared in inputs"), "{path}: {error}");
    }

    for (replacement, expected) in [
        ("../outside.mk", "unsafe"),
        ("arch/*.mk", "unsafe"),
        ("arch/foreign.mk", "not declared in inputs"),
        ("kernel/linker.lds", "must name a .mk file"),
    ] {
        let mut document = fixture.document.clone();
        document["make_include_bindings"] = json!({"config/aros.cfg": replacement});
        fixture.write_contract(&document);
        let error = error_text(fixture.load());
        assert!(error.contains(expected), "{replacement}: {error}");
    }

    let mut duplicate_replacement = fixture.document.clone();
    duplicate_replacement["make_include_bindings"] = json!({
        "config/aros.cfg": "arch/native-config.mk",
        "config/secondary.cfg": "arch/native-config.mk"
    });
    fixture.write_contract(&duplicate_replacement);
    assert!(error_text(fixture.load()).contains("duplicate replacement"));

    let mut null_bindings = fixture.document.clone();
    null_bindings["make_include_bindings"] = Value::Null;
    fixture.write_contract(&null_bindings);
    assert!(fixture.load().is_err());

    let mut null_replacement = fixture.document.clone();
    null_replacement["make_include_bindings"] = json!({"config/aros.cfg": Value::Null});
    fixture.write_contract(&null_replacement);
    assert!(fixture.load().is_err());

    let duplicate = r#"{"config/aros.cfg":"arch/first.mk","config/aros.cfg":"arch/second.mk"}"#;
    let mut deserializer = serde_json::Deserializer::from_str(duplicate);
    assert!(deserialize_make_include_bindings(&mut deserializer)
        .unwrap_err()
        .to_string()
        .contains("duplicate Make include binding"));

    let too_many: BTreeMap<_, _> = (0..=MAX_NATIVE_MAKE_INCLUDE_BINDINGS)
        .map(|index| {
            (
                format!("config/include-{index}.cfg"),
                format!("arch/projection-{index}.mk"),
            )
        })
        .collect();
    let text = serde_json::to_string(&too_many).unwrap();
    assert!(
        deserialize_make_include_bindings(&mut serde_json::Deserializer::from_str(&text)).is_err()
    );
}

#[test]
fn source_optional_meta_dependencies_load_only_selector_edges_on_recipes() {
    let fixture = new_fixture();
    assert!(fixture
        .load()
        .unwrap()
        .optional_meta_dependencies
        .is_empty());

    let mut document = fixture.document.clone();
    document["optional_meta_dependencies"] = json!([
        {
            "recipe": "kernel/makefile.src",
            "target": "optional-meta+native",
            "dependency": "lib${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}"
        },
        {
            "recipe": "kernel/mmakefile",
            "target": "optional-meta-family",
            "dependency": "lib${AROS_TARGET_LEGACY_PLATFORM}_${AROS_TARGET_FAMILY}_${AROS_TARGET_VARIANT}_${AROS_TARGET_CPU32}"
        }
    ]);
    fixture.write_contract(&document);

    let loaded = fixture.load().unwrap();
    assert_eq!(loaded.optional_meta_dependencies.len(), 2);
    assert_eq!(
        loaded.optional_meta_dependencies[0],
        NativeOptionalMetaDependency {
            recipe: "kernel/makefile.src".into(),
            target: "optional-meta+native".into(),
            dependency: "lib${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}".into(),
            absence: NativeMetaAbsence::Selector,
        }
    );
}

#[test]
fn source_optional_meta_dependencies_reject_unsafe_or_unbounded_edges() {
    for dependency in [
        "liboptional",
        "${AROS_TARGET_VENDOR}",
        "${AROS_TARGET_CPU:foo}",
        "${AROS_TARGET_CPU}/include",
        "$(shell touch file)",
        "${AROS_TARGET_CPU}${OTHER}",
        "${AROS_TARGET_CPU",
        "${AROS_TARGET_CPU}?",
        "$${AROS_TARGET_CPU}",
        "${AROS_TARGET_CPU}|${AROS_TARGET_PLATFORM}",
        "${AROS_TARGET_CPU}\\other",
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["optional_meta_dependencies"] = json!([{
            "recipe": "kernel/makefile.src",
            "target": "optional-meta",
            "dependency": dependency
        }]);
        fixture.write_contract(&document);
        let error = error_text(fixture.load());
        assert!(error.contains("dependency"), "{dependency}: {error}");
    }

    for target in ["", "target/name", "target name", "target${AROS_TARGET_CPU}"] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["optional_meta_dependencies"] = json!([{
            "recipe": "kernel/makefile.src",
            "target": target,
            "dependency": "lib${AROS_TARGET_CPU}"
        }]);
        fixture.write_contract(&document);
        let error = error_text(fixture.load());
        assert!(error.contains("target"), "{target}: {error}");
    }

    for recipe in [
        "../outside.src",
        "kernel/not-inventoried.src",
        "kernel/linker.lds",
    ] {
        let fixture = new_fixture();
        let mut document = fixture.document.clone();
        document["optional_meta_dependencies"] = json!([{
            "recipe": recipe,
            "target": "optional-meta",
            "dependency": "lib${AROS_TARGET_CPU}"
        }]);
        fixture.write_contract(&document);
        let error = error_text(fixture.load());
        assert!(error.contains("recipe"), "{recipe}: {error}");
    }

    let fixture = new_fixture();
    let valid_edge = json!({
        "recipe": "kernel/makefile.src",
        "target": "optional-meta",
        "dependency": "lib${AROS_TARGET_CPU}"
    });
    let mut duplicate = fixture.document.clone();
    duplicate["optional_meta_dependencies"] = json!([valid_edge.clone(), valid_edge]);
    fixture.write_contract(&duplicate);
    assert!(error_text(fixture.load()).contains("duplicates"));

    let mut too_many = fixture.document.clone();
    too_many["optional_meta_dependencies"] = Value::Array(
        (0..=64)
            .map(|index| {
                json!({
                    "recipe": "kernel/makefile.src",
                    "target": format!("optional-meta-{index}"),
                    "dependency": "lib${AROS_TARGET_CPU}"
                })
            })
            .collect(),
    );
    fixture.write_contract(&too_many);
    assert!(error_text(fixture.load()).contains("exceeds 64 entries"));

    let mut switches = fixture.document.clone();
    switches["make_variables"] = Value::Object(
        (0..=MAX_MAKE_VARIABLES)
            .map(|index| (format!("SWITCH_{index}"), Value::String(String::new())))
            .collect(),
    );
    fixture.write_contract(&switches);
    assert!(error_text(fixture.load()).contains("make_variables exceeds 256 entries"));

    let mut null_edges = fixture.document.clone();
    null_edges["optional_meta_dependencies"] = Value::Null;
    fixture.write_contract(&null_edges);
    assert!(fixture.load().is_err());

    assert!(
        serde_json::from_value::<NativeOptionalMetaDependency>(json!({
            "recipe": "kernel/makefile.src",
            "target": "optional-meta",
            "dependency": "lib${AROS_TARGET_CPU}",
            "ignored": true
        }))
        .is_err()
    );
}

#[test]
fn source_optional_disabled_owner_is_explicit_and_literal() {
    let fixture = new_fixture();
    let mut document = fixture.document.clone();
    document["optional_meta_dependencies"] = json!([{
        "recipe": "kernel/makefile.src", "target": "aggregate",
        "dependency": "disabled-pkgconfig", "absence": "disabled-owner"
    }]);
    fixture.write_contract(&document);
    assert_eq!(
        fixture.load().unwrap().optional_meta_dependencies[0].absence,
        NativeMetaAbsence::DisabledOwner
    );
    for invalid in ["${AROS_TARGET_CPU}", "foo/bar", "", "$(shell false)"] {
        document["optional_meta_dependencies"][0]["dependency"] = json!(invalid);
        fixture.write_contract(&document);
        assert!(fixture.load().is_err());
    }
    document["optional_meta_dependencies"][0]["dependency"] = json!("disabled-pkgconfig");
    for invalid in [json!("ignore-missing"), json!(null), json!(false)] {
        document["optional_meta_dependencies"][0]["absence"] = invalid;
        fixture.write_contract(&document);
        assert!(fixture.load().is_err());
    }
}

#[cfg(unix)]
#[test]
fn source_optional_meta_dependencies_reject_symlinked_recipes() {
    use std::os::unix::fs::symlink;

    let fixture = new_fixture();
    fs::write(
        fixture.root().join("kernel/uninventoried.src"),
        "synthetic MetaMake recipe\n",
    )
    .unwrap();
    fs::remove_file(fixture.root().join("kernel/mmakefile")).unwrap();
    symlink("uninventoried.src", fixture.root().join("kernel/mmakefile")).unwrap();
    let mut document = fixture.document.clone();
    for input in document["inputs"].as_array_mut().unwrap() {
        if input["path"] == "kernel/mmakefile" {
            input["sha256"] =
                json!(crate::sha256_bytes(b"synthetic MetaMake recipe\n").to_string());
        }
    }
    document["optional_meta_dependencies"] = json!([{
        "recipe": "kernel/mmakefile",
        "target": "optional-meta",
        "dependency": "lib${AROS_TARGET_CPU}"
    }]);
    fixture.write_contract(&document);

    let error = error_text(fixture.load());
    assert!(error.contains("non-symlink source file"), "{error}");
}

#[cfg(unix)]
#[test]
fn source_make_include_bindings_reject_symlinked_original_or_replacement() {
    use std::os::unix::fs::symlink;

    for mapped_path in ["config/aros.cfg", "arch/native-config.mk"] {
        let fixture = make_include_fixture();
        let path = fixture.root().join(mapped_path);
        let target = fixture.root().join("symlink-target.txt");
        fs::rename(&path, &target).unwrap();
        symlink("../symlink-target.txt", &path).unwrap();
        fixture.write_contract(&fixture.document);

        let error = error_text(fixture.load());
        assert!(
            error.contains("non-symlink source file"),
            "{mapped_path}: {error}"
        );
    }
}
