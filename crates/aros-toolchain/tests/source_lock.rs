//! Source-lock parsing is a closed producer contract, not a JSON convenience API.

use aros_common::sha256_bytes;
use aros_toolchain::{
    canonical,
    source_lock::{CompilerFamily, SourceLock},
    Recipe,
};
use serde_json::{json, Value};

fn lock() -> Value {
    json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "11.0.0",
        "sources": [{
            "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
            "patch": "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff",
            "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": "a".repeat(64), "size": 42
        }],
        "host_python_packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako-1.3.10.tar.gz",
            "url": "https://example.invalid/mako.tar.gz", "sha256": "b".repeat(64), "size": 7,
            "source_root": "mako-1.3.10", "python_path": "."
        }, {
            "name": "markupsafe", "version": "3.0.2", "filename": "markupsafe-3.0.2.tar.gz",
            "url": "https://example.invalid/markupsafe.tar.gz", "sha256": "c".repeat(64), "size": 8,
            "source_root": "markupsafe-3.0.2", "python_path": "src"
        }]
    })
}

fn v3_llvm_lock() -> Value {
    let mut document = lock();
    document["schema"] = json!("aros-toolchain-source-lock-v3");
    document
}

fn gnu_lock() -> Value {
    let mut document = lock();
    document["schema"] = json!("aros-toolchain-source-lock-v3");
    document["family"] = json!("gnu");
    document["version"] = json!("13.2.0");
    document["sources"] = json!([
        {
            "component": "gcc", "version": "13.2.0", "purpose": "toolchain-component",
            "patch": "tools/crosstools/gnu/gcc-13.2.0-aros.diff",
            "filename": "gcc-13.2.0.tar.xz", "url": "https://example.invalid/gcc.tar.xz",
            "sha256": "a".repeat(64), "size": 42
        },
        {
            "component": "binutils", "version": "2.42", "purpose": "toolchain-component",
            "patch": "tools/crosstools/gnu/binutils-2.42-aros.diff",
            "filename": "binutils-2.42.tar.xz", "url": "https://example.invalid/binutils.tar.xz",
            "sha256": "d".repeat(64), "size": 43
        }
    ]);
    document
}

fn recipe(patches: &Value) -> Recipe {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
        "source_date_epoch": 0,
        "source_lock_sha256": "d".repeat(64), "profiles_sha256": "e".repeat(64),
        "patches": patches
    });
    value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
    Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn validates_the_complete_payload_and_patch_closure() {
    let parsed = SourceLock::parse(&serde_json::to_vec(&lock()).unwrap()).unwrap();
    assert_eq!(parsed.version(), "11.0.0");
    assert_eq!(parsed.family(), CompilerFamily::Llvm);
    assert_eq!(parsed.payloads().count(), 3);
    assert_eq!(parsed.host_python_packages()[0].name(), "mako");
    assert_eq!(parsed.host_python_packages()[1].python_path(), "src");
    let patches = json!([{
        "path": "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff",
        "sha256": "f".repeat(64)
    }]);
    let recipe = recipe(&patches);
    parsed.verify_recipe_patches(&recipe).unwrap();
}

#[test]
fn accepts_v3_llvm_and_gnu_family_declarations() {
    let llvm = SourceLock::parse(&serde_json::to_vec(&v3_llvm_lock()).unwrap()).unwrap();
    assert_eq!(llvm.family(), CompilerFamily::Llvm);
    assert_eq!(llvm.version(), "11.0.0");

    let gnu = SourceLock::parse(&serde_json::to_vec(&gnu_lock()).unwrap()).unwrap();
    assert_eq!(gnu.family(), CompilerFamily::Gnu);
    assert_eq!(gnu.version(), "13.2.0");
    let components = gnu.source_components().collect::<Vec<_>>();
    assert_eq!(components.len(), 2);
    assert_eq!(components[0].component(), "gcc");
    assert_eq!(
        components[0].purpose(),
        aros_toolchain::source_lock::SourcePurpose::ToolchainComponent
    );
    assert_eq!(components[1].component(), "binutils");
    assert_eq!(gnu.source_patch_paths().count(), 2);
}

#[test]
fn gnu_recipe_binding_requires_the_exact_source_patch_closure() {
    let parsed = SourceLock::parse(&serde_json::to_vec(&gnu_lock()).unwrap()).unwrap();
    let patches = json!([
        {"path": "tools/crosstools/gnu/binutils-2.42-aros.diff", "sha256": "f".repeat(64)},
        {"path": "tools/crosstools/gnu/gcc-13.2.0-aros.diff", "sha256": "e".repeat(64)}
    ]);
    parsed.verify_recipe_patches(&recipe(&patches)).unwrap();
    let mut wrong = patches;
    wrong[1]["path"] = json!("tools/crosstools/llvm/gcc-13.2.0-aros.diff");
    assert!(parsed.verify_recipe_patches(&recipe(&wrong)).is_err());
    assert!(parsed.verify_recipe_patches(&recipe(&json!([]))).is_err());
}

#[test]
fn rejects_ambiguous_or_unsafe_lock_material() {
    let mut duplicate = lock();
    duplicate["host_python_packages"][1]["filename"] = json!("mako-1.3.10.tar.gz");
    assert!(SourceLock::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());

    let mut escaped = lock();
    escaped["sources"][0]["patch"] = json!("../outside-aros.diff");
    assert!(SourceLock::parse(&serde_json::to_vec(&escaped).unwrap()).is_err());

    let mut credential = lock();
    credential["sources"][0]["url"] = json!("https://secret@example.invalid/llvm.tar.xz");
    assert!(SourceLock::parse(&serde_json::to_vec(&credential).unwrap()).is_err());

    let mut query = lock();
    query["sources"][0]["url"] = json!("https://example.invalid/llvm.tar.xz?moving=true");
    assert!(SourceLock::parse(&serde_json::to_vec(&query).unwrap()).is_err());

    let mut unknown = lock();
    unknown["extra"] = json!(true);
    assert!(SourceLock::parse(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let mut missing_hash = lock();
    missing_hash["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("sha256");
    assert!(SourceLock::parse(&serde_json::to_vec(&missing_hash).unwrap()).is_err());
}

#[test]
fn rejects_wrong_schema_or_family_combinations() {
    let mut unknown_schema = v3_llvm_lock();
    unknown_schema["schema"] = json!("aros-toolchain-source-lock-v4");
    assert!(SourceLock::parse(&serde_json::to_vec(&unknown_schema).unwrap()).is_err());

    let mut v2_gnu = lock();
    v2_gnu["family"] = json!("gnu");
    assert!(SourceLock::parse(&serde_json::to_vec(&v2_gnu).unwrap()).is_err());

    let mut unknown_family = v3_llvm_lock();
    unknown_family["family"] = json!("other");
    assert!(SourceLock::parse(&serde_json::to_vec(&unknown_family).unwrap()).is_err());
}

#[test]
fn rejects_incomplete_or_inconsistent_gnu_components() {
    let mut missing_gcc = gnu_lock();
    missing_gcc["sources"] = json!([missing_gcc["sources"][1].clone()]);
    assert!(SourceLock::parse(&serde_json::to_vec(&missing_gcc).unwrap()).is_err());

    let mut missing_binutils = gnu_lock();
    missing_binutils["sources"] = json!([missing_binutils["sources"][0].clone()]);
    assert!(SourceLock::parse(&serde_json::to_vec(&missing_binutils).unwrap()).is_err());

    let mut mismatched_gcc = gnu_lock();
    mismatched_gcc["sources"][0]["version"] = json!("13.2.1");
    assert!(SourceLock::parse(&serde_json::to_vec(&mismatched_gcc).unwrap()).is_err());

    let mut nonnumeric_gcc = gnu_lock();
    nonnumeric_gcc["version"] = json!("13.2-rc1");
    nonnumeric_gcc["sources"][0]["version"] = json!("13.2-rc1");
    assert!(SourceLock::parse(&serde_json::to_vec(&nonnumeric_gcc).unwrap()).is_err());

    let mut unsupported_binutils = gnu_lock();
    unsupported_binutils["sources"][1]["version"] = json!("2.42.1.0.1");
    assert!(SourceLock::parse(&serde_json::to_vec(&unsupported_binutils).unwrap()).is_err());

    let mut gcc_without_toolchain_role = gnu_lock();
    gcc_without_toolchain_role["sources"][0]["purpose"] = json!("target-build-dependency");
    assert!(SourceLock::parse(&serde_json::to_vec(&gcc_without_toolchain_role).unwrap()).is_err());

    let mut duplicate_gcc = gnu_lock();
    let duplicate = duplicate_gcc["sources"][0].clone();
    duplicate_gcc["sources"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(SourceLock::parse(&serde_json::to_vec(&duplicate_gcc).unwrap()).is_err());

    let mut conflicting_binutils = gnu_lock();
    let mut duplicate = conflicting_binutils["sources"][1].clone();
    duplicate["version"] = json!("2.43");
    duplicate["filename"] = json!("binutils-2.43.tar.xz");
    duplicate["patch"] = json!("tools/crosstools/gnu/binutils-2.43-aros.diff");
    conflicting_binutils["sources"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(SourceLock::parse(&serde_json::to_vec(&conflicting_binutils).unwrap()).is_err());
}

#[test]
fn gnu_versions_share_the_bounded_artifact_grammar() {
    for gcc in ["2147483647.0.0", "0.2147483647.0", "0.0.2147483647"] {
        for binutils in ["2147483647.0", "0.2147483647.0", "0.0.0.2147483647"] {
            let mut document = gnu_lock();
            document["version"] = json!(gcc);
            document["sources"][0]["version"] = json!(gcc);
            document["sources"][1]["version"] = json!(binutils);
            let parsed = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
            assert_eq!(parsed.version(), gcc);
            aros_common::validate_gnu_compiler_versions(gcc, binutils).unwrap();
        }
    }

    for gcc in [
        "2147483648.0.0",
        "0.2147483648.0",
        "0.0.2147483648",
        "4294967296.0.0",
        "00000000000.0.0",
    ] {
        let mut document = gnu_lock();
        document["version"] = json!(gcc);
        document["sources"][0]["version"] = json!(gcc);
        let error = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code.to_string(),
            "AX0101"
        );
        assert!(error.to_string().contains("GNU source lock requires"));
        assert!(aros_common::validate_gnu_compiler_versions(gcc, "2.42").is_err());
    }

    for binutils in [
        "2147483648.0",
        "0.2147483648",
        "0.0.2147483648",
        "0.0.0.2147483648",
        "4294967296.0",
        "00000000000.0",
    ] {
        let mut document = gnu_lock();
        document["sources"][1]["version"] = json!(binutils);
        let error = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code.to_string(),
            "AX0101"
        );
        assert!(error.to_string().contains("GNU source lock requires"));
        assert!(aros_common::validate_gnu_compiler_versions("13.2.0", binutils).is_err());
    }
}

#[test]
fn legacy_llvm_source_versions_keep_their_existing_numeric_grammar() {
    for schema in [
        "aros-toolchain-source-lock-v2",
        "aros-toolchain-source-lock-v3",
    ] {
        let mut document = lock();
        document["schema"] = json!(schema);
        document["version"] = json!("2147483648.0.0");
        document["sources"][0]["version"] = json!("2147483648.0.0");
        let parsed = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(parsed.version(), "2147483648.0.0");
    }
}

#[test]
fn rejects_patches_outside_the_selected_compiler_namespace() {
    let mut gnu_with_llvm_patch = gnu_lock();
    gnu_with_llvm_patch["sources"][0]["patch"] =
        json!("tools/crosstools/llvm/gcc-13.2.0-aros.diff");
    assert!(SourceLock::parse(&serde_json::to_vec(&gnu_with_llvm_patch).unwrap()).is_err());

    let mut llvm_with_gnu_patch = v3_llvm_lock();
    llvm_with_gnu_patch["sources"][0]["patch"] =
        json!("tools/crosstools/gnu/llvm-11.0.0-aros.diff");
    assert!(SourceLock::parse(&serde_json::to_vec(&llvm_with_gnu_patch).unwrap()).is_err());
}

#[test]
fn rejects_a_recipe_with_a_different_patch_closure() {
    let parsed = SourceLock::parse(&serde_json::to_vec(&lock()).unwrap()).unwrap();
    let patches = json!([]);
    let recipe = recipe(&patches);
    assert!(parsed.verify_recipe_patches(&recipe).is_err());
}

#[test]
fn v3_binds_source_owned_dependency_patches_without_a_compiler_namespace() {
    for mut document in [gnu_lock(), v3_llvm_lock()] {
        let dependency = json!({
            "component": "fixture-runtime", "version": "1.0",
            "purpose": "target-build-dependency",
            "patch": "workbench/libs/fixture-runtime/1.0-aros.diff",
            "filename": "fixture-runtime.tar.gz", "url": "https://example.invalid/runtime.tar.gz",
            "sha256": "e".repeat(64), "size": 7
        });
        document["sources"].as_array_mut().unwrap().push(dependency);
        let parsed = SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap();
        let mut patches = parsed
            .source_patch_paths()
            .map(|path| json!({"path": path, "sha256": "f".repeat(64)}))
            .collect::<Vec<_>>();
        patches.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        parsed
            .verify_recipe_patches(&recipe(&json!(patches)))
            .unwrap();
        let mut missing = patches;
        missing.pop();
        assert!(parsed
            .verify_recipe_patches(&recipe(&json!(missing)))
            .is_err());

        let index = document["sources"].as_array().unwrap().len() - 1;
        for invalid in [
            "../outside-aros.diff",
            "/outside-aros.diff",
            "workbench//bad-aros.diff",
            "workbench/plain.diff",
        ] {
            let mut wrong = document.clone();
            wrong["sources"][index]["patch"] = json!(invalid);
            assert!(SourceLock::parse(&serde_json::to_vec(&wrong).unwrap()).is_err());
        }
        let mut duplicate = document.clone();
        duplicate["sources"][index]["patch"] = document["sources"][0]["patch"].clone();
        assert!(SourceLock::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        if document["family"] == "llvm" {
            document["schema"] = json!("aros-toolchain-source-lock-v2");
            assert!(SourceLock::parse(&serde_json::to_vec(&document).unwrap()).is_err());
        }
    }
}
