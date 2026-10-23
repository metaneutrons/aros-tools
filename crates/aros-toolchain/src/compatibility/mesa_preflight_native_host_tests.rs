use super::*;
use std::io::Write as _;

const HOST_QUERY_VARIABLES: [&str; 4] = ["MMAKE", "HOST_CC", "HOST_MAKE", "CROSSTOOLS_TARGET"];

struct HostOutputs {
    evidence_root: PathBuf,
    executable: PathBuf,
    namespace: PathBuf,
    manifest: PathBuf,
    fresh_files: Vec<PathBuf>,
    fresh_directories: Vec<PathBuf>,
}

fn host_outputs(fixture: &Fixture) -> HostOutputs {
    let build = fixture.output_root.join("configure");
    let executable = build.join("tools/MetaMake/mmake");
    let namespace = build.join("tools/MetaMake/generated");
    let manifest = namespace.join("manifest.txt");
    HostOutputs {
        evidence_root: fixture.output_root.with_file_name("native-host-build"),
        executable: executable.clone(),
        namespace: namespace.clone(),
        manifest,
        fresh_files: vec![executable],
        fresh_directories: vec![namespace],
    }
}

fn success_recipe() -> Vec<String> {
    [
        "\t@mkdir -p \"$(TOP)/tools/MetaMake/generated\"",
        "\t@printf '#!/bin/sh\\nexit 0\\n' > \"$(MMAKE)\"",
        "\t@chmod 755 \"$(MMAKE)\"",
        "\t@printf '%s\\n' 'host namespace' > \"$(TOP)/tools/MetaMake/generated/manifest.txt\"",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn configure_host_goal(
    fixture: &mut Fixture,
    host_cc: &str,
    host_make: &str,
    crosstools_target: &str,
    recipe: &[String],
) {
    let mut lines = vec![
        "MMAKE := $(TOP)/tools/MetaMake/mmake".to_owned(),
        format!("HOST_CC := {host_cc}"),
        format!("HOST_MAKE := {host_make}"),
        format!("CROSSTOOLS_TARGET := {crosstools_target}"),
        ".PHONY: mmake".to_owned(),
        "mmake:".to_owned(),
    ];
    lines.extend_from_slice(recipe);

    let mut configure = fs::OpenOptions::new()
        .append(true)
        .open(fixture.source_root.join("configure"))
        .unwrap();
    for line in lines {
        writeln!(
            configure,
            "printf '%s\\n' {} >> Makefile",
            shell_quote(&line)
        )
        .unwrap();
    }
    fixture.source_tree_sha256 = aros_common::measure_tree_content_cas(&fixture.source_root)
        .unwrap()
        .payload_digest_excluding(None);
}

fn sealed_host_cc(fixture: &Fixture) -> String {
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = &fixture.environment else {
        unreachable!()
    };
    format!("{} -std=gnu23", host_tools.root.join("cc").display())
}

fn host_selection<'a>(outputs: &'a HostOutputs, goal: &'a str) -> MesaNativeHostBuildSelection<'a> {
    MesaNativeHostBuildSelection {
        output_root: &outputs.evidence_root,
        goal,
        additional_goals: &[],
        executable_variable: "MMAKE",
        fresh_files: &outputs.fresh_files,
        fresh_directories: &outputs.fresh_directories,
        absent_inputs: &[],
    }
}

fn assert_no_completed_preflight(fixture: &Fixture) {
    for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
        assert!(
            !fixture.output_root.join(name).exists(),
            "native host build must not publish {name}"
        );
    }
}

#[test]
fn native_host_goal_runs_original_make_with_sealed_roles_and_tracks_fresh_outputs() {
    let mut fixture = Fixture::new();
    let recipe = success_recipe();
    let host_cc = sealed_host_cc(&fixture);
    configure_host_goal(&mut fixture, &host_cc, "make", "", &recipe);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let query_root = fixture.output_root.with_file_name("native-root-query");
    let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let goals = ["mmake".to_owned()];
    let query_selection = MesaNativeRootQuerySelection {
        output_root: &query_root,
        variables: &variables,
        goals: &goals,
    };
    let root = observe_mesa_native_root(&request, &query_selection, &cancellation).unwrap();

    assert_eq!(root.observed_exit_code(), 2);
    assert_eq!(root.values()["HOST_CC"], host_cc);
    assert_eq!(root.values()["HOST_MAKE"], "make");
    assert_eq!(root.values()["CROSSTOOLS_TARGET"], "");
    assert_eq!(
        root.values()["MMAKE"],
        outputs.executable.to_string_lossy().as_ref()
    );
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        unreachable!()
    };
    assert!(host_tools.tools.contains_key("make"));
    assert_eq!(
        fs::canonicalize(host_tools.root.join("make")).unwrap(),
        host_tools.tools["make"].program
    );

    let selection = host_selection(&outputs, "mmake");
    let built = build_mesa_native_host(root, &request, &query_selection, &selection, &cancellation)
        .unwrap();

    assert_eq!(built.executable(), outputs.executable.as_path());
    let metadata = fs::symlink_metadata(built.executable()).unwrap();
    assert!(metadata.file_type().is_file());
    assert_ne!(metadata.permissions().mode() & 0o111, 0);
    assert_eq!(fs::read(&outputs.manifest).unwrap(), b"host namespace\n");
    assert_eq!(
        built.executable_sha256(),
        &sha256_bytes(&fs::read(built.executable()).unwrap())
    );
    assert!(outputs.namespace.is_dir());
    built
        .revalidate(&request, &query_selection, &selection, &cancellation)
        .unwrap();
    assert_no_completed_preflight(&fixture);
}

#[test]
fn native_host_goal_vector_preserves_original_makecmdgoals_and_output_policy() {
    let mut fixture = Fixture::new();
    let mut recipe = success_recipe();
    recipe.extend([
        "GENMODULE := $(TOP)/tools/MetaMake/genmodule".to_owned(),
        "ifneq ($(MAKECMDGOALS),mmake genmodule)".to_owned(),
        "$(error unexpected host goal context)".to_owned(),
        "endif".to_owned(),
        ".PHONY: genmodule".to_owned(),
        "genmodule:".to_owned(),
        "\t@printf '#!/bin/sh\\nexit 0\\n' > \"$(GENMODULE)\"".to_owned(),
        "\t@chmod 755 \"$(GENMODULE)\"".to_owned(),
        "\t@mkdir -p \"$(GENMODULE).dSYM/Contents\"".to_owned(),
        "\t@printf '%s\\n' 'debug bundle' > \"$(GENMODULE).dSYM/Contents/Info.plist\"".to_owned(),
        "\t@printf '%s\\n' '$(MAKECMDGOALS)' > \"$(TOP)/tools/MetaMake/generated/goals.txt\""
            .to_owned(),
    ]);
    let host_cc = sealed_host_cc(&fixture);
    configure_host_goal(&mut fixture, &host_cc, "make", "", &recipe);
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let mut outputs = host_outputs(&fixture);
    let genmodule = fixture
        .output_root
        .join("configure/tools/MetaMake/genmodule");
    let debug_bundle = fixture
        .output_root
        .join("configure/tools/MetaMake/genmodule.dSYM");
    outputs.fresh_files.push(genmodule.clone());
    outputs.fresh_directories.push(debug_bundle.clone());
    let query_root = fixture.output_root.with_file_name("native-root-query");
    let mut variables = HOST_QUERY_VARIABLES.map(str::to_owned).to_vec();
    variables.extend(["MAKECMDGOALS".into(), "GENMODULE".into()]);
    let goals = ["mmake".to_owned(), "genmodule".to_owned()];
    let query = MesaNativeRootQuerySelection {
        output_root: &query_root,
        variables: &variables,
        goals: &goals,
    };
    let root = observe_mesa_native_root(&request, &query, &cancellation).unwrap();
    assert_eq!(root.values()["MAKECMDGOALS"], "mmake genmodule");
    assert!(!outputs.namespace.exists(), "query must not execute goals");
    let selection = MesaNativeHostBuildSelection {
        additional_goals: &goals[1..],
        ..host_selection(&outputs, "mmake")
    };
    let built = build_mesa_native_host(root, &request, &query, &selection, &cancellation).unwrap();
    assert_eq!(
        fs::read(outputs.namespace.join("goals.txt")).unwrap(),
        b"mmake genmodule\n"
    );
    assert_eq!(fs::read(&genmodule).unwrap(), b"#!/bin/sh\nexit 0\n");
    assert_eq!(
        fs::read(debug_bundle.join("Contents/Info.plist")).unwrap(),
        b"debug bundle\n"
    );
    built
        .revalidate(&request, &query, &selection, &cancellation)
        .unwrap();
    let changed = MesaNativeHostBuildSelection {
        additional_goals: &[],
        ..selection
    };
    assert_preflight_error(
        built.revalidate(&request, &query, &changed, &cancellation),
        "native host build selected different inputs",
    );
    assert_no_completed_preflight(&fixture);
}

#[test]
fn native_host_goal_vector_changes_refuse_before_launch() {
    for selected_goals in [
        vec!["mmake"],
        vec!["genmodule", "mmake"],
        vec!["mmake", "genmodule", "extra"],
        vec!["mmake", "other"],
    ] {
        let mut fixture = Fixture::new();
        let host_cc = sealed_host_cc(&fixture);
        configure_host_goal(&mut fixture, &host_cc, "make", "", &success_recipe());
        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let outputs = host_outputs(&fixture);
        let query_root = fixture.output_root.with_file_name("native-root-query");
        let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
        let goals = ["mmake".to_owned(), "genmodule".to_owned()];
        let query = MesaNativeRootQuerySelection {
            output_root: &query_root,
            variables: &variables,
            goals: &goals,
        };
        let root = observe_mesa_native_root(&request, &query, &cancellation).unwrap();
        let additional = selected_goals[1..]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>();
        let selection = MesaNativeHostBuildSelection {
            additional_goals: &additional,
            ..host_selection(&outputs, selected_goals[0])
        };
        assert_preflight_error(
            build_mesa_native_host(root, &request, &query, &selection, &cancellation),
            "native host goal differs from original Root query",
        );
        assert!(!outputs.evidence_root.exists());
        assert!(!outputs.executable.exists());
        assert!(!outputs.namespace.exists());
    }
}

#[test]
fn native_host_admission_rejects_goal_and_foreign_or_enabled_compiler_roles_before_launch() {
    for case in [
        "goal-mismatch",
        "foreign-host-cc",
        "foreign-recursive-make",
        "compiler-construction-enabled",
    ] {
        let mut fixture = Fixture::new();
        let recipe = success_recipe();
        let sealed_cc = sealed_host_cc(&fixture);
        let (host_cc, host_make, crosstools, query_goal, build_goal, expected) = match case {
            "goal-mismatch" => (
                sealed_cc.as_str(),
                "make",
                "",
                "all",
                "mmake",
                "native host goal differs from original Root query",
            ),
            "foreign-host-cc" => (
                "cc -std=gnu23",
                "make",
                "",
                "mmake",
                "mmake",
                "native host compiler differs from sealed role",
            ),
            "foreign-recursive-make" => (
                sealed_cc.as_str(),
                "gmake",
                "",
                "mmake",
                "mmake",
                "native recursive Make differs from sealed driver",
            ),
            "compiler-construction-enabled" => (
                sealed_cc.as_str(),
                "make",
                "arm-unknown-aros",
                "mmake",
                "mmake",
                "native host build requires compiler construction disabled",
            ),
            _ => unreachable!(),
        };
        configure_host_goal(&mut fixture, host_cc, host_make, crosstools, &recipe);

        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let outputs = host_outputs(&fixture);
        let query_root = fixture.output_root.with_file_name("native-root-query");
        let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
        let query_goals = [query_goal.to_owned()];
        let query_selection = MesaNativeRootQuerySelection {
            output_root: &query_root,
            variables: &variables,
            goals: &query_goals,
        };
        let root = observe_mesa_native_root(&request, &query_selection, &cancellation).unwrap();
        let selection = host_selection(&outputs, build_goal);

        let result =
            build_mesa_native_host(root, &request, &query_selection, &selection, &cancellation);
        assert_preflight_error(result, expected);
        assert!(
            !outputs.evidence_root.exists(),
            "{case} must be rejected before the host-build process launches"
        );
        assert!(!outputs.executable.exists());
        assert!(!outputs.namespace.exists());
        assert!(!fixture
            .output_root
            .join("configure/poison-target-ran")
            .exists());
        assert_no_completed_preflight(&fixture);
    }
}

#[test]
fn native_host_process_failure_retains_bounded_stage_logs() {
    let mut fixture = Fixture::new();
    let recipe = [
        "\t@printf '%s\\n' 'fixture host build failed' >&2".to_owned(),
        "\t@exit 43".to_owned(),
    ];
    let host_cc = sealed_host_cc(&fixture);
    configure_host_goal(&mut fixture, &host_cc, "make", "", &recipe);

    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let outputs = host_outputs(&fixture);
    let query_root = fixture.output_root.with_file_name("native-root-query");
    let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
    let goals = ["mmake".to_owned()];
    let query_selection = MesaNativeRootQuerySelection {
        output_root: &query_root,
        variables: &variables,
        goals: &goals,
    };
    let root = observe_mesa_native_root(&request, &query_selection, &cancellation).unwrap();
    let selection = host_selection(&outputs, "mmake");

    let result =
        build_mesa_native_host(root, &request, &query_selection, &selection, &cancellation);
    assert_preflight_error(
        result,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    let stderr = fs::read_to_string(
        outputs
            .evidence_root
            .join("native-host-metamake-build.stderr"),
    )
    .unwrap();
    assert!(stderr.contains("fixture host build failed"));
    assert!(outputs
        .evidence_root
        .join("native-host-metamake-build.stdout")
        .is_file());
    assert!(!outputs.executable.exists());
    assert_no_completed_preflight(&fixture);
}

#[test]
fn native_host_build_rejects_unselected_writes_and_changes_to_existing_makefile() {
    for case in [
        "outside-fresh-output",
        "unselected-debug-bundle",
        "overwrite-root-makefile",
        "chmod-preparation-root",
    ] {
        let mut fixture = Fixture::new();
        let mut recipe = success_recipe();
        let expected = match case {
            "unselected-debug-bundle" => {
                recipe.extend([
                    "\t@mkdir -p \"$(MMAKE).dSYM/Contents\"".into(),
                    "\t@printf '%s\\n' 'debug bundle' > \"$(MMAKE).dSYM/Contents/Info.plist\""
                        .into(),
                ]);
                "native host wrote outside selected fresh outputs"
            }
            "outside-fresh-output" => {
                recipe
                    .push("\t@printf '%s\\n' 'foreign output' > \"$(TOP)/unselected.txt\"".into());
                "native host wrote outside selected fresh outputs"
            }
            "overwrite-root-makefile" => {
                recipe = vec![
                    "\t@printf '%s\\n' 'overwritten old Makefile' > \"$(TOP)/Makefile\"".into(),
                ];
                "native Root query input changed"
            }
            "chmod-preparation-root" => {
                recipe = vec!["\t@chmod 755 ..".into()];
                "Mesa preparation output root mode changed"
            }
            _ => unreachable!(),
        };
        let host_cc = sealed_host_cc(&fixture);
        configure_host_goal(&mut fixture, &host_cc, "make", "", &recipe);

        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let outputs = host_outputs(&fixture);
        let query_root = fixture.output_root.with_file_name("native-root-query");
        let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
        let goals = ["mmake".to_owned()];
        let query_selection = MesaNativeRootQuerySelection {
            output_root: &query_root,
            variables: &variables,
            goals: &goals,
        };
        let root = observe_mesa_native_root(&request, &query_selection, &cancellation).unwrap();
        let selection = host_selection(&outputs, "mmake");

        let result =
            build_mesa_native_host(root, &request, &query_selection, &selection, &cancellation);
        assert_preflight_error(result, expected);
        assert!(outputs
            .evidence_root
            .join("native-host-metamake-build.stderr")
            .is_file());
        if case == "outside-fresh-output" {
            assert_eq!(
                fs::read(fixture.output_root.join("configure/unselected.txt")).unwrap(),
                b"foreign output\n"
            );
        } else if case == "overwrite-root-makefile" {
            assert_eq!(
                fs::read(fixture.output_root.join("configure/Makefile")).unwrap(),
                b"overwritten old Makefile\n"
            );
        } else if case == "unselected-debug-bundle" {
            assert_eq!(
                fs::read(
                    outputs
                        .executable
                        .with_file_name("mmake.dSYM/Contents/Info.plist")
                )
                .unwrap(),
                b"debug bundle\n"
            );
        } else {
            assert_eq!(
                fs::metadata(&fixture.output_root)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        assert_no_completed_preflight(&fixture);
    }
}

#[test]
fn built_native_host_revalidation_rejects_artifact_and_evidence_mutation() {
    for mutation in ["artifact", "evidence"] {
        let mut fixture = Fixture::new();
        let recipe = success_recipe();
        let host_cc = sealed_host_cc(&fixture);
        configure_host_goal(&mut fixture, &host_cc, "make", "", &recipe);

        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let outputs = host_outputs(&fixture);
        let query_root = fixture.output_root.with_file_name("native-root-query");
        let variables = HOST_QUERY_VARIABLES.map(str::to_owned);
        let goals = ["mmake".to_owned()];
        let query_selection = MesaNativeRootQuerySelection {
            output_root: &query_root,
            variables: &variables,
            goals: &goals,
        };
        let root = observe_mesa_native_root(&request, &query_selection, &cancellation).unwrap();
        let selection = host_selection(&outputs, "mmake");
        let built =
            build_mesa_native_host(root, &request, &query_selection, &selection, &cancellation)
                .unwrap();
        built
            .revalidate(&request, &query_selection, &selection, &cancellation)
            .unwrap();

        let expected = if mutation == "artifact" {
            fs::write(built.executable(), b"changed host executable\n").unwrap();
            "native host build output transition changed"
        } else {
            let mut log = fs::OpenOptions::new()
                .append(true)
                .open(
                    outputs
                        .evidence_root
                        .join("native-host-metamake-build.stdout"),
                )
                .unwrap();
            writeln!(log, "changed retained evidence").unwrap();
            "native host evidence changed"
        };
        assert_preflight_error(
            built.revalidate(&request, &query_selection, &selection, &cancellation),
            expected,
        );
    }
}

#[path = "mesa_preflight_native_launch_inputs_tests.rs"]
mod launch_inputs;

#[path = "mesa_preflight_native_dispatch_phase_tests.rs"]
mod native_dispatch;
