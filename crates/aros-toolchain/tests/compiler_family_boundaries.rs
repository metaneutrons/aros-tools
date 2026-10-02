//! Compiler-family substitution must not route input through another family.
//! Fixture hashes are synthetic; no compiler provenance is claimed.

use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{
    canonical,
    native_declaration::NativeExecutorDeclaration,
    package::{package, PackageRequest},
    package_verify::{verify, PackageVerificationRequest},
    profiles::Profiles,
    source_lock::SourceLock,
    Recipe,
};
use serde_json::json;

const LOCK: &[u8] = include_bytes!("fixtures/gnu-source-lock-v3.json");

fn profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1", "upstream_commit": "d".repeat(40),
        "profiles": [{
            "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    })).unwrap()
}

fn gnu_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2", "family": "gnu", "upstream_commit": "d".repeat(40),
        "profiles": [{
            "name": "rv32-aros", "configure_target": "esp32p4-riscv", "upstream_output_target": "esp32p4-riscv",
            "target_triple": "riscv-aros", "cpu": "riscv", "platform": "esp32p4", "float_abi": "ilp32f",
            "capabilities": ["c", "libgcc", "standalone-collector"],
            "target": {
                "schema": "aros-riscv-target-v1", "isa": "rv32imafc", "abi": "ilp32f", "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0", "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
            }
        }]
    })).unwrap()
}

fn recipe(profiles: &[u8]) -> Recipe {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
        "source_date_epoch": 0,
        "source_lock_sha256": sha256_bytes(LOCK), "profiles_sha256": sha256_bytes(profiles),
        "patches": []
    });
    value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
    Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn native_binding_rejects_gnu_before_any_llvm_phase() {
    let profiles = gnu_profiles();
    let recipe = recipe(&profiles);
    let contract = b"contract";
    let declaration = format!(
        "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/gnu.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
        sha256_bytes(contract), recipe.tools().0.as_str()
    );
    let error = NativeExecutorDeclaration::parse(declaration.as_bytes())
        .unwrap()
        .bind(&recipe, contract, LOCK, &profiles, "rv32-aros")
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("native GNU execution is not implemented"));
}

#[test]
fn native_binding_rejects_a_compiler_family_substitution() {
    let profiles = profiles();
    let recipe = recipe(&profiles);
    let contract = b"contract";
    let declaration = format!(
        "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/gnu.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
        sha256_bytes(contract), recipe.tools().0.as_str()
    );
    let error = NativeExecutorDeclaration::parse(declaration.as_bytes())
        .unwrap()
        .bind(&recipe, contract, LOCK, &profiles, "pc-x86_64")
        .unwrap_err();
    assert!(error.to_string().contains("different compiler families"));
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIdentity
    );
}

#[test]
fn package_and_readback_reject_gnu_without_creating_an_llvm_asset() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("sentinel"), b"unchanged").unwrap();
    let profiles_bytes = profiles();
    let profile = Profiles::parse(&profiles_bytes)
        .unwrap()
        .select("pc-x86_64")
        .unwrap()
        .clone();
    let mut request = PackageRequest {
        candidate_root: candidate.clone(),
        output_dir: temporary.path().join("absent/packages"),
        release_id: "local-test".into(),
        host: "macos-aarch64".into(),
        recipe: recipe(&profiles_bytes),
        source_lock: SourceLock::parse(LOCK).unwrap(),
        profile,
        build_environment: serde_json::Map::default(),
        forbidden_prefixes: vec![],
    };
    let error = package(&request).unwrap_err();
    assert!(error.to_string().contains("different compiler families"));
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerPackage
    );
    assert!(!temporary.path().join("absent").exists());
    assert_eq!(
        std::fs::read(candidate.join("sentinel")).unwrap(),
        b"unchanged"
    );

    // A GNU profile must not enter the LLVM path merely by substituting a
    // valid LLVM source lock. Profile clones retain the document's family.
    let mut llvm_lock: serde_json::Value = serde_json::from_slice(LOCK).unwrap();
    llvm_lock["schema"] = json!("aros-toolchain-source-lock-v2");
    llvm_lock["family"] = json!("llvm");
    llvm_lock["version"] = json!("11.0.0");
    llvm_lock["sources"] = json!([{
        "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
        "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
        "sha256": "a".repeat(64), "size": 1
    }]);
    request.source_lock = SourceLock::parse(&serde_json::to_vec(&llvm_lock).unwrap()).unwrap();
    request.profile = Profiles::parse(&gnu_profiles())
        .unwrap()
        .select("rv32-aros")
        .unwrap()
        .clone();
    let error = package(&request).unwrap_err();
    assert!(error.to_string().contains("different compiler families"));
    assert!(!temporary.path().join("absent").exists());

    let error = verify(&PackageVerificationRequest {
        package_dir: request.output_dir,
        release_id: request.release_id,
        host: request.host,
        recipe: request.recipe,
        source_lock: request.source_lock,
        profile: request.profile,
        build_environment: request.build_environment,
        forbidden_prefixes: request.forbidden_prefixes,
    })
    .unwrap_err();
    assert!(error.to_string().contains("different compiler families"));
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerVerification
    );
    assert!(!temporary.path().join("absent").exists());
}
