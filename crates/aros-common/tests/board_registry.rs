use aros_common::board_registry::{built_in_board_registry, BoardRegistry, BoardTransportContract};
use aros_common::{sha256_bytes, ArosError};

const SOURCE: &str = "test-registry.toml";

const VALID_REGISTRY: &str = r#"format_version = 1

[[boards]]
id = "demo"
backend = "raspberry-pi"
architecture = "arm"
device_tree = "demo-board.dtb"
default_transport = "native-tftp"

[[boards.transports]]
id = "native-tftp"
preset = "demo-arm-debug"
toolchain_preset = "arm-raspi"
build_target = "rpi-artifacts"
artifact_dir = "build/demo/boot"
"#;

fn parse(text: &str) -> aros_common::Result<BoardRegistry> {
    BoardRegistry::parse(SOURCE, text)
}

fn assert_invalid(text: &str, expected_cause: &str) {
    let error = parse(text).expect_err("registry should be rejected");
    match error {
        ArosError::Configuration { file, message } => {
            assert_eq!(file, SOURCE);
            assert!(
                message.contains(expected_cause),
                "expected diagnostic containing {expected_cause:?}, got {message:?}"
            );
        }
        other => panic!("expected a registry configuration error, got {other:?}"),
    }
}

#[test]
fn built_in_registry_exposes_exact_models_defaults_and_capabilities() {
    let registry = built_in_board_registry().unwrap();
    let boards: Vec<_> = registry
        .boards()
        .map(|board| {
            (
                board.id().as_str(),
                board.backend(),
                board.architecture(),
                board.device_tree(),
                board.default_transport(),
                board
                    .transports()
                    .map(BoardTransportContract::id)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    assert_eq!(
        boards,
        vec![
            (
                "milk-v-titan",
                "opensbi-uefi",
                "riscv64",
                None,
                "uefi-esp",
                vec!["uefi-esp"],
            ),
            (
                "rpi3",
                "raspberry-pi",
                "arm",
                Some("bcm2710-rpi-3-b-plus.dtb"),
                "native-tftp",
                vec!["native-tftp"],
            ),
            (
                "rpi4",
                "raspberry-pi",
                "aarch64",
                Some("bcm2711-rpi-4-b.dtb"),
                "native-tftp",
                vec!["native-tftp", "uboot-usb-ecm"],
            ),
            (
                "rpi5",
                "raspberry-pi",
                "aarch64",
                Some("bcm2712-rpi-5-b.dtb"),
                "native-tftp",
                vec!["native-tftp"],
            ),
        ]
    );

    let rpi4 = registry.get("rpi4").unwrap();
    let native = rpi4.transport("native-tftp").unwrap();
    assert_eq!(native.preset(), "rpi4-aarch64-debug");
    assert_eq!(native.toolchain_preset(), "rpi-aarch64");
    assert_eq!(native.build_target(), "rpi-artifacts");
    assert_eq!(native.artifact_dir(), "build/rpi4-aarch64-debug/boot/rpi4");
    assert_eq!(native.media_profile(), None);
    assert_eq!(native.debug_transport(), Some("jtag"));
    assert_eq!(native.legacy_core_architecture(), Some("raspi-aarch64"));

    let uboot = rpi4.transport("uboot-usb-ecm").unwrap();
    assert_eq!(uboot.media_profile(), Some("rpi4-uboot-usb-ecm"));
    assert_eq!(uboot.debug_transport(), Some("jtag"));
    assert_eq!(uboot.legacy_core_architecture(), Some("raspi-aarch64"));

    // The catalog makes no ESP32 claim: it lists four reviewed model IDs only.
    assert_eq!(
        registry
            .boards()
            .map(|board| board.id().as_str())
            .collect::<Vec<_>>(),
        ["milk-v-titan", "rpi3", "rpi4", "rpi5"]
    );
}

#[test]
fn adding_a_board_requires_only_a_data_descriptor() {
    let expanded = format!(
        "{VALID_REGISTRY}\n\
         [[boards]]\n\
         id = \"new-riscv-board\"\n\
         backend = \"opensbi-uefi\"\n\
         architecture = \"riscv64\"\n\
         default_transport = \"uefi-esp\"\n\
         [[boards.transports]]\n\
         id = \"uefi-esp\"\n\
         preset = \"new-riscv-debug\"\n\
         toolchain_preset = \"opensbi-riscv64\"\n\
         build_target = \"opensbi-uefi-artifacts\"\n\
         artifact_dir = \"build/new-riscv-board/boot\"\n"
    );
    let registry = parse(&expanded).unwrap();
    assert_eq!(registry.boards().len(), 2);
    let added = registry.get("new-riscv-board").unwrap();
    assert_eq!(added.backend(), "opensbi-uefi");
    assert_eq!(added.architecture(), "riscv64");
    assert_eq!(added.default_transport(), "uefi-esp");
}

#[test]
fn board_and_transport_iteration_is_sorted_independently_of_document_order() {
    let unsorted = r#"format_version = 1

[[boards]]
id = "zeta"
backend = "raspberry-pi"
architecture = "arm"
device_tree = "zeta.dtb"
default_transport = "uboot-usb-ecm"

[[boards.transports]]
id = "uboot-usb-ecm"
preset = "zeta-debug"
toolchain_preset = "arm-raspi"
build_target = "rpi-artifacts"
artifact_dir = "build/zeta"

[[boards.transports]]
id = "native-tftp"
preset = "zeta-debug"
toolchain_preset = "arm-raspi"
build_target = "rpi-artifacts"
artifact_dir = "build/zeta"

[[boards]]
id = "alpha"
backend = "opensbi-uefi"
architecture = "riscv64"
default_transport = "uefi-esp"

[[boards.transports]]
id = "uefi-esp"
preset = "alpha-debug"
toolchain_preset = "opensbi-riscv64"
build_target = "opensbi-uefi-artifacts"
artifact_dir = "build/alpha"
"#;

    let registry = parse(unsorted).unwrap();
    assert_eq!(
        registry
            .boards()
            .map(|board| board.id().as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert_eq!(
        registry
            .get("zeta")
            .unwrap()
            .transports()
            .map(BoardTransportContract::id)
            .collect::<Vec<_>>(),
        ["native-tftp", "uboot-usb-ecm"]
    );
}

#[test]
fn registry_digest_binds_the_exact_embedded_bytes() {
    const EMBEDDED: &[u8] = include_bytes!("../../../profiles/boards/registry-v1.toml");
    let registry = built_in_board_registry().unwrap();
    assert_eq!(
        registry.sha256().as_str(),
        "0fe63e0841639ee8475cb7d1741d9ad7577885de2d11d3d32257f8ea1600014f"
    );
    assert_eq!(registry.sha256(), &sha256_bytes(EMBEDDED));

    let with_trailing_newline = format!("{}\n", std::str::from_utf8(EMBEDDED).unwrap());
    let changed = parse(&with_trailing_newline).unwrap();
    assert_eq!(
        changed.sha256(),
        &sha256_bytes(with_trailing_newline.as_bytes())
    );
    assert_ne!(changed.sha256(), registry.sha256());
}

#[test]
fn duplicate_board_and_transport_ids_are_rejected_with_specific_diagnostics() {
    let second_board = VALID_REGISTRY.strip_prefix("format_version = 1\n").unwrap();
    let duplicate_board = format!("{VALID_REGISTRY}\n{second_board}");
    assert_invalid(&duplicate_board, "duplicate board ID");

    let duplicate_transport = format!(
        "{VALID_REGISTRY}\n\
         [[boards.transports]]\n\
         id = \"native-tftp\"\n\
         preset = \"demo-arm-debug\"\n\
         toolchain_preset = \"arm-raspi\"\n\
         build_target = \"rpi-artifacts\"\n\
         artifact_dir = \"build/demo/boot\"\n"
    );
    assert_invalid(&duplicate_transport, "duplicate transport ID");
}

#[test]
fn unknown_fields_missing_fields_and_unsupported_versions_are_rejected() {
    assert_invalid(
        &VALID_REGISTRY.replace(
            "format_version = 1",
            "format_version = 1\nunexpected = true",
        ),
        "unknown field `unexpected`",
    );
    assert_invalid(
        &VALID_REGISTRY.replace("architecture = \"arm\"\n", ""),
        "missing field `architecture`",
    );
    assert_invalid(
        &VALID_REGISTRY.replace("format_version = 1", "format_version = 2"),
        "unsupported registry format_version",
    );
}

#[test]
fn malformed_ids_unknown_backend_and_wrong_architecture_are_rejected() {
    assert_invalid(
        &VALID_REGISTRY.replace("id = \"demo\"", "id = \"Demo\""),
        "board ID must be",
    );
    assert_invalid(
        &VALID_REGISTRY.replace("backend = \"raspberry-pi\"", "backend = \"esp32\""),
        "unsupported backend capability",
    );
    assert_invalid(
        &VALID_REGISTRY.replace("backend = \"raspberry-pi\"", "backend = \"opensbi-uefi\""),
        "backend and architecture are incompatible",
    );
}

#[test]
fn unsupported_transport_and_undeclared_default_are_rejected() {
    assert_invalid(
        &VALID_REGISTRY.replace("id = \"native-tftp\"", "id = \"uart\""),
        "unsupported backend/transport capability",
    );
    assert_invalid(
        &VALID_REGISTRY.replace(
            "default_transport = \"native-tftp\"",
            "default_transport = \"uboot-usb-ecm\"",
        ),
        "default_transport is not declared",
    );

    let registry = parse(VALID_REGISTRY).unwrap();
    let error = registry.get("demo").unwrap().transport("uart").unwrap_err();
    assert!(error.to_string().contains("unsupported transport 'uart'"));
}

#[test]
fn unsafe_targets_paths_device_trees_and_media_references_are_rejected() {
    assert_invalid(
        &VALID_REGISTRY.replace("preset = \"demo-arm-debug\"", "preset = \"../debug\""),
        "unsafe target or toolchain reference",
    );
    assert_invalid(
        &VALID_REGISTRY.replace(
            "artifact_dir = \"build/demo/boot\"",
            "artifact_dir = \"../outside\"",
        ),
        "artifact_dir must be a safe relative path",
    );
    assert_invalid(
        &VALID_REGISTRY.replace(
            "device_tree = \"demo-board.dtb\"",
            "device_tree = \"../escape.dtb\"",
        ),
        "Pi device_tree must be a safe DTB basename",
    );
    assert_invalid(
        &VALID_REGISTRY.replace(
            "artifact_dir = \"build/demo/boot\"",
            "artifact_dir = \"build/demo/boot\"\nmedia_profile = \"../outside\"",
        ),
        "invalid media_profile reference",
    );
}

#[test]
fn registry_document_size_is_bounded() {
    let oversized = format!("{VALID_REGISTRY}{}", " ".repeat(256 * 1024));
    assert_invalid(&oversized, "registry exceeds 256 KiB");
}

#[test]
fn explicit_unknown_model_selection_is_distinct_from_malformed_selection() {
    let registry = built_in_board_registry().unwrap();

    let unknown = registry.get("esp32").unwrap_err();
    assert!(unknown.to_string().contains("unknown board 'esp32'"));

    let malformed = registry.get("ESP32").unwrap_err();
    assert!(malformed.to_string().contains("board ID must be"));
}
