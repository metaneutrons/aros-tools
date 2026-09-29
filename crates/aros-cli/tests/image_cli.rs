//! Process-boundary contracts for the opt-in boot-media image workflow.

use aros_common::media_profile::built_in_media_profiles;
use aros_common::media_receipt::{
    MediaBuildFile, MediaBuildIdentity, MediaBuildReceipt, MediaBuildTree, MediaReceiptOrigin,
};
use aros_common::media_tree::measure_media_tree;
use aros_common::{sha256_bytes, toolchain_tree_inventory, ArosToolchainManifest, Sha256Digest};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
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

#[allow(clippy::literal_string_with_formatting_args)] // Git's ^{tree} syntax is literal.
fn iso_inputs(root: &Path) -> (PathBuf, PathBuf) {
    let sys = root.join("SYS");
    fs::create_dir_all(sys.join("Docs/empty")).unwrap();
    fs::write(sys.join("Docs/readme"), b"readme").unwrap();
    let profile = built_in_media_profiles()
        .unwrap()
        .into_iter()
        .find(|entry| entry.profile.id == "pc-bios-iso")
        .unwrap()
        .profile;
    let files = profile
        .required_files
        .iter()
        .map(|required| {
            let path = format!("SYS/{}", required.destination);
            let bytes = vec![0x55; 4096];
            fs::create_dir_all(root.join(&path).parent().unwrap()).unwrap();
            fs::write(root.join(&path), &bytes).unwrap();
            MediaBuildFile {
                role: required.role.clone(),
                path,
                sha256: sha256_bytes(&bytes).to_string(),
                size_bytes: bytes.len() as u64,
            }
        })
        .collect();
    let source = root.parent().unwrap().join("git-source");
    let toolchain = root.parent().unwrap().join("toolchain");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&toolchain).unwrap();
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    fs::write(source.join("source.txt"), b"source").unwrap();
    git(&["add", "source.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "source",
    ]);
    let source_commit = git(&["rev-parse", "HEAD"]);
    fs::write(toolchain.join("clang"), b"compiler").unwrap();
    let (tree_sha256, installed_files) = toolchain_tree_inventory(&toolchain).unwrap();
    let manifest = ArosToolchainManifest {
        schema: 1,
        release_id: "test-iso".into(),
        host: "linux-x86_64".into(),
        target_profile: "pc-x86_64".into(),
        target_triple: "x86_64-unknown-aros".into(),
        tree_sha256: tree_sha256.clone(),
        llvm_version: Some("1.2.3".into()),
        recipe_sha256: "1".repeat(64),
        source_lock_sha256: "2".repeat(64),
        profiles_sha256: "3".repeat(64),
        source_commit: source_commit.clone(),
        producer_commit: source_commit.clone(),
        tools_commit: source_commit.clone(),
        source_date_epoch: 1,
        capabilities: vec!["compiler".into()],
        build_environment: serde_json::Map::new(),
        files: installed_files,
    };
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(toolchain.join("toolchain-manifest.json"), &manifest_bytes).unwrap();
    let tree = measure_media_tree(&sys).unwrap();
    let receipt = MediaBuildReceipt::new_bound_cmake_with_trees(
        profile.target_preset,
        profile.model,
        profile.transport,
        MediaBuildIdentity {
            source_commit,
            source_tree: git(&["rev-parse", "HEAD^{tree}"]),
            toolchain_release_id: manifest.release_id,
            toolchain_tree_sha256: Sha256Digest::parse(&tree_sha256).unwrap(),
            toolchain_manifest_sha256: sha256_bytes(&manifest_bytes),
        },
        files,
        vec![MediaBuildTree {
            role: "sys-tree".into(),
            path: "SYS".into(),
            sha256: tree.sha256,
            file_count: tree.files.len(),
            directory_count: tree.directories.len(),
        }],
    )
    .unwrap();
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    (source, toolchain)
}

#[test]
fn iso_cli_plans_composes_and_readback_verifies_without_device_write() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("iso-inputs");
    let artifact = temporary.path().join("iso-artifact");
    let (source, toolchain) = iso_inputs(&root);
    let command = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_aros"))
            .args([
                "image",
                "build",
                "--profile",
                "pc-bios-iso",
                "--build-root",
                root.to_str().unwrap(),
                "--receipt",
                root.join("receipt.json").to_str().unwrap(),
                "--source-root",
                source.to_str().unwrap(),
                "--toolchain-root",
                toolchain.to_str().unwrap(),
                "--output",
                artifact.to_str().unwrap(),
                "--format",
                "json",
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let preview = success_json(&command(&[]));
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["file_count"], 3);
    assert!(!artifact.exists());
    let built = success_json(&command(&["--apply"]));
    assert_eq!(built["device_written"], false);
    assert_eq!(built["boot_qualified"], false);
    assert!(artifact.join("aros-media.iso").is_file());
    let verified = success_json(
        &Command::new(env!("CARGO_BIN_EXE_aros"))
            .args([
                "image",
                "verify",
                "--artifact",
                artifact.to_str().unwrap(),
                "--format",
                "json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(verified["format"], "iso9660-el-torito");
    assert_eq!(verified["file_count"], 4);
    assert_eq!(verified["image_sha256"], built["image_sha256"]);
    let refused = command(&["--apply"]);
    assert!(!refused.status.success());
    fs::write(root.join("SYS/Docs/readme"), b"changed").unwrap();
    let new_output = temporary.path().join("never-created");
    let changed = Command::new(env!("CARGO_BIN_EXE_aros"))
        .args([
            "image",
            "build",
            "--profile",
            "pc-bios-iso",
            "--build-root",
            root.to_str().unwrap(),
            "--receipt",
            root.join("receipt.json").to_str().unwrap(),
            "--source-root",
            source.to_str().unwrap(),
            "--toolchain-root",
            toolchain.to_str().unwrap(),
            "--output",
            new_output.to_str().unwrap(),
            "--apply",
            "--format",
            "json",
            "--diagnostic-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!changed.status.success());
    let diagnostics: Value = serde_json::from_slice(&changed.stderr).unwrap();
    assert!(diagnostics.to_string().contains("image.build"));
    assert!(!new_output.exists());
    assert!(artifact.join("aros-media.iso").is_file());
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

#[allow(clippy::literal_string_with_formatting_args)] // Git's ^{tree} syntax is literal.
#[test]
fn image_build_binds_v2_source_and_toolchain_before_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("inputs");
    let source = temporary.path().join("source");
    let toolchain = temporary.path().join("toolchain");
    let artifact = temporary.path().join("artifact");
    inputs(&root);
    fs::create_dir(&source).unwrap();
    fs::create_dir(&toolchain).unwrap();
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    fs::write(source.join("source.txt"), b"source").unwrap();
    git(&["add", "source.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "source",
    ]);
    let source_commit = git(&["rev-parse", "HEAD"]);
    fs::write(toolchain.join("clang"), b"compiler").unwrap();
    let (tree_sha256, files) = toolchain_tree_inventory(&toolchain).unwrap();
    let manifest = ArosToolchainManifest {
        schema: 1,
        release_id: "test-1".into(),
        host: "linux-x86_64".into(),
        target_profile: "rpi-aarch64".into(),
        target_triple: "aarch64-unknown-aros".into(),
        tree_sha256: tree_sha256.clone(),
        llvm_version: Some("1.2.3".into()),
        recipe_sha256: "1".repeat(64),
        source_lock_sha256: "2".repeat(64),
        profiles_sha256: "3".repeat(64),
        source_commit: source_commit.clone(),
        producer_commit: source_commit.clone(),
        tools_commit: source_commit.clone(),
        source_date_epoch: 1,
        capabilities: vec!["compiler".into()],
        build_environment: serde_json::Map::new(),
        files,
    };
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(toolchain.join("toolchain-manifest.json"), &manifest_bytes).unwrap();
    let prior: MediaBuildReceipt =
        serde_json::from_slice(&fs::read(root.join("receipt.json")).unwrap()).unwrap();
    let bound = MediaBuildReceipt::new_bound_cmake(
        prior.target_preset,
        prior.model,
        prior.transport,
        MediaBuildIdentity {
            source_commit,
            source_tree: git(&["rev-parse", "HEAD^{tree}"]),
            toolchain_release_id: manifest.release_id,
            toolchain_tree_sha256: Sha256Digest::parse(&tree_sha256).unwrap(),
            toolchain_manifest_sha256: sha256_bytes(&manifest_bytes),
        },
        prior.files,
    )
    .unwrap();
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec(&bound).unwrap(),
    )
    .unwrap();

    let missing_roots = invoke(&root, &artifact, &["--apply"]);
    assert!(!missing_roots.status.success());
    assert!(!artifact.exists());
    let root_args = [
        "--source-root",
        source.to_str().unwrap(),
        "--toolchain-root",
        toolchain.to_str().unwrap(),
    ];
    let plan = success_json(&invoke(&root, &artifact, &root_args));
    assert_eq!(
        plan["build_identity"]["source_commit"],
        bound.build_identity.as_ref().unwrap().source_commit
    );
    assert!(!artifact.exists());
    fs::write(source.join("source.txt"), b"dirty").unwrap();
    let changed = invoke(&root, &artifact, &root_args);
    assert!(!changed.status.success());
    assert!(!artifact.exists());
    fs::write(source.join("source.txt"), b"source").unwrap();
    let mut apply_args = root_args.to_vec();
    apply_args.push("--apply");
    let built = success_json(&invoke(&root, &artifact, &apply_args));
    assert_eq!(
        built["build_identity"]["source_commit"],
        bound.build_identity.unwrap().source_commit
    );
    let verified = success_json(
        &Command::new(env!("CARGO_BIN_EXE_aros"))
            .args([
                "image",
                "verify",
                "--artifact",
                artifact.to_str().unwrap(),
                "--format",
                "json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(verified["build_identity"], built["build_identity"]);
}
