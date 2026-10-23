//! Actual Source archive pipeline -> generic Make -> literal shell AST vs GNU dry run.
//! The selected archive recipe is data; neither GCA_AR nor an archive is run.

use super::*;
use crate::{
    dirs::DirVars,
    make_interpreted_rules,
    make_vars::{MakeProgramPaths, MakeTargetRecipeRequest, MakeVariableProgram},
};
use aros_common::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    },
    measure_tree_content_cas, sha256_bytes,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const SOURCE_RECIPE: &str = "arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src";
const ARCHIVE_TARGET: &str = "private-v3d-archive.a";

#[test]
fn actual_source_gca_archive_pipeline_matches_gnu_dry_run_and_binds_tool_argument() {
    let source_root = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT").expect("real Source fixture is required"),
    )
    .canonicalize()
    .unwrap();
    let source_path = source_root.join(SOURCE_RECIPE);
    let source_bytes = std::fs::read(&source_path).unwrap();
    let source_sha256 = sha256_bytes(&source_bytes);
    let source_text = std::str::from_utf8(&source_bytes).unwrap();
    let pipelines: Vec<_> = source_text
        .lines()
        .filter(|line| line.contains("| xargs"))
        .collect();
    assert_eq!(
        pipelines.len(),
        1,
        "expected one pipeline at the V3D Source coordinate"
    );
    let source_recipe = pipelines[0];
    assert_eq!(
        source_recipe,
        "\t$(Q)cat $(GCA_GENDIR)/gca_private.list | xargs $(GCA_AR) rs $@"
    );

    let make_root = tempfile::tempdir().unwrap();
    let make_root = make_root.path().canonicalize().unwrap();
    let make_root_text = make_root.to_str().unwrap();
    assert!(!make_root_text.chars().any(char::is_whitespace));

    let program_temp = tempfile::tempdir().unwrap();
    let program_root = program_temp.path().canonicalize().unwrap();
    let gca_ar = program_root.join("captured-gca-ar");
    std::fs::write(
        &gca_ar,
        b"immutable synthetic archiver data; never executed",
    )
    .unwrap();
    std::fs::set_permissions(&gca_ar, std::fs::Permissions::from_mode(0o700)).unwrap();
    let gca_ar_text = gca_ar.to_str().unwrap();
    // The Source recipe does not quote $(GCA_AR), so keep its synthetic value
    // free of whitespace instead of changing the original recipe to add quotes.
    assert!(!gca_ar_text.chars().any(char::is_whitespace));
    let programs = CapturedProgramSet::capture(
        &[ProgramRootSelection {
            root: program_root.clone(),
            content_sha256: measure_tree_content_cas(&program_root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
        &[ProgramSelection {
            role: "gca-ar".into(),
            root_index: 0,
            invocation_path: gca_ar.clone(),
            executable_sha256: sha256_bytes(&std::fs::read(&gca_ar).unwrap()),
        }],
    )
    .unwrap();

    let synthetic_makefile = format!(
        "Q := @\nGCA_GENDIR := generated\nGCA_AR := {gca_ar_text}\nroot: {ARCHIVE_TARGET}\n{ARCHIVE_TARGET}:\n{source_recipe}\n"
    );
    std::fs::write(make_root.join("input.mk"), synthetic_makefile).unwrap();
    let snapshot = MakeInputSnapshot::capture(
        &make_root,
        &[MakeInputRootSelection {
            root: make_root.clone(),
            content_sha256: measure_tree_content_cas(&make_root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &make_root,
        relative_dir: Path::new(""),
        top_directory: &make_root,
        working_directory: &make_root,
    };
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
    let mut variables = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let (file, _) = variables
        .read_captured_file("input.mk", &paths, &snapshot)
        .unwrap();
    let interpreted = make_interpreted_rules::collect(&file).unwrap();
    let rules: Vec<_> = interpreted
        .rules
        .iter()
        .flat_map(|rule| rule.instantiate_explicit().unwrap())
        .collect();
    let archive = rules
        .iter()
        .find(|rule| rule.target == ARCHIVE_TARGET)
        .unwrap();
    let automatic = archive.automatic_variables();
    let chain = ["root", ARCHIVE_TARGET];
    let requests: Vec<_> = archive
        .recipes
        .iter()
        .map(|recipe| MakeTargetRecipeRequest {
            chain: &chain,
            expression: &recipe.text,
            automatic_variables: &automatic,
        })
        .collect();
    assert_eq!(requests.len(), 1);
    let (values, _) = variables
        .evaluate_many_captured_target_recipes(&requests, &paths, &snapshot)
        .unwrap();
    assert_eq!(values.len(), 1);
    let shell = values[0].parse_literal_shell().unwrap();
    assert!(shell.prefixes.silent);
    let [LiteralRecipeStatement::Pipeline(stages)] = shell.statements.as_slice() else {
        panic!("actual Source archive recipe did not produce one pipeline")
    };
    assert_eq!(stages.len(), 2);
    assert_eq!(
        stages[0]
            .words()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["cat", "generated/gca_private.list"]
    );
    assert_eq!(
        stages[1]
            .words()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["xargs", gca_ar_text, "rs", ARCHIVE_TARGET]
    );
    assert!(stages.iter().all(|stage| stage.redirections().is_empty()));

    let bindings = crate::make_shell_data::program_binding::bind_literal_programs(
        &shell.statements,
        &programs,
    )
    .unwrap();
    assert_eq!(bindings.commands().len(), 2);
    assert_eq!(bindings.unresolved().count(), 2);
    assert_eq!(
        bindings.commands()[0].position(),
        [
            crate::make_shell_data::program_binding::RecipeProgramStep::Statement(0),
            crate::make_shell_data::program_binding::RecipeProgramStep::PipelineStage(0),
        ]
    );
    let xargs = &bindings.commands()[1];
    assert_eq!(xargs.command().words()[0], "xargs");
    assert_eq!(
        xargs.position(),
        [
            crate::make_shell_data::program_binding::RecipeProgramStep::Statement(0),
            crate::make_shell_data::program_binding::RecipeProgramStep::PipelineStage(1),
        ]
    );
    assert_eq!(
        xargs.argument_role(1, "gca-ar").unwrap().invocation_path(),
        gca_ar
    );

    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let make_program = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(make))
        .find(|path| path.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    let make_sha256 = sha256_bytes(&std::fs::read(&make_program).unwrap());
    let oracle = Command::new(&make_program)
        .args([
            "-rR",
            "-n",
            "--no-print-directory",
            "-f",
            "input.mk",
            "root",
        ])
        .env_clear()
        .current_dir(&make_root)
        .output()
        .unwrap();
    assert!(oracle.status.success(), "GNU dry run failed");
    let native = parse_literal_recipe(std::str::from_utf8(&oracle.stdout).unwrap()).unwrap();
    assert_eq!(shell.statements, native);
    assert!(!make_root.join(ARCHIVE_TARGET).exists());
    assert!(!make_root.join("generated/gca_private.list").exists());

    assert_eq!(source_bytes, std::fs::read(&source_path).unwrap());
    assert_eq!(
        source_sha256,
        sha256_bytes(&std::fs::read(&source_path).unwrap())
    );
    assert_eq!(
        make_sha256,
        sha256_bytes(&std::fs::read(&make_program).unwrap())
    );
    snapshot.verify().unwrap();
    programs.verify().unwrap();
}
