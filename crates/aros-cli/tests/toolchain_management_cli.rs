//! Black-box contracts for checkout-independent M8 import and registration.

use aros_common::{
    toolchain_tree_inventory, ArosToolchainManifest, ArosToolchainManifestEntry,
    AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn command(directory: &Path) -> Command {
    let mut command = Command::new(aros());
    command
        .current_dir(directory)
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE")
        .env_remove("AROS_HOME")
        .env_remove("AROS_CACHE_DIR")
        .env_remove("AROS_CROSS_TOOLCHAINS_DIR");
    command
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn manifest() -> ArosToolchainManifest {
    ArosToolchainManifest {
        schema: 1,
        release_id: "local-fixture".into(),
        host: "linux-x86_64".into(),
        target_profile: "pc-x86_64".into(),
        target_triple: "x86_64-unknown-aros".into(),
        tree_sha256: "a".repeat(64),
        llvm_version: Some("11.0.0".into()),
        recipe_sha256: "b".repeat(64),
        source_lock_sha256: "c".repeat(64),
        profiles_sha256: "d".repeat(64),
        source_commit: "e".repeat(40),
        producer_commit: "f".repeat(40),
        tools_commit: "0".repeat(40),
        source_date_epoch: 1,
        capabilities: vec!["collect-aros".into()],
        build_environment: serde_json::Map::new(),
        files: vec![ArosToolchainManifestEntry {
            path: "bin".into(),
            mode: "0755".into(),
            kind: "directory".into(),
            sha256: None,
            size: None,
            target: None,
        }],
    }
}

fn write_candidate(root: &Path) {
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/aros-collect"), b"fixture collector").unwrap();
    let (tree_sha256, files) = toolchain_tree_inventory(root).unwrap();
    let mut candidate = manifest();
    candidate.tree_sha256 = tree_sha256;
    candidate.files = files;
    fs::write(
        root.join(AROS_TOOLCHAIN_MANIFEST_FILE),
        serde_json::to_vec(&candidate).unwrap(),
    )
    .unwrap();
}

#[test]
fn import_preview_apply_and_inventory_work_outside_an_aros_checkout() {
    let temporary = tempfile::tempdir().unwrap();
    let working_directory = temporary.path().join("not-an-aros-checkout");
    let source = temporary.path().join("candidate");
    let store = temporary.path().join("store");
    fs::create_dir(&working_directory).unwrap();
    write_candidate(&source);

    let preview = output_json(
        &command(&working_directory)
            .args(["toolchain", "import", "--format", "json", "--source"])
            .arg(&source)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(preview["schema"], "aros-toolchain-management-v1");
    assert_eq!(preview["operation"], "import");
    assert_eq!(preview["state"], "preview");
    let token = preview["apply_token"].as_str().unwrap().to_owned();
    assert!(!store.exists());

    let committed = output_json(
        &command(&working_directory)
            .args(["toolchain", "import", "--format", "json", "--source"])
            .arg(&source)
            .arg("--store")
            .arg(&store)
            .args(["--apply", &token])
            .output()
            .unwrap(),
    );
    assert_eq!(committed["state"], "committed");
    assert_eq!(committed["management"], "owned-import");

    let inventory = output_json(
        &command(&working_directory)
            .args(["toolchain", "inventory", "--format", "json", "--store"])
            .arg(&store)
            .output()
            .unwrap(),
    );
    let entry = &inventory["entries"][0];
    assert_eq!(entry["management"], "owned-import");
    assert_eq!(entry["provenance"], "imported-local-receipt");
    assert_eq!(entry["integrity"], "not-checked");
}

#[test]
fn registration_remains_non_owning_outside_an_aros_checkout() {
    let temporary = tempfile::tempdir().unwrap();
    let working_directory = temporary.path().join("not-an-aros-checkout");
    let source = temporary.path().join("candidate");
    let store = temporary.path().join("store");
    fs::create_dir(&working_directory).unwrap();
    write_candidate(&source);

    let preview = output_json(
        &command(&working_directory)
            .args(["toolchain", "register", "--format", "json", "--source"])
            .arg(&source)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap().to_owned();
    assert!(!store.exists());

    let committed = output_json(
        &command(&working_directory)
            .args(["toolchain", "register", "--format", "json", "--source"])
            .arg(&source)
            .arg("--store")
            .arg(&store)
            .args(["--apply", &token])
            .output()
            .unwrap(),
    );
    assert_eq!(committed["state"], "committed");
    assert_eq!(committed["management"], "non-owning-external");
    assert!(source.join(AROS_TOOLCHAIN_MANIFEST_FILE).is_file());
    assert!(!store.join("imports").exists());
}

#[test]
fn stale_import_preview_cannot_publish_after_the_source_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let working_directory = temporary.path().join("not-an-aros-checkout");
    let source = temporary.path().join("candidate");
    let store = temporary.path().join("store");
    fs::create_dir(&working_directory).unwrap();
    write_candidate(&source);

    let preview = output_json(
        &command(&working_directory)
            .args(["toolchain", "import", "--format", "json", "--source"])
            .arg(&source)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    fs::write(source.join("bin/aros-collect"), b"mutated after preview").unwrap();

    let stale_apply = command(&working_directory)
        .args(["toolchain", "import", "--format", "json", "--source"])
        .arg(&source)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    assert!(!stale_apply.status.success());
    assert!(!store.exists());
}
