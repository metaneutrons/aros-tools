//! Process-boundary tests for model-value completion in the installed shells.

use aros_common::board_registry::built_in_board_registry;
use std::collections::BTreeSet;
#[cfg(unix)]
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn generate(directory: &Path, shell: &str, external_registry: Option<&Path>) -> String {
    let mut command = Command::new(aros());
    command
        .current_dir(directory)
        .args(["completions", shell])
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE")
        .env_remove("AROS_BOARDS_FILE");
    if let Some(path) = external_registry {
        command.env("AROS_BOARDS_FILE", path);
    }
    let output = command.output().expect("run aros completions");
    assert!(
        output.status.success(),
        "{shell}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{shell} completion emitted stderr"
    );
    String::from_utf8(output.stdout).expect("completion script is UTF-8")
}

fn candidate_set(output: &Output) -> BTreeSet<String> {
    assert!(
        output.status.success(),
        "shell completion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "shell completion emitted stderr");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split('\t').next().unwrap_or(line).trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

fn model_set() -> BTreeSet<String> {
    built_in_board_registry()
        .expect("embedded board catalog")
        .boards()
        .map(|board| board.id().as_str().to_owned())
        .collect()
}

#[cfg(unix)]
fn rpi_model_set() -> BTreeSet<String> {
    model_set()
        .into_iter()
        .filter(|model| model.starts_with("rpi"))
        .collect()
}

fn write_external_registry(path: &Path) {
    let fixture = include_str!("../../../profiles/boards/registry-v1.toml")
        .replace("rpi3", "external-only-board");
    aros_common::board_registry::BoardRegistry::parse("external-fixture", &fixture)
        .expect("external sentinel fixture remains a valid catalog");
    fs::write(path, fixture).expect("write external catalog sentinel");
}

#[cfg(unix)]
fn available_shell(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

#[cfg(not(unix))]
fn available_shell(_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn fake_command_path(root: &Path) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("fake-bin");
    fs::create_dir(&bin).expect("create isolated fake command directory");
    let fake_aros = bin.join("aros");
    fs::write(
        &fake_aros,
        "#!/bin/sh\nprintf 'called\\n' >> \"$AROS_TEST_COMPLETION_MARKER\"\nprintf 'external-only-board\\n'\n",
    )
    .expect("write fake aros sentinel");
    fs::set_permissions(&fake_aros, fs::Permissions::from_mode(0o755))
        .expect("make fake aros executable");

    let mut paths = vec![bin];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    env::join_paths(paths).expect("assemble isolated PATH")
}

#[cfg(unix)]
fn run_bash(
    shell: &Path,
    script_path: &Path,
    external_registry: &Path,
    path: &std::ffi::OsStr,
    marker: &Path,
    case: &str,
) -> Output {
    const HARNESS: &str = r#"
source "$1"
case "$2" in
    all) COMP_WORDS=(aros board init --model ""); COMP_CWORD=4 ;;
    prefix) COMP_WORDS=(aros board init --model rpi); COMP_CWORD=4 ;;
    equals) COMP_WORDS=(aros board init --model=rpi); COMP_CWORD=3 ;;
    split-equals) COMP_WORDS=(aros board init --model = rp); COMP_CWORD=5 ;;
    current-equals) COMP_WORDS=(aros board init --model =); COMP_CWORD=4 ;;
    profile-collision) COMP_WORDS=(aros board init --profile build --model rpi); COMP_CWORD=6 ;;
    global-collision) COMP_WORDS=(aros --log-file board board init --model rpi); COMP_CWORD=6 ;;
    *) exit 97 ;;
esac
_aros
printf '%s\n' "${COMPREPLY[@]}"
"#;

    Command::new(shell)
        .args(["--noprofile", "--norc", "-c", HARNESS, "aros-test"])
        .arg(script_path)
        .arg(case)
        .env("AROS_BOARDS_FILE", external_registry)
        .env("AROS_TEST_COMPLETION_MARKER", marker)
        .env("PATH", path)
        .output()
        .expect("run bash completion function")
}

#[cfg(unix)]
fn run_zsh(
    shell: &Path,
    script_path: &Path,
    external_registry: &Path,
    path: &std::ffi::OsStr,
    marker: &Path,
    case: &str,
) -> Output {
    const HARNESS: &str = r#"
case "$2" in
    all) words=(aros board init --model ""); CURRENT=5 ;;
    prefix) words=(aros board init --model rpi); CURRENT=5 ;;
    equals) words=(aros board init --model=rpi); CURRENT=4 ;;
    profile-collision) words=(aros board init --profile build --model rpi); CURRENT=7 ;;
    global-collision) words=(aros --log-file board board init --model rpi); CURRENT=7 ;;
    *) exit 97 ;;
esac
PREFIX="${words[CURRENT]}"
# Non-interactive zsh has no active completion context. Capture compadd's
# candidates and emulate its PREFIX filtering while running the generated
# completion function in zsh itself.
compset() {
    if [[ "$1" == -P && "$2" == '*=' ]]; then
        PREFIX="${words[CURRENT]#*=}"
    fi
}
compadd() {
    [[ "$1" == -- ]] && shift
    local candidate
    for candidate in "$@"; do
        [[ "$candidate" == "$PREFIX"* ]] && print -r -- "$candidate"
    done
}
source "$1"
"#;

    Command::new(shell)
        .args(["-f", "-c", HARNESS, "aros-test"])
        .arg(script_path)
        .arg(case)
        .env("AROS_BOARDS_FILE", external_registry)
        .env("AROS_TEST_COMPLETION_MARKER", marker)
        .env("PATH", path)
        .output()
        .expect("run zsh completion function")
}

#[cfg(unix)]
fn run_fish(
    shell: &Path,
    script_path: &Path,
    external_registry: &Path,
    path: &std::ffi::OsStr,
    marker: &Path,
    command_line: &str,
) -> Output {
    const HARNESS: &str = r"
source $argv[1]
complete -C $argv[2]
";

    Command::new(shell)
        .args(["--no-config", "-c", HARNESS])
        .arg(script_path)
        .arg(command_line)
        .env("AROS_BOARDS_FILE", external_registry)
        .env("AROS_TEST_COMPLETION_MARKER", marker)
        .env("PATH", path)
        .output()
        .expect("run fish completion engine")
}

#[test]
fn model_value_completions_use_the_embedded_registry() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let external_registry = temporary.path().join("external-registry.toml");
    write_external_registry(&external_registry);
    let models = model_set();
    let shell_names = ["bash", "zsh", "fish"];
    let mut scripts = Vec::new();

    for shell in shell_names {
        let baseline = generate(temporary.path(), shell, None);
        let configured = generate(temporary.path(), shell, Some(&external_registry));
        assert_eq!(
            configured, baseline,
            "{shell} completion generation must ignore an external catalog path"
        );
        assert!(
            !configured.contains("external-only-board"),
            "{shell} completion script exposed an external catalog entry"
        );
        for model in &models {
            assert!(
                configured.contains(model),
                "{shell} completion script omitted embedded model {model}"
            );
        }
        if shell == "fish" {
            let model_values = models.iter().cloned().collect::<Vec<_>>().join(" ");
            assert!(
                configured.contains(&format!("-l model -r -a '{model_values}'")),
                "fish completion must declare embedded models as required --model values"
            );
        }
        scripts.push((shell, configured));
    }

    #[cfg(unix)]
    {
        let marker = temporary.path().join("external-lookup-ran");
        let path = fake_command_path(temporary.path());
        let expected_rpi = rpi_model_set();
        let script_paths = scripts
            .into_iter()
            .map(|(shell, contents)| {
                let path = temporary.path().join(format!("aros.{shell}"));
                fs::write(&path, contents).expect("write generated script for shell execution");
                (shell, path)
            })
            .collect::<Vec<_>>();

        for (shell_name, script_path) in script_paths {
            let Some(shell) = available_shell(shell_name) else {
                // Fish is intentionally optional; its emitted option declaration
                // was checked above, and its native completion engine is used
                // whenever it is installed on the test host.
                continue;
            };
            match shell_name {
                "bash" => {
                    for case in [
                        "all",
                        "prefix",
                        "equals",
                        "split-equals",
                        "current-equals",
                        "profile-collision",
                        "global-collision",
                    ] {
                        let output = run_bash(
                            &shell,
                            &script_path,
                            &external_registry,
                            &path,
                            &marker,
                            case,
                        );
                        let expected = if matches!(case, "all" | "current-equals") {
                            models.clone()
                        } else if case == "equals" {
                            expected_rpi
                                .iter()
                                .map(|model| format!("--model={model}"))
                                .collect()
                        } else {
                            expected_rpi.clone()
                        };
                        assert_eq!(candidate_set(&output), expected, "bash case {case}");
                    }
                }
                "zsh" => {
                    for case in [
                        "all",
                        "prefix",
                        "equals",
                        "profile-collision",
                        "global-collision",
                    ] {
                        let output = run_zsh(
                            &shell,
                            &script_path,
                            &external_registry,
                            &path,
                            &marker,
                            case,
                        );
                        let expected = if case == "all" {
                            models.clone()
                        } else {
                            expected_rpi.clone()
                        };
                        assert_eq!(candidate_set(&output), expected, "zsh case {case}");
                    }
                }
                "fish" => {
                    for (case, command_line) in [
                        ("all", "aros board init --model "),
                        ("prefix", "aros board init --model rpi"),
                        ("equals", "aros board init --model=rpi"),
                        (
                            "profile-collision",
                            "aros board init --profile build --model rpi",
                        ),
                        (
                            "global-collision",
                            "aros --log-file board board init --model rpi",
                        ),
                    ] {
                        let output = run_fish(
                            &shell,
                            &script_path,
                            &external_registry,
                            &path,
                            &marker,
                            command_line,
                        );
                        let expected = if case == "all" {
                            models.clone()
                        } else {
                            expected_rpi.clone()
                        };
                        let candidates = candidate_set(&output)
                            .into_iter()
                            .map(|candidate| {
                                if case == "equals" {
                                    candidate
                                        .strip_prefix("--model=")
                                        .unwrap_or(&candidate)
                                        .to_owned()
                                } else {
                                    candidate
                                }
                            })
                            .collect::<BTreeSet<_>>();
                        assert_eq!(candidates, expected, "fish case {case}");
                    }
                }
                _ => unreachable!("shell list is fixed"),
            }
        }
        assert!(
            !marker.exists(),
            "completion execution must not invoke an external aros/catalog lookup"
        );
    }
}
