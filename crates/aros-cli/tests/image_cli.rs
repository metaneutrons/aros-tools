//! Process-boundary contracts for the opt-in boot-media image workflow.

use aros_common::media_profile::built_in_media_profiles;
use aros_common::media_receipt::{MediaBuildFile, MediaBuildReceipt, MediaReceiptOrigin};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn inputs(root: &Path) {
    fs::create_dir(root).unwrap();
    let profile = built_in_media_profiles().unwrap().remove(0).profile;
    let files = profile
        .required_files
        .iter()
        .map(|required| {
            let path = format!("{}.bin", required.role);
            let bytes = required.role.as_bytes();
            fs::write(root.join(&path), bytes).unwrap();
            MediaBuildFile {
                role: required.role.clone(),
                path,
                sha256: aros_common::sha256_bytes(bytes).to_string(),
                size_bytes: bytes.len() as u64,
            }
        })
        .collect();
    let receipt = MediaBuildReceipt::new(
        MediaReceiptOrigin::Cmake,
        profile.target_preset,
        profile.model,
        profile.transport,
        files,
    )
    .unwrap();
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

fn invoke(root: &Path, output: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros"))
        .args([
            "image",
            "build",
            "--profile",
            "rpi4-uboot-usb-ecm",
            "--build-root",
            root.to_str().unwrap(),
            "--receipt",
            root.join("receipt.json").to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(extra)
        .current_dir(root.parent().unwrap())
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .output()
        .unwrap()
}

fn success_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn image_build_preview_and_apply_have_distinct_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("inputs");
    let artifact = temporary.path().join("artifact");
    inputs(&root);

    let preview = success_json(&invoke(&root, &artifact, &[]));
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["file_count"], 6);
    assert!(!artifact.exists());

    let result = success_json(&invoke(&root, &artifact, &["--apply"]));
    assert_eq!(result["applied"], true);
    assert_eq!(result["device_written"], false);
    assert_eq!(result["boot_qualified"], false);
    assert!(artifact.join("aros-media.img").is_file());

    let verification = Command::new(env!("CARGO_BIN_EXE_aros"))
        .args([
            "image",
            "verify",
            "--artifact",
            artifact.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let verification = success_json(&verification);
    assert_eq!(verification["image_sha256"], result["image_sha256"]);

    let second = invoke(&root, &artifact, &["--apply"]);
    assert!(!second.status.success());
    assert!(!second.stderr.is_empty());
    assert!(artifact.join("aros-media.img").is_file());
}

#[test]
fn image_build_rejects_changed_inputs_before_any_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("inputs");
    let artifact = temporary.path().join("artifact");
    inputs(&root);
    fs::write(root.join("config.bin"), b"tampered").unwrap();
    let result = invoke(
        &root,
        &artifact,
        &["--apply", "--diagnostic-format", "json"],
    );
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    let diagnostics: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert!(diagnostics.to_string().contains("image.build"));
    assert!(!artifact.exists());
}
