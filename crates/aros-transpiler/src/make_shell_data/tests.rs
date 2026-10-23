use super::{evaluate, fold_stdout, Evaluator, ShellDataContext, ShellDataOutput, MAX_OUTPUT};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs,
    path::{Component, Path},
    process::Command,
};

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("create shell-data fixture");
    for (name, contents) in [
        ("alpha.txt", "alpha"),
        ("beta.txt", "beta"),
        ("space file.txt", "space"),
        ("sub dir/one file", "one"),
        ("sub dir/alpha.txt", "nested"),
    ] {
        let path = root.path().join(name);
        fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("create fixture directory");
        fs::write(path, contents).expect("write fixture file");
    }
    root
}

fn is_fixture_path(path: &Path, root: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|part| part == Component::ParentDir)
}

fn run_with_context(
    command: &str,
    default_ifs: bool,
    charge_limit: Option<usize>,
) -> (Result<ShellDataOutput, String>, usize) {
    let fixture = fixture();
    let root = fs::canonicalize(fixture.path()).expect("canonicalize shell-data fixture");
    let directory_root = root.clone();
    let directory = move |path: &str| -> Result<Option<String>, String> {
        let path = Path::new(path);
        if !is_fixture_path(path, &directory_root) {
            return Ok(None);
        }
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => fs::canonicalize(path)
                .map(|path| Some(path.to_string_lossy().into_owned()))
                .map_err(|_| "fixture directory query failed".to_string()),
            _ => Ok(None),
        }
    };
    let file_root = root.clone();
    let regular_file = move |path: &str| -> Result<bool, String> {
        let path = Path::new(path);
        if !is_fixture_path(path, &file_root) {
            return Ok(false);
        }
        Ok(fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
    };
    let glob_root = root.clone();
    let glob = move |pattern: &str| -> Result<Vec<String>, String> {
        if !is_fixture_path(Path::new(pattern), &glob_root) {
            return Ok(Vec::new());
        }
        let matches = glob::glob(pattern).map_err(|_| "fixture glob query failed".to_string())?;
        let mut paths = Vec::new();
        for entry in matches {
            let path = entry.map_err(|_| "fixture glob query failed".to_string())?;
            let path =
                fs::canonicalize(path).map_err(|_| "fixture glob query failed".to_string())?;
            if !path.starts_with(&glob_root) {
                return Err("fixture glob result escaped its root".into());
            }
            paths.push(path.to_string_lossy().into_owned());
        }
        paths.sort();
        Ok(paths)
    };
    let charged = Cell::new(0usize);
    let charge = |amount: usize| -> Result<(), String> {
        let total = charged.get().saturating_add(amount);
        charged.set(total);
        if charge_limit.is_some_and(|limit| total > limit) {
            Err("fixture work budget exhausted".into())
        } else {
            Ok(())
        }
    };
    let context = ShellDataContext {
        working_directory: &root,
        directory: &directory,
        regular_file: &regular_file,
        glob: &glob,
        default_shell_controls: default_ifs,
        charge: &charge,
    };
    let output = evaluate(command, &context);
    (output, charged.get())
}

fn run(command: &str) -> Result<ShellDataOutput, String> {
    run_with_context(command, true, None).0
}

fn run_native(root: &Path, command: &str) -> (String, u8) {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .env_clear()
        .output()
        .expect("run fixed native shell differential fixture");
    let stdout = String::from_utf8(output.stdout).expect("native shell emits UTF-8 fixture output");
    let status = u8::try_from(output.status.code().expect("native shell exits normally"))
        .expect("native shell status fits in a byte");
    (stdout, status)
}

fn assert_matches_native(command: &str) {
    let fixture = fixture();
    let root = fs::canonicalize(fixture.path()).expect("canonicalize shell-data fixture");
    let (data, _) = run_with_context(command, true, None);
    let data = data.expect("evaluate supported shell-data command");
    let (native_stdout, native_status) = run_native(&root, command);
    assert_eq!(data.status, native_status);
    assert_eq!(fold_stdout(&data.stdout), fold_stdout(&native_stdout));
}

#[test]
fn interprets_cd_for_if_and_file_tests_with_quoted_names() {
    let command = r#"
cd "sub dir"
for item in "one file" missing; do
    if test -f "$item"; then
        printf 'found:%s\n' "$item"
    else
        printf 'missing:%s\n' "$item"
    fi
done
"#;
    let output = run(command).expect("interpret supported file-query script");
    assert_eq!(output.stdout, "found:one file\nmissing:missing\n");
    assert_eq!(output.status, 0);
    assert_matches_native(command);
}

#[test]
fn supports_bracket_directory_tests_and_independent_loop_names() {
    let command = r#"
for selected in "space file.txt" alpha.txt; do
    if [ -f "$selected" ]; then printf '<%s>\n' "$selected"; fi
done
if [ -d "sub dir" ]; then printf 'directory\n'; fi
"#;
    let output = run(command).expect("interpret bracket and directory tests");
    assert_eq!(output.stdout, "<space file.txt>\n<alpha.txt>\ndirectory\n");
    assert_eq!(output.status, 0);
    assert_matches_native(command);
}

#[test]
fn continuation_preserves_comment_token_boundaries() {
    for command in ["echo ok \\\n#comment", "echo pre\\\n#literal"] {
        assert_matches_native(command);
    }
    assert_eq!(run("echo ok \\\n#comment").unwrap().stdout, "ok\n");
}

#[test]
fn shell_owned_loop_parameters_refuse_even_in_inactive_branches() {
    for name in ["IFS", "CDPATH", "PATH", "PWD", "OLDPWD", "OPTIND", "PPID"] {
        let command = format!("if false; then for {name} in value; do :; done; fi");
        assert!(run(&command).is_err());
    }
    assert!(run_with_context("cd 'sub dir'", false, None).0.is_err());
}

#[test]
fn handles_empty_iteration_else_branches_and_final_status() {
    let command = r"
for empty in; do printf 'unreachable'; done
if true; then printf 'then\n'; else printf 'else\n'; fi
if false; then true; else false; fi
";
    let output = run(command).expect("interpret empty loop and branches");
    assert_eq!(output.stdout, "then\n");
    assert_eq!(output.status, 1);
    assert_matches_native(command);
}

#[test]
fn literal_echo_printf_repetition_and_loop_variable_lifetime_match_shell() {
    let command = r#"
echo literal text
printf '%s %% %s\n' one two three four
for chosen in first last; do :; done
printf '<%s>\n' "$chosen"
"#;
    let output = run(command).expect("interpret literal output and loop lifetime");
    assert_eq!(
        output.stdout,
        "literal text\none % two\nthree % four\n<last>\n"
    );
    assert_eq!(output.status, 0);
    assert_matches_native(command);
}

#[test]
fn expands_only_unquoted_globs_and_preserves_unmatched_patterns() {
    let command = r#"
printf '<%s>\n' *.txt
printf '<%s>\n' "*.txt"
printf '<%s>\n' missing-*.zzz
"#;
    let output = run(command).expect("interpret supported glob words");
    assert_eq!(
        output.stdout,
        "<alpha.txt>\n<beta.txt>\n<space file.txt>\n<*.txt>\n<missing-*.zzz>\n"
    );
    assert_matches_native(command);
}

#[test]
fn unquoted_parameter_splitting_requires_bound_default_ifs() {
    let command = r#"for picked in "first second"; do printf '<%s>\n' $picked; done"#;
    let (output, _) = run_with_context(command, true, None);
    let output = output.expect("split parameter with explicitly bound default IFS");
    assert_eq!(output.stdout, "<first>\n<second>\n");

    let (output, _) = run_with_context(command, false, None);
    assert_eq!(
        output.expect_err("refuse unbound default IFS").as_str(),
        "shell parameter splitting needs a bound IFS context"
    );
}

#[test]
fn refuses_unbound_parameters_and_unsupported_shell_features() {
    let error = run(r#"printf '%s' "$private_marker""#).expect_err("refuse unbound parameter");
    assert_eq!(error, "unbound shell data parameter");
    assert!(!error.contains("private_marker"));

    for command in [
        "uname",
        "echo safe > output.txt",
        r#"printf '%s' "$(uname)""#,
        "printf '%d' 7",
        "if false; then uname; fi",
    ] {
        assert!(run(command).is_err(), "unexpectedly accepted {command:?}");
    }
}

#[test]
fn caller_and_internal_work_limits_refuse_without_exposing_command_text() {
    let command = "for item in a b c d e f g h; do printf '%s' \"$item\"; done";
    let limit = command.len() + 180;
    let (output, charged) = run_with_context(command, true, Some(limit));
    let error = output.expect_err("exhaust caller budget during a loop");
    assert_eq!(error, "fixture work budget exhausted");
    assert!(charged > limit);
    assert!(!error.contains(command));

    let payload = "a".repeat(400_000);
    let high_work = format!(
        "for item in {payload}; do for again in x y z; do printf '%s' \"$item\"; done; done"
    );
    let (output, _) = run_with_context(&high_work, true, None);
    assert_eq!(
        output.expect_err("refuse a loop whose expansion exceeds the work cap"),
        "shell data work exceeds its limit"
    );
}

#[test]
fn append_refuses_output_beyond_the_fixed_cap() {
    let cwd = Path::new("/fixture");
    let directory = |_path: &str| -> Result<Option<String>, String> { Ok(None) };
    let regular_file = |_path: &str| -> Result<bool, String> { Ok(false) };
    let glob = |_pattern: &str| -> Result<Vec<String>, String> { Ok(Vec::new()) };
    let charge = |_amount: usize| -> Result<(), String> { Ok(()) };
    let context = ShellDataContext {
        working_directory: cwd,
        directory: &directory,
        regular_file: &regular_file,
        glob: &glob,
        default_shell_controls: true,
        charge: &charge,
    };
    let mut evaluator = Evaluator {
        context: &context,
        cwd: "/fixture".into(),
        variables: BTreeMap::new(),
        stdout: String::new(),
        steps: 0,
    };
    let oversized = "x".repeat(MAX_OUTPUT + 1);
    assert_eq!(
        evaluator
            .append(&oversized)
            .expect_err("refuse oversized stdout"),
        "shell data output exceeds its limit"
    );
    assert!(evaluator.stdout.is_empty());
}

#[test]
fn shell_output_debug_reports_length_without_raw_stdout() {
    let output = run("printf '%s' private-output-marker").expect("interpret literal printf");
    let debug = format!("{output:?}");
    assert!(debug.contains("stdout_bytes"));
    assert!(debug.contains("status"));
    assert!(!debug.contains("private-output-marker"));
    assert_eq!(output.stdout, "private-output-marker");
}

#[test]
fn folds_lf_crlf_and_leaves_bare_carriage_return_untouched() {
    assert_eq!(
        fold_stdout("\nalpha\n\nbeta\r\n\r\ngamma\rdelta\n"),
        " alpha  beta  gamma\rdelta"
    );
    assert_eq!(fold_stdout("\r\n"), "");
}
