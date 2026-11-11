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
fn logging_precedence_distinguishes_file_only_from_explicit_off() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir(&source).unwrap();
    let run = |log: &Path, level: Option<&str>, environment_off: bool| {
        let output_inc = directory.path().join(format!(
            "include-{}",
            log.file_name().unwrap().to_string_lossy()
        ));
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-genmodule"));
        command
            .args(["--scan-dir"])
            .arg(&source)
            .args(["--output-inc"])
            .arg(output_inc)
            .args(["--log-file"])
            .arg(log)
            .arg("--log-format=jsonl");
        if let Some(level) = level {
            command.arg(format!("--log-level={level}"));
        }
        if environment_off {
            command.env("AROS_GENMODULE_LOG_LEVEL", "off");
        }
        command.output().unwrap()
    };
    let file_only = directory.path().join("file-only.jsonl");
    assert!(run(&file_only, None, false).status.success());
    assert!(file_only.is_file());

    let explicit_off = directory.path().join("explicit-off.jsonl");
    assert!(run(&explicit_off, Some("off"), false).status.success());
    assert!(!explicit_off.exists());

    let environment_off = directory.path().join("environment-off.jsonl");
    assert!(run(&environment_off, None, true).status.success());
    assert!(!environment_off.exists());
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
fn explicit_output_root_publishes_the_rv32_sdk_and_symbol_audit_layout() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_library_fixture(&source, "rom/test", "test");
    let output_root = directory.path().join("build");
    let output_inc = output_root.join("SYS/Developer/include");
    let output_gen = output_root.join("gen");
    let output_libbases = output_root.join("symbol-audit/libbases.txt");

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-root",
        output_root.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
        "--output-libbases",
        output_libbases.to_str().unwrap(),
    ]);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output_inc.join("proto/test.h").is_file());
    assert!(output_gen.join("rom/test/test_libdefs.h").is_file());
    assert_eq!(fs::read_to_string(output_libbases).unwrap(), "TestBase\n");
}

#[test]
fn explicit_output_root_rejects_outside_and_traversing_outputs_before_writes() {
    for traversing in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        write_library_fixture(&source, "rom/test", "test");
        let output_root = directory.path().join("build");
        let output_inc = output_root.join("SYS/Developer/include");
        let output_gen = if traversing {
            output_root.join("../escaped/gen")
        } else {
            directory.path().join("outside/gen")
        };

        let result = run_json(&[
            "--scan-dir",
            source.to_str().unwrap(),
            "--output-root",
            output_root.to_str().unwrap(),
            "--output-inc",
            output_inc.to_str().unwrap(),
            "--output-gen",
            output_gen.to_str().unwrap(),
        ]);

        assert!(!result.status.success());
        assert_eq!(diagnostic_code(&result), "AG0201");
        assert!(!output_root.exists());
        assert!(!directory.path().join("outside").exists());
        assert!(!directory.path().join("escaped").exists());
    }
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

#[test]
fn declarations_sharing_a_module_name_publish_no_ambiguous_private_header() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    // A unique module keeps its private header as before.
    write_library_fixture(&source, "rom/test", "test");
    // A resource and a device built from different .conf files share one name.
    let network = source.join("arch/test/network");
    fs::create_dir_all(&network).unwrap();
    fs::write(
        network.join("wifi.conf"),
        "##begin config\nversion 1.0\noptions noincludes\n##end config\n",
    )
    .unwrap();
    fs::write(
        network.join("wifi-device.conf"),
        "##begin config\nversion 1.3\n##end config\n",
    )
    .unwrap();
    fs::write(
        network.join("mmakefile.src"),
        "%build_module mmake=test-wifi-probe modname=wifi modtype=resource files=probe\n\
         %build_module mmake=test-wifi-device modname=wifi modtype=device \\\n\
         \x20   conffile=./wifi-device.conf files=device\n",
    )
    .unwrap();
    let output_root = directory.path().join("build");
    let output_inc = output_root.join("SYS/Developer/include");
    let output_gen = output_root.join("gen");
    // A header left by an earlier run must not survive as a wrong answer.
    let leftover = output_gen.join("arch/test/network/wifi_libdefs.h");
    fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    fs::write(&leftover, "stale\n").unwrap();

    let result = run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-root",
        output_root.to_str().unwrap(),
        "--output-inc",
        output_inc.to_str().unwrap(),
        "--output-gen",
        output_gen.to_str().unwrap(),
    ]);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output_gen.join("rom/test/test_libdefs.h").is_file());
    assert!(output_inc.join("proto/wifi.h").is_file());
    assert!(!leftover.exists());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("arch/test/network/wifi (device+resource)"),
        "{stdout}"
    );
}

/// A module whose conf and build description are given explicitly, so a test
/// can choose the module type and `options` line.
fn write_module_fixture(
    root: &Path,
    relative: &str,
    stem: &str,
    modtype: &str,
    options: Option<&str>,
) {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).unwrap();
    let options = options.map_or_else(String::new, |value| format!("options {value}\n"));
    fs::write(
        directory.join(format!("{stem}.conf")),
        format!("##begin config\nversion 1.0\n{options}##end config\n"),
    )
    .unwrap();
    fs::write(
        directory.join("mmakefile.src"),
        format!("%build_module mmake=test-{stem} modname={stem} modtype={modtype} files=test.c\n"),
    )
    .unwrap();
}

fn run_sdk(source: &Path, build: &Path) -> Output {
    let include = build.join("SDK/include");
    let generated = build.join("gen");
    run_json(&[
        "--scan-dir",
        source.to_str().unwrap(),
        "--output-root",
        build.to_str().unwrap(),
        "--output-inc",
        include.to_str().unwrap(),
        "--output-gen",
        generated.to_str().unwrap(),
    ])
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable();
    names
}

#[test]
fn noincludes_module_yields_a_case_only_collision_to_its_single_owner() {
    // rom/filesystem declares `options noincludes`; the Envoy service of the
    // same name in another case publishes the headers.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_module_fixture(
        &source,
        "rom/filesystem",
        "FileSystem",
        "resource",
        Some("noincludes"),
    );
    write_module_fixture(&source, "net/efsservice", "filesystem", "library", None);
    write_module_fixture(&source, "rom/other", "other", "library", None);
    let build = directory.path().join("build");

    let result = run_sdk(&source, &build);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let include = build.join("SDK/include");
    // Exactly one spelling is published, the owner's.
    assert_eq!(
        names_in(&include.join("proto")),
        ["filesystem.h", "other.h"]
    );
    assert_eq!(
        names_in(&include.join("clib")),
        ["filesystem_protos.h", "other_protos.h"]
    );
    assert_eq!(
        names_in(&include.join("defines")),
        ["filesystem.h", "filesystem_LVO.h", "other.h", "other_LVO.h"]
    );
    // No collision is reported: the conf files named the owner.
    assert!(!include.join("conf-name-collisions.txt").exists());
    // Module-private libdefs stay per module directory in both spellings.
    assert!(build
        .join("gen/rom/filesystem/FileSystem_libdefs.h")
        .is_file());
    assert!(build
        .join("gen/net/efsservice/filesystem_libdefs.h")
        .is_file());
}

#[test]
fn unaffected_modules_publish_the_same_bytes_with_and_without_the_collision() {
    let directory = tempfile::tempdir().unwrap();
    let with = directory.path().join("with");
    write_module_fixture(
        &with,
        "rom/filesystem",
        "FileSystem",
        "resource",
        Some("noincludes"),
    );
    write_module_fixture(&with, "net/efsservice", "filesystem", "library", None);
    write_module_fixture(&with, "rom/other", "other", "library", None);
    let alone = directory.path().join("alone");
    write_module_fixture(&alone, "rom/other", "other", "library", None);

    assert!(run_sdk(&with, &directory.path().join("b-with"))
        .status
        .success());
    assert!(run_sdk(&alone, &directory.path().join("b-alone"))
        .status
        .success());

    for relative in [
        "SDK/include/proto/other.h",
        "SDK/include/clib/other_protos.h",
        "SDK/include/defines/other.h",
        "SDK/include/defines/other_LVO.h",
        "gen/rom/other/other_libdefs.h",
    ] {
        assert_eq!(
            fs::read(directory.path().join("b-with").join(relative)).unwrap(),
            fs::read(directory.path().join("b-alone").join(relative)).unwrap(),
            "{relative}"
        );
    }
}

#[test]
fn case_only_collision_without_a_declaration_is_still_rejected() {
    // Same layout, but nobody declared `noincludes`: nothing resolves the
    // spelling, so the generator keeps refusing the case-folding collision.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_module_fixture(&source, "rom/filesystem", "FileSystem", "resource", None);
    write_module_fixture(&source, "net/efsservice", "filesystem", "library", None);
    let build = directory.path().join("build");

    let result = run_sdk(&source, &build);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
    assert!(String::from_utf8_lossy(&result.stderr).contains("portable output collision"));
    assert!(!build
        .join("SDK/include/proto")
        .join("filesystem.h")
        .exists());
}

#[test]
fn case_only_collision_with_declarations_on_both_modules_is_still_rejected() {
    // Two declared non-owners leave no owner: ambiguous, so rejected as before.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_module_fixture(
        &source,
        "rom/filesystem",
        "FileSystem",
        "resource",
        Some("noincludes"),
    );
    write_module_fixture(
        &source,
        "net/efsservice",
        "filesystem",
        "library",
        Some("noincludes"),
    );
    let build = directory.path().join("build");

    let result = run_sdk(&source, &build);

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "AG0201");
}

#[test]
fn the_owner_is_the_module_without_the_declaration_not_the_other_one() {
    // The declaration sits on the service this time, so the resource owns the
    // name. The rule follows the conf files and does not prefer a module.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_module_fixture(&source, "rom/filesystem", "FileSystem", "resource", None);
    write_module_fixture(
        &source,
        "net/efsservice",
        "filesystem",
        "library",
        Some("noincludes"),
    );
    let build = directory.path().join("build");

    let result = run_sdk(&source, &build);

    assert!(result.status.success());
    assert_eq!(names_in(&build.join("SDK/include/proto")), ["FileSystem.h"]);
}

#[test]
fn two_owners_and_a_declared_non_owner_stay_withheld() {
    // Three modules, one spelling: two of them publish headers, so there is no
    // single owner and the shared name stays withheld and reported.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    write_module_fixture(&source, "a/dup", "dup", "library", None);
    write_module_fixture(&source, "b/dup", "dup", "library", None);
    write_module_fixture(&source, "c/dup", "dup", "resource", Some("noincludes"));
    let build = directory.path().join("build");

    let result = run_sdk(&source, &build);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let include = build.join("SDK/include");
    assert!(!include.join("proto/dup.h").exists());
    assert!(fs::read_to_string(include.join("conf-name-collisions.txt"))
        .unwrap()
        .contains("dup"));
}
