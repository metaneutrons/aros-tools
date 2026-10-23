//! Source-only native calls need no fabricated genmodule producer.

use super::*;
use crate::capability::mesa::configured_native_source;
use aros_common::{
    make_native_sampler::{observe_first_input_make, InitialMakeObservationRequest},
    mesa_configuration::{MesaConfigurationIdentity, MesaMakeCoordinates},
    mesa_native_make::{
        load_native_make, native_make_context_bytes, native_packet_file, LoadedMesaNativeMake,
        MesaNativeMakeBinding, MesaNativeMakeCallBinding, MesaNativeSourceCall,
        NATIVE_CONTEXT_FILE, NATIVE_CONTROL_FILE,
    },
    sha256_bytes, ArosCompilerIdentity, Sha256Digest,
};
use std::os::unix::process::CommandExt as _;

const PROGRAM: &str = concat!(
    "BASE = -DINITIAL\n",
    "USER_CPPFLAGS = -DCPP_INITIAL\n",
    "GCA_ABI_FLAGS := $(BASE) $(USER_CPPFLAGS)\n",
    "TARGET_CFLAGS = $(BASE) $(filter-out -DNOPE,$(USER_CFLAGS))\n",
    "CFLAGS = $(strip $(TARGET_CFLAGS) $(WARN))\n",
    "CXXFLAGS = $(strip $(BASE) $(USER_CXXFLAGS) $(CXX_LOCAL))\n",
    "CPPFLAGS = $(USER_CPPFLAGS)\n",
    "USER_CFLAGS = -DA -UA -DA -DNOPE\n",
    "USER_CXXFLAGS = -DCXX\n",
    "CXX_LOCAL = -DCXX_LOCAL\n",
    "BASE = -DFINAL\n",
    "USER_CPPFLAGS = -DCPP_FINAL\n",
    "WARN = -Wsource\n",
    "COMPILE_C = $(CFLAGS) $(CPPFLAGS)\n",
    "COMPILE_CXX = $(CXXFLAGS) $(CPPFLAGS)\n",
    "root:\n",
    "\t@printf '%s\\n' '$(strip $(COMPILE_C))' '$(strip $(COMPILE_CXX))' '$(strip $(GCA_ABI_FLAGS))'\n",
);

fn make_program() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(make_binary()))
        .find(|path| path.is_file())
        .expect("tests require GNU Make on PATH")
        .canonicalize()
        .unwrap()
}

fn native_call(fixture: &Fixture, source_sha256: Option<Sha256Digest>) -> LoadedMesaNativeMake {
    native_call_with_compiler(
        fixture,
        source_sha256,
        sha256_bytes(b"synthetic compiler data, not execution proof"),
    )
}

fn native_call_with_compiler(
    fixture: &Fixture,
    source_sha256: Option<Sha256Digest>,
    compiler_sha256: Sha256Digest,
) -> LoadedMesaNativeMake {
    let program = make_program();
    let program_sha256 = sha256_bytes(&std::fs::read(&program).unwrap());
    let source_sha256 = source_sha256.unwrap_or_else(|| {
        measure_tree_content_cas(&fixture.source_root)
            .unwrap()
            .payload_digest_excluding(None)
    });
    let configured_stdout = sha256_bytes(b"separately selected configure stdout");
    let identity = MesaConfigurationIdentity {
        target_profile: "synthetic-native-source".into(),
        target_triple: "aarch64-unknown-aros".into(),
        compiler: ArosCompilerIdentity::Llvm {
            version: "11.0.0".into(),
        },
        compiler_tree_sha256: compiler_sha256,
        source_inputs_sha256: source_sha256.clone(),
        configure_invocation_sha256: sha256_bytes(b"synthetic configure identity"),
    };
    let coordinates = MesaMakeCoordinates {
        source_root: fixture.source_root.to_str().unwrap().into(),
        build_root: fixture.build_root.to_str().unwrap().into(),
        ports_root: fixture.build_root.join("Ports").to_str().unwrap().into(),
        generated_root: fixture.build_root.join("gen").to_str().unwrap().into(),
    };
    let mut binding = MesaNativeMakeBinding {
        version: 1,
        source_inputs_sha256: source_sha256,
        compiler_tree_sha256: identity.compiler_tree_sha256.clone(),
        configuration_stdout_sha256: configured_stdout.clone(),
        prepared_tree_sha256: measure_tree_content_cas(&fixture.build_root)
            .unwrap()
            .payload_digest_excluding(None),
        parent_program_sha256: program_sha256.clone(),
        parent_invocation_sha256: sha256_bytes(b"selected parent data only"),
        parent_environment_sha256: sha256_bytes(b"selected parent environment data only"),
        host_metamake_sha256: sha256_bytes(b"selected host MetaMake data only"),
        context_sha256: sha256_bytes(b"placeholder"),
        control_sha256: sha256_bytes(b"placeholder"),
        calls: Vec::new(),
        owners: Vec::new(),
        source_calls: Vec::new(),
    };
    let context = native_make_context_bytes(&binding).unwrap();
    binding.context_sha256 = sha256_bytes(&context);
    let invocation = invocation(&fixture.working_directory, &synthetic_environment())
        .with_argument_zero(make_binary().into())
        .unwrap();
    let packet = observe_first_input_make(
        &InitialMakeObservationRequest {
            program: &program,
            executable_sha256: &program_sha256,
            invocation: &invocation,
            context_sha256: &binding.context_sha256,
            timeout: Duration::from_secs(10),
        },
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let packet_bytes = packet.to_bytes().unwrap();
    binding.calls.push(MesaNativeMakeCallBinding {
        document_sha256: sha256_bytes(&packet_bytes),
        binding: packet.binding().clone(),
    });
    binding.source_calls.push(MesaNativeSourceCall {
        generated_owner: "one/mmakefile".into(),
        call_index: 0,
    });
    let control = serde_json::to_vec(&serde_json::json!({
        "version":4, "phase":"first-input-prelude", "maximum_calls":1,
        "output_root":fixture.build_root.parent().unwrap().join("native-make-calls"),
        "program":program, "executable_sha256":program_sha256,
        "context_sha256":binding.context_sha256, "argument_zero":make_binary(),
    }))
    .unwrap();
    binding.control_sha256 = sha256_bytes(&control);
    let files = BTreeMap::from([
        (NATIVE_CONTEXT_FILE.to_owned(), context),
        (NATIVE_CONTROL_FILE.to_owned(), control),
        (native_packet_file(0).unwrap(), packet_bytes),
    ]);
    load_native_make(
        &binding,
        &identity,
        &configured_stdout,
        &coordinates,
        &[],
        |name, limit| {
            let bytes = files.get(name).unwrap();
            assert!(bytes.len() as u64 <= limit);
            Ok(bytes.clone())
        },
    )
    .unwrap()
}

#[test]
fn source_only_native_call_uses_source_formulas_and_frozen_gca_in_one_program() {
    let fixture = Fixture::new();
    std::fs::write(fixture.working_directory.join("mmakefile"), PROGRAM).unwrap();
    let native = native_call(&fixture, None);
    assert!(native.binding().owners.is_empty());
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let source = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap();
    assert_eq!(source.coordinates().generated_owner(), "one/mmakefile");
    let (_, mut variables, file, _) = source.into_parts();
    assert!(!file.events.is_empty());
    let (values, _) = variables
        .evaluate_many_captured(
            &[
                "$(strip $(COMPILE_C))",
                "$(strip $(COMPILE_CXX))",
                "$(strip $(GCA_ABI_FLAGS))",
            ],
            &paths,
            &snapshot,
        )
        .unwrap();
    assert_eq!(
        values,
        [
            "-DFINAL -DA -UA -DA -Wsource -DCPP_FINAL",
            "-DFINAL -DCXX -DCXX_LOCAL -DCPP_FINAL",
            "-DINITIAL -DCPP_INITIAL",
        ]
    );
    let program = make_program();
    assert_eq!(
        sha256_bytes(&std::fs::read(&program).unwrap()),
        native.binding().parent_program_sha256
    );
    let output = Command::new(program)
        .current_dir(&fixture.working_directory)
        .args([
            "-rR",
            "-s",
            "--no-print-directory",
            "-f",
            "mmakefile",
            "root",
        ])
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    let oracle: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .collect();
    assert_eq!(values, oracle);
}

#[test]
fn native_source_join_refuses_foreign_roots_paths_source_and_build_drift() {
    let fixture = Fixture::new();
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    assert!(configured_native_source::read(
        &native,
        1,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None
    )
    .is_err());
    assert!(configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["unknown".into()],
        &paths,
        &snapshot,
        None
    )
    .is_err());
    let wrong = MakeProgramPaths {
        top_directory: &fixture.source_root,
        ..paths
    };
    assert_eq!(
        configured_native_source::read(
            &native,
            0,
            &fixture.projection,
            &["root".into()],
            &wrong,
            &snapshot,
            None
        )
        .err()
        .unwrap(),
        "native Source paths differ from configured coordinates"
    );
    let foreign = native_call(&fixture, Some(sha256_bytes(b"foreign source selection")));
    assert_eq!(
        configured_native_source::read(
            &foreign,
            0,
            &fixture.projection,
            &["root".into()],
            &paths,
            &snapshot,
            None
        )
        .err()
        .unwrap(),
        "native Source bytes differ from retained selection"
    );
    std::fs::write(
        fixture.working_directory.join("extra.mk"),
        "CHANGED = yes\n",
    )
    .unwrap();
    assert!(configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None
    )
    .is_err());
    let fresh_snapshot = fixture.snapshot();
    assert_eq!(
        configured_native_source::read(
            &native,
            0,
            &fixture.projection,
            &["root".into()],
            &paths,
            &fresh_snapshot,
            None
        )
        .err()
        .unwrap(),
        "native Source input snapshot differs from retained preparation"
    );
}

#[test]
fn native_source_interpretation_errors_do_not_publish_raw_values() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        "VALUE := $(error RAW_PRIVATE_SENTINEL)\n",
    )
    .unwrap();
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let error = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .err()
    .unwrap();
    assert_eq!(error, "native Source program interpretation failed");
    assert!(!error.contains("RAW_PRIVATE_SENTINEL"));
}

#[test]
fn retained_source_recipes_preserve_target_freezes_and_automatic_frames() {
    let fixture = Fixture::new();
    let program_text = concat!(
        "BASE = -Dinitial\nLOCAL = -DA -UA -DA\n",
        "CFLAGS = $(BASE) $(LOCAL)\nCXXFLAGS = $(BASE) $(LOCALXX)\n",
        "GCA_ABI_FLAGS := $(BASE)\n",
        "c.o: FLAGS := $(CFLAGS)\ncxx.o: FLAGS = $(CXXFLAGS)\n",
        "BASE = -Dlast\nLOCAL = -DLATE_C\nLOCALXX = -DLATE_CXX\n",
        "root: c.o cxx.o gca.h\n",
        "c.o: unit.c\n\t@compile-c $(FLAGS) -c $< -o $@\n",
        "cxx.o: unit.cc\n\t@compile-cxx $(FLAGS) -c $< -o $@\n",
        "gca.h:\n\t@gca $(GCA_ABI_FLAGS) -o $@\n",
    );
    for (name, bytes) in [
        ("mmakefile", program_text),
        ("unit.c", "C input\n"),
        ("unit.cc", "CXX input\n"),
    ] {
        std::fs::write(fixture.working_directory.join(name), bytes).unwrap();
    }
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let source = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap();
    let (_, mut variables, file, _) = source.into_parts();
    let interpreted = crate::make_interpreted_rules::collect(&file).unwrap();
    let rules: Vec<_> = interpreted
        .rules
        .iter()
        .flat_map(|rule| rule.instantiate_explicit().unwrap())
        .collect();
    let root = rules.iter().find(|rule| rule.target == "root").unwrap();
    let mut commands = Vec::new();
    for target in &root.prerequisites {
        let rule = rules.iter().find(|rule| &rule.target == target).unwrap();
        assert_eq!(rule.recipes.len(), 1);
        let automatic = rule.automatic_variables();
        let expression = &rule.recipes[0].text;
        let (command, _) = variables
            .evaluate_captured_target_recipe(
                &["root", target],
                expression,
                &automatic,
                &paths,
                &snapshot,
            )
            .unwrap();
        commands.push(command.expression().strip_prefix('@').unwrap().to_owned());
        if target == "c.o" {
            let (without_frame, _) = variables
                .evaluate_captured_recipe(expression, &automatic, &paths, &snapshot)
                .unwrap();
            assert_ne!(without_frame.expression(), command.expression());
            let mut wrong = automatic.clone();
            wrong.insert("@".into(), "foreign.o".into());
            assert!(variables
                .evaluate_captured_target_recipe(
                    &["root", target],
                    expression,
                    &wrong,
                    &paths,
                    &snapshot
                )
                .is_err());
        }
    }
    assert_eq!(
        commands,
        [
            "compile-c -Dinitial -DA -UA -DA -c unit.c -o c.o",
            "compile-cxx -Dlast -DLATE_CXX -c unit.cc -o cxx.o",
            "gca -Dinitial -o gca.h",
        ]
    );
    // The production-facing generic batch keeps all three Source recipe
    // views inside one filesystem/state publication boundary, not three
    // independently accepted scalar queries.
    let selected: Vec<_> = root
        .prerequisites
        .iter()
        .map(|target| {
            let rule = rules.iter().find(|rule| &rule.target == target).unwrap();
            (
                ["root", target.as_str()],
                rule.automatic_variables(),
                rule.recipes[0].text.as_str(),
            )
        })
        .collect();
    let requests: Vec<_> = selected
        .iter()
        .map(
            |(chain, automatic_variables, expression)| crate::make_vars::MakeTargetRecipeRequest {
                chain,
                expression,
                automatic_variables,
            },
        )
        .collect();
    let (batch, _) = variables
        .evaluate_many_captured_target_recipes(&requests, &paths, &snapshot)
        .unwrap();
    let shell_batch: Vec<_> = batch
        .iter()
        .map(|projection| projection.parse_literal_shell().unwrap())
        .collect();
    assert!(shell_batch
        .iter()
        .all(|projection| projection.prefixes.silent
            && !projection.prefixes.ignore_errors
            && !projection.prefixes.explicit_recursive));
    let shell_statements: Vec<_> = shell_batch
        .into_iter()
        .flat_map(|projection| projection.statements)
        .collect();
    assert_eq!(
        batch
            .iter()
            .map(|projection| projection.expression().strip_prefix('@').unwrap())
            .collect::<Vec<_>>(),
        commands
    );
    // Use exactly the observed argv/environment. Its -n prevents these
    // intentionally nonexistent compiler/generator commands from executing.
    let observation = native.observation(0).unwrap().unwrap();
    let invocation = observation.invocation();
    let program = make_program();
    assert_eq!(
        sha256_bytes(&std::fs::read(&program).unwrap()),
        native.binding().parent_program_sha256
    );
    let mut oracle = Command::new(program);
    oracle
        .arg0(invocation.argument_zero().unwrap())
        .args(invocation.arguments())
        .current_dir(invocation.working_directory())
        .env_clear()
        .envs(invocation.environment());
    let output = oracle.output().unwrap();
    assert!(output.status.success());
    let observed: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .collect();
    assert_eq!(commands, observed);
    assert_eq!(
        shell_statements,
        crate::make_shell_data::parse_literal_recipe(std::str::from_utf8(&output.stdout).unwrap())
            .unwrap()
    );
    for name in ["c.o", "cxx.o", "gca.h"] {
        assert!(!fixture.working_directory.join(name).exists());
    }
    // The integrated Source plan derives selection/first-parent frames and
    // automatic variables itself; no manually supplied per-target flag view.
    let plan = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap()
    .trace_explicit_recipes(&paths)
    .unwrap();
    assert_eq!(plan.recipes().len(), 3);
    assert!(plan.implicit_events().is_empty());
    assert!(!plan.non_rule_events().is_empty());
    assert_eq!(
        plan.recipes()
            .iter()
            .flat_map(|recipe| recipe.shell().statements.clone())
            .collect::<Vec<_>>(),
        shell_statements
    );
    for index in 0..plan.recipes().len() {
        assert!(plan.origin(index).unwrap().active);
    }
    assert!(plan.origin(plan.recipes().len()).is_err());
    plan.verify().unwrap();
    std::fs::write(
        fixture.working_directory.join("unit.c"),
        b"changed after recipe projection\n",
    )
    .unwrap();
    assert!(plan.verify().is_err());
}

#[test]
fn source_recipe_plan_cannot_adopt_new_preparation_after_original_read() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        "root:\n\t@echo original\n",
    )
    .unwrap();
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let source = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        "root:\n\t@echo replacement\n",
    )
    .unwrap();
    // A new snapshot is physically valid, but the Source object retains the
    // original selection and exposes no replacement-snapshot argument.
    let _fresh_snapshot = fixture.snapshot();
    let error = source.trace_explicit_recipes(&paths).err().unwrap();
    assert_eq!(error, "native Source recipe transaction failed");
    assert!(!error.contains("replacement"));
}

#[test]
fn source_plan_binds_distinct_invocation_roles_and_frozen_generator_argument_together() {
    use crate::make_shell_data::artifact_binding::{
        RecipeArtifactDirection as Direction, RecipeArtifactRequest, RecipeArtifactSlot,
    };
    use aros_common::captured_programs::{
        CapturedProgramSet, ProgramRootSelection, ProgramSelection,
    };
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = Fixture::new();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let primary = root.join("first-driver");
    let alternate = root.join("second-driver");
    let generator = root.join("abi-tool");
    for path in [&primary, &generator] {
        std::fs::write(path, b"selected program data, never executed").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    symlink("first-driver", &alternate).unwrap();
    let root_sha = measure_tree_content_cas(&root)
        .unwrap()
        .payload_digest_excluding(None);
    let roots = [ProgramRootSelection {
        root: root.clone(),
        content_sha256: root_sha.clone(),
    }];
    let selections: Vec<_> = [
        ("source-c", &primary),
        ("source-cxx", &alternate),
        ("source-abi", &generator),
    ]
    .into_iter()
    .map(|(role, path)| ProgramSelection {
        role: role.into(),
        root_index: 0,
        invocation_path: path.clone(),
        executable_sha256: sha256_bytes(b"selected program data, never executed"),
    })
    .collect();
    let programs = CapturedProgramSet::capture(&roots, &selections).unwrap();
    let source = format!(
        concat!(
            "BASE = -Dearly\nABI := $(BASE) -DFROZEN\n",
            "one.o: FLAGS := $(BASE) -DA -UA -DA\n",
            "two.o: FLAGS = $(BASE) -DCXX\nBASE = -Dlate\n",
            "root: one.o two.o abi.h\n",
            "one.o: one.c\n\t@'{}' $(FLAGS) -c $< -o $@\n",
            "two.o: two.cc\n\t@'{}' $(FLAGS) -c $< -o $@\n",
            "abi.h:\n\t@'{}' abi-script.py --abi-flags=\"$(ABI)\" -o $@\n",
        ),
        primary.display(),
        alternate.display(),
        generator.display()
    );
    std::fs::write(fixture.working_directory.join("mmakefile"), source).unwrap();
    for input in ["one.c", "two.cc", "abi-script.py"] {
        std::fs::write(
            fixture.working_directory.join(input),
            b"uncompiled Source input\n",
        )
        .unwrap();
    }
    let native = native_call_with_compiler(&fixture, None, root_sha);
    assert!(programs.matches_root_selection(&root, &native.binding().compiler_tree_sha256));
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let plan = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap()
    .trace_explicit_recipes(&paths)
    .unwrap();
    let bound = plan.bind_programs(&programs).unwrap();
    assert_eq!(bound.len(), 3);
    let c = bound[0].unique_role("source-c").unwrap();
    let cxx = bound[1].unique_role("source-cxx").unwrap();
    let abi = bound[2].unique_role("source-abi").unwrap();
    assert_eq!(
        c.program().unwrap().resolved_path(),
        cxx.program().unwrap().resolved_path()
    );
    assert_ne!(
        c.program().unwrap().invocation_path(),
        cxx.program().unwrap().invocation_path()
    );
    assert_eq!(
        &c.command().words()[1..],
        ["-Dearly", "-DA", "-UA", "-DA", "-c", "one.c", "-o", "one.o"]
    );
    assert_eq!(
        &cxx.command().words()[1..],
        ["-Dlate", "-DCXX", "-c", "two.cc", "-o", "two.o"]
    );
    assert_eq!(
        &abi.command().words()[1..],
        [
            "abi-script.py",
            "--abi-flags=-Dearly -DFROZEN",
            "-o",
            "abi.h"
        ]
    );
    let request = |recipe, index, path: &str, direction| RecipeArtifactRequest {
        recipe,
        command: 0,
        slot: RecipeArtifactSlot::Word {
            index,
            range: 0..path.len(),
        },
        direction,
    };
    let requests = vec![
        request(0, 6, "one.c", Direction::Input),
        request(0, 8, "one.o", Direction::Output),
        request(1, 4, "two.cc", Direction::Input),
        request(1, 6, "two.o", Direction::Output),
        request(2, 1, "abi-script.py", Direction::Input),
        request(2, 4, "abi.h", Direction::Output),
    ];
    let output_root = aros_common::make_input_snapshot::MakeInputRootSelection {
        root: fixture.build_root.clone(),
        content_sha256: native.binding().prepared_tree_sha256.clone(),
    };
    assert_eq!(
        std::fs::metadata(fixture.working_directory.join("abi-script.py"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    let files = plan
        .bind_artifacts(&programs, Some(&output_root), &requests)
        .unwrap();
    assert_eq!(files.bindings().len(), 3);
    assert_eq!(files.artifacts().len(), 6);
    assert_eq!(files.reads().reads().len(), 3);
    for (artifact, expected) in files.artifacts().iter().zip([
        "one.c",
        "one.o",
        "two.cc",
        "two.o",
        "abi-script.py",
        "abi.h",
    ]) {
        assert_eq!(artifact.path(), fixture.working_directory.join(expected));
        if artifact.request().direction == Direction::Input {
            assert_eq!(
                artifact.input_sha256(),
                Some(&sha256_bytes(b"uncompiled Source input\n"))
            );
        } else {
            assert_eq!(artifact.previous_kind(), None);
            assert_eq!(artifact.input_sha256(), None);
        }
    }
    let wrong_root = aros_common::make_input_snapshot::MakeInputRootSelection {
        root: output_root.root.clone(),
        content_sha256: sha256_bytes(b"not the original preparation"),
    };
    assert!(plan
        .bind_artifacts(&programs, Some(&wrong_root), &requests)
        .is_err());
    let source_as_output = aros_common::make_input_snapshot::MakeInputRootSelection {
        root: fixture.source_root.clone(),
        content_sha256: native.binding().source_inputs_sha256.clone(),
    };
    assert!(
        snapshot.matches_root_selection(&source_as_output.root, &source_as_output.content_sha256)
    );
    assert_eq!(
        plan.bind_artifacts(&programs, Some(&source_as_output), &requests)
            .unwrap_err(),
        "native Source output root differs from original selection"
    );
    let mut late_invalid = requests.clone();
    late_invalid.push(request(2, 999, "absent-slot", Direction::Input));
    assert!(plan
        .bind_artifacts(&programs, Some(&output_root), &late_invalid)
        .is_err());
    std::fs::write(&primary, b"changed executable data").unwrap();
    plan.verify().unwrap();
    assert!(plan.bind_programs(&programs).is_err());
    assert!(plan
        .bind_artifacts(&programs, Some(&output_root), &requests)
        .is_err());
    std::fs::write(&primary, b"selected program data, never executed").unwrap();
    let fresh_programs = CapturedProgramSet::capture(&roots, &selections).unwrap();
    plan.bind_programs(&fresh_programs).unwrap();
    plan.bind_artifacts(&fresh_programs, Some(&output_root), &requests)
        .unwrap();
    std::fs::write(
        fixture.working_directory.join("one.c"),
        b"changed Source data",
    )
    .unwrap();
    assert!(plan.bind_programs(&fresh_programs).is_err());
    assert!(plan
        .bind_artifacts(&fresh_programs, Some(&output_root), &requests)
        .is_err());
    for output in ["one.o", "two.o", "abi.h"] {
        assert!(!fixture.working_directory.join(output).exists());
    }
}

#[test]
fn merged_recipe_provider_frame_matches_original_gnu_make_order() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        concat!(
            "root: before.c\nroot: recipe.c\n",
            "\t@echo first=$< all=$+ unique=$^\n",
        ),
    )
    .unwrap();
    for input in ["before.c", "recipe.c"] {
        std::fs::write(fixture.working_directory.join(input), b"selected input\n").unwrap();
    }
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let plan = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap()
    .trace_explicit_recipes(&paths)
    .unwrap();
    assert_eq!(
        plan.recipes()[0].expression().expression(),
        "@echo first=recipe.c all=recipe.c before.c unique=recipe.c before.c"
    );
    let observation = native.observation(0).unwrap().unwrap();
    let invocation = observation.invocation();
    let program = make_program();
    assert_eq!(
        sha256_bytes(&std::fs::read(&program).unwrap()),
        native.binding().parent_program_sha256
    );
    let mut oracle = Command::new(&program);
    let output = oracle
        .arg0(invocation.argument_zero().unwrap())
        .args(invocation.arguments())
        .current_dir(invocation.working_directory())
        .env_clear()
        .envs(invocation.environment())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        plan.recipes()[0].shell().statements,
        crate::make_shell_data::parse_literal_recipe(std::str::from_utf8(&output.stdout).unwrap())
            .unwrap()
    );
    assert_eq!(
        plan.traversal()
            .visits()
            .iter()
            .map(crate::make_rule_graph::traversal::TargetVisit::target)
            .collect::<Vec<_>>(),
        ["root", "recipe.c", "before.c"]
    );
}

#[test]
fn source_recipe_plan_retains_pipeline_stages_without_executing_them() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        concat!(
            "FLAGS := -DFROZEN -UREPEAT -DREPEAT\n",
            "root: archive\narchive: input.list\n",
            "\t@cat $< | dispatch '$(FLAGS)' rs $@ 2>errors\n",
        ),
    )
    .unwrap();
    std::fs::write(
        fixture.working_directory.join("input.list"),
        b"private input\n",
    )
    .unwrap();
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let plan = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap()
    .trace_explicit_recipes(&paths)
    .unwrap();
    assert_eq!(plan.recipes().len(), 1);
    let projected = &plan.recipes()[0].shell().statements;
    let crate::make_shell_data::LiteralRecipeStatement::Pipeline(stages) = &projected[0] else {
        panic!()
    };
    assert_eq!(stages[0].words(), ["cat", "input.list"]);
    assert_eq!(
        stages[1].words(),
        ["dispatch", "-DFROZEN -UREPEAT -DREPEAT", "rs", "archive"]
    );
    assert_eq!(stages[1].redirections()[0].target(), "errors");
    let observation = native.observation(0).unwrap().unwrap();
    let invocation = observation.invocation();
    let program = make_program();
    assert_eq!(
        sha256_bytes(&std::fs::read(&program).unwrap()),
        native.binding().parent_program_sha256
    );
    let mut oracle = Command::new(&program);
    let output = oracle
        .arg0(invocation.argument_zero().unwrap())
        .args(invocation.arguments())
        .current_dir(invocation.working_directory())
        .env_clear()
        .envs(invocation.environment())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        *projected,
        crate::make_shell_data::parse_literal_recipe(std::str::from_utf8(&output.stdout).unwrap())
            .unwrap()
    );
    assert_eq!(
        sha256_bytes(&std::fs::read(&program).unwrap()),
        native.binding().parent_program_sha256
    );
    plan.verify().unwrap();
    for output in ["archive", "errors"] {
        assert!(!fixture.working_directory.join(output).exists());
    }
}

#[test]
fn source_recipe_plan_cannot_replace_original_matched_build_coordinate() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        "root:\n\t@echo literal\n",
    )
    .unwrap();
    let native = native_call(&fixture, None);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let source = configured_native_source::read(
        &native,
        0,
        &fixture.projection,
        &["root".into()],
        &paths,
        &snapshot,
        None,
    )
    .unwrap();
    let replacement = crate::make_vars::MakeProgramPaths {
        top_directory: &fixture.source_root,
        ..paths
    };
    assert_eq!(
        source.trace_explicit_recipes(&replacement).err().unwrap(),
        "native Source recipe build root differs from original match"
    );
}
