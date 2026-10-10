//! Explicit LLVM compiler-family-v2 package metadata probes.
//! Fixture hashes are synthetic; no compiler provenance is claimed.

use aros_common::{sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest};
use aros_toolchain::{
    canonical,
    package::{package, package_with_format, PackageFormat, PackageRequest},
    package_extract::{verify_and_extract_with_format, PackageExtractionRequest},
    package_verify::{verify, verify_with_format, PackageVerificationRequest},
    profiles::Profiles,
    source_lock::SourceLock,
    Recipe,
};
use serde_json::{json, Value};
use std::path::Path;

fn source_lock_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "17.0.6",
        "sources": [{
            "component": "llvm",
            "version": "17.0.6",
            "purpose": "toolchain-component",
            "filename": "llvm-17.0.6.tar.xz",
            "url": "https://example.invalid/llvm-17.0.6.tar.xz",
            "sha256": "a".repeat(64),
            "size": 1
        }],
        "host_python_packages": [{
            "name": "wheel",
            "version": "0.43.0",
            "filename": "wheel-0.43.0.tar.gz",
            "url": "https://example.invalid/wheel-0.43.0.tar.gz",
            "sha256": "b".repeat(64),
            "size": 1,
            "source_root": "wheel-0.43.0",
            "python_path": "."
        }]
    }))
    .unwrap()
}

fn profiles_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": "a".repeat(40),
        "profiles": [{
            "name": "pc-x86_64",
            "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64",
            "platform": "pc",
            "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap()
}

fn renamed_profile_bytes(name: &str) -> Vec<u8> {
    let mut document: Value = serde_json::from_slice(&profiles_bytes()).unwrap();
    document["profiles"][0]["name"] = json!(name);
    serde_json::to_vec(&document).unwrap()
}

fn recipe(lock: &[u8], profiles: &[u8]) -> Recipe {
    recipe_with_source_commit(lock, profiles, &"1".repeat(40))
}

fn recipe_with_source_commit(lock: &[u8], profiles: &[u8], source_commit: &str) -> Recipe {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": source_commit,
        "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40),
        "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40),
        "tools_tree": "6".repeat(40),
        "source_date_epoch": 0,
        "source_lock_sha256": sha256_bytes(lock),
        "profiles_sha256": sha256_bytes(profiles),
        "patches": []
    });
    value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
    Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn make_request(root: &Path) -> PackageRequest {
    let lock = source_lock_bytes();
    let profiles = profiles_bytes();
    let candidate_root = root.join("candidate");
    std::fs::create_dir(&candidate_root).unwrap();
    std::fs::write(candidate_root.join("clang"), b"fixture compiler payload\n").unwrap();
    let profile = Profiles::parse(&profiles)
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone();
    PackageRequest {
        candidate_root,
        output_dir: root.join("package-v2"),
        release_id: "llvm-family-v2-fixture".into(),
        host: "macos-aarch64".into(),
        recipe: recipe(&lock, &profiles),
        source_lock: SourceLock::parse(&lock).unwrap(),
        profile,
        build_environment: serde_json::Map::default(),
        forbidden_prefixes: vec![],
    }
}

fn verification(request: &PackageRequest) -> PackageVerificationRequest {
    PackageVerificationRequest {
        package_dir: request.output_dir.clone(),
        release_id: request.release_id.clone(),
        host: request.host.clone(),
        recipe: request.recipe.clone(),
        source_lock: request.source_lock.clone(),
        profile: request.profile.clone(),
        build_environment: request.build_environment.clone(),
        forbidden_prefixes: request.forbidden_prefixes.clone(),
    }
}

#[test]
fn llvm_family_v2_roundtrips_while_default_remains_legacy_v1() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = make_request(temporary.path());
    let custom_profiles = renamed_profile_bytes("custom-pc-target");
    request.profile = Profiles::parse(&custom_profiles)
        .unwrap()
        .select("custom-pc-target")
        .unwrap()
        .clone();
    request.recipe = recipe(&source_lock_bytes(), &custom_profiles);
    let output = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap();
    assert_eq!(
        output.archive.file_name().unwrap().to_str().unwrap(),
        "aros-toolchain-v2-llvm17.0.6-macos-aarch64-custom-pc-target.tar.xz"
    );
    let manifest_json: Value =
        serde_json::from_slice(&std::fs::read(&output.manifest).unwrap()).unwrap();
    assert_eq!(manifest_json["schema"], 2);
    assert!(manifest_json.get("llvm_version").is_none());
    assert_eq!(manifest_json["compiler"]["family"], "llvm");
    assert_eq!(manifest_json["compiler"]["version"], "17.0.6");
    let verified =
        verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).unwrap();
    assert_eq!(verified.manifest.schema, 2);
    assert!(verified.manifest.llvm_version.is_none());
    assert_eq!(
        verified.manifest.compiler,
        Some(ArosCompilerIdentity::Llvm {
            version: "17.0.6".into()
        })
    );
    assert!(verify(&verification(&request)).is_err());
    let default_extraction_root = temporary.path().join("default-must-reject-v2");
    assert!(
        aros_toolchain::package_extract::verify_and_extract(&PackageExtractionRequest {
            verification: verification(&request),
            output_root: default_extraction_root.clone(),
        })
        .is_err()
    );
    assert!(!default_extraction_root.exists());
    let extracted = verify_and_extract_with_format(
        &PackageExtractionRequest {
            verification: verification(&request),
            output_root: temporary.path().join("relocated-v2"),
        },
        PackageFormat::CompilerFamilyV2,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(extracted.root.join("clang")).unwrap(),
        b"fixture compiler payload\n"
    );
    ArosToolchainManifest::load(&extracted.root).unwrap();

    let mut legacy_request = request;
    legacy_request.output_dir = temporary.path().join("package-v1");
    let original_profiles = profiles_bytes();
    legacy_request.profile = Profiles::parse(&original_profiles)
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone();
    legacy_request.recipe = recipe(&source_lock_bytes(), &original_profiles);
    let legacy = package(&legacy_request).unwrap();
    assert_eq!(
        legacy.archive.file_name().unwrap().to_str().unwrap(),
        "aros-toolchain-v1-llvm17.0.6-macos-aarch64-pc-x86_64.tar.xz"
    );
    let legacy_json: Value =
        serde_json::from_slice(&std::fs::read(&legacy.manifest).unwrap()).unwrap();
    assert_eq!(legacy_json["schema"], 1);
    assert_eq!(legacy_json["llvm_version"], "17.0.6");
    assert!(legacy_json.get("compiler").is_none());
    verify(&verification(&legacy_request)).unwrap();
}

#[test]
fn family_v2_requires_exact_recipe_bound_lock_and_profile_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = make_request(temporary.path());
    request.output_dir = temporary.path().join("absent-parent/package");
    let mut changed_lock = source_lock_bytes();
    changed_lock.push(b'\n');
    request.source_lock = SourceLock::parse(&changed_lock).unwrap();
    let error = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap_err();
    assert!(error
        .to_string()
        .contains("exact recipe-bound document bytes"));
    assert!(!request.output_dir.parent().unwrap().exists());

    let temporary = tempfile::tempdir().unwrap();
    let mut request = make_request(temporary.path());
    package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap();
    let mut changed_profiles = profiles_bytes();
    changed_profiles.push(b'\n');
    request.profile = Profiles::parse(&changed_profiles)
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone();
    let error =
        verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).unwrap_err();
    assert!(error
        .to_string()
        .contains("exact recipe-bound document bytes"));

    for mutation in ["cpu", "triple"] {
        let temporary = tempfile::tempdir().unwrap();
        let mut request = make_request(temporary.path());
        let mut changed_profiles: Value = serde_json::from_slice(&profiles_bytes()).unwrap();
        match mutation {
            "cpu" => changed_profiles["profiles"][0]["cpu"] = json!("arm"),
            _ => changed_profiles["profiles"][0]["target_triple"] = json!("x86_64-linux"),
        }
        let changed_profiles = serde_json::to_vec(&changed_profiles).unwrap();
        request.profile = Profiles::parse(&changed_profiles)
            .unwrap()
            .select("pc-x86_64")
            .unwrap()
            .clone();
        request.recipe = recipe(&source_lock_bytes(), &changed_profiles);
        let error = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap_err();
        assert!(error.to_string().contains("CPU must match"), "{mutation}");
        assert!(!request.output_dir.exists(), "{mutation}");
    }
}

#[test]
fn verifier_rejects_wrong_schema_compiler_asset_and_archive_corruption() {
    let temporary = tempfile::tempdir().unwrap();
    let request = make_request(temporary.path());
    let output = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap();
    let clean_manifest = std::fs::read(&output.manifest).unwrap();

    let mut wrong_recipe = request.clone();
    wrong_recipe.recipe =
        recipe_with_source_commit(&source_lock_bytes(), &profiles_bytes(), &"7".repeat(40));
    let error = verify_with_format(
        &verification(&wrong_recipe),
        PackageFormat::CompilerFamilyV2,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("not bound to the selected producer inputs"));

    let mut wrong_schema: Value = serde_json::from_slice(&clean_manifest).unwrap();
    wrong_schema["schema"] = json!(1);
    std::fs::write(&output.manifest, serde_json::to_vec(&wrong_schema).unwrap()).unwrap();
    assert!(verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).is_err());
    std::fs::write(&output.manifest, &clean_manifest).unwrap();

    let mut wrong_compiler: Value = serde_json::from_slice(&clean_manifest).unwrap();
    wrong_compiler["compiler"]["version"] = json!("18.0.0");
    std::fs::write(
        &output.manifest,
        serde_json::to_vec(&wrong_compiler).unwrap(),
    )
    .unwrap();
    let error =
        verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).unwrap_err();
    assert!(error
        .to_string()
        .contains("not bound to the selected producer inputs"));
    std::fs::write(&output.manifest, &clean_manifest).unwrap();

    let wrong_asset = output.archive.with_extension("renamed.tar.xz");
    std::fs::rename(&output.archive, &wrong_asset).unwrap();
    assert!(verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).is_err());
    std::fs::rename(wrong_asset, &output.archive).unwrap();

    let mut archive = std::fs::read(&output.archive).unwrap();
    archive[0] ^= 0x01;
    std::fs::write(&output.archive, archive).unwrap();
    assert!(verify_with_format(&verification(&request), PackageFormat::CompilerFamilyV2).is_err());
}

#[test]
fn legacy_format_rejects_gnu_and_package_writer_never_overwrites() {
    let temporary = tempfile::tempdir().unwrap();
    let request = make_request(temporary.path());
    let output = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap();
    let original_archive = std::fs::read(&output.archive).unwrap();
    let error = package_with_format(&request, PackageFormat::CompilerFamilyV2).unwrap_err();
    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read(&output.archive).unwrap(), original_archive);

    let lock = include_bytes!("fixtures/gnu-source-lock-v3.json");
    let profiles = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": "d".repeat(40),
        "profiles": [{
            "name": "rv64-aros",
            "configure_target": "riscv-aros",
            "upstream_output_target": "riscv64-aros",
            "target_triple": "riscv64-aros",
            "cpu": "riscv64",
            "platform": "riscv64",
            "float_abi": "lp64d",
            "capabilities": ["c", "libgcc", "standalone-collector"],
            "target": {
                "schema": "aros-riscv-target-v1",
                "isa": "rva22u64",
                "abi": "lp64d",
                "code_model": "medany",
                "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                "unaligned_access": false,
                "atomic_abi": 0,
                "x3_reg_usage": 0
            }
        }]
    }))
    .unwrap();
    let mut gnu_request = request;
    gnu_request.source_lock = SourceLock::parse(lock).unwrap();
    gnu_request.profile = Profiles::parse(&profiles)
        .unwrap()
        .select("rv64-aros")
        .unwrap()
        .clone();
    gnu_request.recipe = recipe(lock, &profiles);
    gnu_request.output_dir = temporary.path().join("gnu-legacy-output");
    let error = package_with_format(&gnu_request, PackageFormat::LegacyLlvmV1).unwrap_err();
    assert!(error.to_string().contains("only supported for LLVM"));
    assert!(!gnu_request.output_dir.exists());
}
