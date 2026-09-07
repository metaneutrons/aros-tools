//! Source-lock parsing is a closed producer contract, not a JSON convenience API.

use aros_common::sha256_bytes;
use aros_toolchain::{canonical, source_lock::SourceLock, Recipe};
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
fn rejects_a_recipe_with_a_different_patch_closure() {
    let parsed = SourceLock::parse(&serde_json::to_vec(&lock()).unwrap()).unwrap();
    let patches = json!([]);
    let recipe = recipe(&patches);
    assert!(parsed.verify_recipe_patches(&recipe).is_err());
}
