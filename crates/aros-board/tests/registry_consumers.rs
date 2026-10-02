//! Consumer behavior for the embedded board catalog and local format-2 files.

use aros_board::config::{create_template, prepare_template, Transport};
use aros_common::board_registry::BoardId;

fn board_id(value: &str) -> BoardId {
    BoardId::try_from(value.to_owned()).expect("syntactically valid model ID")
}

#[test]
fn unknown_models_and_incompatible_transports_fail_before_publication() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let existing = temporary.path().join("boards.toml");
    let initial = prepare_template(
        Some(&existing),
        "local-rpi4",
        board_id("rpi4"),
        Some(Transport::NativeTftp),
    )
    .expect("known model template");
    create_template(&initial).expect("create the initial local configuration");
    let before = std::fs::read(&existing).expect("read initial configuration");

    let unknown = prepare_template(Some(&existing), "future-board", board_id("esp32-p4"), None)
        .expect_err("valid but unregistered model must be rejected");
    assert!(unknown.to_string().contains("esp32-p4"));
    assert_eq!(
        std::fs::read(&existing).expect("existing config remains"),
        before
    );

    let incompatible = prepare_template(
        Some(&existing),
        "local-rpi5-usb",
        board_id("rpi5"),
        Some(Transport::UbootUsbEcm),
    )
    .expect_err("unsupported model and transport pair must be rejected");
    assert!(incompatible.to_string().contains("uboot-usb-ecm"));
    assert_eq!(
        std::fs::read(&existing).expect("existing config remains"),
        before
    );

    let replacement = prepare_template(Some(&existing), "replacement", board_id("rpi5"), None)
        .expect("valid replacement can be prepared without writing");
    assert!(create_template(&replacement).is_err());
    assert_eq!(
        std::fs::read(&existing).expect("existing config remains"),
        before
    );
}

#[test]
fn failed_template_write_does_not_damage_an_existing_configuration() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let existing = temporary.path().join("boards.toml");
    let initial = prepare_template(
        Some(&existing),
        "local-rpi4",
        board_id("rpi4"),
        Some(Transport::NativeTftp),
    )
    .expect("known model template");
    create_template(&initial).expect("create the initial local configuration");
    let before = std::fs::read(&existing).expect("read initial configuration");

    let blocker = temporary.path().join("regular-file");
    std::fs::write(&blocker, b"not a directory").expect("create a file as a parent blocker");
    let failed_path = blocker.join("boards.toml");
    let prepared = prepare_template(Some(&failed_path), "local-rpi5", board_id("rpi5"), None)
        .expect("preparation does not touch the destination");

    assert!(create_template(&prepared).is_err());
    assert!(!failed_path.exists());
    assert_eq!(
        std::fs::read(&existing).expect("existing config remains"),
        before
    );
}
