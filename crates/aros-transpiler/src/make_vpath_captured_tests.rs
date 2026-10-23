use super::*;
use crate::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
    parser::TargetContext,
};
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    measure_tree_content_cas,
};
use std::fs;

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
}

fn interpreted(source: &str) -> MakeFileProgram {
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new(""),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    };
    MakeVariableProgram::new(&TargetContext::default(), &[])
        .unwrap()
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap()
}

#[test]
fn interpreted_search_values_are_frozen_and_clears_remain_ordered() {
    let file = interpreted(concat!(
        "DIRS := first\n",
        "vpath %.c $(DIRS)\n",
        "DIRS := later\n",
        "ifeq (no,yes)\n",
        "vpath %.c inactive\n",
        "endif\n",
        "vpath %.h headers\n",
        "vpath %.c\n",
        "vpath %.c replacement\n",
    ));
    let projected = project_interpreted(&file).unwrap();
    assert_eq!(projected.entries().len(), 2);
    assert_eq!(projected.entries()[0].pattern, "%.h");
    assert_eq!(projected.entries()[1].directories, ["replacement"]);
    let file = interpreted("DIRS := first\nvpath %.c $(DIRS)\nDIRS := later\n");
    let projected = project_interpreted(&file).unwrap();
    assert_eq!(projected.entries()[0].directories, ["first"]);
    let index = projected.entries()[0].output_line;
    assert!(
        matches!(&file.events[index].kind, MakeFileEventKind::SearchPath(value) if value == "%.c first")
    );
    assert_eq!(projected.entries()[0].source_line, 1);
    assert!(project_interpreted(&interpreted("vpath %.c $$(UNRESOLVED)\n")).is_err());
}

#[test]
fn captured_search_uses_bound_cwd_selective_order_then_global_without_claiming_reads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for dir in ["first", "second", "global"] {
        fs::create_dir(root.join(dir)).unwrap();
    }
    for file in [
        "local.c",
        "first/local.c",
        "first/selected.c",
        "second/selected.c",
        "global/selected.c",
        "second/later.c",
        "global/later.c",
        "global/fallback.c",
    ] {
        fs::write(root.join(file), b"input").unwrap();
    }
    let file = interpreted("vpath %.c first\nvpath %.c second\n");
    let search = project_interpreted(&file).unwrap();
    let input = snapshot(&root);
    let (selected, reads) = input
        .transaction(|reader| {
            assert_eq!(reader.working_directory(), root);
            ["local.c", "selected.c", "later.c", "fallback.c", "absent.c"]
                .map(|name| search.find_regular_input(name, "global", reader))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    assert_eq!(
        selected,
        [
            Some(root.join("local.c")),
            Some(root.join("first/selected.c")),
            Some(root.join("second/later.c")),
            Some(root.join("global/fallback.c")),
            None
        ]
    );
    assert!(reads.reads().is_empty());
    let search = project_interpreted(&interpreted("vpath %.c missing/nested\n")).unwrap();
    let (selected, _) = input
        .transaction(|reader| search.find_regular_input("fallback.c", "global", reader))
        .unwrap();
    assert_eq!(selected, Some(root.join("global/fallback.c")));
    let absolute = root.join("first/selected.c");
    let (selected, _) = input
        .transaction(|reader| {
            search.find_regular_input(absolute.to_str().unwrap(), "invalid$(GLOBAL)", reader)
        })
        .unwrap();
    assert_eq!(selected, Some(absolute));
}

#[test]
fn nonregular_or_outside_first_candidate_never_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("second")).unwrap();
    fs::create_dir(root.join("first/unit.c")).unwrap();
    fs::write(root.join("second/unit.c"), b"input").unwrap();
    let search = project_interpreted(&interpreted("vpath %.c first second\n")).unwrap();
    let input = snapshot(&root);
    assert!(input
        .transaction(|reader| search.find_regular_input("unit.c", "", reader))
        .is_err());
    let outside = tempfile::tempdir().unwrap();
    let outside_root = outside.path().canonicalize().unwrap();
    fs::write(outside_root.join("unit.c"), b"outside").unwrap();
    let declared = project(
        &[SearchPathDeclaration {
            output_line: 0,
            source_line: 0,
            arguments: format!("%.c {} second", outside_root.display()),
        }],
        &|value, _| Ok(value.to_owned()),
    )
    .unwrap();
    assert!(input
        .transaction(|reader| declared.find_regular_input("unit.c", "", reader))
        .is_err());
}

#[test]
fn captured_search_uses_a_distinct_bound_cwd_and_refuses_a_file_ancestor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for directory in ["cwd", "first", "second"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    fs::write(root.join("unit.c"), b"wrong root input").unwrap();
    fs::write(root.join("cwd/unit.c"), b"bound cwd input").unwrap();
    fs::write(root.join("first/blocked"), b"not a directory").unwrap();
    fs::write(root.join("second/other.c"), b"later file").unwrap();
    let input = MakeInputSnapshot::capture(
        &root.join("cwd"),
        &[MakeInputRootSelection {
            root: root.clone(),
            content_sha256: measure_tree_content_cas(&root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap();
    let search =
        project_interpreted(&interpreted("vpath %.c ../first/blocked ../second\n")).unwrap();
    let (found, _) = input
        .transaction(|reader| search.find_regular_input("unit.c", "", reader))
        .unwrap();
    assert_eq!(found, Some(root.join("cwd/unit.c")));
    assert!(input
        .transaction(|reader| search.find_regular_input("other.c", "", reader))
        .is_err());
}

#[test]
fn captured_search_membership_feeds_generic_explicit_traversal_without_inventing_leaves() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("inputs")).unwrap();
    fs::write(root.join("inputs/unit.c"), b"source input").unwrap();
    let input = snapshot(&root);
    let file = interpreted("vpath %.c inputs\nall: unit.o\nunit.o: %.o: %.c\n\tcompile $< -o $@\n");
    let rules = crate::make_interpreted_rules::collect(&file)
        .unwrap()
        .project_explicit()
        .unwrap();
    let search = project_interpreted(&file).unwrap();
    let (walk, _) = input
        .transaction(|reader| {
            let reader = std::cell::RefCell::new(reader);
            crate::make_rule_graph::traversal::select_explicit(&rules.graph, &["all"], &|name| {
                search
                    .find_regular_input(name, "", &mut reader.borrow_mut())
                    .map(|found| found.is_some())
            })
        })
        .unwrap();
    let index = walk
        .visits()
        .iter()
        .position(|visit| visit.target() == "unit.c")
        .unwrap();
    assert_eq!(
        walk.scope_chain(index).unwrap(),
        ["all", "unit.o", "unit.c"]
    );
    let file = interpreted("vpath %.c inputs\nall: absent.o\nabsent.o: %.o: %.c\n");
    let rules = crate::make_interpreted_rules::collect(&file)
        .unwrap()
        .project_explicit()
        .unwrap();
    assert!(input
        .transaction(|reader| {
            let reader = std::cell::RefCell::new(reader);
            crate::make_rule_graph::traversal::select_explicit(&rules.graph, &["all"], &|name| {
                search
                    .find_regular_input(name, "", &mut reader.borrow_mut())
                    .map(|found| found.is_some())
            })
        })
        .is_err());
}

#[test]
fn interpreted_search_bounds_public_event_payloads_before_copying() {
    let mut file = interpreted("vpath %.c first\n");
    let prototype = file.events[0].clone();
    file.events = vec![prototype.clone(); MAX_SOURCE_EVENTS + 1];
    assert!(project_interpreted(&file).is_err());
    file.events = vec![prototype; MAX_ENTRIES + 1];
    assert!(project_interpreted(&file).is_err());
    file.events.truncate(1);
    file.events[0].kind = MakeFileEventKind::SearchPath("x".repeat(MAX_VALUE_BYTES + 1));
    assert!(project_interpreted(&file).is_err());
}

#[cfg(unix)]
#[test]
fn captured_search_refuses_link_instead_of_selecting_later_regular_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("second")).unwrap();
    fs::write(root.join("second/unit.c"), b"input").unwrap();
    std::os::unix::fs::symlink("../second/unit.c", root.join("first/unit.c")).unwrap();
    let input = snapshot(&root);
    let search = project_interpreted(&interpreted("vpath %.c first second\n")).unwrap();
    assert!(input
        .transaction(|reader| search.find_regular_input("unit.c", "", reader))
        .is_err());
}
