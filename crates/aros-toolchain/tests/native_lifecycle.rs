//! A complete synthetic native lifecycle proves orchestration, not a compiler release.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::{sha256_bytes, CancellationToken};
use aros_toolchain::canonical;
use aros_toolchain::executor::{self, BuildRequest};
use aros_toolchain::plan::Backend;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use tar::Builder;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    recipe: PathBuf,
    bridge: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("native-lifecycle");
        fs::create_dir(&root).unwrap();
        for name in ["source", "producer", "tools"] {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            git(&path, &["init", "-q", "--template="]);
        }
        let cache = root.join("cache");
        fs::create_dir(&cache).unwrap();

        let patch_path = "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff";
        fs::create_dir_all(root.join("source/tools/crosstools/llvm")).unwrap();
        fs::create_dir_all(root.join("source/scripts")).unwrap();
        fs::write(root.join("source").join(patch_path), b"fixture patch\n").unwrap();
        write_executable(
            &root.join("source/configure"),
            r#"#!/bin/sh
set -eu
prefix=
cache=
for arg in "$@"; do
  case "$arg" in
    --with-aros-toolchain-install=*) prefix=${arg#*=} ;;
    --with-portssources=*) cache=${arg#*=} ;;
  esac
done
test -n "$prefix"
test -n "$cache"
printf 'crosstools-release:\n\t@$(FETCH) -a llvm-11.0.0.src -s tar.xz -l %s\n\t@mkdir -p %s/bin %s/lib/cmake/llvm\n\t@printf compiler > %s/bin/clang\n\t@chmod 755 %s/bin/clang\n\t@printf producer-only > %s/bin/llvm-config\n' "$cache" "$prefix" "$prefix" "$prefix" "$prefix" "$prefix" > Makefile
"#,
        );
        write_executable(
            &root.join("source/scripts/fetch.sh"),
            "#!/bin/sh\nset -eu\nexit 0\n",
        );

        let source_payload = b"x";
        fs::write(cache.join("llvm-11.0.0.src.tar.xz"), source_payload).unwrap();
        let mako = python_archive(
            "mako",
            &[
                ("mako/__init__.py", b"__version__ = '1.3.10'\n"),
                (
                    "mako/template.py",
                    b"class Template:\n    def __init__(self, text): self.text = text\n    def render(self): return self.text\n",
                ),
            ],
        );
        let markupsafe = python_archive(
            "markupsafe",
            &[("markupsafe/__init__.py", b"__version__ = '3.0.2'\n")],
        );
        fs::write(cache.join("mako.tar.gz"), &mako).unwrap();
        fs::write(cache.join("markupsafe.tar.gz"), &markupsafe).unwrap();
        let vendor = cache.join("cargo-vendor/fixture-dependency-1.0.0");
        fs::create_dir_all(vendor.join("src")).unwrap();
        let vendor_manifest =
            b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nedition = \"2021\"\n";
        let vendor_source = b"pub fn answer() -> u8 { 42 }\n";
        fs::write(vendor.join("Cargo.toml"), vendor_manifest).unwrap();
        fs::write(vendor.join("src/lib.rs"), vendor_source).unwrap();
        let package_checksum = "d".repeat(64);
        fs::write(
            vendor.join(".cargo-checksum.json"),
            serde_json::to_vec(&json!({
                "package": package_checksum,
                "files": {
                    "Cargo.toml": sha256_bytes(vendor_manifest),
                    "src/lib.rs": sha256_bytes(vendor_source)
                }
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            cache.join("cargo-vendor-config.toml"),
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"__CARGO_VENDOR_DIRECTORY__\"\n",
        )
        .unwrap();

        fs::create_dir_all(root.join("tools/contracts")).unwrap();
        fs::create_dir_all(root.join("tools/src")).unwrap();
        let contract = b"native lifecycle fixture contract\n";
        fs::write(
            root.join("tools/contracts/toolchain-producer-v1.toml"),
            contract,
        )
        .unwrap();
        fs::write(
            root.join("tools/Cargo.toml"),
            "[package]\nname = \"aros-collect\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[dependencies]\nfixture-dependency = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("tools/Cargo.lock"), format!("version = 4\n\n[[package]]\nname = \"aros-collect\"\nversion = \"0.0.0\"\ndependencies = [\"fixture-dependency\"]\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{package_checksum}\"\n")).unwrap();
        fs::write(
            root.join("tools/src/main.rs"),
            "fn main() { assert_eq!(fixture_dependency::answer(), 42); }\n",
        )
        .unwrap();
        git(&root.join("tools"), &["add", "."]);
        git(
            &root.join("tools"),
            &["commit", "-qm", "test: native lifecycle tools"],
        );
        let tools_commit = git(&root.join("tools"), &["rev-parse", "HEAD"]);

        let lock = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "patch": patch_path, "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": sha256_bytes(source_payload), "size": source_payload.len()
            }],
            "host_python_packages": [
                {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(&mako), "size": mako.len(), "source_root": "mako", "python_path": "."},
                {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": sha256_bytes(&markupsafe), "size": markupsafe.len(), "source_root": "markupsafe", "python_path": "."}
            ]
        }))
        .unwrap();
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap();
        fs::create_dir_all(root.join("producer/toolchains")).unwrap();
        fs::write(root.join("producer/toolchains/fixture.sources.json"), &lock).unwrap();
        fs::write(root.join("producer/toolchains/profiles-v1.json"), &profiles).unwrap();
        fs::write(
            root.join("producer/toolchains/producer-executor-v1.toml"),
            format!(
                "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/fixture.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
                sha256_bytes(contract), tools_commit
            ),
        )
        .unwrap();

        for name in ["source", "producer"] {
            git(&root.join(name), &["add", "."]);
            git(
                &root.join(name),
                &["commit", "-qm", "test: native lifecycle inputs"],
            );
        }
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": git(&root.join("source"), &["rev-parse", "HEAD"]),
            "source_tree": git(&root.join("source"), &["rev-parse", "HEAD^{tree}"]),
            "producer_commit": git(&root.join("producer"), &["rev-parse", "HEAD"]),
            "producer_tree": git(&root.join("producer"), &["rev-parse", "HEAD^{tree}"]),
            "tools_commit": git(&root.join("tools"), &["rev-parse", "HEAD"]),
            "tools_tree": git(&root.join("tools"), &["rev-parse", "HEAD^{tree}"]),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(&lock),
            "profiles_sha256": sha256_bytes(&profiles),
            "patches": [{"path": patch_path, "sha256": sha256_bytes(b"fixture patch\n")}]
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        let recipe_path = root.join("recipe.json");
        fs::write(&recipe_path, serde_json::to_vec(&recipe).unwrap()).unwrap();

        let bridge = root.join("bridge");
        write_executable(
            &bridge,
            r#"#!/bin/sh
set -eu
test "$1" = toolchain
test "$2" = __metamake-fetch
printf '%s\n' llvm-11.0.0.src.tar.xz >> "$AROS_TOOLCHAIN_FETCH_LEDGER"
exec /bin/bash "$AROS_TOOLCHAIN_FETCH_UPSTREAM" "$@"
"#,
        );
        Self {
            _temporary: temporary,
            root,
            recipe: recipe_path,
            bridge,
        }
    }

    fn request(&self) -> BuildRequest {
        BuildRequest {
            backend: Backend::Native,
            preset: "pc-x86_64".into(),
            recipe: self.recipe.clone(),
            source_dir: self.root.join("source"),
            producer_dir: self.root.join("producer"),
            tools_dir: self.root.join("tools"),
            work_dir: self.root.join("work"),
            output_dir: self.root.join("output"),
            cache_dir: self.root.join("cache"),
            jobs: 1,
            timeout_seconds: 120,
            offline: true,
            release_id: "native-fixture".into(),
            fetch_bridge: Some(self.bridge.clone()),
        }
    }
}

#[test]
fn native_lifecycle_runs_configure_compiler_and_collector_with_receipt_chain() {
    let fixture = Fixture::new();
    let result =
        executor::run(&fixture.request(), &CancellationToken::default()).unwrap_or_else(|error| {
            let log_root = &fixture.root;
            let logs = ["configure", "compiler", "collector"]
                .into_iter()
                .flat_map(|phase| {
                    ["stdout", "stderr"].into_iter().map(move |stream| {
                        let path = log_root
                            .join(format!("work/native-lifecycle/logs/{phase}.{stream}.log"));
                        let content = fs::read_to_string(path)
                            .unwrap_or_else(|_| format!("<no retained {stream} log>"));
                        format!("{phase} {stream}:\n{content}")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            panic!("{error}\n{logs}");
        });
    assert_eq!(result.backend, Backend::Native);
    assert_eq!(result.qualification, "local-only");
    assert_eq!(result.commit_state, "committed");
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].path, "toolchain/bin/aros-collect");
    let public_result = serde_json::to_value(&result).unwrap();
    assert!(public_result.get("environment").is_none());
    let prefix = fixture.root.join("output/toolchain");
    assert!(prefix.join("bin/clang").is_file());
    assert!(prefix.join("bin/aros-collect").is_file());
    assert!(prefix.join("bin/collect-aros").is_symlink());
    assert!(prefix.join("bin/collect-aros32").is_symlink());
    assert!(!prefix.join("bin/llvm-config").exists());
    assert!(!prefix.join("lib/cmake/llvm").exists());
    let mut previous: Option<String> = None;
    for phase in [
        "preflight",
        "environment",
        "configure",
        "compiler",
        "collector",
    ] {
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .join(format!("work/native-lifecycle/receipts/{phase}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            receipt["previous_receipt_sha256"].as_str(),
            previous.as_deref(),
            "{phase} receipt must bind its exact predecessor"
        );
        let expected = receipt["receipt_sha256"].as_str().unwrap().to_owned();
        let mut unsigned = receipt;
        unsigned.as_object_mut().unwrap().remove("receipt_sha256");
        assert_eq!(
            sha256_bytes(&canonical::bytes(&unsigned).unwrap()).as_str(),
            expected
        );
        previous = Some(expected);
    }
    let usage = fs::read_to_string(
        fixture
            .root
            .join("work/native-lifecycle/verified-source-usage.log"),
    )
    .unwrap();
    assert_eq!(usage, "llvm-11.0.0.src.tar.xz\n");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn python_archive(root: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut archive = Builder::new(&mut encoder);
        for (relative, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            archive
                .append_data(&mut header, format!("{root}/{relative}"), *content)
                .unwrap();
        }
        archive.finish().unwrap();
    }
    encoder.finish().unwrap()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Native lifecycle fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Native lifecycle fixture")
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
