//! Closed direct-frame data semantics compared with native target-private overrides.
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_native_invocation::NativeMakeInvocation,
    make_native_sampler::{observe_first_input_make, InitialMakeObservationRequest},
    measure_tree_content_cas, sha256_bytes, CancellationToken,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeTargetRecipeRequest, MakeVariableProgram},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn gnu_make() -> PathBuf {
    let search = std::env::var_os("PATH").unwrap();
    ["gmake", "make"]
        .into_iter()
        .flat_map(|name| std::env::split_paths(&search).map(move |directory| directory.join(name)))
        .find(|path| {
            path.is_file()
                && Command::new(path)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| {
                        output.status.success() && output.stdout.starts_with(b"GNU Make ")
                    })
        })
        .expect("native test requires GNU Make")
        .canonicalize()
        .unwrap()
}

fn paths<'a>(root: &'a Path, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

#[test]
fn direct_literal_overrides_match_native_cli_priority_origin_exports_and_literal_dollars() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let expression = "$(X)|$(origin X)|$(flavor X)|$(value X)|$@|$(MAKEFILE_LIST)";
    let value = "literal-$dollar-$(error DO_NOT_EXPAND)";
    let source = format!("X := global\nexport X\nt: X := source-local\nt:\n\t@printf '%s\\n' '{expression}' \"$$X\"\nchild:\n\t@printf '%s\\n' '$(X)'\n");
    fs::write(root.join("Root.mk"), &source).unwrap();
    fs::write(
        root.join("override.mk"),
        format!("t: private override X := {}\n", value.replace('$', "$$")),
    )
    .unwrap();
    let make = gnu_make();
    let executable_sha256 = sha256_bytes(&fs::read(&make).unwrap());
    let context_sha256 = sha256_bytes(b"direct-override-native-test");
    let invocation = NativeMakeInvocation::new(
        ["-r", "-s", "-f", "Root.mk", "X=cli", "t"].map(str::to_owned),
        root.clone(),
        [],
    )
    .unwrap();
    let observed = observe_first_input_make(
        &InitialMakeObservationRequest {
            program: &make,
            executable_sha256: &executable_sha256,
            context_sha256: &context_sha256,
            invocation: &invocation,
            timeout: Duration::from_secs(20),
        },
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let snapshot = MakeInputSnapshot::capture(
        &root,
        &[MakeInputRootSelection {
            root: root.clone(),
            content_sha256: measure_tree_content_cas(&root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        observed.initial_stdout(),
        &invocation,
        observed.environment_names(),
    )
    .unwrap();
    program
        .read_captured_file("Root.mk", &paths, &snapshot)
        .unwrap();
    let before = program.events().to_vec();
    assert!(program
        .record_captured_input_list_entry(
            "override.mk",
            &sha256_bytes(b"wrong bytes"),
            &paths,
            &snapshot,
        )
        .is_err());
    assert_eq!(program.events(), before);
    let override_sha256 = sha256_bytes(&fs::read(root.join("override.mk")).unwrap());
    let suffix_reads = program
        .record_captured_input_list_entry("override.mk", &override_sha256, &paths, &snapshot)
        .unwrap();
    assert_eq!(suffix_reads.reads().len(), 1);
    assert_eq!(
        program.evaluate("$(MAKEFILE_LIST)", &paths).unwrap(),
        "Root.mk override.mk"
    );
    let automatic = BTreeMap::from([("@".into(), "t".into())]);
    let values = BTreeMap::from([("X".into(), value.into())]);
    let request = MakeTargetRecipeRequest {
        chain: &["t"],
        expression,
        automatic_variables: &automatic,
    };
    let (projection, _) = program
        .evaluate_captured_direct_recipe_with_overrides(request, &values, &paths, &snapshot)
        .unwrap();
    assert_eq!(
        projection.expression(),
        format!("{value}|override|simple|{value}|t|Root.mk override.mk")
    );
    assert_eq!(projection.source_exports()["X"], value);
    let native = Command::new(&make)
        .env_clear()
        .current_dir(&root)
        .args([
            "-r",
            "-s",
            "-f",
            "Root.mk",
            "-f",
            "override.mk",
            "X=cli",
            "t",
            "child",
        ])
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        format!("{}\n{value}\ncli\n", projection.expression())
    );
    assert_eq!(
        program.evaluate("$(X)|$(origin X)", &paths).unwrap(),
        "cli|command line"
    );
    let (ordinary, _) = program
        .evaluate_captured_target_recipe(&["t"], "$(X)", &automatic, &paths, &snapshot)
        .unwrap();
    assert_eq!(ordinary.expression(), "cli");
    for bad_chain in [&["parent", "t"][..], &[][..]] {
        assert!(program
            .evaluate_captured_direct_recipe_with_overrides(
                MakeTargetRecipeRequest {
                    chain: bad_chain,
                    ..request
                },
                &values,
                &paths,
                &snapshot,
            )
            .is_err());
    }
    let bad_values = BTreeMap::from([("X$(error BAD_NAME)".into(), "ignored".into())]);
    assert!(program
        .evaluate_captured_direct_recipe_with_overrides(request, &bad_values, &paths, &snapshot)
        .is_err());
    assert!(program
        .evaluate_captured_direct_recipe_with_overrides(
            MakeTargetRecipeRequest {
                expression: "$(eval X := leaked)",
                ..request
            },
            &values,
            &paths,
            &snapshot,
        )
        .is_err());
    assert_eq!(
        program.evaluate("$(X)|$(origin X)", &paths).unwrap(),
        "cli|command line"
    );
}
