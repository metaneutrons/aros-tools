use aros_transpiler::ast::ParsedMmakefile;
use aros_transpiler::{
    dirs::DirVars, generate_cmake, parse_mmakefile, parse_mmakefile_with_dirs_and_context,
    DependencyGraph, TargetContext,
};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn write_mmakefile(tree: &TempDir, source: &str) -> PathBuf {
    let path = tree.path().join("module/mmakefile.src");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, source).unwrap();
    path
}

fn parse(tree: &TempDir, source: &str) -> ParsedMmakefile {
    let path = write_mmakefile(tree, source);
    parse_mmakefile(&path, tree.path()).unwrap_or_else(|error| panic!("{error:?}"))
}

fn with_module_target(declarations: &str) -> String {
    format!("{declarations}\n%build_linklib mmake=module libname=module files=generic\n")
}

#[test]
fn quoted_srcdir_paths_bind_at_each_archspecific_declaration() {
    let tree = TempDir::new().unwrap();
    let parsed = parse(
        &tree,
        &with_module_target(
            r#"%build_archspecific mainmmake=module arch=arm files=direct incextra="$(SRCDIR)/$(CURDIR)/private headers"
EXTRA_INCLUDE := $(SRCDIR)/$(CURDIR)/before headers
%build_archspecific mainmmake=module arch=arm files=before incextra="$(EXTRA_INCLUDE)"
EXTRA_INCLUDE := $(SRCDIR)/$(CURDIR)/after
%build_archspecific mainmmake=module arch=arm files=after incextra="$(EXTRA_INCLUDE)""#,
        ),
    );

    assert_eq!(
        parsed.arch_sources.len(),
        3,
        "{:#?}",
        parsed.skipped_arch_sources
    );
    assert_eq!(
        parsed.arch_sources[0].extra_quote_include.as_deref(),
        Some("${AROS_SOURCE_DIR}/module/private headers")
    );
    assert_eq!(
        parsed.arch_sources[1].extra_quote_include.as_deref(),
        Some("${AROS_SOURCE_DIR}/module/before headers")
    );
    assert_eq!(
        parsed.arch_sources[2].extra_quote_include.as_deref(),
        Some("${AROS_SOURCE_DIR}/module/after")
    );
    assert_eq!(
        parsed.arch_sources[1].compile_options,
        ["-iquote${AROS_SOURCE_DIR}/module/before headers"]
    );
    assert_eq!(
        parsed.arch_sources[2].compile_options,
        ["-iquote${AROS_SOURCE_DIR}/module/after"]
    );
}

#[test]
fn absent_and_empty_incextra_keep_the_default_include_behavior() {
    let tree = TempDir::new().unwrap();
    let parsed = parse(
        &tree,
        &with_module_target(
            "%build_archspecific mainmmake=module arch=arm files=absent\n\
             %build_archspecific mainmmake=module arch=arm files=empty incextra=\"\"\n\
             EMPTY_INCLUDE :=\n\
             %build_archspecific mainmmake=module arch=arm files=expanded_empty incextra=\"$(EMPTY_INCLUDE)\"\n",
        ),
    );

    assert_eq!(
        parsed.arch_sources.len(),
        3,
        "{:#?}",
        parsed.skipped_arch_sources
    );
    for declaration in &parsed.arch_sources {
        assert_eq!(declaration.extra_quote_include, None);
        assert!(
            declaration.compile_options.is_empty(),
            "{}: {:?}",
            declaration.files[0],
            declaration.compile_options
        );
    }
}

#[test]
fn unresolved_or_unsafe_explicit_incextra_values_are_rejected() {
    let cases = [
        (
            "unknown variable",
            "%build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(MISSING_INCLUDE)\"\n",
        ),
        (
            "cyclic variables",
            "INCLUDE_A = $(INCLUDE_B)\nINCLUDE_B = $(INCLUDE_A)\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_A)\"\n",
        ),
        (
            "conditional assignment",
            "ifeq ($(UNCONFIGURED_SWITCH),yes)\n\
             INCLUDE_PATH := $(SRCDIR)/$(CURDIR)/conditional\n\
             endif\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_PATH)\"\n",
        ),
        (
            "unsafe separator",
            "%build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(SRCDIR)/$(CURDIR)/include;touch marker\"\n",
        ),
        (
            "CMake environment interpolation",
            "%build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(SRCDIR)/$ENV{HOME}\"\n",
        ),
        (
            "CMake cache interpolation through a local",
            "INCLUDE_PATH = $(SRCDIR)/$CACHE{INCLUDE}\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_PATH)\"\n",
        ),
        (
            "unresolved short Make variable",
            "%build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(SRCDIR)/$X\"\n",
        ),
        (
            "append does not reset conditional uncertainty",
            "ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_PATH = unknown\nendif\n\
             INCLUDE_PATH += known\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_PATH)\"\n",
        ),
        (
            "immediate assignment preserves conditional dependency uncertainty",
            "INCLUDE_A := $(SRCDIR)/old\n\
             ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_A := $(SRCDIR)/new\nendif\n\
             INCLUDE_B := $(INCLUDE_A)\nINCLUDE_C := $(INCLUDE_B)\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_C)\"\n",
        ),
        (
            "brace spelling preserves immediate conditional dependency uncertainty",
            "INCLUDE_A := $(SRCDIR)/old\n\
             ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_A := $(SRCDIR)/new\nendif\n\
             INCLUDE_B := ${INCLUDE_A}\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_B)\"\n",
        ),
        (
            "recursive alias preserves immediate conditional dependency uncertainty",
            "INCLUDE_A := $(SRCDIR)/old\n\
             ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_A := $(SRCDIR)/new\nendif\n\
             INCLUDE_B = $(INCLUDE_A)\nINCLUDE_C := $(INCLUDE_B)\n\
             %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_C)\"\n",
        ),
    ];

    for (case, declaration) in cases {
        let tree = TempDir::new().unwrap();
        let source = with_module_target(declaration);
        let path = write_mmakefile(&tree, &source);
        let error = parse_mmakefile(&path, tree.path())
            .expect_err("unsafe explicit incextra must fail closed");
        assert!(
            format!("{error:?}").contains("cannot resolve %build_archspecific incextra"),
            "{case}: unexpected error {error:?}"
        );
    }
}

#[test]
fn proven_replacement_resolves_an_earlier_unknown_conditional() {
    for operator in ["=", ":="] {
        let tree = TempDir::new().unwrap();
        let parsed = parse(&tree, &with_module_target(&format!(
            "ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_PATH = unknown\nendif\n\
             INCLUDE_PATH {operator} $(SRCDIR)/known\n\
             %build_archspecific mainmmake=module arch=arm files=known incextra=\"$(INCLUDE_PATH)\"\n"
        )));
        assert_eq!(
            parsed.arch_sources[0].extra_quote_include.as_deref(),
            Some("${AROS_SOURCE_DIR}/known")
        );
    }
}

#[test]
fn later_assignment_does_not_erase_an_immediate_dependency_failure() {
    let tree = TempDir::new().unwrap();
    let path = write_mmakefile(
        &tree,
        &with_module_target(
            "INCLUDE_A := $(SRCDIR)/old\n\
         ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_A := $(SRCDIR)/new\nendif\n\
         INCLUDE_B := $(INCLUDE_A)\nINCLUDE_A := $(SRCDIR)/known\n\
         %build_archspecific mainmmake=module arch=arm files=bad incextra=\"$(INCLUDE_B)\"\n",
        ),
    );
    assert!(parse_mmakefile(&path, tree.path()).is_err());
    // Replacing B itself, unlike a later change to A, establishes a known value.
    let parsed = parse(
        &tree,
        &with_module_target(
            "INCLUDE_A := $(SRCDIR)/old\n\
         ifeq ($(UNCONFIGURED_SWITCH),yes)\nINCLUDE_A := $(SRCDIR)/new\nendif\n\
         INCLUDE_B := $(INCLUDE_A)\nINCLUDE_B := $(SRCDIR)/known\n\
         %build_archspecific mainmmake=module arch=arm files=known incextra=\"$(INCLUDE_B)\"\n",
        ),
    );
    assert_eq!(
        parsed.arch_sources[0].extra_quote_include.as_deref(),
        Some("${AROS_SOURCE_DIR}/known")
    );
}

#[test]
fn local_in_a_proven_false_branch_retains_the_empty_fallback() {
    let tree = TempDir::new().unwrap();
    let path = write_mmakefile(
        &tree,
        &with_module_target(
            "ifeq ($(CPU),aarch64)\nINCLUDE_PATH = unused\nendif\n\
         %build_archspecific mainmmake=module arch=arm files=empty incextra=\"$(INCLUDE_PATH)\"\n",
        ),
    );
    let context = TargetContext {
        cpu: Some("arm".to_owned()),
        ..TargetContext::default()
    };
    let parsed = parse_mmakefile_with_dirs_and_context(
        &path,
        tree.path(),
        &DirVars::load(tree.path()),
        &context,
    )
    .unwrap();
    assert_eq!(parsed.arch_sources[0].extra_quote_include, None);
    assert!(parsed.arch_sources[0].compile_options.is_empty());
}

#[test]
fn immediate_paths_freeze_missing_locals_before_later_assignments() {
    for reference in ["$(INCLUDE_A)", "${INCLUDE_A}"] {
        let tree = TempDir::new().unwrap();
        let parsed = parse(
            &tree,
            &with_module_target(&format!(
                "INCLUDE_B := {reference}\nINCLUDE_A := $(SRCDIR)/future\n\
             %build_archspecific mainmmake=module arch=arm files=empty incextra=\"$(INCLUDE_B)\"\n"
            )),
        );
        assert_eq!(parsed.arch_sources[0].extra_quote_include, None);
    }
    let tree = TempDir::new().unwrap();
    let path = write_mmakefile(
        &tree,
        &with_module_target(
            "ifeq ($(CPU),aarch64)\nINCLUDE_A = unused\nendif\n\
         INCLUDE_B := $(INCLUDE_A)\nINCLUDE_A := $(SRCDIR)/future\n\
         %build_archspecific mainmmake=module arch=arm files=empty incextra=\"$(INCLUDE_B)\"\n",
        ),
    );
    let context = TargetContext {
        cpu: Some("arm".to_owned()),
        ..TargetContext::default()
    };
    let parsed = parse_mmakefile_with_dirs_and_context(
        &path,
        tree.path(),
        &DirVars::load(tree.path()),
        &context,
    )
    .unwrap();
    assert_eq!(parsed.arch_sources[0].extra_quote_include, None);
    // Recursive assignment intentionally observes the later value.
    let parsed = parse(
        &tree,
        &with_module_target(
            "INCLUDE_B = $(INCLUDE_A)\nINCLUDE_A := $(SRCDIR)/future\n\
         %build_archspecific mainmmake=module arch=arm files=known incextra=\"$(INCLUDE_B)\"\n",
        ),
    );
    assert_eq!(
        parsed.arch_sources[0].extra_quote_include.as_deref(),
        Some("${AROS_SOURCE_DIR}/future")
    );
}

#[test]
fn graph_and_generator_propagate_iquote_to_each_architecture_source() {
    let tree = TempDir::new().unwrap();
    let parsed = parse(
        &tree,
        &with_module_target(
            r#"%build_archspecific mainmmake=module arch=arm files="fast_one fast_two" incextra="$(SRCDIR)/$(CURDIR)/private headers""#,
        ),
    );

    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_arch_sources(parsed.arch_sources);
    graph.resolve_arch_sources();

    let target = &graph.targets["module"];
    for file in ["fast_one", "fast_two"] {
        assert!(
            target.arch_source_options.iter().any(|option| {
                option
                    == &(
                        "arm".to_owned(),
                        "module".to_owned(),
                        file.to_owned(),
                        "-iquote${AROS_SOURCE_DIR}/module/private headers".to_owned(),
                    )
            }),
            "missing per-file quote include for {file}: {:?}",
            target.arch_source_options
        );
    }

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("aros_set_arch_source_options("), "{cmake}");
    assert!(
        cmake.contains("\"arm|module|fast_one|-iquote${AROS_SOURCE_DIR}/module/private headers\""),
        "{cmake}"
    );
    assert!(
        cmake.contains("\"arm|module|fast_two|-iquote${AROS_SOURCE_DIR}/module/private headers\""),
        "{cmake}"
    );
}
