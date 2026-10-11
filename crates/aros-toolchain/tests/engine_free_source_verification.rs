#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, verify_engine_free_source, CommittedSourceIdentity,
    EngineFreeSourceOutput, EngineFreeSourceRequest, EngineFreeSourceVerificationRequest,
};
use aros_toolchain::recipe::GitObjectId;
use tempfile::TempDir;

struct Fixture {
    temporary: TempDir,
    source_root: PathBuf,
    identity: CommittedSourceIdentity,
    materialized: EngineFreeSourceOutput,
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=AROS test",
            "-c",
            "user.email=aros-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn identity_at_head(root: &Path) -> CommittedSourceIdentity {
    CommittedSourceIdentity {
        commit: GitObjectId::try_from(git_text(root, &["rev-parse", "HEAD^0"])).unwrap(),
        tree: GitObjectId::try_from(git_text(root, &["rev-parse", "HEAD:"])).unwrap(),
    }
}

fn create_fixture(with_submodule: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    fs::create_dir(&source_root).unwrap();
    git(&source_root, &["init", "-q"]);

    fs::create_dir(source_root.join("cmake")).unwrap();
    fs::write(source_root.join("cmake/engine.cmake"), "excluded engine\n").unwrap();
    fs::create_dir(source_root.join("include")).unwrap();
    fs::write(source_root.join("include/shared.h"), "#define VALUE 7\n").unwrap();
    fs::write(source_root.join(".gitignore"), "ignored-after-commit.bin\n").unwrap();
    fs::write(source_root.join("configure"), "#!/bin/sh\nexit 0\n").unwrap();
    let mut configure_mode = fs::metadata(source_root.join("configure"))
        .unwrap()
        .permissions();
    configure_mode.set_mode(0o755);
    fs::set_permissions(source_root.join("configure"), configure_mode).unwrap();
    symlink("include/shared.h", source_root.join("header-link")).unwrap();

    if with_submodule {
        let submodule_root = temporary.path().join("submodule-source");
        fs::create_dir(&submodule_root).unwrap();
        git(&submodule_root, &["init", "-q"]);
        fs::write(submodule_root.join("library.txt"), "local submodule\n").unwrap();
        git(&submodule_root, &["add", "library.txt"]);
        git(&submodule_root, &["commit", "-qm", "fixture submodule"]);
        let submodule_path = submodule_root.to_str().unwrap();
        git(
            &source_root,
            &["submodule", "add", "-q", submodule_path, "vendor"],
        );
    }

    git(&source_root, &["add", "-A"]);
    git(&source_root, &["commit", "-qm", "fixture source"]);
    let identity = identity_at_head(&source_root);
    let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: source_root.clone(),
        expected_source: identity.clone(),
        output_root: temporary.path().join("materialized"),
    })
    .unwrap();

    Fixture {
        temporary,
        source_root,
        identity,
        materialized,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    Regular(Vec<u8>),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    mode: u32,
    kind: EntryKind,
}

fn collect_snapshot(root: &Path, directory: &Path, entries: &mut Vec<SnapshotEntry>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o7777;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).unwrap())
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::Regular(fs::read(&path).unwrap())
        } else {
            EntryKind::Other
        };
        entries.push(SnapshotEntry {
            path: relative,
            mode,
            kind,
        });
        if file_type.is_dir() {
            collect_snapshot(root, &path, entries);
        }
    }
}

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    collect_snapshot(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn verify_rejects_unchanged(fixture: &Fixture, expected_source: CommittedSourceIdentity) {
    let source_before = snapshot(&fixture.source_root);
    let materialized_before = snapshot(&fixture.materialized.root);
    let result = verify_engine_free_source(&EngineFreeSourceVerificationRequest {
        source_root: fixture.source_root.clone(),
        expected_source,
        materialized_source_root: fixture.materialized.root.clone(),
    });
    assert!(result.is_err(), "invalid engine-free source was accepted");
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(snapshot(&fixture.materialized.root), materialized_before);
}

fn reject_materialized_mutation(change: impl FnOnce(&Path)) {
    let fixture = create_fixture(false);
    change(&fixture.materialized.root);
    verify_rejects_unchanged(&fixture, fixture.identity.clone());
}

fn different_object_id(id: &GitObjectId) -> GitObjectId {
    let mut value = id.as_str().to_owned();
    let replacement = if value.starts_with('0') { '1' } else { '0' };
    value.replace_range(0..1, &replacement.to_string());
    GitObjectId::try_from(value).unwrap()
}

#[test]
fn verifies_real_materialization_identity_and_content_digest() {
    let fixture = create_fixture(true);
    let root = &fixture.materialized.root;
    assert!(!root.join("cmake").exists());
    assert_eq!(
        fs::read(root.join("include/shared.h")).unwrap(),
        b"#define VALUE 7\n"
    );
    assert_eq!(
        fs::read_link(root.join("header-link")).unwrap(),
        Path::new("include/shared.h")
    );
    assert_eq!(
        fs::read(root.join("vendor/library.txt")).unwrap(),
        b"local submodule\n"
    );
    assert_eq!(
        fs::metadata(root.join("configure"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );

    let verified = verify_engine_free_source(&EngineFreeSourceVerificationRequest {
        source_root: fixture.source_root.clone(),
        expected_source: fixture.identity.clone(),
        materialized_source_root: root.clone(),
    })
    .unwrap();
    assert_eq!(verified, fixture.materialized);
    assert_eq!(verified.source_commit, fixture.identity.commit.as_str());
    assert_eq!(verified.source_tree, fixture.identity.tree.as_str());
}

#[test]
fn rejects_independent_materialized_content_and_entry_mutations_without_writes() {
    reject_materialized_mutation(|root| {
        fs::write(root.join("include/shared.h"), "#define VALUE 8\n").unwrap();
    });
    reject_materialized_mutation(|root| {
        let mut permissions = fs::metadata(root.join("configure")).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(root.join("configure"), permissions).unwrap();
    });
    reject_materialized_mutation(|root| fs::remove_file(root.join("include/shared.h")).unwrap());
    reject_materialized_mutation(|root| {
        fs::write(root.join("unexpected.txt"), "extra regular file\n").unwrap();
    });
    reject_materialized_mutation(|root| fs::create_dir(root.join("empty-directory")).unwrap());
    reject_materialized_mutation(|root| {
        fs::create_dir(root.join("cmake")).unwrap();
        fs::write(root.join("cmake/CMakeLists.txt"), "unexpected engine\n").unwrap();
    });
    reject_materialized_mutation(|root| {
        fs::create_dir(root.join("replacement")).unwrap();
        fs::rename(root.join("include"), root.join("replacement/include-real")).unwrap();
        fs::rename(
            root.join("replacement/include-real"),
            root.join("replacement/include"),
        )
        .unwrap();
        symlink("replacement/include", root.join("include")).unwrap();
    });
}

#[test]
fn rejects_changed_and_unsafe_materialized_symlinks_without_writes() {
    reject_materialized_mutation(|root| {
        fs::remove_file(root.join("header-link")).unwrap();
        fs::write(root.join("include/alternate.h"), "alternate\n").unwrap();
        symlink("include/alternate.h", root.join("header-link")).unwrap();
    });
    reject_materialized_mutation(|root| {
        fs::remove_file(root.join("header-link")).unwrap();
        symlink("../../outside", root.join("header-link")).unwrap();
    });
}

#[test]
fn rejects_content_identical_external_hardlink_without_touching_its_target() {
    let fixture = create_fixture(false);
    let projected = fixture.materialized.root.join("include/shared.h");
    let outside = fixture.temporary.path().join("outside.h");
    fs::write(&outside, fs::read(&projected).unwrap()).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(&projected).unwrap();
    fs::hard_link(&outside, &projected).unwrap();
    let before = snapshot(&fixture.materialized.root);
    let outside_bytes = fs::read(&outside).unwrap();
    let error = verify_engine_free_source(&EngineFreeSourceVerificationRequest {
        source_root: fixture.source_root.clone(),
        expected_source: fixture.identity.clone(),
        materialized_source_root: fixture.materialized.root.clone(),
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("projected regular files must have exactly one link"));
    assert_eq!(snapshot(&fixture.materialized.root), before);
    assert_eq!(fs::read(outside).unwrap(), outside_bytes);
}

#[test]
fn rejects_mismatched_commit_or_tree_without_writes() {
    let fixture = create_fixture(false);
    let mismatched_commit = CommittedSourceIdentity {
        commit: different_object_id(&fixture.identity.commit),
        tree: fixture.identity.tree.clone(),
    };
    verify_rejects_unchanged(&fixture, mismatched_commit);

    let fixture = create_fixture(false);
    let mismatched_tree = CommittedSourceIdentity {
        commit: fixture.identity.commit.clone(),
        tree: different_object_id(&fixture.identity.tree),
    };
    verify_rejects_unchanged(&fixture, mismatched_tree);
}

#[test]
fn rejects_dirty_original_and_ignored_input_without_writes() {
    let fixture = create_fixture(false);
    fs::write(
        fixture.source_root.join("include/shared.h"),
        "#define VALUE 99\n",
    )
    .unwrap();
    verify_rejects_unchanged(&fixture, fixture.identity.clone());

    let fixture = create_fixture(false);
    fs::write(
        fixture.source_root.join("ignored-after-commit.bin"),
        "ignored dirt\n",
    )
    .unwrap();
    verify_rejects_unchanged(&fixture, fixture.identity.clone());
}
