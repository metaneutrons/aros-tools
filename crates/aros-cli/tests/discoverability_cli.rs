//! Black-box contracts for completion and read-only machine inspection output.

use serde_json::Value;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn command(directory: &Path, state: &Path) -> Command {
    let mut command = Command::new(aros());
    command
        .current_dir(directory)
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE")
        .env("AROS_HOME", state)
        .env("AROS_CACHE_DIR", state.join("archive-cache"))
        .env("AROS_HOST_COMPILER_DIR", state.join("host-compiler"))
        .env("AROS_CROSS_TOOLCHAINS_DIR", state.join("cross-toolchains"));
    command
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("successful machine output must be JSON")
}

fn write_checkout(root: &Path) {
    for directory in ["arch", "compiler", "rom", "developer"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(root.join("configure"), "").unwrap();
    fs::write(root.join("Makefile.in"), "").unwrap();
    fs::write(
        root.join("aros-targets.toml"),
        "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
    )
    .unwrap();
}

fn write_host_matrix_lock(root: &Path) {
    let mut content = String::from(
        "schema = 1\nrelease_id = 'fixture-release'\nbase_url = 'https://example.invalid/toolchains'\n",
    );
    for host in [
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
        "macos-aarch64",
    ] {
        write!(
            content,
            "\n[[artifacts]]\nhost = '{host}'\ntarget_profile = 'pc-x86_64'\ntarget_triple = 'x86_64-unknown-aros'\nasset = 'fixture.tar.xz'\nsha256 = '{}'\ntree_sha256 = '{}'\nenabled = true\nrequired_paths = []\n",
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap();
    }
    fs::write(root.join("aros-toolchains.lock.toml"), content).unwrap();
}

fn shell_is_available(shell: &str) -> bool {
    Command::new(shell)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn completions_are_deterministic_public_and_side_effect_free() {
    let temporary = tempfile::tempdir().unwrap();
    let working_directory = temporary.path().join("empty");
    let state = temporary.path().join("state");
    fs::create_dir(&working_directory).unwrap();

    for (shell, marker) in [
        ("bash", "complete -F"),
        ("zsh", "#compdef aros"),
        ("fish", "complete -c aros"),
    ] {
        let first = command(&working_directory, &state)
            .args(["completions", shell])
            .output()
            .unwrap();
        let second = command(&working_directory, &state)
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(
            first.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert!(first.stderr.is_empty(), "{shell} completion emitted stderr");
        assert_eq!(first.stdout, second.stdout, "{shell} completion changed");
        let generated = String::from_utf8(first.stdout).unwrap();
        assert!(
            generated.contains(marker),
            "{shell} completion lacks its marker"
        );
        assert!(generated.contains("toolchain"));
        assert!(
            generated.contains("--diagnostic-format"),
            "{shell} completion omitted a visible global option"
        );
        assert!(
            generated.contains("--format"),
            "{shell} completion omitted a visible leaf option"
        );
        for visible_shell in ["bash", "zsh", "fish"] {
            assert!(
                generated.contains(visible_shell),
                "{shell} completion omitted the completions positional value {visible_shell}"
            );
        }
        assert!(
            !generated.contains("__metamake-fetch"),
            "{shell} completion exposed a hidden internal command"
        );
        assert!(
            !state.exists(),
            "{shell} completion must not create state or logs"
        );

        if shell_is_available(shell) {
            let generated_path = temporary.path().join(format!("aros.{shell}"));
            fs::write(&generated_path, generated).unwrap();
            let output = Command::new(shell)
                .arg("-n")
                .arg(&generated_path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{shell} syntax check failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn info_json_is_versioned_and_distinguishes_checkout_availability() {
    let temporary = tempfile::tempdir().unwrap();
    let outside = temporary.path().join("outside");
    let state = temporary.path().join("state");
    fs::create_dir(&outside).unwrap();

    let unavailable = output_json(
        &command(&outside, &state)
            .args(["info", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert_eq!(unavailable["schema"], "aros-info-v1");
    assert!(unavailable["tool_version"].is_string());
    assert_eq!(unavailable["checkout"]["state"], "unavailable");
    assert!(unavailable["checkout"]["root"].is_null());
    assert!(unavailable["host_compiler"]["status"].is_string());
    assert!(unavailable["state"]["cross_toolchain_store"].is_string());
    assert!(!state.exists(), "info must not create a state directory");

    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    write_checkout(&checkout);
    let available = output_json(
        &command(&checkout, &state)
            .args(["info", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert_eq!(available["schema"], "aros-info-v1");
    assert_eq!(available["checkout"]["state"], "available");
    assert_eq!(
        available["checkout"]["target_profile_source"],
        "checkout-override"
    );
    assert_eq!(
        available["checkout"]["target_profiles"],
        serde_json::json!(["pc-x86_64"])
    );
    assert!(available["checkout"]["toolchain_lock"].is_null());
}

#[test]
fn toolchain_list_json_preserves_lock_and_verification_states() {
    let temporary = tempfile::tempdir().unwrap();
    let checkout = temporary.path().join("checkout");
    let state = temporary.path().join("state");
    fs::create_dir(&checkout).unwrap();
    write_checkout(&checkout);
    write_host_matrix_lock(&checkout);

    let list = output_json(
        &command(&checkout, &state)
            .args(["toolchain", "list", "--format", "json"])
            .output()
            .unwrap(),
    );
    assert_eq!(list["schema"], "aros-toolchain-list-v1");
    assert_eq!(list["observation"], "lock-and-local-installation");
    assert_eq!(list["release_id"], "fixture-release");
    let artifacts = list["artifacts"].as_array().unwrap();
    assert_eq!(
        artifacts.len(),
        1,
        "the current-host matrix must be filtered"
    );
    assert_eq!(artifacts[0]["status"], "available");
    assert_eq!(artifacts[0]["verification"], "metadata-only");
    assert_eq!(artifacts[0]["enabled"], true);
    assert!(!state.exists(), "listing must not create an installation");
}
