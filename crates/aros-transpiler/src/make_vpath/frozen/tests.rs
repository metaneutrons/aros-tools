use super::*;
use crate::{dirs::DirVars, make_vars::MakeFileProgram, parser::TargetContext};
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    measure_tree_content_cas,
};
use std::fs;

fn paths<'a>(dirs: &'a DirVars, root: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn program(source: &str, paths: &MakeProgramPaths<'_>) -> (MakeVariableProgram, MakeFileProgram) {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let file = program
        .read_file("root.mk", source, paths, &|_, _| Ok(None))
        .unwrap();
    (program, file)
}

fn snapshot(root: &Path, cwd: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
}

#[test]
fn global_search_stays_separate_from_first_parent_target_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for directory in ["general", "selective", "alpha", "beta"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    for file in [
        "general/shared.dat",
        "alpha/shared.dat",
        "beta/shared.dat",
        "general/probe.dat",
        "selective/probe.dat",
    ] {
        fs::write(root.join(file), b"fixture").unwrap();
    }
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let (mut program, file) = program(
        concat!(
            "VPATH := early\n",
            "vpath probe.dat selective\n",
            "VPATH := general\n",
            "left: VPATH = alpha\nleft: shared\n",
            "right: VPATH = beta\nright: shared\n",
            "shared: shared.dat\n",
        ),
        &paths,
    );
    let projection = crate::make_interpreted_rules::collect(&file)
        .unwrap()
        .project_explicit()
        .unwrap();
    let frozen = super::super::project_interpreted(&file)
        .unwrap()
        .freeze_global(&mut program, &paths)
        .unwrap();
    assert_eq!(frozen.global_vpath(), "general");
    let input = snapshot(&root, &root);
    for (goals, expected) in [(["left", "right"], "alpha"), (["right", "left"], "beta")] {
        let trace =
            crate::make_rule_graph::traversal::trace_declared(&projection.graph, &goals).unwrap();
        let leaf = trace.unresolved_inputs().next().unwrap().0;
        assert_eq!(
            program
                .evaluate_target_contexts(
                    &[trace.scope_chain(leaf).unwrap()],
                    &["$(VPATH)"],
                    &paths
                )
                .unwrap(),
            [vec![expected.to_owned()]]
        );
        let (rows, reads) = input
            .transaction(|reader| frozen.observe_declared_inputs(&trace, reader))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].visit(), leaf);
        assert_eq!(
            rows[0].observed(),
            Some(root.join("general/shared.dat").as_path())
        );
        assert!(reads.reads().is_empty());
    }
    let (found, _) = input
        .transaction(|reader| frozen.find_regular_input("probe.dat", reader))
        .unwrap();
    assert_eq!(found, Some(root.join("selective/probe.dat")));
    program
        .read_file("later.mk", "VPATH := changed\n", &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(frozen.global_vpath(), "general");
}

#[test]
fn declared_frontier_preserves_safe_absence_and_refuses_foreign_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("cwd")).unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let (mut program, file) = program(
        "VPATH := missing/nested\nall: absent.c\ncomplete:\n",
        &paths,
    );
    let projection = crate::make_interpreted_rules::collect(&file)
        .unwrap()
        .project_explicit()
        .unwrap();
    let trace =
        crate::make_rule_graph::traversal::trace_declared(&projection.graph, &["all"]).unwrap();
    let frozen = super::super::project_interpreted(&file)
        .unwrap()
        .freeze_global(&mut program, &paths)
        .unwrap();
    let (rows, _) = snapshot(&root, &root)
        .transaction(|reader| frozen.observe_declared_inputs(&trace, reader))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].observed(), None);
    assert_eq!(trace.unresolved_inputs().count(), 1);
    assert!(snapshot(&root, &root.join("cwd"))
        .transaction(|reader| frozen.observe_declared_inputs(&trace, reader))
        .unwrap_err()
        .contains("cwd differs"));
    let empty_trace =
        crate::make_rule_graph::traversal::trace_declared(&projection.graph, &["complete"])
            .unwrap();
    assert_eq!(empty_trace.unresolved_inputs().count(), 0);
    assert!(snapshot(&root, &root)
        .transaction(|reader| frozen.observe_declared_inputs(&empty_trace, reader))
        .unwrap()
        .0
        .is_empty());
    assert!(snapshot(&root, &root.join("cwd"))
        .transaction(|reader| frozen.observe_declared_inputs(&empty_trace, reader))
        .unwrap_err()
        .contains("cwd differs"));
}

#[test]
fn unsafe_first_hit_refuses_the_entire_declared_batch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for directory in ["first", "second", "first/blocked.c"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    fs::write(root.join("first/good.c"), b"good").unwrap();
    fs::write(root.join("second/blocked.c"), b"later").unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let (mut program, file) = program("VPATH := first second\nall: good.c blocked.c\n", &paths);
    let projection = crate::make_interpreted_rules::collect(&file)
        .unwrap()
        .project_explicit()
        .unwrap();
    let trace =
        crate::make_rule_graph::traversal::trace_declared(&projection.graph, &["all"]).unwrap();
    let frozen = super::super::project_interpreted(&file)
        .unwrap()
        .freeze_global(&mut program, &paths)
        .unwrap();
    assert!(snapshot(&root, &root)
        .transaction(|reader| frozen.observe_declared_inputs(&trace, reader))
        .unwrap_err()
        .contains("not a captured regular input"));
}

#[test]
fn freeze_refuses_effects_filesystem_functions_and_unsafe_global_words() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    for source in [
        "VPATH = $(eval CHANGED := yes)\n",
        "VPATH = $(wildcard *)\n",
        "VPATH = $(UNMEASURED)\n",
        "VPATH := bad$$path\n",
        "VPATH := unresolved%path\n",
    ] {
        let (mut program, file) = program(&format!("CHANGED := before\n{source}"), &paths);
        assert!(super::super::project_interpreted(&file)
            .unwrap()
            .freeze_global(&mut program, &paths)
            .is_err());
        assert_eq!(
            program
                .evaluate_target_contexts(&[vec![]], &["$(CHANGED)"], &paths)
                .unwrap(),
            [vec!["before".to_owned()]]
        );
    }
}

#[test]
fn measured_empty_global_path_keeps_literal_cwd_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("unit.c"), b"local").unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let (mut program, file) = program("VPATH :=\nall: unit.c\n", &paths);
    let frozen = super::super::project_interpreted(&file)
        .unwrap()
        .freeze_global(&mut program, &paths)
        .unwrap();
    assert_eq!(frozen.global_vpath(), "");
    let (found, _) = snapshot(&root, &root)
        .transaction(|reader| frozen.find_regular_input("unit.c", reader))
        .unwrap();
    assert_eq!(found, Some(root.join("unit.c")));
}
