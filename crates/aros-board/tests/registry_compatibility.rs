//! Transitional probes keep catalog data aligned until consumers migrate.

use aros_board::config::{prepare_template, BoardModel, BoardsConfig, Transport};
use aros_common::board_registry::built_in_board_registry;
use std::path::Path;

#[test]
fn reviewed_registry_matches_every_existing_model_and_template() {
    let registry = built_in_board_registry().expect("reviewed board catalog");
    let models = [
        BoardModel::Rpi3,
        BoardModel::Rpi4,
        BoardModel::Rpi5,
        BoardModel::MilkVTitan,
    ];
    assert_eq!(registry.boards().len(), models.len());
    for model in models {
        let contract = registry.get(model.as_str()).expect("existing model");
        assert_eq!(contract.backend(), model.backend().to_string());
        assert_eq!(contract.device_tree(), model.dtb_filename());
        assert_eq!(
            contract.default_transport(),
            model.default_transport().to_string()
        );
        for transport in [
            Transport::NativeTftp,
            Transport::UbootUsbEcm,
            Transport::UefiEsp,
        ] {
            assert_eq!(
                contract.transport(&transport.to_string()).is_ok(),
                model.supports_transport(transport),
                "model {model}, transport {transport}"
            );
            if !model.supports_transport(transport) {
                continue;
            }
            let defaults = contract
                .transport(&transport.to_string())
                .expect("declared transport");
            let template = prepare_template(
                Some(Path::new("unused.toml")),
                "fixture",
                model,
                Some(transport),
            )
            .expect("existing preview-only template");
            let parsed: BoardsConfig =
                toml::from_str(template.contents()).expect("existing template TOML");
            assert_eq!(parsed.format_version, 2);
            let board = &parsed.boards["fixture"];
            assert_eq!(board.preset, defaults.preset());
            assert_eq!(board.toolchain_preset, defaults.toolchain_preset());
            assert_eq!(board.build_target, defaults.build_target());
            assert_eq!(
                board.artifact_dir.as_deref(),
                Some(Path::new(defaults.artifact_dir()))
            );
            assert_eq!(
                board.debug_transport.map(|value| value.to_string()),
                defaults.debug_transport().map(str::to_owned)
            );
        }
    }
}

#[test]
fn registry_media_references_match_the_reviewed_layouts() {
    let registry = built_in_board_registry().expect("reviewed board catalog");
    let media =
        aros_common::media_profile::built_in_media_profiles().expect("reviewed media profiles");
    for board in registry.boards() {
        for transport in board.transports() {
            if let Some(id) = transport.media_profile() {
                let matches: Vec<_> = media
                    .iter()
                    .filter(|entry| entry.profile.id == id)
                    .collect();
                assert_eq!(matches.len(), 1, "unique media profile {id}");
                let selected = &matches[0].profile;
                assert_eq!(selected.model, board.id().as_str());
                assert_eq!(selected.transport, transport.id());
                assert_eq!(selected.target_preset, transport.toolchain_preset());
            }
        }
    }
}
