use super::{arch_compatible, define_header_compile_targets, target_runtime_name, DependencyGraph};
use crate::ast::{CopyDirectoryDecl, DefineHeaderDecl, MetaTargetRule, ModuleType};
use crate::copy_includes::CopyIncludesDecl;
use crate::dirs::DirVars;
use crate::fetch::FetchDecl;
use crate::packages::{PackageDecl, ResolvedPackageMember};
use crate::{parse_mmakefile_with_dirs, parse_mmakefile_with_dirs_and_context, TargetContext};
use std::collections::HashSet;
use std::path::Path;
use walkdir::WalkDir;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

#[test]
fn identical_header_copies_keep_each_distinct_mmake_owner() {
    let first = CopyIncludesDecl {
        name: "first-includes".to_owned(),
        dest: "GL".to_owned(),
        source_dir: "${AROS_PORTS_DIR}/example/include/GL".to_owned(),
        patterns: vec!["gl.h".to_owned()],
        excludes: Vec::new(),
        flatten: true,
    };
    let mut second = first.clone();
    second.name = "second-includes".to_owned();

    let mut graph = DependencyGraph::new();
    graph.add_copy_includes(vec![first.clone(), first, second]);

    assert_eq!(graph.copy_includes.len(), 2);
    assert_eq!(graph.copy_includes[0].name, "first-includes");
    assert_eq!(graph.copy_includes[1].name, "second-includes");
}

#[test]
fn port_directory_copy_binds_its_unique_fetch_owner() {
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
    assert_eq!(graph.copy_directories.len(), 1);
    assert_eq!(
        graph.copy_directories[0].dependencies,
        ["compiler-boost-fetch"]
    );
}

#[test]
fn catalog_source_consumers_follow_resolved_sibling_sources() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = TargetContext {
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
        "workbench/libs/muimaster/mmakefile.src",
        "workbench/libs/muimaster/classes/mmakefile.src",
        "workbench/libs/muimaster/catalogs/mmakefile.src",
    ] {
        let parsed =
            parse_mmakefile_with_dirs_and_context(&root.join(relative), &root, &dirs, &context)
                .unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_catalogs(parsed.catalogs);
    }

    graph.resolve_catalog_consumers();

    let catalog = graph
        .catalogs
        .iter()
        .find(|catalog| catalog.mmake == "workbench-libs-muimaster-catalogs")
        .expect("muimaster catalog declaration");
    assert_eq!(
        catalog.consumers,
        [
            "workbench-classes-zune-aboutmui",
            "workbench-classes-zune-coloradjust",
            "workbench-classes-zune-dirlist",
            "workbench-classes-zune-frameadjust",
            "workbench-classes-zune-imageadjust",
            "workbench-classes-zune-palette",
            "workbench-classes-zune-penadjust",
            "workbench-classes-zune-popframe",
            "workbench-classes-zune-poppen",
            "workbench-classes-zune-volumelist",
            "workbench-libs-muimaster",
        ]
    );
}

#[test]
fn catalog_program_group_consumers_name_only_matching_members() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut programs =
        parse_mmakefile_with_dirs(&root.join("tools/dtdesc/mmakefile.src"), &root, &dirs)
            .unwrap()
            .targets
            .into_iter()
            .find(|target| target.mmake_name == "tools-dtdesc")
            .expect("dtdesc program group");
    assert_eq!(programs.module_type, ModuleType::ProgramGroup);
    programs.dir_path = "workbench/demo/classes".into();
    programs.source_files = vec!["../locale".to_owned(), "unrelated".to_owned()];
    programs.cxx_source_files.clear();
    programs.objc_source_files.clear();
    programs.asm_source_files.clear();

    let mut catalog = parse_mmakefile_with_dirs(
        &root.join("workbench/libs/muimaster/catalogs/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap()
    .catalogs
    .into_iter()
    .next()
    .expect("catalog declaration");
    catalog.mmake = "demo-catalogs".to_owned();
    catalog.declaring_dir = "workbench/demo/catalogs".to_owned();
    catalog.source = Some("../strings.h".to_owned());

    let mut graph = DependencyGraph::new();
    graph.add_target(programs);
    graph.add_catalogs(vec![catalog]);
    graph.resolve_catalog_consumers();

    assert_eq!(
        graph.catalogs[0].consumers,
        ["tools-dtdesc-locale"],
        "the aggregate program-group id is not a compile target"
    );
}

fn package_graph(target_file: &str, package_file: &str, kind: &str, name: &str) -> DependencyGraph {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs(&root.join(target_file), &root, &dirs).unwrap();
    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_packages(vec![PackageDecl {
        file: package_file.to_owned(),
        mmake: "test-package".to_owned(),
        output: "${AROS_BOOT_ARCH_DIR}/test.pkg".to_owned(),
        members: vec![(kind.to_owned(), name.to_owned())],
        startup: None,
        uselibs: Vec::new(),
        is_kickstart: false,
        resolved: Vec::new(),
        arch: String::new(),
    }]);
    graph
}

#[test]
fn an_abi_skeleton_is_a_linklib_provider_but_never_a_runtime_member() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs(
        &root.join("rom/bluetooth/classes/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap();
    let abi = parsed
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "kernel-bluetooth-btclass")
        .expect("btclass ABI target");
    assert_eq!(target_runtime_name(&abi), None);

    let mut consumer = parse_mmakefile_with_dirs(
        &root.join("workbench/libs/version/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap()
    .targets
    .into_iter()
    .find(|target| target.mmake_name == "workbench-libs-version")
    .expect("version target");
    consumer.mmake_name = "test-abi-consumer".to_owned();
    consumer.use_libs = vec!["btclass".to_owned()];

    let mut graph = DependencyGraph::new();
    graph.add_target(abi);
    graph.add_target(consumer);
    let unresolved = graph.resolve_use_libs();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.targets["test-abi-consumer"].link_libs,
        ["kernel-bluetooth-btclass-linklib"]
    );

    graph.add_packages(vec![PackageDecl {
        file: "test/mmakefile.src".to_owned(),
        mmake: "test-package".to_owned(),
        output: "${AROS_BOOT_ARCH_DIR}/test.pkg".to_owned(),
        members: vec![("library".to_owned(), "btclass".to_owned())],
        startup: None,
        uselibs: Vec::new(),
        is_kickstart: false,
        resolved: Vec::new(),
        arch: String::new(),
    }]);
    let unresolved = graph.resolve_packages();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(unresolved[0].contains("(btclass.library) has no target"));
    assert!(graph.packages[0].resolved.is_empty());
}

#[test]
fn a_library_provides_its_module_and_explicit_linklib_names() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &TargetContext {
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

    let mut alias_consumer = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "workbench-libs-z-minigzip")
        .expect("minigzip")
        .clone();
    alias_consumer.mmake_name = "z-alias-consumer".to_owned();
    alias_consumer.use_libs = vec!["z".to_owned()];
    alias_consumer.link_libs.clear();

    let mut external_flag_consumer = alias_consumer.clone();
    external_flag_consumer.mmake_name = "raw-external-consumer".to_owned();
    external_flag_consumer.use_libs.clear();
    external_flag_consumer.link_options = vec!["-lprivate-port-runtime".to_owned()];

    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_target(alias_consumer);
    graph.add_target(external_flag_consumer);
    for relative in [
        "compiler/crt/posixc/mmakefile.src",
        "compiler/crt/stdc/mmakefile.src",
        "compiler/pthread/mmakefile.src",
    ] {
        let provider = parse_mmakefile_with_dirs_and_context(
            &root.join(relative),
            &root,
            &dirs,
            &TargetContext {
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
    let unresolved = graph.resolve_use_libs();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(
        unresolved[0].contains(
            "raw-external-consumer link option -lprivate-port-runtime has no link library"
        ),
        "{unresolved:#?}"
    );
    assert_eq!(
        graph.targets["workbench-libs-z-minigzip"].link_libs,
        ["workbench-libs-z-linklib"]
    );
    assert_eq!(
        graph.targets["z-alias-consumer"].link_libs,
        ["workbench-libs-z-linklib"]
    );
    assert!(graph.targets["raw-external-consumer"].link_libs.is_empty());
    assert!(graph.targets["raw-external-consumer"]
        .link_options
        .is_empty());
    assert!(graph
        .meta_targets
        .get("raw-external-consumer")
        .is_none_or(HashSet::is_empty));
    assert_eq!(
        graph.targets["workbench-libs-z"].link_libs,
        ["compiler-posixc-linklib-rel", "compiler-stdc-linklib-rel"]
    );
    for consumer in [
        "workbench-libs-z",
        "workbench-libs-z-minigzip",
        "z-alias-consumer",
    ] {
        assert!(graph.meta_targets[consumer].contains("linklibs-pthread"));
        assert_eq!(graph.targets[consumer].link_options, ["-lpthread"]);
    }
    assert!(
        graph.targets["compiler-posixc"]
            .genmodule_linklibs
            .as_ref()
            .unwrap()
            .enabled
    );
    assert!(
        graph.targets["compiler-stdc"]
            .genmodule_linklibs
            .as_ref()
            .unwrap()
            .enabled
    );
    assert!(graph.targets["linklibs-pthread"].canonical_linklib_output);
}

#[test]
fn the_default_link_set_binds_archives_and_promotes_canonical_names() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some("x86_64".to_owned()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some("hard".to_owned()),
    };
    let mut graph = DependencyGraph::new();
    for relative in ["compiler/alib/mmakefile.src", "rom/dos/mmakefile.src"] {
        let parsed =
            parse_mmakefile_with_dirs_and_context(&root.join(relative), &root, &dirs, &context)
                .unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
    }

    let set = crate::default_link_set::DefaultLinkSet {
        items: vec![
            crate::default_link_set::DefaultLinkItem {
                name: "amiga".to_owned(),
                require_absent: Vec::new(),
                require_present: Vec::new(),
            },
            crate::default_link_set::DefaultLinkItem {
                name: "dos".to_owned(),
                require_absent: Vec::new(),
                require_present: Vec::new(),
            },
            crate::default_link_set::DefaultLinkItem {
                name: "nothing-builds-this".to_owned(),
                require_absent: vec!["nosysbase".to_owned()],
                require_present: Vec::new(),
            },
        ],
    };
    let unresolved = graph.resolve_default_link_set(&set);

    // %build_linklib libname=amiga must publish libamiga.a. Nothing in the
    // mmakefile tree links -lamiga, so only the spec makes it canonical.
    assert!(
        graph.targets["linklibs-amiga"].canonical_linklib_output,
        "the spec is linklibs-amiga's only consumer"
    );
    let bound: Vec<(&str, &str)> = graph
        .default_link_set
        .iter()
        .map(|item| (item.name.as_str(), item.archive.as_str()))
        .collect();
    assert_eq!(
        bound,
        [("amiga", "linklibs-amiga"), ("dos", "kernel-dos-linklib")],
        "a module publishes its client archive as a separate target"
    );
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert!(unresolved[0].contains("libnothing-builds-this.a"));
}

#[test]
fn private_linklib_requires_the_exact_consumer_search_directory() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &TargetContext {
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
    provider.use_libs.clear();
    provider.link_libs.clear();
    provider.link_options.clear();
    let mut other_provider = provider.clone();
    other_provider.mmake_name = "other-private-gallium-provider".to_owned();
    other_provider.linklib_output_dir = Some("${AROS_BUILD_DIR}/gen/lib/other-mesa".to_owned());

    let mut matching = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "workbench-libs-z-minigzip")
        .expect("sourceful consumer")
        .clone();
    matching.mmake_name = "matching-private-consumer".to_owned();
    matching.use_libs.clear();
    matching.link_libs.clear();
    matching.link_options = vec![
        "-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8".to_owned(),
        "-lgallium_i915".to_owned(),
    ];

    let mut mismatched = matching.clone();
    mismatched.mmake_name = "mismatched-private-consumer".to_owned();
    mismatched.link_options = vec![
        "-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8/subdir".to_owned(),
        "-lgallium_i915".to_owned(),
    ];

    let mut graph = DependencyGraph::new();
    graph.add_target(provider);
    graph.add_target(other_provider);
    graph.add_target(matching);
    graph.add_target(mismatched);

    let unresolved = graph.resolve_use_libs();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(
            unresolved[0].contains(
                "mismatched-private-consumer link option -lgallium_i915 has no public or matching private archive"
            ),
            "{unresolved:#?}"
        );
    assert_eq!(
        graph.targets["matching-private-consumer"].link_options,
        ["-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8", "-lgallium_i915"]
    );
    assert_eq!(
        graph.targets["mismatched-private-consumer"].link_options,
        ["-L${AROS_BUILD_DIR}/gen/lib/mesa20.0.8/subdir"]
    );
    assert!(graph.meta_targets["matching-private-consumer"].contains("private-gallium-provider"));
    assert!(
        !graph.meta_targets["matching-private-consumer"].contains("other-private-gallium-provider")
    );
    assert!(graph
        .meta_targets
        .get("mismatched-private-consumer")
        .is_none_or(HashSet::is_empty));
    assert!(!graph.targets["private-gallium-provider"].canonical_linklib_output);
    assert!(!graph.targets["other-private-gallium-provider"].canonical_linklib_output);
}

#[test]
fn zlib_sources_and_transformed_header_have_direct_fetch_edges() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &TargetContext {
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

    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_fetches(parsed.fetches);
    graph.add_copy_includes(parsed.copy_includes);
    graph.add_header_transforms(parsed.header_transforms);
    graph.resolve_header_inventory_fetches(None);
    assert_eq!(graph.source_inventory_fetches, ["zlib-fetch"]);

    let materialized = tempfile::tempdir().unwrap();
    let relative_source = graph.copy_includes[0]
        .source_dir
        .strip_prefix("${AROS_PORTS_DIR}/")
        .unwrap();
    std::fs::create_dir_all(materialized.path().join(relative_source)).unwrap();
    graph.source_inventory_fetches.clear();
    graph.resolve_header_inventory_fetches(Some(materialized.path()));
    assert!(graph.source_inventory_fetches.is_empty());
    assert!(graph.resolve_port_source_fetches().is_empty());
    for target in [
        "workbench-libs-z",
        "linklibs-z-static",
        "linklibs-z-nogzip-static",
        "workbench-libs-z-minigzip",
    ] {
        assert!(
            graph.meta_targets[target].contains("zlib-fetch"),
            "{target}"
        );
    }

    assert!(graph.resolve_header_transforms().is_empty());
    assert_eq!(graph.header_transforms.len(), 1);
    let transform = &graph.header_transforms[0];
    assert_eq!(transform.dependencies, ["zlib-fetch"]);
    assert_eq!(
        transform.consumers,
        [
            "linklibs-z-nogzip-static",
            "linklibs-z-static",
            "workbench-libs-z",
            "workbench-libs-z-linklib",
            "workbench-libs-z-minigzip",
        ]
    );
}

#[test]
fn atheros_hal_header_has_direct_provider_and_device_edges() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = TargetContext {
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
        let parsed =
            parse_mmakefile_with_dirs_and_context(&root.join(relative), &root, &dirs, &context)
                .unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_define_headers(parsed.define_headers);
    }

    let unresolved = graph.resolve_use_libs();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.targets["workbench-devs-networks-atheros5000"].link_libs,
        ["workbench-devs-networks-atheros5000-hal"]
    );
    let unresolved = graph.resolve_define_headers();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(graph.define_headers.len(), 1);
    assert_eq!(
        graph.define_headers[0].consumers,
        [
            "workbench-devs-networks-atheros5000",
            "workbench-devs-networks-atheros5000-hal",
        ]
    );
    for consumer in [
        "workbench-devs-networks-atheros5000",
        "workbench-devs-networks-atheros5000-hal",
    ] {
        assert!(
            graph.meta_targets[consumer].contains("workbench-devs-networks-atheros5000-hal-opts")
        );
    }
}

#[test]
fn define_header_program_group_consumers_expand_to_compile_members() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
    };
    let hal = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/devs/networks/atheros5000/hal/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();
    let mut programs = parse_mmakefile_with_dirs_and_context(
        &root.join("tools/dtdesc/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap()
    .targets
    .into_iter()
    .find(|target| target.mmake_name == "tools-dtdesc")
    .expect("dtdesc program group");
    assert_eq!(programs.module_type, ModuleType::ProgramGroup);
    programs.link_libs = vec!["workbench-devs-networks-atheros5000-hal".to_owned()];

    let mut graph = DependencyGraph::new();
    for target in hal.targets {
        graph.add_target(target);
    }
    graph.add_define_headers(hal.define_headers);
    graph.add_target(programs);

    let unresolved = graph.resolve_define_headers();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.define_headers[0].consumers,
        [
            "tools-dtdesc-createdtdesc",
            "tools-dtdesc-examinedtdesc",
            "workbench-devs-networks-atheros5000-hal",
        ]
    );
    assert!(!graph.meta_targets.contains_key("tools-dtdesc"));
    for member in ["tools-dtdesc-createdtdesc", "tools-dtdesc-examinedtdesc"] {
        assert!(graph.meta_targets[member].contains("workbench-devs-networks-atheros5000-hal-opts"));
    }
}

#[test]
fn genmodule_only_library_mmake_id_is_still_a_compile_target() {
    let root = root();
    let dirs = DirVars::load(&root);
    let target = parse_mmakefile_with_dirs(
        &root.join("workbench/libs/version/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap()
    .targets
    .into_iter()
    .find(|target| target.mmake_name == "workbench-libs-version")
    .expect("version genmodule-only library");
    assert_eq!(target.module_type, ModuleType::Library);
    assert!(target.genmodule_only);
    assert_eq!(
        define_header_compile_targets(&target.mmake_name, &target),
        ["workbench-libs-version"]
    );
}

#[test]
fn define_header_without_a_concrete_provider_stays_unresolved() {
    let mut graph = DependencyGraph::new();
    graph.add_define_headers(vec![DefineHeaderDecl {
        owner: "example-options".to_owned(),
        file: "example/options.mk".to_owned(),
        line: 7,
        output: "${AROS_BUILD_DIR}/example/options.h".to_owned(),
        definitions: vec!["EXAMPLE 1".to_owned()],
        dependencies: vec!["${CMAKE_SOURCE_DIR}/example/options.mk".to_owned()],
        provider: "missing-provider".to_owned(),
        consumers: Vec::new(),
    }]);

    let unresolved = graph.resolve_define_headers();
    assert_eq!(
            unresolved,
            ["example/options.mk:7: example-options provider missing-provider has no concrete target"]
        );
    assert!(graph.meta_targets.is_empty());
    assert!(graph.define_headers[0].consumers.is_empty());
}

#[test]
fn a_meta_cycle_becomes_one_shared_external_dependency_closure() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "a".to_owned(),
        dependencies: vec!["b".to_owned(), "x".to_owned()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "b".to_owned(),
        dependencies: vec!["a".to_owned(), "y".to_owned()],
    });

    let reports = graph.flatten_meta_cycles();
    assert_eq!(reports.len(), 1);
    let expected: HashSet<String> = ["x", "y"].into_iter().map(str::to_owned).collect();
    assert_eq!(graph.meta_targets["a"], expected);
    assert_eq!(graph.meta_targets["b"], expected);
}

#[test]
fn an_acyclic_meta_graph_is_unchanged() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "a".to_owned(),
        dependencies: vec!["b".to_owned()],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "b".to_owned(),
        dependencies: vec!["leaf".to_owned()],
    });
    let before = graph.meta_targets.clone();
    assert!(graph.flatten_meta_cycles().is_empty());
    assert_eq!(graph.meta_targets, before);
}

#[test]
fn a_meta_self_loop_is_removed_without_losing_other_dependencies() {
    let mut graph = DependencyGraph::new();
    graph.add_meta_rule(MetaTargetRule {
        name: "test".to_owned(),
        dependencies: vec!["test".to_owned(), "test-leaf".to_owned()],
    });

    let reports = graph.flatten_meta_cycles();
    assert_eq!(reports.len(), 1);
    assert_eq!(
        graph.meta_targets["test"],
        std::iter::once("test-leaf".to_owned()).collect()
    );
}

#[test]
fn port_sources_depend_directly_on_the_longest_fetch_destination_owner() {
    let root = root();
    let dirs = DirVars::load(&root);
    let target = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
    };
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/png/mmakefile.src"),
        &root,
        &dirs,
        &target,
    )
    .unwrap();

    let mut graph = DependencyGraph::new();
    for target in parsed.targets.clone() {
        graph.add_target(target);
    }
    graph.add_fetches(parsed.fetches.clone());
    assert!(graph.resolve_port_source_fetches().is_empty());
    assert!(graph.meta_targets["workbench-libs-png"].contains("libpng-fetch"));
    assert!(graph.meta_targets["linklibs-png-nostdio"].contains("libpng-fetch"));

    let mut consumer = parsed
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "workbench-libs-png")
        .unwrap();
    consumer.mmake_name = "synthetic-consumer".to_owned();
    consumer.source_files = vec![
        "${AROS_PORTS_DIR}/libpng/version/source".to_owned(),
        "${AROS_PORTS_DIR}/ownerless/source".to_owned(),
    ];
    consumer.cxx_source_files.clear();
    consumer.objc_source_files.clear();
    consumer.asm_source_files.clear();
    consumer.include_dirs = vec![
        "${AROS_PORTS_DIR}/libpng/include".to_owned(),
        "${AROS_PORTS_DIR}/include-ownerless".to_owned(),
    ];

    let template = parsed.fetches.into_iter().next().unwrap();
    let mut broad = template.clone();
    broad.name = "libpng-broad-fetch".to_owned();
    broad.destination = "${AROS_PORTS_DIR}/libpng".to_owned();
    let mut narrow = template;
    narrow.name = "libpng-narrow-fetch".to_owned();
    narrow.destination = "${AROS_PORTS_DIR}/libpng/version".to_owned();

    let mut graph = DependencyGraph::new();
    graph.add_target(consumer);
    graph.add_fetches(vec![broad, narrow]);
    assert_eq!(
        graph.resolve_port_source_fetches(),
        [
            "synthetic-consumer|${AROS_PORTS_DIR}/include-ownerless",
            "synthetic-consumer|${AROS_PORTS_DIR}/ownerless/source"
        ]
    );
    assert_eq!(
        graph.meta_targets["synthetic-consumer"],
        ["libpng-broad-fetch", "libpng-narrow-fetch"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

#[test]
fn wider_cpus_accept_their_32_bit_compatible_candidates() {
    for (ctx_cpu, cand_cpu) in [("x86_64", "i386"), ("aarch64", "arm"), ("riscv64", "riscv")] {
        let candidate = (cand_cpu.to_owned(), "pc".to_owned());
        let context = (ctx_cpu.to_owned(), "pc".to_owned());
        assert!(arch_compatible(Some(&candidate), Some(&context)));
        assert!(!arch_compatible(Some(&context), Some(&candidate)));
    }
}

#[test]
fn a_unique_foreign_package_candidate_is_rejected() {
    let mut graph = package_graph(
        "arch/all-linux/hidd/linuxinput/mmakefile.src",
        "arch/x86_64-pc/boot/mmakefile.src",
        "hidd",
        "linuxinput",
    );
    let unresolved = graph.resolve_packages();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(unresolved[0].contains("no target for this architecture"));
    assert!(graph.packages[0].resolved.is_empty());
}

#[test]
fn a_unique_compatible_32_bit_candidate_is_accepted() {
    let mut graph = package_graph(
        "arch/i386-pc/drivers/serial.hidd/mmakefile.src",
        "arch/x86_64-pc/boot/mmakefile.src",
        "hidd",
        "serial",
    );
    let unresolved = graph.resolve_packages();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.packages[0].resolved[0].target,
        "kernel-pc-i386-serial"
    );
}

#[test]
fn one_explicit_foreign_package_candidate_is_accepted() {
    let mut graph = package_graph(
        "arch/all-linux/hidd/linuxinput/mmakefile.src",
        "arch/x86_64-pc/boot/mmakefile.src",
        "hidd",
        "linuxinput",
    );
    graph.add_meta_rule(MetaTargetRule {
        name: "test-package".to_owned(),
        dependencies: vec!["kernel-hidd-linuxinput-kobj".to_owned()],
    });
    let unresolved = graph.resolve_packages();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.packages[0].resolved[0].target,
        "kernel-hidd-linuxinput"
    );
}

#[test]
fn multiple_explicit_candidates_are_still_architecture_filtered() {
    let mut graph = package_graph(
        "arch/i386-pc/drivers/serial.hidd/mmakefile.src",
        "arch/x86_64-pc/boot/mmakefile.src",
        "hidd",
        "serial",
    );
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs(
        &root.join("arch/m68k-amiga/hidd/serial/mmakefile.src"),
        &root,
        &dirs,
    )
    .unwrap();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.add_meta_rule(MetaTargetRule {
        name: "test-package".to_owned(),
        dependencies: vec![
            "kernel-pc-i386-serial-kobj".to_owned(),
            "amiga-m68k-hidd-serial-kobj".to_owned(),
        ],
    });

    let unresolved = graph.resolve_packages();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
    assert_eq!(
        graph.packages[0].resolved[0].target,
        "kernel-pc-i386-serial"
    );
}

#[test]
fn package_targets_and_runtime_names_stay_aligned() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut graph = DependencyGraph::new();
    for relative in [
        "rom/filesys/ram/mmakefile.src",
        "rom/log/serial/mmakefile.src",
        "rom/usb/classes/bootkeyboard/mmakefile.src",
        "rom/usb/classes/hid/mmakefile.src",
        "workbench/devs/USB/classes/HID/mmakefile.src",
    ] {
        let parsed = parse_mmakefile_with_dirs(&root.join(relative), &root, &dirs).unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
    }
    graph.add_packages(vec![PackageDecl {
        file: "rom/test/mmakefile.src".to_owned(),
        mmake: "test-package".to_owned(),
        output: "${AROS_BOOT_DIR}/test.pkg".to_owned(),
        members: vec![
            ("handler".to_owned(), "ram".to_owned()),
            ("logger".to_owned(), "serial".to_owned()),
            ("class".to_owned(), "USB/bootkeyboard".to_owned()),
            ("class".to_owned(), "USB/hid".to_owned()),
            ("handler".to_owned(), "missing".to_owned()),
            // `$^` removes this duplicate producer. Its whole pair must
            // disappear, not just the target side.
            ("handler".to_owned(), "ram".to_owned()),
        ],
        startup: None,
        uselibs: Vec::new(),
        is_kickstart: false,
        resolved: Vec::new(),
        arch: String::new(),
    }]);

    let unresolved = graph.resolve_packages();
    assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
    assert!(unresolved[0].contains("handler=missing"));
    assert_eq!(
        graph.packages[0].resolved,
        vec![
            ResolvedPackageMember {
                target: "kernel-fs-ram".to_owned(),
                runtime_name: "ram-handler".to_owned(),
            },
            ResolvedPackageMember {
                target: "kernel-log-serial".to_owned(),
                runtime_name: "serial.logger".to_owned(),
            },
            ResolvedPackageMember {
                target: "kernel-usb-classes-bootkeyboard".to_owned(),
                runtime_name: "bootkeyboard.class".to_owned(),
            },
            ResolvedPackageMember {
                target: "kernel-usb-classes-hid".to_owned(),
                runtime_name: "hid.class".to_owned(),
            },
        ]
    );
}

#[test]
fn real_tree_packages_resolve_to_exact_runtime_files() {
    let root = root();
    let dirs = DirVars::load(&root);
    let target = TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
    };
    let skip_dirs = ["build", "target", ".git"];
    let mut files: Vec<_> = WalkDir::new(&root)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || entry.depth() == 0
                || !skip_dirs
                    .iter()
                    .any(|dir| entry.file_name().to_string_lossy() == *dir)
        })
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name() == "mmakefile.src")
        .map(walkdir::DirEntry::into_path)
        .collect();
    files.sort();

    let mut graph = DependencyGraph::new();
    for file in files {
        let parsed = parse_mmakefile_with_dirs_and_context(&file, &root, &dirs, &target).unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_packages(parsed.packages);
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
    }

    let unresolved = graph.resolve_packages();
    assert_eq!(
        unresolved,
        vec![concat!(
            "arch/ppc-chrp/efika/boot/mmakefile.src: ",
            "kernel-package-chrp-ppc-usb class=USB/storage ",
            "(storage.class) has no target"
        )
        .to_owned()]
    );

    let packages: Vec<_> = graph
        .packages
        .iter()
        .filter(|package| !package.is_kickstart)
        .collect();
    let kickstarts: Vec<_> = graph
        .packages
        .iter()
        .filter(|package| package.is_kickstart)
        .collect();
    assert_eq!(packages.len(), 17);
    assert_eq!(kickstarts.len(), 4);
    assert_eq!(
        packages
            .iter()
            .map(|package| package.resolved.len())
            .sum::<usize>(),
        397
    );
    assert_eq!(
        kickstarts
            .iter()
            .map(|package| package.resolved.len())
            .sum::<usize>(),
        19
    );

    for package in &graph.packages {
        for member in &package.resolved {
            let target = &graph.targets[&member.target];
            assert_eq!(
                target_runtime_name(target).as_deref(),
                Some(member.runtime_name.as_str()),
                "{}: {}",
                package.mmake,
                member.target
            );
        }
    }
}
