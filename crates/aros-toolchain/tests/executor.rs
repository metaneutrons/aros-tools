//! The local adapter uses fresh isolated views and the shared process runner.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::{sha256_bytes, CancellationToken, DiagnosticCode};
use aros_toolchain::canonical;
use aros_toolchain::executor::{self, BuildRequest};
use aros_toolchain::plan::Backend;
use serde_json::json;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    recipe: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("executor fixture");
        fs::create_dir(&root).unwrap();
        for name in ["source", "producer", "tools"] {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            git(&path, &["init", "-q", "--template="]);
        }
        fs::write(root.join("source/patch.diff"), b"fixture patch\n").unwrap();
        fs::write(root.join("source/configure"), b"#!/bin/sh\nexit 0\n").unwrap();

        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1",
            "upstream_commit": "4".repeat(40),
            "profiles": [{
                "name": "pc-x86_64",
                "configure_target": "pc-x86_64",
                "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros",
                "cpu": "x86_64",
                "platform": "pc",
                "float_abi": "",
                "capabilities": ["c", "cxx"]
            }]
        }))
        .unwrap();
        let lock = br#"{"schema":"aros-toolchain-source-lock-v2"}
"#;
        fs::create_dir_all(root.join("producer/toolchains")).unwrap();
        fs::create_dir_all(root.join("producer/scripts/toolchain")).unwrap();
        fs::write(root.join("producer/toolchains/profiles-v1.json"), &profiles).unwrap();
        fs::write(root.join("producer/toolchains/fixture.sources.json"), lock).unwrap();
        let driver = br#"#!/bin/sh
set -eu
output=
while [ "$#" -gt 0 ]; do
  if [ "$1" = --output-dir ]; then output=$2; shift 2; else shift; fi
done
if [ "${AROS_TEST_SECRET-}" != "" ] || [ "${CFLAGS-}" != "" ]; then exit 41; fi
mkdir -p "$output"
printf '%s\n' 'isolated adapter output' > "$output/candidate.txt"
"#;
        let driver_path = root.join("producer/scripts/toolchain/build-release.sh");
        fs::write(&driver_path, driver).unwrap();
        fs::set_permissions(&driver_path, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("tools/README"), b"tools fixture\n").unwrap();
        for name in ["source", "producer", "tools"] {
            let path = root.join(name);
            git(&path, &["add", "."]);
            git(&path, &["commit", "-qm", "test: executor fixture"]);
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
            "source_lock_sha256": sha256_bytes(lock),
            "profiles_sha256": sha256_bytes(&profiles),
            "patches": [{"path": "patch.diff", "sha256": sha256_bytes(b"fixture patch\n")}]
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        let recipe_path = root.join("recipe.json");
        fs::write(&recipe_path, serde_json::to_vec(&recipe).unwrap()).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        Self {
            _temporary: temporary,
            root,
            recipe: recipe_path,
        }
    }

    fn request(&self) -> BuildRequest {
        BuildRequest {
            backend: Backend::LegacyPreview,
            preset: "pc-x86_64".into(),
            recipe: self.recipe.clone(),
            source_dir: self.root.join("source"),
            producer_dir: self.root.join("producer"),
            tools_dir: self.root.join("tools"),
            work_dir: self.root.join("work"),
            output_dir: self.root.join("output"),
            cache_dir: self.root.join("cache"),
            jobs: 1,
            timeout_seconds: 60,
            offline: true,
            release_id: "local-fixture".into(),
            fetch_bridge: None,
        }
    }
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Adapter fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Adapter fixture")
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

#[test]
fn successful_preview_is_local_only_and_sanitizes_the_child_environment() {
    let fixture = Fixture::new();
    std::env::set_var("AROS_TEST_SECRET", "must-not-cross-boundary");
    let result = executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    std::env::remove_var("AROS_TEST_SECRET");
    assert_eq!(result.qualification, "local-only");
    assert_eq!(result.commit_state, "committed");
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].path, "candidate.txt");
    assert!(result
        .evidence
        .iter()
        .any(|item| item.check == "origin" && item.status == "not-run"));
    assert!(result.environment.iter().any(|item| item.name == "git"));
}

#[test]
fn cancellation_before_reservation_has_no_side_effect() {
    let fixture = Fixture::new();
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let error = executor::run(&fixture.request(), &cancellation).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerState
    );
    assert!(!fixture.root.join("work").exists());
    assert!(!fixture.root.join("output").exists());
}

#[test]
fn non_offline_preview_is_rejected_before_checkout_access() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.offline = false;
    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    assert!(error.to_string().contains("offline cache"));
}
