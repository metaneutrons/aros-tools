use super::*;

fn launch_variables() -> Vec<String> {
    let mut variables: Vec<String> = HOST_QUERY_VARIABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    variables.push("MAKECMDGOALS".to_owned());
    variables.push("LAUNCH_CONTEXT".to_owned());
    variables
}

fn configure_launch_context(fixture: &mut Fixture, extra_lines: &[&str]) {
    let mut recipe = success_recipe();
    recipe.extend(
        [
            "ifeq ($(MAKECMDGOALS),launch)",
            "LAUNCH_CONTEXT := launch-goal",
            "else",
            "LAUNCH_CONTEXT := mmake-goal",
            "endif",
            ".PHONY: launch",
            "launch:",
            "\t@printf '%s\\n' 'launch goal ran' > \"$(TOP)/launch-goal-ran\"",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    recipe.extend(extra_lines.iter().map(|line| (*line).to_owned()));
    let host_cc = sealed_host_cc(fixture);
    configure_host_goal(fixture, &host_cc, "make", "", &recipe);
}

fn launch_selection<'a>(
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

#[test]
fn native_launch_query_observes_new_goal_context_without_changing_host_output() {
    let mut fixture = Fixture::new();
    configure_launch_context(&mut fixture, &[]);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let host_goals = ["mmake".to_owned()];
    let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
    let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
    let host_build = host_selection(&outputs, "mmake");
    let built =
        build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation).unwrap();

    let launch_root = fixture.output_root.with_file_name("native-launch-query");
    let variables = launch_variables();
    let goals = ["launch".to_owned()];
    let query = launch_selection(&launch_root, &variables, &goals);
    let observed = observe_mesa_native_launch_inputs(
        built,
        &request,
        &host_query,
        &host_build,
        &query,
        &cancellation,
    )
    .unwrap();

    assert_eq!(host_query.goals, host_goals.as_slice());
    assert_eq!(observed.values()["MAKECMDGOALS"], "launch");
    assert_eq!(observed.values()["LAUNCH_CONTEXT"], "launch-goal");
    assert_ne!(observed.values()["LAUNCH_CONTEXT"], "mmake-goal");
    assert!(observed
        .read_inputs()
        .iter()
        .any(|(path, _)| path == &fixture.output_root.join("configure/Makefile")));
    assert!(observed
        .read_inputs()
        .iter()
        .any(|(path, _)| path == &launch_root.join("query.mk")));
    assert_eq!(
        fs::read(&outputs.executable).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    assert_eq!(fs::read(&outputs.manifest).unwrap(), b"host namespace\n");
    assert!(!fixture
        .output_root
        .join("configure/launch-goal-ran")
        .exists());
    observed
        .revalidate(&request, &host_query, &host_build, &query, &cancellation)
        .unwrap();
    assert_no_completed_preflight(&fixture);
}

#[test]
fn launch_query_rejects_changed_host_before_creating_launch_evidence() {
    let mut fixture = Fixture::new();
    configure_launch_context(&mut fixture, &[]);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let host_goals = ["mmake".to_owned()];
    let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
    let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
    let host_build = host_selection(&outputs, "mmake");
    let built =
        build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation).unwrap();
    fs::write(built.executable(), b"changed host executable\n").unwrap();

    let launch_root = fixture.output_root.with_file_name("native-launch-query");
    let variables = launch_variables();
    let goals = ["launch".to_owned()];
    let query = launch_selection(&launch_root, &variables, &goals);
    let result = observe_mesa_native_launch_inputs(
        built,
        &request,
        &host_query,
        &host_build,
        &query,
        &cancellation,
    );
    assert_preflight_error(result, "native host build output transition changed");
    assert!(!launch_root.exists());
    assert_no_completed_preflight(&fixture);
}

#[test]
fn launch_query_rejects_enabled_compiler_construction_after_query_stop() {
    let mut fixture = Fixture::new();
    configure_launch_context(
        &mut fixture,
        &[
            "ifeq ($(MAKECMDGOALS),launch)",
            "CROSSTOOLS_TARGET := arm-unknown-aros",
            "endif",
        ],
    );

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let host_goals = ["mmake".to_owned()];
    let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
    let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
    let host_build = host_selection(&outputs, "mmake");
    let built =
        build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation).unwrap();

    let launch_root = fixture.output_root.with_file_name("native-launch-query");
    let variables = launch_variables();
    let goals = ["launch".to_owned()];
    let query = launch_selection(&launch_root, &variables, &goals);
    let result = observe_mesa_native_launch_inputs(
        built,
        &request,
        &host_query,
        &host_build,
        &query,
        &cancellation,
    );
    assert_preflight_error(
        result,
        "native host build requires compiler construction disabled",
    );

    let stdout = fs::read_to_string(launch_root.join("native-root-query.stdout")).unwrap();
    let stderr = fs::read_to_string(launch_root.join("native-root-query.stderr")).unwrap();
    assert!(stdout.contains("CROSSTOOLS_TARGET=arm-unknown-aros"));
    assert!(stderr.contains("AROS_NATIVE_ROOT_QUERY_STOP_"));
    assert!(stderr.contains("Stop."));
    assert!(!fixture
        .output_root
        .join("configure/launch-goal-ran")
        .exists());
    assert_no_completed_preflight(&fixture);
}

#[test]
fn launch_goal_cannot_reselect_host_executable_or_sealed_roles() {
    for (assignment, expected) in [
        (
            "MMAKE := /foreign/mmake",
            "native launch goal selected a different host executable",
        ),
        (
            "HOST_CC := cc",
            "native host compiler differs from sealed role",
        ),
        (
            "HOST_MAKE := gmake",
            "native recursive Make differs from sealed driver",
        ),
    ] {
        let mut fixture = Fixture::new();
        configure_launch_context(
            &mut fixture,
            &["ifeq ($(MAKECMDGOALS),launch)", assignment, "endif"],
        );
        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let outputs = host_outputs(&fixture);
        let host_query_root = fixture.output_root.with_file_name("native-root-query");
        let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
        let host_goals = ["mmake".to_owned()];
        let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
        let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
        let host_build = host_selection(&outputs, "mmake");
        let built = build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation)
            .unwrap();
        let launch_root = fixture.output_root.with_file_name("native-launch-query");
        let variables = launch_variables();
        let goals = ["launch".to_owned()];
        let query = launch_selection(&launch_root, &variables, &goals);
        assert_preflight_error(
            observe_mesa_native_launch_inputs(
                built,
                &request,
                &host_query,
                &host_build,
                &query,
                &cancellation,
            ),
            expected,
        );
        assert!(!fixture
            .output_root
            .join("configure/launch-goal-ran")
            .exists());
        assert_no_completed_preflight(&fixture);
    }
}

#[test]
fn launch_result_revalidation_rejects_foreign_goal_and_changed_query_evidence() {
    let mut fixture = Fixture::new();
    configure_launch_context(&mut fixture, &[]);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let host_goals = ["mmake".to_owned()];
    let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
    let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
    let host_build = host_selection(&outputs, "mmake");
    let built =
        build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation).unwrap();

    let launch_root = fixture.output_root.with_file_name("native-launch-query");
    let variables = launch_variables();
    let goals = ["launch".to_owned()];
    let query = launch_selection(&launch_root, &variables, &goals);
    let observed = observe_mesa_native_launch_inputs(
        built,
        &request,
        &host_query,
        &host_build,
        &query,
        &cancellation,
    )
    .unwrap();
    observed
        .revalidate(&request, &host_query, &host_build, &query, &cancellation)
        .unwrap();

    let foreign_goals = ["foreign".to_owned()];
    let foreign_query = launch_selection(&launch_root, &variables, &foreign_goals);
    assert_preflight_error(
        observed.revalidate(
            &request,
            &host_query,
            &host_build,
            &foreign_query,
            &cancellation,
        ),
        "native Root query selected different inputs",
    );

    let mut stdout = fs::OpenOptions::new()
        .append(true)
        .open(launch_root.join("native-root-query.stdout"))
        .unwrap();
    writeln!(stdout, "foreign mutation").unwrap();
    assert_preflight_error(
        observed.revalidate(&request, &host_query, &host_build, &query, &cancellation),
        "native Root query evidence changed",
    );
    assert_no_completed_preflight(&fixture);
}

#[test]
fn launch_query_cancellation_stops_before_creating_launch_evidence() {
    let mut fixture = Fixture::new();
    configure_launch_context(&mut fixture, &[]);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let host_variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let host_goals = ["mmake".to_owned()];
    let host_query = launch_selection(&host_query_root, &host_variables, &host_goals);
    let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
    let host_build = host_selection(&outputs, "mmake");
    let built =
        build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation).unwrap();
    cancellation.cancel();

    let launch_root = fixture.output_root.with_file_name("native-launch-query");
    let variables = launch_variables();
    let goals = ["launch".to_owned()];
    let query = launch_selection(&launch_root, &variables, &goals);
    let result = observe_mesa_native_launch_inputs(
        built,
        &request,
        &host_query,
        &host_build,
        &query,
        &cancellation,
    );
    assert_preflight_error(result, "invalid or cancelled Mesa preflight selection");
    assert!(!launch_root.exists());
    assert_no_completed_preflight(&fixture);
}
