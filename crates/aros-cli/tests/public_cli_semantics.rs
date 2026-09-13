//! Process-level semantic cases for public `aros` examples and contracts.
//!
//! The structural contract snapshots cover every public command and option. These
//! cases exercise the meanings that cannot be inferred from Clap alone: default
//! effects, rejected stale values, environment resolution, and controlled local
//! publication. They deliberately use only temporary directories and never
//! discover a checkout, contact a network service, or touch a physical board.

use aros_release::archive::BINARIES;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const BOARDS_WORKFLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/workflows/boards.md"
));
const INSTALLATION_GUIDE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/getting-started/installation.md"
));

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn run(arguments: &[&str]) -> Output {
    Command::new(aros())
        .args(arguments)
        .output()
        .expect("public CLI semantic case must execute")
}

fn assert_success(output: &Output, case: &str) {
    assert!(
        output.status.success(),
        "{case} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, case: &str) -> String {
    assert!(
        !output.status.success(),
        "{case} unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn public_board_init_semantic_cases_cover_models_defaults_and_environment() {
    let temporary = tempfile::tempdir().expect("temporary semantic-case root");
    let cases = [
        ("rpi3", "native-tftp"),
        ("rpi4", "native-tftp"),
        ("rpi5", "native-tftp"),
        ("milk-v-titan", "uefi-esp"),
    ];

    for (model, transport) in cases {
        let config = temporary.path().join(format!("{model}.toml"));
        let config = config.to_str().expect("temporary path is UTF-8");
        let output = run(&[
            "board",
            "init",
            "--profile",
            model,
            "--model",
            model,
            "--config",
            config,
        ]);
        assert_success(&output, "board init dry-run default transport");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("Model: {model}")));
        assert!(stdout.contains(&format!("Transport: {transport}")));
        assert!(
            !Path::new(config).exists(),
            "board init must stay non-mutating until --apply"
        );
    }

    let applied_config = temporary.path().join("configured-by-environment.toml");
    let output = Command::new(aros())
        .env("AROS_BOARDS_FILE", &applied_config)
        .args([
            "board",
            "init",
            "--profile",
            "semantic-rpi5",
            "--model",
            "rpi5",
            "--apply",
        ])
        .output()
        .expect("environment semantic case must execute");
    assert_success(&output, "board init environment-selected apply");
    let contents = fs::read_to_string(&applied_config).expect("applied template exists");
    assert!(contents.contains("[boards.semantic-rpi5]"));
    assert!(contents.contains("model = \"rpi5\""));
    assert!(contents.contains("transport = \"native-tftp\""));

    assert!(
        BOARDS_WORKFLOW.contains("aros board init --profile rpi4-usb --model rpi4"),
        "the documented board example must stay aligned with the exercised model selector"
    );
}

#[test]
fn public_parser_semantic_cases_reject_stale_values_and_conflicts_before_setup() {
    let missing_model = assert_failure(
        &run(&["board", "init", "--profile", "missing-model"]),
        "board init without its required model",
    );
    assert!(missing_model.contains("--model <MODEL>"));

    let stale_model = assert_failure(
        &run(&[
            "board",
            "init",
            "--profile",
            "stale-model",
            "--model",
            "rpi2",
        ]),
        "board init with a deliberately stale model value",
    );
    assert!(stale_model.contains("invalid value 'rpi2'"));
    assert!(stale_model.contains("rpi3, rpi4, rpi5, milk-v-titan"));

    let conflicting_setup = assert_failure(
        &run(&["setup", "--preset", "pc-x86_64", "--all"]),
        "setup with mutually exclusive scope selectors",
    );
    assert!(conflicting_setup.contains("cannot be used with '--all'"));
    assert!(
        !conflicting_setup.contains("repository discovery"),
        "parser conflicts must fail before repository discovery"
    );
}

#[test]
fn diagnostic_format_environment_has_the_documented_process_effect() {
    let output = Command::new(aros())
        .env("AROS_DIAGNOSTIC_FORMAT", "json")
        .arg("deliberately-stale-public-command")
        .output()
        .expect("diagnostic environment semantic case must execute");
    assert_failure(&output, "unknown command under JSON diagnostic environment");
    let diagnostic: Value =
        serde_json::from_slice(&output.stderr).expect("environment-selected diagnostics are JSON");
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"][0]["code"], "AR0001");
}

#[test]
fn installation_intent_conflicts_are_rejected_before_logging_or_repository_access() {
    let temporary = tempfile::tempdir().expect("temporary invocation root");
    let cases: &[&[&str]] = &[
        &["setup", "--preset", "pc-x86_64", "--force"],
        &["host-compiler", "install", "--force"],
        &["toolchain", "install", "--preset", "pc-x86_64", "--force"],
    ];
    for (index, arguments) in cases.iter().enumerate() {
        let log_file = temporary.path().join(format!("conflict-{index}.jsonl"));
        let output = Command::new(aros())
            .current_dir(temporary.path())
            .env("AROS_OFFLINE", "true")
            .arg("--log-file")
            .arg(&log_file)
            .args(*arguments)
            .output()
            .expect("installation conflict semantic case must execute");
        let diagnostic = assert_failure(&output, "environment-selected offline conflict");
        assert!(diagnostic.contains("cannot be used with"));
        assert!(diagnostic.contains("--force"));
        assert!(diagnostic.contains("--offline"));
        assert!(
            !diagnostic.contains("repository discovery"),
            "an invocation conflict must precede repository discovery"
        );
        assert!(
            !log_file.exists(),
            "an invocation conflict must precede log-file creation"
        );
    }
}

fn create_checkout_markers(root: &Path) {
    for directory in ["arch", "compiler", "rom", "nested"] {
        fs::create_dir_all(root.join(directory)).expect("checkout marker directory");
    }
    for file in ["configure", "Makefile.in"] {
        fs::write(root.join(file), "").expect("checkout marker file");
    }
}

#[test]
fn explicit_clean_scope_and_relative_log_file_keep_the_invocation_origin() {
    let temporary = tempfile::tempdir().expect("temporary checkout root");
    let checkout = temporary.path().join("AROS");
    create_checkout_markers(&checkout);
    let selected = checkout.join("build/pc-x86_64");
    let preserved = checkout.join("build/other-preset");
    let cache = checkout.join("cache/sentinel");
    fs::create_dir_all(&selected).expect("selected build directory");
    fs::create_dir_all(&preserved).expect("other build directory");
    fs::create_dir_all(cache.parent().expect("cache parent")).expect("cache directory");
    fs::write(selected.join("selected"), "remove").expect("selected build payload");
    fs::write(preserved.join("preserved"), "keep").expect("preserved build payload");
    fs::write(&cache, "keep").expect("cache sentinel");

    let nested = checkout.join("nested");
    let preview = Command::new(aros())
        .current_dir(&nested)
        .args(["clean", "--preset", "pc-x86_64", "--dry-run"])
        .output()
        .expect("clean preview semantic case must execute");
    assert_success(&preview, "explicit clean preview");
    assert!(String::from_utf8_lossy(&preview.stdout).contains(&selected.display().to_string()));
    assert!(
        selected.exists(),
        "clean preview must not remove its target"
    );

    let log_file = nested.join("invocation.jsonl");
    let applied = Command::new(aros())
        .current_dir(&nested)
        .args([
            "--log-level",
            "info",
            "--log-file",
            "invocation.jsonl",
            "clean",
            "--preset",
            "pc-x86_64",
        ])
        .output()
        .expect("explicit clean apply semantic case must execute");
    assert_success(&applied, "explicit preset clean");
    assert!(
        !selected.exists(),
        "selected build directory must be removed"
    );
    assert!(preserved.exists(), "other build presets must be preserved");
    assert!(cache.exists(), "checkout caches must be preserved");
    assert!(
        log_file.is_file(),
        "a relative log file must resolve from the invocation directory"
    );
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &[u8]) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::write(path, contents).expect("write synthetic suite member");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make synthetic suite member executable");
}

#[cfg(unix)]
#[test]
fn native_suite_install_semantic_cases_require_an_exact_snapshotted_suite() {
    let temporary = tempfile::tempdir().expect("temporary suite root");
    let source = temporary.path().join("source");
    let prefix = temporary.path().join("prefix");
    fs::create_dir_all(&source).expect("create source suite directory");
    fs::create_dir_all(&prefix).expect("create installation prefix");
    for (index, name) in BINARIES.iter().enumerate() {
        write_executable(
            &source.join(name),
            format!("semantic-suite-member-{index}\n").as_bytes(),
        );
    }

    let output = Command::new(aros())
        .args(["install", "--source-bin"])
        .arg(&source)
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .expect("native suite install semantic case must execute");
    assert_success(&output, "exact native suite installation");
    for (index, name) in BINARIES.iter().enumerate() {
        assert_eq!(
            fs::read(prefix.join("bin").join(name)).expect("installed suite member"),
            format!("semantic-suite-member-{index}\n").as_bytes()
        );
    }

    let stale_source = temporary.path().join("stale-source");
    let stale_prefix = temporary.path().join("stale-prefix");
    fs::create_dir_all(&stale_source).expect("create stale source suite directory");
    fs::create_dir_all(&stale_prefix).expect("create stale installation prefix");
    for name in BINARIES {
        write_executable(&stale_source.join(name), b"expected-member\n");
    }
    write_executable(&stale_source.join("unexpected-tool"), b"stale-member\n");
    let rejected = Command::new(aros())
        .args(["install", "--source-bin"])
        .arg(&stale_source)
        .arg("--prefix")
        .arg(&stale_prefix)
        .output()
        .expect("stale suite semantic case must execute");
    let rejected_diagnostic = assert_failure(&rejected, "stale suite inventory");
    assert!(rejected_diagnostic.contains("source suite inventory is not exact"));
    assert!(
        !stale_prefix.join("bin").exists(),
        "a rejected suite must not publish a partial destination"
    );
    assert!(
        INSTALLATION_GUIDE.contains("validates the exact eight-file inventory"),
        "the installation guide must keep the exercised exact-inventory boundary"
    );
}
