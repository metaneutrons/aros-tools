use super::repository_security::normalize_url;
use super::*;

#[test]
fn validates_unambiguous_refs() {
    for valid in [
        "refs/heads/master",
        "refs/tags/v1.2.3",
        "0123456789012345678901234567890123456789",
    ] {
        validate_init_ref(valid).unwrap();
    }
    for invalid in [
        "",
        "master",
        "--help",
        "refs/heads/../main",
        "refs/heads/topic/.hidden",
        "refs/heads/topic.lock",
        "refs/heads/HEAD",
        "a@{1}",
    ] {
        assert!(validate_init_ref(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn accepts_only_reviewable_transport_families() {
    assert_eq!(
        validate_url("https://example.invalid/AROS.git", "test").unwrap(),
        TransportKind::Https
    );
    assert_eq!(
        validate_url("git@example.invalid:AROS.git", "test").unwrap(),
        TransportKind::Ssh
    );
    assert!(validate_url("https://token@example.invalid/AROS.git", "test").is_err());
    assert!(validate_url("ssh://-oProxyCommand=evil/repo", "test").is_err());
    assert!(validate_url("bad$user@example.invalid:repo", "test").is_err());
    assert!(validate_url("ext::sh -c exploit", "test").is_err());
    assert!(validate_url("custom://example.invalid/repo", "test").is_err());
    assert!(validate_url("file://relative/repository", "test").is_err());
}

#[test]
fn compares_reviewed_urls_without_cosmetic_git_suffixes() {
    assert_eq!(
        normalize_url("HTTPS://GITHUB.COM/example/AROS.GIT/"),
        "https://github.com/example/AROS"
    );
    assert_ne!(
        normalize_url("https://git.example/Owner/AROS.git"),
        normalize_url("https://git.example/owner/AROS.git")
    );
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn destination_race_never_clobbers_existing_data() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("staged-checkout");
    let destination = temporary.path().join("AROS");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("source-sentinel"), "staged").unwrap();
    ensure_destination_absent(&destination).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("contender-sentinel"), "existing").unwrap();

    let error = publish_source_checkout(&source, &destination).unwrap_err();
    let diagnostic = observability::report_diagnostic(
        &error,
        SOURCE_PUBLICATION,
        aros_common::DiagnosticContext::default(),
    );
    assert!(diagnostic
        .message
        .contains("without replacing existing data"));
    assert_eq!(
        diagnostic.context.unwrap().commit_state,
        Some(CommitState::RolledBack)
    );
    assert_eq!(
        fs::read_to_string(destination.join("contender-sentinel")).unwrap(),
        "existing"
    );
    assert_eq!(
        fs::read_to_string(source.join("source-sentinel")).unwrap(),
        "staged"
    );
}

#[cfg(unix)]
#[test]
fn gitmodules_must_be_a_no_follow_regular_file() {
    use std::os::unix::fs::symlink;

    let repository = tempfile::tempdir().unwrap();
    let external = repository.path().join("external-config");
    fs::write(
        &external,
        "[submodule \"component\"]\npath = external/component\nurl = https://example.invalid/component.git\n",
    )
    .unwrap();
    symlink(&external, repository.path().join(".gitmodules")).unwrap();

    let error = direct_submodules(repository.path()).unwrap_err();
    assert!(error
        .to_string()
        .contains("no-follow regular-file snapshot"));
}

#[test]
fn gitmodules_snapshot_rejects_include_escape() {
    for contents in [
        "[include]\npath = /tmp/unreviewed-gitmodules\n",
        "[includeIf \"gitdir:/tmp/**\"]\npath = /tmp/unreviewed-gitmodules\n",
    ] {
        let repository = tempfile::tempdir().unwrap();
        fs::write(repository.path().join(".gitmodules"), contents).unwrap();

        let error = direct_submodules(repository.path()).unwrap_err();
        assert!(error.to_string().contains("must not contain include"));
    }
}
