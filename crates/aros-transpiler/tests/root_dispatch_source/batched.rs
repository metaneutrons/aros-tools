//! Regression coverage for the composed Root recipe projection transaction.

use super::*;
use aros_common::{
    make_input_snapshot::VerifiedMakeInputReads,
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, ValidatedMakeNamespace},
};

fn source_with_controls(extra: &str) -> String {
    root_source(
        &format!("SHELL := /selected/posix-shell\n.SHELLFLAGS := -ec\n{extra}"),
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    )
}

fn source_namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [
            (".VARIABLES", MakeVariableOrigin::Default),
            ("MAKE_VERSION", MakeVariableOrigin::Default),
            ("SHELL", MakeVariableOrigin::Default),
            (".SHELLFLAGS", MakeVariableOrigin::Default),
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

fn source_program(paths: &MakeProgramPaths<'_>) -> MakeVariableProgram {
    MakeVariableProgram::new_source_program(
        paths,
        &source_namespace(),
        &[
            MakeInitialValue {
                name: "SHELL",
                value: "/bin/sh",
                flavor: MakeInitialFlavor::Simple,
                origin: MakeVariableOrigin::Default,
            },
            MakeInitialValue {
                name: ".SHELLFLAGS",
                value: "-c",
                flavor: MakeInitialFlavor::Simple,
                origin: MakeVariableOrigin::Default,
            },
        ],
    )
    .unwrap()
}

fn derive(
    fixture: &Fixture,
    source: &str,
    program: &mut MakeVariableProgram,
    paths: &MakeProgramPaths<'_>,
    snapshot: &aros_common::make_input_snapshot::MakeInputSnapshot,
) -> Result<SourceCheckedRootDispatchPlan, String> {
    let startup = LiteralMakeStartupArguments::admit(&[
        "-r".into(),
        "-f".into(),
        ROOT_INPUT.into(),
        GOAL.into(),
    ])
    .unwrap();
    RootDispatchCapturePlan::derive_captured(
        ROOT_INPUT,
        &expected_sha256(source),
        fixture.selected(),
        program,
        paths,
        snapshot,
        &startup,
    )
}

fn identities(reads: &VerifiedMakeInputReads) -> Vec<String> {
    reads
        .reads()
        .iter()
        .map(|read| read.identity.clone())
        .collect()
}

#[test]
fn composed_dispatch_matches_the_standalone_projection_sequence() {
    let fixture = Fixture::new();
    let source = source_with_controls("");
    fixture.write_inputs(&source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &fixture.root,
    };

    let mut standalone_program = source_program(&paths);
    let standalone = derive(
        &fixture,
        &source,
        &mut standalone_program,
        &paths,
        &snapshot,
    )
    .unwrap();
    let standalone = standalone
        .with_captured_recipe_controls(&mut standalone_program, &paths)
        .unwrap();
    let standalone = standalone
        .with_original_recipe(&mut standalone_program, &paths)
        .unwrap();
    let (standalone_recorder, standalone_recorder_reads) = standalone
        .project_instrumented_recipe(&mut standalone_program, &paths)
        .unwrap();

    let mut composed_program = source_program(&paths);
    let composed = derive(&fixture, &source, &mut composed_program, &paths, &snapshot).unwrap();
    let (composed, composed_recorder, composed_recorder_reads) = composed
        .with_dispatch_recipe_projections(&mut composed_program, &paths)
        .unwrap();

    let standalone_controls = standalone.recipe_controls().unwrap();
    let composed_controls = composed.recipe_controls().unwrap();
    assert_eq!(composed_controls.shell(), "/selected/posix-shell");
    assert_eq!(composed_controls.flags(), "-ec");
    assert_eq!(composed_controls.shell(), standalone_controls.shell());
    assert_eq!(composed_controls.flags(), standalone_controls.flags());
    assert_eq!(composed_controls.reads(), standalone_controls.reads());
    assert_eq!(
        composed.original_recipe().unwrap().expression(),
        standalone.original_recipe().unwrap().expression()
    );
    assert_eq!(
        composed.original_recipe().unwrap().source_exports(),
        standalone.original_recipe().unwrap().source_exports()
    );
    assert_eq!(
        composed.original_recipe_reads(),
        standalone.original_recipe_reads()
    );
    assert_eq!(
        composed_recorder.expression(),
        standalone_recorder.expression()
    );
    assert_eq!(
        composed_recorder.source_exports(),
        standalone_recorder.source_exports()
    );
    assert_eq!(composed_recorder_reads, standalone_recorder_reads);
    assert_eq!(
        identities(composed.source_reads()),
        vec![ROOT_INPUT.to_owned(), "roles.mk".to_owned()]
    );
    assert_eq!(composed_program.events(), standalone_program.events());
    fixture.assert_recorder_absent();
}

#[test]
fn dispatch_batch_refuses_compound_or_effectful_source_without_running_it() {
    let fixture = Fixture::new();
    let compound = root_source(
        "SHELL := /selected/posix-shell\n.SHELLFLAGS := -ec\n",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@; touch forbidden-command",
    );
    fixture.write_inputs(&compound, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    assert!(capture(
        &fixture,
        &expected_sha256(&compound),
        fixture.selected(),
        &snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();
    assert!(!fixture.root.join("forbidden-command").exists());

    let fixture = Fixture::new();
    let effectful = source_with_controls("export PROOF = $(eval LEAK := changed)\n");
    fixture.write_inputs(&effectful, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &fixture.root,
    };
    let mut program = source_program(&paths);
    let plan = derive(&fixture, &effectful, &mut program, &paths, &snapshot).unwrap();
    let before = program.events().to_vec();
    assert!(plan
        .with_dispatch_recipe_projections(&mut program, &paths)
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
    assert!(!fixture.root.join("forbidden-command").exists());
    fixture.assert_recorder_absent();

    let fixture = Fixture::new();
    let empty_shell = root_source(
        "SHELL :=\n.SHELLFLAGS := -ec\n",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&empty_shell, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &fixture.root,
    };
    let mut program = source_program(&paths);
    let plan = derive(&fixture, &empty_shell, &mut program, &paths, &snapshot).unwrap();
    let Err(error) = plan.with_dispatch_recipe_projections(&mut program, &paths) else {
        panic!("an empty measured SHELL must refuse dispatch projection");
    };
    assert_eq!(error, "Root recipe controls require a measured shell");
    fixture.assert_recorder_absent();
}

#[test]
fn dispatch_batch_rejects_a_different_working_directory_before_projection() {
    let fixture = Fixture::new();
    let source = source_with_controls("");
    fixture.write_inputs(&source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &fixture.root,
    };
    let mut program = source_program(&paths);
    let plan = derive(&fixture, &source, &mut program, &paths, &snapshot).unwrap();
    let before = program.events().to_vec();
    let other_cwd = fixture.root.join("other-cwd");
    fs::create_dir(&other_cwd).unwrap();
    let wrong_paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &other_cwd,
    };

    assert!(plan
        .with_dispatch_recipe_projections(&mut program, &wrong_paths)
        .is_err());
    assert_eq!(program.events(), before);
    fixture.assert_recorder_absent();
}
