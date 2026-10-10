use aros_common::ArosToolchainManifest;
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("fixtures/toolchain-manifest-v2.examples.json");

fn fixture_manifest(name: &str) -> Value {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("v2 example fixture must be JSON");
    let mut matches = fixture["examples"]
        .as_array()
        .expect("fixture examples must be an array")
        .iter()
        .filter(|example| example["name"] == name);
    let selected = matches
        .next()
        .unwrap_or_else(|| panic!("missing fixture example {name}"));
    assert!(matches.next().is_none(), "duplicate fixture example {name}");
    selected["manifest"].clone()
}

fn assert_manifest_rejected(candidate: Value, case: &str) {
    if let Ok(manifest) = serde_json::from_value::<ArosToolchainManifest>(candidate) {
        assert!(
            manifest.validate().is_err(),
            "invalid manifest case was accepted: {case}"
        );
    }
}

#[test]
fn synthetic_v2_examples_parse_validate_and_roundtrip() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("v2 example fixture must be JSON");
    assert!(fixture["notice"].as_str().unwrap().contains("Synthetic"));

    let examples = fixture["examples"].as_array().unwrap();
    assert_eq!(examples.len(), 3);
    let names: std::collections::BTreeSet<_> = examples
        .iter()
        .map(|example| example["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), examples.len(), "example names must be unique");
    for example in examples {
        assert!(example["evidence"]
            .as_str()
            .unwrap()
            .contains("no build or SDK proof"));
        let name = example["name"].as_str().unwrap();
        let manifest: ArosToolchainManifest = serde_json::from_value(example["manifest"].clone())
            .unwrap_or_else(|error| panic!("{name} did not deserialize: {error}"));
        assert_eq!(manifest.schema, 2, "{name}");
        assert!(manifest.llvm_version.is_none(), "{name}");
        assert!(manifest.compiler.is_some(), "{name}");
        manifest
            .validate()
            .unwrap_or_else(|error| panic!("{name} did not validate: {error}"));

        let serialized = serde_json::to_vec(&manifest).expect("manifest must serialize");
        let roundtrip: ArosToolchainManifest = serde_json::from_slice(&serialized)
            .unwrap_or_else(|error| panic!("{name} roundtrip did not deserialize: {error}"));
        roundtrip
            .validate()
            .unwrap_or_else(|error| panic!("{name} roundtrip did not validate: {error}"));
        assert_eq!(roundtrip, manifest, "{name} roundtrip changed the manifest");
    }
}

#[test]
fn v2_semantic_negative_cases_are_rejected() {
    for llvm_version in [Value::Null, json!("18.1.8")] {
        let mut candidate = fixture_manifest("llvm-x86_64");
        candidate["llvm_version"] = llvm_version;
        assert_manifest_rejected(candidate, "schema-v2 llvm_version, including null");
    }

    let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
    candidate["compiler"]["family"] = json!("unknown");
    assert_manifest_rejected(candidate, "unknown compiler family");

    let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
    candidate["compiler"]["unexpected"] = json!(true);
    assert_manifest_rejected(candidate, "unknown compiler field");

    let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
    candidate["compiler"]["target"]["unexpected"] = json!(true);
    assert_manifest_rejected(candidate, "unknown RISC-V target field");

    let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
    candidate
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), json!(true));
    assert_manifest_rejected(candidate, "unknown manifest field");

    let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
    candidate["target_triple"] = json!("riscv64-unknown-aros");
    assert_manifest_rejected(candidate, "RV32 target with RV64 triple");

    let mut candidate = fixture_manifest("gnu-rv64-rva22u64-lp64d");
    candidate["target_triple"] = json!("riscv64-aros\n");
    assert_manifest_rejected(candidate, "newline after GNU target triple");

    for (field, unsafe_selector) in [("isa", "rv32imafc\n"), ("architecture", "rv32i2p1_f2p2\n")] {
        let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
        candidate["compiler"]["target"][field] = json!(unsafe_selector);
        assert_manifest_rejected(candidate, &format!("newline in RISC-V {field}"));
    }

    for (field, invalid_version) in [
        ("gcc_version", "14.x.0"),
        ("gcc_version", "2147483648.0.0"),
        ("gcc_version", "16.2.0\n"),
        ("binutils_version", "2.43.x"),
        ("binutils_version", "2.2147483648"),
        ("binutils_version", "2.47\n"),
    ] {
        let mut candidate = fixture_manifest("gnu-rv32-p4-ilp32f");
        candidate["compiler"][field] = json!(invalid_version);
        assert_manifest_rejected(
            candidate,
            &format!("invalid GNU {field}: {invalid_version}"),
        );
    }

    for invalid_version in ["18.x.8", "2147483648.0.0", "18.1.8\n"] {
        let mut candidate = fixture_manifest("llvm-x86_64");
        candidate["compiler"]["version"] = json!(invalid_version);
        assert_manifest_rejected(
            candidate,
            &format!("invalid LLVM version: {invalid_version}"),
        );
    }

    for field in ["tree_sha256", "source_commit", "files"] {
        let mut candidate = fixture_manifest("gnu-rv64-rva22u64-lp64d");
        match field {
            "tree_sha256" => candidate[field] = json!(format!("{}\n", "a".repeat(64))),
            "source_commit" => candidate[field] = json!(format!("{}\n", "a".repeat(40))),
            "files" => candidate["files"][2]["sha256"] = json!(format!("{}\n", "a".repeat(64))),
            _ => unreachable!(),
        }
        assert_manifest_rejected(candidate, &format!("newline in {field} hash or OID"));
    }

    let mut candidate = fixture_manifest("llvm-x86_64");
    candidate["files"][0]["mode"] = json!("0644");
    assert_manifest_rejected(candidate, "directory with invalid mode");

    let mut candidate = fixture_manifest("llvm-x86_64");
    candidate["files"][2]["target"] = json!("unexpected-target-field");
    assert_manifest_rejected(candidate, "file entry with invalid fields");

    let mut candidate = fixture_manifest("llvm-x86_64");
    candidate["files"].as_array_mut().unwrap().reverse();
    assert_manifest_rejected(candidate, "unsorted inventory");

    let mut candidate = fixture_manifest("llvm-x86_64");
    candidate["files"][3]["target"] = json!("../../escape");
    assert_manifest_rejected(candidate, "symlink escaping toolchain root");
}

#[test]
fn source_date_epoch_accepts_u64_max_and_rejects_overflow() {
    let mut candidate = fixture_manifest("gnu-rv64-rva22u64-lp64d");
    candidate["source_date_epoch"] = json!(u64::MAX);
    let manifest: ArosToolchainManifest = serde_json::from_value(candidate)
        .expect("u64::MAX is within the manifest source_date_epoch range");
    manifest.validate().expect("u64::MAX should validate");

    let overflow = serde_json::to_string(&manifest)
        .expect("manifest must serialize")
        .replace(&u64::MAX.to_string(), "18446744073709551616");
    assert!(
        serde_json::from_str::<ArosToolchainManifest>(&overflow).is_err(),
        "source_date_epoch greater than u64::MAX must fail deserialization"
    );
}
