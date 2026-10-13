use super::*;
use crate::ast::MetaTargetRule;
use crate::DependencyGraph;
use std::fs;

const RULE: &str = ".PHONY: unit-genfiles\n#MM\nunit-genfiles : $(GENDIR)/$(CURDIR)/generated.c\n";

fn fixture() -> (tempfile::TempDir, ScriptOutputDecl, DirVars) {
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("compiler/unit")).unwrap();
    let generator = ScriptOutputDecl {
        directory: "compiler/unit".into(),
        output: "${AROS_BUILD_DIR}/gen/compiler/unit/generated.c".into(),
        additional_outputs: Vec::new(),
        script: "${AROS_SOURCE_DIR}/compiler/unit/generate.py".into(),
        arguments: vec!["${AROS_BUILD_DIR}/gen/compiler/unit/generated.c".into()],
        stdout: false,
        working_directory: None,
        depends: Vec::new(),
        consumer_source_stems: Vec::new(),
        consumer_targets: Vec::new(),
    };
    let dirs = DirVars::from_config_text("GENDIR := $(TOP)/gen\n");
    (tree, generator, dirs)
}

fn capture(
    tree: &Path,
    generator: &ScriptOutputDecl,
    dirs: &DirVars,
    content: &str,
) -> Vec<ScriptOutputAggregate> {
    collect(
        content,
        Path::new("compiler/unit/mmakefile.src"),
        tree,
        dirs,
        Some(&TargetContext::default()),
        &["unit-genfiles".into()],
        std::slice::from_ref(generator),
    )
}

#[test]
fn physical_dependency_only_rule_binds_exact_same_recipe_output() {
    let (tree, generator, dirs) = fixture();
    let result = capture(tree.path(), &generator, &dirs, RULE);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].owner, "unit-genfiles");
    assert_eq!(result[0].line, 3);
    assert_eq!(result[0].outputs, [canonical_output(&generator.output)]);
    let mut other = generator;
    other.directory = "compiler/other".into();
    assert!(capture(tree.path(), &other, &dirs, RULE).is_empty());
}

#[test]
fn opaque_incomplete_or_duplicate_physical_rules_are_not_providers() {
    let (tree, generator, dirs) = fixture();
    for content in [
        format!("{RULE}\t$(SHELL) opaque-command\n"),
        RULE.replace("generated.c", "different.c"),
        RULE.replace("generated.c", "generated.c ; opaque-command"),
        RULE.replace("generated.c", "generated.c other"),
        RULE.replace(" : ", " :: "),
        format!("{RULE}{RULE}"),
        format!("ifeq ($(UNKNOWN),yes)\n{RULE}endif\n"),
        format!("define HIDDEN\n{RULE}endef\n"),
        format!("private export define HIDDEN\n{RULE}endef\n"),
        format!(".RECIPEPREFIX := !\n{RULE}!bad-command: hidden\n"),
        format!("override .RECIPEPREFIX := :\n{RULE}:opaque-command\n"),
        format!("private export .RECIPEPREFIX := :\n{RULE}:opaque-command\n"),
        format!("include recipe-prefix.mk\n{RULE}:opaque-command\n"),
        format!("load opaque-plugin.so\n{RULE}"),
        format!("NAME := .RECIPE\n$(NAME)PREFIX := :\n{RULE}:opaque-command\n"),
        format!("$(eval opaque-generated-rule)\n{RULE}"),
        format!("{RULE}VARIABLE := value\n\tbad-command\n"),
        RULE.replace(".PHONY: unit-genfiles\n", ""),
        RULE.replace(".PHONY: unit-genfiles\n", ".PHONY: $(UNKNOWN)\n"),
        RULE.replace(
            ".PHONY: unit-genfiles\n",
            "define HIDDEN\n.PHONY: unit-genfiles\nendef\n",
        ),
        RULE.replace(
            ".PHONY: unit-genfiles\n",
            "ifeq ($(UNKNOWN),yes)\n.PHONY: unit-genfiles\nendif\n",
        ),
    ] {
        assert!(
            capture(tree.path(), &generator, &dirs, &content).is_empty(),
            "admitted: {content}"
        );
    }
    assert!(collect(
        RULE,
        Path::new("compiler/unit/mmakefile.src"),
        tree.path(),
        &dirs,
        None,
        &["unit-genfiles".into()],
        &[generator.clone(), generator]
    )
    .is_empty());
}

#[test]
fn included_recipe_syntax_is_checked_even_inside_a_bound_configuration() {
    let (tree, generator, dirs) = fixture();
    fs::create_dir_all(tree.path().join("config")).unwrap();
    fs::write(tree.path().join("config/aros.cfg"), "").unwrap();
    let path = "config/native.mk";
    let hidden_prefix = "override .RECIPEPREFIX := :\n";
    fs::write(tree.path().join(path), hidden_prefix).unwrap();
    let mut selection = TargetContext::default();
    selection
        .make_include_bindings
        .insert("config/aros.cfg".into(), path.into());
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(hidden_prefix.as_bytes()).to_string(),
    );
    selection.native_configuration_inputs.insert(
        "config/aros.cfg".into(),
        aros_common::sha256_bytes(b"").to_string(),
    );
    let content = format!("include $(SRCDIR)/config/aros.cfg\n{RULE}:opaque-command\n");
    assert!(collect(
        &content,
        Path::new("compiler/unit/mmakefile.src"),
        tree.path(),
        &dirs,
        Some(&selection),
        &["unit-genfiles".into()],
        std::slice::from_ref(&generator),
    )
    .is_empty());
    let safe = "SAFE_VALUE := 1\n";
    fs::write(tree.path().join(path), safe).unwrap();
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(safe.as_bytes()).to_string(),
    );
    let content = format!("include $(SRCDIR)/config/aros.cfg\n{RULE}");
    assert_eq!(
        collect(
            &content,
            Path::new("compiler/unit/mmakefile.src"),
            tree.path(),
            &dirs,
            Some(&selection),
            &["unit-genfiles".into()],
            std::slice::from_ref(&generator),
        )
        .len(),
        1
    );
    // Included assignments participate in the effective .PHONY proof, not
    // just in the syntax guard. Raw-source values must not override them.
    let changed_owner = "OWNER := other\n";
    fs::write(tree.path().join(path), changed_owner).unwrap();
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(changed_owner.as_bytes()).to_string(),
    );
    let content = format!(
        "OWNER := unit-genfiles\ninclude $(SRCDIR)/config/aros.cfg\n{}",
        RULE.replace(".PHONY: unit-genfiles", ".PHONY: $(OWNER)")
    );
    assert!(collect(
        &content,
        Path::new("compiler/unit/mmakefile.src"),
        tree.path(),
        &dirs,
        Some(&selection),
        &["unit-genfiles".into()],
        std::slice::from_ref(&generator),
    )
    .is_empty());
    let correct_owner = "OWNER := unit-genfiles\n";
    fs::write(tree.path().join(path), correct_owner).unwrap();
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(correct_owner.as_bytes()).to_string(),
    );
    assert_eq!(
        collect(
            &content,
            Path::new("compiler/unit/mmakefile.src"),
            tree.path(),
            &dirs,
            Some(&selection),
            &["unit-genfiles".into()],
            std::slice::from_ref(&generator),
        )
        .len(),
        1
    );
    let prerequisites_content = format!(
        "OUTPUT := $(GENDIR)/$(CURDIR)/generated.c\ninclude $(SRCDIR)/config/aros.cfg\n{}",
        RULE.replace("$(GENDIR)/$(CURDIR)/generated.c", "$(OUTPUT)")
    );
    assert_eq!(
        collect(
            &prerequisites_content,
            Path::new("compiler/unit/mmakefile.src"),
            tree.path(),
            &dirs,
            Some(&selection),
            &["unit-genfiles".into()],
            std::slice::from_ref(&generator),
        )
        .len(),
        1
    );
    let different_output = "OUTPUT := other\n";
    fs::write(tree.path().join(path), different_output).unwrap();
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(different_output.as_bytes()).to_string(),
    );
    assert!(collect(
        &prerequisites_content,
        Path::new("compiler/unit/mmakefile.src"),
        tree.path(),
        &dirs,
        Some(&selection),
        &["unit-genfiles".into()],
        std::slice::from_ref(&generator),
    )
    .is_empty());
    fs::write(tree.path().join(path), correct_owner).unwrap();
    selection.native_configuration_inputs.insert(
        path.into(),
        aros_common::sha256_bytes(correct_owner.as_bytes()).to_string(),
    );
    let scan = crate::local_make_includes::inline_native_make_configuration_for_selection(
        &content,
        tree.path(),
        Path::new("compiler/unit/mmakefile.src"),
        crate::LocalMakeIncludeLimits::default(),
        Some(&selection),
    );
    let joined = crate::parser::join_continuations(&scan.expanded);
    assert!(crate::parser::architecture_scope_positions(
        tree.path(),
        Path::new("compiler/unit/mmakefile.src"),
        &content,
        &scan,
        &joined,
    )
    .is_some());
    fs::write(
        tree.path().join(path),
        vec![b'x'; crate::LocalMakeIncludeLimits::default().bytes + 1],
    )
    .unwrap();
    assert!(crate::parser::architecture_scope_positions(
        tree.path(),
        Path::new("compiler/unit/mmakefile.src"),
        &content,
        &scan,
        &joined,
    )
    .is_none());
}

#[test]
fn implicit_make_recipe_is_refused_until_source_explicitly_marks_owner_phony() {
    let (tree, mut generator, dirs) = fixture();
    generator.output = tree
        .path()
        .join("generated.c")
        .to_string_lossy()
        .into_owned();
    fs::write(&generator.output, "generated input\n").unwrap();
    fs::write(tree.path().join("foo.c"), "implicit input\n").unwrap();
    let rule = format!(
        "#MM\nfoo.o: {}\n%.o: %.c\n\tprintf implicit > $@\n",
        generator.output
    );
    let capture_rule = |content: &str| {
        collect(
            content,
            Path::new("compiler/unit/mmakefile.src"),
            tree.path(),
            &dirs,
            Some(&TargetContext::default()),
            &["foo.o".into()],
            std::slice::from_ref(&generator),
        )
    };
    let run_make = |content: &str| {
        fs::write(tree.path().join("Makefile"), content).unwrap();
        let execution = std::process::Command::new("make")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(["-f", "Makefile", "foo.o"])
            .current_dir(tree.path())
            .output()
            .unwrap();
        assert!(
            execution.status.success(),
            "{}",
            String::from_utf8_lossy(&execution.stderr)
        );
    };
    assert!(capture_rule(&rule).is_empty());
    run_make(&rule);
    assert_eq!(
        fs::read_to_string(tree.path().join("foo.o")).unwrap(),
        "implicit"
    );
    fs::remove_file(tree.path().join("foo.o")).unwrap();
    let phony = format!(".PHONY: foo.o\n{rule}");
    assert_eq!(capture_rule(&phony).len(), 1);
    run_make(&phony);
    assert!(!tree.path().join("foo.o").exists());
}

#[test]
fn aggregate_only_generator_has_real_selection_and_late_cmake_binding() {
    let (tree, generator, dirs) = fixture();
    let mut graph = DependencyGraph::default();
    graph.script_output_aggregates = capture(tree.path(), &generator, &dirs, RULE);
    graph.make_meta_providers.insert("unit-genfiles".into());
    let raw_output = generator.output.clone();
    graph.add_meta_rule(MetaTargetRule {
        name: "unit-genfiles".into(),
        dependencies: vec![raw_output.clone()],
    });
    graph.add_explicit_meta_rule(MetaTargetRule {
        name: "unit-genfiles".into(),
        dependencies: vec![raw_output.clone()],
    });
    graph.add_script_outputs(vec![generator]);
    assert!(graph.resolve_script_outputs().is_empty());
    assert_eq!(graph.script_outputs.len(), 1);
    assert!(graph.script_outputs[0].consumers.is_empty());
    let owner = graph.script_outputs[0].owner.clone();
    assert_eq!(
        graph.script_output_aggregates[0].generators.as_slice(),
        std::slice::from_ref(&owner)
    );
    assert!(graph.meta_targets["unit-genfiles"].contains(&owner));
    assert!(!graph.meta_targets["unit-genfiles"].contains(&raw_output));
    assert!(graph
        .explicit_meta_edges
        .contains(&("unit-genfiles".into(), raw_output)));
    let cmake = crate::generate_cmake(&graph);
    let declaration = cmake.find("add_custom_target(\"unit-genfiles\")").unwrap();
    let binding = cmake.find("add_dependencies(\"unit-genfiles\"").unwrap();
    assert!(declaration < binding);
    assert!(!cmake.contains("aros_bind_python_output_consumers("));
    let context = TargetContext::default();
    let selected = graph
        .selected_dependency_closure(&["unit-genfiles".into()], &context, &[])
        .unwrap();
    assert!(selected.contains(&owner));
}

#[test]
fn generator_identity_uses_full_output_path_not_sanitized_basename_or_order() {
    let (tree, generator, dirs) = fixture();
    let mut second = generator.clone();
    second.output = generator.output.replace("generated.c", "generated-c");
    second.arguments = vec![second.output.clone()];
    let declarations = [generator, second];
    let content = RULE.replace(
        "generated.c\n",
        "generated.c $(GENDIR)/$(CURDIR)/generated-c\n",
    );
    let make_graph = |declarations: Vec<ScriptOutputDecl>| {
        let mut graph = DependencyGraph::default();
        graph.script_output_aggregates = collect(
            &content,
            Path::new("compiler/unit/mmakefile.src"),
            tree.path(),
            &dirs,
            Some(&TargetContext::default()),
            &["unit-genfiles".into()],
            &declarations,
        );
        assert_eq!(graph.script_output_aggregates.len(), 1);
        graph.add_script_outputs(declarations);
        graph.resolve_script_outputs();
        graph
            .script_outputs
            .into_iter()
            .map(|output| (output.outputs, output.owner))
            .collect::<BTreeMap<_, _>>()
    };
    let forward = make_graph(declarations.to_vec());
    let reverse = make_graph(declarations.into_iter().rev().collect());
    assert_eq!(forward, reverse);
    assert_eq!(forward.values().collect::<BTreeSet<_>>().len(), 2);
}

#[test]
fn unresolved_aggregate_never_becomes_a_concrete_empty_target() {
    let (tree, generator, dirs) = fixture();
    let mut graph = DependencyGraph::default();
    graph.script_output_aggregates = capture(tree.path(), &generator, &dirs, RULE);
    graph.make_meta_providers.insert("unit-genfiles".into());
    graph.add_meta_rule(MetaTargetRule {
        name: "unit-genfiles".into(),
        dependencies: Vec::new(),
    });
    graph.resolve_script_outputs();
    assert!(graph.script_output_aggregates[0].generators.is_empty());
    assert!(graph
        .selected_dependency_closure(&["unit-genfiles".into()], &TargetContext::default(), &[])
        .is_err());
    assert!(crate::generate_cmake(&graph).contains("script aggregate has no concrete generator"));
}

#[test]
fn parsed_named_generator_executes_through_the_actual_cmake_engine() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("source");
    fs::create_dir_all(source.join("compiler/unit")).unwrap();
    fs::write(
        source.join("compiler/unit/generate.py"),
        "import pathlib, sys\npathlib.Path(sys.argv[1]).write_text('generated\\n')\n",
    )
    .unwrap();
    let content = concat!(
        ".PHONY: unit-genfiles\n",
        "$(GENDIR)/$(CURDIR)/a/generated.c: $(SRCDIR)/$(CURDIR)/generate.py\n",
        "\t$(PYTHON) $(SRCDIR)/$(CURDIR)/generate.py $@\n",
        "$(GENDIR)/$(CURDIR)/b/generated.c: $(SRCDIR)/$(CURDIR)/generate.py\n",
        "\t$(PYTHON) $(SRCDIR)/$(CURDIR)/generate.py $@\n",
        "#MM\nunit-genfiles : $(GENDIR)/$(CURDIR)/a/generated.c $(GENDIR)/$(CURDIR)/b/generated.c\n"
    );
    fs::write(source.join("compiler/unit/mmakefile.src"), content).unwrap();
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &source.join("compiler/unit/mmakefile.src"),
        &source,
        &DirVars::from_config_text("GENDIR := $(TOP)/gen\n"),
        &TargetContext::default(),
    )
    .unwrap();
    assert_eq!(
        parsed.script_output_aggregates.len(),
        1,
        "{:?}",
        parsed.native_graph_errors
    );
    assert!(!parsed
        .native_graph_errors
        .iter()
        .any(|error| error.message.contains("nonvirtual Make provider")));
    let mut graph = DependencyGraph::default();
    for rule in parsed.meta_rules {
        graph.add_meta_rule(rule);
    }
    graph.make_meta_providers.extend(parsed.make_meta_providers);
    graph
        .script_output_aggregates
        .extend(parsed.script_output_aggregates);
    graph.add_script_outputs(parsed.script_outputs);
    graph.resolve_script_outputs();
    assert_eq!(graph.script_outputs.len(), 2);
    assert_ne!(graph.script_outputs[0].owner, graph.script_outputs[1].owner);
    assert_eq!(graph.script_output_aggregates[0].generators.len(), 2);
    fs::write(
        source.join("generated.cmake"),
        crate::generate_cmake(&graph),
    )
    .unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/PythonGenerators.cmake")
        .canonicalize()
        .unwrap();
    // Keep the fixture focused on script production, without bootstrapping a
    // fabricated AROS SDK. Reuse the exact production finalization function
    // required by the full emitter rather than providing a mock implementation.
    let core = fs::read_to_string(engine.with_file_name("AROS.cmake")).unwrap();
    let marker = "function(aros_finalize_link_libraries)";
    let function = core
        .split_once(marker)
        .unwrap()
        .1
        .split_once("endfunction()")
        .unwrap()
        .0;
    fs::write(
        source.join("production-finalizer.cmake"),
        format!("{marker}{function}endfunction()\n"),
    )
    .unwrap();
    fs::write(source.join("CMakeLists.txt"), format!(concat!(
        "cmake_minimum_required(VERSION 3.25)\nproject(script_aggregate NONE)\n",
        "set(AROS_SOURCE_DIR \"${{CMAKE_CURRENT_SOURCE_DIR}}\")\nset(AROS_BUILD_DIR \"${{CMAKE_BINARY_DIR}}\")\n",
        "include(\"{}\")\ninclude(\"${{CMAKE_CURRENT_SOURCE_DIR}}/production-finalizer.cmake\")\ninclude(\"${{CMAKE_CURRENT_SOURCE_DIR}}/generated.cmake\")\n"), engine.display())).unwrap();
    let build = tree.path().join("build");
    let configured = std::process::Command::new("cmake")
        .args(["-G", "Ninja", "-S"])
        .arg(&source)
        .arg("-B")
        .arg(&build)
        .output()
        .unwrap();
    assert!(
        configured.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&configured.stdout),
        String::from_utf8_lossy(&configured.stderr)
    );
    let run = || {
        std::process::Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", "unit-genfiles"])
            .output()
            .unwrap()
    };
    let execution = run();
    assert!(
        execution.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&execution.stdout),
        String::from_utf8_lossy(&execution.stderr)
    );
    let output = build.join("gen/compiler/unit/a/generated.c");
    assert_eq!(fs::read_to_string(&output).unwrap(), "generated\n");
    assert_eq!(
        fs::read_to_string(build.join("gen/compiler/unit/b/generated.c")).unwrap(),
        "generated\n"
    );
    fs::remove_file(&output).unwrap();
    fs::write(
        source.join("compiler/unit/generate.py"),
        "raise SystemExit(7)\n",
    )
    .unwrap();
    assert!(
        !run().status.success(),
        "a failed generator was hidden by the aggregate"
    );
    assert!(!output.exists());
}
