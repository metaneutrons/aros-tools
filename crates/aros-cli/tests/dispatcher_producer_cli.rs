//! The protected dispatcher commands are reachable through the `aros` runtime
//! and fail closed without provider credentials.

#![cfg(unix)]

use std::process::Command;

fn aros() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
    // These tests must never reach a provider.
    for key in ["GH_TOKEN", "GH_HOST", "GITHUB_TOKEN", "GITHUB_EVENT_PATH"] {
        command.env_remove(key);
    }
    command
}

const LEAVES: [&str; 5] = [
    "dispatcher-select",
    "dispatcher-claim",
    "dispatcher-verify-claim",
    "dispatcher-dispatch",
    "dispatcher-verify-binding",
];

#[test]
fn every_dispatcher_leaf_is_a_public_producer_command_with_help() {
    let group = aros()
        .args(["toolchain", "producer", "--help"])
        .output()
        .unwrap();
    assert!(group.status.success());
    let text = String::from_utf8_lossy(&group.stdout);
    for leaf in LEAVES {
        assert!(text.contains(leaf), "{leaf} is listed");
        let help = aros()
            .args(["toolchain", "producer", leaf, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success(), "{leaf} --help");
        assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));
    }
}

#[test]
fn dispatch_needs_every_independent_selector_and_never_retries() {
    let missing = aros()
        .args(["toolchain", "producer", "dispatcher-dispatch"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let text = String::from_utf8_lossy(&missing.stderr);
    assert!(
        text.contains("--record") && text.contains("--source-context"),
        "{text}"
    );
    let retry = aros()
        .args([
            "toolchain",
            "producer",
            "dispatcher-dispatch",
            "--retry",
            "3",
        ])
        .output()
        .unwrap();
    assert!(!retry.status.success());
}
