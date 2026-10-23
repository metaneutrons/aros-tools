use super::*;

fn shell_context(fixture: &Fixture, command: &str) -> invocation::InvocationContext {
    let request = fixture.request();
    let mut resolved = environment::resolve(request.environment).unwrap();
    resolved.variables.insert("LC_ALL".into(), "C".into());
    invocation::InvocationContext::new(
        &Path::new("/bin/sh").canonicalize().unwrap(),
        &["-c".into(), command.into()],
        &fixture.source_root,
        resolved,
    )
    .unwrap()
}

#[test]
fn expected_nonzero_exit_returns_stage_digests_and_private_invocation_context() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let control = (&request, &cancellation, Instant::now());
    let command = "printf 'expected stdout\\n'; printf 'expected stderr\\n' >&2; exit 2";

    let (stage, observed_exit) = run_prepared_outcome(
        control,
        "expected-nonzero",
        shell_context(&fixture, command),
        2,
    )
    .unwrap();

    assert_eq!(observed_exit, 2);
    assert_eq!(stage.stdout, sha256_bytes(b"expected stdout\n"));
    assert_eq!(stage.stderr, sha256_bytes(b"expected stderr\n"));
    assert_eq!(
        fs::read(fixture.output_root.join("expected-nonzero.stdout")).unwrap(),
        b"expected stdout\n"
    );
    assert_eq!(
        fs::read(fixture.output_root.join("expected-nonzero.stderr")).unwrap(),
        b"expected stderr\n"
    );
    assert_eq!(stage.name, "expected-nonzero");
    let shell = Path::new("/bin/sh").canonicalize().unwrap();
    assert_eq!(stage.invocation_context.program(), shell.as_path());
    let arguments = stage.invocation_context.arguments();
    assert_eq!(arguments.len(), 2);
    assert_eq!(arguments[0], "-c");
    assert_eq!(arguments[1], command);
    assert_eq!(
        stage.invocation_context.working_directory(),
        fixture.source_root.as_path()
    );
    assert_eq!(
        stage
            .invocation_context
            .environment()
            .variables
            .get("LC_ALL")
            .map(String::as_str),
        Some("C")
    );

    let normal = run_prepared(
        control,
        "expected-nonzero-normal",
        shell_context(&fixture, command),
    );
    assert!(
        normal.is_err(),
        "the normal runner must still require exit 0"
    );
    assert_eq!(
        fs::read(fixture.output_root.join("expected-nonzero-normal.stdout")).unwrap(),
        b"expected stdout\n"
    );
    assert_eq!(
        fs::read(fixture.output_root.join("expected-nonzero-normal.stderr")).unwrap(),
        b"expected stderr\n"
    );
    let (_, observed_exit) = run_prepared_outcome(
        control,
        "expected-maximum-exit",
        shell_context(&fixture, "exit 255"),
        255,
    )
    .unwrap();
    assert_eq!(observed_exit, 255);
}

#[test]
fn matching_nonzero_exit_cannot_admit_source_drift_during_the_child() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let marker = fixture.source_root.join("outcome-source-drift");
    assert!(!marker.exists());
    let result = run_prepared_outcome(
        (&request, &cancellation, Instant::now()),
        "source-drift-expected-nonzero",
        shell_context(
            &fixture,
            "printf changed > outcome-source-drift; printf 'child ran'; exit 2",
        ),
        2,
    );
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Mesa preflight source changed"));
    assert_eq!(fs::read(marker).unwrap(), b"changed");
    assert_eq!(
        fs::read(
            fixture
                .output_root
                .join("source-drift-expected-nonzero.stdout")
        )
        .unwrap(),
        b"child ran",
    );
}

#[test]
fn expected_nonzero_exit_rejects_zero_other_codes_and_signal_termination() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let control = (&request, &cancellation, Instant::now());

    for (stage, command) in [
        ("expected-two-got-zero", "printf 'actual zero\\n'; exit 0"),
        ("expected-two-got-three", "printf 'actual three\\n'; exit 3"),
        (
            "expected-two-got-signal",
            "printf 'actual signal\\n'; kill -TERM $$",
        ),
    ] {
        assert!(
            run_prepared_outcome(control, stage, shell_context(&fixture, command), 2).is_err(),
            "{stage} must reject a child whose outcome differs from exit 2"
        );
        assert!(fixture
            .output_root
            .join(format!("{stage}.stdout"))
            .is_file());
        assert!(fixture
            .output_root
            .join(format!("{stage}.stderr"))
            .is_file());
    }
}

#[test]
fn invalid_expected_exit_codes_are_rejected_before_child_or_log_creation() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let control = (&request, &cancellation, Instant::now());

    for (stage, expected, command, marker) in [
        (
            "expected-negative",
            -1,
            "printf 'ran' > ../negative-exit-child-ran; exit 2",
            "negative-exit-child-ran",
        ),
        (
            "expected-too-large",
            256,
            "printf 'ran' > ../large-exit-child-ran; exit 2",
            "large-exit-child-ran",
        ),
    ] {
        assert!(
            run_prepared_outcome(control, stage, shell_context(&fixture, command), expected)
                .is_err(),
            "{stage} must reject an expected exit outside 0..=255"
        );
        assert!(!fixture.source_root.parent().unwrap().join(marker).exists());
        assert!(!fixture.output_root.join(format!("{stage}.stdout")).exists());
        assert!(!fixture.output_root.join(format!("{stage}.stderr")).exists());
    }
}

#[test]
fn expected_nonzero_exit_does_not_admit_a_live_timed_out_child() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let mut request = fixture.request();
    request.timeout = Duration::from_secs(1);
    let cancellation = CancellationToken::default();
    let context = shell_context(
        &fixture,
        "trap 'exit 2' TERM; printf 'child started\\n'; while :; do :; done",
    );
    assert!(run_prepared_outcome(
        (&request, &cancellation, Instant::now()),
        "live-timeout-expected-nonzero",
        context,
        2,
    )
    .is_err());
    assert_eq!(
        fs::read(
            fixture
                .output_root
                .join("live-timeout-expected-nonzero.stdout")
        )
        .unwrap(),
        b"child started\n",
        "the child must actually run, not merely encounter an expired pre-launch clock",
    );
}

#[test]
fn expected_nonzero_exit_does_not_bypass_cancellation_or_original_deadline() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();

    let cancellation = CancellationToken::default();
    cancellation.cancel();
    assert!(run_prepared_outcome(
        (&request, &cancellation, Instant::now()),
        "cancelled-expected-nonzero",
        shell_context(&fixture, "printf 'ran' > ../cancelled-child-ran; exit 2"),
        2,
    )
    .is_err());
    assert!(!fixture
        .source_root
        .parent()
        .unwrap()
        .join("cancelled-child-ran")
        .exists());
    assert!(!fixture
        .output_root
        .join("cancelled-expected-nonzero.stdout")
        .exists());
    assert!(!fixture
        .output_root
        .join("cancelled-expected-nonzero.stderr")
        .exists());

    let cancellation = CancellationToken::default();
    let expired_started = Instant::now().checked_sub(request.timeout).unwrap();
    assert!(run_prepared_outcome(
        (&request, &cancellation, expired_started),
        "expired-expected-nonzero",
        shell_context(&fixture, "printf 'ran' > ../expired-child-ran; exit 2"),
        2,
    )
    .is_err());
    assert!(!fixture
        .source_root
        .parent()
        .unwrap()
        .join("expired-child-ran")
        .exists());
    assert!(!fixture
        .output_root
        .join("expired-expected-nonzero.stdout")
        .exists());
    assert!(!fixture
        .output_root
        .join("expired-expected-nonzero.stderr")
        .exists());
}
