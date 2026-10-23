use super::{GnuShellQueryBudget, GnuShellQueryExecution};
use aros_common::{
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    sha256_bytes, CancellationToken, Sha256Digest,
};
use aros_transpiler::make_shell_effects::gnu_query::{
    GnuShellQuery, GnuShellQueryResult, LiteralQueryVariable,
};
use std::{
    cell::Cell,
    env, fs,
    os::unix::fs::{symlink, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

mod source_bindings;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    program: PathBuf,
    program_sha256: Sha256Digest,
    original: NativeMakeInvocation,
}

impl Fixture {
    fn new(extra_environment: &[(&str, &str)], argument_zero: Option<&str>) -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("gnu-query-")
            .tempdir_in("/tmp")
            .expect("create a private GNU query test directory");
        let root = temporary
            .path()
            .canonicalize()
            .expect("canonicalize the test directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("make the test directory private");
        assert_syntax_safe(&root);
        fs::write(
            root.join("original.mk"),
            "$(error ORIGINAL_INPUT_MUST_NOT_RUN_IN_QUERY)\n.PHONY: original-goal\noriginal-goal:\n\t@:\n",
        )
        .expect("write the original fixture makefile");

        let mut environment = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LC_ALL".to_owned(), "C".to_owned()),
        ];
        environment.extend(
            extra_environment
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        );
        let arguments = vec![
            "-r".to_owned(),
            "-f".to_owned(),
            "original.mk".to_owned(),
            "QUERY_ARGUMENT=selected-argument".to_owned(),
            "original-goal".to_owned(),
        ];
        let original = NativeMakeInvocation::new(arguments, root.clone(), environment)
            .expect("create the original Make invocation");
        let original = if let Some(value) = argument_zero {
            original
                .with_argument_zero(value.to_owned())
                .expect("select argv0")
        } else {
            original
        };

        let program = selected_gnu_make();
        let program_sha256 = super::super::measure_executable(&program)
            .expect("measure the selected GNU Make executable")
            .0;
        Self {
            _temporary: temporary,
            root,
            program,
            program_sha256,
            original,
        }
    }

    fn query_directory(&self, label: &str) -> PrivateNativeCaptureDirectory {
        assert!(label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
        let path = self.root.join(format!("query-{label}"));
        fs::create_dir(&path).expect("create a fresh query directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("make the query directory private");
        let path = path
            .canonicalize()
            .expect("canonicalize the query directory");
        assert_syntax_safe(&path);
        PrivateNativeCaptureDirectory::open(&path).expect("bind the private query directory")
    }

    fn request<'a>(
        &'a self,
        directory: &'a PrivateNativeCaptureDirectory,
        bundle: &'a GnuShellQuery,
    ) -> GnuShellQueryExecution<'a> {
        GnuShellQueryExecution {
            program: &self.program,
            program_sha256: &self.program_sha256,
            original: &self.original,
            bundle,
            directory,
        }
    }
}

fn selected_gnu_make() -> PathBuf {
    let command_name = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let path = env::var_os("PATH").expect("PATH is required to select GNU Make");
    let program = env::split_paths(&path)
        .map(|directory| directory.join(command_name))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .unwrap_or_else(|| panic!("{command_name} is required on PATH for GNU query tests"));
    let program = program
        .canonicalize()
        .expect("canonicalize GNU Make selected from PATH");
    let version = Command::new(&program)
        .arg("--version")
        .output()
        .expect("read the selected Make version");
    assert!(version.status.success(), "GNU Make --version failed");
    let version_text = String::from_utf8(version.stdout).expect("GNU Make version is UTF-8");
    let version = version_text
        .lines()
        .find_map(|line| line.strip_prefix("GNU Make "))
        .unwrap_or_else(|| panic!("{command_name} on PATH is not GNU Make"));
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .expect("parse GNU Make major version");
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .expect("parse GNU Make minor version");
    assert!(
        (major, minor) >= (4, 2),
        "GNU Make >= 4.2 is required, found {version}"
    );
    program
}

fn assert_syntax_safe(path: &Path) {
    let text = path.to_str().expect("test path is UTF-8");
    assert!(text.starts_with('/'));
    assert!(text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte)));
    assert!(!text.contains("/../") && !text.contains("/./"));
}

fn render(
    directory: &PrivateNativeCaptureDirectory,
    command: &str,
    variables: &[LiteralQueryVariable<'_>],
) -> GnuShellQuery {
    GnuShellQuery::render(directory.path(), command, variables)
        .expect("render a fresh private GNU query")
}

#[test]
fn real_gnu_make_preserves_literal_dollar_folds_newline_and_records_shell_status() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("literal-output");
    let bundle = render(&directory, "printf '%s\\n' '$literal'; exit 23", &[]);
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");

    let result = budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .expect("query should succeed even when the shell command exits nonzero");
    assert_eq!(result.value, "$literal");
    assert_eq!(result.status, 23);
    assert_eq!(budget.attempted_queries(), 1);
    assert!(budget.observed_output_bytes() > 0);
}

#[test]
fn original_input_counter_really_refuses_when_the_original_phase_is_selected() {
    let fixture = Fixture::new(&[], None);
    let mut command = Command::new(&fixture.program);
    command
        .args(fixture.original.arguments())
        .current_dir(&fixture.root)
        .env_clear()
        .envs(fixture.original.environment());
    let output = aros_common::run_output_with_control(
        &mut command,
        4096,
        Duration::from_secs(5),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(!output.cancelled && !output.timed_out);
    assert!(!output.status.success());
    let stderr = output.stderr.exact_bytes().unwrap();
    assert!(String::from_utf8_lossy(stderr).contains("ORIGINAL_INPUT_MUST_NOT_RUN_IN_QUERY"));
}

#[test]
fn original_environment_arguments_and_argument_zero_select_shell_context() {
    let fixture = Fixture::new(
        &[("QUERY_SELECTION", "selected-environment")],
        Some("selected-query-argv0"),
    );
    let directory = fixture.query_directory("selected-context");
    let variables = [
        LiteralQueryVariable {
            name: "MAKE",
            value: None,
            export: Some(true),
        },
        LiteralQueryVariable {
            name: "QUERY_ARGUMENT",
            value: None,
            export: Some(true),
        },
    ];
    let bundle = render(
        &directory,
        "printf '%s\\n' \"$QUERY_SELECTION|$QUERY_ARGUMENT|$MAKE\"",
        &variables,
    );
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");

    let result = budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .expect("query should use the selected original process context");
    assert_eq!(
        result.value,
        "selected-environment|selected-argument|selected-query-argv0"
    );
    assert_eq!(result.status, 0);
    let diagnostic = fs::read_to_string(directory.path().join(super::QUERY_STDOUT_RECORD)).unwrap();
    assert!(diagnostic.contains("Nothing to be done"));
    assert!(fs::read(directory.path().join(super::QUERY_STDERR_RECORD))
        .unwrap()
        .is_empty());
}

#[test]
fn bounded_shell_diagnostics_are_retained_but_truncated_streams_refuse() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("bounded-diagnostics");
    let bundle = render(&directory, "printf diagnostic >&2; printf value", &[]);
    let budget = GnuShellQueryBudget::new(2, 4 * 1024 * 1024).unwrap();
    let result = budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .unwrap();
    assert_eq!(result.value, "value");
    assert_eq!(
        fs::read(directory.path().join(super::QUERY_STDERR_RECORD)).unwrap(),
        b"diagnostic"
    );
    assert_eq!(
        fs::metadata(directory.path().join(super::QUERY_STDERR_RECORD))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let overflow = fixture.query_directory("truncated-diagnostics");
    let bundle = render(
        &overflow,
        &format!(
            "printf '%s' '{}' >&2; printf value",
            "x".repeat(super::STREAM_BYTES + 1)
        ),
        &[],
    );
    let result = budget.execute(
        &fixture.request(&overflow, &bundle),
        &CancellationToken::default(),
        || Ok(Duration::from_secs(30)),
    );
    assert_eq!(
        result.unwrap_err(),
        "selected GNU query did not complete cleanly"
    );
    assert!(
        fs::metadata(overflow.path().join(super::QUERY_STDERR_RECORD))
            .unwrap()
            .len()
            < super::STREAM_BYTES as u64 + 256
    );
    let outcome = fs::read_to_string(overflow.path().join(super::OUTCOME_RECORD)).unwrap();
    assert!(outcome.contains("stderr_truncated=true"));
    assert_eq!(budget.attempted_queries(), 2);
}

#[test]
fn shell_cannot_change_query_payload_and_return_a_successful_result() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("changed-payload");
    let command = format!(
        "printf altered > {}; printf value",
        directory.path().join("command.txt").display()
    );
    let bundle = render(&directory, &command, &[]);
    let budget = GnuShellQueryBudget::new(1, 4096).unwrap();
    let checks = Cell::new(0);
    let result = budget.execute(
        &fixture.request(&directory, &bundle),
        &CancellationToken::default(),
        || {
            checks.set(checks.get() + 1);
            Ok(Duration::from_secs(30))
        },
    );
    assert_eq!(result.unwrap_err(), "GNU query input payload changed");
    assert_eq!(checks.get(), 3);
    assert_eq!(
        fs::read(directory.path().join("command.txt")).unwrap(),
        b"altered"
    );
}

#[test]
fn shared_count_covers_fresh_query_directories_and_refuses_an_extra_attempt() {
    let fixture = Fixture::new(&[], None);
    let budget = GnuShellQueryBudget::new(2, 4096).expect("valid query budget");
    for label in ["first", "second"] {
        let directory = fixture.query_directory(label);
        let bundle = render(&directory, "printf '%s' 'ok'", &[]);
        let result = budget
            .execute(
                &fixture.request(&directory, &bundle),
                &CancellationToken::default(),
                || Ok(Duration::from_secs(30)),
            )
            .expect("fresh query directory should share the owner budget");
        assert_eq!(result.value, "ok");
    }
    assert_eq!(budget.attempted_queries(), 2);

    let third_directory = fixture.query_directory("third");
    let third_bundle = render(&third_directory, "printf '%s' 'no'", &[]);
    assert!(budget
        .execute(
            &fixture.request(&third_directory, &third_bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .is_err());
    assert_eq!(budget.attempted_queries(), 2);
    assert_eq!(fs::read_dir(third_directory.path()).unwrap().count(), 0);
}

#[test]
fn original_file_options_and_goals_are_replaced_while_allowed_arguments_keep_order() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("argument-rewrite");
    let original = NativeMakeInvocation::new(
        [
            "--no-print-directory",
            "-f",
            "first.mk",
            "MODE=one",
            "-o",
            "old-a.mk",
            "--file=second.mk",
            "--old-file=old-b.mk",
            "goal-one",
            "-r",
            "MODE=two",
            "goal-two",
        ]
        .into_iter()
        .map(str::to_owned),
        fixture.root.clone(),
        fixture
            .original
            .environment()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .expect("create a valid repeated-input original invocation");

    let arguments = super::query_arguments(&original, &directory.path().join("query.mk"))
        .expect("admit and rewrite the selected query phase");
    assert_eq!(
        arguments,
        vec![
            "--no-print-directory".to_owned(),
            "MODE=one".to_owned(),
            "-o".to_owned(),
            "old-a.mk".to_owned(),
            "--old-file=old-b.mk".to_owned(),
            "-r".to_owned(),
            "MODE=two".to_owned(),
            "-f".to_owned(),
            directory
                .path()
                .join("query.mk")
                .to_str()
                .unwrap()
                .to_owned(),
            "__aros_private_shell_query".to_owned(),
        ]
    );
}

#[test]
fn unsupported_startup_and_preload_controls_refuse_before_query_launch() {
    let mut unsupported_startup = Fixture::new(&[], None);
    unsupported_startup.original = NativeMakeInvocation::new(
        [
            "--eval=touch shell-ran",
            "-f",
            "original.mk",
            "original-goal",
        ]
        .into_iter()
        .map(str::to_owned),
        unsupported_startup.root.clone(),
        unsupported_startup
            .original
            .environment()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .expect("construct an unsupported startup option");

    let inherited_preload = Fixture::new(&[("MAKEFILES", "make-hook.mk")], None);
    for (fixture, label) in [
        (&unsupported_startup, "unsupported-startup"),
        (&inherited_preload, "inherited-preload"),
    ] {
        let directory = fixture.query_directory(label);
        let bundle = render(&directory, ": > shell-ran; printf '%s' 'done'", &[]);
        let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
        let owner_checks = Cell::new(0);
        assert!(budget
            .execute(
                &fixture.request(&directory, &bundle),
                &CancellationToken::default(),
                || {
                    owner_checks.set(owner_checks.get() + 1);
                    Ok(Duration::from_secs(30))
                },
            )
            .is_err());
        assert_eq!(owner_checks.get(), 2);
        assert!(!fixture.root.join("shell-ran").exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn output_overflow_refuses_and_poisoned_budget_cannot_be_retried() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("large-output");
    let bundle = render(
        &directory,
        &format!("printf '%s' '{}'", "x".repeat(64)),
        &[],
    );
    let budget = GnuShellQueryBudget::new(2, 16).expect("valid small output budget");

    assert!(budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .is_err());
    assert!(budget.observed_output_bytes() > 16);
    assert_eq!(budget.attempted_queries(), 1);

    let retry_directory = fixture.query_directory("retry-after-overflow");
    let retry_bundle = render(&retry_directory, "printf '%s' 'retry'", &[]);
    assert!(budget
        .execute(
            &fixture.request(&retry_directory, &retry_bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .is_err());
    assert_eq!(budget.attempted_queries(), 1);
    assert_eq!(fs::read_dir(retry_directory.path()).unwrap().count(), 0);
}

#[test]
fn zero_caller_deadline_and_precancellation_do_not_write_or_run_shell_marker() {
    let fixture = Fixture::new(&[], None);
    for (label, remaining, cancelled) in [
        ("zero-deadline", Duration::ZERO, false),
        ("pre-cancelled", Duration::from_secs(30), true),
    ] {
        let directory = fixture.query_directory(label);
        let bundle = render(&directory, ": > shell-ran; printf '%s' 'done'", &[]);
        let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
        let cancellation = CancellationToken::default();
        if cancelled {
            cancellation.cancel();
        }
        let owner_checks = Cell::new(0);
        assert!(budget
            .execute(&fixture.request(&directory, &bundle), &cancellation, || {
                owner_checks.set(owner_checks.get() + 1);
                Ok(remaining)
            },)
            .is_err());
        assert_eq!(owner_checks.get(), 1);
        assert!(!fixture.root.join("shell-ran").exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        assert_eq!(budget.attempted_queries(), 1);
    }
}

#[test]
fn an_expired_shared_deadline_cannot_be_renewed_by_the_owner_callback() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("expired-shared-deadline");
    let bundle = render(&directory, ": > shell-ran; printf '%s' 'done'", &[]);
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
    budget.deadline.set(Some(
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("represent an expired monotonic deadline"),
    ));
    let owner_checks = Cell::new(0);

    assert!(budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || {
                owner_checks.set(owner_checks.get() + 1);
                Ok(Duration::from_secs(60))
            },
        )
        .is_err());
    assert_eq!(owner_checks.get(), 1);
    assert!(!fixture.root.join("shell-ran").exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn original_owner_postcheck_refusal_discards_completed_query_value() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("postcheck-refusal");
    let bundle = render(&directory, ": > shell-ran; printf '%s' 'done'", &[]);
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
    let owner_checks = Cell::new(0);
    let postcheck_refused = Cell::new(false);

    let result = budget.execute(
        &fixture.request(&directory, &bundle),
        &CancellationToken::default(),
        || {
            owner_checks.set(owner_checks.get() + 1);
            if bundle.result_file().is_file() {
                postcheck_refused.set(true);
                Err("original owner changed after query".to_owned())
            } else {
                Ok(Duration::from_secs(30))
            }
        },
    );
    assert!(result.is_err());
    assert!(owner_checks.get() >= 2);
    assert!(postcheck_refused.get());
    assert!(fixture.root.join("shell-ran").is_file());
    assert!(bundle.result_file().is_file());
    assert_eq!(budget.attempted_queries(), 1);
}

#[test]
fn timed_out_query_still_rechecks_owner_after_process_failure() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("timed-out-query");
    let bundle = render(&directory, ": > shell-started; sleep 10", &[]);
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
    let postcheck_after_spawn = Cell::new(false);

    let result = budget.execute(
        &fixture.request(&directory, &bundle),
        &CancellationToken::default(),
        || {
            if fixture.root.join("shell-started").exists() {
                postcheck_after_spawn.set(true);
            }
            Ok(Duration::from_millis(250))
        },
    );
    assert!(result.is_err());
    assert!(fixture.root.join("shell-started").is_file());
    assert!(postcheck_after_spawn.get());
    assert_eq!(budget.attempted_queries(), 1);
}

#[test]
fn preexisting_result_symlink_is_preserved_and_query_is_never_launched() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("linked-result");
    let bundle = render(&directory, ": > shell-ran; printf '%s' 'done'", &[]);
    let target = fixture.root.join("protected-result");
    fs::write(&target, b"preserve this file").expect("write the protected target");
    symlink(&target, bundle.result_file()).expect("precreate a result symlink");
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");

    assert!(budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .is_err());
    assert!(fs::symlink_metadata(bundle.result_file())
        .expect("result symlink remains present")
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"preserve this file");
    assert!(!fixture.root.join("shell-ran").exists());
}

#[test]
fn completed_query_directory_cannot_be_replayed_or_overwritten() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("single-use-directory");
    let bundle = render(&directory, "printf '%s' 'first'", &[]);
    let budget = GnuShellQueryBudget::new(2, 4096).expect("valid query budget");
    let first = budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .expect("first query should complete");
    assert_eq!(first.value, "first");
    let prior_result = fs::read(bundle.result_file()).expect("read the first result");
    let prior_command =
        fs::read(directory.path().join("command.txt")).expect("read the retained command payload");

    assert!(budget
        .execute(
            &fixture.request(&directory, &bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .is_err());
    assert_eq!(fs::read(bundle.result_file()).unwrap(), prior_result);
    assert_eq!(
        fs::read(directory.path().join("command.txt")).unwrap(),
        prior_command
    );
    assert_eq!(budget.attempted_queries(), 2);
}

#[test]
fn result_decoder_accepts_only_framed_canonical_values() {
    let decoded = GnuShellQueryResult::decode(b"literal\n", b"23\n").expect("decode framing");
    assert_eq!(decoded.value, "literal");
    assert_eq!(decoded.status, 23);
    for (value, status) in [
        (&b"missing framing"[..], &b"0\n"[..]),
        (&b"line one\nline two\n"[..], &b"0\n"[..]),
        (&b"value\n"[..], &b"023\n"[..]),
        (&b"value\n"[..], &b"256\n"[..]),
    ] {
        assert!(GnuShellQueryResult::decode(value, status).is_err());
    }
}

#[test]
fn wrong_executable_digest_refuses_before_payload_write_or_launch() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("wrong-executable");
    let bundle = render(&directory, ": > shell-ran", &[]);
    let wrong_digest = sha256_bytes(b"not the selected executable");
    let mut request = fixture.request(&directory, &bundle);
    request.program_sha256 = &wrong_digest;
    let budget = GnuShellQueryBudget::new(1, 4096).unwrap();
    let result = budget.execute(&request, &CancellationToken::default(), || {
        Ok(Duration::from_secs(30))
    });
    assert_eq!(result.unwrap_err(), "GNU query executable changed");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    assert!(!fixture.root.join("shell-ran").exists());
}

#[test]
fn bounded_output_read_accepts_private_regular_data_and_refuses_links_or_writable_files() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("output-read-types");
    let bundle = render(&directory, ":", &[]);
    let request = fixture.request(&directory, &bundle);
    let budget = GnuShellQueryBudget::new(1, 4096).unwrap();
    let result = bundle.result_file();
    fs::write(&result, b"data").unwrap();
    fs::set_permissions(&result, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(budget.read_output(&request, &result, 4).unwrap(), b"data");
    assert_eq!(budget.observed_output_bytes(), 4);
    assert_eq!(
        budget.read_output(&request, &result, 3).unwrap_err(),
        "GNU query output is not bounded and privately owned"
    );
    fs::set_permissions(&result, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        budget.read_output(&request, &result, 4).unwrap_err(),
        "GNU query output is not bounded and privately owned"
    );
    fs::set_permissions(&result, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&result, directory.path().join("second-link")).unwrap();
    assert_eq!(
        budget.read_output(&request, &result, 4).unwrap_err(),
        "GNU query output is not bounded and privately owned"
    );
    let linked = directory.path().join("symbolic-output");
    symlink(&result, &linked).unwrap();
    assert_eq!(
        budget.read_output(&request, &linked, 4).unwrap_err(),
        "GNU query output is not a nonlinked regular file"
    );
    assert_eq!(fs::read(&result).unwrap(), b"data");
}
