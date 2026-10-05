//! Explicit local zstd source probe; does not fetch ports or scan the full tree.

use aros_common::{native_build_contract::load_bound_native_build_contract, TargetProfile};
use aros_transpiler::{
    dirs::DirVars, parse_mmakefile_with_context, parse_mmakefile_with_dirs_and_context,
    DependencyGraph, ModuleType, TargetContext,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn selected_source_context() -> (PathBuf, TargetContext) {
    let root =
        PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").expect("select isolated source"));
    let expected =
        std::env::var("AROS_TEST_NATIVE_CONTRACT_SHA256").expect("pin native contract bytes");
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
        ..TargetContext::default()
    };
    (root, target)
}

#[test]
#[ignore = "requires an explicitly selected isolated P4 source and contract digest"]
fn selected_p4_zstd_cold_inventory_and_optional_warm_archive_endpoint() {
    let (root, context) = selected_source_context();
    let recipe = root.join("workbench/libs/zstd/mmakefile.src");
    let cold = parse_mmakefile_with_context(&recipe, &root, &context).unwrap();

    eprintln!(
        "zstd cold skipped declarations: {:#?}",
        cold.skipped_programs
    );
    eprintln!(
        "zstd cold partial source lists: {:#?}",
        cold.partial_source_lists
    );
    eprintln!(
        "zstd cold deferred inventory patterns: {:#?}",
        cold.source_inventory_patterns
    );

    let expected_patterns: BTreeSet<_> = [
        "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib/common/*.c",
        "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib/compress/*.c",
        "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib/decompress/*.c",
        "${AROS_PORTS_DIR}/zstd/zstd-1.5.7/lib/dictBuilder/*.c",
    ]
    .into_iter()
    .collect();
    let actual_patterns: BTreeSet<_> = cold
        .source_inventory_patterns
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        actual_patterns, expected_patterns,
        "zstd cold wildcard inventory"
    );

    let fetch = cold
        .fetches
        .iter()
        .find(|fetch| fetch.name == "workbench-libs-zstd-fetch")
        .expect("zstd wildcard inventory must retain its owning fetch");
    assert_eq!(fetch.archive, "zstd-1.5.7");
    assert_eq!(fetch.destination, "${AROS_PORTS_DIR}/zstd");

    assert!(
        cold.targets
            .iter()
            .all(|target| target.mmake_name != "workbench-libs-zstd-library"
                && target.mmake_name != "linklibs-zstd"),
        "cold zstd parsing must not fabricate source-less library/archive targets: {:#?}",
        cold.targets
    );
    let alias = "workbench-libs-zstd-library-linklib".to_owned();
    let mut cold_graph = DependencyGraph::new();
    for target in cold.targets.iter().cloned() {
        cold_graph.add_target(target);
    }
    assert!(
        cold_graph
            .selected_dependency_closure(std::slice::from_ref(&alias), &context, &[])
            .is_err(),
        "cold parsing must not expose an archive alias without its fetched source list"
    );

    let Some(ports_dir) = std::env::var_os("AROS_TEST_PORTS_DIR").map(PathBuf::from) else {
        eprintln!("zstd warm probe skipped; set AROS_TEST_PORTS_DIR to materialized P4 ports");
        return;
    };
    let mut dirs = DirVars::load(&root);
    dirs.set_materialized_path("AROS_PORTS_DIR", ports_dir);
    let warm = parse_mmakefile_with_dirs_and_context(&recipe, &root, &dirs, &context).unwrap();
    eprintln!(
        "zstd warm skipped declarations: {:#?}",
        warm.skipped_programs
    );
    eprintln!(
        "zstd warm partial source lists: {:#?}",
        warm.partial_source_lists
    );
    eprintln!(
        "zstd warm deferred inventory patterns: {:#?}",
        warm.source_inventory_patterns
    );

    let module = warm
        .targets
        .iter()
        .find(|module| module.mmake_name == "workbench-libs-zstd-library")
        .unwrap_or_else(|| {
            panic!(
                "missing warm zstd library target; skipped={:#?}, partial={:#?}, inventory={:#?}",
                warm.skipped_programs, warm.partial_source_lists, warm.source_inventory_patterns
            )
        });
    assert_eq!(module.module_type, ModuleType::Library, "{module:#?}");
    assert_eq!(module.target_name, "zstd");
    // This P4 recipe does not override linklibname. Library/genmodule
    // metadata supplies the default client archive; no explicit override
    // should be invented merely because another source revision names one.
    assert_eq!(module.linklib_name, None);
    assert!(
        module
            .genmodule_linklibs
            .as_ref()
            .is_some_and(|metadata| metadata.enabled),
        "zstd genmodule archive metadata is absent or disabled: {:#?}",
        module.genmodule_linklibs
    );

    let expected_sources: Vec<String> = [
        "lib/common/debug",
        "lib/common/entropy_common",
        "lib/common/error_private",
        "lib/common/fse_decompress",
        "lib/common/pool",
        "lib/common/threading",
        "lib/common/xxhash",
        "lib/common/zstd_common",
        "lib/compress/fse_compress",
        "lib/compress/hist",
        "lib/compress/huf_compress",
        "lib/compress/zstd_compress",
        "lib/compress/zstd_compress_literals",
        "lib/compress/zstd_compress_sequences",
        "lib/compress/zstd_compress_superblock",
        "lib/compress/zstd_double_fast",
        "lib/compress/zstd_fast",
        "lib/compress/zstd_lazy",
        "lib/compress/zstd_ldm",
        "lib/compress/zstd_opt",
        "lib/compress/zstd_preSplit",
        "lib/compress/zstdmt_compress",
        "lib/decompress/huf_decompress",
        "lib/decompress/zstd_ddict",
        "lib/decompress/zstd_decompress",
        "lib/decompress/zstd_decompress_block",
        "lib/dictBuilder/cover",
        "lib/dictBuilder/divsufsort",
        "lib/dictBuilder/fastcover",
        "lib/dictBuilder/zdict",
    ]
    .into_iter()
    .map(|stem| format!("${{AROS_PORTS_DIR}}/zstd/zstd-1.5.7/{stem}"))
    .collect();
    assert_eq!(
        module.source_files, expected_sources,
        "warm zstd source inventory"
    );

    // Keep unrelated zstd link dependencies out of this one-recipe probe. The
    // parsed library metadata alone must make the selected-graph alias real.
    let alias = format!("{}-linklib", module.mmake_name);
    let mut isolated_module = module.clone();
    isolated_module.dependencies.clear();
    isolated_module.link_libs.clear();
    let mut graph = DependencyGraph::new();
    graph.add_target(isolated_module);
    let closure = graph
        .selected_dependency_closure(std::slice::from_ref(&alias), &context, &[])
        .unwrap_or_else(|error| panic!("missing implicit archive endpoint {alias}: {error}"));
    assert!(
        closure.contains(&alias),
        "implicit alias absent: {closure:#?}"
    );
    assert!(
        closure.contains("workbench-libs-zstd-library"),
        "library runtime endpoint absent: {closure:#?}"
    );
}
