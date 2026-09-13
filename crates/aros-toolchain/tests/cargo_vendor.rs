//! Cargo vendor generations are isolated, validated and safe to consume offline.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use aros_common::{sha256_bytes, CancellationToken};
use aros_toolchain::cargo_vendor::{
    fetch_vendor_generation, verify_vendor_generation, CargoVendorEnvironment, CargoVendorRequest,
};
use serde_json::json;

const PACKAGE_CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn vendor_checksum(files: &[(&str, &[u8])]) -> Vec<u8> {
    let files = files
        .iter()
        .map(|(path, contents)| {
            (
                (*path).to_owned(),
                serde_json::Value::String(sha256_bytes(contents).to_string()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::to_vec(&json!({
        "$comment": "Cargo-generated checksum metadata",
        "files": files,
        "package": PACKAGE_CHECKSUM,
    }))
    .unwrap()
}

fn cache(root: &std::path::Path) {
    fs::create_dir(root.join("cargo-vendor")).unwrap();
    let package = root.join("cargo-vendor/example-1.0.0");
    fs::create_dir(&package).unwrap();
    let cargo_toml = b"[package]\nname = \"example\"\nversion = \"1.0.0\"\n";
    let source = b"pub fn value() {}\n";
    fs::write(package.join("Cargo.toml"), cargo_toml).unwrap();
    fs::write(package.join("src.rs"), source).unwrap();
    fs::write(
        package.join(".cargo-checksum.json"),
        vendor_checksum(&[("Cargo.toml", cargo_toml), ("src.rs", source)]),
    )
    .unwrap();
    fs::write(
        root.join("cargo-vendor-config.toml"),
        "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"__CARGO_VENDOR_DIRECTORY__\"\n",
    )
    .unwrap();
    fs::write(
        root.join("Cargo.lock"),
        format!(
            "version = 4\n\n[[package]]\nname = \"example\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{PACKAGE_CHECKSUM}\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn snapshots_and_validates_a_complete_private_vendor_tree() {
    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    let environment = CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .unwrap();
    assert_eq!(environment.package_count(), 1);
    assert!(environment.vendor().join("example-1.0.0/src.rs").is_file());
    let config = fs::read_to_string(environment.config()).unwrap();
    assert!(config.contains(environment.vendor().to_string_lossy().as_ref()));
    assert!(!config.contains("__CARGO_VENDOR_DIRECTORY__"));
    let mut command = std::process::Command::new("env");
    environment.apply_to(&mut command);
    let output = command.output().unwrap();
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("CARGO_NET_OFFLINE=true"));
    assert!(output.contains("CARGO_INCREMENTAL=0"));
}

#[test]
fn rejects_a_vendor_file_that_disagrees_with_its_checksum_record() {
    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    fs::write(
        cache_root.join("cargo-vendor/example-1.0.0/src.rs"),
        b"tampered\n",
    )
    .unwrap();
    let error = CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}

#[test]
fn rejects_unknown_checksum_record_fields_after_accepting_cargo_comment() {
    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    let checksum_path = cache_root.join("cargo-vendor/example-1.0.0/.cargo-checksum.json");
    let mut checksum: serde_json::Value =
        serde_json::from_slice(&fs::read(&checksum_path).unwrap()).unwrap();
    checksum["unexpected"] = json!(true);
    fs::write(&checksum_path, serde_json::to_vec(&checksum).unwrap()).unwrap();
    let error = CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}

#[test]
fn rejects_any_template_that_can_leave_the_vendor_mapping() {
    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    fs::write(
        cache_root.join("cargo-vendor-config.toml"),
        "[source.crates-io]\nregistry = \"https://example.invalid/index\"\n\n[source.vendored-sources]\ndirectory = \"__CARGO_VENDOR_DIRECTORY__\"\n",
    )
    .unwrap();
    assert!(CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .is_err());
}

#[test]
fn rejects_a_symlink_anywhere_in_the_cache_vendor_tree() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    symlink(
        "src.rs",
        cache_root.join("cargo-vendor/example-1.0.0/linked-source.rs"),
    )
    .unwrap();
    assert!(CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .is_err());
}

#[test]
fn rejects_a_vendor_tree_that_does_not_match_the_selected_cargo_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let cache_root = temporary.path().join("cache");
    fs::create_dir(&cache_root).unwrap();
    cache(&cache_root);
    fs::write(
        cache_root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"other\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
    )
    .unwrap();
    let error = CargoVendorEnvironment::prepare(
        &cache_root.canonicalize().unwrap(),
        &cache_root.join("Cargo.lock").canonicalize().unwrap(),
        &temporary.path().join("private-vendor"),
    )
    .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}

#[test]
fn fetches_an_immutable_generation_without_inheriting_cargo_credentials() {
    if std::env::var_os("AROS_CARGO_VENDOR_CREDENTIAL_TEST_CHILD").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fetches_an_immutable_generation_without_inheriting_cargo_credentials",
                "--nocapture",
            ])
            .env("AROS_CARGO_VENDOR_CREDENTIAL_TEST_CHILD", "1")
            .env("AROS_CARGO_VENDOR_TEST_SECRET", "not-for-cargo")
            .env("CARGO_REGISTRIES_CRATES_IO_TOKEN", "not-for-cargo")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "credential-isolation child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let producer = temporary.path().join("producer");
    let tools = temporary.path().join("tools");
    let cache_root = temporary.path().join("cache");
    let bin = temporary.path().join("bin");
    fs::create_dir_all(producer.join("toolchains")).unwrap();
    fs::create_dir_all(&tools).unwrap();
    fs::create_dir(&cache_root).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::write(
        producer.join("toolchains/rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.96.1\"\nprofile = \"minimal\"\n",
    )
    .unwrap();
    let package_manifest = b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\n";
    let package_source = b"pub fn value() {}\n";
    fs::write(
        tools.join("Cargo.toml"),
        "[package]\nname = \"fixture-tools\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        tools.join("Cargo.lock"),
        format!(
            "version = 4\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{PACKAGE_CHECKSUM}\"\n"
        ),
    )
    .unwrap();
    git(&tools, &["init", "-q", "--template="]);
    git(&tools, &["add", "."]);
    git(&tools, &["commit", "-qm", "test: fixture Cargo workspace"]);

    let fake_cargo = bin.join("cargo");
    write_executable(
        &fake_cargo,
        &format!(
            r#"#!/bin/sh
set -e
if /usr/bin/env | /usr/bin/grep -q '^AROS_CARGO_VENDOR_TEST_SECRET='; then
  exit 91
fi
if /usr/bin/env | /usr/bin/grep -q '^CARGO_REGISTRIES_CRATES_IO_TOKEN='; then
  exit 92
fi
case "$1" in
  --version)
    printf '%s\n' 'cargo 1.96.1 (fixture)'
    ;;
  vendor)
    vendor=
    for argument in "$@"; do vendor="$argument"; done
    /bin/mkdir -p "$vendor/fixture-dependency-1.0.0/src"
    printf '%s' '[package]
name = "fixture-dependency"
version = "1.0.0"
' > "$vendor/fixture-dependency-1.0.0/Cargo.toml"
    printf '%s' 'pub fn value() {{}}
' > "$vendor/fixture-dependency-1.0.0/src/lib.rs"
    printf '%s' '{{"files":{{"Cargo.toml":"{}","src/lib.rs":"{}"}},"package":"{}"}}' > "$vendor/fixture-dependency-1.0.0/.cargo-checksum.json"
    printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '[source.vendored-sources]' "directory = \"$vendor\""
    ;;
  *)
    exit 64
    ;;
esac
"#,
            sha256_bytes(package_manifest),
            sha256_bytes(package_source),
            PACKAGE_CHECKSUM,
        ),
    );
    let request = CargoVendorRequest {
        producer_dir: producer,
        tools_dir: tools,
        tools_tree: None,
        cargo: fake_cargo,
        cache_dir: cache_root,
    };
    let generation =
        fetch_vendor_generation(&request, false, &CancellationToken::default()).unwrap();
    assert_eq!(generation.package_count, 1);
    assert!(generation.generation_dir.join("receipt.json").is_file());
    assert!(generation.generation_dir.join("cargo-vendor").is_dir());
    let verified = verify_vendor_generation(&request).unwrap();
    assert_eq!(verified.vendor_tree_sha256, generation.vendor_tree_sha256);
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Cargo vendor fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Cargo vendor fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
