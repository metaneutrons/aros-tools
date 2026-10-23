//! Explicit external input roots remain bounded and transactional.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::ValidatedMakeNamespace,
    measure_tree_content_cas, sha256_bytes,
};
use std::path::{Path, PathBuf};

struct Roots {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    build: PathBuf,
    dependency: PathBuf,
}

impl Roots {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path();
        let source = base.join("source");
        let build = base.join("build");
        let dependency = base.join("dependency");
        for path in [&source, &build, &dependency] {
            std::fs::create_dir(path).unwrap();
        }
        std::fs::write(source.join("marker.data"), "source root\n").unwrap();
        std::fs::write(build.join("marker.data"), "build root\n").unwrap();
        std::fs::write(dependency.join("marker.data"), "dependency root\n").unwrap();
        Self {
            source: source.canonicalize().unwrap(),
            build: build.canonicalize().unwrap(),
            dependency: dependency.canonicalize().unwrap(),
            _temporary: temporary,
        }
    }

    fn selected(&self, include_dependency: bool) -> Vec<MakeInputRootSelection> {
        let mut selections = vec![selection(&self.source), selection(&self.build)];
        if include_dependency {
            selections.push(selection(&self.dependency));
        }
        selections
    }

    fn paths<'a>(&'a self, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
        MakeProgramPaths {
            dirs,
            source_root: &self.source,
            relative_dir: Path::new(""),
            top_directory: &self.build,
            working_directory: &self.build,
        }
    }
}

fn selection(root: &Path) -> MakeInputRootSelection {
    let expected = measure_tree_content_cas(root).unwrap();
    MakeInputRootSelection {
        root: root.to_owned(),
        content_sha256: expected.payload_digest_excluding(None),
    }
}

fn capture(working_directory: &Path, selections: &[MakeInputRootSelection]) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(working_directory, selections)
        .unwrap()
        .with_include_directories(&[])
        .unwrap()
}

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::from_measurement("v1:2;.VARIABLES|default;MAKE_VERSION|default;end")
        .unwrap()
}

fn source_file(root: &Path, name: &str) -> String {
    root.join(name).to_str().unwrap().to_owned()
}

#[test]
fn explicitly_selected_dependency_root_supplies_wildcards_and_proves_missing_libraries() {
    let roots = Roots::new();
    let library_directory = roots.dependency.join("libraries");
    std::fs::create_dir(&library_directory).unwrap();
    for name in ["alpha.library", "beta.library"] {
        std::fs::write(library_directory.join(name), "fixture\n").unwrap();
    }
    let input = roots.source.join("root.mk");
    std::fs::write(
        &input,
        format!(
            "FOUND := $(wildcard {}/libraries/*.library)\nMISSING := $(wildcard {}/libraries/absent-*.library)\n",
            roots.dependency.display(),
            roots.dependency.display(),
        ),
    )
    .unwrap();

    let selections: Vec<MakeInputRootSelection> = roots.selected(true);
    let snapshot = capture(&roots.build, &selections);
    let dirs = DirVars::default();
    let paths = roots.paths(&dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    let (file, reads) = program
        .read_captured_file(&source_file(&roots.source, "root.mk"), &paths, &snapshot)
        .unwrap();

    assert_eq!(file.events.len(), 2);
    assert_eq!(
        program.evaluate("$(FOUND)", &paths).unwrap(),
        format!(
            "{}/libraries/alpha.library {}/libraries/beta.library",
            roots.dependency.display(),
            roots.dependency.display(),
        )
    );
    assert_eq!(program.evaluate("$(MISSING)", &paths).unwrap(), "");
    assert_eq!(reads.reads().len(), 1);
    assert_eq!(reads.reads()[0].path, roots.source.join("root.mk"));
}

#[test]
fn omitted_dependency_root_refuses_atomically_and_preserves_prior_values_and_events() {
    let roots = Roots::new();
    let good = roots.source.join("good.mk");
    let bad = roots.source.join("bad.mk");
    std::fs::write(&good, "VALUE := original\n").unwrap();
    std::fs::write(
        &bad,
        format!(
            "VALUE := changed\nFILES := $(wildcard {}/libraries/*.library)\n",
            roots.dependency.display(),
        ),
    )
    .unwrap();
    let selections: Vec<MakeInputRootSelection> = roots.selected(false);
    let snapshot = capture(&roots.build, &selections);
    let dirs = DirVars::default();
    let paths = roots.paths(&dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file(&source_file(&roots.source, "good.mk"), &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();

    assert!(program
        .read_captured_file(&source_file(&roots.source, "bad.mk"), &paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), events);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
}

#[test]
fn capture_rejects_a_dependency_root_with_the_wrong_expected_digest() {
    let roots = Roots::new();
    let selections: Vec<MakeInputRootSelection> = vec![
        selection(&roots.source),
        selection(&roots.build),
        MakeInputRootSelection {
            root: roots.dependency.clone(),
            content_sha256: sha256_bytes(b"incorrect expected dependency content"),
        },
    ];

    let error = MakeInputSnapshot::capture(&roots.build, &selections)
        .expect_err("mismatched caller-supplied root digest must refuse capture");
    assert!(error.contains("independently selected content"));
}

#[test]
fn changed_external_tree_refuses_a_later_transaction_without_replacing_state() {
    let roots = Roots::new();
    let library_directory = roots.dependency.join("libraries");
    std::fs::create_dir(&library_directory).unwrap();
    std::fs::write(library_directory.join("alpha.library"), "before\n").unwrap();
    std::fs::write(roots.source.join("stable.mk"), "VALUE := original\n").unwrap();
    std::fs::write(
        roots.source.join("replacement.mk"),
        "VALUE := replacement\n",
    )
    .unwrap();
    let selections: Vec<MakeInputRootSelection> = roots.selected(true);
    let snapshot = capture(&roots.build, &selections);
    let dirs = DirVars::default();
    let paths = roots.paths(&dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file(&source_file(&roots.source, "stable.mk"), &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();

    std::fs::write(library_directory.join("alpha.library"), "after\n").unwrap();
    assert!(program
        .read_captured_file(
            &source_file(&roots.source, "replacement.mk"),
            &paths,
            &snapshot,
        )
        .is_err());
    assert_eq!(program.events(), events);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
}

#[test]
fn unselected_dependency_sibling_still_refuses_atomically() {
    let roots = Roots::new();
    let dependency_parent = roots.dependency.parent().unwrap();
    let selected = dependency_parent.join("selected");
    let sibling = dependency_parent.join("sibling");
    std::fs::rename(&roots.dependency, &selected).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    let selected = selected.canonicalize().unwrap();
    let sibling = sibling.canonicalize().unwrap();
    std::fs::write(selected.join("marker.data"), "selected dependency\n").unwrap();
    std::fs::write(sibling.join("available.library"), "sibling fixture\n").unwrap();
    let good = roots.source.join("good.mk");
    let bad = roots.source.join("bad.mk");
    std::fs::write(&good, "VALUE := original\n").unwrap();
    std::fs::write(
        &bad,
        format!(
            "VALUE := changed\nFILES := $(wildcard {}/*.library)\n",
            sibling.display()
        ),
    )
    .unwrap();
    let selections: Vec<MakeInputRootSelection> = vec![
        selection(&roots.source),
        selection(&roots.build),
        selection(&selected),
    ];
    let snapshot = capture(&roots.build, &selections);
    let dirs = DirVars::default();
    let paths = roots.paths(&dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file(&source_file(&roots.source, "good.mk"), &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();

    assert!(program
        .read_captured_file(&source_file(&roots.source, "bad.mk"), &paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), events);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
}

#[cfg(unix)]
#[test]
fn symlink_in_selected_external_root_refuses_atomically() {
    use std::os::unix::fs::symlink;

    let roots = Roots::new();
    let library_directory = roots.dependency.join("libraries");
    std::fs::create_dir(&library_directory).unwrap();
    std::fs::write(library_directory.join("real.library"), "fixture\n").unwrap();
    symlink("real.library", library_directory.join("alias.library")).unwrap();
    std::fs::write(roots.source.join("good.mk"), "VALUE := original\n").unwrap();
    std::fs::write(
        roots.source.join("bad.mk"),
        format!(
            "VALUE := changed\nFILES := $(wildcard {}/libraries/alias.library)\n",
            roots.dependency.display(),
        ),
    )
    .unwrap();
    let selections: Vec<MakeInputRootSelection> = roots.selected(true);
    let snapshot = capture(&roots.build, &selections);
    let dirs = DirVars::default();
    let paths = roots.paths(&dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file(&source_file(&roots.source, "good.mk"), &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();

    assert!(program
        .read_captured_file(&source_file(&roots.source, "bad.mk"), &paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), events);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
}
