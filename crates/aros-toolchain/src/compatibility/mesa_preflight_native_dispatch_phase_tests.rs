use super::*;

#[path = "mesa_preflight_native_dispatch_recorder_tests.rs"]
mod recorder_tests;

struct DispatchInputs {
    outputs: HostOutputs,
    host_query_root: PathBuf,
    host_variables: Vec<String>,
    host_goals: Vec<String>,
    launch_query_root: PathBuf,
    launch_variables: Vec<String>,
    launch_goals: Vec<String>,
    dispatch_root: PathBuf,
    observer: PathBuf,
    observer_sha256: Sha256Digest,
    host_genmodule: Option<String>,
}

impl DispatchInputs {
    fn host_query(&self) -> MesaNativeRootQuerySelection<'_> {
        MesaNativeRootQuerySelection {
            output_root: &self.host_query_root,
            variables: &self.host_variables,
            goals: &self.host_goals,
        }
    }

    fn host_build(&self) -> MesaNativeHostBuildSelection<'_> {
        let mut selection = host_selection(&self.outputs, "mmake");
        selection.additional_goals = &self.host_goals[1..];
        selection
    }

    fn launch_query(&self) -> MesaNativeRootQuerySelection<'_> {
        MesaNativeRootQuerySelection {
            output_root: &self.launch_query_root,
            variables: &self.launch_variables,
            goals: &self.launch_goals,
        }
    }

    fn dispatch_selection(&self) -> MesaNativeDispatchSelection<'_> {
        MesaNativeDispatchSelection {
            output_root: &self.dispatch_root,
            observer: &self.observer,
            observer_sha256: &self.observer_sha256,
            recipe_shell: Path::new("/bin/sh"),
            pattern: "%-quick",
            project: "Fixture",
            prefix_variable: "CALL",
            program_variable: "MMAKE",
            options_variable: "MMAKE_OPTIONS",
        }
    }
}

fn observed_launch(
    fixture: &mut Fixture,
    goal: &str,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_startup_hook(fixture, goal, None)
}

fn observed_launch_with_startup_hook(
    fixture: &mut Fixture,
    goal: &str,
    hook: Option<&str>,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_export(
        fixture,
        goal,
        hook.map(|name| (name, "/nonexistent/unmodeled-startup-hook")),
    )
}

fn observed_launch_with_export(
    fixture: &mut Fixture,
    goal: &str,
    export: Option<(&str, &str)>,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_program(fixture, goal, export, None)
}

fn observed_launch_with_program(
    fixture: &mut Fixture,
    goal: &str,
    export: Option<(&str, &str)>,
    program: Option<&str>,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_program_and_genmodule(fixture, goal, export, program, None, true, false)
}

fn observed_launch_with_genmodule(
    fixture: &mut Fixture,
    goal: &str,
    program: &str,
    genmodule_script: &str,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_program_and_genmodule(
        fixture,
        goal,
        None,
        Some(program),
        Some(genmodule_script),
        true,
        false,
    )
}

fn observed_launch_with_unqueried_genmodule(
    fixture: &mut Fixture,
    goal: &str,
    program: &str,
    genmodule_script: &str,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_program_and_genmodule(
        fixture,
        goal,
        None,
        Some(program),
        Some(genmodule_script),
        false,
        false,
    )
}

fn observed_launch_with_native_genmodule(
    fixture: &mut Fixture,
    goal: &str,
    program: &str,
    genmodule_source: &str,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    observed_launch_with_program_and_genmodule(
        fixture,
        goal,
        None,
        Some(program),
        Some(genmodule_source),
        true,
        true,
    )
}

fn observed_launch_with_program_and_genmodule(
    fixture: &mut Fixture,
    goal: &str,
    export: Option<(&str, &str)>,
    program: Option<&str>,
    genmodule_script: Option<&str>,
    query_genmodule: bool,
    compile_genmodule: bool,
) -> (ObservedMesaNativeLaunchInputs, DispatchInputs) {
    let mut recipe = success_recipe();
    recipe[1] = "\t@printf '#!/bin/sh\\n: > \"$(TOP)/native-dispatch-root-recipe-ran\"\\nexit 0\\n' > \"$(MMAKE)\"".into();
    if let Some(program) = program {
        let lines = program
            .lines()
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
            .replace('$', "$$");
        recipe[1] = format!("\t@printf '%s\\n' {lines} > \"$(MMAKE)\"");
    }
    if let Some(genmodule_script) = genmodule_script {
        let filename = if compile_genmodule {
            "genmodule.c"
        } else {
            "genmodule"
        };
        recipe.push(format!(
            "\t@: > \"$(TOP)/tools/MetaMake/generated/{filename}\""
        ));
        for line in genmodule_script.lines() {
            let line = shell_quote(line).replace('$', "$$");
            recipe.push(format!(
                "\t@printf '%s\\n' {line} >> \"$(TOP)/tools/MetaMake/generated/{filename}\""
            ));
        }
        if compile_genmodule {
            recipe.push("\t@$(HOST_CC) -x c \"$(TOP)/tools/MetaMake/generated/genmodule.c\" -o \"$(TOP)/tools/MetaMake/generated/genmodule\"".into());
        } else {
            recipe.push("\t@chmod 755 \"$(TOP)/tools/MetaMake/generated/genmodule\"".into());
        }
        // Keep the role in the original Root query while creating its bytes
        // inside the already selected fresh MetaMake namespace.
        recipe.push("GENMODULE := $(TOP)/tools/MetaMake/generated/genmodule".into());
    }
    let call_prefix = if genmodule_script.is_some() {
        "CALL := env DISPATCH_SENTINEL=held SOURCE_UNEXPORT_SENTINEL=present"
    } else {
        "CALL := env DISPATCH_SENTINEL=held"
    };
    recipe.extend(
        [
            call_prefix,
            "MMAKE_OPTIONS := --fixture",
            "export ROOT_INPUTS = $(MAKEFILE_LIST)",
            "%-quick:",
            "\t@+$(CALL) $(MMAKE) $(MMAKE_OPTIONS) Fixture.$@",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    if program.is_some() {
        recipe.push("MMAKE_OPTIONS := --srcdir=$(SRCDIR) --builddir=$(TOP)".into());
    }
    if let Some((name, value)) = export {
        recipe.extend([
            "ifeq ($(MAKECMDGOALS),build-quick)".into(),
            format!("export {name} = {value}"),
            "endif".into(),
        ]);
    }
    let host_cc = sealed_host_cc(fixture);
    configure_host_goal(fixture, &host_cc, "make", "", &recipe);

    let outputs = host_outputs(fixture);
    let host_query_root = fixture.output_root.with_file_name("native-root-query");
    let mut host_variables = HOST_QUERY_VARIABLES.map(str::to_owned).to_vec();
    if query_genmodule && genmodule_script.is_some() {
        host_variables.push("GENMODULE".into());
    }
    let mut host_goals = vec!["mmake".to_owned()];
    if compile_genmodule {
        host_goals.push(
            fixture
                .output_root
                .join("configure/tools/MetaMake/generated/genmodule")
                .to_str()
                .unwrap()
                .to_owned(),
        );
    }
    let launch_query_root = fixture.output_root.with_file_name("native-launch-query");
    let mut launch_variables = HOST_QUERY_VARIABLES.map(str::to_owned).to_vec();
    launch_variables.extend(
        ["MAKECMDGOALS", "CALL", "MMAKE_OPTIONS"]
            .into_iter()
            .map(str::to_owned),
    );
    if query_genmodule && genmodule_script.is_some() {
        launch_variables.push("GENMODULE".into());
    }
    let launch_goals = vec![goal.to_owned()];
    let dispatch_root = fixture
        .output_root
        .with_file_name("native-dispatch-observation");
    let observer = std::env::current_exe().unwrap();
    let observer_sha256 = sha256_bytes(&fs::read(&observer).unwrap());
    let mut inputs = DispatchInputs {
        outputs,
        host_query_root,
        host_variables,
        host_goals,
        launch_query_root,
        launch_variables,
        launch_goals,
        dispatch_root,
        observer,
        observer_sha256,
        host_genmodule: None,
    };

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let mut queried_genmodule = None;
    let launch = {
        let host_query = inputs.host_query();
        let root = observe_mesa_native_root(&request, &host_query, &cancellation).unwrap();
        if query_genmodule && genmodule_script.is_some() {
            queried_genmodule = Some(
                root.values()
                    .get("GENMODULE")
                    .expect("GENMODULE was independently included in the original host query")
                    .clone(),
            );
        }
        let host_build = inputs.host_build();
        let built = build_mesa_native_host(root, &request, &host_query, &host_build, &cancellation)
            .unwrap();
        let launch_query = inputs.launch_query();
        observe_mesa_native_launch_inputs(
            built,
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &cancellation,
        )
        .unwrap()
    };
    inputs.host_genmodule = queried_genmodule;
    (launch, inputs)
}

#[test]
fn native_dispatch_refuses_original_only_hook_hidden_by_recorder_roles() {
    let mut fixture = Fixture::new();
    let (launch, inputs) = observed_launch_with_export(
        &mut fixture,
        "build-quick",
        Some((
            "BASH_ENV",
            "$(if $(findstring --capture-make-invocation,$(CALL)),,/nonexistent/original-only-hook)",
        )),
    );
    let result = observe_mesa_native_dispatch(
        launch,
        &fixture.request(),
        &inputs.host_query(),
        &inputs.host_build(),
        &inputs.launch_query(),
        &inputs.dispatch_selection(),
        &CancellationToken::default(),
    );
    assert!(result.is_err());
    let refusal = fs::read_to_string(inputs.dispatch_root.join("source-refusal.txt")).unwrap();
    assert!(refusal.contains("original recipe has an unmodeled startup hook"));
    assert!(!refusal.contains("/nonexistent/original-only-hook"));
    assert!(!inputs.dispatch_root.join("pending-command.json").exists());
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
}

#[test]
fn native_dispatch_refuses_source_shell_startup_hooks_before_a_root_recipe() {
    for hook in ["ENV", "BASH_ENV"] {
        let mut fixture = Fixture::new();
        let (launch, inputs) =
            observed_launch_with_startup_hook(&mut fixture, "build-quick", Some(hook));
        let result = observe_mesa_native_dispatch(
            launch,
            &fixture.request(),
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &CancellationToken::default(),
        );
        assert!(result.is_err());
        let refusal = fs::read_to_string(inputs.dispatch_root.join("source-refusal.txt")).unwrap();
        assert!(refusal.contains("startup hook"));
        assert!(!refusal.contains("/nonexistent/unmodeled-startup-hook"));
        assert!(!inputs.dispatch_root.join("pending-command.json").exists());
        assert!(!fixture
            .output_root
            .join("configure/native-dispatch-root-recipe-ran")
            .exists());
    }
}

#[test]
fn fenced_dispatch_observes_full_argv_replays_input_list_and_keeps_original_owner() {
    let mut fixture = Fixture::new();
    let (launch, inputs) = observed_launch(&mut fixture, "build-quick");
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let candidate = observe_mesa_native_dispatch(
        launch,
        &request,
        &inputs.host_query(),
        &inputs.host_build(),
        &inputs.launch_query(),
        &inputs.dispatch_selection(),
        &cancellation,
    )
    .unwrap();
    let candidate_context = candidate
        .initial_observation()
        .binding()
        .context_sha256
        .clone();
    let original = candidate.source_plan().original_recipe().unwrap();
    let root = fixture.output_root.join("configure/Makefile");
    let original_inputs = format!(
        "{} {}",
        root.display(),
        fixture
            .output_root
            .join("configure/config/make.cfg")
            .display()
    );
    assert_eq!(original.source_exports()["ROOT_INPUTS"], original_inputs);
    assert!(candidate.source_plan().original_recipe_reads().is_some());
    let fenced = candidate
        .observe_fenced(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation,
        )
        .unwrap();
    let full = fenced.initial_observation();
    assert_ne!(full.binding().context_sha256, candidate_context);
    assert_eq!(
        full.invocation().arguments(),
        fenced.source_plan().source().startup().arguments()
    );
    let root = fixture.output_root.join("configure/Makefile");
    let override_file = inputs.dispatch_root.join("capture-override.mk");
    let fence = inputs.dispatch_root.join("default-fence.mk");
    assert_eq!(
        fenced.source_plan().source().startup().input_files(),
        [
            root.to_str().unwrap(),
            override_file.to_str().unwrap(),
            fence.to_str().unwrap(),
        ]
    );
    assert_eq!(
        fenced.source_plan().projection().source_exports()["ROOT_INPUTS"],
        format!(
            "{} {} {} {}",
            root.display(),
            fixture
                .output_root
                .join("configure/config/make.cfg")
                .display(),
            override_file.display(),
            fence.display()
        )
    );
    assert!(!inputs.dispatch_root.join("pending-command.json").exists());
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
    fenced
        .revalidate(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation,
        )
        .unwrap();
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(fenced
        .revalidate(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancelled
        )
        .is_err());
    fs::write(fence, b"changed\n").unwrap();
    assert!(fenced
        .revalidate(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation
        )
        .is_err());
}

#[test]
fn fenced_dispatch_refuses_nonfresh_evidence_before_sampling() {
    let mut fixture = Fixture::new();
    let (launch, inputs) = observed_launch(&mut fixture, "build-quick");
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let candidate = observe_mesa_native_dispatch(
        launch,
        &request,
        &inputs.host_query(),
        &inputs.host_build(),
        &inputs.launch_query(),
        &inputs.dispatch_selection(),
        &cancellation,
    )
    .unwrap();
    let fence = inputs.dispatch_root.join("default-fence.mk");
    fs::write(&fence, b"preexisting\n").unwrap();
    assert!(candidate
        .observe_fenced(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation
        )
        .is_err());
    assert_eq!(fs::read(fence).unwrap(), b"preexisting\n");
    assert!(!inputs.dispatch_root.join("fenced.context.json").exists());
}

#[test]
fn fenced_dispatch_refuses_source_shell_options_before_recorder_execution() {
    let mut fixture = Fixture::new();
    let (launch, inputs) =
        observed_launch_with_startup_hook(&mut fixture, "build-quick", Some("SHELLOPTS"));
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let candidate = observe_mesa_native_dispatch(
        launch,
        &request,
        &inputs.host_query(),
        &inputs.host_build(),
        &inputs.launch_query(),
        &inputs.dispatch_selection(),
        &cancellation,
    )
    .unwrap();
    let error = candidate
        .observe_fenced(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation,
        )
        .err()
        .unwrap();
    assert!(error
        .to_string()
        .contains("option, function or loader controls"));
    assert!(!error
        .to_string()
        .contains("/nonexistent/unmodeled-startup-hook"));
    assert!(!inputs.dispatch_root.join("pending-command.json").exists());
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
}

#[test]
fn native_dispatch_observes_first_input_context_and_checks_source_without_running_root_recipe() {
    let mut fixture = Fixture::new();
    let (launch, inputs) = observed_launch(&mut fixture, "build-quick");
    assert_eq!(launch.values()["MAKECMDGOALS"], "build-quick");
    assert_eq!(launch.values()["CALL"], "env DISPATCH_SENTINEL=held");
    assert_eq!(launch.values()["MMAKE_OPTIONS"], "--fixture");
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let host_query = inputs.host_query();
    let host_build = inputs.host_build();
    let launch_query = inputs.launch_query();
    let selection = inputs.dispatch_selection();

    let observed = observe_mesa_native_dispatch(
        launch,
        &request,
        &host_query,
        &host_build,
        &launch_query,
        &selection,
        &cancellation,
    )
    .unwrap();

    let initial = observed.initial_observation();
    let recorder = observed
        .recorder_projection()
        .parse_literal_shell()
        .unwrap();
    let [aros_transpiler::make_shell_data::LiteralRecipeStatement::Command(command)] =
        recorder.statements.as_slice()
    else {
        panic!("expected one literal recorder command")
    };
    assert_eq!(command.words()[0], inputs.observer.to_str().unwrap());
    let namespace = aros_common::make_namespace::ValidatedMakeNamespace::from_make_stdout(
        initial.initial_stdout(),
    )
    .unwrap();
    let recipe_controls =
        aros_common::make_initial_values::ValidatedMakeRecipeControls::from_make_stdout(
            initial.initial_stdout(),
            &namespace,
        )
        .unwrap();
    let final_controls = observed.source_plan().recipe_controls().unwrap();
    assert!(!final_controls.shell().is_empty());
    assert_eq!(
        final_controls.flags(),
        recipe_controls.entries().next().unwrap().value
    );
    assert_eq!(
        initial.binding().phase,
        aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
    );
    let root_makefile = fixture.output_root.join("configure/Makefile");
    let expected_arguments = vec![
        "-s".to_owned(),
        "-r".to_owned(),
        "--no-print-directory".to_owned(),
        "-f".to_owned(),
        root_makefile.to_str().unwrap().to_owned(),
        "build-quick".to_owned(),
    ];
    assert_eq!(
        initial.invocation().arguments(),
        expected_arguments.as_slice()
    );
    let expected_working_directory = fixture.output_root.join("configure");
    assert_eq!(
        initial.invocation().working_directory(),
        expected_working_directory.as_path()
    );
    assert_eq!(
        initial.binding().invocation_sha256,
        sha256_bytes(&initial.invocation().to_bytes().unwrap())
    );
    assert_eq!(
        initial.binding().stdout_sha256,
        sha256_bytes(initial.initial_stdout())
    );

    let mut expected_environment = crate::compatibility::environment::resolve(request.environment)
        .unwrap()
        .variables;
    // The original configured-values stage adds this deterministic locale in
    // mesa_preflight::run; dispatch inherits that held context, not just request.
    expected_environment.insert("LC_ALL".into(), "C".into());
    assert_eq!(initial.invocation().environment(), &expected_environment);
    assert!(initial
        .invocation()
        .environment()
        .keys()
        .all(|name| initial.environment_names().contains(name)));

    assert!(observed.source_plan().revalidate().is_ok());
    assert!(!observed
        .source_plan()
        .capture_plan()
        .override_bytes()
        .is_empty());
    assert!(
        !inputs.dispatch_root.join("pending-command.json").exists(),
        "dispatch preflight must not create a pending command capture"
    );
    assert!(
        !fixture
            .output_root
            .join("configure/poison-target-ran")
            .exists(),
        "the first-input observer must stop before the Root recipe"
    );
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
    assert_no_completed_preflight(&fixture);

    let changed_pattern = MesaNativeDispatchSelection {
        output_root: selection.output_root,
        observer: selection.observer,
        observer_sha256: selection.observer_sha256,
        recipe_shell: selection.recipe_shell,
        pattern: "%-other-quick",
        project: selection.project,
        prefix_variable: selection.prefix_variable,
        program_variable: selection.program_variable,
        options_variable: selection.options_variable,
    };
    assert!(observed
        .revalidate(
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &changed_pattern,
            &cancellation,
        )
        .is_err());

    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(observed
        .revalidate(
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &selection,
            &cancelled,
        )
        .is_err());

    fs::write(
        inputs.dispatch_root.join("foreign-mutation"),
        b"changed evidence",
    )
    .unwrap();
    assert!(observed
        .revalidate(
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &selection,
            &cancellation,
        )
        .is_err());
}

#[test]
fn native_dispatch_refuses_an_ordinary_goal_before_creating_evidence() {
    let mut fixture = Fixture::new();
    let (launch, inputs) = observed_launch(&mut fixture, "all");
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let host_query = inputs.host_query();
    let host_build = inputs.host_build();
    let launch_query = inputs.launch_query();
    let selection = inputs.dispatch_selection();

    assert_preflight_error(
        observe_mesa_native_dispatch(
            launch,
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &selection,
            &cancellation,
        ),
        "native dispatch requires a Source quick goal",
    );
    assert!(
        !inputs.dispatch_root.exists(),
        "an ordinary goal must be refused before dispatch evidence is created"
    );
    assert!(!fixture
        .output_root
        .join("configure/poison-target-ran")
        .exists());
    assert_no_completed_preflight(&fixture);
}

#[test]
fn native_dispatch_rejects_changed_source_or_host_before_creating_evidence() {
    for changed_owner in ["source", "host"] {
        let mut fixture = Fixture::new();
        let (launch, inputs) = observed_launch(&mut fixture, "build-quick");
        if changed_owner == "source" {
            fs::write(
                fixture.source_root.join("foreign-source"),
                b"changed source",
            )
            .unwrap();
        } else {
            fs::write(&inputs.outputs.executable, b"changed host executable").unwrap();
        }

        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let host_query = inputs.host_query();
        let host_build = inputs.host_build();
        let launch_query = inputs.launch_query();
        let selection = inputs.dispatch_selection();

        assert!(observe_mesa_native_dispatch(
            launch,
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &selection,
            &cancellation,
        )
        .is_err());
        assert!(
            !inputs.dispatch_root.exists(),
            "changed {changed_owner} must be rejected before dispatch evidence is created"
        );
        assert_no_completed_preflight(&fixture);
    }
}
