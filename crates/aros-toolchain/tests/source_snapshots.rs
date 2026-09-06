//! Real Git/filesystem proofs for material isolation, not compiler qualification.
#![cfg(unix)]

use aros_common::{sha256_bytes, CancellationToken, DiagnosticCode};
use aros_toolchain::{
    canonical,
    plan::{Backend, PlanRequest},
    snapshot::{SourceRole, SourceSnapshot},
    workspace::RunDirectories,
    Recipe,
};
use serde_json::json;
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};
use std::{fs, path::Path, process::Command, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(30);

#[path = "source_snapshots/legacy_views.rs"]
mod legacy_views;

struct Fixture {
    _temporary: tempfile::TempDir,
    request: PlanRequest,
    token: CancellationToken,
}

impl Fixture {
    fn new() -> Self {
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
                backend: Backend::LegacyPreview,
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
    fn recipe(&self) -> Recipe {
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
    fn reserve(&self) -> RunDirectories {
        RunDirectories::reserve(
            &self.request,
            &sha256_bytes(b"operation fixture"),
            &self.token,
        )
        .unwrap()
    }
    fn work(&self) -> &Path {
        self.request.work_dir.as_deref().unwrap()
    }
    fn commit_source(&self) {
        git(&self.request.source_dir, &["add", "."]);
        git(
            &self.request.source_dir,
            &["commit", "-qm", "test: source update"],
        );
    }
}

fn git(root: &Path, args: &[&str]) -> String {
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

#[test]
fn three_roles_are_independent_metadata_free_copies_not_hardlinks() {
    let fixture = Fixture::new();
    let recipe = fixture.recipe();
    let run = fixture.reserve();
    for (role, name) in [
        (SourceRole::Source, "source"),
        (SourceRole::Producer, "producer"),
        (SourceRole::Tools, "tools"),
    ] {
        let snapshot =
            SourceSnapshot::prepare(&run, role, &recipe, TIMEOUT, &fixture.token).unwrap();
        assert_eq!(snapshot.root(), fixture.work().join(name));
        assert!(!snapshot.root().join(".git").exists());
        assert_eq!(
            fs::read(snapshot.root().join("dir/Größe file")).unwrap(),
            b"committed\0bytes\n"
        );
        assert_eq!(
            fs::metadata(snapshot.root().join("empty")).unwrap().len(),
            0
        );
        assert_eq!(
            fs::metadata(snapshot.root().join("run")).unwrap().mode() & 0o777,
            0o700
        );
        let metadata = fs::metadata(snapshot.root().join("dir/Größe file")).unwrap();
        assert_eq!(metadata.nlink(), 1);
        let original = fixture
            .request
            .source_dir
            .parent()
            .unwrap()
            .join(name)
            .join("dir/Größe file");
        assert_ne!(metadata.ino(), fs::metadata(&original).unwrap().ino());
        fs::write(original, b"later original change").unwrap();
        snapshot.revalidate(TIMEOUT, &fixture.token).unwrap();
        assert!(SourceSnapshot::prepare(&run, role, &recipe, TIMEOUT, &fixture.token).is_err());
    }
    assert!(!fixture.request.cache_dir.as_ref().unwrap().exists());
    assert_eq!(
        fs::read_dir(fixture.request.output_dir.as_ref().unwrap())
            .unwrap()
            .count(),
        1
    );
    run.release().unwrap();
    assert!(fixture.work().join("source/run").exists());
}

#[test]
fn hidden_dirty_or_untracked_material_fails_before_staging_without_filters() {
    for tracked in [false, true] {
        let fixture = Fixture::new();
        fs::write(
            fixture.request.source_dir.join(".gitattributes"),
            "* filter=sentinel\n",
        )
        .unwrap();
        fixture.commit_source();
        let recipe = fixture.recipe();
        git(
            &fixture.request.source_dir,
            &["update-index", "--assume-unchanged", "dir/Größe file"],
        );
        let marker = fixture
            .request
            .source_dir
            .parent()
            .unwrap()
            .join("filter-ran");
        git(
            &fixture.request.source_dir,
            &[
                "config",
                "filter.sentinel.clean",
                &format!("touch '{}'; cat", marker.display()),
            ],
        );
        fs::write(
            fixture.request.source_dir.join(if tracked {
                "dir/Größe file"
            } else {
                "untracked-private"
            }),
            b"changed",
        )
        .unwrap();
        let run = fixture.reserve();
        let error =
            SourceSnapshot::prepare(&run, SourceRole::Source, &recipe, TIMEOUT, &fixture.token)
                .err()
                .unwrap();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerIdentity
        );
        assert!(!error.to_string().contains("untracked-private"));
        assert!(!marker.exists());
        assert!(!fixture.work().join(".source-pending").exists());
    }
}

#[test]
fn recursive_gitlinks_are_flattened_and_internal_link_chains_are_preserved() {
    let fixture = Fixture::new();
    let source = &fixture.request.source_dir;
    git(
        source,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            fixture.request.tools_dir.to_str().unwrap(),
            "module",
        ],
    );
    let module = source.join("module");
    git(
        &module,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            fixture.request.producer_dir.to_str().unwrap(),
            "nested",
        ],
    );
    git(&module, &["commit", "-qam", "test: nested module"]);
    symlink("module/nested/dir", source.join("shortcut")).unwrap();
    symlink("shortcut/Größe file", source.join("through-chain")).unwrap();
    symlink("../empty", source.join("dir/up-link")).unwrap();
    fixture.commit_source();
    let run = fixture.reserve();
    let snapshot = SourceSnapshot::prepare(
        &run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    )
    .unwrap();
    for relative in [".git", "module/.git", "module/nested/.git"] {
        assert!(!snapshot.root().join(relative).exists());
    }
    assert_eq!(
        fs::read(snapshot.root().join("through-chain")).unwrap(),
        b"committed\0bytes\n"
    );
    assert_eq!(
        fs::read_link(snapshot.root().join("dir/up-link")).unwrap(),
        Path::new("../empty")
    );
    fs::write(module.join("nested/dir/Größe file"), b"original edited").unwrap();
    snapshot.revalidate(TIMEOUT, &fixture.token).unwrap();
}

#[test]
fn unsafe_missing_or_cyclic_links_never_publish_and_failed_material_is_retained() {
    for target in [
        "/etc/passwd",
        "../outside",
        "missing",
        "link",
        "empty/../empty",
        "dir/../../outside",
    ] {
        let fixture = Fixture::new();
        symlink(target, fixture.request.source_dir.join("link")).unwrap();
        fixture.commit_source();
        let run = fixture.reserve();
        let recipe = fixture.recipe();
        assert!(
            SourceSnapshot::prepare(&run, SourceRole::Source, &recipe, TIMEOUT, &fixture.token)
                .is_err(),
            "{target}"
        );
        assert!(!fixture.work().join("source").exists());
        assert!(fixture.work().join(".source-pending").is_dir());
        assert!(!fixture.work().join(".source-pending/link").exists());
        assert!(SourceSnapshot::prepare(
            &run,
            SourceRole::Source,
            &recipe,
            TIMEOUT,
            &fixture.token
        )
        .is_err());
    }
}

#[test]
fn revalidation_rejects_content_mode_membership_metadata_and_hardlink_changes() {
    for mutation in [
        "bytes", "mode", "extra", "metadata", "hardlink", "symlink", "fifo", "missing",
    ] {
        let fixture = Fixture::new();
        let run = fixture.reserve();
        let snapshot = SourceSnapshot::prepare(
            &run,
            SourceRole::Source,
            &fixture.recipe(),
            TIMEOUT,
            &fixture.token,
        )
        .unwrap();
        let file = snapshot.root().join("run");
        match mutation {
            "bytes" => fs::write(&file, b"changed").unwrap(),
            "mode" => fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap(),
            "extra" => fs::create_dir(snapshot.root().join("extra")).unwrap(),
            "metadata" => fs::create_dir(snapshot.root().join(".git")).unwrap(),
            "hardlink" => fs::hard_link(&file, fixture.work().join("alias")).unwrap(),
            "symlink" => {
                fs::remove_file(&file).unwrap();
                symlink("empty", &file).unwrap();
            }
            "fifo" => {
                fs::remove_file(&file).unwrap();
                assert!(Command::new("mkfifo")
                    .arg(&file)
                    .status()
                    .unwrap()
                    .success());
            }
            "missing" => fs::remove_file(&file).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            snapshot.revalidate(TIMEOUT, &fixture.token).is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn occupied_leaves_and_root_replacement_never_adopt_foreign_data() {
    for leaf in ["source", ".source-pending"] {
        let fixture = Fixture::new();
        let run = fixture.reserve();
        fs::create_dir(fixture.work().join(leaf)).unwrap();
        fs::write(fixture.work().join(leaf).join("foreign"), b"keep").unwrap();
        assert!(SourceSnapshot::prepare(
            &run,
            SourceRole::Source,
            &fixture.recipe(),
            TIMEOUT,
            &fixture.token
        )
        .is_err());
        assert_eq!(
            fs::read(fixture.work().join(leaf).join("foreign")).unwrap(),
            b"keep"
        );
    }
    let fixture = Fixture::new();
    let run = fixture.reserve();
    let snapshot = SourceSnapshot::prepare(
        &run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    )
    .unwrap();
    fs::rename(snapshot.root(), fixture.work().join("retained-source")).unwrap();
    fs::create_dir(snapshot.root()).unwrap();
    assert!(snapshot.revalidate(TIMEOUT, &fixture.token).is_err());
    assert!(fixture.work().join("retained-source/run").exists());
}

#[test]
fn cancellation_and_invalid_deadlines_fail_without_staging_or_success() {
    let fixture = Fixture::new();
    let run = fixture.reserve();
    let recipe = fixture.recipe();
    for timeout in [Duration::ZERO, Duration::MAX, Duration::from_nanos(1)] {
        assert!(SourceSnapshot::prepare(
            &run,
            SourceRole::Source,
            &recipe,
            timeout,
            &fixture.token
        )
        .is_err());
        assert!(!fixture.work().join(".source-pending").exists());
    }
    fixture.token.cancel();
    let error = SourceSnapshot::prepare(&run, SourceRole::Source, &recipe, TIMEOUT, &fixture.token)
        .err()
        .unwrap();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerState
    );
    assert!(!fixture.work().join(".source-pending").exists());
    run.release().unwrap();
}

#[test]
fn binary_blob_larger_than_one_batch_and_empty_blobs_copy_exactly() {
    let fixture = Fixture::new();
    let content = vec![0xff; 9 * 1024 * 1024];
    fs::write(fixture.request.source_dir.join("large"), &content).unwrap();
    fixture.commit_source();
    let run = fixture.reserve();
    let snapshot = SourceSnapshot::prepare(
        &run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    )
    .unwrap();
    assert_eq!(fs::read(snapshot.root().join("large")).unwrap(), content);
    snapshot.revalidate(TIMEOUT, &fixture.token).unwrap();
}

#[test]
fn changed_link_target_or_cancelled_revalidation_cannot_pass() {
    let fixture = Fixture::new();
    symlink("empty", fixture.request.source_dir.join("link")).unwrap();
    fixture.commit_source();
    let run = fixture.reserve();
    let snapshot = SourceSnapshot::prepare(
        &run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    )
    .unwrap();
    fs::remove_file(snapshot.root().join("link")).unwrap();
    symlink("../../outside", snapshot.root().join("link")).unwrap();
    let error = snapshot.revalidate(TIMEOUT, &fixture.token).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIdentity
    );
    assert_eq!(
        error.diagnostics().diagnostics[0]
            .location
            .as_ref()
            .unwrap()
            .path,
        "link"
    );
    fixture.token.cancel();
    assert_eq!(
        snapshot
            .revalidate(TIMEOUT, &fixture.token)
            .unwrap_err()
            .diagnostics()
            .diagnostics[0]
            .code,
        DiagnosticCode::ProducerState
    );
    drop(snapshot);
    run.release().unwrap();
    assert!(fs::symlink_metadata(fixture.work().join("source/link"))
        .unwrap()
        .file_type()
        .is_symlink());
}
