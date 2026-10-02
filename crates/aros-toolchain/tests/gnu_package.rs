//! GNU v2 packaging/read-back identity probes, not compiler build evidence.

use aros_common::{sha256_bytes, ArosCompilerIdentity, DiagnosticCode};
use aros_toolchain::{
    canonical,
    package::{package, PackageRequest},
    package_extract::{verify_and_extract, PackageExtractionRequest},
    package_verify::{verify, PackageVerificationRequest},
    profiles::Profiles,
    source_lock::SourceLock,
    Recipe,
};
use serde_json::{json, Value};

const LOCK: &[u8] = include_bytes!("fixtures/gnu-source-lock-v3.json");

fn profiles(width: u8) -> Vec<u8> {
    let (cpu, triple, abi, isa, architecture) = if width == 32 {
        (
            "riscv",
            "riscv-aros",
            "ilp32f",
            "rv32imafc",
            "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
        )
    } else {
        (
            "riscv64",
            "riscv64-aros",
            "lp64d",
            "rva22u64",
            "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
        )
    };
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2", "family": "gnu", "upstream_commit": "d".repeat(40),
        "profiles": [{
            "name": format!("rv{width}-aros"), "configure_target": format!("fixture-rv{width}"),
            "upstream_output_target": format!("fixture-rv{width}"), "target_triple": triple,
            "cpu": cpu, "platform": "fixture", "float_abi": abi,
            "capabilities": ["c", "libgcc", "standalone-collector"],
            "target": {"schema": "aros-riscv-target-v1", "isa": isa, "abi": abi,
                "code_model": "medany", "architecture": architecture, "unaligned_access": false,
                "atomic_abi": 0, "x3_reg_usage": 0}
        }]
    }))
    .unwrap()
}

fn recipe(profiles: &[u8]) -> Recipe {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
        "source_date_epoch": 0, "source_lock_sha256": sha256_bytes(LOCK),
        "profiles_sha256": sha256_bytes(profiles), "patches": []
    });
    value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
    Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn request(temporary: &std::path::Path, width: u8) -> PackageRequest {
    let candidate = temporary.join("candidate");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("fixture-input"), b"not an actual compiler\n").unwrap();
    let document = profiles(width);
    PackageRequest {
        candidate_root: candidate,
        output_dir: temporary.join("package-a"),
        release_id: "rv2-local-fixture".into(),
        host: "macos-aarch64".into(),
        recipe: recipe(&document),
        source_lock: SourceLock::parse(LOCK).unwrap(),
        profile: Profiles::parse(&document)
            .unwrap()
            .select(&format!("rv{width}-aros"))
            .unwrap()
            .clone(),
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
fn both_widths_package_verify_extract_with_exact_family_and_target_identity() {
    for width in [32, 64] {
        let temporary = tempfile::tempdir().unwrap();
        let mut request = request(temporary.path(), width);
        let output = package(&request).unwrap();
        assert_eq!(
            output.archive.file_name().unwrap().to_str().unwrap(),
            format!("aros-toolchain-v2-gcc16.2.0-binutils2.47-macos-aarch64-rv{width}-aros.tar.xz")
        );
        let verified = verify(&verification(&request)).unwrap();
        assert_eq!(verified.manifest.schema, 2);
        assert!(verified.manifest.llvm_version.is_none());
        assert!(matches!(
            verified.manifest.compiler,
            Some(ArosCompilerIdentity::Gnu { .. })
        ));
        let bytes = std::fs::read(&output.manifest).unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json.get("llvm_version").is_none());
        assert_eq!(
            json["compiler"]["target"]["abi"],
            if width == 32 { "ilp32f" } else { "lp64d" }
        );
        let extracted = verify_and_extract(&PackageExtractionRequest {
            verification: verification(&request),
            output_root: temporary.path().join("relocated"),
        })
        .unwrap();
        assert_eq!(
            std::fs::read(extracted.root.join("fixture-input")).unwrap(),
            b"not an actual compiler\n"
        );
        request.output_dir = temporary.path().join("package-b");
        let repeat = package(&request).unwrap();
        assert_eq!(repeat.archive_sha256, output.archive_sha256);
        assert_eq!(repeat.archive_size, output.archive_size);
    }
}

#[test]
fn raw_source_or_profile_substitution_fails_before_filesystem_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let mut request = request(temporary.path(), 32);
    request.output_dir = temporary.path().join("absent-parent/packages");
    let mut modified_lock = LOCK.to_vec();
    modified_lock.push(b'\n');
    request.source_lock = SourceLock::parse(&modified_lock).unwrap();
    let error = package(&request).unwrap_err();
    assert!(error.to_string().contains("recipe-bound document bytes"));
    assert!(!request.output_dir.exists());
    assert!(!temporary.path().join("absent-parent").exists());
    let error = verify(&verification(&request)).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerVerification
    );
    assert!(error.to_string().contains("recipe-bound document bytes"));
    request.source_lock = SourceLock::parse(LOCK).unwrap();
    let mut modified_profiles = profiles(32);
    modified_profiles.push(b'\n');
    request.profile = Profiles::parse(&modified_profiles)
        .unwrap()
        .select("rv32-aros")
        .unwrap()
        .clone();
    assert!(package(&request)
        .unwrap_err()
        .to_string()
        .contains("recipe-bound document bytes"));
    assert!(!request.output_dir.exists());
    assert!(!temporary.path().join("absent-parent").exists());
    assert_eq!(
        std::fs::read(request.candidate_root.join("fixture-input")).unwrap(),
        b"not an actual compiler\n"
    );
}

#[test]
fn readback_rejects_changed_compiler_version_and_target_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let request = request(temporary.path(), 32);
    let output = package(&request).unwrap();
    let bytes = std::fs::read(&output.manifest).unwrap();
    let pristine: Value = serde_json::from_slice(&bytes).unwrap();
    for key in ["gcc_version", "binutils_version", "target"] {
        let mut altered = pristine.clone();
        match key {
            "gcc_version" => altered["compiler"][key] = json!("16.3.0"),
            "binutils_version" => altered["compiler"][key] = json!("2.48"),
            _ => altered["compiler"]["target"]["code_model"] = json!("medlow"),
        }
        std::fs::write(&output.manifest, serde_json::to_vec(&altered).unwrap()).unwrap();
        let error = verify(&verification(&request)).unwrap_err();
        assert!(error
            .to_string()
            .contains("not bound to the selected producer inputs"));
        let extraction_root = temporary.path().join("must-remain-absent");
        let error = verify_and_extract(&PackageExtractionRequest {
            verification: verification(&request),
            output_root: extraction_root.clone(),
        })
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("not bound to the selected producer inputs"));
        assert!(!extraction_root.exists());
    }
    std::fs::write(&output.manifest, bytes).unwrap();
    verify(&verification(&request)).unwrap();
}
