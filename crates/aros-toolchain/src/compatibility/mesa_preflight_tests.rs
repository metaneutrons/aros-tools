use super::*;
use crate::compatibility::{
    inspect_llvm_sdk_drivers, prepare_host_tool_closure, CompatibilityHostTool,
    HostToolClosureRequest, LlvmSdkDriverRequest,
};
use crate::profiles::Profiles;
use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};
use aros_common::{
    measure_tree_content_cas, sha256_bytes, ArosCompilerIdentity, CancellationToken, Sha256Digest,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const COMPILER_VERSION: &str = "18.1.8";
const PROFILE_NAME: &str = "arm-raspi";
const PROFILE_TRIPLE: &str = "arm-unknown-aros";

#[path = "mesa_preflight_stage_outcome_tests.rs"]
mod stage_outcome;

#[path = "mesa_preflight_native_root_tests.rs"]
mod native_root_query;

#[path = "mesa_preflight_native_host_tests.rs"]
mod native_host_build;

#[path = "mesa_preflight_actual_include_selection_tests.rs"]
mod actual_include_selection;

#[test]
fn prepared_phase_is_unpublished_and_unchanged_completion_keeps_legacy_evidence() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
    prepared.revalidate(&request, &cancellation).unwrap();
    assert_eq!(prepared.configured.stages.len(), 4);
    for name in [
        "receipt.json",
        INPUT_FILE,
        "mesa-configuration.json",
        "genmodule-build.stdout",
    ] {
        assert!(!fixture.output_root.join(name).exists());
    }
    let completed = prepared.complete(&request, &cancellation).unwrap();
    completed.revalidate(&request, &cancellation).unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output_root.join("receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema"], "aros-mesa-preflight-v3");
    assert_eq!(receipt["stages"].as_array().unwrap().len(), 6);
    let input = fs::read(fixture.output_root.join(INPUT_FILE)).unwrap();
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &fixture.output_root.join(INPUT_FILE),
        &sha256_bytes(&input),
    )
    .unwrap();
    loaded.revalidate().unwrap();
    let programs = loaded
        .compiler_programs()
        .expect("current producer must transport selected compiler programs");
    assert!(programs.matches_root_selection(
        request.drivers.compiler_root(),
        request.drivers.compiler_tree_sha256()
    ));
    assert_eq!(
        programs
            .identity_for_role("target-c")
            .unwrap()
            .invocation_path(),
        request.drivers.clang_path()
    );
    assert_eq!(
        programs
            .identity_for_role("target-cxx")
            .unwrap()
            .invocation_path(),
        request.drivers.clangxx_path()
    );
    let (proof, files) = completed.data_evidence(&request, &cancellation).unwrap();
    assert_eq!(proof.receipt_sha256(), completed.receipt_sha256());
    assert_eq!(files.len(), 17);
}

#[test]
fn prepared_phase_refuses_unbound_mutation_reselection_cancellation_and_expiry_before_children() {
    for counter in [
        "output",
        "source",
        "compiler",
        "timeout",
        "output-root",
        "cancel",
        "expiry",
    ] {
        let fixture = Fixture::new();
        let mut request = fixture.request();
        let cancellation = CancellationToken::default();
        let mut prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
        let other_output = fixture.output_root.with_file_name("foreign-output");
        match counter {
            "output" => fs::write(
                fixture.output_root.join("unbound-native-result"),
                b"unbound",
            )
            .unwrap(),
            "source" => fs::write(fixture.source_root.join("changed-source"), b"changed").unwrap(),
            "compiler" => {
                fs::write(fixture.compiler_root.join("changed-compiler"), b"changed").unwrap();
            }
            "timeout" => request.timeout += Duration::from_secs(1),
            "output-root" => request.output_root = &other_output,
            "cancel" => cancellation.cancel(),
            "expiry" => {
                prepared.configured.started = Instant::now().checked_sub(request.timeout).unwrap();
            }
            _ => unreachable!(),
        }
        let expected = match counter {
            "output" => "Mesa preparation output changed before completion",
            "source" => "Mesa preflight source changed",
            "compiler" => "LLVM compiler tree changed after driver inspection",
            "timeout" | "output-root" => "Mesa preparation selected different inputs",
            "cancel" => "invalid or cancelled Mesa preflight selection",
            "expiry" => "Mesa preparation deadline expired",
            _ => unreachable!(),
        };
        assert_preflight_error(prepared.complete(&request, &cancellation), expected);
        for root in [&fixture.output_root, &other_output] {
            for name in [
                "receipt.json",
                INPUT_FILE,
                "mesa-configuration.json",
                "genmodule-build.stdout",
                "genmodule-make.stdout",
            ] {
                assert!(!root.join(name).exists(), "{counter}: {name}");
            }
        }
    }
}

#[test]
fn explicit_hour_budget_is_bounded_and_does_not_admit_invalid_requests() {
    let fixture = Fixture::new();
    let cancellation = CancellationToken::default();
    for seconds in [0, 3601] {
        let request = fixture.request_with_timeout(Duration::from_secs(seconds));
        assert_preflight_error(
            prepare_mesa_preflight(&request, &cancellation),
            "invalid or cancelled Mesa preflight selection",
        );
        assert!(!fixture.output_root.exists());
    }
    let request = fixture.request_with_timeout(Duration::from_mins(60));
    let mut prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
    assert!(prepared.remaining().unwrap() <= Duration::from_mins(60));
    prepared.configured.started = Instant::now().checked_sub(Duration::from_mins(60)).unwrap();
    assert_preflight_error(prepared.remaining(), "Mesa preparation deadline expired");
}

#[test]
fn prepared_phase_keeps_original_deadline_instead_of_renewing_each_child_clock() {
    let fixture = Fixture::new();
    let request = fixture.request_with_timeout(Duration::from_secs(30));
    let cancellation = CancellationToken::default();
    let mut prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
    let remaining = prepared.remaining().unwrap();
    assert!(remaining > Duration::ZERO && remaining < request.timeout);
    prepared.configured.started = Instant::now().checked_sub(Duration::from_secs(25)).unwrap();
    assert!(prepared.remaining().unwrap() <= Duration::from_secs(5));
    prepared.configured.started = Instant::now().checked_sub(request.timeout).unwrap();
    assert_preflight_error(prepared.remaining(), "Mesa preparation deadline expired");
    let extended = fixture.request_with_timeout(Duration::from_mins(20));
    assert_preflight_error(
        prepared.complete(&extended, &cancellation),
        "Mesa preparation deadline expired",
    );
    assert!(!fixture.output_root.join("genmodule-build.stdout").exists());
}

#[test]
fn completed_stage_inherits_runtime_environment_not_reconstructed_policy() {
    // Generic runner fixture only: no configure, source recipe or target build.
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let control = (&request, &cancellation, Instant::now());
    let program = Path::new("/usr/bin/env");
    let additions = BTreeMap::from([("AROS_STAGE_TEST_INPUT".into(), "runtime-only".into())]);
    let parent = run(
        control,
        "synthetic-parent",
        program,
        &[],
        &fixture.source_root,
        &additions,
    )
    .unwrap();
    let inherited = run_inheriting(
        control,
        "synthetic-inherited",
        program,
        &[],
        &fixture.source_root,
        &parent,
    )
    .unwrap();
    let independently_resolved = run(
        control,
        "synthetic-base-policy",
        program,
        &[],
        &fixture.source_root,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(inherited.environment, parent.environment);
    assert_eq!(
        inherited.invocation_context.environment(),
        parent.invocation_context.environment()
    );
    assert_ne!(independently_resolved.environment, parent.environment);
    let inherited_bytes = fs::read(fixture.output_root.join("synthetic-inherited.stdout")).unwrap();
    let base_bytes = fs::read(fixture.output_root.join("synthetic-base-policy.stdout")).unwrap();
    let line = b"AROS_STAGE_TEST_INPUT=runtime-only";
    assert!(inherited_bytes
        .split(|byte| *byte == b'\n')
        .any(|value| value == line));
    assert!(!base_bytes
        .split(|byte| *byte == b'\n')
        .any(|value| value == line));
}

#[test]
fn owner_scope_refuses_a_different_completed_phase_before_writing_scope_inputs() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output_root).unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let control = (&request, &cancellation, Instant::now());
    let mut stages = Vec::new();
    for name in [
        "synthetic-one",
        "synthetic-two",
        "synthetic-three",
        "not-configured-values",
    ] {
        stages.push(
            run(
                control,
                name,
                Path::new("/usr/bin/env"),
                &[],
                &fixture.source_root,
                &BTreeMap::new(),
            )
            .unwrap(),
        );
    }
    let context = aros_common::make_scope_snapshot::MakeScopeContext {
        owner: "synthetic-owner".into(),
        source_inputs_sha256: fixture.source_tree_sha256.clone(),
        compiler_tree_sha256: fixture.drivers.compiler_tree_sha256().clone(),
        configuration_stdout_sha256: stages[3].stdout.clone(),
        working_directory: fixture
            .output_root
            .join("configure/owner")
            .to_str()
            .unwrap()
            .into(),
        inputs: BTreeMap::from([("TARGET".into(), "synthetic-owner".into())]),
    };
    let error =
        scope::produce(control, Path::new("/usr/bin/env"), &context, &mut stages).unwrap_err();
    assert!(error
        .to_string()
        .contains("owner Make scope lacks its configured invocation"));
    assert!(!fixture.output_root.join("configure").exists());
}

/// Explicit retained inputs only: this opt-in test runs source configure and
/// features, not a compiler/SDK build or release qualification. Host command
/// and Python inputs must already be selected and verified by the operator.
#[test]
#[ignore = "requires explicit retained source/compiler/host/Python case JSON"]
fn actual_source_preflight_with_original_compiler() {
    let case_path = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").expect("set explicit case JSON"),
    );
    assert!(case_path.is_absolute());
    let case: serde_json::Value = serde_json::from_slice(&fs::read(case_path).unwrap()).unwrap();
    let path = |key: &str| {
        let path = PathBuf::from(case[key].as_str().unwrap());
        assert!(path.is_absolute());
        path
    };
    let source = path("source_root").canonicalize().unwrap();
    let compiler = path("compiler_root").canonicalize().unwrap();
    let output = path("output_root");
    let profiles = Profiles::parse(&fs::read(path("profiles_file")).unwrap()).unwrap();
    let profile = profiles.select(case["profile"].as_str().unwrap()).unwrap();
    let source_selection = actual_source_profile(&case, &source);
    let preflight_profile = source_selection.as_ref().map_or_else(
        || profile.into(),
        crate::compatibility::MesaPreflightProfile::from,
    );
    let manifest = aros_common::ArosToolchainManifest::load(&compiler).unwrap();
    assert_eq!(manifest.target_triple, profile.target_triple());
    assert_eq!(manifest.target_profile, preflight_profile.name());
    let cancellation = CancellationToken::default();
    let drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: compiler,
            compiler: manifest.compiler_identity().unwrap(),
            target: source_selection.as_ref().map_or_else(
                || sdk_target(profile.target_triple()),
                |source| source.target().clone(),
            ),
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    )
    .unwrap();
    let host_document: serde_json::Value =
        serde_json::from_slice(&fs::read(path("host_tools_manifest")).unwrap()).unwrap();
    let mut selected_host_tools: Vec<_> = host_document["host_tools"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, program)| CompatibilityHostTool {
            name: name.clone(),
            program: PathBuf::from(program.as_str().unwrap()),
        })
        .collect();
    if let Some(aliases) = case.get("host_tool_aliases") {
        for (alias, role) in aliases.as_object().expect("explicit host-role aliases") {
            let program = selected_host_tools
                .iter()
                .find(|tool| tool.name == role.as_str().unwrap())
                .expect("alias selects an existing explicit host role")
                .program
                .clone();
            selected_host_tools.push(CompatibilityHostTool {
                name: alias.clone(),
                program,
            });
        }
    }
    let host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: output.parent().unwrap().join("host-tools"),
        tools: selected_host_tools,
    })
    .unwrap();
    let python: serde_json::Value =
        serde_json::from_slice(&fs::read(path("python_environment_request")).unwrap()).unwrap();
    let mut variables = BTreeMap::from([
        ("PATH".into(), "/nonexistent".into()),
        ("AROS_FETCH_OFFLINE".into(), "1".into()),
        ("LC_ALL".into(), "C".into()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONHASHSEED".into(), "0".into()),
        ("PYTHONNOUSERSITE".into(), "1".into()),
    ]);
    for name in ["PYTHON", "PYTHONPATH"] {
        variables.insert(
            name.into(),
            python["environment"][name].as_str().unwrap().into(),
        );
    }
    let environment = CompatibilityEnvironment::SealedHostTools {
        variables,
        host_tools,
    };
    let (local_source, source_tree_sha256) = actual_source_selection(&case, &source);
    let request = MesaPreflightRequest {
        source_root: &source,
        source_tree_sha256: &source_tree_sha256,
        local_source: local_source.as_ref(),
        profile: preflight_profile,
        genmodule_selection: None,
        drivers: &drivers,
        environment: &environment,
        output_root: &output,
        timeout: actual_preflight_timeout(&case),
    };
    if let Some(probe) = case.get("inventory_probe") {
        actual_inventory_probe(
            &request,
            source_selection.as_ref().expect("explicit Source profile"),
            probe,
            &cancellation,
        );
        return;
    }
    if let Some(query) = case.get("native_root_query") {
        assert!(case.get("preseal_root_capture").is_none());
        assert!(case.get("root_capture").is_none());
        let query_root = PathBuf::from(query["output_root"].as_str().unwrap());
        let variables = actual_string_array(&query["variables"]);
        let goals = actual_string_array(&query["goals"]);
        let selection = MesaNativeRootQuerySelection {
            output_root: &query_root,
            variables: &variables,
            goals: &goals,
        };
        let observed = observe_mesa_native_root(&request, &selection, &cancellation).unwrap();
        if case.get("native_collection").is_none() && case.get("native_dispatch").is_none() {
            observed
                .revalidate(&request, &selection, &cancellation)
                .unwrap();
        }
        for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
            assert!(!request.output_root.join(name).exists());
        }
        write_actual_root_report(&output, &observed);
        if let Some(host) = case.get("native_host_build") {
            let coordinates = |key: &str| {
                host[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|coordinate| {
                        let variable = coordinate["variable"].as_str().unwrap();
                        let suffix = coordinate["suffix"].as_str().unwrap();
                        assert!(!Path::new(suffix).is_absolute());
                        assert!(Path::new(suffix)
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_))));
                        let base = PathBuf::from(&observed.values()[variable]);
                        if suffix.is_empty() {
                            base
                        } else {
                            base.join(suffix)
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let files = coordinates("fresh_files");
            let directories = coordinates("fresh_directories");
            let absent = coordinates("absent_inputs");
            let host_root = PathBuf::from(host["output_root"].as_str().unwrap());
            let additional_goals = host
                .get("additional_goals")
                .map_or_else(Vec::new, actual_string_array);
            let host_selection = MesaNativeHostBuildSelection {
                output_root: &host_root,
                goal: host["goal"].as_str().unwrap(),
                additional_goals: &additional_goals,
                executable_variable: host["executable_variable"].as_str().unwrap(),
                fresh_files: &files,
                fresh_directories: &directories,
                absent_inputs: &absent,
            };
            let built = build_mesa_native_host(
                observed,
                &request,
                &selection,
                &host_selection,
                &cancellation,
            )
            .unwrap();
            if case.get("native_collection").is_none() && case.get("native_dispatch").is_none() {
                built
                    .revalidate(&request, &selection, &host_selection, &cancellation)
                    .unwrap();
            }
            for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
                assert!(!request.output_root.join(name).exists());
            }
            write(&output.parent().unwrap().join("native-host-observation.json"),
                &crate::canonical::bytes(&serde_json::json!({
                    "qualification":"local-native-host-build-only-not-native-preparation-or-sdk-proof",
                    "executable":built.executable(),"sha256":built.executable_sha256(),
                    "query_evidence":query_root,"host_evidence":host_root,
                })).unwrap()).unwrap();
            if let Some(launch) = case.get("native_launch_query") {
                let launch_root = PathBuf::from(launch["output_root"].as_str().unwrap());
                let selected_strings = |key: &str| {
                    launch[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                };
                let launch_variables = selected_strings("variables");
                let launch_goals = selected_strings("goals");
                let launch_selection = MesaNativeRootQuerySelection {
                    output_root: &launch_root,
                    variables: &launch_variables,
                    goals: &launch_goals,
                };
                let launch_inputs = observe_mesa_native_launch_inputs(
                    built,
                    &request,
                    &selection,
                    &host_selection,
                    &launch_selection,
                    &cancellation,
                )
                .unwrap();
                if case.get("native_collection").is_none() && case.get("native_dispatch").is_none()
                {
                    launch_inputs
                        .revalidate(
                            &request,
                            &selection,
                            &host_selection,
                            &launch_selection,
                            &cancellation,
                        )
                        .unwrap();
                }
                write(&output.parent().unwrap().join("native-launch-inputs.json"),
                    &crate::canonical::bytes(&serde_json::json!({
                        "qualification":"local-native-launch-goal-query-not-root-launch-or-sdk-proof",
                        "goals": launch_goals, "values": launch_inputs.values(),
                        "read_inputs": launch_inputs.read_inputs(), "evidence": launch_root,
                    })).unwrap()).unwrap();
                if let Some(dispatch) = case.get("native_dispatch") {
                    assert!(
                        case.get("native_collection").is_none(),
                        "dispatch and collection are distinct consuming phases"
                    );
                    let evidence = PathBuf::from(dispatch["output_root"].as_str().unwrap());
                    let frontend = PathBuf::from(dispatch["observer"].as_str().unwrap());
                    let observer_sha =
                        Sha256Digest::parse(dispatch["observer_sha256"].as_str().unwrap()).unwrap();
                    let recipe_shell = PathBuf::from(
                        dispatch["recipe_shell"]
                            .as_str()
                            .expect("explicit independent recipe-shell policy"),
                    );
                    let selected = MesaNativeDispatchSelection {
                        output_root: &evidence,
                        observer: &frontend,
                        observer_sha256: &observer_sha,
                        recipe_shell: &recipe_shell,
                        pattern: dispatch["pattern"].as_str().unwrap(),
                        project: dispatch["project"].as_str().unwrap(),
                        prefix_variable: dispatch["prefix_variable"].as_str().unwrap(),
                        program_variable: dispatch["program_variable"].as_str().unwrap(),
                        options_variable: dispatch["options_variable"].as_str().unwrap(),
                    };
                    let observed = observe_mesa_native_dispatch(
                        launch_inputs,
                        &request,
                        &selection,
                        &host_selection,
                        &launch_selection,
                        &selected,
                        &cancellation,
                    )
                    .unwrap();
                    assert!(!evidence.join("pending-command.json").exists());
                    if actual_recorder_requested(dispatch) {
                        let recorded = observed
                            .observe_fenced(
                                &request,
                                &selection,
                                &host_selection,
                                &launch_selection,
                                &selected,
                                &cancellation,
                            )
                            .unwrap()
                            .capture_recorder(
                                &request,
                                &selection,
                                &host_selection,
                                &launch_selection,
                                &selected,
                                &cancellation,
                            )
                            .unwrap();
                        write_actual_recorder_report(&output, &evidence, recorded.captured());
                        if let Some(direct) = dispatch.get("direct_collection") {
                            collect_actual_direct(
                                recorded,
                                &request,
                                &crate::compatibility::MesaNativeDirectOrigin {
                                    host_query: &selection,
                                    host_build: &host_selection,
                                    launch_query: &launch_selection,
                                    dispatch: &selected,
                                },
                                direct,
                                &output,
                                &cancellation,
                            );
                        }
                        return;
                    }
                    write(&output.parent().unwrap().join("native-dispatch.json"),
                        &crate::canonical::bytes(&serde_json::json!({
                            "qualification":"original-owner-first-input-observation-and-source-selection-not-Root-command-or-SDK",
                            "binding":observed.initial_observation().binding(),
                            "source_reads":observed.source_plan().source_reads().reads().iter().map(|read|
                                serde_json::json!({"identity":read.identity,"digest":read.content_sha256})).collect::<Vec<_>>(),
                            "startup_arguments":observed.source_plan().startup().arguments(),
                            "root_sha256":observed.source_plan().capture_plan().original_sha256(),
                        })).unwrap()).unwrap();
                    return;
                }
                if let Some(collection) = case.get("native_collection") {
                    let evidence = PathBuf::from(collection["output_root"].as_str().unwrap());
                    let frontend = PathBuf::from(collection["observer"].as_str().unwrap());
                    let observer_sha256 =
                        Sha256Digest::parse(collection["observer_sha256"].as_str().unwrap())
                            .unwrap();
                    let selected = MesaNativeCollectionSelection {
                        output_root: &evidence,
                        observer: &frontend,
                        observer_sha256: &observer_sha256,
                        project: collection["project"].as_str().unwrap(),
                        source_command_word: collection["source_command_word"].as_str().unwrap(),
                        maximum_calls: usize::try_from(
                            collection["maximum_calls"].as_u64().unwrap(),
                        )
                        .unwrap(),
                    };
                    // Each consuming factory performs the original full-owner
                    // check; omit redundant test-only checks above, not any
                    // production check or the original total deadline.
                    let collected = collect_mesa_native_root(
                        launch_inputs,
                        &request,
                        &selection,
                        &host_selection,
                        &launch_selection,
                        &selected,
                        &cancellation,
                    )
                    .unwrap();
                    write(&output.parent().unwrap().join("native-root-collection.json"),
                        &crate::canonical::bytes(&serde_json::json!({
                            "qualification":"supervised-instrumented-data-not-source-owner-join-or-sdk",
                            "goals":launch_goals,"calls":collected.calls().len(),
                            "observer_context":collected.observer_context_sha256(),
                            "observer_control":collected.observer_control_sha256(),
                            "evidence":evidence,
                            "invocations":collected.calls().iter().map(|call| serde_json::json!({
                                "binding":call.binding(),"cwd":call.invocation().working_directory(),
                                "argv":call.invocation().arguments(),"argv0":call.invocation().argument_zero(),
                            })).collect::<Vec<_>>(),
                        })).unwrap()).unwrap();
                    if let Some(join) = collection.get("source_join") {
                        let config = aros_common::TargetProfile::load_config(
                            &source.join("aros-targets.toml"),
                        )
                        .unwrap();
                        let source_profile = config
                            .targets
                            .iter()
                            .find(|profile| {
                                profile.name == join["source_profile"].as_str().unwrap()
                            })
                            .unwrap();
                        let source_profile =
                            crate::compatibility::SourceMesaPreflightProfile::load(
                                &source,
                                source_profile,
                            )
                            .unwrap();
                        let joined = crate::compatibility::join_mesa_native_source(
                            collected,
                            &request,
                            &selection,
                            &host_selection,
                            &launch_selection,
                            &crate::compatibility::MesaNativeSourceSelection {
                                collection: &selected,
                                source_profile: &source_profile,
                                host: join["host"].as_str().unwrap(),
                            },
                            &cancellation,
                        )
                        .unwrap();
                        write(&output.parent().unwrap().join("native-source-join.json"),
                            &crate::canonical::bytes(&serde_json::json!({
                                "qualification":"source-policy-runtime-coordinate-join-not-leaf-execution-or-sdk",
                                "source_metadata_selection":joined.metadata_selection(),
                                "coordinates":joined.coordinates().iter().map(|coordinate| serde_json::json!({
                                    "generated_owner":coordinate.generated_owner(),
                                    "source_input":coordinate.source_input(),"goals":coordinate.goals(),
                                })).collect::<Vec<_>>(),
                            })).unwrap()).unwrap();
                        if join
                            .get("read_children")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                        {
                            let children = joined
                                .read_children(
                                    &request,
                                    &selection,
                                    &host_selection,
                                    &launch_selection,
                                    &selected,
                                    &cancellation,
                                )
                                .unwrap();
                            let children: Vec<_> = children
                                .into_iter()
                                .map(|child| {
                                    let (coordinate, _program, _file, reads) = child.into_parts();
                                    serde_json::json!({
                                        "generated_owner": coordinate.generated_owner(),
                                        "source_input": coordinate.source_input(),
                                        "goals": coordinate.goals(),
                                        "read_count": reads.reads().len(),
                                    })
                                })
                                .collect();
                            write(&output.parent().unwrap().join("native-source-programs.json"),
                                &crate::canonical::bytes(&serde_json::json!({
                                    "qualification":"input-consistent-native-source-programs-not-recipes-or-sdk",
                                    "children":children,
                                })).unwrap()).unwrap();
                        }
                    }
                }
                for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
                    assert!(!request.output_root.join(name).exists());
                }
            }
        }
        return;
    }
    if let Some(capture) = case.get("preseal_root_capture") {
        assert!(case.get("root_capture").is_none());
        let prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
        super::root_capture_tests::capture_prepared(&request, &prepared, capture, &cancellation);
        // Until native stages have their own closed receipt binding, refuse
        // them rather than emitting a legacy receipt around extra output.
        assert!(prepared.complete(&request, &cancellation).is_err());
        return;
    }
    let result = execute_mesa_preflight(&request, &cancellation).unwrap();
    result.revalidate(&request, &cancellation).unwrap();
    let arguments = result
        .cmake_configuration_arguments(&request, &cancellation)
        .unwrap();
    assert_eq!(arguments.len(), 3);
    assert_eq!(arguments[0], "-DAROS_MESA_CONFIGURATION_MODE=bound");
    assert_eq!(
        arguments[1],
        format!(
            "-DAROS_MESA_CONFIGURATION_INPUT={}",
            output.join(INPUT_FILE).display()
        )
    );
    let digest = Sha256Digest::parse(
        arguments[2]
            .strip_prefix("-DAROS_MESA_CONFIGURATION_INPUT_SHA256=")
            .unwrap(),
    )
    .unwrap();
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &output.join(INPUT_FILE),
        &digest,
    )
    .unwrap();
    assert_eq!(loaded.configuration(), result.configuration());
    assert_eq!(loaded.receipt_sha256(), result.receipt_sha256());
    loaded.revalidate().unwrap();
    let (proof, files) = result.data_evidence(&request, &cancellation).unwrap();
    assert_eq!(files.len(), 17);
    assert_eq!(proof.receipt_sha256(), result.receipt_sha256());
    assert_eq!(
        proof.configuration_identity(),
        result.configuration().identity()
    );
    result.revalidate(&request, &cancellation).unwrap();
    assert!(!result.configuration().target_isa_flags().is_empty());
    if let Some(capture) = case.get("root_capture") {
        super::root_capture_tests::capture(&request, &result, capture, &cancellation);
    }
    eprintln!(
        "local source preflight {} PASS original receipt {} (optional capture is separate)",
        profile.name(),
        result.receipt_sha256()
    );
}

fn actual_string_array(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

fn actual_source_profile(
    case: &serde_json::Value,
    source: &Path,
) -> Option<crate::compatibility::SourceMesaPreflightProfile> {
    case.get("source_preset").map(|preset| {
        let config =
            aros_common::TargetProfile::load_config(&source.join("aros-targets.toml")).unwrap();
        let selected = config
            .targets
            .iter()
            .find(|profile| profile.name == preset.as_str().unwrap())
            .unwrap();
        crate::compatibility::SourceMesaPreflightProfile::load(source, selected).unwrap()
    })
}

fn actual_source_selection(
    case: &serde_json::Value,
    source: &Path,
) -> (
    Option<aros_common::local_source::LocalSourceIdentity>,
    Sha256Digest,
) {
    let local = case.get("local_source").map(|selection| {
        let selection = selection
            .as_object()
            .expect("local Source selection object");
        assert_eq!(
            selection.len(),
            1,
            "only an explicit preset selects a local Source tree"
        );
        let preset = selection["preset"].as_str().expect("local Source preset");
        require_actual_build_namespace(source, preset);
        aros_common::local_source::LocalSourceIdentity::capture(source, preset)
            .expect("independently capture the selected local Source tree")
    });
    let digest = local.as_ref().map_or_else(
        || {
            measure_tree_content_cas(source)
                .unwrap()
                .payload_digest_excluding(None)
        },
        |local| local.content_sha256.clone(),
    );
    (local, digest)
}

fn require_actual_build_namespace(source: &Path, preset: &str) {
    aros_common::local_source::LocalSourceIdentity::validate_build_namespace(source, preset)
        .expect("valid caller-selected local Source build namespace");
    assert!(
        source.join("build").join(preset).is_dir(),
        "create the selected nominal build directory before capturing local Source inputs"
    );
}

/// Isolated data probe of the same argument factory/helper, not recovery of a failed owner.
fn actual_inventory_probe(
    request: &MesaPreflightRequest<'_>,
    profile: &crate::compatibility::SourceMesaPreflightProfile,
    probe: &serde_json::Value,
    cancellation: &CancellationToken,
) {
    let mut selection = probe.as_object().expect("explicit inventory probe").clone();
    let ports = PathBuf::from(
        selection
            .remove("ports_root")
            .expect("explicit probe ports root")
            .as_str()
            .expect("probe ports root is text"),
    );
    assert!(ports.is_absolute());
    let (helper, digest, scope, evidence) = actual_inventory_selection(&selection.into());
    assert_eq!(
        evidence, request.output_root,
        "probe must select its own fresh output"
    );
    assert_eq!(
        crate::compatibility::measure_executable(&helper).unwrap().0,
        digest
    );
    let output = request
        .output_root
        .join(aros_common::mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE);
    let arguments =
        genmodule::inventory_arguments(request, profile, scope, &output, ports.to_str().unwrap())
            .unwrap();
    fs::create_dir(request.output_root).unwrap();
    let stage = run(
        (request, cancellation, Instant::now()),
        "inventory-probe",
        &helper,
        &arguments,
        request.source_root,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(stage.program, digest);
    let inventory =
        aros_common::mesa_genmodule_inventory::ValidatedMesaGenmoduleSelection::parse_bound(
            &read_bytes(&output).unwrap(),
            &profile.genmodule_inventory_binding(scope),
        )
        .unwrap();
    profile.revalidate().unwrap();
    write(&request.output_root.join("probe-result.json"), &crate::canonical::bytes(&serde_json::json!({
        "qualification":"isolated-source-inventory-data-not-original-owner-genmodule-execution-or-sdk",
        "program":stage.program,"invocation":stage.invocation,"environment":stage.environment,
        "source":request.source_tree_sha256,"producers":inventory.producers().len(),
        "records":inventory.inventory().records.len(),
    })).unwrap()).unwrap();
}

fn actual_preflight_timeout(case: &serde_json::Value) -> Duration {
    let seconds = case.get("timeout_seconds").map_or(600, |value| {
        value
            .as_u64()
            .filter(|seconds| (1..=3600).contains(seconds))
            .expect("explicit whole-preflight timeout must be 1..=3600 seconds")
    });
    Duration::from_secs(seconds)
}

#[test]
fn actual_preflight_timeout_keeps_default_and_accepts_bounded_override() {
    assert_eq!(
        actual_preflight_timeout(&serde_json::json!({})),
        Duration::from_secs(600)
    );
    for seconds in [1, 1200, 3600] {
        assert_eq!(
            actual_preflight_timeout(&serde_json::json!({"timeout_seconds": seconds})),
            Duration::from_secs(seconds)
        );
    }
}

#[test]
fn preflight_timeout_selection_accepts_hour_and_rejects_excess_before_output() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    let cancellation = CancellationToken::default();
    request.timeout = Duration::from_secs(3600);
    validate_selection(&request, &cancellation).unwrap();
    for seconds in [0, 3601] {
        request.timeout = Duration::from_secs(seconds);
        assert_preflight_error(
            validate_selection(&request, &cancellation),
            "invalid or cancelled Mesa preflight selection",
        );
        assert!(!fixture.output_root.exists());
    }
}

#[test]
fn actual_preflight_timeout_rejects_invalid_or_unbounded_override() {
    for value in [
        serde_json::json!(0),
        serde_json::json!(3601),
        serde_json::json!(-1),
        serde_json::json!("3600"),
        serde_json::Value::Null,
    ] {
        let case = serde_json::json!({"timeout_seconds": value});
        assert!(std::panic::catch_unwind(|| actual_preflight_timeout(&case)).is_err());
    }
}

fn actual_recorder_requested(dispatch: &serde_json::Value) -> bool {
    dispatch.get("capture_recorder").is_some_and(|enabled| {
        enabled
            .as_bool()
            .expect("explicit boolean recorder phase selection")
    })
}

fn write_actual_root_report(output: &Path, observed: &ObservedMesaNativeRoot) {
    let summary = crate::canonical::bytes(&serde_json::json!({
        "qualification":"local-native-root-query-only-not-native-preparation-or-sdk-proof",
        "identity": observed.configuration().identity(),
        "observed_exit_code": observed.observed_exit_code(),
        "values": observed.values(), "read_inputs": observed.read_inputs(),
        "query_root": observed.output_root(),
    }))
    .unwrap();
    write(
        &output
            .parent()
            .unwrap()
            .join("native-root-observation.json"),
        &summary,
    )
    .unwrap();
}

#[derive(serde::Serialize)]
struct DirectChildSummary {
    original_slot: usize,
    source_input: String,
    generated_owner: String,
    goals: Vec<String>,
    verified_read_count: usize,
    /// Bounded data-only original recipe projection; never an execution receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_recipes: Option<String>,
}

#[derive(Default, serde::Serialize)]
#[serde(transparent)]
struct DirectChildSummaries(Vec<DirectChildSummary>);

impl DirectChildSummaries {
    fn retained_bytes(&self) -> Result<usize, String> {
        let bytes = self
            .0
            .capacity()
            .checked_mul(std::mem::size_of::<DirectChildSummary>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()));
        self.0
            .iter()
            .fold(bytes, |bytes, child| {
                let bytes = bytes?
                    .checked_add(child.source_input.capacity())?
                    .checked_add(child.generated_owner.capacity())?
                    .checked_add(child.declared_recipes.as_ref().map_or(0, String::capacity))?
                    .checked_add(
                        child
                            .goals
                            .capacity()
                            .checked_mul(std::mem::size_of::<String>())?,
                    )?;
                child
                    .goals
                    .iter()
                    .try_fold(bytes, |bytes, goal| bytes.checked_add(goal.capacity()))
            })
            .ok_or_else(|| "direct child summary retained size overflow".to_owned())
    }
}

#[test]
fn direct_child_summary_accounting_includes_unused_owned_capacity() {
    let mut summaries = DirectChildSummaries(Vec::with_capacity(4));
    let empty_bytes = summaries.retained_bytes().unwrap();
    assert_eq!(
        empty_bytes,
        std::mem::size_of::<DirectChildSummaries>()
            + summaries.0.capacity() * std::mem::size_of::<DirectChildSummary>()
    );
    let source_input = String::with_capacity(128);
    let generated_owner = String::with_capacity(256);
    let mut goals = Vec::with_capacity(3);
    goals.push(String::with_capacity(512));
    let declared_recipes = String::with_capacity(1024);
    let owned_bytes = source_input.capacity()
        + generated_owner.capacity()
        + goals.capacity() * std::mem::size_of::<String>()
        + goals[0].capacity()
        + declared_recipes.capacity();
    summaries.0.push(DirectChildSummary {
        original_slot: 0,
        source_input,
        generated_owner,
        goals,
        verified_read_count: 0,
        declared_recipes: Some(declared_recipes),
    });
    assert_eq!(
        summaries.retained_bytes().unwrap(),
        empty_bytes + owned_bytes
    );
    assert!(summaries.retained_bytes().unwrap() > serde_json::to_vec(&summaries).unwrap().len());
}

fn collect_actual_direct(
    recorded: crate::compatibility::RecordedMesaNativeDispatch,
    request: &MesaPreflightRequest<'_>,
    origin: &crate::compatibility::MesaNativeDirectOrigin<'_, '_>,
    direct: &serde_json::Value,
    output: &Path,
    cancellation: &CancellationToken,
) {
    let read_children = direct.get("read_children");
    let fold_children = direct.get("fold_children");
    let execute_recipe = direct.get("execute_recipe");
    let genmodule_inventory = direct
        .get("genmodule_inventory")
        .map(actual_inventory_selection);
    let make_data = direct
        .get("genmodule_make_data")
        .map(actual_genmodule_make_selections);
    assert!(
        make_data.is_none() || genmodule_inventory.is_some(),
        "Make data requires the same selected inventory and Source fold"
    );
    assert!(
        [
            read_children.is_some(),
            fold_children.is_some(),
            execute_recipe.is_some()
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count()
            <= 1,
        "direct child read, fold and execution modes are mutually exclusive"
    );
    assert!(
        genmodule_inventory.is_none()
            || (fold_children.is_some() && read_children.is_none() && execute_recipe.is_none()),
        "native genmodule inventory may accompany only a direct child fold"
    );
    let maximum_calls = usize::try_from(
        direct["maximum_calls"]
            .as_u64()
            .expect("explicit direct collection call limit"),
    )
    .expect("direct collection call limit fits host");
    let fold_selection = fold_children.map(|fold| {
        let object = fold.as_object().expect("explicit direct child fold object");
        let fields = [
            "maximum_children",
            "maximum_read_bytes",
            "maximum_reads",
            "maximum_queries",
            "maximum_query_names",
            "maximum_query_bytes",
            "maximum_record_bytes",
            "maximum_retained_bytes",
            "maximum_projected_bytes",
        ];
        assert_eq!(
            object.len(),
            fields.len() + usize::from(object.contains_key("required")),
            "unexpected direct fold field"
        );
        assert!(
            object
                .keys()
                .all(|field| field == "required" || fields.contains(&field.as_str())),
            "unexpected direct fold field"
        );
        assert!(
            fields.iter().all(|field| object.contains_key(*field)),
            "direct child fold requires explicit limits"
        );
        assert!(
            genmodule_inventory.is_some() || object.contains_key("required"),
            "direct child fold requires explicit requirements without an inventory selection"
        );

        let required = object.get("required").map(|required| {
            let required_values = required
                .as_array()
                .expect("explicit required direct Source coordinates");
            assert!(
                !required_values.is_empty() && required_values.len() <= 4096,
                "direct Source fold requirements are outside 1..=4096"
            );
            let mut unique_requirements = std::collections::BTreeSet::new();
            let mut required_bytes = 0usize;
            let required_byte_limit = 16 * 1024 * 1024
                - std::mem::size_of::<
                    Vec<aros_transpiler::native_owner_projection::MatchedOwnerChildCoordinates>,
                >();
            for value in required_values {
                let pair = value
                    .as_object()
                    .expect("direct Source fold requirement object");
                assert_eq!(pair.len(), 2, "unexpected direct Source requirement field");
                let source_input = pair["source_input"]
                    .as_str()
                    .expect("explicit original Source input");
                let goal = pair["goal"].as_str().expect("explicit observed child goal");
                assert!(
                    !source_input.is_empty() && !goal.is_empty(),
                    "direct Source fold requirement fields must be nonempty"
                );
                assert!(
                    unique_requirements.insert((source_input, goal)),
                    "direct Source fold requirements must be unique"
                );
                let cost = source_input
                    .len()
                    .checked_add(goal.len())
                    .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>()))
                    .expect("direct Source requirement byte bound fits host");
                required_bytes = required_bytes
                    .checked_add(cost)
                    .expect("direct Source requirement byte total fits host");
                assert!(
                    required_bytes <= required_byte_limit,
                    "direct Source fold requirements exceed their byte bound"
                );
            }
            required_values
                .iter()
                .map(|value| {
                    (
                        value["source_input"].as_str().unwrap().to_owned(),
                        value["goal"].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        });

        let limit = |name: &str| {
            usize::try_from(
                object[name]
                    .as_u64()
                    .expect("explicit positive direct child fold limit"),
            )
            .expect("direct child fold limit fits host")
        };
        let inputs = aros_common::make_input_snapshot::MakeInputBatchLimits {
            maximum_children: limit("maximum_children"),
            maximum_read_bytes: limit("maximum_read_bytes"),
            maximum_reads: limit("maximum_reads"),
            maximum_queries: limit("maximum_queries"),
            maximum_query_names: limit("maximum_query_names"),
            maximum_query_bytes: limit("maximum_query_bytes"),
            maximum_record_bytes: limit("maximum_record_bytes"),
        };
        assert!((1..=4096).contains(&maximum_calls));
        assert_eq!(
            inputs.maximum_children, maximum_calls,
            "direct fold child limit must equal the explicit call limit"
        );
        assert!((1..=4096).contains(&inputs.maximum_children));
        assert!((1..=1024 * 1024 * 1024).contains(&inputs.maximum_read_bytes));
        for value in [
            inputs.maximum_reads,
            inputs.maximum_queries,
            inputs.maximum_query_names,
        ] {
            assert!((1..=1_000_000).contains(&value));
        }
        for value in [inputs.maximum_query_bytes, inputs.maximum_record_bytes] {
            assert!((1..=64 * 1024 * 1024).contains(&value));
        }
        let maximum_retained_bytes = limit("maximum_retained_bytes");
        let maximum_projected_bytes = limit("maximum_projected_bytes");
        assert!((1..=1024 * 1024 * 1024).contains(&maximum_retained_bytes));
        assert!((1..=1024 * 1024 * 1024).contains(&maximum_projected_bytes));
        (
            required,
            aros_transpiler::native_owner_projection::RetainedChildSourceFoldLimits {
                inputs,
                maximum_child_retained_bytes: maximum_retained_bytes,
                maximum_projected_bytes,
            },
        )
    });

    let evidence = PathBuf::from(direct["output_root"].as_str().unwrap());
    let observer = PathBuf::from(direct["observer"].as_str().unwrap());
    let observer_sha256 = Sha256Digest::parse(direct["observer_sha256"].as_str().unwrap()).unwrap();
    let selected = crate::compatibility::MesaNativeCollectionSelection {
        output_root: &evidence,
        observer: &observer,
        observer_sha256: &observer_sha256,
        project: direct["project"].as_str().unwrap(),
        source_command_word: direct["source_command_word"].as_str().unwrap(),
        maximum_calls,
    };
    let targets =
        aros_common::TargetProfile::load_config(&request.source_root.join("aros-targets.toml"))
            .unwrap();
    let profile = targets
        .targets
        .iter()
        .find(|target| target.name == direct["source_profile"].as_str().unwrap())
        .unwrap();
    let profile =
        crate::compatibility::SourceMesaPreflightProfile::load(request.source_root, profile)
            .unwrap();
    let roots = direct["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let prepared = crate::compatibility::prepare_mesa_native_direct_collection(
        recorded,
        request,
        origin,
        &crate::compatibility::MesaNativeSourceSelection {
            collection: &selected,
            source_profile: &profile,
            host: direct["host"].as_str().unwrap(),
        },
        &roots,
        cancellation,
    )
    .unwrap();
    write(
        &output
            .parent()
            .unwrap()
            .join("native-direct-preparation.json"),
        &crate::canonical::bytes(&serde_json::json!({
            "qualification":"original-owner-direct-preparation-not-execution-or-sdk",
            "selection":prepared.selection_sha256(),"roots":prepared.roots(),
            "source_metadata":prepared.metadata_selection(),
        }))
        .unwrap(),
    )
    .unwrap();
    let collected = prepared
        .collect(request, origin, &selected, cancellation)
        .unwrap();
    // Retain this completed data stage before any later helper can fail. A
    // diagnostic may read it, but it cannot restore the consumed live owner.
    write(&output.parent().unwrap().join("native-direct-collection.json"),
        &crate::canonical::bytes(&serde_json::json!({
            "qualification":"direct-metamake-first-input-and-source-coverage-not-leaf-or-sdk",
            "selection":collected.preparation().selection_sha256(),"roots":roots,
            "evidence":evidence,"calls":collected.calls().len(),
            "coordinates":collected.coordinates().iter().map(|coordinate| serde_json::json!({
                "owner":coordinate.generated_owner(),"source":coordinate.source_input(),"goals":coordinate.goals(),
            })).collect::<Vec<_>>(),
            "invocations":collected.calls().iter().map(|call| serde_json::json!({
                "binding":call.binding(),"cwd":call.invocation().working_directory(),
                "argv0":call.invocation().argument_zero(),"argv":call.invocation().arguments(),
            })).collect::<Vec<_>>(),
        })).unwrap()).unwrap();
    let control = (request, origin, &selected, cancellation);
    let (collected, selected_inventory, inventory_required) =
        if let Some((helper, helper_sha256, scope, evidence_root)) = genmodule_inventory {
            let inventory_selection = NativeGenmoduleInventorySelection {
                helper: &helper,
                helper_sha256: &helper_sha256,
                scope,
                evidence_root: &evidence_root,
            };
            let (collected, inventory) = collected
                .collect_genmodule_inventory(control, &inventory_selection)
                .unwrap();
            let mut required = Vec::with_capacity(inventory.owners().len());
            let mut unique = std::collections::BTreeSet::new();
            for owner in inventory.owners() {
                let coordinate = collected
                    .coordinates()
                    .get(owner.call_index)
                    .expect("selected inventory call index addresses an original call slot");
                assert!(
                    coordinate.goals().contains(&owner.owner),
                    "selected inventory owner must match its original Source call slot"
                );
                let pair = (coordinate.source_input().to_owned(), owner.owner.clone());
                assert!(
                    unique.insert(pair.clone()),
                    "selected inventory Source requirements must be unique"
                );
                required.push(pair);
            }
            assert!(
                !required.is_empty(),
                "selected native genmodule inventory has no original Source owners"
            );
            assert!(
                required.len() <= 4096,
                "selected native genmodule requirements exceed the fold bound"
            );
            let required_bytes = required.iter().try_fold(0usize, |bytes, (source, goal)| {
                bytes
                    .checked_add(source.len())
                    .and_then(|bytes| bytes.checked_add(goal.len()))
                    .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>()))
            });
            let required_byte_limit = 16 * 1024 * 1024
                - std::mem::size_of::<
                    Vec<aros_transpiler::native_owner_projection::MatchedOwnerChildCoordinates>,
                >();
            assert!(
                required_bytes.is_some_and(|bytes| bytes <= required_byte_limit),
                "selected native genmodule requirements exceed their byte bound"
            );
            inventory
                .revalidate(&collected, control)
                .expect("selected original genmodule inventory should revalidate");
            (collected, Some(inventory), Some(required))
        } else {
            (collected, None, None)
        };
    assert!(!output.join("receipt.json").exists());
    if let Some(reader) = read_children {
        let limit = |name| {
            usize::try_from(reader[name].as_u64().expect("explicit child reader limit"))
                .expect("child reader limit fits host")
        };
        let limits = aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits {
            inputs: aros_common::make_input_snapshot::MakeInputBatchLimits {
                maximum_children: limit("maximum_children"),
                maximum_read_bytes: limit("maximum_read_bytes"),
                maximum_reads: limit("maximum_reads"),
                maximum_queries: limit("maximum_queries"),
                maximum_query_names: limit("maximum_query_names"),
                maximum_query_bytes: limit("maximum_query_bytes"),
                maximum_record_bytes: limit("maximum_record_bytes"),
            },
            maximum_retained_bytes: limit("maximum_retained_bytes"),
        };
        if let Some(execution) = reader.get("generate_include") {
            execute_actual_include(collected, control, limits, execution, output);
            return;
        }
        let children = collected
            .read_children(request, origin, &selected, limits, cancellation)
            .unwrap();
        assert_eq!(children.len(), collected.calls().len());
        write(
            &output
                .parent()
                .unwrap()
                .join("native-direct-child-source.json"),
            &crate::canonical::bytes(&serde_json::json!({
                "qualification":"original-owner-child-source-data-not-leaf-execution-or-sdk",
                "selection":collected.preparation().selection_sha256(),
                "children":children.len(),
                "coordinates":children.iter().map(|child| serde_json::json!({
                    "owner":child.coordinates().generated_owner(),
                    "source":child.coordinates().source_input(),
                    "goals":child.coordinates().goals(),
                })).collect::<Vec<_>>(),
            }))
            .unwrap(),
        )
        .unwrap();
    } else if let Some(execution) = execute_recipe {
        execute_actual_direct_recipe(
            collected,
            request,
            origin,
            &selected,
            execution,
            output,
            cancellation,
        );
    } else if let Some((explicit_required, limits)) = fold_selection {
        let required = if let Some(inventory_required) = &inventory_required {
            if let Some(explicit_required) = explicit_required {
                let explicit = explicit_required
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                let derived = inventory_required
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                assert_eq!(
                    explicit, derived,
                    "explicit direct fold requirements must equal the selected inventory owners"
                );
            }
            inventory_required.clone()
        } else {
            explicit_required.expect("direct fold requirements were required without inventory")
        };
        let source_projection = selected_inventory.is_some();
        let original_build = collected
            .preparation()
            .invocation()
            .working_directory()
            .to_owned();
        let original_working_directories = collected
            .calls()
            .iter()
            .map(|call| call.invocation().working_directory().to_owned())
            .collect::<Vec<_>>();
        let projected_limit = limits.maximum_projected_bytes;
        let data_limits =
            aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits {
                inputs: limits.inputs,
                maximum_retained_bytes: limits.maximum_child_retained_bytes,
            };
        let (collected, folded) = collected
            .fold_children(
                request,
                origin,
                &selected,
                limits,
                cancellation,
                (
                    &required,
                    DirectChildSummaries::default(),
                    |summary: &mut DirectChildSummaries, original_slot, child| {
                        let coordinates = child.coordinates();
                        let source_input = coordinates.source_input().to_owned();
                        let generated_owner = coordinates.generated_owner().to_owned();
                        let goals = coordinates.goals().to_vec();
                        summary.0.push(DirectChildSummary {
                            original_slot,
                            source_input,
                            generated_owner,
                            goals,
                            verified_read_count: 0,
                            declared_recipes: None,
                        });
                        let remaining = projected_limit
                            .checked_sub(summary.retained_bytes()?)
                            .ok_or("direct Source summary exceeds its projected byte limit")?;
                        let (reads, recipes) = if source_projection {
                            let (reads, recipes) = project_actual_inventory_child(
                                child,
                                (
                                    request.source_root,
                                    &original_build,
                                    original_working_directories
                                        .get(original_slot)
                                        .ok_or("original recipe call slot is absent")?,
                                ),
                                remaining,
                            )?;
                            (reads, Some(recipes))
                        } else {
                            let (_, _program, _file, reads) = child.into_parts();
                            (reads.reads().len(), None)
                        };
                        let result = summary
                            .0
                            .last_mut()
                            .ok_or("direct Source summary is absent")?;
                        result.verified_read_count = reads;
                        result.declared_recipes = recipes;
                        summary.retained_bytes()
                    },
                ),
            )
            .unwrap();
        collected
            .revalidate(request, origin, &selected, cancellation)
            .unwrap();
        if let Some(inventory) = &selected_inventory {
            inventory
                .revalidate(&collected, control)
                .expect("selected genmodule inventory should revalidate after Source fold");
        }
        let selected_children = folded.children();
        let maximum_child_retained_bytes = folded.maximum_child_retained_bytes();
        let projected_bytes = folded.projected_bytes();
        let summary = folded.into_value();
        assert_eq!(summary.0.len(), selected_children);
        write(
            &output
                .parent()
                .unwrap()
                .join("native-direct-child-source.json"),
            &crate::canonical::bytes(&serde_json::json!({
                "qualification":"original-owner-selected-child-source-data-not-recipes-or-sdk",
                "selection":collected.preparation().selection_sha256(),
                "full_calls":collected.calls().len(),
                "selected_children":selected_children,
                "requirements":required.iter().map(|(source_input, goal)| serde_json::json!({
                    "source_input":source_input,"goal":goal,
                })).collect::<Vec<_>>(),
                "summary":summary,
                "maximum_child_retained_bytes":maximum_child_retained_bytes,
                "projected_bytes":projected_bytes,
                "projected_accounting":"owned representation including collection/string capacity; not allocator overhead or RSS",
            }))
            .unwrap(),
        )
        .unwrap();
        if let Some(selections) = make_data {
            execute_actual_genmodule_make_data(
                collected,
                control,
                selected_inventory.as_ref().unwrap(),
                data_limits,
                &selections,
                output,
            );
        }
    }
}

fn execute_actual_include(
    collected: crate::compatibility::CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &crate::compatibility::MesaNativeDirectOrigin<'_, '_>,
        &crate::compatibility::MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits,
    selection: &serde_json::Value,
    output: &Path,
) {
    let object = selection
        .as_object()
        .expect("include execution selection object");
    assert_eq!(
        object.len(),
        4,
        "include execution has only explicit coordinates/evidence"
    );
    let source = object["source_input"].as_str().unwrap();
    let goal = object["goal"].as_str().unwrap();
    let basename = object["include_basename"].as_str().unwrap();
    assert!(!basename.is_empty() && !basename.contains('/'));
    let evidence = PathBuf::from(object["evidence_root"].as_str().unwrap());
    let required = [(source.to_owned(), goal.to_owned())];
    let (collected, folded) = collected
        .fold_children(
            control.0,
            control.1,
            control.2,
            aros_transpiler::native_owner_projection::RetainedChildSourceFoldLimits {
                inputs: limits.inputs,
                maximum_child_retained_bytes: limits.maximum_retained_bytes,
                maximum_projected_bytes: 16 * 1024,
            },
            control.3,
            (
                &required,
                None::<(usize, usize, String)>,
                |state, slot, child| {
                    if state.is_some() {
                        return Err("one original include child required".into());
                    }
                    let (_, _, file, _) = child.into_parts();
                    let mut events = file.events.iter().enumerate().filter_map(|(index, event)| {
                        if !event.active {
                            return None;
                        }
                        match &event.kind {
                            aros_transpiler::make_vars::MakeFileEventKind::Include {
                                requested,
                                present: false,
                                ..
                            } if Path::new(requested).file_name()
                                == Some(std::ffi::OsStr::new(basename)) =>
                            {
                                Some((index, requested))
                            }
                            _ => None,
                        }
                    });
                    let (event, requested) =
                        events.next().ok_or("original missing include absent")?;
                    if events.next().is_some() || requested.len() > 8192 {
                        return Err("original missing include ambiguous or oversized".into());
                    }
                    *state = Some((slot, event, requested.clone()));
                    Ok(std::mem::size_of_val(state) + requested.len())
                },
            ),
        )
        .expect("selected original include input must be interpreted under complete call coverage");
    assert_eq!(folded.children(), 1);
    let (slot, event, requested) = folded.into_value().unwrap();
    let generated = collected
        .generate_original_make_include(
            control,
            limits,
            &crate::compatibility::NativeMakeIncludeExecution {
                child_slot: slot,
                include_event: event,
                evidence_root: &evidence,
            },
        )
        .expect("original directory and GENMODULE include producer must complete");
    let include = generated.include_path().to_owned();
    let (generated, declared) = generated
        .read_declared_child(control, limits)
        .expect("completed original include must reparse at its original source site");
    generated.revalidate(control).unwrap();
    write(&output.parent().unwrap().join("native-original-include.json"),
        &crate::canonical::bytes(&serde_json::json!({
            "qualification": "original-directory-genmodule-and-positioned-include-not-full-mesa-or-sdk",
            "source_input": source, "goal": goal, "requested": requested,
            "include": include, "include_sha256": sha256_bytes(&fs::read(&include).unwrap()),
            "declared_child": declared.coordinates().generated_owner(), "evidence": evidence,
        })).unwrap()).unwrap();
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ActualGenmoduleMakeSelection {
    owner: String,
    recipe_index: usize,
    nominal_destination: PathBuf,
    evidence_root: PathBuf,
}

fn actual_genmodule_make_selections(
    value: &serde_json::Value,
) -> Vec<ActualGenmoduleMakeSelection> {
    let rows = value
        .as_array()
        .expect("explicit original-owner Make data selections");
    assert!(
        (1..=2048).contains(&rows.len()),
        "bounded nonempty Make data selection count"
    );
    let selections = rows
        .iter()
        .map(|row| {
            serde_json::from_value::<ActualGenmoduleMakeSelection>(row.clone())
                .expect("closed Make data selection fields")
        })
        .collect::<Vec<_>>();
    let mut owners = std::collections::BTreeSet::new();
    for selection in &selections {
        assert!(
            !selection.owner.is_empty()
                && selection.owner.len() <= 255
                && owners.insert(selection.owner.as_str()),
            "unique nonempty bounded Make data owner"
        );
        assert!(
            selection.recipe_index < 65_536,
            "bounded Make data recipe index"
        );
        for path in [&selection.nominal_destination, &selection.evidence_root] {
            assert!(
                path.is_absolute()
                    && path.components().all(|part| matches!(
                        part,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )),
                "absolute normalized Make data path"
            );
        }
    }
    selections
}

#[test]
fn actual_genmodule_make_selection_requires_closed_bounded_unique_policy() {
    let row = serde_json::json!({"owner":"source-owner", "recipe_index":2, "nominal_destination":"/private/selected/gen", "evidence_root":"/private/selected/data"});
    let valid = actual_genmodule_make_selections(&serde_json::json!([row]));
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].recipe_index, 2);
    let mut unknown = row.clone();
    unknown["inherited_role"] = "foreign".into();
    let mut missing = row.clone();
    missing.as_object_mut().unwrap().remove("recipe_index");
    let mut index = row.clone();
    index["recipe_index"] = 65536.into();
    let mut path = row.clone();
    path["evidence_root"] = "/private/../foreign".into();
    for (value, expected) in [
        (serde_json::json!([]), "selection count"),
        (
            serde_json::json!([row.clone(), row]),
            "unique nonempty bounded",
        ),
        (
            serde_json::json!([unknown]),
            "closed Make data selection fields",
        ),
        (
            serde_json::json!([missing]),
            "closed Make data selection fields",
        ),
        (serde_json::json!([index]), "bounded Make data recipe index"),
        (
            serde_json::json!([path]),
            "absolute normalized Make data path",
        ),
    ] {
        let failure = std::panic::catch_unwind(|| actual_genmodule_make_selections(&value))
            .err()
            .expect("invalid policy must refuse");
        let reason = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(
            reason.contains(expected),
            "unexpected policy refusal: {reason}"
        );
    }
}

fn execute_actual_genmodule_make_data(
    mut owner: crate::compatibility::CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &crate::compatibility::MesaNativeDirectOrigin<'_, '_>,
        &crate::compatibility::MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    inventory: &SelectedNativeGenmoduleInventory,
    limits: aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits,
    selections: &[ActualGenmoduleMakeSelection],
    output: &Path,
) {
    let expected = inventory
        .inventory()
        .producers()
        .iter()
        .map(|producer| producer.owner.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        expected,
        selections
            .iter()
            .map(|selection| selection.owner.as_str())
            .collect(),
        "the actual data gate must cover every selected inventory owner"
    );
    let mut results = Vec::with_capacity(selections.len());
    for selection in selections {
        let (next, data) = owner
            .produce_genmodule_make_data(
                control,
                inventory,
                limits,
                &crate::compatibility::NativeGenmoduleMakeSelection {
                    owner: &selection.owner,
                    recipe_index: selection.recipe_index,
                    nominal_destination: &selection.nominal_destination,
                    evidence_root: &selection.evidence_root,
                },
            )
            .expect("actual original-owner fixed Genmodule Make data operation");
        owner = next;
        results.push(data);
    }
    for data in &results {
        data.revalidate(&owner, control, inventory)
            .expect("all original-owner Genmodule data outcomes revalidate together");
    }
    write(&output.parent().unwrap().join("native-genmodule-make-data.json"), &crate::canonical::bytes(&serde_json::json!({
        "qualification":"original-owner-fixed-genmodule-make-data-not-project-writefiles-or-mesa-sdk",
        "selection":owner.preparation().selection_sha256(),
        "owners":results.iter().map(|data| serde_json::json!({"output":data.output_binding(),"producer_sha256":data.producer_sha256(),"makefile":data.raw_makefile_path()})).collect::<Vec<_>>(),
    })).unwrap()).unwrap();
    assert!(!output.join("receipt.json").exists());
}

fn actual_inventory_selection(
    selection: &serde_json::Value,
) -> (
    PathBuf,
    Sha256Digest,
    aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    PathBuf,
) {
    let object = selection
        .as_object()
        .expect("explicit native genmodule inventory selection object");
    let fields = ["helper", "helper_sha256", "scope", "evidence_root"];
    assert_eq!(
        object.len(),
        fields.len(),
        "unexpected native genmodule inventory field"
    );
    assert!(
        fields.iter().all(|field| object.contains_key(*field)),
        "native genmodule inventory requires explicit helper, digest, scope and evidence"
    );
    let helper = PathBuf::from(
        object["helper"]
            .as_str()
            .expect("explicit selected native genmodule helper"),
    );
    assert!(
        helper.is_absolute(),
        "native genmodule helper must be absolute"
    );
    let helper_sha256 = Sha256Digest::parse(
        object["helper_sha256"]
            .as_str()
            .expect("explicit native genmodule helper digest"),
    )
    .expect("valid native genmodule helper digest");
    let scope = serde_json::from_value::<
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    >(object["scope"].clone())
    .expect("explicit native genmodule inventory scope");
    let evidence_root = PathBuf::from(
        object["evidence_root"]
            .as_str()
            .expect("explicit native genmodule evidence root"),
    );
    assert!(
        evidence_root.is_absolute(),
        "native genmodule evidence root must be absolute"
    );
    (helper, helper_sha256, scope, evidence_root)
}

#[test]
fn actual_inventory_selection_keeps_closed_fields_and_explicit_paths() {
    let valid = serde_json::json!({"helper":"/fixture/helper", "helper_sha256":"0".repeat(64),
        "scope":"contract-roots", "evidence_root":"/fixture/inventory"});
    let selected = actual_inventory_selection(&valid);
    assert_eq!(selected.0, Path::new("/fixture/helper"));
    assert_eq!(
        selected.2,
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::ContractRoots
    );
    for (field, value) in [
        ("helper", serde_json::json!("relative")),
        ("evidence_root", serde_json::json!("relative")),
        ("scope", serde_json::json!("other")),
        ("helper_sha256", serde_json::json!("invalid")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut counter = valid.clone();
        counter[field] = value;
        assert!(std::panic::catch_unwind(|| actual_inventory_selection(&counter)).is_err());
    }
    let mut absent = valid;
    absent.as_object_mut().unwrap().remove("helper");
    assert!(std::panic::catch_unwind(|| actual_inventory_selection(&absent)).is_err());
}

#[derive(serde::Serialize)]
struct ActualRecipeLine<'a> {
    target: &'a str,
    visit: usize,
    line: usize,
    body: &'a str,
    input: &'a str,
    source_line: usize,
    generated: bool,
    active: bool,
    variable_position: usize,
}

fn project_actual_inventory_child(
    child: aros_transpiler::native_owner_projection::MatchedChildSource,
    roots: (&Path, &Path, &Path),
    maximum_bytes: usize,
) -> Result<(usize, String), String> {
    let relative = Path::new(child.coordinates().generated_owner())
        .parent()
        .ok_or("original owner lacks directory")?
        .to_owned();
    let dirs = aros_transpiler::dirs::DirVars::default();
    let paths = aros_transpiler::make_vars::MakeProgramPaths {
        dirs: &dirs,
        source_root: roots.0,
        relative_dir: &relative,
        top_directory: roots.1,
        working_directory: roots.2,
    };
    let recipes = child.trace_declared_rules()?.project_recipes(&paths)?;
    let lines = recipes.recipes().iter().enumerate().map(|(index, recipe)| {
        let origin = recipes.origin(index)?;
        let target = recipes
            .declared()
            .traversal()
            .visits()
            .get(recipe.visit())
            .ok_or("original recipe visit is absent")?
            .target();
        Ok(ActualRecipeLine {
            target,
            visit: recipe.visit(),
            line: recipe.recipe(),
            body: recipe.expression().expression(),
            input: &origin.input,
            source_line: origin.source_line,
            generated: origin.generated,
            active: origin.active,
            variable_position: origin.variable_position,
        })
    });
    let encoded = bounded_actual_recipe_array(lines, maximum_bytes)?;
    recipes.verify()?;
    Ok((recipes.input_reads().reads().len(), encoded))
}

/// Stream borrowed records into a fixed output budget, not an unbounded Vec<Value>.
fn bounded_actual_recipe_array<T: serde::Serialize>(
    lines: impl IntoIterator<Item = Result<T, String>>,
    maximum_bytes: usize,
) -> Result<String, String> {
    use std::io::Write as _;
    let mut bytes = vec![0; maximum_bytes];
    let mut writer = std::io::Cursor::new(bytes.as_mut_slice());
    let refusal = |_| "original recipe projection exceeds its selected byte limit".to_owned();
    writer.write_all(b"[").map_err(refusal)?;
    for (index, line) in lines.into_iter().enumerate() {
        if index != 0 {
            writer.write_all(b",").map_err(refusal)?;
        }
        serde_json::to_writer(&mut writer, &line?)
            .map_err(|_| "original recipe projection exceeds its selected byte limit")?;
    }
    writer.write_all(b"]").map_err(refusal)?;
    let length =
        usize::try_from(writer.position()).map_err(|_| "recipe projection size overflow")?;
    bytes.truncate(length);
    bytes.shrink_to_fit();
    String::from_utf8(bytes).map_err(|_| "recipe projection is not UTF-8".to_owned())
}

#[test]
fn actual_recipe_projection_enforces_its_byte_limit_during_serialization() {
    let lines = || [Ok("abc")];
    assert_eq!(
        bounded_actual_recipe_array(lines(), 7).unwrap(),
        "[\"abc\"]"
    );
    assert!(bounded_actual_recipe_array(lines(), 6)
        .unwrap_err()
        .contains("byte limit"));
    assert!(bounded_actual_recipe_array([Ok("abc"), Ok("abc")], 7).is_err());
    assert!(bounded_actual_recipe_array(lines(), 0).is_err());
}

fn execute_actual_direct_recipe(
    collected: crate::compatibility::CollectedMesaNativeDirect,
    request: &MesaPreflightRequest<'_>,
    origin: &crate::compatibility::MesaNativeDirectOrigin<'_, '_>,
    selected: &crate::compatibility::MesaNativeCollectionSelection<'_>,
    execution: &serde_json::Value,
    output: &Path,
    cancellation: &CancellationToken,
) {
    let object = execution
        .as_object()
        .expect("explicit recipe execution selection");
    let fields = [
        "child_slot",
        "recipe_index",
        "shell",
        "evidence_root",
        "mutable_files",
        "replaceable_files",
        "fresh_files",
        "fresh_directories",
        "limits",
    ];
    assert_eq!(
        object.len(),
        fields.len(),
        "unexpected recipe execution field"
    );
    assert!(fields.iter().all(|field| object.contains_key(*field)));
    let number = |value: &serde_json::Value| {
        usize::try_from(
            value
                .as_u64()
                .expect("explicit nonnegative execution limit"),
        )
        .expect("execution limit fits host")
    };
    let limits = execution["limits"]
        .as_object()
        .expect("explicit recipe input limits");
    let limit_fields = [
        "maximum_children",
        "maximum_read_bytes",
        "maximum_reads",
        "maximum_queries",
        "maximum_query_names",
        "maximum_query_bytes",
        "maximum_record_bytes",
        "maximum_retained_bytes",
    ];
    assert_eq!(limits.len(), limit_fields.len());
    assert!(limit_fields.iter().all(|field| limits.contains_key(*field)));
    let names = |field: &str| {
        execution[field]
            .as_array()
            .expect("explicit recipe output names")
            .iter()
            .map(|value| value.as_str().expect("recipe output name is text"))
            .collect::<Vec<_>>()
    };
    let mutable = names("mutable_files");
    let replaceable = names("replaceable_files");
    let fresh = names("fresh_files");
    let directories = names("fresh_directories");
    let shell = Path::new(
        execution["shell"]
            .as_str()
            .expect("explicit selected recipe shell"),
    );
    let evidence = Path::new(
        execution["evidence_root"]
            .as_str()
            .expect("explicit fresh execution evidence"),
    );
    assert!(shell.is_absolute() && evidence.is_absolute());
    let result = collected
        .execute_mesa26_original_recipe(
            (request, origin, selected, cancellation),
            aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits {
                inputs: aros_common::make_input_snapshot::MakeInputBatchLimits {
                    maximum_children: number(&limits["maximum_children"]),
                    maximum_read_bytes: number(&limits["maximum_read_bytes"]),
                    maximum_reads: number(&limits["maximum_reads"]),
                    maximum_queries: number(&limits["maximum_queries"]),
                    maximum_query_names: number(&limits["maximum_query_names"]),
                    maximum_query_bytes: number(&limits["maximum_query_bytes"]),
                    maximum_record_bytes: number(&limits["maximum_record_bytes"]),
                },
                maximum_retained_bytes: number(&limits["maximum_retained_bytes"]),
            },
            &crate::compatibility::Mesa26OriginalRecipeExecution {
                child_slot: number(&execution["child_slot"]),
                recipe_index: number(&execution["recipe_index"]),
                selected_shell: shell,
                evidence_root: evidence,
                mutable_files: &mutable,
                replaceable_files: &replaceable,
                fresh_files: &fresh,
                fresh_directories: &directories,
            },
        )
        .unwrap();
    write(&output.parent().unwrap().join("native-direct-recipe-execution.json"),
        &crate::canonical::bytes(&serde_json::json!({
            "qualification":"one-original-recipe-captured-input-observation-not-rule-producer-or-sdk",
            "selection":result.original_selection(), "invocation":result.invocation_sha256(),
            "output_payload":result.output_snapshot().payload_digest_excluding(None),
            "evidence_payload":result.evidence_snapshot().payload_digest_excluding(None),
        })).unwrap()).unwrap();
    assert!(!output.join("receipt.json").exists());
}

fn write_actual_recorder_report(
    output: &Path,
    evidence: &Path,
    captured: &aros_transpiler::root_dispatch_capture::CapturedRootDispatch,
) {
    write(&output.parent().unwrap().join("native-dispatch-capture.json"),
        &crate::canonical::bytes(&serde_json::json!({
            "qualification":"original-owner-fenced-recorder-and-checked-source-export-data-not-MetaMake-or-SDK",
            "evidence": evidence,
            "raw_invocation_sha256": sha256_bytes(&captured.raw().to_bytes().unwrap()),
            "source_restored_invocation_sha256": sha256_bytes(&captured.invocation().to_bytes().unwrap()),
            "raw_arguments": captured.raw().arguments(),
            "arguments": captured.invocation().arguments(),
            "argument_zero": captured.invocation().argument_zero(),
            "working_directory": captured.invocation().working_directory(),
            "raw_environment_names": captured.raw().environment().keys().collect::<Vec<_>>(),
            "source_restored_environment_names": captured.invocation().environment().keys().collect::<Vec<_>>(),
        })).unwrap()).unwrap();
    assert!(evidence.join("pending-command.json").is_file());
    assert!(!output.join("receipt.json").exists());
}

#[test]
#[ignore = "requires explicit retained real source/compiler case; driver-query ABI counter only"]
fn actual_source_preflight_rejects_changed_sdk_abi_or_source_root() {
    let case: serde_json::Value = serde_json::from_slice(
        &fs::read(std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").expect("select explicit case"))
            .unwrap(),
    )
    .unwrap();
    let source = PathBuf::from(case["source_root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let compiler = PathBuf::from(case["compiler_root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let config =
        aros_common::TargetProfile::load_config(&source.join("aros-targets.toml")).unwrap();
    let profile = config
        .targets
        .iter()
        .find(|profile| profile.name == case["source_preset"].as_str().unwrap())
        .unwrap();
    let selection =
        crate::compatibility::SourceMesaPreflightProfile::load(&source, profile).unwrap();
    let mut target = selection.target().clone();
    let SdkAbiValue::Boolean(mmu) = target.abi.get_mut("use_mmu").unwrap() else {
        panic!("selected MMU ABI must be boolean")
    };
    *mmu = !*mmu;
    assert_eq!(target.target_triple, selection.target().target_triple);
    assert_ne!(&target, selection.target());
    let manifest = aros_common::ArosToolchainManifest::load(&compiler).unwrap();
    let cancellation = CancellationToken::default();
    let drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: compiler.clone(),
            compiler: manifest.compiler_identity().unwrap(),
            target,
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    )
    .unwrap();
    let digest = measure_tree_content_cas(&source)
        .unwrap()
        .payload_digest_excluding(None);
    let environment = CompatibilityEnvironment::Poisoned {
        variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
    };
    let output = tempfile::tempdir().unwrap();
    let output = output.path().canonicalize().unwrap().join("must-not-exist");
    let result = execute_mesa_preflight(
        &MesaPreflightRequest {
            source_root: &source,
            source_tree_sha256: &digest,
            local_source: None,
            profile: (&selection).into(),
            genmodule_selection: None,
            drivers: &drivers,
            environment: &environment,
            output_root: &output,
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    );
    assert_preflight_error(result, "invalid or cancelled Mesa preflight selection");
    assert!(
        !output.exists(),
        "ABI disagreement must reject before output creation"
    );
    let matching = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: compiler,
            compiler: manifest.compiler_identity().unwrap(),
            target: selection.target().clone(),
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    )
    .unwrap();
    let other_source = tempfile::tempdir().unwrap();
    let other_source = other_source.path().canonicalize().unwrap();
    let result = execute_mesa_preflight(
        &MesaPreflightRequest {
            source_root: &other_source,
            source_tree_sha256: &digest,
            local_source: None,
            profile: (&selection).into(),
            genmodule_selection: None,
            drivers: &matching,
            environment: &environment,
            output_root: &output,
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    );
    assert_preflight_error(result, "invalid or cancelled Mesa preflight selection");
    assert!(
        !output.exists(),
        "source-root disagreement must reject before output creation"
    );
}

#[derive(Clone, Copy)]
enum ConfigureMode {
    Normal,
    RecipeFlags,
    InvalidLto,
    RecipeSysroot,
    Fail,
    Spin,
}

#[derive(Clone, Copy)]
enum FeatureMode {
    Complete,
    NonEmptySysroot,
    MissingFirstCompilerOption,
    MalformedFirstCompilerOption,
}

struct Fixture {
    temporary: TempDir,
    source_root: PathBuf,
    compiler_root: PathBuf,
    output_root: PathBuf,
    source_tree_sha256: Sha256Digest,
    profiles: Profiles,
    drivers: LlvmSdkDriverBinding,
    environment: CompatibilityEnvironment,
}

impl Fixture {
    fn new() -> Self {
        Self::configured(
            PROFILE_TRIPLE,
            "arm",
            "raspi",
            ConfigureMode::Normal,
            FeatureMode::Complete,
        )
    }

    fn configured(
        driver_triple: &str,
        configured_cpu: &str,
        configured_arch: &str,
        configure_mode: ConfigureMode,
        feature_mode: FeatureMode,
    ) -> Self {
        Self::configured_directory(
            driver_triple,
            configured_cpu,
            configured_arch,
            configure_mode,
            feature_mode,
            tempfile::tempdir().unwrap(),
        )
    }

    fn configured_directory(
        driver_triple: &str,
        configured_cpu: &str,
        configured_arch: &str,
        configure_mode: ConfigureMode,
        feature_mode: FeatureMode,
        temporary: TempDir,
    ) -> Self {
        let root = temporary.path().canonicalize().unwrap();
        let compiler_root = root.join("compiler");
        fs::create_dir_all(compiler_root.join("sysroot")).unwrap();
        let clang_path = compiler_root.join("bin/clang");
        let source_root = root.join("source");
        fs::create_dir_all(source_root.join("config")).unwrap();
        write_executable(
            &source_root.join("configure"),
            configure_script(
                &source_root,
                &clang_path,
                configured_cpu,
                configured_arch,
                configure_mode,
            )
            .as_bytes(),
        );
        write_executable(
            &source_root.join("config/features"),
            feature_script(configured_cpu, &clang_path, feature_mode).as_bytes(),
        );
        write_genmodule_fixture(&source_root);
        let source_root = source_root.canonicalize().unwrap();
        let source_tree_sha256 = measure_tree_content_cas(&source_root)
            .unwrap()
            .payload_digest_excluding(None);

        let bin = compiler_root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("clang"), driver_script(driver_triple).as_bytes());
        symlink("clang", bin.join("clang++")).unwrap();
        let compiler_root = compiler_root.canonicalize().unwrap();
        let drivers = inspect_llvm_sdk_drivers(
            &LlvmSdkDriverRequest {
                compiler_root: compiler_root.clone(),
                compiler: ArosCompilerIdentity::Llvm {
                    version: COMPILER_VERSION.into(),
                },
                target: sdk_target(driver_triple),
                timeout: Duration::from_secs(10),
            },
            &CancellationToken::default(),
        )
        .unwrap();

        let host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: root.join("host-tools"),
            tools: [
                ("bash", which("bash")),
                ("sh", PathBuf::from("/bin/sh")),
                ("make", gnu_make()),
                ("cc", which("cc")),
                ("c++", which("c++")),
                ("mkdir", which("mkdir")),
                ("chmod", which("chmod")),
            ]
            .into_iter()
            .map(|(name, program)| CompatibilityHostTool {
                name: name.into(),
                program,
            })
            .collect(),
        })
        .unwrap();
        let environment = CompatibilityEnvironment::SealedHostTools {
            variables: BTreeMap::from([
                ("PATH".into(), "/nonexistent".into()),
                ("AROS_FETCH_OFFLINE".into(), "1".into()),
            ]),
            host_tools,
        };
        let profiles = Profiles::parse(
            &serde_json::to_vec(&serde_json::json!({
                "schema": "aros-toolchain-profiles-v1",
                "upstream_commit": "a".repeat(40),
                "profiles": [{
                    "name": PROFILE_NAME,
                    "configure_target": PROFILE_NAME,
                    "upstream_output_target": PROFILE_NAME,
                    "target_triple": PROFILE_TRIPLE,
                    "cpu": "arm",
                    "platform": "raspi",
                    "float_abi": "hard",
                    "capabilities": ["c", "cxx"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        Self {
            temporary,
            source_root,
            compiler_root,
            output_root: root.join("output"),
            source_tree_sha256,
            profiles,
            drivers,
            environment,
        }
    }

    fn request(&self) -> MesaPreflightRequest<'_> {
        self.request_with_timeout(Duration::from_secs(10))
    }

    fn request_with_timeout(&self, timeout: Duration) -> MesaPreflightRequest<'_> {
        MesaPreflightRequest {
            source_root: &self.source_root,
            source_tree_sha256: &self.source_tree_sha256,
            local_source: None,
            profile: self.profiles.select(PROFILE_NAME).unwrap().into(),
            genmodule_selection: None,
            drivers: &self.drivers,
            environment: &self.environment,
            output_root: &self.output_root,
            timeout,
        }
    }
}

// Process-level fixture only: the opt-in real Source test exercises the
// original C compiler/Make producer. This script supplies literal output so
// deterministic tests can check lifecycle, binding and failed-stage refusal.
fn write_genmodule_fixture(source: &Path) {
    use aros_common::mesa_configuration_input::{
        MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_OVERRIDE_CONFIG,
    };
    use aros_common::mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES;
    let directory = source.join("tools/genmodule");
    fs::create_dir_all(&directory).unwrap();
    for (relative, bytes) in [
        (MESA_GENMODULE_BASE_CONFIG, "fixture base\n"),
        (MESA_GENMODULE_OVERRIDE_CONFIG, "fixture override\n"),
    ] {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let data = MESA_GENMODULE_MAKE_SUFFIXES
        .iter()
        .map(|suffix| shell_quote(&format!("mesa3dgl26-0_{suffix} +=")))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "#!/bin/sh\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -d ]; then shift; out=$1; fi\nshift\ndone\n[ -n \"$out\" ] || exit 42\nprintf '%s\\n' {data} > \"$out/Makefile.mesa3dgl26-0library\"\n"
    );
    let quoted_lines = script
        .lines()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
        .replace('$', "$$");
    fs::write(
        directory.join("Makefile"),
        format!(
            ".PHONY: all\nall: $(GENMODULE)\n$(GENMODULE):\n\t@printf '%s\\n' {quoted_lines} > \"$@\"\n\t@chmod 755 \"$@\"\n"
        ),
    )
    .unwrap();
    fs::write(directory.join("Makefile.deps"), "# fixture inventory\n").unwrap();
}

#[test]
fn binds_original_genmodule_output_before_preflight_publication() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
    let input_bytes = fs::read(fixture.output_root.join(INPUT_FILE)).unwrap();
    let document: MesaConfigurationInputDocument = serde_json::from_slice(&input_bytes).unwrap();
    assert_eq!(document.version, 3);
    let binding = document.genmodule.as_ref().unwrap();
    let raw_path = fixture
        .output_root
        .join(aros_common::mesa_configuration_input::MESA_GENMODULE_MAKE_FILE);
    assert_eq!(
        binding.makefile_sha256,
        sha256_bytes(&fs::read(&raw_path).unwrap())
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output_root.join("receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema"], "aros-mesa-preflight-v3");
    assert_eq!(receipt["stages"].as_array().unwrap().len(), 6);
    assert_eq!(
        receipt["stages"][5]["program"],
        binding.producer_sha256.as_str()
    );
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &fixture.output_root.join(INPUT_FILE),
        &sha256_bytes(&input_bytes),
    )
    .unwrap();
    assert_eq!(loaded.genmodule_binding(), Some(binding));
    assert_eq!(
        loaded.genmodule_makefile().unwrap().module(),
        "mesa3dgl26-0"
    );
    let (_, files) = completed.data_evidence(&request, &cancellation).unwrap();
    assert_eq!(files.len(), 17);
    assert!(files.contains_key("mesa-preflight-genmodule.make"));
    fs::write(&raw_path, "corrupted generated output\n").unwrap();
    assert!(completed.revalidate(&request, &cancellation).is_err());
    assert!(loaded.revalidate().is_err());
}

#[test]
fn failed_genmodule_build_cannot_publish_an_input_or_receipt() {
    let mut fixture = Fixture::new();
    fs::write(
        fixture.source_root.join("tools/genmodule/Makefile"),
        ".PHONY: all\nall:\n\t@exit 43\n",
    )
    .unwrap();
    fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let result = execute_mesa_preflight(&fixture.request(), &CancellationToken::default());
    assert_preflight_error(
        result,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(fixture.output_root.join("genmodule-build.stderr").is_file());
    assert!(!fixture.output_root.join("receipt.json").exists());
    assert!(!fixture.output_root.join(INPUT_FILE).exists());
}

#[test]
fn confines_unconditional_make_roots_and_remeasured_sysroots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let build = root.join("build");
    let source = root.join("source");
    let compiler = root.join("compiler");
    for path in [&build, &source, &compiler] {
        fs::create_dir(path).unwrap();
    }
    let top = format!("TOP := {}\n", build.display());
    let src = format!("SRCDIR := {}\n", source.display());
    let include = "include $(TOP)/config/make.cfg\n";
    validate_make_roots(&format!("{top}{src}{include}"), &build, &source).unwrap();
    assert!(validate_make_roots(
        &format!("ifeq (0,1)\n{top}endif\n{src}{include}"),
        &build,
        &source
    )
    .is_err());
    assert!(
        validate_make_roots(&format!("TOP := /wrong\n{src}{include}"), &build, &source).is_err()
    );
    assert!(validate_make_roots(&format!("{top}{top}{src}{include}"), &build, &source).is_err());
    assert!(validate_make_path(&root.join("$(shell touch escaped)")).is_err());
    validate_sysroot("", &source, &compiler).unwrap();
    validate_sysroot(
        &format!("--sysroot={}", compiler.display()),
        &source,
        &compiler,
    )
    .unwrap();
    assert!(
        validate_sysroot(&format!("--sysroot={}", root.display()), &source, &compiler).is_err()
    );
    assert!(validate_sysroot("--sysroot=/missing extra-flag", &source, &compiler).is_err());
}

#[test]
fn initial_value_snapshot_preserves_raw_data_and_excludes_environment_values() {
    use std::io::Write as _;
    use std::process::Stdio;
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let run = |prefix: &str| {
        let source = format!(
            "{prefix}\n$(info AROS_MAKE_INITIAL_NAMESPACE=v1:$(words $(sort $(.VARIABLES)));$(foreach __AROS_NAMESPACE_ITEM,$(sort $(.VARIABLES)),$(__AROS_NAMESPACE_ITEM)|$(origin $(__AROS_NAMESPACE_ITEM));)end)\n{}\n.PHONY: probe\nprobe:\n",
            initial_values_fragment()
        );
        let mut child = Command::new(make)
            .args(["-r", "-n", "-s", "--no-print-directory", "-f", "-", "probe"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("PRIVATE_INPUT", "environment-value-must-not-be-retained")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let output = run(
        "SIMPLE := literal $$cash%|;\nDELAYED = $(error must-not-expand)\ndefine MULTILINE\n leading\n\ttrailing \nendef\n",
    );
    assert!(
        output.status.success(),
        "synthetic raw Make observation failed"
    );
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    assert!(namespace.contains("PRIVATE_INPUT"));
    let values = ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
    let value = |name: &str| {
        values
            .entries()
            .find(|entry| entry.name == name)
            .unwrap()
            .value
            .as_str()
    };
    assert_eq!(value("SIMPLE"), "literal $cash%|;");
    assert_eq!(value("DELAYED"), "$(error must-not-expand)");
    assert_eq!(value("MULTILINE"), " leading\n\ttrailing ");
    assert!(!values.entries().any(|entry| entry.name == "PRIVATE_INPUT"));
    assert!(!String::from_utf8(output.stdout)
        .unwrap()
        .contains("environment-value-must-not-be-retained"));
    let configured =
        run("COORDINATE := configure\nFROZEN := $(COORDINATE)\nDEFERRED = $(COORDINATE)\n");
    let owner = run("COORDINATE := owner\nFROZEN := $(COORDINATE)\nDEFERRED = $(COORDINATE)\n");
    let capture = |output: &std::process::Output| {
        assert!(output.status.success());
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap()
    };
    let configured = capture(&configured);
    let owner = capture(&owner);
    let value = |values: &ValidatedMakeInitialValues, name: &str| {
        values
            .entries()
            .find(|entry| entry.name == name)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(value(&configured, "DEFERRED"), value(&owner, "DEFERRED"));
    assert_ne!(
        value(&configured, "FROZEN"),
        value(&owner, "FROZEN"),
        "a global simple-value snapshot must not be transplanted into another owner context"
    );
    for helper in ["__AROS_INITIAL_VALUES_ITEM", "__AROS_INITIAL_VALUES_NL"] {
        let output = run(&format!("{helper} := collision\n"));
        assert!(!output.status.success());
        assert!(!String::from_utf8(output.stdout)
            .unwrap()
            .contains("AROS_MAKE_INITIAL_VALUES="));
    }
}

#[test]
fn namespace_is_captured_after_features_before_probe_helpers_without_values() {
    let mut fixture = Fixture::new();
    let configure = fixture.source_root.join("configure");
    let original = fs::read_to_string(&configure).unwrap();
    fs::write(&configure, original + "\nprintf '%s\\n' 'GENERIC_EMPTY :=' 'GENERIC_VALUE := do-not-expand-this-value' >> config/make.cfg\n").unwrap();
    fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let completed =
        execute_mesa_preflight(&fixture.request(), &CancellationToken::default()).unwrap();
    let namespace = completed.configuration().initial_namespace().unwrap();
    assert!(namespace.contains("GENERIC_EMPTY"));
    assert!(namespace.contains("GENERIC_VALUE"));
    assert!(namespace.contains("CFLAGS_GNU11"));
    // Owner Make preserves default variables. The probe must not use -R and
    // accidentally turn an unmeasured built-in into proved absence.
    assert!(namespace.contains("CC"));
    assert!(namespace.contains("AR"));
    assert!(namespace.proves_absent("UNBOUND_GENERIC_NAME"));
    assert!(namespace.proves_absent("MESA_MEASURE_TOP"));
    assert!(namespace.proves_absent("__AROS_NAMESPACE_ITEM"));
    assert!(!completed
        .configuration()
        .make_variables()
        .contains_key(MAKE_NAMESPACE_METADATA_KEY));
    let stdout = fs::read(fixture.output_root.join("configured-values.stdout")).unwrap();
    let retained = ValidatedMakeNamespace::from_make_stdout(&stdout).unwrap();
    assert_eq!(&retained, namespace);
    let namespace_line = String::from_utf8(stdout)
        .unwrap()
        .lines()
        .find(|line| line.starts_with("AROS_MAKE_INITIAL_NAMESPACE="))
        .unwrap()
        .to_owned();
    assert!(!namespace_line.contains("do-not-expand-this-value"));
    let initial = completed.configuration().initial_values().unwrap();
    assert_eq!(
        initial
            .entries()
            .find(|entry| entry.name == "GENERIC_EMPTY")
            .unwrap()
            .value,
        ""
    );
    assert_eq!(
        initial
            .entries()
            .find(|entry| entry.name == "GENERIC_VALUE")
            .unwrap()
            .value,
        "do-not-expand-this-value"
    );
    completed
        .revalidate(&fixture.request(), &CancellationToken::default())
        .unwrap();
    completed
        .data_evidence(&fixture.request(), &CancellationToken::default())
        .unwrap();
}

#[test]
fn namespace_iterator_collision_refuses_capture_before_namespace_output() {
    let mut fixture = Fixture::new();
    let configure = fixture.source_root.join("configure");
    let original = fs::read_to_string(&configure).unwrap();
    fs::write(
        &configure,
        original + "\nprintf '%s\\n' '__AROS_NAMESPACE_ITEM :=' >> config/make.cfg\n",
    )
    .unwrap();
    fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    assert_preflight_error(
        execute_mesa_preflight(&fixture.request(), &CancellationToken::default()),
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(
        fs::read_to_string(fixture.output_root.join("configured-values.stderr"))
            .unwrap()
            .contains("Make namespace iterator collision")
    );
    assert!(
        !fs::read_to_string(fixture.output_root.join("configured-values.stdout"))
            .unwrap()
            .contains(MAKE_NAMESPACE_METADATA_KEY)
    );
    assert!(!fixture.output_root.join("mesa-configuration.json").exists());
}

#[test]
fn observes_undefined_source_local_variables_and_rejects_defined_empty_or_nonempty_globals() {
    let positive = Fixture::new();
    let result =
        execute_mesa_preflight(&positive.request(), &CancellationToken::default()).unwrap();
    assert_eq!(
        result.configuration().undefined_variables(),
        &["CUR_MESADIR"]
    );
    for value in ["", "source/view"] {
        let mut fixture = Fixture::new();
        let configure = fixture.source_root.join("configure");
        let original = fs::read_to_string(&configure).unwrap();
        fs::write(
            &configure,
            original + &format!("\nprintf '%s\\n' 'CUR_MESADIR := {value}' >> config/make.cfg\n"),
        )
        .unwrap();
        fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
            .unwrap()
            .payload_digest_excluding(None);
        assert_preflight_error(
            execute_mesa_preflight(&fixture.request(), &CancellationToken::default()),
            "Mesa preflight child failed; inspect retained stage logs",
        );
        let error =
            fs::read_to_string(fixture.output_root.join("configured-values.stderr")).unwrap();
        assert!(
            error.contains("CUR_MESADIR must be undefined before source evaluation"),
            "{error}"
        );
        assert!(!fixture.output_root.join("mesa-configuration.json").exists());
    }
}

#[test]
fn rejects_ambient_make_overrides_and_undefined_baseline_flags() {
    let mut ambient = Fixture::new();
    let CompatibilityEnvironment::SealedHostTools { variables, .. } = &mut ambient.environment
    else {
        unreachable!()
    };
    variables.insert("MAKEFLAGS".into(), "-f /unmeasured".into());
    assert_preflight_error(
        execute_mesa_preflight(&ambient.request(), &CancellationToken::default()),
        "Mesa preflight environment contains undeclared inputs or permits fetching",
    );
    assert!(!ambient.output_root.exists());
    let mut missing = Fixture::new();
    let configure = missing.source_root.join("configure");
    let previous = fs::read_to_string(&configure).unwrap();
    assert!(previous.contains("CONFIG_CFLAGS := -Dconfig-c"));
    fs::write(
        &configure,
        previous.replace(
            "CONFIG_CFLAGS := -Dconfig-c",
            "FIXTURE_UNUSED := -Dconfig-c",
        ),
    )
    .unwrap();
    missing.source_tree_sha256 = measure_tree_content_cas(&missing.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    assert_preflight_error(
        execute_mesa_preflight(&missing.request(), &CancellationToken::default()),
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(
        fs::read_to_string(missing.output_root.join("configured-values.stderr"))
            .unwrap()
            .contains("missing configured Mesa variable CONFIG_CFLAGS")
    );
}

#[test]
fn preflight_rejects_online_fetch_policy_before_creating_output() {
    let mut fixture = Fixture::new();
    let CompatibilityEnvironment::SealedHostTools { variables, .. } = &mut fixture.environment
    else {
        unreachable!()
    };
    variables.insert("AROS_FETCH_OFFLINE".into(), "0".into());
    assert_preflight_error(
        execute_mesa_preflight(&fixture.request(), &CancellationToken::default()),
        "Mesa preflight environment contains undeclared inputs or permits fetching",
    );
    assert!(!fixture.output_root.exists());
}

#[test]
fn measures_configuration_without_running_or_adopting_the_default_target() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
    assert_eq!(
        completed.configured_invocation.working_directory(),
        fixture.output_root.join("configure")
    );
    assert_eq!(
        completed.configured_invocation.arguments(),
        make_arguments(&fixture.output_root.join("configured-values.mk")).unwrap()
    );
    let mut exact_environment = environment::resolve(request.environment).unwrap();
    exact_environment
        .variables
        .insert("LC_ALL".into(), "C".into());
    assert_eq!(
        completed.configured_invocation.environment(),
        &exact_environment
    );

    assert_eq!(
        completed.configuration().identity().target_profile,
        PROFILE_NAME
    );
    assert_eq!(
        completed.configuration().identity().target_triple,
        PROFILE_TRIPLE
    );
    assert_eq!(
        completed
            .configuration()
            .target_isa_flags()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["-march=armv7", "-mfloat-abi=hard"]
    );
    assert_eq!(completed.make_values()["CONFIG_CFLAGS"], "-Dconfig-c");
    assert_eq!(
        completed.make_values()["MESA_COMPILEOPTIONS"],
        "-Dmesa-feature"
    );
    assert_eq!(completed.make_values()["CFLAGS_GNU11"], "-std=gnu11");
    assert_eq!(
        completed.make_values()["LLVM_NOWARN"],
        "-Wno-implicit-int",
        "recursive warnings must resolve after the generated feature include"
    );
    let transported = completed.configuration().make_variables();
    for name in MESA_BASELINE_VARIABLES {
        assert_eq!(
            transported.get(*name),
            completed.make_values().get(*name),
            "measured baseline lost at transport boundary: {name}"
        );
    }
    assert_eq!(
        completed.make_values()["CFLAGS_NO_BUILTIN_FFS"],
        "-fno-builtin-ffs"
    );
    assert_eq!(completed.make_values()["CFLAGS_NO_BUILTIN_FFSLL"], "");
    for key in MESA_COMPILER_OPTION_VARIABLES {
        assert!(completed.make_values().contains_key(*key), "missing {key}");
    }
    for (key, _) in BUILTINS {
        assert!(completed.make_values().contains_key(*key), "missing {key}");
    }

    let receipt = fs::read(fixture.output_root.join("receipt.json")).unwrap();
    assert_eq!(completed.receipt_sha256(), &sha256_bytes(&receipt));
    completed.revalidate(&request, &cancellation).unwrap();
    let arguments = completed
        .cmake_configuration_arguments(&request, &cancellation)
        .unwrap();
    let input_path = fixture.output_root.join(INPUT_FILE);
    let input_digest = sha256_bytes(&fs::read(&input_path).unwrap());
    assert_eq!(
        arguments,
        vec![
            "-DAROS_MESA_CONFIGURATION_MODE=bound".into(),
            format!("-DAROS_MESA_CONFIGURATION_INPUT={}", input_path.display()),
            format!("-DAROS_MESA_CONFIGURATION_INPUT_SHA256={input_digest}"),
        ]
    );
    let first = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &input_path,
        &input_digest,
    )
    .unwrap();
    assert_eq!(first.configuration(), completed.configuration());
    assert_eq!(first.receipt_sha256(), completed.receipt_sha256());
    assert_eq!(
        completed
            .cmake_configuration_arguments(&request, &cancellation)
            .unwrap(),
        arguments,
        "every pass must consume the same completed producer"
    );
    first.revalidate().unwrap();
    assert!(
        !fixture
            .output_root
            .join("configure/poison-target-ran")
            .exists(),
        "the ordinary Makefile target after the include boundary must not run"
    );

    let occupied = Fixture::new();
    fs::create_dir(&occupied.output_root).unwrap();
    fs::write(occupied.output_root.join("keep"), b"existing output").unwrap();
    let error = execute_mesa_preflight(&occupied.request(), &CancellationToken::default());
    assert_preflight_error(error, "Mesa preflight output must be fresh");
    assert_eq!(
        fs::read(occupied.output_root.join("keep")).unwrap(),
        b"existing output"
    );
}

#[test]
fn measures_recipe_flags_and_rejects_unknown_lto_without_publishing_configuration() {
    let fixture = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::RecipeFlags,
        FeatureMode::Complete,
    );
    let completed =
        execute_mesa_preflight(&fixture.request(), &CancellationToken::default()).unwrap();
    assert_eq!(
        completed.make_values()["SAFETY_CFLAGS"],
        "-DSAFETY=1 -DSAFETY=1"
    );
    assert_eq!(completed.make_values()["TARGET_LTO"], "yes");
    assert_eq!(completed.make_values()["LTO_CFLAGS"], "-flto");
    assert_eq!(
        completed.make_values(),
        &completed.configuration().make_variables()
    );
    let invalid = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::InvalidLto,
        FeatureMode::Complete,
    );
    assert_preflight_error(
        execute_mesa_preflight(&invalid.request(), &CancellationToken::default()),
        "invalid Mesa recipe observations",
    );
    assert!(!invalid.output_root.join("mesa-configuration.json").exists());
}

#[test]
fn nonempty_recipe_sysroot_and_defined_empty_safety_roundtrip_through_readback() {
    let fixture = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::RecipeSysroot,
        FeatureMode::NonEmptySysroot,
    );
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&fixture.request(), &cancellation).unwrap();
    let expected_sysroot = format!(
        "--sysroot={}",
        fixture.compiler_root.join("sysroot").display()
    );
    assert_eq!(
        completed
            .configuration()
            .recipe_configuration()
            .target_sysroot
            .as_slice(),
        std::slice::from_ref(&expected_sysroot)
    );
    assert_eq!(completed.make_values()["TARGET_SYSROOT"], expected_sysroot);
    assert_eq!(completed.make_values()["SAFETY_CFLAGS"], "");
    assert!(completed
        .configuration()
        .recipe_configuration()
        .undefined_variables()
        .is_empty());
    completed
        .data_evidence(&fixture.request(), &cancellation)
        .unwrap();
    let undefined = Fixture::new();
    let completed = execute_mesa_preflight(&undefined.request(), &cancellation).unwrap();
    assert!(!completed.make_values().contains_key("SAFETY_CFLAGS"));
    assert_eq!(
        completed
            .configuration()
            .recipe_configuration()
            .undefined_variables(),
        ["SAFETY_CFLAGS"]
    );
}

#[test]
fn generated_final_sysroot_is_bound_to_the_private_developer_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let build = temporary.path().canonicalize().unwrap();
    let developer = build.join("sdkroot");
    let other = build.join("other");
    fs::create_dir(&developer).unwrap();
    fs::create_dir(&other).unwrap();
    for prefix in ["--sysroot=", "--sysroot "] {
        assert!(super::validate_final_sysroot(
            &format!("{prefix}{}", developer.display()),
            "",
            developer.to_str().unwrap(),
            &build
        )
        .is_ok());
        assert!(super::validate_final_sysroot(
            &format!("{prefix}{}", other.display()),
            "",
            developer.to_str().unwrap(),
            &build
        )
        .is_err());
    }
    assert!(super::validate_final_sysroot("--sysroot=/", "", "/", &build).is_err());
}

#[test]
fn refuses_to_issue_transpiler_arguments_after_preflight_output_drift() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
    fs::write(fixture.output_root.join(INPUT_FILE), b"changed input").unwrap();
    let error = completed.cmake_configuration_arguments(&request, &cancellation);
    assert!(error.is_err());
}

#[test]
fn local_preflight_uses_the_exact_shared_generated_subtree() {
    let fixture = Fixture::new();
    for args in [
        vec!["init", "-q"],
        vec!["add", "configure", "config/features"],
        vec!["commit", "-qm", "Synthetic Mesa source"],
    ] {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Mesa fixture",
                "-c",
                "user.email=mesa@example.invalid",
                "-C",
            ])
            .arg(&fixture.source_root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(std::panic::catch_unwind(|| require_actual_build_namespace(
        &fixture.source_root,
        "fixture"
    ))
    .is_err());
    fs::create_dir_all(fixture.source_root.join("build/fixture")).unwrap();
    require_actual_build_namespace(&fixture.source_root, "fixture");
    let local =
        aros_common::local_source::LocalSourceIdentity::capture(&fixture.source_root, "fixture")
            .unwrap();
    let mut request = fixture.request_with_timeout(Duration::from_secs(30));
    request.source_tree_sha256 = &local.content_sha256;
    request.local_source = Some(&local);
    let cancellation = CancellationToken::default();
    let prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
    let configure = &prepared.configured.stages[0];
    assert_eq!(configure.name, "configure");
    assert!(configure
        .invocation_context
        .arguments()
        .contains(&"--with-mmake-ignore-directory=build/fixture".to_owned()));
    let completed = prepared.complete(&request, &cancellation).unwrap();
    let input: MesaConfigurationInputDocument =
        serde_json::from_slice(&fs::read(fixture.output_root.join(INPUT_FILE)).unwrap()).unwrap();
    assert_eq!(input.version, 4);
    assert_eq!(input.local_source.as_ref(), Some(&local));
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &fixture.output_root.join(INPUT_FILE),
        &completed.input_sha256,
    )
    .unwrap();
    assert_eq!(loaded.local_source(), Some(&local));
    fs::write(
        fixture.source_root.join("build/fixture/graph.cmake"),
        b"generated",
    )
    .unwrap();
    completed.revalidate(&request, &cancellation).unwrap();
    assert_eq!(
        completed
            .cmake_configuration_arguments(&request, &cancellation)
            .unwrap()
            .len(),
        3
    );
    // A local snapshot is not a complete-tree portable native proof.
    assert!(completed.data_evidence(&request, &cancellation).is_err());
    fs::create_dir_all(fixture.source_root.join("build/other")).unwrap();
    fs::write(
        fixture.source_root.join("build/other/input.c"),
        b"not excluded",
    )
    .unwrap();
    assert!(completed.revalidate(&request, &cancellation).is_err());
}

#[test]
fn rejects_missing_or_malformed_compiler_features_without_inferring_empty_values() {
    let missing = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::MissingFirstCompilerOption,
    );
    let error = execute_mesa_preflight(&missing.request(), &CancellationToken::default());
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(
        fs::read_to_string(missing.output_root.join("configured-values.stderr"))
            .unwrap()
            .contains("missing configured Mesa variable CFLAGS_GNU11")
    );
    let measured =
        fs::read_to_string(missing.output_root.join("configured-values.stdout")).unwrap();
    assert!(measured.lines().any(|line| line == "CFLAGS_GNU11="));

    let malformed = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::MalformedFirstCompilerOption,
    );
    let error = execute_mesa_preflight(&malformed.request(), &CancellationToken::default());
    assert_preflight_error(error, "source produced invalid Mesa configuration values");
}

#[test]
fn rejects_stale_source_and_compiler_before_creating_output() {
    let stale_source = Fixture::new();
    fs::write(
        stale_source.source_root.join("changed-after-measurement"),
        b"changed",
    )
    .unwrap();
    let error = execute_mesa_preflight(&stale_source.request(), &CancellationToken::default());
    assert_preflight_error(error, "Mesa preflight source changed");
    assert!(!stale_source.output_root.exists());

    let stale_compiler = Fixture::new();
    fs::write(
        stale_compiler.compiler_root.join("post-query-change"),
        b"changed",
    )
    .unwrap();
    let error = execute_mesa_preflight(&stale_compiler.request(), &CancellationToken::default());
    assert_preflight_error(error, "LLVM compiler tree changed after driver inspection");
    assert!(!stale_compiler.output_root.exists());
}

#[test]
fn retains_exact_non_utf8_success_logs_bound_by_the_receipt() {
    let mut fixture = Fixture::new();
    let configure = fixture.source_root.join("configure");
    let mut script = fs::read_to_string(&configure).unwrap();
    script.push_str("printf '\\377configured\\376'\nprintf '\\375warning\\374' >&2\n");
    fs::write(&configure, script).unwrap();
    fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
    let stdout = fs::read(fixture.output_root.join("configure.stdout")).unwrap();
    let stderr = fs::read(fixture.output_root.join("configure.stderr")).unwrap();
    assert_eq!(stdout, b"\xffconfigured\xfe");
    assert_eq!(stderr, b"\xfdwarning\xfc");
    assert!(std::str::from_utf8(&stdout).is_err());
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output_root.join("receipt.json")).unwrap())
            .unwrap();
    assert_eq!(
        receipt["stages"][0]["stdout"],
        sha256_bytes(&stdout).as_str()
    );
    assert_eq!(
        receipt["stages"][0]["stderr"],
        sha256_bytes(&stderr).as_str()
    );
    assert_ne!(
        sha256_bytes(String::from_utf8_lossy(&stdout).as_bytes()),
        sha256_bytes(&stdout)
    );
    completed.revalidate(&request, &cancellation).unwrap();
}

#[test]
fn rejects_driver_and_configured_target_disagreement() {
    let driver_mismatch = Fixture::configured(
        "arm-other-aros",
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(&driver_mismatch.request(), &CancellationToken::default());
    assert_preflight_error(error, "invalid or cancelled Mesa preflight selection");
    assert!(!driver_mismatch.output_root.exists());

    let configured_mismatch = Fixture::configured(
        PROFILE_TRIPLE,
        "aarch64",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(
        &configured_mismatch.request(),
        &CancellationToken::default(),
    );
    assert_preflight_error(
        error,
        "configured Mesa CPU/platform differs from selected profile",
    );
    assert!(configured_mismatch.output_root.exists());
}

#[test]
fn retains_failed_child_logs_and_detects_tampered_receipt() {
    let failed = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Fail,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(&failed.request(), &CancellationToken::default());
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert_eq!(
        fs::read(failed.output_root.join("configure.stderr")).unwrap(),
        b"fixture configure failed\n"
    );
    assert!(failed.output_root.join("configure.stdout").exists());

    let successful = Fixture::new();
    let request = successful.request();
    let completed = execute_mesa_preflight(&request, &CancellationToken::default()).unwrap();
    completed
        .revalidate(&request, &CancellationToken::default())
        .unwrap();
    fs::write(
        successful.output_root.join("receipt.json"),
        b"tampered receipt",
    )
    .unwrap();
    assert_preflight_error(
        completed.revalidate(&request, &CancellationToken::default()),
        "Mesa preflight output changed",
    );
}

#[test]
fn enforces_the_total_deadline_and_cancels_a_running_child() {
    let expired = Fixture::new();
    let error = execute_mesa_preflight(
        &expired.request_with_timeout(Duration::from_nanos(1)),
        &CancellationToken::default(),
    );
    assert_preflight_error(error, "Mesa preflight deadline expired");
    assert!(expired.output_root.exists());
    assert!(!expired.output_root.join("configure.stdout").exists());

    let running = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Spin,
        FeatureMode::Complete,
    );
    let cancellation = CancellationToken::default();
    let trigger = cancellation.clone();
    let started = running.output_root.join("configure/started");
    let canceller = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        trigger.cancel();
    });
    let error = execute_mesa_preflight(
        &running.request_with_timeout(Duration::from_secs(10)),
        &cancellation,
    );
    canceller.join().unwrap();
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(running.output_root.join("configure.stdout").exists());
    assert!(running.output_root.join("configure.stderr").exists());
}

fn configure_script(
    source_root: &Path,
    clang_path: &Path,
    configured_cpu: &str,
    configured_arch: &str,
    mode: ConfigureMode,
) -> String {
    match mode {
        ConfigureMode::Fail => {
            return "#!/bin/bash\nprintf 'fixture configure failed\\n' >&2\nexit 23\n".into();
        }
        ConfigureMode::Spin => {
            return "#!/bin/bash\nprintf started > started\nwhile :; do :; done\n".into();
        }
        ConfigureMode::Normal
        | ConfigureMode::RecipeFlags
        | ConfigureMode::InvalidLto
        | ConfigureMode::RecipeSysroot => {}
    }

    let mut script = String::from(
        "#!/bin/bash\nset -eu\n[ \"$1\" = \"--target=arm-raspi\" ] || exit 91\n\
         mkdir -p config gen/config\n",
    );
    writeln!(script,
        r#"printf 'default: all\nTOP := %s\nSRCDIR := %s\ninclude $(TOP)/config/make.cfg\nall:\n\t@: > poison-target-ran\n' "$PWD" {} > Makefile"#,
        shell_quote(&source_root.to_string_lossy())
    ).unwrap();
    for line in make_configuration_lines(configured_cpu, configured_arch, clang_path) {
        writeln!(
            script,
            "printf '%s\\n' {} >> config/make.cfg",
            shell_quote(&line)
        )
        .unwrap();
    }
    match mode {
        ConfigureMode::RecipeFlags => script.push_str("printf '%s\\n' 'SAFETY_CFLAGS := -DSAFETY=1 -DSAFETY=1' 'TARGET_LTO := yes' >> config/make.cfg\n"),
        ConfigureMode::InvalidLto => script.push_str("printf '%s\\n' 'TARGET_LTO := guessed' >> config/make.cfg\n"),
        ConfigureMode::RecipeSysroot => {
            let sysroot = clang_path.parent().unwrap().parent().unwrap().join("sysroot");
            writeln!(script, "printf '%s\\n' {} 'AROS_DEVELOPER := $(TOP)/sdkroot' 'PORTSDIR := $(TOP)/Ports' 'SAFETY_CFLAGS :=' >> config/make.cfg\nmkdir -p sdkroot Ports", shell_quote(&format!("TARGET_SYSROOT := --sysroot={}", sysroot.display()))).unwrap();
        }
        _ => {}
    }
    script
}

fn make_configuration_lines(
    configured_cpu: &str,
    configured_arch: &str,
    clang_path: &Path,
) -> Vec<String> {
    let clang = clang_path.to_string_lossy();
    let clangxx = clang_path
        .with_file_name("clang++")
        .to_string_lossy()
        .into_owned();
    vec![
        format!("AROS_TARGET_CPU := {configured_cpu}"),
        format!("AROS_TARGET_ARCH := {configured_arch}"),
        format!("TARGET_CC := {clang}"),
        "TARGET_SYSROOT :=".into(),
        "TARGET_CFLAGS := -target-c".into(),
        "DEBUG_CFLAGS := -g".into(),
        "OPTIMIZATION_CFLAGS := -O2".into(),
        format!("TARGET_CPP := {clang} -E"),
        format!("TARGET_CXX := {clangxx}"),
        format!("AROS_CXX := {clangxx}"),
        "TARGET_LTO := no".into(),
        "LTO_CFLAGS := -flto".into(),
        "CFLAGS_IQUOTE := -iquote".into(),
        "CFLAGS_IQUOTE_END :=".into(),
        "TARGET_CXXFLAGS := -target-cxx".into(),
        "AROS_DEVELOPER := $(TOP)/Developer".into(),
        "AROS_BUILD_HOST := x86_64-unknown-linux-gnu".into(),
        "GENDIR := $(TOP)/gen".into(),
        "PORTSDIR := $(TOP)/Ports".into(),
        "TARGET_ISA_CFLAGS := -march=armv7 -mfloat-abi=hard".into(),
        "TARGET_ISA_CXXFLAGS := -march=armv7 -mfloat-abi=hard".into(),
        "CONFIG_CFLAGS := -Dconfig-c".into(),
        "CONFIG_CXXFLAGS := -Dconfig-cxx".into(),
        "CONFIG_CPPFLAGS := -Dconfig-cpp".into(),
        "TARGET_CPPFLAGS := -target-cpp".into(),
        "DEBUG_CXXFLAGS := -g".into(),
        "LLVM_NOWARN = $(NOWARN_IMPLICIT_INT)".into(),
        "MESA_COMPILEOPTIONS := -Dmesa-feature".into(),
    ]
}

fn feature_script(configured_cpu: &str, clang_path: &Path, mode: FeatureMode) -> String {
    let clang = clang_path.to_string_lossy();
    let clangxx = clang_path
        .with_file_name("clang++")
        .to_string_lossy()
        .into_owned();
    let sysroot = if matches!(mode, FeatureMode::NonEmptySysroot) {
        format!(
            "--sysroot={}",
            clang_path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("sysroot")
                .display()
        )
    } else {
        String::new()
    };
    let developer_check = if matches!(mode, FeatureMode::NonEmptySysroot) {
        "case \"$AROS_DEVELOPER\" in */configure/sdkroot) ;; *) exit 38 ;; esac"
    } else {
        "[ \"$AROS_DEVELOPER\" = \"${PWD%/config}/Developer\" ] || exit 38"
    };
    let mut script = format!(
        "#!/bin/bash\nset -eu\n\
         [ \"$1\" = \"--host=x86_64-unknown-linux-gnu\" ] || exit 30\n\
         [ \"$AROS_TARGET_CPU\" = {configured_cpu} ] || exit 31\n\
         [ \"$CC\" = \"{clang} {sysroot}\" ] || exit 32\n\
         [ \"$CONFIG_STATUS\" = features.status ] || exit 33\n\
         [ \"$CPP\" = \"{clang} -E {sysroot}\" ] || exit 35\n\
         [ \"$CXX\" = \"{clangxx} {sysroot}\" ] || exit 36\n\
         [ \"$CFLAGS\" = \"-target-c -g -O2\" ] || exit 37\n\
         {developer_check}\n\
         case \"$2\" in --with-config-dir=*) config_dir=${{2#*=}} ;; *) exit 34 ;; esac\n\
         mkdir -p \"$config_dir\"\n\
         : > \"$config_dir/compiler.cfg\"\n"
    );
    for (index, name) in MESA_COMPILER_OPTION_VARIABLES.iter().enumerate() {
        if index == 0 && matches!(mode, FeatureMode::MissingFirstCompilerOption) {
            continue;
        }
        let value = if index == 0 {
            match mode {
                FeatureMode::MalformedFirstCompilerOption => "@malformed".into(),
                FeatureMode::Complete
                | FeatureMode::NonEmptySysroot
                | FeatureMode::MissingFirstCompilerOption => "-std=gnu11".into(),
            }
        } else if index == 1 {
            "-std=c++17".into()
        } else {
            String::new()
        };
        script.push_str(&assignment_command(name, &value));
    }
    for (index, (name, value)) in BUILTINS.iter().enumerate() {
        let value = if index % 2 == 0 { *value } else { "" };
        script.push_str(&assignment_command(name, value));
    }
    script.push_str(&assignment_command(
        "NOWARN_IMPLICIT_INT",
        "-Wno-implicit-int",
    ));
    script
}

fn assignment_command(name: &str, value: &str) -> String {
    format!(
        "printf '%s\\n' {} >> \"$config_dir/compiler.cfg\"\n",
        shell_quote(&format!("{name} := {value}"))
    )
}

fn driver_script(triple: &str) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then\n\
           printf 'clang version {COMPILER_VERSION}\\n'\n\
         elif [ \"$1\" = \"--target={triple}\" ] && [ \"$2\" = \"-dumpmachine\" ]; then\n\
           printf '%s\\n' '{triple}'\n\
         else\n\
           exit 12\n\
         fi\n"
    )
}

fn sdk_target(triple: &str) -> SdkTarget {
    SdkTarget {
        target_triple: triple.into(),
        abi: BTreeMap::from([(
            "source_declared_isa".into(),
            SdkAbiValue::Text("fixture-isa".into()),
        )]),
    }
}

fn which(name: &str) -> PathBuf {
    let output = Command::new("which")
        .arg(name)
        .output()
        .unwrap_or_else(|error| panic!("cannot run which for {name}: {error}"));
    assert!(
        output.status.success(),
        "required host tool {name} is unavailable"
    );
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        .canonicalize()
        .unwrap()
}

fn gnu_make() -> PathBuf {
    for name in ["gmake", "make"] {
        let output = Command::new("which").arg(name).output().unwrap();
        if !output.status.success() {
            continue;
        }
        let path = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let version = Command::new(&path).arg("--version").output().unwrap();
        if version.status.success()
            && String::from_utf8_lossy(&version.stdout)
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("GNU Make "))
        {
            return path;
        }
    }
    panic!("tests require GNU Make, found through which gmake or which make");
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn assert_preflight_error<T>(result: Result<T, ContractError>, expected_message: &str) {
    let Err(error) = result else {
        panic!("fixture must be rejected with {expected_message}")
    };
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(diagnostics.diagnostics[0].message, expected_message);
}

const SOURCE_GENMODULE_PRESET: &str = "source-arm";
const SOURCE_GENMODULE_CONTRACT: &str = "contracts/mesa-sdk.json";
const SOURCE_GENMODULE_OWNERS: [&str; 2] = ["mesa3dgl-owner-a", "mesa3dgl-owner-b"];

struct SyntheticGenmoduleSelection {
    source_profile: crate::compatibility::SourceMesaPreflightProfile,
    scope: aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    helper: PathBuf,
    helper_sha256: Sha256Digest,
}

impl Fixture {
    fn source_genmodule_selection(
        &mut self,
        scope: aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    ) -> SyntheticGenmoduleSelection {
        use aros_common::mesa_genmodule_inventory::{
            MesaGenmoduleInventory, MesaGenmoduleInventoryRecord, MesaGenmoduleSelectionDocument,
            MESA_GENMODULE_INVENTORY_SCHEMA, MESA_GENMODULE_SELECTION_SCHEMA,
        };

        assert_eq!(
            scope,
            aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget
        );
        let source_profile_text = format!(
            "[[targets]]\nname = '{SOURCE_GENMODULE_PRESET}'\ntoolchain_profile = '{PROFILE_NAME}'\narch = 'arm'\nplatform = 'raspi'\nbsp = 'generic'\nfloat_abi = 'hard'\nsdk_consumer_contract = '{SOURCE_GENMODULE_CONTRACT}'\n[targets.transpiler]\nfamily = 'arm-native'\nvariant = 'raspi'\ntoolchain = 'llvm'\ncpu32 = 'i386'\nuse_mmu = true\nmesa_version = '26.0.0'\n"
        );
        let profile_bytes = source_profile_text.as_bytes();
        fs::write(self.source_root.join("aros-targets.toml"), profile_bytes).unwrap();
        let policy = b"{}\n";
        let c_probe = b"int mesa_preflight_c_probe(void) { return 0; }\n";
        let cxx_probe = b"int mesa_preflight_cxx_probe() { return 0; }\n";
        fs::create_dir_all(self.source_root.join("contracts")).unwrap();
        fs::write(self.source_root.join("policy.json"), policy).unwrap();
        fs::write(self.source_root.join("probe.c"), c_probe).unwrap();
        fs::write(self.source_root.join("probe.cpp"), cxx_probe).unwrap();

        let target = SdkTarget {
            target_triple: PROFILE_TRIPLE.into(),
            abi: BTreeMap::from([
                ("source_cpu".into(), SdkAbiValue::Text("arm".into())),
                ("pointer_width".into(), SdkAbiValue::Unsigned(32)),
                ("float_abi".into(), SdkAbiValue::Text("hard".into())),
                ("use_mmu".into(), SdkAbiValue::Boolean(true)),
            ]),
        };
        let contract = serde_json::json!({
            "schema": "aros-sdk-consumer-contract-v1",
            "profile": SOURCE_GENMODULE_PRESET,
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes", "linklibs"],
            "inputs": [
                {"path": "aros-targets.toml", "sha256": sha256_bytes(profile_bytes)},
                {"path": "policy.json", "sha256": sha256_bytes(policy)},
                {"path": "probe.c", "sha256": sha256_bytes(c_probe)},
                {"path": "probe.cpp", "sha256": sha256_bytes(cxx_probe)}
            ],
            "target": target,
            "mesa_configure_target": "arm-raspi",
            "metamake_projection": "policy.json",
            "native_sdk_link_probes": {
                "c": {"source": "probe.c", "libraries": []},
                "cxx": {"source": "probe.cpp", "libraries": []}
            }
        });
        fs::write(
            self.source_root.join(SOURCE_GENMODULE_CONTRACT),
            serde_json::to_vec(&contract).unwrap(),
        )
        .unwrap();

        let selected =
            aros_common::TargetProfile::load_config(&self.source_root.join("aros-targets.toml"))
                .unwrap()
                .targets
                .into_iter()
                .find(|profile| profile.name == SOURCE_GENMODULE_PRESET)
                .unwrap();
        let source_profile =
            crate::compatibility::SourceMesaPreflightProfile::load(&self.source_root, &selected)
                .unwrap();
        assert_ne!(SOURCE_GENMODULE_PRESET, PROFILE_NAME);

        let inventory = MesaGenmoduleInventory {
            schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
            records: SOURCE_GENMODULE_OWNERS
                .iter()
                .map(|owner| MesaGenmoduleInventoryRecord {
                    owner: (*owner).into(),
                    declaration_directory: "workbench/libs/mesa".into(),
                    module: "mesa3dgl26-0".into(),
                    module_type: "library".into(),
                    base_configuration:
                        aros_common::mesa_configuration_input::MESA_GENMODULE_BASE_CONFIG.into(),
                    override_configuration: Some(
                        aros_common::mesa_configuration_input::MESA_GENMODULE_OVERRIDE_CONFIG
                            .into(),
                    ),
                    consumer_path: format!("runtime/{owner}"),
                    components: Vec::new(),
                })
                .collect(),
        };
        let document = MesaGenmoduleSelectionDocument {
            schema: MESA_GENMODULE_SELECTION_SCHEMA.into(),
            selection: source_profile.genmodule_inventory_binding(scope),
            inventory,
        };
        let document = serde_json::to_string(&document).unwrap();
        let binding = source_profile.genmodule_inventory_binding(scope);
        let helper = self
            .source_root
            .parent()
            .unwrap()
            .join("selected-inventory-helper");
        let script = format!(
            "#!/bin/sh\nset -eu\noutput=\nprofile_seen=0\ncontract_seen=0\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --mesa-genmodule-full-inventory-profile)\n      shift\n      [ \"$1\" = {} ] || exit 41\n      profile_seen=1\n      ;;\n    --mesa-genmodule-full-contract-sha256)\n      shift\n      [ \"$1\" = {} ] || exit 42\n      contract_seen=1\n      ;;\n    --output)\n      shift\n      output=$1\n      ;;\n  esac\n  shift\ndone\n[ \"$profile_seen\" = 1 ] || exit 43\n[ \"$contract_seen\" = 1 ] || exit 44\n[ -n \"$output\" ] || exit 45\nprintf '%s\\n' {} > \"$output\"\n",
            shell_quote(SOURCE_GENMODULE_PRESET),
            shell_quote(&binding.contract_sha256.to_string()),
            shell_quote(&document),
        );
        write_executable(&helper, script.as_bytes());
        let helper_sha256 = sha256_bytes(&fs::read(&helper).unwrap());

        self.source_tree_sha256 = measure_tree_content_cas(&self.source_root)
            .unwrap()
            .payload_digest_excluding(None);
        SyntheticGenmoduleSelection {
            source_profile,
            scope,
            helper,
            helper_sha256,
        }
    }

    fn source_genmodule_drivers(
        &self,
        selection: &SyntheticGenmoduleSelection,
    ) -> LlvmSdkDriverBinding {
        inspect_llvm_sdk_drivers(
            &LlvmSdkDriverRequest {
                compiler_root: self.compiler_root.clone(),
                compiler: ArosCompilerIdentity::Llvm {
                    version: COMPILER_VERSION.into(),
                },
                target: selection.source_profile.target().clone(),
                timeout: Duration::from_secs(10),
            },
            &CancellationToken::default(),
        )
        .unwrap()
    }

    fn source_genmodule_request<'a>(
        &'a self,
        selection: &'a SyntheticGenmoduleSelection,
        drivers: &'a LlvmSdkDriverBinding,
    ) -> MesaPreflightRequest<'a> {
        MesaPreflightRequest {
            source_root: &self.source_root,
            source_tree_sha256: &self.source_tree_sha256,
            local_source: None,
            profile: (&selection.source_profile).into(),
            genmodule_selection: Some(crate::compatibility::MesaGenmodulePreflightSelection {
                source_profile: &selection.source_profile,
                scope: selection.scope,
                transpiler: &selection.helper,
                transpiler_sha256: &selection.helper_sha256,
            }),
            drivers,
            environment: &self.environment,
            output_root: &self.output_root,
            timeout: Duration::from_secs(30),
        }
    }
}

fn source_collection_fixture() -> (Fixture, SyntheticGenmoduleSelection, LlvmSdkDriverBinding) {
    let mut fixture = Fixture::new();
    let selection = fixture.source_genmodule_selection(
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget,
    );
    let drivers = fixture.source_genmodule_drivers(&selection);
    (fixture, selection, drivers)
}

#[test]
fn prepared_local_source_collection_completes_as_v8_not_portable_or_legacy_scope() {
    let (fixture, selection, drivers) = source_collection_fixture();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec!["commit", "-qm", "Synthetic Mesa collection fixture"],
    ] {
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=Mesa fixture",
                "-c",
                "user.email=mesa@example.invalid",
                "-C"
            ])
            .arg(&fixture.source_root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    fs::create_dir_all(fixture.source_root.join("build/fixture")).unwrap();
    let local =
        aros_common::local_source::LocalSourceIdentity::capture(&fixture.source_root, "fixture")
            .unwrap();
    let mut request = fixture.source_genmodule_request(&selection, &drivers);
    request.source_tree_sha256 = &local.content_sha256;
    request.local_source = Some(&local);
    let cancellation = CancellationToken::default();
    let prepared = prepare_mesa_preflight(&request, &cancellation).unwrap();
    assert!(!fixture.output_root.join(INPUT_FILE).exists());
    let completed = prepared.complete(&request, &cancellation).unwrap();
    let input_path = fixture.output_root.join(INPUT_FILE);
    let bytes = fs::read(&input_path).unwrap();
    let mut input: MesaConfigurationInputDocument = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(input.version, 8);
    assert_eq!(input.local_source.as_ref(), Some(&local));
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &input_path,
        &sha256_bytes(&bytes),
    )
    .unwrap();
    assert_eq!(loaded.initial_scopes().len(), 2);
    assert_eq!(loaded.genmodule_collection().unwrap().outputs().len(), 2);
    loaded.revalidate().unwrap();
    completed.revalidate(&request, &cancellation).unwrap();
    assert!(completed.data_evidence(&request, &cancellation).is_err());
    // Downgrading this otherwise unchanged data must not discard its required
    // local-source semantics or become a v7 portable scope collection.
    input.version = 7;
    let downgraded = serde_json::to_vec(&input).unwrap();
    fs::write(&input_path, &downgraded).unwrap();
    let error = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &input_path,
        &sha256_bytes(&downgraded),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("unsupported Mesa configuration input version 7"));
}

#[test]
fn source_owned_full_inventory_preflight_collects_two_same_module_producers() {
    let (fixture, selection, drivers) = source_collection_fixture();
    let request = fixture.source_genmodule_request(&selection, &drivers);
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
    assert_eq!(
        completed.configuration().identity().target_profile,
        PROFILE_NAME
    );

    let input_path = fixture.output_root.join(INPUT_FILE);
    let input_bytes = fs::read(&input_path).unwrap();
    let input: aros_common::mesa_configuration_input::MesaConfigurationInputDocument =
        serde_json::from_slice(&input_bytes).unwrap();
    assert_eq!(input.version, 7);
    let loaded = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &input_path,
        &sha256_bytes(&input_bytes),
    )
    .unwrap();
    let collection = loaded.genmodule_collection().unwrap();
    assert_eq!(collection.outputs().len(), 2);
    assert_eq!(loaded.initial_scopes().len(), 2);
    for owner in SOURCE_GENMODULE_OWNERS {
        let scope = &loaded.initial_scopes()[owner];
        assert_eq!(scope.context().owner, owner);
        assert_eq!(scope.context().inputs["TARGET"], owner);
        assert_eq!(
            scope.namespace().origin("CURDIR"),
            Some(aros_common::make_namespace::MakeVariableOrigin::CommandLine)
        );
    }
    assert_eq!(
        collection
            .outputs()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        SOURCE_GENMODULE_OWNERS.to_vec()
    );
    assert_eq!(
        collection.selection().binding().profile,
        SOURCE_GENMODULE_PRESET
    );
    assert_eq!(
        collection.selection().binding().scope,
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget
    );
    loaded.revalidate().unwrap();

    let (proof, files) = completed.data_evidence(&request, &cancellation).unwrap();
    assert_eq!(proof.receipt_sha256(), completed.receipt_sha256());
    assert_eq!(files.len(), 27);
    assert!(files.contains_key("mesa-preflight-genmodule-selection.json"));
    for owner in SOURCE_GENMODULE_OWNERS {
        assert!(files.contains_key(&format!("mesa-preflight-genmodule-{owner}.make")));
    }
    completed.revalidate(&request, &cancellation).unwrap();

    // Rebind all surrounding digests after swapping two valid scope outputs.
    // The independently selected context, not just a stale checksum, must
    // reject the otherwise well-formed other owner's observation.
    let first = format!(
        "mesa-preflight-genmodule-initial-{}.stdout",
        SOURCE_GENMODULE_OWNERS[0]
    );
    let second = format!(
        "mesa-preflight-genmodule-initial-{}.stdout",
        SOURCE_GENMODULE_OWNERS[1]
    );
    let mut swapped = files.clone();
    swapped.insert(first.clone(), files[&second].clone());
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&swapped["mesa-preflight-receipt.json"]).unwrap();
    receipt["stages"][6]["stdout"] = serde_json::to_value(sha256_bytes(&swapped[&first])).unwrap();
    let invocation = crate::canonical::bytes(&serde_json::json!({
        "stages": receipt["stages"],
        "profile": request.profile.document_sha256(),
    }))
    .unwrap();
    receipt["identity"]["configure_invocation_sha256"] =
        serde_json::to_value(sha256_bytes(&invocation)).unwrap();
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&swapped["mesa-preflight-configuration.json"]).unwrap();
    configuration["identity"] = receipt["identity"].clone();
    let configuration_bytes = crate::canonical::bytes(&configuration).unwrap();
    receipt["configuration_sha256"] =
        serde_json::to_value(sha256_bytes(&configuration_bytes)).unwrap();
    swapped.insert(
        "mesa-preflight-configuration.json".into(),
        configuration_bytes.clone(),
    );
    let receipt_bytes = crate::canonical::bytes(&receipt).unwrap();
    let mut input: serde_json::Value =
        serde_json::from_slice(&swapped["mesa-preflight-input.json"]).unwrap();
    input["identity"] = receipt["identity"].clone();
    input["configuration_sha256"] = receipt["configuration_sha256"].clone();
    input["receipt_sha256"] = serde_json::to_value(sha256_bytes(&receipt_bytes)).unwrap();
    swapped.insert("mesa-preflight-receipt.json".into(), receipt_bytes.clone());
    swapped.insert(
        "mesa-preflight-input.json".into(),
        crate::canonical::bytes(&input).unwrap(),
    );
    let expected = super::super::mesa_preflight_evidence::MesaPreflightEvidenceExpectations {
        profile_name: request.profile.name(),
        target_triple: request.profile.target_triple(),
        profile_document_sha256: request.profile.document_sha256(),
        compiler: request.drivers.compiler(),
        compiler_tree_sha256: request.drivers.compiler_tree_sha256(),
        compiler_root: Some(request.drivers.compiler_root()),
        source_tree_sha256: request.source_tree_sha256,
        cpu: request.profile.cpu(),
        platform: request.profile.platform(),
        clang_path: request.drivers.clang_path(),
        clangxx_path: request.drivers.clangxx_path(),
        output_root: request.output_root,
        genmodule_selection: Some(
            selection
                .source_profile
                .genmodule_inventory_binding(selection.scope),
        ),
        genmodule_helper_sha256: Some(selection.helper_sha256.clone()),
    };
    super::super::mesa_preflight_evidence::readback(&files, &expected).unwrap();
    let error = super::super::mesa_preflight_evidence::readback(&swapped, &expected).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("owner scope differs from its selected context"),
        "{error}"
    );

    // Exercise the filesystem reader independently with the same rebound copy.
    fs::write(
        fixture
            .output_root
            .join(first.strip_prefix("mesa-preflight-").unwrap()),
        &swapped[&first],
    )
    .unwrap();
    fs::write(fixture.output_root.join("receipt.json"), receipt_bytes).unwrap();
    fs::write(
        fixture.output_root.join("mesa-configuration.json"),
        configuration_bytes,
    )
    .unwrap();
    let input_bytes = &swapped["mesa-preflight-input.json"];
    fs::write(&input_path, input_bytes).unwrap();
    let error = aros_common::mesa_configuration_input::load_mesa_configuration_input(
        &input_path,
        &sha256_bytes(input_bytes),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("scope context digest does not match"));
}

#[test]
fn changed_selected_inventory_helper_refuses_before_creating_output() {
    let mut fixture = Fixture::new();
    let selection = fixture.source_genmodule_selection(
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget,
    );
    let drivers = fixture.source_genmodule_drivers(&selection);
    let mut changed = fs::read(&selection.helper).unwrap();
    changed.extend_from_slice(b"# changed after measurement\n");
    write_executable(&selection.helper, &changed);
    let request = fixture.source_genmodule_request(&selection, &drivers);
    assert_preflight_error(
        execute_mesa_preflight(&request, &CancellationToken::default()),
        "selected genmodule inventory helper changed",
    );
    assert!(!fixture.output_root.exists());
}

#[test]
fn source_call_transport_cannot_be_replaced_by_root_prefix_defaults() {
    let (mut fixture, selection, drivers) = source_collection_fixture();
    let configure = fixture.source_root.join("configure");
    let mut script = fs::read_to_string(&configure).unwrap();
    script.push_str(
        "printf '%s\\n' 'CALL := env CUSTOM_NATIVE_INPUT=from-source' >> config/make.cfg\n",
    );
    fs::write(&configure, script).unwrap();
    fixture.source_tree_sha256 = measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let request = fixture.source_genmodule_request(&selection, &drivers);
    assert_preflight_error(
        execute_mesa_preflight(&request, &CancellationToken::default()),
        "source-defined MetaMake environment transport is not yet modeled",
    );
    assert!(!fixture.output_root.join(INPUT_FILE).exists());
    assert!(!fixture
        .output_root
        .join("genmodule-inventory.stdout")
        .exists());
}

#[test]
fn completed_source_inventory_refuses_scope_reselection() {
    let (fixture, mut selection, drivers) = source_collection_fixture();
    let request = fixture.source_genmodule_request(&selection, &drivers);
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();

    selection.scope =
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::ContractRoots;
    let changed_request = fixture.source_genmodule_request(&selection, &drivers);
    assert!(completed
        .revalidate(&changed_request, &cancellation)
        .is_err());
    assert!(completed
        .cmake_configuration_arguments(&changed_request, &cancellation)
        .is_err());
}

#[test]
fn completed_source_inventory_refuses_raw_and_owner_mutations() {
    {
        let (fixture, selection, drivers) = source_collection_fixture();
        let request = fixture.source_genmodule_request(&selection, &drivers);
        let cancellation = CancellationToken::default();
        let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
        let raw = fixture
            .output_root
            .join("genmodule-outputs")
            .join(SOURCE_GENMODULE_OWNERS[0])
            .join("module.make");
        fs::write(&raw, b"changed raw genmodule output\n").unwrap();
        assert!(completed.revalidate(&request, &cancellation).is_err());
    }

    {
        let (fixture, selection, drivers) = source_collection_fixture();
        let request = fixture.source_genmodule_request(&selection, &drivers);
        let cancellation = CancellationToken::default();
        let completed = execute_mesa_preflight(&request, &cancellation).unwrap();
        let inventory_path = fixture.output_root.join("mesa-genmodule-selection.json");
        let mut inventory: serde_json::Value =
            serde_json::from_slice(&fs::read(&inventory_path).unwrap()).unwrap();
        inventory["inventory"]["records"][0]["owner"] =
            serde_json::json!("mutated-genmodule-owner");
        fs::write(&inventory_path, serde_json::to_vec(&inventory).unwrap()).unwrap();
        assert!(completed.revalidate(&request, &cancellation).is_err());

        let input_path = fixture.output_root.join(INPUT_FILE);
        let input_bytes = fs::read(&input_path).unwrap();
        assert!(
            aros_common::mesa_configuration_input::load_mesa_configuration_input(
                &input_path,
                &sha256_bytes(&input_bytes),
            )
            .is_err()
        );
    }
}
