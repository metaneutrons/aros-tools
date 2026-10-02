//! The embedded catalog is the board CLI's source for generated defaults.

use aros_board::config::{
    create_template, load_board, prepare_template, BoardBackend, BoardConfig, BoardsConfig,
    DebugTransport, Transport,
};
use aros_common::board_registry::{built_in_board_registry, BoardContract, BoardTransportContract};
use aros_common::media_profile::built_in_media_profiles;
use aros_common::Sha256Digest;
use std::path::{Path, PathBuf};

fn transport_from_id(id: &str) -> Transport {
    match id {
        "native-tftp" => Transport::NativeTftp,
        "uboot-usb-ecm" => Transport::UbootUsbEcm,
        "uefi-esp" => Transport::UefiEsp,
        _ => panic!("registry transport '{id}' has no typed board transport"),
    }
}

fn assert_catalog_defaults(
    config: &BoardConfig,
    contract: &BoardContract,
    transport: &BoardTransportContract,
) {
    assert_eq!(config.model.as_str(), contract.id().as_str());
    assert_eq!(config.backend.to_string(), contract.backend());
    assert_eq!(config.transport.as_str(), transport.id());
    assert_eq!(config.preset, transport.preset());
    assert_eq!(config.toolchain_preset, transport.toolchain_preset());
    assert_eq!(config.build_target, transport.build_target());
    assert_eq!(
        config.artifact_dir.as_deref(),
        Some(Path::new(transport.artifact_dir()))
    );
    assert_eq!(
        config.debug_transport.map(|value| value.to_string()),
        transport.debug_transport().map(str::to_owned)
    );
}

fn assert_template_round_trip(
    root: &Path,
    contract: &BoardContract,
    transport: &BoardTransportContract,
    board_name: &str,
    selected_transport: Option<Transport>,
    registry_sha256: &Sha256Digest,
) {
    let path = root.join(format!("{board_name}.toml"));
    let template = prepare_template(
        Some(&path),
        board_name,
        contract.id().clone(),
        selected_transport,
    )
    .expect("catalog model and transport produce a template");

    assert_eq!(template.model().as_str(), contract.id().as_str());
    assert_eq!(template.transport(), transport_from_id(transport.id()));
    assert_eq!(template.registry_sha256(), registry_sha256);
    assert_ne!(template.board_name(), template.model().as_str());
    assert!(!path.exists(), "preparation must not publish a file");

    let parsed: BoardsConfig = toml::from_str(template.contents()).expect("format-2 template");
    assert_eq!(parsed.format_version, 2);
    assert_eq!(parsed.boards.len(), 1);
    assert_catalog_defaults(
        parsed.boards.get(board_name).expect("local profile alias"),
        contract,
        transport,
    );

    create_template(&template).expect("publish to the temporary directory");
    let loaded = load_board(Some(&path), board_name).expect("loaded profile validates");
    assert_catalog_defaults(&loaded.config, contract, transport);
    assert_eq!(loaded.name, board_name);
}

#[test]
fn every_catalog_default_and_declared_transport_round_trips_through_format2() {
    let registry = built_in_board_registry().expect("reviewed board catalog");
    let contracts: Vec<_> = registry.boards().collect();
    let ids: Vec<_> = contracts
        .iter()
        .map(|contract| contract.id().as_str())
        .collect();
    assert_eq!(ids, ["milk-v-titan", "rpi3", "rpi4", "rpi5"]);

    let temporary = tempfile::tempdir().expect("temporary directory");
    for contract in contracts {
        let default = contract
            .transport(contract.default_transport())
            .expect("declared default transport");
        let default_name = format!("local-{}-default", contract.id());
        assert_template_round_trip(
            temporary.path(),
            contract,
            default,
            &default_name,
            None,
            registry.sha256(),
        );

        for transport in contract.transports() {
            let transport_name = format!("local-{}-{}", contract.id(), transport.id());
            assert_template_round_trip(
                temporary.path(),
                contract,
                transport,
                &transport_name,
                Some(transport_from_id(transport.id())),
                registry.sha256(),
            );
        }
    }
}

#[test]
fn selected_usb_profile_retains_its_reviewed_build_defaults() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("lab-rpi4-usb.toml");
    let template = prepare_template(
        Some(&path),
        "lab-rpi4-usb",
        aros_common::board_registry::BoardId::try_from("rpi4".to_owned()).expect("stable model ID"),
        Some(Transport::UbootUsbEcm),
    )
    .expect("reviewed USB-ECM template");

    let parsed: BoardsConfig = toml::from_str(template.contents()).expect("format-2 template");
    let profile = parsed.boards.get("lab-rpi4-usb").expect("local alias");
    assert_eq!(profile.backend, BoardBackend::RaspberryPi);
    assert_eq!(profile.model.as_str(), "rpi4");
    assert_eq!(profile.transport, Transport::UbootUsbEcm);
    assert_eq!(profile.preset, "rpi4-aarch64-debug");
    assert_eq!(profile.toolchain_preset, "rpi-aarch64");
    assert_eq!(profile.build_target, "rpi-artifacts");
    assert_eq!(
        profile.artifact_dir.as_deref(),
        Some(Path::new("build/rpi4-aarch64-debug/boot/rpi4"))
    );
    assert_eq!(profile.debug_transport, Some(DebugTransport::Jtag));

    create_template(&template).expect("publish to the temporary directory");
    let loaded = load_board(Some(&path), "lab-rpi4-usb").expect("loaded profile validates");
    assert_eq!(loaded.config.model.as_str(), "rpi4");
    assert_eq!(loaded.config.transport, Transport::UbootUsbEcm);
}

#[test]
fn all_legacy_format2_examples_load_and_keep_local_overrides() {
    const EXAMPLE: &str = include_str!("../../../support/rpi-debug/boards.example.toml");

    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("boards.toml");
    std::fs::write(&path, EXAMPLE).expect("copy example into the temporary directory");

    let parsed: BoardsConfig = toml::from_str(EXAMPLE).expect("legacy format-2 example");
    assert_eq!(parsed.format_version, 2);
    assert_eq!(parsed.boards.len(), 5);
    for (name, config) in &parsed.boards {
        let loaded = load_board(Some(&path), name).expect("legacy profile remains valid");
        assert_eq!(loaded.name, *name);
        assert_eq!(loaded.config.model, config.model);
        assert_eq!(loaded.config.transport, config.transport);
    }

    let usb = load_board(Some(&path), "rpi4-usb").expect("legacy USB profile");
    assert_eq!(usb.config.model.as_str(), "rpi4");
    assert_eq!(
        usb.config.tftp_root,
        Some(PathBuf::from("/absolute/path/to/aros-tftp"))
    );
    assert_eq!(
        usb.config.tftp_prefix.as_deref(),
        Some(Path::new("rpi4-usb/current"))
    );
    assert_eq!(
        usb.config.serial_device.as_deref(),
        Some(Path::new("/dev/cu.usbserial-REPLACE_ME"))
    );
    let usb_network = usb.config.usb_ecm.as_ref().expect("local USB settings");
    assert_eq!(usb_network.host_address.to_string(), "192.168.74.1");
    assert_eq!(usb_network.target_address.to_string(), "192.168.74.2");
    assert_eq!(
        usb_network
            .identity
            .as_ref()
            .expect("local device identity")
            .serial,
        "aros-rpi4-lab-01"
    );
}

#[test]
fn reviewed_media_references_match_model_transport_toolchain_and_backend() {
    let registry = built_in_board_registry().expect("reviewed board catalog");
    let media = built_in_media_profiles().expect("reviewed media profiles");
    let mut references = 0;

    for board in registry.boards() {
        for transport in board.transports() {
            let Some(profile_id) = transport.media_profile() else {
                continue;
            };
            let matches: Vec<_> = media
                .iter()
                .filter(|entry| entry.profile.id == profile_id)
                .collect();
            assert_eq!(matches.len(), 1, "unique media profile {profile_id}");

            let profile = &matches[0].profile;
            assert_eq!(profile.model, board.id().as_str());
            assert_eq!(profile.transport, transport.id());
            // Media target_preset names the compiler profile, not the CMake preset.
            assert_eq!(profile.target_preset, transport.toolchain_preset());

            let profile_board = registry
                .get(&profile.model)
                .expect("media profile model is in the embedded catalog");
            assert_eq!(profile_board.id(), board.id());
            assert_eq!(profile_board.backend(), board.backend());
            references += 1;
        }
    }

    assert_eq!(references, 2, "the two existing reviewed media references");
}
