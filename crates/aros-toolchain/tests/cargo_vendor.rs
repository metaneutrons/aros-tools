//! Cargo vendor generations are isolated, validated and safe to consume offline.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aros_cache::{acquire_write_lease, CacheLifecycleError};
use aros_common::{sha256_bytes, CancellationToken};
use aros_toolchain::cargo_vendor::{
    fetch_vendor_generation, open_verified_vendor_generation, retain_vendor_generation,
    select_vendor_generation, select_vendor_lifecycle_object, verify_vendor_generation,
    CargoVendorEnvironment, CargoVendorRequest,
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

struct VendorGenerationFixture {
    _temporary: tempfile::TempDir,
    producer: PathBuf,
    tools: PathBuf,
    cache: PathBuf,
    cargo: PathBuf,
}

impl VendorGenerationFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let producer = root.join("producer");
        let tools = root.join("tools");
        let cache = root.join("cache");
        let bin = root.join("bin");
        fs::create_dir_all(producer.join("toolchains")).unwrap();
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::write(
            producer.join("toolchains/rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.96.1\"\nprofile = \"minimal\"\n",
        )
        .unwrap();
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
        let cargo = bin.join("cargo");
        write_executable(&cargo, &fixture_cargo_script("", "", true));
        Self {
            _temporary: temporary,
            producer,
            tools,
            cache,
            cargo,
        }
    }

    fn request(&self) -> CargoVendorRequest {
        CargoVendorRequest {
            producer_dir: self.producer.clone(),
            tools_dir: self.tools.clone(),
            tools_tree: None,
            cargo: self.cargo.clone(),
            cache_dir: self.cache.clone(),
        }
    }

    fn generation(&self) -> PathBuf {
        let selection = select_vendor_generation(&self.request()).unwrap();
        self.cache.join("cargo/v1").join(selection.generation)
    }

    fn set_cargo_script(&self, prologue: &str, configuration_suffix: &str, populate: bool) {
        write_executable(
            &self.cargo,
            &fixture_cargo_script(prologue, configuration_suffix, populate),
        );
    }

    fn replace_lock(&self, lock: &str) {
        fs::write(self.tools.join("Cargo.lock"), lock).unwrap();
        git(&self.tools, &["add", "Cargo.lock"]);
        git(&self.tools, &["commit", "-qm", "test: change Cargo lock"]);
    }

    fn replace_manifest(&self, manifest: &str) {
        fs::write(self.tools.join("Cargo.toml"), manifest).unwrap();
        git(&self.tools, &["add", "Cargo.toml"]);
        git(
            &self.tools,
            &["commit", "-qm", "test: change Cargo manifest"],
        );
    }
}

fn fixture_cargo_script(prologue: &str, configuration_suffix: &str, populate: bool) -> String {
    let package_manifest = b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\n";
    let package_source = b"pub fn value() {}\n";
    let payload = if populate {
        format!(
            r#"\
    /bin/mkdir -p "$vendor/fixture-dependency-1.0.0/src"
    printf '%s' '[package]
name = "fixture-dependency"
version = "1.0.0"
' > "$vendor/fixture-dependency-1.0.0/Cargo.toml"
    printf '%s' 'pub fn value() {{}}
' > "$vendor/fixture-dependency-1.0.0/src/lib.rs"
    printf '%s' '{{"files":{{"Cargo.toml":"{}","src/lib.rs":"{}"}},"package":"{}"}}' > "$vendor/fixture-dependency-1.0.0/.cargo-checksum.json"
"#,
            sha256_bytes(package_manifest),
            sha256_bytes(package_source),
            PACKAGE_CHECKSUM,
        )
    } else {
        String::new()
    };
    format!(
        r#"#!/bin/sh
set -eu
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
    {prologue}
    vendor=
    for argument in "$@"; do vendor="$argument"; done
{payload}    printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '[source.vendored-sources]' "directory = \"$vendor\""
    {configuration_suffix}
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
}

#[test]
fn cancellation_never_publishes_a_partial_vendor_generation() {
    let fixture = VendorGenerationFixture::new();
    let marker = fixture.cache.join("vendor-started");
    fixture.set_cargo_script(
        &format!(": > {}\n    /bin/sleep 5", shell_quote(&marker)),
        "",
        true,
    );
    let generation = fixture.generation();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let operation_cancellation = cancellation.clone();
    let operation =
        thread::spawn(move || fetch_vendor_generation(&request, false, &operation_cancellation));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.is_file(), "fixture Cargo process did not start");
    cancellation.cancel();
    let error = operation.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(!generation.exists(), "cancelled generation was published");
    assert!(verify_vendor_generation(&fixture.request()).is_err());
}

#[test]
fn cooperating_fetchers_publish_one_complete_vendor_generation() {
    let fixture = VendorGenerationFixture::new();
    let calls = fixture.cache.join("vendor-calls");
    fixture.set_cargo_script(
        &format!(
            "printf '%s\\n' vendor >> {}\n    /bin/sleep 1",
            shell_quote(&calls)
        ),
        "",
        true,
    );
    let first_request = fixture.request();
    let second_request = fixture.request();
    let first = thread::spawn(move || {
        fetch_vendor_generation(&first_request, false, &CancellationToken::default())
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !calls.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(calls.is_file(), "first fixture Cargo process did not start");
    let second =
        fetch_vendor_generation(&second_request, false, &CancellationToken::default()).unwrap();
    let first = first.join().unwrap().unwrap();
    assert_eq!(first.generation_dir, second.generation_dir);
    assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 1);
    assert_eq!(
        verify_vendor_generation(&fixture.request())
            .unwrap()
            .vendor_tree_sha256,
        first.vendor_tree_sha256
    );
}

#[test]
fn verified_vendor_consumption_blocks_lifecycle_mutation_until_the_reader_releases() {
    let fixture = VendorGenerationFixture::new();
    let request = fixture.request();
    fetch_vendor_generation(&request, false, &CancellationToken::default()).unwrap();
    let (_, object) = select_vendor_lifecycle_object(&request).unwrap();

    let leased = open_verified_vendor_generation(&request).unwrap();
    assert_eq!(leased.generation().generation_dir, fixture.generation());
    assert!(matches!(
        acquire_write_lease(&object),
        Err(CacheLifecycleError::Io {
            action: "acquire lifecycle lease",
            ..
        })
    ));
    assert!(retain_vendor_generation(&request, "release-candidate").is_err());
    drop(leased);

    let (selection, retention) = retain_vendor_generation(&request, "release-candidate").unwrap();
    assert_eq!(retention.objects.len(), 1);
    assert_eq!(
        retention.objects[0].relative_path,
        "cargo/v1/".to_owned() + &selection.generation
    );
}

#[test]
fn verification_rejects_changed_tools_source_lock_or_cargo_identity() {
    let source = VendorGenerationFixture::new();
    fetch_vendor_generation(&source.request(), false, &CancellationToken::default()).unwrap();
    source.replace_manifest(
        "[package]\nname = \"fixture-tools\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
    );
    assert!(verify_vendor_generation(&source.request()).is_err());

    let lock = VendorGenerationFixture::new();
    fetch_vendor_generation(&lock.request(), false, &CancellationToken::default()).unwrap();
    lock.replace_lock(&format!(
        "version = 4\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{PACKAGE_CHECKSUM}\"\n"
    ));
    assert!(verify_vendor_generation(&lock.request()).is_err());

    let cargo = VendorGenerationFixture::new();
    fetch_vendor_generation(&cargo.request(), false, &CancellationToken::default()).unwrap();
    cargo.set_cargo_script("# changed selected Cargo bytes", "", true);
    assert!(verify_vendor_generation(&cargo.request()).is_err());
}

#[test]
fn fetch_rejects_a_missing_git_dependency_and_unsupported_configuration() {
    let git_dependency = VendorGenerationFixture::new();
    git_dependency.replace_lock(
        "version = 4\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nsource = \"git+https://example.invalid/fixture#0123456789abcdef0123456789abcdef01234567\"\n",
    );
    git_dependency.set_cargo_script("", "", false);
    let generation = git_dependency.generation();
    assert!(fetch_vendor_generation(
        &git_dependency.request(),
        false,
        &CancellationToken::default(),
    )
    .is_err());
    assert!(!generation.exists());

    let unsupported = VendorGenerationFixture::new();
    unsupported.set_cargo_script(
        "",
        "printf '%s\\n' '[source.untrusted]' 'registry = \"https://example.invalid/index\"'",
        true,
    );
    let generation = unsupported.generation();
    assert!(
        fetch_vendor_generation(&unsupported.request(), false, &CancellationToken::default(),)
            .is_err()
    );
    assert!(!generation.exists());
}

fn shell_quote(path: &Path) -> String {
    format!(
        "'{}'",
        path.display().to_string().replace('\'', "'\\\"'\\\"'")
    )
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
