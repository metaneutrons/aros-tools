use super::*;
use crate::ast::{GenmoduleLinklibs, MetaTargetRule, TargetDefinition};
use crate::fetch::FetchDecl;
use aros_common::{DiagnosticCode, DiagnosticContext, DiagnosticStage};

#[test]
fn optional_cut_cannot_hide_an_independent_required_alias_ingress() {
    let mut graph = DependencyGraph::new();
    for (name, dependency) in [
        ("optional", "alias"),
        ("required", "alias"),
        ("alias", "missing"),
    ] {
        graph.add_meta_rule(MetaTargetRule {
            name: name.into(),
            dependencies: vec![dependency.into()],
        });
    }
    let cuts = BTreeSet::from([("optional".into(), "alias".into())]);
    let context = TargetContext::default();
    let optional_only = graph
        .native_required_metadata_reachability(&["optional".into()], &context, &cuts)
        .unwrap();
    assert!(!optional_only.contains("alias"));
    let shared = graph
        .native_required_metadata_reachability(
            &["optional".into(), "required".into()],
            &context,
            &cuts,
        )
        .unwrap();
    assert!(shared.contains("alias"));
    assert!(shared.contains("missing"));
}

#[test]
fn writefiles_stamp_rejects_selected_python_and_flexcat_owner_collisions() {
    let mut graph = DependencyGraph::new();
    graph.genmodule_writefiles_rules.push(
        crate::genmodule_writefiles_rules::GenmoduleWritefilesRuleDecl {
            owner: "shared-generator".into(),
            file: "unit/mmakefile.src".into(),
            line: 1,
            declaring_dir: "unit".into(),
            config: "unit/module.conf".into(),
            module: "module".into(),
            modtype: crate::genmodule_header_rules::GenmoduleType::Library,
        },
    );
    graph.python_outputs.push(
        serde_json::from_value(serde_json::json!({
            "owner": "shared-generator", "source_root": "ports/source",
            "build_root": "gen/python", "fetch_target": "fixture-fetch",
            "source_inputs": [], "jobs": [], "audited_source_dir": "ports/source",
            "local_patch_files": [], "consumers": [], "dir_path": "unit"
        }))
        .unwrap(),
    );
    graph.add_meta_rule(MetaTargetRule {
        name: "fixture-fetch".into(),
        dependencies: vec![],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "unrelated-root".into(),
        dependencies: vec![],
    });
    assert!(
        graph
            .selected_dependency_closure(&["unrelated-root".into()], &TargetContext::default(), &[])
            .is_ok(),
        "an unselected collision must not poison unrelated graphs"
    );
    let error = graph
        .selected_dependency_closure(&["shared-generator".into()], &TargetContext::default(), &[])
        .unwrap_err();
    assert!(error.to_string().contains(
        "selected genmodule writefiles stamp shared-generator has conflicting concrete producers"
    ), "{error}");
    graph.python_outputs.clear();
    graph.flexcat_sources.push(
        serde_json::from_value(serde_json::json!({
            "owner": "shared-generator", "declaring_dir": "unit", "line": 1,
            "source": "locale.c", "header": "locale.h", "description": "unit/module.cd",
            "header_template": "unit/header.sd", "source_template": "unit/source.sd",
            "catalog_destination": null, "catalog_name": null, "catalog_source_dir": null,
            "languages": [], "consumers": []
        }))
        .unwrap(),
    );
    assert!(graph
        .selected_dependency_closure(&["unrelated-root".into()], &TargetContext::default(), &[])
        .is_ok());
    let error = graph
        .selected_dependency_closure(&["shared-generator".into()], &TargetContext::default(), &[])
        .unwrap_err();
    assert!(error.to_string().contains(
        "selected genmodule writefiles stamp shared-generator has conflicting concrete producers"
    ), "{error}");
}

fn sdk_group(owner: &str) -> crate::sdk_objects::SdkObjectGroupDecl {
    crate::sdk_objects::SdkObjectGroupDecl {
        owner: owner.into(),
        file: "compiler/example/mmakefile.src".into(),
        line: 1,
        objects: vec![crate::sdk_objects::SdkObjectDecl {
            source: "compiler/example/entry.c".into(),
            intermediate: "${AROS_BUILD_DIR}/gen/compiler/example/entry.o".into(),
            output: "${AROS_DEVELOPER_LIB_DIR}/entry.o".into(),
            language: "C".into(),
            defines: Vec::new(),
            undefines: Vec::new(),
            options: Vec::new(),
            includes: Vec::new(),
            line: 4,
        }],
    }
}

fn literal_group(owner: &str) -> crate::literal_objects::LiteralObjectGroupDecl {
    crate::literal_objects::LiteralObjectGroupDecl {
        owner: owner.into(),
        file: "compiler/example/mmakefile.src".into(),
        line: 2,
        objects: vec![crate::literal_objects::LiteralObjectDecl {
            source: "${AROS_SOURCE_DIR}/compiler/example/entry.c".into(),
            output: "${AROS_BUILD_DIR}/gen/compiler/example/entry.o".into(),
            language: "C".into(),
            arguments: vec!["-DX=1".into(), "-UX".into(), "-DX=2".into()],
            line: 1,
        }],
    }
}

#[test]
fn literal_object_groups_are_real_selected_providers() {
    let mut graph = DependencyGraph::new();
    graph.literal_object_groups = vec![
        literal_group("literal-selected"),
        literal_group("unrelated"),
    ];
    graph.make_meta_providers.insert("literal-selected".into());
    let context = TargetContext::default();
    let selected = graph
        .selected_dependency_closure(&["literal-selected".into()], &context, &[])
        .unwrap();
    graph.retain_native_selection(&selected, &context).unwrap();
    assert_eq!(graph.literal_object_groups.len(), 1);
    assert_eq!(graph.literal_object_groups[0].owner, "literal-selected");
}

#[test]
fn assembly_header_selection_keeps_its_source_prerequisites_and_emits_real_producers() {
    let mut graph = DependencyGraph::new();
    graph
        .assembly_headers
        .push(crate::assembly_headers::AssemblyHeaderDecl {
            owner: "provider-riscv".into(),
            aggregate_owner: "headers".into(),
            aggregate_dependencies: vec!["prep".into(), "provider-riscv".into()],
            file: "compiler/include/mmakefile.src".into(),
            line: 15,
            source: "${AROS_SOURCE_DIR}/compiler/include/asm.c".into(),
            assembly_output: "${AROS_BUILD_DIR}/gen/include/asm.s".into(),
            header_output: "${AROS_BUILD_DIR}/GENINCDIR/aros/riscv/asm.h".into(),
            header_root: "${AROS_BUILD_DIR}/GENINCDIR".into(),
            arguments: vec!["-DX=1".into(), "-UX".into(), "-DX=2".into()],
            token: ".asciz".into(),
        });
    graph.add_meta_rule(MetaTargetRule {
        name: "prep".into(),
        dependencies: vec![],
    });
    let context = TargetContext::default();
    let selected = graph
        .selected_dependency_closure(&["provider-riscv".into()], &context, &[])
        .unwrap();
    assert!(selected.contains("prep"));
    graph.retain_native_selection(&selected, &context).unwrap();
    assert_eq!(graph.assembly_headers.len(), 1);
    let cmake = crate::generator::generate_cmake(&graph);
    assert!(cmake.contains("aros_generate_assembly_header("));
    assert!(cmake.contains("TOKEN \".asciz\""));
    assert!(cmake.find("-DX=1").unwrap() < cmake.find("-UX").unwrap());
    assert!(cmake.find("-UX").unwrap() < cmake.find("-DX=2").unwrap());
    assert!(!cmake.contains("if(NOT TARGET \"provider-riscv\")"));
    graph.targets.insert(
        "provider-riscv".into(),
        target("provider-riscv", ModuleType::Program),
    );
    assert!(graph
        .selected_dependency_closure(&["provider-riscv".into()], &context, &[])
        .is_err());
}

#[test]
fn assembly_headers_share_aggregates_without_sibling_dependency_cycles() {
    let mut graph = DependencyGraph::new();
    for name in ["first", "second"] {
        graph
            .assembly_headers
            .push(crate::assembly_headers::AssemblyHeaderDecl {
                owner: name.into(),
                aggregate_owner: "headers".into(),
                aggregate_dependencies: vec!["prep".into(), "first".into(), "second".into()],
                file: "compiler/include/mmakefile.src".into(),
                line: 15,
                source: format!("${{AROS_SOURCE_DIR}}/compiler/include/{name}.c"),
                assembly_output: format!("${{AROS_BUILD_DIR}}/gen/include/{name}.s"),
                header_output: format!("${{AROS_BUILD_DIR}}/GENINCDIR/aros/{name}.h"),
                header_root: "${AROS_BUILD_DIR}/GENINCDIR".into(),
                arguments: vec![],
                token: ".ascii".into(),
            });
    }
    graph.add_meta_rule(MetaTargetRule {
        name: "prep".into(),
        dependencies: vec![],
    });
    let context = TargetContext::default();
    let selected = graph
        .selected_dependency_closure(&["headers".into()], &context, &[])
        .unwrap();
    for name in ["headers", "first", "second", "prep"] {
        assert!(selected.contains(name));
    }
    let first = graph
        .selected_dependency_closure(&["first".into()], &context, &[])
        .unwrap();
    assert!(!first.contains("second"));
    graph.retain_native_selection(&selected, &context).unwrap();
    let cmake = crate::generator::generate_cmake(&graph);
    assert_eq!(cmake.matches("aros_generate_assembly_header(").count(), 2);
    for declaration in cmake.split("aros_generate_assembly_header(").skip(1) {
        let declaration = declaration.split_once("\n)").unwrap().0;
        assert!(declaration.contains("prep"));
        assert!(!declaration.contains("DEPENDS \"first\""));
        assert!(!declaration.contains("DEPENDS \"second\""));
    }
    graph.assembly_headers[1].header_output = graph.assembly_headers[0].header_output.clone();
    assert!(graph
        .selected_dependency_closure(&["headers".into()], &context, &[])
        .is_err());
}

#[test]
fn selected_architecture_objects_retain_their_module_declaration_without_a_back_edge() {
    use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
    let mut graph = DependencyGraph::new();
    let context = TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        ..TargetContext::default()
    };
    // Inventory preparation uses the same declaration retention contract
    // as a full compile graph, without fabricating compile targets.
    graph
        .arch_sources
        .insert("kernel-example".into(), Vec::new());
    graph.arch_sources.insert("unrelated".into(), Vec::new());
    graph.arch_endpoint_effects.push(ArchEndpointEffect {
        recipe: "arch/riscv-all/example/mmakefile.src".into(),
        line: 1,
        endpoint: "kernel-example-riscv".into(),
        dependencies: vec!["kernel-example-riscv-includes".into()],
        data: ArchEndpointEffectData::ArchModuleObjects {
            mainmmake: "kernel-example".into(),
            tag: "riscv".into(),
            module_sources: vec!["entry".into()],
            directory: "arch/riscv-all/example".into(),
        },
    });
    let selected = BTreeSet::from([
        "kernel-example-riscv".into(),
        "kernel-example-riscv-includes".into(),
    ]);
    let edges = graph.selection_edges(&[], false, &context).edges;
    assert_eq!(
        edges["kernel-example-riscv"],
        BTreeSet::from(["kernel-example-riscv-includes".into()])
    );
    graph.retain_native_selection(&selected, &context).unwrap();
    assert!(graph.arch_sources.contains_key("kernel-example"));
    assert!(!graph.arch_sources.contains_key("unrelated"));
    assert_eq!(graph.arch_endpoint_effects.len(), 1);
}

#[test]
fn native_selection_normalizes_handwritten_edge_provenance_with_the_graph() {
    let mut graph = DependencyGraph::new();
    graph.add_explicit_meta_rule(MetaTargetRule {
        name: "consumer-${AROS_TARGET_CPU}".into(),
        dependencies: vec!["linklibs-${AROS_TARGET_CPU}".into()],
    });
    let context = TargetContext {
        cpu: Some("riscv".into()),
        ..TargetContext::default()
    };
    let selected = ["consumer-riscv".into(), "linklibs-riscv".into()]
        .into_iter()
        .collect();
    graph.retain_native_selection(&selected, &context).unwrap();
    assert!(graph.meta_targets["consumer-riscv"].contains("linklibs-riscv"));
    assert_eq!(
        graph.explicit_meta_edges,
        std::iter::once(("consumer-riscv".into(), "linklibs-riscv".into())).collect()
    );
}

#[test]
fn a_library_reached_only_through_its_fd_alias_is_reduced_to_its_headers() {
    let mut graph = DependencyGraph::new();
    let mut header_only = target("header-only", ModuleType::Library);
    header_only.genmodule_abi = true;
    header_only.source_files = vec!["lib.c".into()];
    header_only.use_libs = vec!["absent".into()];
    graph.targets.insert("header-only".into(), header_only);
    let mut built = target("built", ModuleType::Library);
    built.genmodule_abi = true;
    built.source_files = vec!["lib.c".into()];
    built.use_libs = vec!["absent".into()];
    graph.targets.insert("built".into(), built);
    let context = TargetContext::default();
    let selected = BTreeSet::from(["header-only-fd".into(), "built".into()]);
    graph.retain_native_selection(&selected, &context).unwrap();

    let reduced = &graph.targets["header-only"];
    assert_eq!(reduced.module_type, ModuleType::ModuleHeaders);
    assert_eq!(reduced.declared_mod_type.as_deref(), Some("library"));
    let cmake = crate::generator::generate_cmake(&graph);
    assert!(cmake.contains("aros_add_module_headers("));
    assert!(cmake.contains("MODTYPE \"library\""));
    assert!(reduced.source_files.is_empty() && reduced.use_libs.is_empty());
    // A selected runtime keeps every request it declared.
    let kept = &graph.targets["built"];
    assert_eq!(kept.module_type, ModuleType::Library);
    assert_eq!(kept.use_libs, ["absent"]);
    // Only the built module can fail the link-library resolution.
    let unresolved = graph.resolve_use_libs();
    assert!(unresolved.iter().all(|item| !item.contains("header-only")));
}

#[test]
fn a_kept_header_transform_lists_only_the_consumers_the_selection_kept() {
    let mut graph = DependencyGraph::new();
    graph.header_transforms.push(
        serde_json::from_value(serde_json::json!({
            "name": "header", "file": "f.src", "line": 1, "input": "in.h",
            "output": "out.h", "match_text": "a", "replacement": "b",
            "consumers": ["kept", "dropped"],
        }))
        .unwrap(),
    );
    let selected = BTreeSet::from(["header".to_owned(), "kept".to_owned()]);
    graph
        .retain_native_selection(&selected, &TargetContext::default())
        .unwrap();
    assert_eq!(graph.header_transforms[0].consumers, ["kept"]);
}

#[test]
fn literal_object_groups_refuse_conflicting_kinds_and_outputs() {
    let context = TargetContext::default();
    let mut graph = DependencyGraph::new();
    graph.literal_object_groups = vec![literal_group("shared")];
    graph
        .directory_setups
        .push(crate::directory_setup::DirectorySetupDecl {
            owner: "shared".into(),
            directories: vec!["${AROS_BUILD_DIR}/gen/compiler/example".into()],
        });
    assert!(graph
        .selected_dependency_closure(&["shared".into()], &context, &[])
        .is_err());
    graph.directory_setups.clear();
    graph.literal_object_groups.push(literal_group("other"));
    graph.literal_object_groups[1].objects[0]
        .arguments
        .push("-g".into());
    assert!(graph
        .selected_dependency_closure(&["shared".into(), "other".into()], &context, &[])
        .is_err());
    graph.literal_object_groups.pop();
    graph.sdk_object_groups.push(sdk_group("sdk"));
    assert!(graph
        .selected_dependency_closure(&["shared".into(), "sdk".into()], &context, &[])
        .is_err());
}

#[test]
fn sdk_object_groups_are_real_providers_and_prune_unselected_groups() {
    let mut graph = DependencyGraph::new();
    graph.sdk_object_groups = vec![sdk_group("sdk-selected"), sdk_group("sdk-unrelated")];
    graph.make_meta_providers.insert("sdk-selected".into());
    let context = TargetContext::default();
    let selected = graph
        .selected_dependency_closure(&["sdk-selected".into()], &context, &[])
        .unwrap();
    graph.retain_native_selection(&selected, &context).unwrap();
    assert_eq!(graph.sdk_object_groups.len(), 1);
    assert_eq!(graph.sdk_object_groups[0].owner, "sdk-selected");
}

#[test]
fn shared_sdk_objects_require_identical_declarations_and_unique_provider_kinds() {
    let mut graph = DependencyGraph::new();
    graph.sdk_object_groups = vec![sdk_group("sdk-a"), sdk_group("sdk-b")];
    let roots = ["sdk-a".into(), "sdk-b".into()];
    let context = TargetContext::default();
    assert!(graph
        .selected_dependency_closure(&roots, &context, &[])
        .is_ok());
    graph.sdk_object_groups[1].objects[0]
        .defines
        .push("ALTERED=1".into());
    assert!(graph
        .selected_dependency_closure(&roots, &context, &[])
        .unwrap_err()
        .to_string()
        .contains("conflicting ownership"));
    graph.sdk_object_groups[1].objects[0].defines.clear();
    graph
        .directory_setups
        .push(crate::directory_setup::DirectorySetupDecl {
            owner: "sdk-a".into(),
            directories: vec!["${AROS_BUILD_DIR}/gen/compiler/example".into()],
        });
    assert!(graph
        .selected_dependency_closure(&roots, &context, &[])
        .unwrap_err()
        .to_string()
        .contains("conflicting concrete producers"));
}

#[test]
fn native_binary_dependencies_preserve_the_cmake_architecture_gate() {
    let context = TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        ..TargetContext::default()
    };
    let mut graph = DependencyGraph::default();
    graph.add_target(target("kernel", ModuleType::Program));
    for tag in ["pc-i386", "pc-x86_64", "esp32p4-riscv"] {
        graph
            .binary_objects
            .push(crate::binary_objects::BinaryObjectDecl {
                name: "same-image".into(),
                output: format!("{tag}/wrapped.o"),
                directory: format!("arch/{tag}"),
                sources: vec!["image".into()],
                start: "0".into(),
                ldflags: Vec::new(),
                consumer: "kernel".into(),
                arch_tag: tag.into(),
            });
    }
    let selected = graph
        .selected_dependency_closure(&["kernel".into()], &context, &[])
        .unwrap();
    assert!(selected.contains("same-image"));
    graph.retain_native_selection(&selected, &context).unwrap();
    assert_eq!(graph.binary_objects.len(), 1);
    assert_eq!(graph.binary_objects[0].arch_tag, "esp32p4-riscv");
    graph.binary_objects.clear();
    graph
        .binary_objects
        .push(crate::binary_objects::BinaryObjectDecl {
            name: "foreign".into(),
            output: "foreign/wrapped.o".into(),
            directory: "arch/x86_64-pc".into(),
            sources: vec!["image".into()],
            start: "0".into(),
            ldflags: Vec::new(),
            consumer: "kernel".into(),
            arch_tag: "pc-x86_64".into(),
        });
    let selected = graph
        .selected_dependency_closure(&["kernel".into()], &context, &[])
        .unwrap();
    assert!(!selected.contains("foreign"));
}

fn target(name: &str, module_type: ModuleType) -> TargetDefinition {
    TargetDefinition {
        mmake_name: name.to_owned(),
        target_name: name.to_owned(),
        module_type,
        module_macro: None,
        kobj_scoped_inputs: None,
        genmodule_only: false,
        genmodule_abi: false,
        empty_archive: false,
        source_files: Vec::new(),
        cxx_source_files: Vec::new(),
        always_cxx_link: false,
        no_startup: false,
        detach: false,
        objc_source_files: Vec::new(),
        asm_source_files: Vec::new(),
        use_libs: Vec::new(),
        dependencies: Vec::new(),
        dir_path: std::path::PathBuf::new(),
        target_dir: None,
        variant_32bit: false,
        link_libs: Vec::new(),
        declared_mod_type: None,
        mod_suffix: None,
        linklib_name: None,
        config_file: None,
        config_override_file: None,
        genmodule_linklibs: None,
        config_relative_libraries: Vec::new(),
        linklib_output_dir: None,
        canonical_linklib_output: false,
        canonical_linklib_eligible: false,
        compiler_flags: Vec::new(),
        include_dirs: Vec::new(),
        arch_modules: Vec::new(),
        arch_includes: Vec::new(),
        defines: Vec::new(),
        undefines: Vec::new(),
        compile_options: Vec::new(),
        link_options: Vec::new(),
        spec_switches: Vec::new(),
        driver_link_options: Vec::new(),
        isa_link_options: Vec::new(),
        arch_sources: Vec::new(),
        arch_defines: Vec::new(),
        arch_compile_options: Vec::new(),
        arch_source_options: Vec::new(),
        selection_headers_only: false,
    }
}

fn native_contract_with_link_libraries(names: &[&str]) -> NativeBuildContract {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "profile": "fixture",
        "board": "fixture-board",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "qualification": "experimental-unqualified",
        "inputs": [],
        "abi": {
            "source_cpu": "riscv",
            "target_triple": "riscv-aros",
            "isa": "rv32imafc",
            "abi": "ilp32f",
            "code_model": "medany",
            "flavour": "standalone",
            "platform_smp": false,
            "use_mmu": false
        },
        "core": {
            "recipe": "kernel/makefile.src",
            "linker_script": "kernel/linker.lds",
            "resources": [],
            "libraries": [],
            "devices": [],
            "link_libraries": names,
            "compiler_runtime_role": "libgcc",
            "residency_check": "kernel/check.sh",
            "residency_policy": {
                "algorithm": "riscv32-xip-v1",
                "section": ".sramtext",
                "flash_start": 1_073_741_824,
                "flash_end": 1_140_850_688,
                "sram_start": 1_341_128_704,
                "sram_end": 1_341_652_992
            }
        },
        "package": {
            "recipe": "package/makefile.src",
            "format": "aros-pkg-v1",
            "target": "fixture-package",
            "limit_from_board": "FIXTURE_PACKAGE_LIMIT"
        },
        "media": {
            "chip": "fixture-chip",
            "board_rules": "board/board.mk",
            "partition_table": "boot/partitions.csv",
            "core_partition": "core",
            "package_partition": "package",
            "development_volume_offset_from_board": "FIXTURE_VOLUME_OFFSET",
            "bootloader_configuration": "boot/config.defaults",
            "bootloader_patch": "boot/fixture.diff",
            "idf_version": "6.0.1"
        }
    }))
    .unwrap()
}

fn bound_source_archive(
    owner: &str,
    library: &str,
    file: &str,
) -> crate::source_archive_binding::BoundSourceArchive {
    crate::source_archive_binding::BoundSourceArchive {
        declaration: crate::source_archive_rules::SourceArchiveDecl {
            owner: owner.into(),
            file: file.into(),
            line: 12,
            owner_line: 8,
            output: format!("${{AROS_BUILD_DIR}}/SYS/Developer/lib/lib{library}.a"),
            members: crate::source_archive_rules::ArchiveMembers::Exact(Vec::new()),
        },
        command: crate::source_archive_command::SourceArchiveCommand {
            flags: vec!["cr".into()],
        },
        members: Vec::new(),
        compile_groups: Vec::new(),
        producer_owners: BTreeSet::new(),
    }
}

fn add_empty_native_package(graph: &mut DependencyGraph) {
    graph.packages.push(crate::packages::PackageDecl {
        file: "package/makefile.src".into(),
        mmake: "fixture-package".into(),
        output: "${AROS_BUILD_DIR}/fixture.pkg".into(),
        members: Vec::new(),
        startup: None,
        uselibs: Vec::new(),
        is_kickstart: false,
        resolved: Vec::new(),
        arch: String::new(),
    });
}

fn native_context() -> TargetContext {
    TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        ..TargetContext::default()
    }
}

#[test]
fn native_contract_roots_bind_exact_source_archive_names() {
    let mut graph = DependencyGraph::new();
    graph.source_archives.push(bound_source_archive(
        "linklibs-foo-source",
        "foo",
        "compiler/libfoo/mmakefile.src",
    ));
    graph.source_archives.push(bound_source_archive(
        "linklibs-foobar-source",
        "foobar",
        "compiler/libfoobar/mmakefile.src",
    ));
    add_empty_native_package(&mut graph);

    let roots = graph
        .native_contract_roots(
            &native_contract_with_link_libraries(&["foo"]),
            &native_context(),
        )
        .unwrap();

    assert!(roots.contains(&"linklibs-foo-source-archive".to_owned()));
    assert!(!roots.contains(&"linklibs-foobar-source-archive".to_owned()));
}

#[test]
fn native_contract_archive_root_rejects_source_and_target_collision() {
    let mut graph = DependencyGraph::new();
    graph.source_archives.push(bound_source_archive(
        "linklibs-foo-source",
        "foo",
        "compiler/libfoo/mmakefile.src",
    ));
    let mut target = target("native-foo", ModuleType::LinkLib);
    target.target_name = "foo".into();
    target.canonical_linklib_output = true;
    graph.targets.insert(target.mmake_name.clone(), target);
    add_empty_native_package(&mut graph);

    let error = graph
        .native_contract_roots(
            &native_contract_with_link_libraries(&["foo"]),
            &native_context(),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("native archive foo requires one applicable producer"));
    assert!(error.to_string().contains("native-foo"));
    assert!(error.to_string().contains("linklibs-foo-source-archive"));
}

#[test]
fn native_contract_archive_root_keeps_architecture_and_32bit_filters() {
    let mut graph = DependencyGraph::new();
    graph.source_archives.push(bound_source_archive(
        "linklibs-foo-source",
        "foo",
        "compiler/libfoo/mmakefile.src",
    ));
    let mut legacy = target("legacy-foo", ModuleType::LinkLib);
    legacy.target_name = "foo".into();
    legacy.variant_32bit = true;
    legacy.canonical_linklib_output = true;
    graph.targets.insert(legacy.mmake_name.clone(), legacy);
    add_empty_native_package(&mut graph);

    let roots = graph
        .native_contract_roots(
            &native_contract_with_link_libraries(&["foo"]),
            &native_context(),
        )
        .unwrap();
    assert!(roots.contains(&"linklibs-foo-source-archive".to_owned()));

    graph.source_archives[0].declaration.file = "arch/i386-pc/compiler/libfoo/mmakefile.src".into();
    let error = graph
        .native_contract_roots(
            &native_contract_with_link_libraries(&["foo"]),
            &native_context(),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("native archive foo requires one applicable producer"));
    assert!(error.to_string().contains("found []"));
}

fn fetch(name: &str, destination: &str) -> FetchDecl {
    FetchDecl {
        name: name.to_owned(),
        archive: name.to_owned(),
        suffixes: "tar.gz".to_owned(),
        origins: "cache://".to_owned(),
        checksums: String::new(),
        location: "${AROS_PORTS_SOURCE_DIR}".to_owned(),
        destination: destination.to_owned(),
        base: destination.to_owned(),
        patch_origins: String::new(),
        patches: String::new(),
        dir: "ports".to_owned(),
    }
}

fn copy_includes(name: &str, source_dir: &str) -> crate::CopyIncludesDecl {
    crate::CopyIncludesDecl {
        name: name.to_owned(),
        dest: "fixture".to_owned(),
        source_dir: source_dir.to_owned(),
        patterns: vec!["fixture.h".to_owned()],
        excludes: Vec::new(),
        flatten: false,
        proven_empty: false,
    }
}

fn diagnostic(owner: Option<&str>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::CapabilityDrift,
        DiagnosticStage::CapabilityValidation,
        "fixture capability is unavailable",
    )
    .with_context(DiagnosticContext {
        target: owner.map(str::to_owned),
        ..DiagnosticContext::default()
    })
}

fn optional_variant() -> NativeOptionalMetaDependency {
    NativeOptionalMetaDependency {
        recipe: "compiler/mmakefile.src".into(),
        target: "includes".into(),
        dependency: "includes-${AROS_TARGET_PLATFORM}-${AROS_TARGET_VARIANT}".into(),
        absence: NativeMetaAbsence::Selector,
    }
}

fn optional_graph() -> (DependencyGraph, TargetContext) {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "includes".into(),
        dependencies: vec![optional_variant().dependency, "required-headers".into()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "required-headers".into(),
        dependencies: vec![],
    });
    let context = TargetContext {
        platform: Some("fixture".into()),
        variant: Some(String::new()),
        ..TargetContext::default()
    };
    (graph, context)
}

#[test]
fn optional_selector_cannot_erase_an_active_virtual_dependency() {
    let (mut graph, context) = optional_graph();
    assert!(graph
        .selected_dependency_closure(&["includes".into()], &context, &[])
        .is_err());
    let error = graph
        .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("includes-fixture-"));
    assert!(error.contains("fix upstream"));
    assert!(error.contains("compiler/mmakefile.src"));
    assert!(graph.meta_targets["includes"].contains(&optional_variant().dependency));
    graph
        .meta_targets
        .get_mut("includes")
        .unwrap()
        .insert("literal-typo".into());
    assert!(graph
        .selected_dependency_closure(&["includes".into()], &context, &[])
        .is_err());
}

#[test]
fn optional_selector_preserves_present_and_rejected_providers() {
    for rejected in [false, true] {
        let (mut graph, context) = optional_graph();
        graph.add_meta_rule(MetaTargetRule {
            name: "includes-fixture-".into(),
            dependencies: vec![],
        });
        let diagnostics = if rejected {
            graph.make_meta_providers.insert("includes-fixture-".into());
            vec![diagnostic(Some("includes-fixture-"))]
        } else {
            vec![]
        };
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &diagnostics)
            .unwrap()
            .is_empty());
        let selected =
            graph.selected_dependency_closure(&["includes".into()], &context, &diagnostics);
        if rejected {
            assert!(selected
                .unwrap_err()
                .to_string()
                .contains("fixture capability"));
        } else {
            assert!(selected.unwrap().contains("includes-fixture-"));
        }
    }
}

#[test]
fn disabled_owner_dependency_is_an_upstream_error_not_an_optional_omission() {
    let declaration = NativeOptionalMetaDependency {
        recipe: "compiler/mmakefile.src".into(),
        target: "includes".into(),
        dependency: "disabled-pkgconfig".into(),
        absence: NativeMetaAbsence::DisabledOwner,
    };
    let context = TargetContext::default();
    for active in [false, true] {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "includes".into(),
            dependencies: vec![declaration.dependency.clone()],
        });
        let mut attributed = diagnostic(Some(&declaration.dependency));
        attributed.context.as_mut().unwrap().mode =
            Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
        let mut diagnostics = vec![attributed];
        if active {
            // A separate active-but-rejected declaration still vetoes omission.
            diagnostics.push(diagnostic(Some(&declaration.dependency)));
        }
        let error = graph.omit_absent_optional_meta_dependencies(
            std::slice::from_ref(&declaration),
            &context,
            &diagnostics,
        );
        if active {
            assert!(error.unwrap().is_empty());
        } else {
            let error = error.unwrap_err().to_string();
            assert!(error.contains("fix upstream"));
            assert!(error.contains("compiler/mmakefile.src"));
            assert!(error.contains("disabled-pkgconfig"));
        }
        assert!(graph.meta_targets["includes"].contains("disabled-pkgconfig"));
        assert!(graph
            .selected_dependency_closure(&["includes".into()], &context, &diagnostics)
            .is_err());
    }
}

#[test]
fn disabled_owner_attribution_never_hides_a_declared_provider() {
    let declaration = NativeOptionalMetaDependency {
        recipe: "compiler/mmakefile.src".into(),
        target: "includes".into(),
        dependency: "disabled-pkgconfig".into(),
        absence: NativeMetaAbsence::DisabledOwner,
    };
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "includes".into(),
        dependencies: vec![declaration.dependency.clone()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: declaration.dependency.clone(),
        dependencies: vec![],
    });
    let mut attributed = diagnostic(Some(&declaration.dependency));
    attributed.context.as_mut().unwrap().mode =
        Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
    let diagnostics = [attributed];
    let context = TargetContext::default();
    assert!(graph
        .omit_absent_optional_meta_dependencies(&[declaration], &context, &diagnostics)
        .unwrap()
        .is_empty());
    assert!(graph
        .selected_dependency_closure(&["includes".into()], &context, &diagnostics)
        .is_err());
}

#[test]
fn disabled_owner_attribution_without_optional_contract_remains_fatal() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "includes".into(),
        dependencies: vec!["disabled-pkgconfig".into()],
    });
    let mut attributed = diagnostic(Some("disabled-pkgconfig"));
    attributed.context.as_mut().unwrap().mode =
        Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
    assert!(graph
        .selected_dependency_closure(
            &["includes".into()],
            &TargetContext::default(),
            &[attributed],
        )
        .is_err());
}

#[test]
fn optional_selectors_reject_unknown_literal_and_unproven_edges() {
    let (graph, mut context) = optional_graph();
    context.variant = None;
    assert!(graph
        .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &[])
        .is_err());
    // Validation is transactional: the failed probe leaves the graph intact.
    assert!(graph.meta_targets["includes"].contains(&optional_variant().dependency));
    context.variant = Some(String::new());
    let mut wrong = optional_variant();
    wrong.dependency = "literal-typo".into();
    assert!(graph
        .omit_absent_optional_meta_dependencies(&[wrong], &context, &[])
        .is_err());
    wrong = optional_variant();
    wrong.target = "unproven".into();
    assert!(graph
        .omit_absent_optional_meta_dependencies(&[wrong], &context, &[])
        .is_err());
}

#[test]
fn optional_architecture_edge_does_not_prove_concrete_owner_capability() {
    for supported in [false, true] {
        let (mut graph, context) = optional_graph();
        graph.make_meta_providers.insert("includes".into());
        if supported {
            graph.add_target(target("includes", ModuleType::LinkLib));
        }
        let diagnostics = if supported {
            vec![]
        } else {
            vec![diagnostic(Some("includes"))]
        };
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &diagnostics)
            .is_err());
        let selected =
            graph.selected_dependency_closure(&["includes".into()], &context, &diagnostics);
        assert!(selected.is_err());
    }
}

#[test]
fn exact_closed_roots_exclude_unrelated_rejected_capabilities() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "core".into(),
        dependencies: vec!["archive".into()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "archive".into(),
        dependencies: vec!["headers".into()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "headers".into(),
        dependencies: vec![],
    });
    let result = graph
        .selected_dependency_closure(
            &["core".into()],
            &TargetContext::default(),
            &[diagnostic(Some("unrelated"))],
        )
        .unwrap();
    assert_eq!(
        result,
        BTreeSet::from(["core".into(), "archive".into(), "headers".into()])
    );
}

#[test]
fn unselected_dynamic_aggregate_does_not_relax_selected_endpoints() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "core".into(),
        dependencies: vec![],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "icons-${UNKNOWN}".into(),
        dependencies: vec!["also-${UNKNOWN}".into()],
    });
    assert!(graph
        .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
        .is_ok());
    assert!(graph
        .selected_dependency_closure(&["icons-${UNKNOWN}".into()], &TargetContext::default(), &[])
        .is_err());
    graph.add_meta_rule(MetaTargetRule {
        name: "core".into(),
        dependencies: vec!["icons-${UNKNOWN}".into()],
    });
    assert!(graph
        .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
        .is_err());
}

#[test]
fn selected_unowned_missing_and_dynamic_dependencies_fail_closed() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "core".into(),
        dependencies: vec!["rejected".into()],
    });
    assert!(graph
        .selected_dependency_closure(
            &["core".into()],
            &TargetContext::default(),
            &[diagnostic(Some("rejected"))]
        )
        .is_err());
    assert!(graph
        .selected_dependency_closure(
            &["core".into()],
            &TargetContext::default(),
            &[diagnostic(None)]
        )
        .is_err());
    assert!(graph
        .selected_dependency_closure(&["missing".into()], &TargetContext::default(), &[])
        .is_err());
    assert!(graph
        .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
        .is_err());
    assert!(graph
        .selected_dependency_closure(&[], &TargetContext::default(), &[])
        .is_err());
    assert!(endpoint("core-${AROS_TARGET_CPU}", &TargetContext::default()).is_err());
    assert!(endpoint("core-${UNKNOWN}", &TargetContext::default()).is_err());
}

#[test]
fn dependency_selector_uses_explicit_source_context() {
    let context = TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("fixture".into()),
        variant: Some(String::new()),
        ..TargetContext::default()
    };
    assert_eq!(
        endpoint("kernel-${AROS_TARGET_LEGACY_PLATFORM}", &context).unwrap(),
        "kernel-fixture-riscv"
    );
    let unknown_variant = TargetContext {
        variant: None,
        ..context.clone()
    };
    assert!(endpoint("kernel-${AROS_TARGET_LEGACY_PLATFORM}", &unknown_variant).is_err());
    assert!(endpoint("../core", &context).is_err());
}

#[test]
fn genmodule_only_library_does_not_invent_a_relative_archive_endpoint() {
    let mut graph = DependencyGraph::new();
    let mut library = target("fixture-library", ModuleType::Library);
    library.genmodule_only = true;
    library.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..GenmoduleLinklibs::default()
    });
    graph.targets.insert(library.mmake_name.clone(), library);
    let error = graph
        .selected_dependency_closure(
            &["fixture-library-linklib-rel".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .unwrap_err();
    assert!(error.to_string().contains("has no proven endpoint"));
}

#[test]
fn genmodule_only_library_archive_selects_runtime_dependencies_and_guarded_defaults() {
    let mut graph = DependencyGraph::new();
    let mut library = target("fixture-library", ModuleType::Library);
    library.genmodule_only = true;
    library.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        ..GenmoduleLinklibs::default()
    });
    library.dependencies = vec!["runtime-prerequisite".to_owned()];
    library.link_libs = vec!["runtime-link-provider".to_owned()];
    library.spec_switches = vec!["static".to_owned()];
    graph.targets.insert(library.mmake_name.clone(), library);
    for name in [
        "fixture-library-includes",
        "runtime-prerequisite",
        "runtime-link-provider",
        "stdc-static-provider",
        "stdc-dynamic-provider",
    ] {
        graph.add_meta_rule(MetaTargetRule {
            name: name.to_owned(),
            dependencies: Vec::new(),
        });
    }
    graph.add_meta_rule(MetaTargetRule {
        name: "fixture-library-linklib".to_owned(),
        dependencies: vec!["fixture-library-includes".to_owned()],
    });
    graph.default_link_set = vec![
        crate::graph::ResolvedDefaultLinkItem {
            name: "stdc.static".to_owned(),
            archive: "stdc-static-provider".to_owned(),
            require_absent: Vec::new(),
            require_present: vec!["nostdc".to_owned()],
        },
        crate::graph::ResolvedDefaultLinkItem {
            name: "stdc".to_owned(),
            archive: "stdc-dynamic-provider".to_owned(),
            require_absent: vec!["nostdc".to_owned()],
            require_present: Vec::new(),
        },
    ];

    let selected = graph
        .selected_dependency_closure(
            &["fixture-library-linklib".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .unwrap();

    assert!(selected.contains("fixture-library"));
    assert!(selected.contains("fixture-library-includes"));
    assert!(selected.contains("runtime-prerequisite"));
    assert!(selected.contains("runtime-link-provider"));
    assert!(selected.contains("stdc-static-provider"));
    assert!(!selected.contains("stdc-dynamic-provider"));
}

#[test]
fn full_library_relative_archive_alias_selects_runtime_but_abi_alias_does_not() {
    let mut graph = DependencyGraph::new();
    let mut library = target("full-library", ModuleType::Library);
    library.source_files = vec!["library.c".to_owned()];
    library.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..GenmoduleLinklibs::default()
    });
    graph.targets.insert(library.mmake_name.clone(), library);
    let abi = target("abi-only", ModuleType::Abi);
    graph.targets.insert(abi.mmake_name.clone(), abi);

    let relative = graph
        .selected_dependency_closure(
            &["full-library-linklib-rel".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .unwrap();
    assert!(relative.contains("full-library"));

    graph
        .default_link_set
        .push(crate::graph::ResolvedDefaultLinkItem {
            name: "archive".to_owned(),
            archive: "default-archive".to_owned(),
            require_absent: Vec::new(),
            require_present: Vec::new(),
        });
    graph.add_meta_rule(MetaTargetRule {
        name: "default-archive".to_owned(),
        dependencies: Vec::new(),
    });
    let abi_archive = graph
        .selected_dependency_closure(
            &["abi-only-linklib".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .unwrap();
    assert!(abi_archive.contains("abi-only-linklib"));
    assert!(!abi_archive.contains("abi-only"));
    assert!(!abi_archive.contains("default-archive"));
}

#[test]
fn fetched_copy_includes_uses_the_unique_longest_owner() {
    let mut graph = DependencyGraph::new();
    graph.fetches = vec![
        fetch("broad-fetch", "${AROS_PORTS_DIR}/fixture"),
        fetch("header-fetch", "${AROS_PORTS_DIR}/fixture/source"),
    ];
    graph.copy_includes = vec![copy_includes(
        "fixture-headers",
        "${AROS_PORTS_DIR}/fixture/source/include",
    )];

    let selected = graph
        .selected_dependency_closure(
            &["fixture-headers".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .unwrap();

    assert!(selected.contains("header-fetch"));
    assert!(!selected.contains("broad-fetch"));
}

#[test]
fn fetched_copy_includes_reject_selected_ties_but_ignore_unselected_ties() {
    let mut graph = DependencyGraph::new();
    graph.fetches = vec![
        fetch("first-fetch", "${AROS_PORTS_DIR}/fixture"),
        fetch("second-fetch", "${AROS_PORTS_DIR}/fixture"),
    ];
    graph.copy_includes = vec![copy_includes(
        "fixture-headers",
        "${AROS_PORTS_DIR}/fixture/include",
    )];
    graph.add_meta_rule(MetaTargetRule {
        name: "unrelated-root".to_owned(),
        dependencies: Vec::new(),
    });

    assert!(graph
        .selected_dependency_closure(
            &["unrelated-root".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .is_ok());
    assert!(graph
        .selected_dependency_closure(
            &["fixture-headers".to_owned()],
            &TargetContext::default(),
            &[],
        )
        .is_err());
}
