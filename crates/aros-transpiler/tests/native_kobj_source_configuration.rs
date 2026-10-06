//! Explicit local source probe, not a full graph or P4 payload qualification.

use aros_common::{native_build_contract::load_bound_native_build_contract, TargetProfile};
use aros_transpiler::{
    ast::ParsedMmakefile, dirs::DirVars, kobj_scoped_inputs::ScopedMakeWords,
    parse_mmakefile_with_dirs_and_context, DependencyGraph, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

fn selected_source_context() -> (PathBuf, TargetContext) {
    let root =
        PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").expect("select isolated source"));
    let expected = std::env::var("AROS_TEST_NATIVE_CONTRACT_SHA256").expect("pin contract bytes");
    let profiles = TargetProfile::load_from_file(&root.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let loaded = load_bound_native_build_contract(
        &root,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
    )
    .unwrap();
    assert_eq!(loaded.sha256.as_str(), expected);
    let generated_make_templates =
        aros_common::native_make_template::resolve_generated_make_templates(
            &root,
            &loaded.contract.generated_make_templates,
            &loaded
                .contract
                .inputs
                .iter()
                .map(|input| (input.path.clone(), input.sha256.clone()))
                .collect(),
        )
        .unwrap();
    let selectors = profile.transpiler.as_ref().unwrap();
    let make_variables = loaded
        .contract
        .make_variables_for_host(aros_common::target::native_host_key().unwrap_or(""))
        .unwrap();
    let target = TargetContext {
        cpu: Some(loaded.contract.abi.source_cpu),
        platform: Some(profile.platform.clone()),
        family: Some(selectors.family.clone()),
        variant: Some(selectors.variant.clone()),
        toolchain: Some(selectors.toolchain.clone()),
        cpu32: Some(selectors.cpu32.clone()),
        use_mmu: Some(if selectors.use_mmu { "1" } else { "0" }.into()),
        float_abi: profile.float_abi.clone(),
        make_variables,
        make_include_bindings: loaded.contract.make_include_bindings,
        generated_make_templates,
        ..TargetContext::default()
    };
    (root, target)
}

fn parse_native_file(path: &Path, root: &Path, target: &TargetContext) -> ParsedMmakefile {
    // Mirror the explicit native CLI path, not the unqualified library entry
    // point: selected_source_context has independently validated the contract.
    let mut dirs = DirVars::load(root);
    dirs.bind_native_target_tool_roles();
    parse_mmakefile_with_dirs_and_context(path, root, &dirs, target).unwrap()
}

#[test]
#[ignore = "requires an explicitly selected isolated P4 source and contract digest"]
fn selected_p4_execbase_header_has_a_complete_non_smp_source_pipeline() {
    let (root, target) = selected_source_context();
    let parsed = parse_native_file(&root.join("compiler/include/mmakefile.src"), &root, &target);
    let pipeline = parsed
        .source_header_pipelines
        .iter()
        .find(|pipeline| pipeline.owner == "includes-execbase_h")
        .unwrap_or_else(|| panic!("missing execbase pipeline: {parsed:#?}"));
    assert_eq!(pipeline.steps.len(), 2);
    assert_eq!(
        pipeline.source_prerequisite,
        "${AROS_SOURCE_DIR}/compiler/include/exec/execbase.inc"
    );
    assert_eq!(
        pipeline.sdk_output,
        "${AROS_SDK_INCLUDE_DIR}/exec/execbase.h"
    );
    // Parser acceptance alone is not enough: legacy projections from the
    // same real source file must reconcile at the complete graph boundary.
    let mut graph = DependencyGraph::default();
    graph.source_header_pipelines = parsed.source_header_pipelines;
    graph.layered_header_projections = parsed.layered_header_projections;
    graph.source_directory_groups = parsed.source_directory_groups;
    graph.adhoc_header_rules = parsed.adhoc_header_rules;
    graph.header_transforms = parsed.header_transforms;
    graph.copy_includes = parsed.copy_includes;
    let diagnostics = graph.bind_source_headers();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert!(graph
        .source_layered_headers
        .iter()
        .any(|declaration| declaration.owner == "compiler-includes"));
    assert!(graph.header_transforms.iter().any(|transform| {
        transform.name == "includes-execbase_h"
            && transform.output == "${AROS_SDK_INCLUDE_DIR}/exec/execbase.h"
    }));
}

#[test]
#[ignore = "requires an explicitly selected isolated P4 source and contract digest"]
fn selected_p4_literal_object_rules_preserve_source_flags_and_native_sdk() {
    let (root, target) = selected_source_context();
    let parsed = parse_native_file(&root.join("compiler/libinit/mmakefile.src"), &root, &target);
    assert_eq!(parsed.literal_object_groups.len(), 2, "{parsed:#?}");
    for owner in ["libinit-libentry", "libinit-kickentry"] {
        let diagnostics: Vec<_> = parsed
            .native_graph_errors
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()
                    .and_then(|context| context.target.as_deref())
                    == Some(owner)
            })
            .collect();
        assert!(diagnostics.is_empty(), "{owner}: {diagnostics:#?}");
        let group = parsed
            .literal_object_groups
            .iter()
            .find(|group| group.owner == owner)
            .unwrap();
        assert_eq!(group.objects.len(), 1);
        assert_eq!(group.objects[0].language, "C");
        let lane = [
            "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc",
            "-mabi=ilp32f",
            "-mcmodel=medany",
            "-O2",
            "-Wall",
            "-Werror",
            "-Wno-pointer-sign",
            "-Wno-parentheses",
        ];
        let mut expected = vec!["--sysroot=${AROS_BUILD_DIR}/SYS/Developer"];
        expected.extend(lane);
        expected.extend(lane);
        assert_eq!(group.objects[0].arguments, expected);
    }
}

#[test]
#[ignore = "requires an explicitly selected isolated P4 source and contract digest"]
fn selected_p4_startup_objects_preserve_source_lanes_and_declaration_flags() {
    let (root, target) = selected_source_context();
    let parsed = parse_native_file(&root.join("compiler/startup/mmakefile.src"), &root, &target);
    assert!(
        parsed.native_graph_errors.is_empty(),
        "{:#?}",
        parsed.native_graph_errors
    );
    let group = parsed
        .sdk_object_groups
        .iter()
        .find(|group| group.owner == "linklibs-startup")
        .unwrap_or_else(|| panic!("missing startup producer: {parsed:#?}"));
    assert_eq!(group.objects.len(), 8, "{:#?}", group.objects);
    for (stem, language, lane, xopen) in [
        ("startup", "C", "", false),
        ("cxx-startup", "C", "", false),
        ("detach", "C", "", false),
        ("elf-startup", "C", "", false),
        ("nixmain", "C", "nix/", true),
        ("static-cxx-ops", "CXX", "cxx/", true),
        ("static-cxx-personality", "CXX", "cxx/", true),
        ("static-cxx-cxa-pure-virtual", "CXX", "cxx/", true),
    ] {
        let object = group
            .objects
            .iter()
            .find(|object| object.output.ends_with(&format!("/{stem}.o")))
            .unwrap_or_else(|| panic!("missing {stem}: {:#?}", group.objects));
        assert_eq!(object.language, language);
        let extension = if language == "C" { "c" } else { "cpp" };
        assert_eq!(
            object.source,
            format!("${{AROS_SOURCE_DIR}}/compiler/startup/{stem}.{extension}")
        );
        assert!(
            object
                .intermediate
                .ends_with(&format!("/compiler/startup/{lane}{stem}.o")),
            "{object:?}"
        );
        assert_eq!(
            object
                .defines
                .iter()
                .any(|define| define == "_XOPEN_SOURCE=700"),
            xopen,
            "{object:?}"
        );
        assert!(
            !object
                .defines
                .iter()
                .any(|define| define.starts_with("DEBUG")),
            "{object:?}"
        );
    }
    let quick = parsed
        .sdk_object_groups
        .iter()
        .find(|group| group.owner == "linklibs-startup-quick")
        .expect("source-declared quick aggregate");
    assert_eq!(quick.objects, group.objects);
}

#[test]
#[ignore = "requires an explicitly selected isolated P4 source and contract digest"]
fn selected_p4_core_members_have_source_bound_native_configuration() {
    let (root, target) = selected_source_context();
    let members = [
        ("rom/kernel/mmakefile.src", "kernel"),
        ("rom/task/mmakefile.src", "task"),
        ("rom/exec/mmakefile.src", "exec"),
        ("rom/debug/mmakefile.src", "debug"),
        ("rom/timer/mmakefile.src", "timer"),
        ("arch/riscv-esp32p4/flashdisk/mmakefile.src", "flashdisk"),
    ];
    for (relative, name) in members {
        let parsed = parse_native_file(&root.join(relative), &root, &target);
        let module = parsed
            .targets
            .iter()
            .find(|module| module.target_name == name)
            .unwrap_or_else(|| panic!("{relative}: no source module {name}"));
        let inputs = module.kobj_scoped_inputs.as_ref().expect("scoped inputs");
        for (field, value) in [
            ("user_objects", &inputs.user_objects),
            ("defname_libs", &inputs.defname_libs),
            ("user_ldflags", &inputs.user_ldflags),
            ("use_libs", &inputs.use_libs),
            ("kobj_ldflags", &inputs.kobj_ldflags),
            ("kernel_kobj_ldscript", &inputs.kernel_kobj_ldscript),
            ("funcinstr_libs", &inputs.funcinstr_libs),
            ("function_instrumentation", &inputs.function_instrumentation),
        ] {
            assert!(
                !matches!(value, ScopedMakeWords::Unresolved { .. }),
                "{relative}: {field}: {value:?}"
            );
        }
        assert!(
            matches!(inputs.kobj_ldflags, ScopedMakeWords::KnownEmpty { .. }),
            "{relative}: {:?}",
            inputs.kobj_ldflags
        );
        assert!(
            matches!(
                inputs.kernel_kobj_ldscript,
                ScopedMakeWords::KnownEmpty { .. }
            ),
            "{relative}: {:?}",
            inputs.kernel_kobj_ldscript
        );
        assert!(
            matches!(&inputs.function_instrumentation, ScopedMakeWords::Exact { words, .. }
            if words == &["no"]),
            "{relative}: {:?}",
            inputs.function_instrumentation
        );
        assert!(
            matches!(&inputs.funcinstr_libs, ScopedMakeWords::Exact { words, .. }
            if words == &["instrfunc"]),
            "{relative}: {:?}",
            inputs.funcinstr_libs
        );
        assert!(inputs
            .included_configuration_files
            .iter()
            .any(|file| file.path == "arch/riscv-esp32p4/native-kobj-config.mk"));
    }
}

#[test]
#[ignore = "requires selected isolated source and pinned contract"]
fn selected_source_gallium_projects_headers_without_runtime_or_client_archive() {
    let (root, target) = selected_source_context();
    for mesa_version in [None, Some("20.0.8".to_owned())] {
        let selected = TargetContext {
            mesa_version,
            ..target.clone()
        };
        let parsed = parse_native_file(
            &root.join("workbench/libs/gallium/mmakefile.src"),
            &root,
            &selected,
        );
        eprintln!("Mesa selector: {:?}", selected.mesa_version);
        eprintln!("Skipped: {:#?}", parsed.skipped_programs);
        eprintln!("Capabilities: {:#?}", parsed.capability_errors);
        let module = parsed
            .targets
            .iter()
            .find(|module| module.mmake_name == "workbench-libs-gallium")
            .expect("source-proven header declaration");
        assert_eq!(module.module_type, ModuleType::ModuleHeaders);
        assert!(module.genmodule_abi);
        assert!(module.genmodule_linklibs.is_none());
        assert!(module.kobj_scoped_inputs.is_none());
        assert!(!parsed.capability_errors.is_empty());
        for module in &parsed.targets {
            eprintln!(
                "Target: {} / {} ABI={}",
                module.mmake_name, module.target_name, module.genmodule_abi
            );
        }
    }
}
