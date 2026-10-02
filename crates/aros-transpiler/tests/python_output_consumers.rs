use aros_transpiler::ast::{PythonGeneratorJob, PythonOutputsDecl, TargetDefinition};
use aros_transpiler::dirs::DirVars;
use aros_transpiler::{generate_cmake, parse_mmakefile_with_dirs, DependencyGraph, ModuleType};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn parsed_target(
    root: &Path,
    directory: &str,
    mmakefile: &str,
    source_stems: &[&str],
) -> TargetDefinition {
    let module = root.join(directory);
    fs::create_dir_all(&module).unwrap();
    for stem in source_stems {
        fs::write(module.join(format!("{stem}.c")), "").unwrap();
    }
    let path = module.join("mmakefile.src");
    fs::write(&path, mmakefile).unwrap();
    let parsed = parse_mmakefile_with_dirs(&path, root, &DirVars::load(root)).unwrap();
    assert!(
        parsed.capability_errors.is_empty(),
        "{:#?}",
        parsed.capability_errors
    );
    assert_eq!(parsed.targets.len(), 1, "{:#?}", parsed.skipped_programs);
    parsed.targets.into_iter().next().unwrap()
}

fn python_outputs(consumers: &[&str]) -> PythonOutputsDecl {
    PythonOutputsDecl {
        owner: "fixture-generated-owner".to_owned(),
        source_root: "${AROS_PORTS_DIR}/fixture".to_owned(),
        build_root: "${AROS_BUILD_DIR}/gen/fixture".to_owned(),
        fetch_target: "fixture-fetch".to_owned(),
        source_inputs: Vec::new(),
        local_inputs: Vec::new(),
        jobs: vec![PythonGeneratorJob {
            script: "generate.py".to_owned(),
            local_script: false,
            output: "generated.h".to_owned(),
            arguments: Vec::new(),
            depends_on_outputs: Vec::new(),
        }],
        driver_script: None,
        requires_flex_bison: false,
        python_packages: Vec::new(),
        audited_source_dir: "${AROS_PORTS_DIR}/fixture".to_owned(),
        local_patch_files: Vec::new(),
        consumers: consumers
            .iter()
            .map(|consumer| (*consumer).to_owned())
            .collect(),
        dir_path: PathBuf::from("generator"),
    }
}

fn graph_with_output(consumers: &[&str]) -> DependencyGraph {
    let mut graph = DependencyGraph::new();
    graph.add_python_outputs(python_outputs(consumers));
    graph
}

#[test]
fn no_python_declarations_and_empty_consumer_lists_remain_valid() {
    let graph = DependencyGraph::new();
    assert_eq!(graph.validate_python_output_consumers(), Ok(()));
    assert!(!generate_cmake(&graph).contains("aros_bind_python_output_consumers("));

    let graph = graph_with_output(&[]);
    assert_eq!(graph.validate_python_output_consumers(), Ok(()));
    assert!(!generate_cmake(&graph).contains("aros_bind_python_output_consumers("));
}

#[test]
fn consumers_from_separate_mmakefiles_validate_and_bind_after_target_emission() {
    let tree = TempDir::new().unwrap();
    let target = parsed_target(
        tree.path(),
        "compiler",
        "%build_linklib mmake=compiler-consumer libname=compiler-consumer files=compiler_source\n",
        &["compiler_source"],
    );
    let mut graph = graph_with_output(&["compiler-consumer"]);
    graph.add_target(target);

    assert_eq!(graph.validate_python_output_consumers(), Ok(()));

    let cmake = generate_cmake(&graph);
    let owner = cmake.find("aros_generate_python_outputs(").unwrap();
    let compile_target = cmake.find("MMAKE_ID compiler-consumer").unwrap();
    let binding = cmake.find("aros_bind_python_output_consumers(").unwrap();
    assert!(owner < compile_target, "{cmake}");
    assert!(compile_target < binding, "{cmake}");
    assert!(cmake[binding..].contains("CONSUMERS \"compiler-consumer\""));
}

#[test]
fn missing_and_misspelled_consumers_fail_with_owner_and_consumer() {
    let tree = TempDir::new().unwrap();
    let target = parsed_target(
        tree.path(),
        "compiler",
        "%build_linklib mmake=compiler-consumer libname=compiler-consumer files=compiler_source\n",
        &["compiler_source"],
    );

    for missing in ["absent-consumer", "compiler-consumre"] {
        let mut graph = graph_with_output(&[missing]);
        graph.add_target(target.clone());
        let errors = graph.validate_python_output_consumers().unwrap_err();
        let diagnostics = errors.join("\n");
        assert!(
            diagnostics.contains("fixture-generated-owner"),
            "{diagnostics}"
        );
        assert!(diagnostics.contains(missing), "{diagnostics}");
        assert!(diagnostics.contains("complete graph"), "{diagnostics}");
    }
}

#[test]
fn program_group_consumers_name_emitted_members_not_the_aggregate() {
    let tree = TempDir::new().unwrap();
    let target = parsed_target(
        tree.path(),
        "programs",
        "%build_progs mmake=program-group files=first second\n",
        &["first", "second"],
    );
    assert_eq!(target.module_type, ModuleType::ProgramGroup);

    let mut graph = graph_with_output(&["program-group-first"]);
    graph.add_target(target.clone());
    assert_eq!(graph.validate_python_output_consumers(), Ok(()));

    let cmake = generate_cmake(&graph);
    let group_target = cmake.find("MMAKE_ID program-group").unwrap();
    let binding = cmake.find("aros_bind_python_output_consumers(").unwrap();
    assert!(group_target < binding, "{cmake}");
    assert!(cmake[binding..].contains("CONSUMERS \"program-group-first\""));

    let mut aggregate_graph = graph_with_output(&["program-group"]);
    aggregate_graph.add_target(target);
    let errors = aggregate_graph
        .validate_python_output_consumers()
        .unwrap_err();
    let diagnostics = errors.join("\n");
    assert!(
        diagnostics.contains("fixture-generated-owner"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("program-group"), "{diagnostics}");
    assert!(
        diagnostics.contains("does not name a compiling target"),
        "{diagnostics}"
    );
}

#[test]
fn abi_consumers_name_the_emitted_linklib_not_the_aggregate() {
    let tree = TempDir::new().unwrap();
    let target = parsed_target(
        tree.path(),
        "abi",
        "%build_module_abi mmake=abi-owner modname=AbiOwner modtype=library files=\n",
        &[],
    );
    assert_eq!(target.module_type, ModuleType::Abi);

    let mut linklib_graph = graph_with_output(&["abi-owner-linklib"]);
    linklib_graph.add_target(target.clone());
    assert_eq!(linklib_graph.validate_python_output_consumers(), Ok(()));

    let cmake = generate_cmake(&linklib_graph);
    let abi_target = cmake.find("aros_add_module_abi(").unwrap();
    let binding = cmake.find("aros_bind_python_output_consumers(").unwrap();
    assert!(abi_target < binding, "{cmake}");
    assert!(cmake[binding..].contains("CONSUMERS \"abi-owner-linklib\""));

    let mut aggregate_graph = graph_with_output(&["abi-owner"]);
    aggregate_graph.add_target(target);
    let diagnostics = aggregate_graph
        .validate_python_output_consumers()
        .unwrap_err()
        .join("\n");
    assert!(
        diagnostics.contains("fixture-generated-owner"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("abi-owner"), "{diagnostics}");
    assert!(
        diagnostics.contains("does not name a compiling target"),
        "{diagnostics}"
    );
}

#[test]
fn sourceful_custom_target_compiles_but_empty_custom_target_does_not() {
    let tree = TempDir::new().unwrap();
    let mut target = parsed_target(
        tree.path(),
        "custom",
        "%build_module mmake=custom-compiler modname=CustomCompiler modtype=unlisted files=custom_source\n",
        &["custom_source"],
    );
    assert_eq!(target.module_type, ModuleType::Custom);

    let mut graph = graph_with_output(&["custom-compiler"]);
    graph.add_target(target.clone());
    assert_eq!(graph.validate_python_output_consumers(), Ok(()));

    target.mmake_name = "empty-custom".to_owned();
    target.source_files.clear();
    target.cxx_source_files.clear();
    target.objc_source_files.clear();
    target.asm_source_files.clear();
    target.arch_sources.clear();
    let mut empty_graph = graph_with_output(&["empty-custom"]);
    empty_graph.add_target(target);
    let errors = empty_graph.validate_python_output_consumers().unwrap_err();
    let diagnostics = errors.join("\n");
    assert!(
        diagnostics.contains("fixture-generated-owner"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("empty-custom"), "{diagnostics}");
    assert!(
        diagnostics.contains("does not name a compiling target"),
        "{diagnostics}"
    );
}
