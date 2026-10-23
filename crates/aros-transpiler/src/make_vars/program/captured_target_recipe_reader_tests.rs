//! Caller-owned captured reader target recipe evaluation.

use super::super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, ValidatedMakeNamespace},
    measure_tree_content_cas,
};
use std::{collections::BTreeMap, path::Path};

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap()
}

fn paths<'a>(dirs: &'a DirVars, root: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn snapshot(root: &Path, cwd: &Path) -> MakeInputSnapshot {
    let content_sha256 = measure_tree_content_cas(root)
        .unwrap()
        .payload_digest_excluding(None);
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: root.into(),
            content_sha256,
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

#[test]
fn reader_target_recipe_batch_checks_cwd_and_keeps_failed_state_staged() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let foreign_cwd = root.join("foreign-cwd");
    std::fs::create_dir(&foreign_cwd).unwrap();
    std::fs::write(
        root.join("root.mk"),
        "FLAGS = global\nparent: FLAGS = parent\nexport FRAME = $(FLAGS)|$@|$<\n",
    )
    .unwrap();

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let selected = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &selected)
        .unwrap();
    let before = program.events().to_vec();

    let automatic = BTreeMap::from([("@".into(), "one.out".into()), ("<".into(), "one.c".into())]);
    let chain = ["parent", "one.out"];
    let request = MakeTargetRecipeRequest {
        chain: &chain,
        expression: "$@ $(FLAGS)",
        automatic_variables: &automatic,
    };
    let (values, _) = selected
        .transaction(|reader| {
            program.evaluate_many_captured_target_recipes_in_reader(&[request], &paths, reader)
        })
        .unwrap();
    assert_eq!(values[0].expression(), "one.out parent");
    assert_eq!(values[0].source_exports()["FRAME"], "parent|one.out|one.c");

    let foreign = snapshot(&root, &foreign_cwd);
    assert!(foreign
        .transaction(|reader| {
            program.evaluate_many_captured_target_recipes_in_reader(&[request], &paths, reader)
        })
        .is_err());

    let failed = MakeTargetRecipeRequest {
        expression: "$(eval FLAGS := changed)",
        ..request
    };
    assert!(selected
        .transaction(|reader| {
            program.evaluate_many_captured_target_recipes_in_reader(&[failed], &paths, reader)
        })
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(FLAGS)", &paths).unwrap(), "global");
}
