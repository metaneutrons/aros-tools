//! Process-boundary regressions for board model selection and publication.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary CLI fixture");
    let working = temporary.path().join("empty-workdir");
    fs::create_dir(&working).expect("create empty invocation directory");
    let config = temporary.path().join("config/boards.toml");
    (temporary, working, config)
}

fn invoke(
    temporary: &TempDir,
    working: &Path,
    config: &Path,
    profile: &str,
    model: &str,
    transport: Option<&str>,
    apply: bool,
) -> Output {
    let home = temporary.path().join("home");
    let xdg = temporary.path().join("xdg");
    let aros_home = temporary.path().join("aros-home");
    let mut command = Command::new(aros());
    command
        .current_dir(working)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env("AROS_HOME", &aros_home)
        .env("AROS_DIAGNOSTIC_FORMAT", "json")
        .env("AROS_LOG_LEVEL", "off")
        .env_remove("AROS_BOARDS_FILE")
        .env_remove("AROS_CROSS_TOOLCHAINS_DIR")
        .env_remove("AROS_CACHE_DIR")
        .env_remove("AROS_LOG_FILE")
        .args([
            "board",
            "init",
            "--profile",
            profile,
            "--model",
            model,
            "--config",
            config.to_str().expect("temporary config path is UTF-8"),
        ]);
    if let Some(transport) = transport {
        command.args(["--transport", transport]);
    }
    if apply {
        command.arg("--apply");
    }
    command.output().expect("board init CLI process executes")
}

fn assert_diagnostic(output: &Output, case: &str, expected_code: &str) -> Value {
    assert!(
        !output.status.success(),
        "{case} unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostic: Value = serde_json::from_slice(&output.stderr)
        .unwrap_or_else(|error| panic!("{case} did not emit JSON diagnostics: {error}"));
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(
        diagnostic["diagnostics"][0]["code"], expected_code,
        "{case} returned the wrong stable diagnostic code: {diagnostic}"
    );
    diagnostic
}

#[test]
fn board_init_selection_and_publication_are_process_safe() {
    const LEGACY_CONFIG: &str = include_str!("../../../support/rpi-debug/boards.example.toml");

    let (temporary, working, config) = fixture();
    let unknown = invoke(
        &temporary,
        &working,
        &config,
        "future-board",
        "esp32-p4",
        None,
        false,
    );
    let diagnostic = assert_diagnostic(&unknown, "unknown but syntactically valid model", "AR0001");
    assert!(diagnostic.to_string().contains("esp32-p4"));
    assert!(
        !config.exists(),
        "unknown model must not create configuration"
    );
    assert!(config.parent().is_some_and(|parent| !parent.exists()));
    assert!(!temporary.path().join("aros-home").exists());
    assert!(!temporary.path().join("home").exists());
    assert!(!temporary.path().join("xdg").exists());
    assert!(
        fs::read_dir(&working)
            .expect("inspect empty invocation directory")
            .next()
            .is_none(),
        "unknown model must not create files in the invocation directory"
    );

    fs::create_dir_all(config.parent().expect("config parent"))
        .expect("create local config directory");
    fs::write(&config, LEGACY_CONFIG).expect("write valid legacy format-2 configuration");
    aros_board::config::load_board(Some(&config), "rpi4-usb")
        .expect("existing format-2 configuration validates before process cases");
    let original = fs::read(&config).expect("read original configuration bytes");

    let incompatible = invoke(
        &temporary,
        &working,
        &config,
        "candidate-rpi5-usb",
        "rpi5",
        Some("uboot-usb-ecm"),
        true,
    );
    let diagnostic = assert_diagnostic(&incompatible, "incompatible model and transport", "AR0801");
    assert!(diagnostic.to_string().contains("uboot-usb-ecm"));
    assert_eq!(
        fs::read(&config).expect("configuration remains intact"),
        original
    );

    let preview = invoke(
        &temporary,
        &working,
        &config,
        "preview-rpi4",
        "rpi4",
        Some("native-tftp"),
        false,
    );
    assert!(
        preview.status.success(),
        "known model preview failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(String::from_utf8_lossy(&preview.stdout).contains("Dry run: no file was created"));
    assert_eq!(
        fs::read(&config).expect("preview preserves existing configuration"),
        original
    );

    let apply = invoke(
        &temporary,
        &working,
        &config,
        "replacement-rpi4",
        "rpi4",
        Some("native-tftp"),
        true,
    );
    let diagnostic = assert_diagnostic(&apply, "apply refuses existing configuration", "AR0801");
    assert!(diagnostic.to_string().contains("Refusing to overwrite"));
    assert_eq!(
        fs::read(&config).expect("failed apply preserves configuration"),
        original
    );
    assert!(!temporary.path().join("aros-home").exists());
}
