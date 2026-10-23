use super::*;
use std::io::Write as _;

fn query_selection<'a>(
    output_root: &'a Path,
    variables: &'a [String],
    goals: &'a [String],
) -> MesaNativeRootQuerySelection<'a> {
    MesaNativeRootQuerySelection {
        output_root,
        variables,
        goals,
    }
}

fn selection_for(fixture: &Fixture) -> (PathBuf, Vec<String>, Vec<String>) {
    (
        fixture.output_root.with_file_name("native-root-query"),
        ["AROS_TARGET_CPU", "TARGET_CC"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        vec!["all".to_owned()],
    )
}

#[test]
fn native_root_query_observes_only_selected_values_and_never_completes_preflight() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (query_root, variables, goals) = selection_for(&fixture);
    let selection = query_selection(&query_root, &variables, &goals);
    let cancellation = CancellationToken::default();

    let observed = observe_mesa_native_root(&request, &selection, &cancellation).unwrap();

    assert_eq!(observed.values()["AROS_TARGET_CPU"], "arm");
    assert_eq!(
        observed.values()["TARGET_CC"],
        request.drivers.clang_path().to_string_lossy().as_ref()
    );
    assert!(observed.values().contains_key("MAKEFILE_LIST"));
    assert_eq!(observed.observed_exit_code(), 2);
    assert_eq!(observed.output_root(), query_root.as_path());
    let inputs = observed.read_inputs();
    assert!(inputs
        .iter()
        .any(|(path, _)| { path == &fixture.output_root.join("configure/Makefile") }));
    assert!(inputs
        .iter()
        .any(|(path, _)| path == &query_root.join("query.mk")));
    assert!(
        !fixture
            .output_root
            .join("configure/poison-target-ran")
            .exists(),
        "the selected all goal must not run the configured Makefile recipe"
    );
    for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
        assert!(
            !fixture.output_root.join(name).exists(),
            "native observation must not publish {name}"
        );
    }
    observed
        .revalidate(&request, &selection, &cancellation)
        .unwrap();
}

#[test]
fn native_root_query_revalidation_rejects_log_mutation_and_reselection() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (query_root, variables, goals) = selection_for(&fixture);
    let selection = query_selection(&query_root, &variables, &goals);
    let cancellation = CancellationToken::default();
    let observed = observe_mesa_native_root(&request, &selection, &cancellation).unwrap();

    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(query_root.join("native-root-query.stdout"))
        .unwrap();
    writeln!(log, "foreign mutation").unwrap();
    assert!(observed
        .revalidate(&request, &selection, &cancellation)
        .is_err());

    let second = Fixture::new();
    let request = second.request();
    let (query_root, variables, goals) = selection_for(&second);
    let selection = query_selection(&query_root, &variables, &goals);
    let observed =
        observe_mesa_native_root(&request, &selection, &CancellationToken::default()).unwrap();
    let changed_goals = ["clean".to_owned()];
    let changed_selection = query_selection(&query_root, &variables, &changed_goals);
    assert!(observed
        .revalidate(&request, &changed_selection, &CancellationToken::default())
        .is_err());

    let changed_root = query_root.with_file_name("reselected-native-root-query");
    let changed_root_selection = query_selection(&changed_root, &variables, &goals);
    assert!(observed
        .revalidate(
            &request,
            &changed_root_selection,
            &CancellationToken::default()
        )
        .is_err());
}

#[test]
fn native_root_query_revalidation_rejects_cancellation() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (query_root, variables, goals) = selection_for(&fixture);
    let selection = query_selection(&query_root, &variables, &goals);
    let cancellation = CancellationToken::default();
    let observed = observe_mesa_native_root(&request, &selection, &cancellation).unwrap();

    cancellation.cancel();
    assert!(observed
        .revalidate(&request, &selection, &cancellation)
        .is_err());
}

#[test]
fn native_root_revalidation_keeps_full_source_and_selection_checks() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let (query_root, variables, goals) = selection_for(&fixture);
    let selection = query_selection(&query_root, &variables, &goals);
    let cancellation = CancellationToken::default();
    let observed = observe_mesa_native_root(&request, &selection, &cancellation).unwrap();
    observed
        .revalidate(&request, &selection, &cancellation)
        .unwrap();

    let foreign_digest = sha256_bytes(b"foreign source selection");
    let mut reselected = fixture.request();
    reselected.source_tree_sha256 = &foreign_digest;
    assert!(observed
        .revalidate(&reselected, &selection, &cancellation)
        .is_err());

    // This file is not in the query's MAKEFILE_LIST. Full held Source, rather
    // than only observed includes, must still detect its introduction.
    fs::write(fixture.source_root.join("unread-source"), b"foreign source").unwrap();
    assert!(observed
        .revalidate(&request, &selection, &cancellation)
        .is_err());
}

#[test]
fn invalid_or_overlapping_native_query_selection_is_rejected_before_preparation() {
    for invalid in ["variable", "goal-assignment", "goal-option"] {
        let fixture = Fixture::new();
        let request = fixture.request();
        let query_root = fixture.output_root.with_file_name("native-root-query");
        let variables = match invalid {
            "variable" => vec!["AROS_TARGET_CPU=arm".to_owned()],
            _ => vec!["AROS_TARGET_CPU".to_owned()],
        };
        let goals = match invalid {
            "goal-assignment" => vec!["TARGET_CC=/foreign/compiler".to_owned()],
            "goal-option" => vec!["-n".to_owned()],
            _ => vec!["all".to_owned()],
        };
        let selection = query_selection(&query_root, &variables, &goals);
        assert!(
            observe_mesa_native_root(&request, &selection, &CancellationToken::default()).is_err(),
            "invalid selection {invalid} must be rejected"
        );
        assert!(
            !fixture.output_root.exists(),
            "invalid selection {invalid} must be rejected before ordinary preparation"
        );
    }

    let occupied = Fixture::new();
    let request = occupied.request();
    let occupied_root = occupied.output_root.with_file_name("occupied-native-query");
    fs::create_dir(&occupied_root).unwrap();
    fs::write(occupied_root.join("keep"), b"existing query output").unwrap();
    let variables = ["AROS_TARGET_CPU".to_owned()];
    let goals = ["all".to_owned()];
    let selection = query_selection(&occupied_root, &variables, &goals);
    assert!(observe_mesa_native_root(&request, &selection, &CancellationToken::default()).is_err());
    assert!(!occupied.output_root.exists());
    assert_eq!(
        fs::read(occupied_root.join("keep")).unwrap(),
        b"existing query output"
    );

    for overlap in ["output", "source", "compiler", "host-tools"] {
        let fixture = Fixture::new();
        let request = fixture.request();
        let overlap_root = match overlap {
            "output" => request.output_root.to_path_buf(),
            "source" => request.source_root.join("native-query"),
            "compiler" => request.drivers.compiler_root().join("native-query"),
            "host-tools" => match request.environment {
                CompatibilityEnvironment::SealedHostTools { host_tools, .. } => {
                    host_tools.root.join("native-query")
                }
                CompatibilityEnvironment::Poisoned { .. } => unreachable!(),
            },
            _ => unreachable!(),
        };
        let variables = ["AROS_TARGET_CPU".to_owned()];
        let goals = ["all".to_owned()];
        let selection = query_selection(&overlap_root, &variables, &goals);
        assert!(
            observe_mesa_native_root(&request, &selection, &CancellationToken::default()).is_err()
        );
        assert!(!fixture.output_root.exists());
    }
}

#[test]
fn foreign_make_error_exit_two_cannot_satisfy_the_selected_query_protocol() {
    let mut fixture = Fixture::new();
    let mut configure = fs::OpenOptions::new()
        .append(true)
        .open(fixture.source_root.join("configure"))
        .unwrap();
    writeln!(
        configure,
        "printf '%s\\n' '$(error foreign-fatal)' >> Makefile"
    )
    .unwrap();
    fixture.source_tree_sha256 = aros_common::measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);

    let request = fixture.request();
    let (query_root, variables, goals) = selection_for(&fixture);
    let selection = query_selection(&query_root, &variables, &goals);
    let result = observe_mesa_native_root(&request, &selection, &CancellationToken::default());

    assert!(
        result.is_err(),
        "an unrelated Make parse error with status 2 must not count as a successful query"
    );
    assert!(
        fixture.output_root.exists(),
        "ordinary four-stage preparation should complete"
    );
    let stderr = fs::read_to_string(query_root.join("native-root-query.stderr")).unwrap();
    assert!(
        stderr.contains("foreign-fatal"),
        "unexpected query stderr: {stderr}"
    );
    for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
        assert!(!fixture.output_root.join(name).exists());
    }
}
