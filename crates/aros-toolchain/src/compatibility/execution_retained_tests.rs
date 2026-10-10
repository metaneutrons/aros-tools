//! Filesystem retained-evidence checks use expectations selected before execution.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aros_common::{CancellationToken, DiagnosticCode};
use serde_json::json;

use super::gnu_tests::{gnu_fixture, Fixture};
use super::retained::ExpectedRetainedEvidence;
use super::{
    execute_native_compatibility, readback_retained_native_compatibility, CompatibilityPhase,
    NativeCompatibilityRequest,
};
use crate::profiles::{Profile, Profiles};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SnapshotValue {
    Directory {
        mode: u32,
        inode: u64,
        modified: Option<SystemTime>,
    },
    File {
        bytes: Vec<u8>,
        mode: u32,
        inode: u64,
        links: u64,
        modified: Option<SystemTime>,
    },
    Symlink {
        target: PathBuf,
        mode: u32,
        inode: u64,
    },
    Other {
        mode: u32,
        inode: u64,
        size: u64,
    },
}

#[derive(Clone, Copy)]
enum Mutation {
    ChangedLog,
    SymlinkLog,
    UnexpectedReportEntry,
    MissingLog,
    OversizedLog,
    ChangedStandaloneElf,
    ChangedMeasuredHelper,
    UnexpectedStandaloneEntry,
    NonemptyPublicationLock,
}

#[test]
fn reads_back_synthetic_rv32_configure_only_execution_without_creating_output_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 32, false);
    assert_output_roots_absent(&fixture.request, "before preparing expectations");

    let expected = ExpectedRetainedEvidence::prepare(&fixture.request, &fixture.profiles).unwrap();
    assert_output_roots_absent(&fixture.request, "after preparing expectations");

    let report =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap();
    assert_eq!(
        report
            .probes
            .reports
            .get(&CompatibilityPhase::CmakeConsumer)
            .unwrap()
            .commands
            .len(),
        1,
        "fixture should exercise the configure-only CMake phase"
    );
    let readback = expected
        .readback(&fixture.request, &fixture.profiles)
        .unwrap();
    assert_eq!(readback.receipt_sha256, report.receipt.sha256);

    let before_public_readback = snapshot_tree(temporary.path());
    let public_readback =
        readback_retained_native_compatibility(&fixture.request, &fixture.profiles).unwrap();
    assert_eq!(public_readback.receipt_sha256, report.receipt.sha256);
    assert_eq!(
        snapshot_tree(temporary.path()),
        before_public_readback,
        "public readback changed retained roots, command markers or logs"
    );
}

#[test]
fn public_readback_rejects_missing_outputs_without_creating_them() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 32, false);
    assert_output_roots_absent(&fixture.request, "before public readback");
    let before = snapshot_tree(temporary.path());

    let error =
        readback_retained_native_compatibility(&fixture.request, &fixture.profiles).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerCompatibility,
        "missing retained output must fail with AX0703"
    );
    assert!(
        error
            .to_string()
            .contains("retained standalone output root cannot be inspected"),
        "missing retained output returned an unexpected diagnostic: {error}"
    );
    assert_output_roots_absent(&fixture.request, "after rejected public readback");
    assert_eq!(snapshot_tree(temporary.path()), before);
}

#[test]
fn rejects_mutated_retained_evidence_after_execution() {
    let cases = [
        (
            "changed stdout log",
            Mutation::ChangedLog,
            "native compatibility retained command log bytes differ from their report hashes",
        ),
        (
            "symlinked stdout log",
            Mutation::SymlinkLog,
            "cannot safely open retained compatibility command log",
        ),
        (
            "unexpected report entry",
            Mutation::UnexpectedReportEntry,
            "retained compatibility evidence directory has unexpected entries",
        ),
        (
            "missing stderr log",
            Mutation::MissingLog,
            "cannot safely open retained compatibility command log",
        ),
        (
            "oversized stdout log",
            Mutation::OversizedLog,
            "retained compatibility command log is not a regular file within its configured limit",
        ),
        (
            "changed standalone ELF",
            Mutation::ChangedStandaloneElf,
            "standalone output is not a supported little-endian ELF object",
        ),
        (
            "changed measured helper",
            Mutation::ChangedMeasuredHelper,
            "compatibility helper changed after preparation",
        ),
        (
            "unexpected standalone entry",
            Mutation::UnexpectedStandaloneEntry,
            "retained compatibility evidence directory has unexpected entries",
        ),
        (
            "nonempty publication lock",
            Mutation::NonemptyPublicationLock,
            "retained report publication lock is not a regular file within its configured limit",
        ),
    ];

    for (label, mutation, expected_message) in cases {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = gnu_fixture(temporary.path(), 32, false);
        prepare_expectations(&fixture);
        execute_native_compatibility(&fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic!("{label}: fixture execution failed: {error}"));

        mutate_retained_fixture(&fixture, temporary.path(), mutation);
        let before_readback = snapshot_tree(temporary.path());
        let error = readback_retained_native_compatibility(&fixture.request, &fixture.profiles)
            .unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerCompatibility,
            "{label}: retained read-back should fail with AX0703"
        );
        assert!(
            error.to_string().contains(expected_message),
            "{label}: expected {expected_message:?}, got {error}"
        );
        assert_eq!(
            snapshot_tree(temporary.path()),
            before_readback,
            "{label}: public readback changed retained evidence"
        );
    }
}

#[test]
fn public_readback_rejects_wrong_independent_profile_source_and_runtime_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 32, false);
    execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap();

    let absent_profile = profiles_with_renamed_entry(
        &fixture.request.profile,
        fixture.request.upstream_source_commit.as_str(),
    );
    assert_public_readback_rejected_unchanged(
        &fixture.request,
        &absent_profile,
        temporary.path(),
        "retained compatibility profile is absent from the independently selected inputs",
        "profile absent from independently selected matrix",
    );

    let wrong_profiles = profiles_with_changed_configure_target(
        &fixture.request.profile,
        fixture.request.upstream_source_commit.as_str(),
    );
    assert_public_readback_rejected_unchanged(
        &fixture.request,
        &wrong_profiles,
        temporary.path(),
        "retained compatibility profiles differ from the independently selected inputs",
        "wrong independent profile document",
    );

    let mut wrong_source = fixture.request.clone();
    wrong_source.upstream_source_commit =
        crate::recipe::GitObjectId::try_from("f".repeat(40)).unwrap();
    assert_public_readback_rejected_unchanged(
        &wrong_source,
        &fixture.profiles,
        temporary.path(),
        "retained compatibility profiles differ from the independently selected inputs",
        "wrong upstream source commit selection",
    );

    let mut wrong_runtime = fixture.request.clone();
    let aclocal = wrong_runtime
        .host_tools
        .tools
        .get_mut("aclocal")
        .expect("fixture host closure includes aclocal");
    aclocal.size = aclocal.size.checked_add(1).unwrap();
    assert_public_readback_rejected_unchanged(
        &wrong_runtime,
        &fixture.profiles,
        temporary.path(),
        "compatibility host-tool executable changed after preparation",
        "wrong independently selected host-tool identity",
    );
}

fn prepare_expectations(fixture: &Fixture) -> ExpectedRetainedEvidence {
    assert_output_roots_absent(&fixture.request, "before preparing expectations");
    let expected = ExpectedRetainedEvidence::prepare(&fixture.request, &fixture.profiles).unwrap();
    assert_output_roots_absent(&fixture.request, "after preparing expectations");
    expected
}

fn assert_output_roots_absent(request: &NativeCompatibilityRequest, stage: &str) {
    for root in [
        &request.cmake_build_root,
        &request.upstream_build_root,
        &request.standalone_output_root,
        &request.reports_root,
    ] {
        assert!(
            !root.exists(),
            "{} must remain absent {stage}",
            root.display()
        );
    }
}

fn assert_public_readback_rejected_unchanged(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
    root: &Path,
    expected_message: &str,
    label: &str,
) {
    let before = snapshot_tree(root);
    let error = readback_retained_native_compatibility(request, profiles).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerCompatibility,
        "{label}: expected AX0703, got {error}"
    );
    assert!(
        error.to_string().contains(expected_message),
        "{label}: expected {expected_message:?}, got {error}"
    );
    assert_eq!(
        snapshot_tree(root),
        before,
        "{label}: public readback changed retained evidence"
    );
}

fn profiles_with_changed_configure_target(profile: &Profile, upstream_commit: &str) -> Profiles {
    let selected = json!({
        "name": profile.name(),
        "configure_target": "fixture-rv32-selected-away",
        "upstream_output_target": profile.upstream_output_target(),
        "target_triple": profile.target_triple(),
        "cpu": profile.cpu(),
        "platform": profile.platform(),
        "float_abi": profile.float_abi(),
        "capabilities": profile.capabilities(),
        "target": profile.target().unwrap(),
    });
    Profiles::parse(
        &serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2",
            "family": "gnu",
            "upstream_commit": upstream_commit,
            "profiles": [selected],
        }))
        .unwrap(),
    )
    .unwrap()
}

fn profiles_with_renamed_entry(profile: &Profile, upstream_commit: &str) -> Profiles {
    let selected = json!({
        "name": "fixture-rv32-other",
        "configure_target": profile.configure_target(),
        "upstream_output_target": profile.upstream_output_target(),
        "target_triple": profile.target_triple(),
        "cpu": profile.cpu(),
        "platform": profile.platform(),
        "float_abi": profile.float_abi(),
        "capabilities": profile.capabilities(),
        "target": profile.target().unwrap(),
    });
    Profiles::parse(
        &serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2",
            "family": "gnu",
            "upstream_commit": upstream_commit,
            "profiles": [selected],
        }))
        .unwrap(),
    )
    .unwrap()
}

pub(super) fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, SnapshotValue> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, SnapshotValue>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let value = if metadata.file_type().is_symlink() {
            SnapshotValue::Symlink {
                target: fs::read_link(path).unwrap(),
                mode: metadata.mode(),
                inode: metadata.ino(),
            }
        } else if metadata.is_file() {
            SnapshotValue::File {
                bytes: fs::read(path).unwrap(),
                mode: metadata.mode(),
                inode: metadata.ino(),
                links: metadata.nlink(),
                modified: metadata.modified().ok(),
            }
        } else if metadata.is_dir() {
            SnapshotValue::Directory {
                mode: metadata.mode(),
                inode: metadata.ino(),
                modified: metadata.modified().ok(),
            }
        } else {
            SnapshotValue::Other {
                mode: metadata.mode(),
                inode: metadata.ino(),
                size: metadata.len(),
            }
        };
        entries.insert(relative, value);
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), entries);
            }
        }
    }

    let root = root.canonicalize().unwrap();
    let mut entries = BTreeMap::new();
    visit(&root, &root, &mut entries);
    entries
}

fn mutate_retained_fixture(fixture: &Fixture, root: &std::path::Path, mutation: Mutation) {
    let stdout_log = fixture
        .request
        .reports_root
        .join("upstream-configure.stdout.log");
    match mutation {
        Mutation::ChangedLog => fs::write(&stdout_log, b"changed retained stdout\n").unwrap(),
        Mutation::SymlinkLog => {
            let target = root.join("replacement-log-target");
            fs::write(&target, b"replacement stdout\n").unwrap();
            fs::remove_file(&stdout_log).unwrap();
            symlink(target, stdout_log).unwrap();
        }
        Mutation::UnexpectedReportEntry => {
            fs::write(
                fixture.request.reports_root.join("unexpected.txt"),
                b"extra\n",
            )
            .unwrap();
        }
        Mutation::MissingLog => fs::remove_file(
            fixture
                .request
                .reports_root
                .join("upstream-configure.stderr.log"),
        )
        .unwrap(),
        Mutation::OversizedLog => fs::write(
            &stdout_log,
            vec![b'x'; super::super::MAX_RENDERED_LOG_BYTES + 1],
        )
        .unwrap(),
        Mutation::ChangedStandaloneElf => {
            let triple = fixture.request.profile.target_triple();
            let cpu = triple.split_once('-').map_or(triple, |(cpu, _)| cpu);
            fs::write(
                fixture
                    .request
                    .standalone_output_root
                    .join(format!("c-{cpu}.o")),
                b"changed standalone ELF",
            )
            .unwrap();
        }
        Mutation::ChangedMeasuredHelper => {
            let helper = &fixture.request.preparation.helpers["aros-transpiler"];
            fs::write(&helper.path, b"changed measured helper").unwrap();
        }
        Mutation::UnexpectedStandaloneEntry => fs::write(
            fixture.request.standalone_output_root.join("unexpected.o"),
            b"extra",
        )
        .unwrap(),
        Mutation::NonemptyPublicationLock => {
            let journal = aros_common::publication_journal_path(&stdout_log, "file").unwrap();
            let lock = aros_common::publication_journal_lock_path(&journal).unwrap();
            fs::write(lock, b"not an empty publisher lock").unwrap();
        }
    }
}

#[test]
fn rejects_mismatched_upstream_selection_before_output_creation() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    fixture.request.upstream_source_commit =
        crate::recipe::GitObjectId::try_from("f".repeat(40)).unwrap();
    let error = super::execute_native_compatibility_with_readback(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerCompatibility
    );
    assert!(error
        .to_string()
        .contains("profiles differ from the independently selected inputs"));
    assert_output_roots_absent(&fixture.request, "after rejecting upstream mismatch");
}
