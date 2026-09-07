//! The vendor tree is snapshotted and checked before Cargo could execute.

#![cfg(unix)]

use std::fs;

use aros_common::sha256_bytes;
use aros_toolchain::cargo_vendor::CargoVendorEnvironment;
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
