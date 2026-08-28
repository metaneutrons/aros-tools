use super::{generate_cmake, generated_header};
use crate::ast::{CopyDirectoryDecl, MetaTargetRule};
use crate::catalogs::CatalogDecl;
use crate::copy_includes::CopyIncludesDecl;
use crate::dirs::DirVars;
use crate::fetch::FetchDecl;
use crate::graph::DependencyGraph;
use crate::icons::IconTarget;
use crate::packages::{PackageDecl, ResolvedPackageMember};
use crate::parse_mmakefile_with_dirs;
use std::path::Path;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn icon(name: &str) -> IconTarget {
    IconTarget {
        mmake: name.to_owned(),
        directory: "images/icons".to_owned(),
    }
}

#[test]
fn unixio_public_header_is_the_exact_foreign_arch_exception() {
    let mut graph = DependencyGraph::new();
    graph.add_copy_includes(vec![CopyIncludesDecl {
        name: "includes-copy".to_owned(),
        dest: "hidd".to_owned(),
        source_dir: "arch/all-unix/hidd/unixio/include".to_owned(),
        patterns: vec!["*.h".to_owned()],
        excludes: Vec::new(),
        flatten: true,
    }]);

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains(
        "SOURCE \"arch/all-unix/hidd/unixio/include\" PATTERNS \"*.h\" FLATTEN ALLOW_FOREIGN_ARCH"
    ));
}

#[test]
fn a_catalog_is_a_real_mmake_target_with_all_outputs_described() {
    let mut graph = DependencyGraph::new();
    graph.add_catalogs(vec![CatalogDecl {
        mmake: "sample-catalogs".to_owned(),
        name: "Sample".to_owned(),
        subdir: "System/Tools".to_owned(),
        catalogs: vec!["german".to_owned(), "polish".to_owned()],
        source: Some("../strings.h".to_owned()),
        description: "sample".to_owned(),
        dir: "${AROS_BUILD_DIR}/SYS/Locale/Catalogs".to_owned(),
        source_description: "${AROS_BUILD_DIR}/hosttools/C_h_aros".to_owned(),
        srcdir: "${CMAKE_SOURCE_DIR}/workbench/tools/sample/catalogs".to_owned(),
        declaring_dir: "workbench/tools/sample/catalogs".to_owned(),
        line: 12,
        consumers: vec![
            "sample-consumer".to_owned(),
            "sample-program-locale".to_owned(),
        ],
    }]);
    graph.add_meta_rule(MetaTargetRule {
        name: "workbench".to_owned(),
        dependencies: vec!["sample-catalogs".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("aros_build_catalogs(\n    MMAKE_ID sample-catalogs"));
    assert!(cmake.contains("    NAME \"Sample\""));
    assert!(cmake.contains("    SUBDIR \"System/Tools\""));
    assert!(cmake.contains("    SOURCE \"../strings.h\""));
    assert!(cmake.contains("    CONSUMERS \"sample-consumer\" \"sample-program-locale\""));
    assert!(cmake.contains("    LANGUAGES \"german\" \"polish\""));
    assert!(cmake.contains("aros_add_target_dependency(\"workbench\" \"${dep}\")"));
    assert!(!cmake.contains("add_custom_target(\"sample-catalogs\")"));
}

#[test]
fn hand_written_flexcat_source_is_declared_before_and_bound_after_its_mcp() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs(
        &root.join("workbench/classes/zune/nlist/nlistviews_mcp/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap();
    assert!(parsed.skipped_flexcat_sources.is_empty());

    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_flexcat_sources(parsed.flexcat_sources);
    graph.resolve_flexcat_source_consumers();

    let declaration = graph
        .flexcat_sources
        .iter()
        .find(|declaration| declaration.owner == "classes-zune-nlistviews-mcp-catalogs")
        .expect("NListviews FlexCat declaration");
    assert_eq!(
        declaration.consumers,
        [
            "classes-zune-nlistviews-mcp",
            "classes-zune-nlistviews-mcp-test"
        ]
    );

    let cmake = generate_cmake(&graph);
    let declaration_at = cmake.find("aros_declare_flexcat_sources(").unwrap();
    let module_at = cmake.find("MMAKE_ID classes-zune-nlistviews-mcp").unwrap();
    let binding_at = cmake.find("aros_bind_flexcat_source_consumers(").unwrap();
    assert!(
        declaration_at < module_at && module_at < binding_at,
        "{cmake}"
    );
    assert!(cmake.contains(
        "CATALOG_NAME \"NListviews_mcp\"\n    CATALOG_SOURCE_DIR \"locale\"\n    LANGUAGES"
    ));
    assert!(cmake.contains(
            "OWNER \"classes-zune-nlistviews-mcp-catalogs\"\n    CONSUMERS \"classes-zune-nlistviews-mcp\" \"classes-zune-nlistviews-mcp-test\""
        ));
}

#[test]
fn header_only_flexcat_owner_is_concrete_before_openurl() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs(
        &root.join("external/openurl/prefs/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap();
    assert!(parsed.skipped_flexcat_sources.is_empty());

    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_flexcat_headers(parsed.flexcat_headers);
    for rule in parsed.meta_rules {
        graph.add_meta_rule(rule);
    }

    let cmake = generate_cmake(&graph);
    let declaration_at = cmake.find("aros_declare_flexcat_header(").unwrap();
    let module_at = cmake.find("MMAKE_ID external-openurl-prefs").unwrap();
    assert!(declaration_at < module_at, "{cmake}");
    assert!(cmake.contains(
            "OWNER \"external-openurl-prefs-setup\"\n    DIRECTORY \"external/openurl/prefs\"\n    HEADER \"locale.h\""
        ));
    assert!(
        cmake.contains("DESCRIPTION \"${CMAKE_SOURCE_DIR}/external/openurl/locale/OpenURL.pot\"")
    );
    assert!(
        !cmake.contains("if(NOT TARGET \"external-openurl-prefs-setup\")\n    add_custom_target")
    );
}

#[test]
fn an_icon_target_can_also_receive_meta_dependencies() {
    let mut graph = DependencyGraph::new();
    graph.add_icons(vec![icon("icons"), icon("leaf")], Vec::new());
    graph.add_meta_rule(MetaTargetRule {
        name: "icons".to_owned(),
        dependencies: vec!["leaf".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("aros_add_target_dependency(\"icons\" \"${dep}\")"));
    assert!(!cmake.contains("add_custom_target(\"icons\")"));
}

#[test]
fn fetch_targets_survive_meta_dependency_filtering() {
    let mut graph = DependencyGraph::new();
    graph.add_fetches(vec![FetchDecl {
        name: "example-fetch".to_owned(),
        archive: "example-1.0".to_owned(),
        suffixes: "tar.gz".to_owned(),
        origins: "https://example.invalid".to_owned(),
        location: "${AROS_PORTS_SOURCE_DIR}".to_owned(),
        destination: "${AROS_PORTS_DIR}".to_owned(),
        base: String::new(),
        patch_origins: String::new(),
        patches: String::new(),
        dir: "external/example".to_owned(),
    }]);
    graph.add_meta_rule(MetaTargetRule {
        name: "consumer".to_owned(),
        dependencies: vec!["example-fetch".to_owned()],
    });
    assert!(graph
        .resolve_source_inventory_fetches(&["${AROS_PORTS_DIR}/example-1.0/src/*.c".to_owned()])
        .is_empty());

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("aros_fetch_archive(NAME \"example-fetch\""));
    assert_eq!(graph.source_inventory_fetches, ["example-fetch"]);
    assert!(cmake.contains("aros_add_target_dependency(\"consumer\" \"${dep}\")"));
    assert!(!cmake.contains("add_custom_target(\"example-fetch\")"));
}

#[test]
fn recursive_directory_copy_emits_concrete_target_and_fetch_dependency() {
    let mut graph = DependencyGraph::new();
    graph.add_fetches(vec![FetchDecl {
        name: "compiler-boost-fetch".to_owned(),
        archive: "boost_1_89_0".to_owned(),
        suffixes: "tar.gz".to_owned(),
        origins: "https://example.invalid/boost.tar.gz".to_owned(),
        location: "${AROS_PORTS_SOURCE_DIR}".to_owned(),
        destination: "${AROS_PORTS_DIR}/boost".to_owned(),
        base: String::new(),
        patch_origins: String::new(),
        patches: String::new(),
        dir: "compiler/boost".to_owned(),
    }]);
    assert!(graph
        .add_copy_directories(vec![CopyDirectoryDecl {
            name: "compiler-boost-geninc-copy".to_owned(),
            source: "${AROS_PORTS_DIR}/boost/boost_1_89_0/boost".to_owned(),
            destination: "${AROS_GENINC_DIR}/boost".to_owned(),
            file: "compiler/boost/mmakefile.src".to_owned(),
            line: 27,
            dependencies: Vec::new(),
        }])
        .is_empty());
    assert!(graph.resolve_copy_directories().is_empty());
    graph.add_meta_rule(MetaTargetRule {
        name: "ports-includes".to_owned(),
        dependencies: vec!["compiler-boost-geninc-copy".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains(
            "aros_copy_dir_recursive(\n    NAME \"compiler-boost-geninc-copy\"\n    SOURCE \"${AROS_PORTS_DIR}/boost/boost_1_89_0/boost\"\n    DESTINATION \"${AROS_GENINC_DIR}/boost\"\n    DEPENDS \"compiler-boost-fetch\"\n)"
        ));
    assert!(cmake.contains("aros_add_target_dependency(\"ports-includes\" \"${dep}\")"));
    assert!(!cmake.contains("add_custom_target(\"compiler-boost-geninc-copy\")"));
}

#[test]
fn all_meta_targets_are_declared_before_any_edges_are_attached() {
    let mut graph = DependencyGraph::new();
    graph.add_icons(vec![icon("leaf")], Vec::new());
    graph.add_meta_rule(MetaTargetRule {
        name: "parent".to_owned(),
        dependencies: vec!["child".to_owned()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "child".to_owned(),
        dependencies: vec!["leaf".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    let parent = cmake.find("add_custom_target(\"parent\")").unwrap();
    let child = cmake.find("add_custom_target(\"child\")").unwrap();
    let first_edge = cmake.find("foreach(dep IN ITEMS").unwrap();
    assert!(parent < first_edge && child < first_edge, "{cmake}");
}

#[test]
fn a_dynamic_selected_iconset_dependency_is_not_filtered_out() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "workbench".to_owned(),
        dependencies: vec!["iconset-${AROS_TARGET_ICONSET}-wbench-icons".to_owned()],
    });
    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("iconset-${AROS_TARGET_ICONSET}-wbench-icons"));
}

#[test]
fn an_orphan_meta_target_remains_nameable_from_its_parent() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "parent".to_owned(),
        dependencies: vec!["orphan".to_owned()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "orphan".to_owned(),
        dependencies: vec!["missing".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert!(cmake.contains("add_custom_target(\"parent\")"));
    assert!(cmake.contains("add_custom_target(\"orphan\")"));
    assert!(cmake.contains("foreach(dep IN ITEMS \"orphan\")"));
    assert!(!cmake.contains("foreach(dep IN ITEMS \"missing\")"));
}

#[test]
fn the_reserved_clean_target_is_not_redeclared() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "clean".to_owned(),
        dependencies: vec!["missing".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert!(!cmake.contains("add_custom_target(\"clean\")"));
}

#[test]
fn the_reserved_install_target_is_not_redeclared() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "install".to_owned(),
        dependencies: vec!["leaf".to_owned()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "leaf".to_owned(),
        dependencies: Vec::new(),
    });

    let cmake = generate_cmake(&graph);
    assert!(!cmake.contains("add_custom_target(\"install\")"));
    assert!(cmake.contains("add_custom_target(\"leaf\")"));
}

#[test]
fn module_output_metadata_is_emitted_for_the_cmake_builders() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut graph = DependencyGraph::new();
    for relative in [
        "developer/debug/test/library/mmakefile.src",
        "workbench/tools/SysExplorer/mmakefile.src",
        "rom/usb/classes/serialpl2303/mmakefile.src",
    ] {
        let parsed = parse_mmakefile_with_dirs(&root.join(relative), &root, &dirs).unwrap();
        assert!(
            parsed.skipped_programs.is_empty(),
            "{relative}: {:#?}",
            parsed.skipped_programs
        );
        for target in parsed.targets {
            graph.add_target(target);
        }
    }

    let cmake = generate_cmake(&graph);
    assert!(
        cmake.contains(
            "    INSTALL_DIR \"${AROS_BUILD_DIR}/SYS/Developer/Debug/Tests/Library/Libs\""
        ),
        "{cmake}"
    );
    assert!(
        cmake.contains("    INSTALL_DIR \"Tools/SysExpModules\""),
        "{cmake}"
    );
    assert!(cmake.contains("    MODTYPE \"usbclass\""), "{cmake}");
    assert!(cmake.contains("    MODSUFFIX \"class\""), "{cmake}");
    assert!(cmake.contains("    MODSUFFIX \"sysexp\""), "{cmake}");
}

#[test]
fn private_linklib_output_and_search_path_are_emitted_verbatim() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &crate::TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        },
    )
    .unwrap();

    let mut provider = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "linklibs-z-static")
        .expect("ordinary linklib")
        .clone();
    provider.mmake_name = "private-gallium-provider".to_owned();
    provider.target_name = "gallium_i915".to_owned();
    provider.canonical_linklib_output = false;
    provider.canonical_linklib_eligible = false;
    provider.linklib_output_dir = Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8".to_owned());

    let mut consumer = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "workbench-libs-z-minigzip")
        .expect("sourceful consumer")
        .clone();
    consumer.mmake_name = "private-gallium-consumer".to_owned();
    consumer.link_options = vec![
        "-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8".to_owned(),
        "-lgallium_i915".to_owned(),
    ];

    let mut graph = DependencyGraph::new();
    graph.add_target(provider);
    graph.add_target(consumer);
    let cmake = generate_cmake(&graph);

    let provider_at = cmake
        .find("MMAKE_ID private-gallium-provider")
        .expect("private provider declaration");
    let provider_end = cmake[provider_at..].find("\n)\n").unwrap() + provider_at;
    let provider_decl = &cmake[provider_at..provider_end];
    assert!(provider_decl.contains("    OUTPUT_DIR \"${AROS_BUILD_DIR}/gen/lib/mesa20.0.8\""));
    assert!(!provider_decl.contains("CANONICAL_OUTPUT"));

    let consumer_at = cmake
        .find("MMAKE_ID private-gallium-consumer")
        .expect("private consumer declaration");
    let consumer_end = cmake[consumer_at..].find("\n)\n").unwrap() + consumer_at;
    assert!(cmake[consumer_at..consumer_end].contains(
        "    LINK_OPTIONS \"-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8\" \"-lgallium_i915\""
    ));
}

#[test]
fn zlib_emits_positional_flags_outputs_provider_and_header_transform() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = crate::parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &crate::TargetContext {
            cpu: Some("aarch64".to_owned()),
            platform: Some("raspi".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some(String::new()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        },
    )
    .unwrap();
    let mut raw_z_consumer = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "workbench-libs-z-minigzip")
        .expect("minigzip declaration")
        .clone();
    raw_z_consumer.mmake_name = "raw-z-consumer".to_owned();
    raw_z_consumer.target_name = "raw-z-consumer".to_owned();
    raw_z_consumer.use_libs.clear();
    raw_z_consumer.link_libs.clear();
    raw_z_consumer.link_options = vec!["-lz".to_owned()];
    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_target(raw_z_consumer);
    for relative in [
        "compiler/crt/posixc/mmakefile.src",
        "compiler/crt/stdc/mmakefile.src",
        "compiler/pthread/mmakefile.src",
    ] {
        let provider = crate::parse_mmakefile_with_dirs_and_context(
            &root.join(relative),
            &root,
            &dirs,
            &crate::TargetContext {
                cpu: Some("aarch64".to_owned()),
                platform: Some("raspi".to_owned()),
                family: Some(String::new()),
                variant: Some(String::new()),
                toolchain: Some("llvm".to_owned()),
                cpu32: Some(String::new()),
                use_mmu: Some("1".to_owned()),
                float_abi: Some(String::new()),
            },
        )
        .unwrap();
        for mut target in provider.targets.into_iter().filter(|target| {
            matches!(
                target.mmake_name.as_str(),
                "compiler-posixc" | "compiler-stdc" | "linklibs-pthread"
            )
        }) {
            if target.mmake_name != "linklibs-pthread" {
                target.use_libs.clear();
            }
            graph.add_target(target);
        }
    }
    for rule in parsed.meta_rules {
        graph.add_meta_rule(rule);
    }
    graph.add_fetches(parsed.fetches);
    graph.add_header_transforms(parsed.header_transforms);
    let unresolved = graph.resolve_use_libs();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert!(graph.resolve_port_source_fetches().is_empty());
    assert!(graph.resolve_header_transforms().is_empty());

    let cmake = generate_cmake(&graph);
    let module_at = cmake
        .find("aros_add_library(\n    TARGET z1")
        .expect("z1 module");
    let module_end = cmake[module_at..].find("\n)\n").unwrap() + module_at;
    let module = &cmake[module_at..module_end];
    assert!(module.contains("    LINKLIB_NAME \"z\""), "{module}");
    assert!(module.contains("    GENMODULE_LINKLIBS"), "{module}");
    assert!(
        module.contains("    LIBS \"compiler-posixc-linklib-rel\" \"compiler-stdc-linklib-rel\"")
    );
    assert!(
        module.contains("    LINK_OPTIONS \"-lpthread\""),
        "{module}"
    );
    assert!(
        module.contains("    COMPILE_OPTIONS \"-march=armv8-a+crc+crypto\""),
        "{module}"
    );
    // Twenty source entries plus the one declaration-local include path.
    assert_eq!(
        module
            .matches("${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1")
            .count(),
        21
    );

    for target in ["z.static", "z-nogzip.static"] {
        let at = cmake
            .find(&format!("aros_add_linklib(\n    TARGET {target}"))
            .unwrap();
        let end = cmake[at..].find("\n)\n").unwrap() + at;
        assert!(cmake[at..end].contains("    CANONICAL_OUTPUT"));
    }
    assert!(cmake.contains("    LIBS \"workbench-libs-z-linklib\""));
    assert!(cmake.contains("    MMAKE_ID compiler-posixc\n    GENMODULE_LINKLIBS"));
    assert!(cmake.contains("    MMAKE_ID compiler-stdc\n    GENMODULE_LINKLIBS"));
    assert!(cmake.contains("MMAKE_ID linklibs-pthread\n    CANONICAL_OUTPUT"));
    assert!(cmake.contains("aros_add_target_dependency(\"workbench-libs-z\" \"${dep}\")"));
    let raw_edge = cmake
        .find("if(TARGET \"raw-z-consumer\")")
        .expect("raw z consumer dependency block");
    let raw_edge_end = cmake[raw_edge..].find("\nendif()\n\n").unwrap() + raw_edge;
    assert!(
        cmake[raw_edge..raw_edge_end].contains("\"workbench-libs-z-linklib\""),
        "{}",
        &cmake[raw_edge..raw_edge_end]
    );
    assert!(cmake.contains("aros_transform_header(\n    NAME \"workbench-libs-z-geninc\""));
    assert!(cmake.contains("    DEPENDS \"zlib-fetch\""));
    assert!(cmake.contains("\"workbench-libs-z-linklib\""));
}

#[test]
fn atheros_define_header_is_a_real_owner_after_both_compile_consumers() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = crate::TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
    };
    let mut graph = DependencyGraph::new();
    for relative in [
        "workbench/devs/networks/atheros5000/hal/mmakefile.src",
        "workbench/devs/networks/atheros5000/mmakefile.src",
    ] {
        let parsed = crate::parse_mmakefile_with_dirs_and_context(
            &root.join(relative),
            &root,
            &dirs,
            &context,
        )
        .unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_define_headers(parsed.define_headers);
    }
    assert!(graph.resolve_use_libs().is_empty());
    assert!(graph.resolve_define_headers().is_empty());

    let cmake = generate_cmake(&graph);
    let device_at = cmake
        .find("MMAKE_ID workbench-devs-networks-atheros5000\n")
        .expect("device declaration");
    let hal_at = cmake
        .find("MMAKE_ID workbench-devs-networks-atheros5000-hal\n")
        .expect("HAL declaration");
    let header_at = cmake
        .find("aros_generate_defines_header(\n")
        .expect("literal define header");
    let finalize_at = cmake
        .find("aros_finalize_link_libraries()\n")
        .expect("deferred link finalizer");
    assert!(device_at < finalize_at && hal_at < finalize_at);
    assert!(finalize_at < header_at);
    assert_eq!(cmake.matches("aros_finalize_link_libraries()").count(), 1);
    assert_eq!(cmake.matches("aros_generate_defines_header(").count(), 1);
    let device_end = cmake[device_at..].find("\n)\n").unwrap() + device_at;
    assert!(cmake[device_at..device_end]
        .contains("    LIBS \"workbench-devs-networks-atheros5000-hal\""));
    let header_end = cmake[header_at..].find("\n)\n").unwrap() + header_at;
    let header = &cmake[header_at..header_end];
    assert!(header.contains("    OWNER \"workbench-devs-networks-atheros5000-hal-opts\""));
    assert!(header.contains(
        "    OUTPUT \"${AROS_BUILD_DIR}/workbench/devs/networks/atheros5000/hal/opt_ah.h\""
    ));
    assert!(header.contains(
            "    DEFINES \"AH_HAS_RF 1\" \"AH_SUPPORT_AR5211 1\" \"AH_SUPPORT_AR5212 1\" \"AH_SUPPORT_AR5416 1\" \"AH_SUPPORT_2316 1\" \"AH_SUPPORT_2317 1\" \"AH_SUPPORT_2133 1\" \"AH_SUPPORT_2413 1\" \"AH_SUPPORT_2417 1\" \"AH_SUPPORT_2425 1\" \"AH_SUPPORT_5111 1\" \"AH_SUPPORT_5112 1\" \"AH_SUPPORT_5413 1\" \"AH_ENABLE_FORCEBIAS 1\""
        ));
    assert!(header.contains(
            "    DEPENDS \"${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/Makefile.inc\" \"${CMAKE_SOURCE_DIR}/workbench/devs/networks/atheros5000/hal/mmakefile.src\""
        ));
    assert!(header.contains(
            "    CONSUMERS \"workbench-devs-networks-atheros5000\" \"workbench-devs-networks-atheros5000-hal\""
        ));
    assert!(!cmake.contains("add_custom_target(\"workbench-devs-networks-atheros5000-hal-opts\")"));
}

#[test]
fn sourceful_and_sourceless_full_modules_keep_their_exact_cmake_contracts() {
    let root = root();
    let dirs = DirVars::load(&root);
    let target = crate::TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
    };
    let zstd = crate::parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/zstd/mmakefile.src"),
        &root,
        &dirs,
        &target,
    )
    .unwrap();
    let version = crate::parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/version/mmakefile.src"),
        &root,
        &dirs,
        &target,
    )
    .unwrap();

    let mut graph = DependencyGraph::new();
    for target in zstd.targets.into_iter().chain(version.targets) {
        graph.add_target(target);
    }
    for rule in zstd.meta_rules.into_iter().chain(version.meta_rules) {
        graph.add_meta_rule(rule);
    }
    graph.add_fetches(zstd.fetches);
    graph.add_copy_includes(zstd.copy_includes);
    assert!(graph.resolve_port_source_fetches().is_empty());
    for mmake in ["linklibs-zstd", "workbench-libs-zstd-library"] {
        assert!(
            graph.meta_targets[mmake].contains("workbench-libs-zstd-fetch"),
            "{mmake}: {:#?}",
            graph.meta_targets[mmake]
        );
    }

    let cmake = generate_cmake(&graph);
    let static_at = cmake
        .find("aros_add_linklib(\n    TARGET zstd-static\n    MMAKE_ID linklibs-zstd")
        .expect("real zstd static target");
    let static_end = cmake[static_at..].find("\n)\n").unwrap() + static_at;
    let static_decl = &cmake[static_at..static_end];
    assert!(static_decl.contains("    CANONICAL_OUTPUT"));
    assert!(static_decl.contains("    DEFINES \"ZSTD_NO_TRACE\""));
    assert_eq!(static_decl.matches("/zstd/zstd-1.5.7/").count(), 30);

    let module_at = cmake
        .find("aros_add_library(\n    TARGET zstd\n    MMAKE_ID workbench-libs-zstd-library")
        .expect("sourceful zstd module");
    let module_end = cmake[module_at..].find("\n)\n").unwrap() + module_at;
    let module = &cmake[module_at..module_end];
    assert!(
        static_at < module_at,
        "the real colliding target must exist first"
    );
    assert!(module.contains("    LINKLIB_NAME \"zstd\""));
    assert!(module.contains("    GENMODULE_LINKLIBS"));
    assert!(module.contains("    DEFINES \"ZSTD_NO_TRACE\""));
    assert_eq!(module.matches("/zstd/zstd-1.5.7/").count(), 30);

    let version_at = cmake
        .find("aros_add_library(\n    TARGET version\n    MMAKE_ID workbench-libs-version")
        .expect("source-free version module");
    let version_end = cmake[version_at..].find("\n)\n").unwrap() + version_at;
    assert!(cmake[version_at..version_end].contains("    GENMODULE_ONLY"));
}

#[test]
fn abi_and_genmodule_only_targets_use_their_dedicated_cmake_contracts() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut graph = DependencyGraph::new();
    for relative in [
        "rom/bluetooth/classes/mmakefile.src",
        "workbench/libs/dxtn/mmakefile.src",
        "workbench/libs/version/mmakefile.src",
    ] {
        let parsed = parse_mmakefile_with_dirs(&root.join(relative), &root, &dirs).unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
    }

    let cmake = generate_cmake(&graph);
    let abi_start = cmake
        .find("aros_add_module_abi(\n    TARGET btclass")
        .expect("ABI CMake call");
    let abi_end = cmake[abi_start..].find("\n)\n").unwrap() + abi_start;
    let abi = &cmake[abi_start..abi_end];
    assert!(abi.contains("    MMAKE_ID kernel-bluetooth-btclass"));
    assert!(abi.contains("    MODTYPE \"library\""));
    assert!(abi.contains("    DIRECTORY \"${CMAKE_SOURCE_DIR}/rom/bluetooth/classes\""));
    assert!(!abi.contains("SOURCES"), "{abi}");
    assert!(!abi.contains("GENMODULE_ONLY"), "{abi}");
    assert!(!abi.contains("CONFFILE"), "{abi}");

    let version_start = cmake
        .find("aros_add_library(\n    TARGET version")
        .expect("version CMake call");
    let version_end = cmake[version_start..].find("\n)\n").unwrap() + version_start;
    let version = &cmake[version_start..version_end];
    assert!(version.contains("    MMAKE_ID workbench-libs-version"));
    assert!(version.contains("    GENMODULE_ONLY"));
    assert!(!version.contains("SOURCES"), "{version}");

    let dxtn_meta_start = cmake
        .find("if(TARGET \"workbench-libs-dxtn-linklib\")")
        .expect("the ABI linklib keeps its architecture-selected meta edge");
    let dxtn_meta_end = cmake[dxtn_meta_start..].find("\nendif()\n\n").unwrap() + dxtn_meta_start;
    let dxtn_meta = &cmake[dxtn_meta_start..dxtn_meta_end];
    assert!(dxtn_meta.contains("workbench-libs-dxtn-${AROS_TARGET_CPU}-linklib"));
    assert!(
        !dxtn_meta.contains("workbench-libs-dxtn-includes"),
        "{dxtn_meta}"
    );
    assert!(cmake.contains("if(TARGET \"workbench-libs-dxtn-includes\")"));
}

#[test]
fn package_member_names_align_with_modules_but_do_not_reach_kickstart() {
    let mut graph = DependencyGraph::new();
    let members = vec![
        ResolvedPackageMember {
            target: "kernel-fs-ram".to_owned(),
            runtime_name: "ram-handler".to_owned(),
        },
        ResolvedPackageMember {
            target: "kernel-log-serial".to_owned(),
            runtime_name: "serial.logger".to_owned(),
        },
    ];
    graph.add_packages(vec![
        PackageDecl {
            file: "rom/mmakefile.src".to_owned(),
            mmake: "package".to_owned(),
            output: "${AROS_BOOT_DIR}/package.pkg".to_owned(),
            members: Vec::new(),
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: members.clone(),
            arch: String::new(),
        },
        PackageDecl {
            file: "arch/test/boot/mmakefile.src".to_owned(),
            mmake: "kickstart".to_owned(),
            output: "${AROS_BOOT_ARCH_DIR}/kernel".to_owned(),
            members: Vec::new(),
            startup: Some("kernel".to_owned()),
            uselibs: Vec::new(),
            is_kickstart: true,
            resolved: members,
            arch: "test".to_owned(),
        },
    ]);

    let cmake = generate_cmake(&graph);
    let package_start = cmake.find("aros_make_package(").unwrap();
    let kickstart_start = cmake.find("aros_link_kickstart(").unwrap();
    let package = &cmake[package_start..kickstart_start];
    let kickstart = &cmake[kickstart_start..];
    assert!(
        package.contains("    MODULES \"kernel-fs-ram\" \"kernel-log-serial\""),
        "{package}"
    );
    assert!(
        package.contains("    MEMBER_NAMES \"ram-handler\" \"serial.logger\""),
        "{package}"
    );
    assert!(kickstart.contains("    MODULES \"kernel-fs-ram\" \"kernel-log-serial\""));
    assert!(!kickstart.contains("MEMBER_NAMES"), "{kickstart}");
}

#[test]
fn the_banner_warns_names_its_version_and_states_the_target() {
    let header = generated_header(Some(&crate::TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: None,
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: None,
    }));

    assert!(header.contains("GENERATED FILE - DO NOT EDIT"), "{header}");
    // Says the edit is lost, not merely that the file is generated.
    assert!(header.contains("lost at the next"), "{header}");
    // Points at the source of truth and at the omission reports.
    assert!(header.contains("mmakefile.src"), "{header}");
    assert!(header.contains("generated_targets.*.txt"), "{header}");
    // The version comes from the crate, so it cannot drift from Cargo.toml.
    assert!(
        header.contains(&format!("aros-transpiler {}", env!("CARGO_PKG_VERSION"))),
        "{header}"
    );
    // The target-selecting arguments, an empty one shown as such.
    assert!(
        header.contains(&format!("{:<13}x86_64", "--cpu")),
        "{header}"
    );
    assert!(
        header.contains(&format!("{:<13}\"\"", "--family")),
        "{header}"
    );
    // Arguments that were not given are not invented.
    assert!(!header.contains("--variant"), "{header}");
    assert!(!header.contains("--float-abi"), "{header}");
    // Absolute host paths would tie the file to one checkout, so
    // --source-dir and --output appear only as prose saying they are left
    // out, never with a value.
    assert!(header.contains("are omitted here"), "{header}");
    assert!(!header.contains("--source-dir /"), "{header}");
    assert!(!header.contains("--output /"), "{header}");
    // No timestamp: the file is rewritten on every configure.
    assert!(!header.to_lowercase().contains("generated on"), "{header}");

    let bare = generated_header(None);
    assert!(bare.contains("no target selected"), "{bare}");
    assert!(bare.contains("DO NOT EDIT"), "{bare}");
}
