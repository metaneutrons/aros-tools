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
fn repository_discovery_has_its_own_stable_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .args(["--diagnostic-format=json", "clean"])
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
fn boot_test_rejects_an_early_qemu_exit_without_positive_evidence() {
    use std::os::unix::fs::PermissionsExt;

    let checkout = temporary_checkout(
        "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n",
    );
    let boot = checkout.path().join("build/pc-x86_64/SYS/boot/pc");
    fs::create_dir_all(&boot).unwrap();
    fs::write(boot.join("bootstrap"), b"bootstrap fixture").unwrap();
    fs::write(boot.join("kernel"), b"kernel fixture").unwrap();

    let tools = tempfile::tempdir().unwrap();
    let qemu = tools.path().join("qemu-system-x86_64");
    fs::write(&qemu, "#!/bin/sh\nexit 42\n").unwrap();
    fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755)).unwrap();
    let evidence_root = checkout.path().join("evidence");

    let output = command()
        .current_dir(checkout.path())
        .env("PATH", tools.path())
        .args([
            "--diagnostic-format=json",
            "test",
            "--preset",
            "pc-x86_64",
            "--timeout",
            "1",
            "--evidence",
        ])
        .arg(&evidence_root)
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
