use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::OnceLock;

static CHECKOUT: OnceLock<tempfile::TempDir> = OnceLock::new();

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn command() -> Command {
    let mut command = Command::new(aros());
    command
        .current_dir(checkout())
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE")
        .env_remove("AROS_HOME")
        .env_remove("AROS_CACHE_DIR")
        .env_remove("AROS_HOST_COMPILER_DIR")
        .env_remove("AROS_CROSS_TOOLCHAINS_DIR");
    command
}

fn checkout() -> &'static Path {
    CHECKOUT
        .get_or_init(|| {
            let checkout = tempfile::tempdir().expect("temporary AROS checkout");
            for directory in ["arch", "compiler", "rom", "developer"] {
                fs::create_dir_all(checkout.path().join(directory)).expect("checkout directory");
            }
            fs::write(checkout.path().join("configure"), "").expect("configure marker");
            fs::write(checkout.path().join("Makefile.in"), "").expect("make marker");
            fs::write(
                checkout.path().join("aros-targets.toml"),
                "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
            )
            .expect("target configuration");
            checkout
        })
        .path()
}

fn json(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    serde_json::from_slice(&output.stderr).unwrap()
}

fn temporary_checkout(targets: &str) -> tempfile::TempDir {
    let checkout = tempfile::tempdir().expect("temporary AROS checkout");
    for directory in ["arch", "compiler", "rom"] {
        fs::create_dir_all(checkout.path().join(directory)).expect("checkout directory");
    }
    fs::write(checkout.path().join("configure"), "").expect("configure marker");
    fs::write(checkout.path().join("Makefile.in"), "").expect("make marker");
    fs::write(checkout.path().join("aros-targets.toml"), targets).expect("target configuration");
    checkout
}

#[cfg(unix)]
#[test]
fn native_install_is_global_complete_and_never_clobbers() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source/bin");
    let prefix = root.path().join("prefix");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir(&prefix).unwrap();
    let binaries = [
        "aros",
        "aros-ahi-runner",
        "aros-collect",
        "aros-fetch",
        "aros-genmodule",
        "aros-romtool",
        "aros-transpiler",
        "aros-verify",
    ];
    for binary in binaries {
        let path = source.join(binary);
        fs::write(&path, format!("{binary}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = command()
        .current_dir(root.path())
        .args(["install", "--source-bin"])
        .arg(&source)
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for binary in binaries {
        assert_eq!(
            fs::read_to_string(prefix.join("bin").join(binary)).unwrap(),
            format!("{binary}\n")
        );
    }

    let conflict = command()
        .current_dir(root.path())
        .args(["--diagnostic-format=json", "install", "--source-bin"])
        .arg(&source)
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .unwrap();
    let value = json(&conflict);
    assert_eq!(value["diagnostics"][0]["code"], "AR0901");
    assert_eq!(
        value["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert_eq!(fs::read(prefix.join("bin/aros")).unwrap(), b"aros\n");
}

#[cfg(unix)]
#[test]
fn help_treats_a_closed_stdout_consumer_as_normal_termination() {
    let script = r#"
        set -o pipefail
        "$1" --help | false
        statuses=("${PIPESTATUS[@]}")
        test "${statuses[0]}" -eq 0
    "#;
    let status = Command::new("bash")
        .args(["-c", script, "aros-help-contract", aros()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn normal_output_treats_a_closed_stdout_consumer_as_normal_termination() {
    let script = r#"
        set -o pipefail
        "$1" info | false
        statuses=("${PIPESTATUS[@]}")
        test "${statuses[0]}" -eq 0
    "#;
    let status = Command::new("bash")
        .args(["-c", script, "aros-output-contract", aros()])
        .current_dir(checkout())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn invalid_invocation_is_one_versioned_json_diagnostic() {
    let output = command()
        .arg("--diagnostic-format=json")
        .arg("--definitely-invalid")
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AR0001");
    assert_eq!(value["diagnostics"][0]["stage"], "invocation");
}

#[test]
fn enabled_logging_without_a_file_is_an_observability_error() {
    let output = command()
        .args(["--diagnostic-format=json", "--log-level=info", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0002");
    assert_eq!(value["diagnostics"][0]["stage"], "observability");
}

#[test]
fn explicit_log_level_off_disables_a_selected_log_file() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("disabled.jsonl");
    let output = command()
        .args(["--log-level=off", "--log-file"])
        .arg(&log)
        .arg("info")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        !log.exists(),
        "an explicit off level must not create the selected log file"
    );
}

#[test]
fn explicit_environment_log_level_off_disables_a_selected_log_file() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("disabled-from-environment.jsonl");
    let output = command()
        .env("AROS_LOG_LEVEL", "off")
        .args(["--log-file"])
        .arg(&log)
        .arg("info")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        !log.exists(),
        "an explicit environment off level must not create the selected log file"
    );
}

#[test]
fn file_only_logging_uses_the_documented_info_default() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("file-only.jsonl");
    let output = command()
        .args([
            "--log-file",
            "file-only.jsonl",
            "--log-format=jsonl",
            "info",
        ])
        .current_dir(directory.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let records: Vec<serde_json::Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "invocation.complete");
}

#[test]
fn final_log_failure_reports_only_the_mutation_state_proved_by_the_board_owner() {
    let directory = tempfile::tempdir().unwrap();
    let applied_config = directory.path().join("applied-boards.toml");
    let applied_log = directory.path().join("applied.jsonl");
    let applied = command()
        .env("AROS_TEST_LOG_FAIL_EVENT", "invocation.complete")
        .args(["--diagnostic-format=json", "--log-level=info", "--log-file"])
        .arg(&applied_log)
        .args([
            "board",
            "init",
            "--profile",
            "rpi4-test",
            "--model",
            "rpi4",
            "--config",
        ])
        .arg(&applied_config)
        .arg("--apply")
        .output()
        .unwrap();
    let applied_diagnostic = json(&applied);
    assert_eq!(
        applied_diagnostic["diagnostics"][0]["context"]["commit_state"],
        "committed"
    );
    assert!(
        applied_config.is_file(),
        "the board owner must have published the template before final reporting failed"
    );

    let preview_config = directory.path().join("preview-boards.toml");
    let preview_log = directory.path().join("preview.jsonl");
    let preview = command()
        .env("AROS_TEST_LOG_FAIL_EVENT", "invocation.complete")
        .args(["--diagnostic-format=json", "--log-level=info", "--log-file"])
        .arg(&preview_log)
        .args([
            "board",
            "init",
            "--profile",
            "rpi4-preview",
            "--model",
            "rpi4",
            "--config",
        ])
        .arg(&preview_config)
        .output()
        .unwrap();
    let preview_diagnostic = json(&preview);
    assert!(
        preview_diagnostic["diagnostics"][0]["context"]
            .get("commit_state")
            .is_none(),
        "a preview must not be reported as committed merely because final logging failed"
    );
    assert!(!preview_config.exists());
}

#[cfg(unix)]
#[test]
fn final_log_failure_after_native_suite_publication_is_committed() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let prefix = directory.path().join("prefix");
    let log = directory.path().join("install.jsonl");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&prefix).unwrap();
    for binary in [
        "aros",
        "aros-ahi-runner",
        "aros-collect",
        "aros-fetch",
        "aros-genmodule",
        "aros-romtool",
        "aros-transpiler",
        "aros-verify",
    ] {
        let path = source.join(binary);
        fs::write(&path, binary).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let output = command()
        .current_dir(directory.path())
        .env("AROS_TEST_LOG_FAIL_EVENT", "invocation.complete")
        .args(["--diagnostic-format=json", "--log-level=info", "--log-file"])
        .arg(&log)
        .arg("install")
        .arg("--source-bin")
        .arg(&source)
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .unwrap();
    let diagnostic = json(&output);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "committed"
    );
    assert_eq!(fs::read(prefix.join("bin/aros")).unwrap(), b"aros");
}

#[cfg(unix)]
#[test]
fn final_log_failure_after_source_initialization_retains_the_committed_state() {
    fn git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let directory = tempfile::tempdir().unwrap();
    let upstream = directory.path().join("upstream");
    let destination = directory.path().join("new-checkout");
    let log = directory.path().join("source.jsonl");
    fs::create_dir(&upstream).unwrap();
    for directory in ["arch", "compiler", "rom", "developer"] {
        fs::create_dir(upstream.join(directory)).unwrap();
    }
    fs::write(upstream.join("configure"), "fixture configure\n").unwrap();
    fs::write(upstream.join("Makefile.in"), "fixture makefile\n").unwrap();
    git(&upstream, &["init", "--initial-branch=main"]);
    git(&upstream, &["add", "."]);
    git(
        &upstream,
        &[
            "-c",
            "user.name=AROS Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    );

    let output = command()
        .current_dir(directory.path())
        .env("AROS_TEST_LOG_FAIL_EVENT", "invocation.complete")
        .args(["--diagnostic-format=json", "--log-level=info", "--log-file"])
        .arg(&log)
        .args(["source", "init"])
        .arg(&destination)
        .arg("--upstream")
        .arg(&upstream)
        .output()
        .unwrap();
    let diagnostic = json(&output);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["commit_state"],
        "committed"
    );
    assert!(destination.join(".git").is_dir());
    assert!(destination.join("configure").is_file());
}

#[test]
fn repository_discovery_has_its_own_stable_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .args(["--diagnostic-format=json", "clean", "--preset", "pc-x86_64"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0101");
    assert_eq!(value["diagnostics"][0]["stage"], "repository_discovery");
    assert_eq!(value["diagnostics"][0]["context"]["mode"], "clean");
}

#[test]
fn info_is_useful_without_repository_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .arg("info")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Source checkout:        none discovered"));
    assert!(stdout.contains("aros source init PATH"));
    assert!(stdout.contains("AROS state root:"));
    assert!(stdout.contains("Archive cache:"));
    assert!(stdout.contains("Cross-toolchain store:"));
}

#[test]
fn info_never_falls_back_to_a_relative_state_directory() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .env_remove("HOME")
        .args(["--diagnostic-format=json", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("HOME is unset")));
}

#[test]
fn optional_build_tool_check_reaches_tool_resolution_outside_a_checkout() {
    let directory = tempfile::tempdir().unwrap();
    let empty_tools = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .env("AROS_BUILD_TOOLS_DIR", empty_tools.path())
        .args(["--diagnostic-format=json", "build-tools", "check"])
        .output()
        .unwrap();

    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0301");
    assert_eq!(value["diagnostics"][0]["stage"], "tool_resolution");
}

#[test]
fn repository_configuration_is_loaded_from_the_discovered_root() {
    let output = command()
        .current_dir(checkout().join("developer"))
        .arg("info")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("Configured targets:     pc-x86_64"));
}

#[test]
fn pristine_upstream_info_reports_built_in_target_contract() {
    let checkout = temporary_checkout(
        "[[targets]]\nname='temporary'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
    );
    fs::remove_file(checkout.path().join("aros-targets.toml")).unwrap();

    let output = command()
        .current_dir(checkout.path())
        .arg("info")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("pc-x86_64, rpi-aarch64, arm-raspi, opensbi-riscv64"));
    assert!(stdout.contains("built into aros-tools; pristine upstream checkout"));
}

#[test]
fn info_fails_closed_before_output_for_an_invalid_target_configuration() {
    let checkout = temporary_checkout(
        "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\nenabled_typo=true\n",
    );
    let output = command()
        .current_dir(checkout.path())
        .args(["--diagnostic-format=json", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0201");
    assert_eq!(value["diagnostics"][0]["stage"], "configuration");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("enabled_typo")));
}

#[test]
fn info_fails_closed_before_output_for_an_invalid_toolchain_lock() {
    let checkout = temporary_checkout(
        "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
    );
    fs::write(
        checkout.path().join("aros-toolchains.lock.toml"),
        "schema = 1\nrelease_id = 'release-v1'\nunexpected_policy = true\n",
    )
    .unwrap();
    let output = command()
        .current_dir(checkout.path())
        .args(["--diagnostic-format=json", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0201");
    assert_eq!(value["diagnostics"][0]["stage"], "configuration");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("unexpected_policy")));
}

#[test]
fn command_failure_and_local_jsonl_log_are_structured_and_separate() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("aros.jsonl");
    let output = command()
        .args([
            "setup",
            "--preset",
            "pc-x86_64",
            "--local",
            "/tmp",
            "--diagnostic-format=json",
            "--log-format=jsonl",
            "--log-file",
        ])
        .arg(&log)
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0401");
    assert_eq!(value["diagnostics"][0]["context"]["mode"], "setup");

    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "aros-cli-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "diagnostic");
    assert_eq!(records[1]["diagnostic_code"], "AR0401");
    assert!(records
        .iter()
        .all(|record| record.get("timestamp").is_none()));
}

#[test]
fn help_succeeds_without_repository_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("OBSERVABILITY:"));
}

#[test]
fn reported_version_comes_from_the_cargo_package() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("aros {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn board_is_the_only_physical_board_command() {
    let board = command().args(["board", "--help"]).output().unwrap();
    assert!(board.status.success());
    assert!(board.stderr.is_empty());

    let pi = command()
        .args(["--diagnostic-format=json", "pi", "--help"])
        .output()
        .unwrap();
    assert!(pi.stdout.is_empty());
    let value = json(&pi);
    assert_eq!(value["diagnostics"][0]["code"], "AR0001");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("unrecognized subcommand 'pi'"));
}

#[test]
fn clean_rejects_a_preset_path_before_touching_the_filesystem() {
    let output = command()
        .args([
            "clean",
            "--preset",
            "../outside",
            "--diagnostic-format=json",
        ])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0901");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid CMake preset"));
}

#[cfg(unix)]
#[test]
fn child_exit_status_is_preserved_as_structured_context() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let tool = directory.path().join("sccache");
    fs::write(&tool, "#!/bin/sh\necho raw-child-error >&2\nexit 23\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = command()
        .env("PATH", directory.path())
        .args(["ccache", "--clear", "--diagnostic-format=json"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0301");
    assert_eq!(value["diagnostics"][0]["context"]["tool"], "sccache");
    assert_eq!(value["diagnostics"][0]["context"]["exit_code"], 23);
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("stderr:\nraw-child-error"));
}

#[cfg(unix)]
#[test]
fn json_diagnostics_preserve_one_envelope_after_a_noisy_successful_child() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let tool = directory.path().join("sccache");
    fs::write(
        &tool,
        "#!/bin/sh\ncase \"$1\" in\n  -z) printf '%s\\n' successful-child-warning >&2; exit 0;;\n  -s) printf '%s\\n' failing-child-error >&2; exit 23;;\nesac\nexit 64\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = command()
        .env("PATH", directory.path())
        .args(["--diagnostic-format=json", "ccache", "--clear"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        output.stderr.starts_with(b"{"),
        "machine diagnostics must not prefix raw child stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    let diagnostics = value["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["severity"] == "warning"
            && diagnostic["context"]["tool"] == "sccache"
            && diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("successful-child-warning"))
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["severity"] == "error"
            && diagnostic["context"]["tool"] == "sccache"
            && diagnostic["context"]["exit_code"] == 23
            && diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("failing-child-error"))
    }));
}

#[cfg(unix)]
#[test]
fn boot_test_rejects_an_early_qemu_exit_without_positive_evidence() {
    use std::os::unix::fs::PermissionsExt;

    let checkout = temporary_checkout(
        "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
    );
    let boot = checkout.path().join("build/pc-x86_64/SYS/boot/pc");
    fs::create_dir_all(&boot).unwrap();
    fs::write(boot.join("bootstrap"), b"bootstrap fixture").unwrap();
    fs::write(boot.join("kernel"), b"kernel fixture").unwrap();
    let nested = checkout.path().join("developer/invocation");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("module.elf"), b"module fixture").unwrap();

    let tools = tempfile::tempdir().unwrap();
    let qemu = tools.path().join("qemu-system-x86_64");
    fs::write(
        &qemu,
        "#!/bin/sh\nif test -f module.elf; then exit 42; fi\nexit 43\n",
    )
    .unwrap();
    fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755)).unwrap();
    let evidence_root = nested.join("evidence");

    let output = command()
        .current_dir(&nested)
        .env("PATH", tools.path())
        .args([
            "--diagnostic-format=json",
            "test",
            "--preset",
            "pc-x86_64",
            "--timeout",
            "1",
            "--module",
            "module.elf",
            "--evidence",
        ])
        .arg("evidence")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("PASS:"), "{stdout}");
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0701");
    assert_eq!(value["diagnostics"][0]["stage"], "boot_validation");
    let message = value["diagnostics"][0]["message"].as_str().unwrap();
    assert!(message.contains("retained evidence"), "{message}");
    assert_eq!(value["diagnostics"][0]["context"]["exit_code"], 42);
    assert!(value["diagnostics"][0]["context"]["tool"]
        .as_str()
        .is_some_and(|tool| tool.ends_with("qemu-system-x86_64")));

    let runs = fs::read_dir(&evidence_root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(runs.len(), 1, "one private evidence directory per run");
    let run = runs[0].path();
    assert!(run.join("serial.log").is_file());
    assert!(run.join("exceptions.log").is_file());
}
