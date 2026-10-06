//! ABI headers are independent of runtime compile capabilities, not fake archives.

use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_context, DependencyGraph, ModuleType, TargetContext,
};
use std::fs;

fn context() -> TargetContext {
    TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("gnu".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("0".into()),
        float_abi: Some("ilp32f".into()),
        ..TargetContext::default()
    }
}

fn fixture(macro_name: &str, args: &str, config: Option<&str>) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("workbench/libs/gallium");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("mmakefile.src"),
        format!("#MM workbench-libs-gallium :\n%{macro_name} mmake=workbench-libs-gallium modname=gallium modtype=library files=runtime uselibs=missing_runtime_lib {args}\n"),
    ).unwrap();
    if let Some(config) = config {
        fs::write(directory.join("gallium.conf"), config).unwrap();
    }
    root
}

#[test]
fn unsupported_runtime_keeps_source_headers_but_no_runtime_or_archive_endpoints() {
    let root = fixture(
        "build_module",
        "",
        Some("##begin config\nversion 1.0\n##end config\n"),
    );
    let context = context();
    let parsed = parse_mmakefile_with_context(
        &root.path().join("workbench/libs/gallium/mmakefile.src"),
        root.path(),
        &context,
    )
    .unwrap();
    let target = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "workbench-libs-gallium")
        .unwrap();
    assert_eq!(target.module_type, ModuleType::ModuleHeaders);
    assert_eq!(target.declared_mod_type.as_deref(), Some("library"));
    assert!(target.genmodule_abi);
    assert!(!target.genmodule_only);
    assert!(target.kobj_scoped_inputs.is_none());
    assert!(target.genmodule_linklibs.is_none());
    assert!(target.source_files.is_empty());
    assert!(target.use_libs.is_empty());
    assert!(!parsed.capability_errors.is_empty());
    let mut graph = DependencyGraph::new();
    graph.add_target(target.clone());
    for rule in parsed.meta_rules {
        graph.add_meta_rule(rule);
    }
    graph.meta_targets.insert(
        "includes-generate-deps".into(),
        std::collections::HashSet::new(),
    );
    assert!(graph.resolve_use_libs().is_empty());
    let selected = graph
        .selected_dependency_closure(
            &["workbench-libs-gallium-includes".into()],
            &context,
            &parsed.capability_errors,
        )
        .unwrap();
    assert!(selected.contains("workbench-libs-gallium-makefile"));
    assert!(selected.contains("includes-generate-deps"));
    assert!(!selected.contains("workbench-libs-gallium"));
    for runtime in [
        "workbench-libs-gallium",
        "workbench-libs-gallium-quick",
        "workbench-libs-gallium-kobj",
        "workbench-libs-gallium-linklib",
        "workbench-libs-gallium-linklib-rel",
        "linklibs-gallium",
    ] {
        assert!(
            graph
                .selected_dependency_closure(&[runtime.into()], &context, &parsed.capability_errors)
                .is_err(),
            "{runtime}"
        );
    }
    graph.retain_native_selection(&selected, &context).unwrap();
    let cmake = generate_cmake(&graph);
    assert!(
        cmake.contains("aros_add_module_headers(\n    TARGET \"gallium\""),
        "{cmake}"
    );
    for forbidden in [
        "aros_add_library(",
        "aros_add_module_abi(",
        "aros_record_module_kobj_sources(",
        "LINKLIB_NAME",
        "USELIBS",
        "add_custom_target(\"workbench-libs-gallium\")",
    ] {
        assert!(!cmake.contains(forbidden), "{forbidden}: {cmake}");
    }
}

#[test]
fn header_projection_never_substitutes_a_default_for_invalid_explicit_config() {
    for args in [
        "conffile=$(UNKNOWN)",
        "conffile=missing.conf",
        "confoverride=$(UNKNOWN)",
        "confoverride=missing.conf",
    ] {
        let root = fixture(
            "build_module",
            args,
            Some("##begin config\nversion 1.0\n##end config\n"),
        );
        let parsed = parse_mmakefile_with_context(
            &root.path().join("workbench/libs/gallium/mmakefile.src"),
            root.path(),
            &context(),
        )
        .unwrap();
        assert!(parsed.targets.is_empty(), "{args}: {:?}", parsed.targets);
        assert!(!parsed.capability_errors.is_empty());
    }
    let root = fixture("build_module", "", None);
    let parsed = parse_mmakefile_with_context(
        &root.path().join("workbench/libs/gallium/mmakefile.src"),
        root.path(),
        &context(),
    )
    .unwrap();
    assert!(parsed.targets.is_empty());
}

#[test]
fn runtime_only_macro_is_not_projected_and_explicit_abi_does_not_need_runtime_capability() {
    for (macro_name, expected) in [
        ("build_module_library", None),
        ("build_module_abi", Some(ModuleType::Abi)),
    ] {
        let root = fixture(
            macro_name,
            "",
            Some("##begin config\nversion 1.0\n##end config\n"),
        );
        let parsed = parse_mmakefile_with_context(
            &root.path().join("workbench/libs/gallium/mmakefile.src"),
            root.path(),
            &context(),
        )
        .unwrap();
        assert_eq!(
            parsed
                .targets
                .first()
                .map(|target| target.module_type.clone()),
            expected,
            "{macro_name}"
        );
        if macro_name == "build_module_abi" {
            assert!(
                parsed.capability_errors.is_empty(),
                "{:?}",
                parsed.capability_errors
            );
        }
    }
}
