//! Atomic composition of captured scalar controls and direct recipe frames.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot, VerifiedMakeInputReads},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, ValidatedMakeNamespace},
    make_native_invocation::NativeMakeInvocation,
    measure_tree_content_cas, sha256_bytes,
};
use std::{collections::BTreeMap, path::Path};

const ROOT_INPUT: &str = "root.mk";
const SCALAR_EXPRESSIONS: [&str; 3] = [
    "$(SHELL)",
    "$(.SHELLFLAGS)",
    "$(eval BATCH := staged)$(BATCH)|$(wildcard *.src)|$(realpath data.txt)",
];

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [
            (".VARIABLES", MakeVariableOrigin::Default),
            ("MAKE_VERSION", MakeVariableOrigin::Default),
            ("SHELL", MakeVariableOrigin::Default),
            (".SHELLFLAGS", MakeVariableOrigin::Default),
            ("MAKEFLAGS", MakeVariableOrigin::Default),
            ("MFLAGS", MakeVariableOrigin::Default),
            ("MAKELEVEL", MakeVariableOrigin::Default),
            ("MAKEOVERRIDES", MakeVariableOrigin::Default),
            ("CLI", MakeVariableOrigin::CommandLine),
        ]
        .into_iter()
        .map(|(name, origin)| MakeNamespaceEntry {
            name: name.into(),
            origin,
        })
        .collect(),
    })
    .unwrap()
}

fn make_paths<'a>(dirs: &'a DirVars, root: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn snapshot(root: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        root,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

fn initial_values() -> [MakeInitialValue<'static>; 7] {
    [
        MakeInitialValue {
            name: "SHELL",
            value: "/incoming/shell",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
        MakeInitialValue {
            name: ".SHELLFLAGS",
            value: "-c",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
        MakeInitialValue {
            name: "MAKEFLAGS",
            value: "",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
        MakeInitialValue {
            name: "MFLAGS",
            value: "",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
        MakeInitialValue {
            name: "MAKELEVEL",
            value: "0",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
        MakeInitialValue {
            name: "CLI",
            value: "command-line",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::CommandLine,
        },
        MakeInitialValue {
            name: "MAKEOVERRIDES",
            value: "",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Default,
        },
    ]
}

fn invocation(root: &Path) -> NativeMakeInvocation {
    NativeMakeInvocation::new(
        [
            "-r".into(),
            "-f".into(),
            ROOT_INPUT.into(),
            "subdir/target.out".into(),
            "CLI=command-line".into(),
        ],
        root.to_owned(),
        [("SHELL".into(), "/incoming/shell".into())],
    )
    .unwrap()
}

fn source_program(
    paths: &MakeProgramPaths<'_>,
    selected: &MakeInputSnapshot,
    invocation: &NativeMakeInvocation,
) -> MakeVariableProgram {
    let mut program =
        MakeVariableProgram::new_source_program(paths, &namespace(), &initial_values()).unwrap();
    program
        .read_captured_file(ROOT_INPUT, paths, selected)
        .unwrap();
    program.state.native_recipe_context_sha256 = Some(sha256_bytes(
        &invocation
            .to_bytes()
            .expect("serialize the synthetic bound invocation"),
    ));
    program
}

fn read_identities(reads: &VerifiedMakeInputReads) -> Vec<String> {
    reads
        .reads()
        .iter()
        .map(|read| read.identity.clone())
        .collect()
}

#[test]
fn direct_recipe_batch_matches_standalone_values_exports_and_serial_environment() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    std::fs::write(root.join("a.src"), "source\n").unwrap();
    std::fs::write(root.join("data.txt"), "captured data\n").unwrap();
    std::fs::write(
        root.join(ROOT_INPUT),
        concat!(
            "SHELL := /selected/posix-shell\n",
            ".SHELLFLAGS := -ec\n",
            "FRAME := global\n",
            "CLI := file-value\n",
            "subdir/target.out: FRAME := target\n",
            "subdir/target.out: CLI := target-value\n",
            "other.out: FRAME := other\n",
            "other.out: CLI := other-value\n",
            "export FRAME_EXPORT = $(FRAME)|$(CLI)|$@|$<|$(@D)|$(@F)\n",
        ),
    )
    .unwrap();
    let selected = snapshot(&root);
    let dirs = DirVars::default();
    let paths = make_paths(&dirs, &root);
    let native = invocation(&root);
    let automatic_one = BTreeMap::from([
        ("@".into(), "subdir/target.out".into()),
        ("*".into(), "target".into()),
        ("<".into(), "source.in".into()),
        ("@D".into(), "subdir".into()),
        ("@F".into(), "target.out".into()),
    ]);
    let automatic_two = BTreeMap::from([
        ("@".into(), "other.out".into()),
        ("*".into(), "other".into()),
        ("<".into(), "other.in".into()),
        ("@D".into(), ".".into()),
        ("@F".into(), "other.out".into()),
    ]);
    let chain_one = ["subdir/target.out"];
    let chain_two = ["other.out"];
    let requests = [
        MakeTargetRecipeRequest {
            chain: &chain_one,
            expression: "$(BATCH)|$(FRAME)|$(CLI)|$@|$*|$<|$(@D)|$(@F)|$(wildcard *.src)|$(realpath data.txt)",
            automatic_variables: &automatic_one,
        },
        MakeTargetRecipeRequest {
            chain: &chain_two,
            expression: "$(FRAME)|$(CLI)|$@|$*|$<|$(@D)|$(@F)|$(wildcard *.src)|$(realpath data.txt)",
            automatic_variables: &automatic_two,
        },
    ];

    let mut standalone = source_program(&paths, &selected, &native);
    let (standalone_scalars, standalone_scalar_reads) = standalone
        .evaluate_many_captured(&SCALAR_EXPRESSIONS, &paths, &selected)
        .unwrap();
    let standalone_recipes = requests
        .iter()
        .map(|request| {
            standalone
                .evaluate_captured_direct_recipe_with_overrides(
                    *request,
                    &BTreeMap::new(),
                    &paths,
                    &selected,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();

    let mut composed = source_program(&paths, &selected, &native);
    let (composed_scalars, composed_scalar_reads, composed_recipes) = composed
        .with_captured_direct_recipes(
            &SCALAR_EXPRESSIONS,
            &requests,
            &paths,
            &selected,
            |(values, scalar_reads), recipes| Ok((values, scalar_reads, recipes)),
        )
        .unwrap();

    assert_eq!(composed_scalars, standalone_scalars);
    assert_eq!(composed_scalar_reads, standalone_scalar_reads);
    assert_eq!(composed_recipes.len(), standalone_recipes.len());
    for ((composed_projection, composed_reads), (standalone_projection, standalone_reads)) in
        composed_recipes.iter().zip(&standalone_recipes)
    {
        assert_eq!(
            composed_projection.expression(),
            standalone_projection.expression()
        );
        assert_eq!(
            composed_projection.source_exports(),
            standalone_projection.source_exports()
        );
        assert_eq!(composed_reads, standalone_reads);
        let composed_environment = composed_projection
            .serial_native_environment(&native)
            .expect("synthetic native serial environment is fully measured");
        let standalone_environment = standalone_projection
            .serial_native_environment(&native)
            .expect("standalone serial environment is fully measured");
        assert_eq!(composed_environment, standalone_environment);
        assert_eq!(composed_environment["MAKELEVEL"], "1");
        assert_eq!(composed_environment["SHELL"], "/incoming/shell");
        assert_eq!(
            composed_environment["FRAME_EXPORT"],
            composed_projection.source_exports()["FRAME_EXPORT"]
        );
    }
    assert_eq!(composed_scalars[0], "/selected/posix-shell");
    assert_eq!(composed_scalars[1], "-ec");
    assert_eq!(
        composed_scalars[2],
        format!("staged|a.src|{}", root.join("data.txt").display())
    );
    // Wildcard and realpath consult the selected snapshot's path inventory;
    // neither operation reads a Make input file or adds a file-read identity.
    assert!(read_identities(&composed_scalar_reads).is_empty());
    assert!(composed_recipes
        .iter()
        .all(|(_, reads)| read_identities(reads).is_empty()));
    assert_eq!(
        composed_recipes[0].0.expression(),
        format!(
            "staged|target|command-line|subdir/target.out|target|source.in|subdir|target.out|a.src|{}",
            root.join("data.txt").display()
        )
    );
    assert_eq!(
        composed_recipes[1].0.expression(),
        format!(
            "other|command-line|other.out|other|other.in|.|other.out|a.src|{}",
            root.join("data.txt").display()
        )
    );
    assert_eq!(
        composed_recipes[0].0.source_exports()["FRAME_EXPORT"],
        "target|command-line|subdir/target.out|source.in|subdir|target.out"
    );
    assert_eq!(
        composed_recipes[1].0.source_exports()["FRAME_EXPORT"],
        "other|command-line|other.out|other.in|.|other.out"
    );
    assert_eq!(composed.events(), standalone.events());
    assert_eq!(composed.evaluate("$(BATCH)", &paths).unwrap(), "staged");
}

#[test]
fn direct_batch_consumer_and_late_input_failures_roll_back_staged_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("seed.txt"), "before\n").unwrap();
    let selected = snapshot(&root);
    let dirs = DirVars::default();
    let paths = make_paths(&dirs, &root);
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let automatic = BTreeMap::from([("@".into(), "target.out".into())]);
    let chain = ["target.out"];
    let request = MakeTargetRecipeRequest {
        chain: &chain,
        expression: "$(LEAK)|$@",
        automatic_variables: &automatic,
    };
    let before_events = program.events().to_vec();
    let before_work = program.state_work.get();
    let rejected = program.with_captured_direct_recipes(
        &["$(eval LEAK := staged)$(LEAK)"],
        &[request],
        &paths,
        &selected,
        |_, _| Err::<(), String>("consumer rejected complete batch".into()),
    );
    assert_eq!(rejected.unwrap_err(), "consumer rejected complete batch");
    assert_eq!(program.events(), before_events);
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
    assert!(program.state_work.get() > before_work);

    let rejected_shell_effect = program.with_captured_direct_recipes(
        &["$(shell touch forbidden-effect)"],
        &[request],
        &paths,
        &selected,
        |_, _| Ok(()),
    );
    assert!(rejected_shell_effect.is_err());
    assert!(!root.join("forbidden-effect").exists());
    assert_eq!(program.events(), before_events);

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("seed.txt"), "before\n").unwrap();
    let selected = snapshot(&root);
    let paths = make_paths(&dirs, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let request = MakeTargetRecipeRequest {
        expression: "$(LEAK)|$@|$(wildcard *.txt)",
        ..request
    };
    let before_events = program.events().to_vec();
    let before_work = program.state_work.get();
    let failure = program.with_captured_direct_recipes(
        &["$(eval LEAK := staged)$(LEAK)"],
        &[request],
        &paths,
        &selected,
        |_, _| {
            std::fs::write(root.join("late.txt"), "changed during consumer\n")
                .map_err(|error| error.to_string())?;
            Ok(())
        },
    );
    assert!(
        failure.is_err(),
        "late snapshot drift must discard the batch"
    );
    assert_eq!(program.events(), before_events);
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
    assert!(program.state_work.get() > before_work);
}

#[test]
fn direct_batch_rejects_bad_frames_cwd_changed_roots_and_count_overflow() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let foreign_cwd = root.join("foreign-cwd");
    std::fs::create_dir(&foreign_cwd).unwrap();
    std::fs::write(root.join("seed.txt"), "before\n").unwrap();
    let selected = snapshot(&root);
    let dirs = DirVars::default();
    let paths = make_paths(&dirs, &root);
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let automatic = BTreeMap::from([("@".into(), "target.out".into())]);
    let chain = ["target.out"];
    let request = MakeTargetRecipeRequest {
        chain: &chain,
        expression: "$@",
        automatic_variables: &automatic,
    };
    let before = program.events().to_vec();
    let before_work = program.state_work.get();

    let inherited = ["parent", "target.out"];
    let invalid_frame = MakeTargetRecipeRequest {
        chain: &inherited,
        ..request
    };
    assert!(program
        .with_captured_direct_recipes(
            &["$(eval LEAK := staged)"],
            &[invalid_frame],
            &paths,
            &selected,
            |_, _| Ok(())
        )
        .is_err());

    let too_many = vec![""; MAX_STATEMENTS + 1];
    assert!(program
        .with_captured_direct_recipes(&too_many, &[request], &paths, &selected, |_, _| Ok(()))
        .is_err());
    assert_eq!(program.state_work.get(), before_work);

    let foreign_paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &foreign_cwd,
    };
    assert!(program
        .with_captured_direct_recipes(
            &["$(eval LEAK := staged)"],
            &[request],
            &foreign_paths,
            &selected,
            |_, _| Ok(())
        )
        .is_err());

    std::fs::write(root.join("late.txt"), "changed before evaluation\n").unwrap();
    assert!(program
        .with_captured_direct_recipes(
            &["$(eval LEAK := staged)"],
            &[request],
            &paths,
            &selected,
            |_, _| Ok(())
        )
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
}
