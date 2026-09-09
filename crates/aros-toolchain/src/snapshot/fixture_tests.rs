//! Shared source snapshot fixtures for unit and integration tests.

use super::{canonical, PlanRequest, Recipe, RunDirectories};
use aros_common::{sha256_bytes, CancellationToken};
use serde_json::json;
use std::os::unix::fs::PermissionsExt as _;
use std::{fs, path::Path, process::Command, time::Duration};

pub const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Fixture {
    pub(crate) _temporary: tempfile::TempDir,
    pub(crate) request: PlanRequest,
    pub(crate) token: CancellationToken,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for name in ["source", "producer", "tools"] {
            let checkout = root.join(name);
            fs::create_dir(&checkout).unwrap();
            git(&checkout, &["init", "-q"]);
            fs::create_dir(checkout.join("dir")).unwrap();
            fs::write(checkout.join("dir/Größe file"), b"committed\0bytes\n").unwrap();
            fs::write(checkout.join("empty"), b"").unwrap();
            fs::write(checkout.join("run"), b"#!/bin/sh\nexit 98\n").unwrap();
            fs::set_permissions(checkout.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
            git(&checkout, &["add", "."]);
            git(&checkout, &["commit", "-qm", "test: snapshot fixture"]);
        }
        Self {
            request: PlanRequest {
                preset: "fixture".into(),
                recipe: root.join("unused-recipe-path"),
                source_dir: root.join("source"),
                producer_dir: root.join("producer"),
                tools_dir: root.join("tools"),
                work_dir: Some(root.join("work")),
                output_dir: Some(root.join("output")),
                cache_dir: Some(root.join("cache")),
                jobs: Some(1),
                timeout_seconds: Some(30),
                offline: true,
            },
            _temporary: temporary,
            token: CancellationToken::default(),
        }
    }

    pub(super) fn recipe(&self) -> Recipe {
        let mut material = json!({"schema":"aros-toolchain-recipe-v2", "source_date_epoch":0,
            "source_lock_sha256":sha256_bytes(b"not a semantic source lock"),
            "profiles_sha256":sha256_bytes(b"not a profile validation"), "patches":[]});
        for (name, checkout) in [
            ("source", &self.request.source_dir),
            ("producer", &self.request.producer_dir),
            ("tools", &self.request.tools_dir),
        ] {
            material[format!("{name}_commit")] = json!(git(checkout, &["rev-parse", "HEAD"]));
            material[format!("{name}_tree")] = json!(git(checkout, &["rev-parse", "HEAD^{tree}"]));
        }
        material["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&material).unwrap()));
        Recipe::parse(&serde_json::to_vec(&material).unwrap()).unwrap()
    }

    pub(super) fn reserve(&self) -> RunDirectories {
        RunDirectories::reserve(
            &self.request,
            &sha256_bytes(b"operation fixture"),
            &self.token,
        )
        .unwrap()
    }

    pub(super) fn work(&self) -> &Path {
        self.request.work_dir.as_deref().unwrap()
    }

    pub(super) fn commit_source(&self) {
        git(&self.request.source_dir, &["add", "."]);
        git(
            &self.request.source_dir,
            &["commit", "-qm", "test: source update"],
        );
    }
}

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Source fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Source fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .current_dir(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
