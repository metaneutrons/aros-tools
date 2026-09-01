use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_library_fixture(root: &Path, relative: &str, module: &str) -> PathBuf {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{module}.conf")),
        "##begin config\nversion 1.0\n##end config\n",
    )
    .unwrap();
    fs::write(
        directory.join("mmakefile.src"),
        format!(
            "%build_module mmake=test-{module} modname={module} modtype=library files=test.c\n"
        ),
    )
    .unwrap();
    directory
}

fn run_json(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .arg("--diagnostic-format")
        .arg("json")
        .args(arguments)
        .output()
        .unwrap()
}

fn diagnostic_code(output: &Output) -> String {
    let document: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    document["diagnostics"][0]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn find_path_containing(root: &Path, fragment: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().contains(fragment) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_path_containing(&path, fragment) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn invocation_and_observability_failures_use_stable_codes() {
    let invalid = run_json(&["--unknown-option"]);
    assert!(!invalid.status.success());
    assert_eq!(diagnostic_code(&invalid), "AG0001");

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    let output_inc = directory.path().join("include");
    let missing_log_file = run_json(&[
        "--log-level",
        "info",
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
    ]);
    assert!(!missing_log_file.status.success());
    assert_eq!(diagnostic_code(&missing_log_file), "AG0002");
}

#[test]
fn explicit_jsonl_log_uses_the_stable_schema() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    let output_inc = directory.path().join("include");
    let log = directory.path().join("genmodule.jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(["--log-level", "info", "--log-format", "jsonl", "--log-file"])
        .arg(&log)
        .arg("--scan-dir")
        .arg(&source)
        .arg("--output-inc")
        .arg(&output_inc)
        .output()
        .unwrap();

    assert!(result.status.success());
    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "aros-genmodule-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "invocation.complete");
    assert!(records[0].get("timestamp").is_none());
    assert!(records[0].get("hostname").is_none());
}

#[test]
fn missing_scan_root_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let output_inc = directory.path().join("include");
    let result = run_json(&[
        "--scan-dir",
        missing.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(diagnostic_code(&result), "AG0101");
}

#[test]
fn header_failure_is_nonzero_and_does_not_prune_stale_libdefs() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include-is-a-file");
    fs::write(&output_inc, b"sentinel").unwrap();
    let output_gen = directory.path().join("gen");
    let stale = output_gen.join("rom/old/old_libdefs.h");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, b"stale sentinel").unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert_eq!(fs::read(stale).unwrap(), b"stale sentinel");
    assert_eq!(fs::read(output_inc).unwrap(), b"sentinel");
}

#[test]
fn link_library_failure_is_nonzero() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include");
    let output_gen = directory.path().join("gen");
    let output_linklib = directory.path().join("linklib-is-a-file");
    fs::write(&output_linklib, b"sentinel").unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
        "--output-linklib",
        output_linklib.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert_eq!(fs::read(output_linklib).unwrap(), b"sentinel");
}

#[test]
fn library_base_inventory_failure_is_nonzero() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    let output_inc = directory.path().join("include");
    let blocked_parent = directory.path().join("blocked-parent");
    fs::write(&blocked_parent, b"sentinel").unwrap();
    let output_libbases = blocked_parent.join("libbases.txt");
    let output_gen = directory.path().join("gen");
    let stale = output_gen.join("rom/old/old_libdefs.h");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, b"stale sentinel").unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
        "--output-libbases",
        output_libbases.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert_eq!(fs::read(blocked_parent).unwrap(), b"sentinel");
    assert_eq!(fs::read(stale).unwrap(), b"stale sentinel");
}

#[test]
fn remove_failure_is_nonzero() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/one", "shared");
    write_library_fixture(&source, "rom/two", "shared");
    let output_inc = directory.path().join("include");
    let ambiguous_header = output_inc.join("proto/shared.h");
    fs::create_dir_all(&ambiguous_header).unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        directory.path().join("gen").to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert!(ambiguous_header.is_dir());
}

#[test]
fn version_and_generated_banner_follow_the_crate_package() {
    let version = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("aros-genmodule {}", env!("CARGO_PKG_VERSION"))
    );

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include");
    let output_gen = directory.path().join("gen");
    let generation = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(["--scan-dir"])
        .arg(&source)
        .arg("--output-inc")
        .arg(&output_inc)
        .arg("--output-gen")
        .arg(&output_gen)
        .output()
        .unwrap();
    assert!(
        generation.status.success(),
        "{}",
        String::from_utf8_lossy(&generation.stderr)
    );
    let generated = fs::read_to_string(output_gen.join("rom/test/test_libdefs.h")).unwrap();
    assert!(generated.starts_with(&format!(
        "/* Auto-generated by AROS genmodule v{} */",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(!generated.contains("AROS-NG"));
}

#[test]
fn generated_path_components_are_portable_and_cannot_traverse() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let module = source.join("rom/test");
    fs::create_dir_all(&module).unwrap();
    fs::write(
        module.join("test.conf"),
        "##begin config\nversion 1.0\n##end config\n",
    )
    .unwrap();
    fs::write(
        module.join("mmakefile.src"),
        "%build_module modname=../../escape modtype=library conffile=test.conf\n",
    )
    .unwrap();
    let output = directory.path().join("include");

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0101");
    assert!(!directory.path().join("escape").exists());
}

#[test]
fn confoverride_base_must_remain_inside_canonical_scan_root() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let module = source.join("rom/test");
    fs::create_dir_all(&module).unwrap();
    fs::write(
        module.join("override.conf"),
        "##begin config\nversion 1.0\n##end config\n",
    )
    .unwrap();
    let outside = directory.path().join("outside.conf");
    fs::write(
        &outside,
        "##begin functionlist\nOutside()\n##end functionlist\n",
    )
    .unwrap();
    fs::write(
        module.join("mmakefile.src"),
        format!(
            "%build_module modname=test modtype=library conffile={} confoverride=override.conf\n",
            outside.display()
        ),
    )
    .unwrap();
    let output = directory.path().join("include");

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0101");
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_output_component_is_rejected_without_escape() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), directory.path().join("include")).unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        directory.path().join("include").to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn crash_recovery_restores_original_inode_metadata_and_hardlinks() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let module = write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include");
    let output_gen = directory.path().join("gen");
    let arguments = [
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
    ];
    let initial = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(initial.status.success());

    let target = output_gen.join("rom/test/test_libdefs.h");
    let hardlink = directory.path().join("libdefs-hardlink");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    fs::hard_link(&target, &hardlink).unwrap();
    let original = fs::metadata(&target).unwrap();
    fs::write(
        module.join("test.conf"),
        "##begin config\nversion 2.0\n##end config\n",
    )
    .unwrap();

    let crashed = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .env("AROS_PUBLICATION_TEST_CRASH_AT", "after-backup")
        .args(arguments)
        .output()
        .unwrap();
    assert!(!crashed.status.success());
    assert!(
        !target.exists(),
        "the journalled file set is crash-recoverable, not a live snapshot: after a backup rename the target can be temporarily absent"
    );

    fs::remove_dir_all(&source).unwrap();
    let recovery_log = directory.path().join("recovery.jsonl");
    let recovery = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(["--log-level", "warn", "--log-format", "jsonl", "--log-file"])
        .arg(&recovery_log)
        .args(arguments)
        .output()
        .unwrap();
    assert!(!recovery.status.success(), "missing input must still fail");
    let restored = fs::metadata(&target).unwrap();
    assert_eq!(restored.ino(), original.ino());
    assert_eq!(restored.mode() & 0o777, 0o640);
    assert_eq!(restored.modified().unwrap(), original.modified().unwrap());
    assert_eq!(fs::metadata(&hardlink).unwrap().ino(), original.ino());
    assert_eq!(fs::read(&hardlink).unwrap(), fs::read(&target).unwrap());
    assert!(find_path_containing(directory.path(), ".aros-genmodule-").is_none());
    let recovery_records: Vec<serde_json::Value> = fs::read_to_string(recovery_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(recovery_records.iter().any(|record| {
        record["event"] == "publication.recovery"
            && record["message"]
                .as_str()
                .is_some_and(|message| message.contains("rolled_back"))
    }));
}

#[cfg(unix)]
#[test]
fn crash_after_stage_creation_is_recovered_without_orphaned_auxiliaries() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include");
    let output_gen = directory.path().join("gen");
    let arguments = [
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
    ];

    let crashed = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .env(
            "AROS_PUBLICATION_TEST_CRASH_AT",
            "after-stage-before-journal-update",
        )
        .args(arguments)
        .output()
        .unwrap();
    assert!(!crashed.status.success());
    assert!(find_path_containing(directory.path(), ".aros-stage-").is_some());
    assert!(find_path_containing(directory.path(), ".aros-genmodule-").is_some());
    assert!(!output_inc.join("proto/test.h").exists());

    let recovered = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(output_inc.join("proto/test.h").exists());
    assert!(find_path_containing(directory.path(), ".aros-stage-").is_none());
    assert!(find_path_containing(directory.path(), ".aros-genmodule-").is_none());
}

#[cfg(unix)]
#[test]
fn closed_stdout_after_commit_keeps_success_status() {
    use std::process::Stdio;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output = directory.path().join("include");
    let mut child = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(["--scan-dir"])
        .arg(&source)
        .arg("--output-inc")
        .arg(&output)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(output.join("proto/test.h").exists());
}

#[cfg(unix)]
#[test]
fn closed_help_pipe_is_normal_termination_not_a_panic() {
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .arg("--help")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert!(child.wait().unwrap().success());
}

#[cfg(unix)]
#[test]
fn rollback_secondary_failure_is_distinct_and_other_cleanup_continues() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let module = write_library_fixture(&source, "rom/test", "test");
    let output_inc = directory.path().join("include");
    let output_gen = directory.path().join("gen");
    let arguments = [
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
    ];
    assert!(Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .args(arguments)
        .status()
        .unwrap()
        .success());
    fs::write(
        module.join("test.conf"),
        "##begin config\nversion 3.0\n##end config\n",
    )
    .unwrap();
    let crashed = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"))
        .env("AROS_PUBLICATION_TEST_CRASH_AT", "after-install")
        .args(arguments)
        .output()
        .unwrap();
    assert!(!crashed.status.success());

    let backup = find_path_containing(directory.path(), ".aros-backup-")
        .expect("crash must leave the inode-preserving backup");
    fs::remove_file(&backup).unwrap();
    fs::create_dir(&backup).unwrap();

    let recovery = run_json(&arguments);
    assert!(!recovery.status.success());
    assert_eq!(diagnostic_code(&recovery), "AG0301");
    assert!(find_path_containing(directory.path(), ".aros-genmodule-").is_some());
    assert!(
        find_path_containing(directory.path(), ".aros-stage-").is_none(),
        "best-effort rollback must still clean every independent staged file"
    );
}
