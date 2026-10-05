use super::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn checkout() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "user.name", "Local source test"]);
    git(
        temp.path(),
        &["config", "user.email", "source-test@example.invalid"],
    );
    fs::write(temp.path().join("input.c"), b"initial source\n").unwrap();
    fs::write(temp.path().join(".gitignore"), b"*.cache\nbuild/\n").unwrap();
    git(temp.path(), &["add", "input.c", ".gitignore"]);
    git(temp.path(), &["commit", "-qm", "Initial source fixture"]);
    fs::create_dir_all(temp.path().join("build/fixture")).unwrap();
    temp
}

#[test]
fn local_source_identity_binds_dirty_ignored_and_untracked_bytes() {
    let temp = checkout();
    let first = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    first.verify(temp.path(), "fixture").unwrap();
    for name in ["input.c", "untracked.c", "ignored.cache"] {
        let before = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
        fs::write(temp.path().join(name), name.as_bytes()).unwrap();
        assert!(
            before.verify(temp.path(), "fixture").is_err(),
            "{name} must be measured"
        );
        let new = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
        assert_eq!(new.head_baseline, first.head_baseline);
        assert_ne!(new.content_sha256, before.content_sha256);
    }
}

#[test]
fn local_source_identity_excludes_only_selected_build_and_git_metadata() {
    let temp = checkout();
    let first = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    fs::write(
        temp.path().join("build/fixture/output.o"),
        b"generated output",
    )
    .unwrap();
    fs::write(temp.path().join(".git/inspection-cache"), b"Git metadata").unwrap();
    first.verify(temp.path(), "fixture").unwrap();
    fs::create_dir_all(temp.path().join("build/other")).unwrap();
    fs::write(temp.path().join("build/other/input"), b"not selected build").unwrap();
    assert!(first.verify(temp.path(), "fixture").is_err());
}

#[test]
fn local_source_identity_refuses_tracked_output_even_when_staged_deleted() {
    let temp = checkout();
    fs::write(temp.path().join("build/fixture/input.c"), b"tracked source").unwrap();
    git(temp.path(), &["add", "-f", "build/fixture/input.c"]);
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture")
        .unwrap_err()
        .contains("tracked"));
    git(temp.path(), &["commit", "-qm", "Tracked build source"]);
    git(temp.path(), &["rm", "--cached", "build/fixture/input.c"]);
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture")
        .unwrap_err()
        .contains("tracked"));
}

#[test]
fn local_source_identity_cannot_hide_tracked_output_with_git_replace() {
    let temp = checkout();
    let initial = git_text(temp.path(), &["rev-parse", "HEAD"]).unwrap();
    fs::write(temp.path().join("build/fixture/input.c"), b"tracked source").unwrap();
    git(temp.path(), &["add", "-f", "build/fixture/input.c"]);
    git(
        temp.path(),
        &["commit", "-qm", "Tracked source below build"],
    );
    let head = git_text(temp.path(), &["rev-parse", "HEAD"]).unwrap();
    git(temp.path(), &["rm", "--cached", "build/fixture/input.c"]);
    git(temp.path(), &["replace", &head, &initial]);
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture")
        .unwrap_err()
        .contains("tracked"));
}

#[test]
fn local_source_identity_refuses_nested_root_invalid_preset_and_unborn_head() {
    let temp = checkout();
    for preset in ["", "../outside", "fixture/subdir", ".git", "bad name"] {
        assert!(LocalSourceIdentity::capture(temp.path(), preset).is_err());
    }
    assert!(LocalSourceIdentity::capture(&temp.path().join("build"), "fixture").is_err());
    let unborn = tempfile::tempdir().unwrap();
    git(unborn.path(), &["init", "-q"]);
    assert!(LocalSourceIdentity::capture(unborn.path(), "fixture").is_err());
}

#[test]
fn local_source_identity_allows_a_missing_safe_generated_preset() {
    let temp = checkout();
    LocalSourceIdentity::validate_build_namespace(temp.path(), "not-created-yet").unwrap();
    let identity = LocalSourceIdentity::capture(temp.path(), "not-created-yet").unwrap();
    assert_eq!(identity.generated_subtree, "build/not-created-yet");
}

#[test]
fn local_source_identity_accepts_sha256_git_baseline_when_supported() {
    let temp = tempfile::tempdir().unwrap();
    let initialized = Command::new("git")
        .arg("-C")
        .arg(temp.path())
        .args(["init", "-q", "--object-format=sha256"])
        .output()
        .unwrap();
    if !initialized.status.success() {
        return;
    }
    git(temp.path(), &["config", "user.name", "Local source test"]);
    git(
        temp.path(),
        &["config", "user.email", "source-test@example.invalid"],
    );
    fs::write(temp.path().join("input.c"), b"initial source\n").unwrap();
    git(temp.path(), &["add", "input.c"]);
    git(
        temp.path(),
        &["commit", "-qm", "Initial SHA-256 source fixture"],
    );

    let identity = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    assert_eq!(identity.head_baseline.len(), 64);
    identity.verify(temp.path(), "fixture").unwrap();
}

#[cfg(unix)]
#[test]
fn local_source_identity_rejects_symlinked_build_parent_before_cleanup() {
    let temp = checkout();
    let external = tempfile::tempdir().unwrap();
    let sentinel = external.path().join("fixture/sentinel.txt");
    fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    fs::write(&sentinel, b"preserve outside checkout").unwrap();

    fs::remove_dir_all(temp.path().join("build")).unwrap();
    symlink(external.path(), temp.path().join("build")).unwrap();
    assert!(LocalSourceIdentity::validate_build_namespace(temp.path(), "fixture").is_err());
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture").is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve outside checkout");
}

#[cfg(unix)]
#[test]
fn local_source_identity_rejects_selected_symlink_and_regular_file_prefix() {
    let temp = checkout();
    let external = tempfile::tempdir().unwrap();
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"preserve outside checkout").unwrap();
    fs::remove_dir_all(temp.path().join("build/fixture")).unwrap();
    symlink(external.path(), temp.path().join("build/fixture")).unwrap();
    assert!(LocalSourceIdentity::validate_build_namespace(temp.path(), "fixture").is_err());
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture").is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve outside checkout");

    let temp = checkout();
    fs::remove_dir_all(temp.path().join("build")).unwrap();
    fs::write(temp.path().join("build"), b"not a directory").unwrap();
    assert!(LocalSourceIdentity::validate_build_namespace(temp.path(), "fixture").is_err());
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture").is_err());

    let temp = checkout();
    fs::remove_dir_all(temp.path().join("build/fixture")).unwrap();
    fs::write(temp.path().join("build/fixture"), b"not a directory").unwrap();
    assert!(LocalSourceIdentity::validate_build_namespace(temp.path(), "fixture").is_err());
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture").is_err());
}

#[test]
fn local_source_identity_binds_submodule_content_and_rejects_uninitialized() {
    let temp = checkout();
    let dependency = checkout();
    git(
        temp.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            dependency.path().to_str().unwrap(),
            "vendor/dependency",
        ],
    );
    git(temp.path(), &["commit", "-qam", "Add submodule fixture"]);
    let first = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    let child = temp.path().join("vendor/dependency");
    git(
        &child,
        &["config", "user.name", "Submodule identity fixture"],
    );
    git(&child, &["config", "user.email", "child@example.invalid"]);
    git(
        &child,
        &[
            "commit",
            "--allow-empty",
            "-qm",
            "Different commit, same source bytes",
        ],
    );
    let changed = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    assert_eq!(first.content_sha256, changed.content_sha256);
    assert_ne!(first.submodules_sha256, changed.submodules_sha256);
    assert!(first.verify(temp.path(), "fixture").is_err());
    fs::write(
        temp.path().join("vendor/dependency/input.c"),
        b"dirty dependency source",
    )
    .unwrap();
    assert!(first.verify(temp.path(), "fixture").is_err());
    git(
        temp.path(),
        &["submodule", "deinit", "-f", "vendor/dependency"],
    );
    assert!(LocalSourceIdentity::capture(temp.path(), "fixture")
        .unwrap_err()
        .contains("uninitialized"));
}

#[test]
fn local_source_identity_schema_does_not_accept_release_claims() {
    let temp = checkout();
    let identity = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    let mut json = serde_json::to_value(&identity).unwrap();
    json["release_id"] = "pretend-release".into();
    assert!(serde_json::from_value::<LocalSourceIdentity>(json).is_err());
    let mut invalid = identity;
    invalid.schema = "aros-released-source-v1".into();
    assert!(invalid.verify(temp.path(), "fixture").is_err());
}
