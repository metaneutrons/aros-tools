//! Black-box contracts for project-scoped release-lock selection.

use aros_common::{ArosToolchainArtifact, ArosToolchainLock};
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

fn diagnostic_json(output: &Output) -> Value {
    assert!(!output.status.success());
    serde_json::from_slice(&output.stderr).unwrap()
}

fn lock(release_id: &str) -> ArosToolchainLock {
    ArosToolchainLock {
        schema: 1,
        release_id: release_id.into(),
        base_url: Some(format!("https://example.invalid/releases/{release_id}")),
        artifacts: vec![ArosToolchainArtifact {
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            asset: format!("aros-toolchain-{release_id}.tar.xz"),
            sha256: "a".repeat(64),
            tree_sha256: "b".repeat(64),
            llvm_version: Some("11.0.0".into()),
            size: Some(1),
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: vec!["bin/clang".into()],
        }],
    }
}

fn write_lock(path: &Path, lock: &ArosToolchainLock) {
    fs::write(path, toml::to_string(lock).unwrap()).unwrap();
}

fn checkout(root: &Path) {
    for directory in ["arch", "compiler", "rom"] {
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

#[test]
fn select_preview_then_apply_publishes_the_project_lock_and_derived_reference() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    write_lock(
        &project.join("aros-toolchains.lock.toml"),
        &lock("old-release"),
    );
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(preview["schema"], "aros-toolchain-selection-v1");
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["old_release_id"], "old-release");
    assert_eq!(preview["new_release_id"], "new-release");
    let token = preview["apply_token"].as_str().unwrap();

    let committed = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .args(["--apply", token])
            .output()
            .unwrap(),
    );
    assert_eq!(committed["state"], "committed");
    assert_eq!(committed["new_release_id"], "new-release");
    let reference = committed["project_reference"].as_str().unwrap();
    let reference: Value = serde_json::from_slice(&fs::read(reference).unwrap()).unwrap();
    assert_eq!(reference["schema"], "aros-toolchain-project-reference-v1");
    assert_eq!(reference["release_id"], "new-release");
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(
            &fs::read_to_string(project.join("aros-toolchains.lock.toml")).unwrap()
        )
        .unwrap()
        .release_id,
        "new-release"
    );
}

#[test]
fn stale_select_preview_never_overwrites_a_changed_project_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    let destination = project.join("aros-toolchains.lock.toml");
    write_lock(&destination, &lock("old-release"));
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    write_lock(&destination, &lock("concurrent-release"));

    let stale_apply = command(&project)
        .args(["toolchain", "select", "--format", "json", "--release-lock"])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    assert!(!stale_apply.status.success());
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(&fs::read_to_string(destination).unwrap())
            .unwrap()
            .release_id,
        "concurrent-release"
    );
}

#[test]
fn stale_select_preview_never_accepts_a_new_project_reference() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    let destination = project.join("aros-toolchains.lock.toml");
    write_lock(&destination, &lock("old-release"));
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    let reference = preview["project_reference"].as_str().unwrap();
    let receipt = serde_json::json!({
        "schema": "aros-toolchain-project-reference-v1",
        "project": fs::canonicalize(&project).unwrap().display().to_string(),
        "project_lock": fs::canonicalize(&destination).unwrap().display().to_string(),
        "release_id": "old-release",
        "lock_sha256": aros_common::sha256_bytes(&fs::read(&destination).unwrap()).to_string(),
    });
    fs::create_dir_all(Path::new(reference).parent().unwrap()).unwrap();
    fs::write(reference, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let stale_apply = command(&project)
        .args(["toolchain", "select", "--format", "json", "--release-lock"])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    assert!(!stale_apply.status.success());
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(&fs::read_to_string(destination).unwrap())
            .unwrap()
            .release_id,
        "old-release"
    );
}

#[test]
fn stale_select_preview_never_accepts_a_replaced_candidate_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    let destination = project.join("aros-toolchains.lock.toml");
    write_lock(&destination, &lock("old-release"));
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    write_lock(&candidate, &lock("replacement-release"));

    let stale_apply = command(&project)
        .args(["toolchain", "select", "--format", "json", "--release-lock"])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    assert!(!stale_apply.status.success());
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(&fs::read_to_string(destination).unwrap())
            .unwrap()
            .release_id,
        "old-release"
    );
}

#[cfg(unix)]
#[test]
fn select_refuses_a_symlinked_candidate_lock() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let target = temporary.path().join("candidate-target.toml");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    write_lock(&target, &lock("new-release"));
    symlink(&target, &candidate).unwrap();

    let output = command(&project)
        .args(["toolchain", "select", "--format", "json", "--release-lock"])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!project.join("aros-toolchains.lock.toml").exists());
    assert!(!store.exists());
}

#[cfg(unix)]
#[test]
fn select_reports_post_rename_uncertainty_and_retains_the_complete_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    let output = command(&project)
        .env(
            "AROS_PUBLICATION_TEST_FAIL_AT",
            "file-after-rename-before-sync",
        )
        .args([
            "--diagnostic-format=json",
            "toolchain",
            "select",
            "--format",
            "json",
            "--release-lock",
        ])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    let diagnostic = diagnostic_json(&output);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "indeterminate"
    );
    assert!(
        diagnostic["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("complete target retained"),
        "{diagnostic}"
    );
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(
            &fs::read_to_string(project.join("aros-toolchains.lock.toml")).unwrap()
        )
        .unwrap()
        .release_id,
        "new-release"
    );
}

#[cfg(unix)]
#[test]
fn select_preserves_the_previous_lock_on_a_prepublication_storage_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    let destination = project.join("aros-toolchains.lock.toml");
    write_lock(&destination, &lock("old-release"));
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    let output = command(&project)
        .env("AROS_PUBLICATION_TEST_FAIL_AT", "stage-before-write")
        .args([
            "--diagnostic-format=json",
            "toolchain",
            "select",
            "--format",
            "json",
            "--release-lock",
        ])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    let diagnostic = diagnostic_json(&output);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert_eq!(
        toml::from_str::<ArosToolchainLock>(&fs::read_to_string(destination).unwrap())
            .unwrap()
            .release_id,
        "old-release"
    );
}

#[cfg(unix)]
#[test]
fn select_refuses_a_read_only_project_before_publishing_any_lock() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let candidate = temporary.path().join("candidate.toml");
    let store = temporary.path().join("store");
    checkout(&project);
    write_lock(&candidate, &lock("new-release"));

    let preview = output_json(
        &command(&project)
            .args(["toolchain", "select", "--format", "json", "--release-lock"])
            .arg(&candidate)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap(),
    );
    let token = preview["apply_token"].as_str().unwrap();
    let original_permissions = fs::metadata(&project).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_mode(0o500);
    fs::set_permissions(&project, read_only).unwrap();
    let output = command(&project)
        .args([
            "--diagnostic-format=json",
            "toolchain",
            "select",
            "--format",
            "json",
            "--release-lock",
        ])
        .arg(&candidate)
        .arg("--store")
        .arg(&store)
        .args(["--apply", token])
        .output()
        .unwrap();
    fs::set_permissions(&project, original_permissions).unwrap();

    let diagnostic = diagnostic_json(&output);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert!(!project.join("aros-toolchains.lock.toml").exists());
}
