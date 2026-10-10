//! Synthetic release-index contract checks; these do not establish build or publication evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{
    canonical,
    compatibility::{extract_two_roots, extract_two_roots_with_format, TwoRootRelocationRequest},
    package::{package_with_format, PackageFormat, PackageRequest},
    package_verify::PackageVerificationRequest,
    release_index_v2::NativeReleaseIndexV2,
    release_inputs::ReleaseInputs,
};
use serde_json::{json, Value};

const GNU_SOURCE_COMMIT: &str = "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";
const GNU_RV32_SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
const GNU_RV64_SOURCE_COMMIT: &str = "2222222222222222222222222222222222222222";
const LLVM_SOURCE_COMMIT: &str = "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2";
const LLVM_BASELINE_SOURCE_COMMIT: &str = "3333333333333333333333333333333333333333";
const PRODUCER_COMMIT: &str = "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3";
const TOOLS_COMMIT: &str = "b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4";
const PRODUCER_TREE: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
const TOOLS_TREE: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";
const HOSTS: &[&str] = &["linux-aarch64", "linux-x86_64", "macos-aarch64"];
const LLVM_BASELINE_PROFILES: &[(&str, &str)] = &[
    ("pc-x86_64", "x86_64-unknown-aros"),
    ("arm-raspi", "arm-unknown-aros"),
    ("rpi-aarch64", "aarch64-unknown-aros"),
];
const SUPPORT_FILES: &[&str] = &[
    "toolchain-release-inputs-v2.json",
    "toolchain-index-v2.json",
    "SHA256SUMS",
    "toolchain-provenance.sigstore.json",
    "toolchain-manifest-v2.schema.json",
    "tree-digest-v1.fixture.json",
];

#[derive(Clone)]
struct Fixture {
    collection: Value,
    documents: BTreeMap<String, Vec<u8>>,
}

type MutationCase = (&'static str, fn(&mut Value));

fn target(abi: &str, architecture: &str, isa: &str) -> Value {
    json!({
        "schema": "aros-riscv-target-v1",
        "isa": isa,
        "abi": abi,
        "code_model": "medany",
        "architecture": architecture,
        "unaligned_access": false,
        "atomic_abi": 0,
        "x3_reg_usage": 0
    })
}

fn gnu_profile(width: u8) -> Value {
    let (cpu, triple, abi, architecture, isa) = if width == 32 {
        (
            "riscv",
            "riscv-aros",
            "ilp32f",
            "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
            "rv32imafc",
        )
    } else {
        (
            "riscv64",
            "riscv64-aros",
            "lp64d",
            "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            "rva22u64",
        )
    };
    json!({
        "name": format!("rv{width}-aros"),
        "configure_target": format!("fixture-rv{width}"),
        "upstream_output_target": format!("fixture-rv{width}"),
        "target_triple": triple,
        "cpu": cpu,
        "platform": "fixture",
        "float_abi": abi,
        "capabilities": ["c", "libgcc", "standalone-collector"],
        "target": target(abi, architecture, isa)
    })
}

fn gnu_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v3",
        "family": "gnu",
        "version": "16.2.0",
        "sources": [
            {
                "component": "gcc", "version": "16.2.0", "purpose": "toolchain-component",
                "filename": "gcc.tar.xz", "url": "https://example.invalid/gcc.tar.xz",
                "sha256": "1".repeat(64), "size": 1
            },
            {
                "component": "binutils", "version": "2.47", "purpose": "toolchain-component",
                "filename": "binutils.tar.xz", "url": "https://example.invalid/binutils.tar.xz",
                "sha256": "2".repeat(64), "size": 1
            }
        ],
        "host_python_packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako.tar.gz",
            "url": "https://example.invalid/mako.tar.gz", "sha256": "3".repeat(64),
            "size": 1, "source_root": "mako", "python_path": "."
        }]
    }))
    .unwrap()
}

fn gnu_profiles_for(source_commit: &str, widths: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": source_commit,
        "profiles": widths.iter().map(|width| gnu_profile(*width)).collect::<Vec<_>>()
    }))
    .unwrap()
}

fn gnu_profiles() -> Vec<u8> {
    gnu_profiles_for(GNU_SOURCE_COMMIT, &[32, 64])
}

fn llvm_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "17.0.6",
        "sources": [{
            "component": "llvm", "version": "17.0.6", "purpose": "toolchain-component",
            "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": "4".repeat(64), "size": 1
        }],
        "host_python_packages": [{
            "name": "wheel", "version": "0.43.0", "filename": "wheel.tar.gz",
            "url": "https://example.invalid/wheel.tar.gz", "sha256": "5".repeat(64),
            "size": 1, "source_root": "wheel", "python_path": "."
        }]
    }))
    .unwrap()
}

fn llvm_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "llvm",
        "upstream_commit": LLVM_SOURCE_COMMIT,
        "profiles": [{
            "name": "pc-x86_64", "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64", "platform": "pc", "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap()
}

fn llvm_baseline_profiles() -> Vec<u8> {
    let profiles = LLVM_BASELINE_PROFILES
        .iter()
        .map(|(name, _)| match *name {
            "pc-x86_64" => json!({
                "name": "pc-x86_64", "configure_target": "pc-x86_64",
                "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
                "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": [
                    "c", "cxx", "objc", "compiler-rt", "compiler-rt32", "libcxx",
                    "libcxxabi", "libunwind", "standalone-collector", "multilib-collector"
                ]
            }),
            "arm-raspi" => json!({
                "name": "arm-raspi", "configure_target": "raspi-armhf",
                "upstream_output_target": "raspi-arm", "target_triple": "arm-unknown-aros",
                "cpu": "arm", "platform": "raspi", "float_abi": "hard",
                "capabilities": [
                    "c", "cxx", "objc", "compiler-rt", "libcxx", "libcxxabi",
                    "libunwind", "standalone-collector"
                ]
            }),
            "rpi-aarch64" => json!({
                "name": "rpi-aarch64", "configure_target": "raspi-aarch64",
                "upstream_output_target": "raspi-aarch64", "target_triple": "aarch64-unknown-aros",
                "cpu": "aarch64", "platform": "raspi", "float_abi": "",
                "capabilities": [
                    "c", "cxx", "objc", "compiler-rt", "libcxx", "libcxxabi",
                    "libunwind", "standalone-collector"
                ]
            }),
            _ => unreachable!("baseline profile list and definitions must stay aligned"),
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": LLVM_BASELINE_SOURCE_COMMIT,
        "profiles": profiles
    }))
    .unwrap()
}

fn recipe_bytes(source_lock: &[u8], profiles: &[u8], source_commit: &str) -> Vec<u8> {
    let mut recipe = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": source_commit,
        "source_tree": "7".repeat(40),
        "producer_commit": PRODUCER_COMMIT,
        "producer_tree": PRODUCER_TREE,
        "tools_commit": TOOLS_COMMIT,
        "tools_tree": TOOLS_TREE,
        "source_date_epoch": 0,
        "source_lock_sha256": sha256_bytes(source_lock),
        "profiles_sha256": sha256_bytes(profiles),
        "patches": []
    });
    recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
    serde_json::to_vec(&recipe).unwrap()
}

fn reference(file: &str, bytes: &[u8]) -> Value {
    json!({"file": file, "sha256": sha256_bytes(bytes)})
}

fn add_group(
    documents: &mut BTreeMap<String, Vec<u8>>,
    id: &str,
    source_commit: &str,
    source_lock: Vec<u8>,
    profiles: Vec<u8>,
) -> Value {
    let recipe = recipe_bytes(&source_lock, &profiles, source_commit);
    let names = [
        (format!("{id}-recipe.json"), recipe),
        (format!("{id}-source-lock.json"), source_lock),
        (format!("{id}-profiles.json"), profiles),
    ];
    let refs = names
        .iter()
        .map(|(name, bytes)| reference(name, bytes))
        .collect::<Vec<_>>();
    for (name, bytes) in names {
        documents.insert(name, bytes);
    }
    json!({
        "id": id,
        "recipe": refs[0],
        "source_lock": refs[1],
        "profiles": refs[2]
    })
}

fn fixture() -> Fixture {
    let mut documents = BTreeMap::new();
    let gnu = add_group(
        &mut documents,
        "gnu-riscv",
        GNU_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles(),
    );
    let llvm = add_group(
        &mut documents,
        "llvm-pc",
        LLVM_SOURCE_COMMIT,
        llvm_source_lock(),
        llvm_profiles(),
    );
    Fixture {
        collection: json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": HOSTS,
            "groups": [gnu, llvm]
        }),
        documents,
    }
}

fn collection_bytes(fixture: &Fixture) -> Vec<u8> {
    serde_json::to_vec(&fixture.collection).unwrap()
}

fn validated_inputs(fixture: &Fixture) -> ReleaseInputs {
    ReleaseInputs::parse(&collection_bytes(fixture), &fixture.documents).unwrap()
}

fn gnu_compiler(width: u8) -> Value {
    let (abi, architecture, isa) = if width == 32 {
        ("ilp32f", "rv32i2p1_m2p0_a2p1_f2p2_c2p0", "rv32imafc")
    } else {
        ("lp64d", "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0", "rva22u64")
    };
    json!({
        "family": "gnu",
        "gcc_version": "16.2.0",
        "binutils_version": "2.47",
        "target": target(abi, architecture, isa)
    })
}

fn artifact(
    group_id: &str,
    profile: &str,
    triple: &str,
    host: &str,
    source_commit: &str,
    family: &str,
) -> Value {
    let (asset, compiler) = if family == "gnu" {
        let width = if profile == "rv32-aros" { 32 } else { 64 };
        (
            format!("aros-toolchain-v2-gcc16.2.0-binutils2.47-{host}-{profile}.tar.xz"),
            gnu_compiler(width),
        )
    } else {
        (
            format!("aros-toolchain-v2-llvm17.0.6-{host}-{profile}.tar.xz"),
            json!({"family": "llvm", "version": "17.0.6"}),
        )
    };
    json!({
        "group_id": group_id,
        "asset": asset,
        "sha256": "a".repeat(64),
        "size": 4096,
        "host": host,
        "target_profile": profile,
        "target_triple": triple,
        "source_commit": source_commit,
        "compiler": compiler,
        "tree_sha256": "b".repeat(64),
        "enabled": true,
        "strip_components": 1,
        "required_paths": ["bin/riscv-aros-gcc", "share/aros/toolchain-manifest-v2.json"]
    })
}

fn index_value(fixture: &Fixture) -> Value {
    let mut artifacts = Vec::new();
    for host in HOSTS {
        artifacts.push(artifact(
            "gnu-riscv",
            "rv32-aros",
            "riscv-aros",
            host,
            GNU_SOURCE_COMMIT,
            "gnu",
        ));
        artifacts.push(artifact(
            "gnu-riscv",
            "rv64-aros",
            "riscv64-aros",
            host,
            GNU_SOURCE_COMMIT,
            "gnu",
        ));
        artifacts.push(artifact(
            "llvm-pc",
            "pc-x86_64",
            "x86_64-unknown-aros",
            host,
            LLVM_SOURCE_COMMIT,
            "llvm",
        ));
    }
    artifacts.sort_by(|left, right| left["asset"].as_str().cmp(&right["asset"].as_str()));
    json!({
        "schema": 2,
        "release_id": "release-2026.10",
        "base_url": "https://example.invalid/aros/releases/2026.10",
        "inputs_sha256": sha256_bytes(&collection_bytes(fixture)),
        "producer_commit": PRODUCER_COMMIT,
        "tools_commit": TOOLS_COMMIT,
        "artifacts": artifacts
    })
}

fn split_gnu_fixture() -> Fixture {
    let mut documents = BTreeMap::new();
    let gnu32 = add_group(
        &mut documents,
        "gnu-rv32",
        GNU_RV32_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles_for(GNU_RV32_SOURCE_COMMIT, &[32]),
    );
    let gnu64 = add_group(
        &mut documents,
        "gnu-rv64",
        GNU_RV64_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles_for(GNU_RV64_SOURCE_COMMIT, &[64]),
    );
    let llvm = add_group(
        &mut documents,
        "llvm-pc",
        LLVM_SOURCE_COMMIT,
        llvm_source_lock(),
        llvm_profiles(),
    );
    Fixture {
        collection: json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": HOSTS,
            "groups": [gnu32, gnu64, llvm]
        }),
        documents,
    }
}

fn rv32_llvm_baseline_fixture() -> Fixture {
    let mut documents = BTreeMap::new();
    let gnu_rv32 = add_group(
        &mut documents,
        "gnu-rv32",
        GNU_RV32_SOURCE_COMMIT,
        gnu_source_lock(),
        gnu_profiles_for(GNU_RV32_SOURCE_COMMIT, &[32]),
    );
    let llvm = add_group(
        &mut documents,
        "llvm-baseline",
        LLVM_BASELINE_SOURCE_COMMIT,
        llvm_source_lock(),
        llvm_baseline_profiles(),
    );
    Fixture {
        collection: json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": HOSTS,
            "groups": [gnu_rv32, llvm]
        }),
        documents,
    }
}

fn rv32_llvm_baseline_index_value(fixture: &Fixture) -> Value {
    let mut artifacts = Vec::new();
    for host in HOSTS {
        artifacts.push(artifact(
            "gnu-rv32",
            "rv32-aros",
            "riscv-aros",
            host,
            GNU_RV32_SOURCE_COMMIT,
            "gnu",
        ));
        for (profile, triple) in LLVM_BASELINE_PROFILES {
            artifacts.push(artifact(
                "llvm-baseline",
                profile,
                triple,
                host,
                LLVM_BASELINE_SOURCE_COMMIT,
                "llvm",
            ));
        }
    }
    artifacts.sort_by(|left, right| left["asset"].as_str().cmp(&right["asset"].as_str()));
    json!({
        "schema": 2,
        "release_id": "release-2026.10",
        "base_url": "https://example.invalid/aros/releases/2026.10",
        "inputs_sha256": sha256_bytes(&collection_bytes(fixture)),
        "producer_commit": PRODUCER_COMMIT,
        "tools_commit": TOOLS_COMMIT,
        "artifacts": artifacts
    })
}

fn split_gnu_index_value(fixture: &Fixture) -> Value {
    let mut artifacts = Vec::new();
    for host in HOSTS {
        artifacts.push(artifact(
            "gnu-rv32",
            "rv32-aros",
            "riscv-aros",
            host,
            GNU_RV32_SOURCE_COMMIT,
            "gnu",
        ));
        artifacts.push(artifact(
            "gnu-rv64",
            "rv64-aros",
            "riscv64-aros",
            host,
            GNU_RV64_SOURCE_COMMIT,
            "gnu",
        ));
        artifacts.push(artifact(
            "llvm-pc",
            "pc-x86_64",
            "x86_64-unknown-aros",
            host,
            LLVM_SOURCE_COMMIT,
            "llvm",
        ));
    }
    artifacts.sort_by(|left, right| left["asset"].as_str().cmp(&right["asset"].as_str()));
    json!({
        "schema": 2,
        "release_id": "release-2026.10",
        "base_url": "https://example.invalid/aros/releases/2026.10",
        "inputs_sha256": sha256_bytes(&collection_bytes(fixture)),
        "producer_commit": PRODUCER_COMMIT,
        "tools_commit": TOOLS_COMMIT,
        "artifacts": artifacts
    })
}

fn parse_value(
    input: &ReleaseInputs,
    value: &Value,
) -> Result<NativeReleaseIndexV2, aros_toolchain::ContractError> {
    NativeReleaseIndexV2::parse(&serde_json::to_vec(value).unwrap(), input)
}

fn assert_index_error(input: &ReleaseInputs, value: &Value) -> aros_toolchain::ContractError {
    let error = parse_value(input, value).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
    error
}

#[test]
fn binds_mixed_compiler_lanes_roundtrips_and_derives_exact_inventory() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let wire = index_value(&fixture);
    let index = parse_value(&inputs, &wire).unwrap();

    assert_eq!(index.release_id(), "release-2026.10");
    assert_eq!(
        index.base_url(),
        "https://example.invalid/aros/releases/2026.10"
    );
    assert_eq!(
        index.inputs_sha256(),
        &sha256_bytes(&collection_bytes(&fixture))
    );
    assert_eq!(index.producer_commit(), inputs.producer_commit());
    assert_eq!(index.tools_commit(), inputs.tools_commit());
    assert_eq!(index.artifacts().len(), 9);

    let encoded = index.to_json_bytes().unwrap();
    let encoded_value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(encoded_value, wire);
    let roundtrip = NativeReleaseIndexV2::parse(&encoded, &inputs).unwrap();
    assert_eq!(roundtrip.to_json_bytes().unwrap(), encoded);
    for (parsed, declared) in index
        .artifacts()
        .iter()
        .zip(wire["artifacts"].as_array().unwrap())
    {
        assert_eq!(parsed.group_id(), declared["group_id"].as_str().unwrap());
        assert_eq!(parsed.asset(), declared["asset"].as_str().unwrap());
        assert_eq!(
            parsed.sha256().to_string(),
            declared["sha256"].as_str().unwrap()
        );
        assert_eq!(parsed.size(), declared["size"].as_u64().unwrap());
        assert_eq!(parsed.host(), declared["host"].as_str().unwrap());
        assert_eq!(
            parsed.target_profile(),
            declared["target_profile"].as_str().unwrap()
        );
        assert_eq!(
            parsed.target_triple(),
            declared["target_triple"].as_str().unwrap()
        );
        assert_eq!(
            parsed.source_commit().as_str(),
            declared["source_commit"].as_str().unwrap()
        );
        assert_eq!(
            serde_json::to_value(parsed.compiler()).unwrap(),
            declared["compiler"]
        );
        assert_eq!(
            parsed.tree_sha256().to_string(),
            declared["tree_sha256"].as_str().unwrap()
        );
        assert!(parsed.enabled());
        assert_eq!(parsed.strip_components(), 1);
        assert_eq!(
            parsed.required_paths(),
            declared["required_paths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|path| path.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        );
    }

    let mut expected = SUPPORT_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    for group in inputs.groups() {
        expected.insert(group.recipe_reference().file().to_owned());
        expected.insert(group.source_lock_reference().file().to_owned());
        expected.insert(group.profiles_reference().file().to_owned());
    }
    for declared in wire["artifacts"].as_array().unwrap() {
        let asset = declared["asset"].as_str().unwrap();
        expected.insert(asset.to_owned());
        expected.insert(format!("{asset}.manifest.json"));
        expected.insert(format!("{asset}.sha256"));
        expected.insert(format!("{asset}.spdx.json"));
    }
    assert_eq!(index.expected_inventory(), &expected);
    assert_eq!(index.expected_inventory().len(), 48);
    let folded = index
        .expected_inventory()
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    assert_eq!(folded.len(), index.expected_inventory().len());
}

#[test]
fn rejects_invalid_top_level_binding_and_closed_document_variants() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let valid = index_value(&fixture);
    let cases: &[MutationCase] = &[
        ("schema zero", |v| v["schema"] = json!(0)),
        ("schema unknown", |v| v["schema"] = json!(3)),
        ("wrong raw inputs digest", |v| {
            v["inputs_sha256"] = json!("9".repeat(64));
        }),
        ("uppercase digest", |v| {
            v["inputs_sha256"] = json!("A".repeat(64));
        }),
        ("wrong producer commit", |v| {
            v["producer_commit"] = json!("9".repeat(40));
        }),
        ("wrong tools commit", |v| {
            v["tools_commit"] = json!("8".repeat(40));
        }),
        ("missing artifact", |v| {
            v["artifacts"].as_array_mut().unwrap().pop();
        }),
        ("extra artifact", |v| {
            let item = v["artifacts"][0].clone();
            v["artifacts"].as_array_mut().unwrap().push(item);
        }),
        ("duplicate asset", |v| {
            let first = v["artifacts"][0].clone();
            v["artifacts"][1] = first;
        }),
        ("unsorted assets", |v| {
            v["artifacts"].as_array_mut().unwrap().swap(0, 1);
        }),
        ("unknown top-level field", |v| {
            v["private-index-field-marker"] = json!("private-index-value-marker");
        }),
    ];
    for (name, mutate) in cases {
        let mut changed = valid.clone();
        mutate(&mut changed);
        let error = assert_index_error(&inputs, &changed);
        assert!(
            !error.to_string().contains("private-index-field-marker"),
            "{name}"
        );
        assert!(
            !error.to_string().contains("private-index-value-marker"),
            "{name}"
        );
    }

    let bytes = serde_json::to_vec(&valid).unwrap();
    let duplicate_schema =
        String::from_utf8(bytes)
            .unwrap()
            .replacen("\"schema\":2", "\"schema\":2,\"schema\":2", 1);
    let error = NativeReleaseIndexV2::parse(duplicate_schema.as_bytes(), &inputs).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );

    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    let error = NativeReleaseIndexV2::parse(&oversized, &inputs).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerIndex
    );
}

#[test]
fn rejects_release_identifiers_and_noncanonical_or_credentialed_urls() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let valid = index_value(&fixture);
    for release_id in ["Release-2026.10", "", ".", ".."] {
        let mut changed = valid.clone();
        changed["release_id"] = json!(release_id);
        assert_index_error(&inputs, &changed);
    }
    let mut overlong_id = valid.clone();
    overlong_id["release_id"] = json!("a".repeat(129));
    assert_index_error(&inputs, &overlong_id);
    for base_url in [
        "http://example.invalid/releases/2026.10",
        "https://user:private-token@example.invalid/releases/2026.10",
        "https://example.invalid/",
        "https://example.invalid/releases/2026.10/",
        "https://example.invalid/releases/2026.10?token=private-token",
        "https://example.invalid/releases/2026.10#private-token",
        "HTTPS://example.invalid/releases/2026.10",
        "https://EXAMPLE.invalid/releases/2026.10",
    ] {
        let mut changed = valid.clone();
        changed["base_url"] = json!(base_url);
        let error = assert_index_error(&inputs, &changed);
        assert!(!error.to_string().contains("private-token"));
    }
}

#[test]
fn rejects_artifacts_that_do_not_match_the_derived_lane_matrix() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let valid = index_value(&fixture);
    let cases: &[MutationCase] = &[
        ("wrong group", |v| {
            v["artifacts"][0]["group_id"] = json!("llvm-pc");
        }),
        ("wrong host", |v| {
            v["artifacts"][0]["host"] = json!("linux-riscv64");
        }),
        ("Intel host", |v| {
            v["artifacts"][0]["host"] = json!("macos-x86_64");
        }),
        ("wrong profile", |v| {
            v["artifacts"][0]["target_profile"] = json!("pc-x86_64");
        }),
        ("wrong package name", |v| {
            v["artifacts"][0]["asset"] = json!("aros-toolchain-v2-intel.tar.xz");
        }),
        ("wrong triple", |v| {
            v["artifacts"][0]["target_triple"] = json!("riscv64-unknown-aros");
        }),
        ("wrong source commit", |v| {
            v["artifacts"][0]["source_commit"] = json!("9".repeat(40));
        }),
        ("wrong compiler family", |v| {
            v["artifacts"][0]["compiler"] = json!({"family":"llvm","version":"17.0.6"});
        }),
        ("wrong compiler version", |v| {
            v["artifacts"][0]["compiler"]["gcc_version"] = json!("16.3.0");
        }),
        ("wrong compiler ABI", |v| {
            v["artifacts"][0]["compiler"]["target"]["abi"] = json!("lp64d");
        }),
        ("unknown artifact field", |v| {
            v["artifacts"][0]["private-artifact-field-marker"] =
                json!("private-index-value-marker");
        }),
        ("legacy LLVM version field", |v| {
            let index = v["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .position(|artifact| artifact["group_id"] == "llvm-pc")
                .unwrap();
            v["artifacts"][index]["llvm_version"] = json!("17.0.6");
        }),
        ("unknown compiler field", |v| {
            v["artifacts"][0]["compiler"]["private-compiler-field-marker"] =
                json!("private-index-value-marker");
        }),
        ("unknown target field", |v| {
            v["artifacts"][0]["compiler"]["target"]["private-target-field-marker"] =
                json!("private-index-value-marker");
        }),
        ("missing artifact key", |v| {
            v["artifacts"][0].as_object_mut().unwrap().remove("sha256");
        }),
    ];
    for (name, mutate) in cases {
        let mut changed = valid.clone();
        mutate(&mut changed);
        let error = assert_index_error(&inputs, &changed);
        assert!(!error.to_string().contains(name));
        for sentinel in [
            "private-artifact-field-marker",
            "private-compiler-field-marker",
            "private-target-field-marker",
            "private-index-value-marker",
        ] {
            assert!(!error.to_string().contains(sentinel), "{name}");
        }
    }

    let llvm_index = valid["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .position(|item| item["group_id"] == "llvm-pc")
        .unwrap();
    let mut wrong_llvm_version = valid;
    wrong_llvm_version["artifacts"][llvm_index]["compiler"]["version"] = json!("17.0.7");
    assert_index_error(&inputs, &wrong_llvm_version);
}

#[test]
fn rejects_invalid_artifact_digests_sizes_layout_and_required_paths() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let valid = index_value(&fixture);
    let cases: &[MutationCase] = &[
        ("bad archive digest", |v| {
            v["artifacts"][0]["sha256"] = json!("A".repeat(64));
        }),
        ("bad tree digest", |v| {
            v["artifacts"][0]["tree_sha256"] = json!("g".repeat(64));
        }),
        ("zero size", |v| v["artifacts"][0]["size"] = json!(0)),
        ("overbound size", |v| {
            v["artifacts"][0]["size"] = json!(32_u64 * 1024 * 1024 * 1024 + 1);
        }),
        ("disabled", |v| v["artifacts"][0]["enabled"] = json!(false)),
        ("wrong strip depth", |v| {
            v["artifacts"][0]["strip_components"] = json!(2);
        }),
        ("empty paths", |v| {
            v["artifacts"][0]["required_paths"] = json!([]);
        }),
        ("unsorted paths", |v| {
            v["artifacts"][0]["required_paths"] = json!(["z", "a"]);
        }),
        ("duplicate paths", |v| {
            v["artifacts"][0]["required_paths"] = json!(["bin/tool", "bin/tool"]);
        }),
        ("parent path", |v| {
            v["artifacts"][0]["required_paths"] = json!(["../escape"]);
        }),
        ("dot path segment", |v| {
            v["artifacts"][0]["required_paths"] = json!(["bin/./tool"]);
        }),
        ("empty path segment", |v| {
            v["artifacts"][0]["required_paths"] = json!(["bin//tool"]);
        }),
        ("backslash path", |v| {
            v["artifacts"][0]["required_paths"] = json!(["bin\\tool"]);
        }),
        ("control path", |v| {
            v["artifacts"][0]["required_paths"] = json!(["bin/\u{0001}tool"]);
        }),
        ("overlong path", |v| {
            v["artifacts"][0]["required_paths"] = json!(["p".repeat(1025)]);
        }),
        ("absolute path", |v| {
            v["artifacts"][0]["required_paths"] = json!(["/bin/tool"]);
        }),
    ];
    for (name, mutate) in cases {
        let mut changed = valid.clone();
        mutate(&mut changed);
        let error = assert_index_error(&inputs, &changed);
        assert!(!error.to_string().contains("bin\\tool"), "{name}");
    }

    let mut too_many_paths = valid.clone();
    too_many_paths["artifacts"][0]["required_paths"] = json!(vec!["p"; 500_001]);
    assert_index_error(&inputs, &too_many_paths);

    let mut maximum_size = valid.clone();
    maximum_size["artifacts"][0]["size"] = json!(32_u64 * 1024 * 1024 * 1024);
    assert!(parse_value(&inputs, &maximum_size).is_ok());

    let mut maximum_path = valid;
    maximum_path["artifacts"][0]["required_paths"] = json!(["p".repeat(1024)]);
    assert!(parse_value(&inputs, &maximum_path).is_ok());
}

#[test]
fn rejects_duplicate_nested_keys_and_colliding_derived_inventory_names() {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let valid = index_value(&fixture);
    let encoded = String::from_utf8(serde_json::to_vec(&valid).unwrap()).unwrap();
    let duplicate_fields = [
        (
            "\"asset\":\"aros-toolchain-v2-gcc16.2.0-binutils2.47-linux-aarch64-rv32-aros.tar.xz\"",
            "\"asset\":\"aros-toolchain-v2-gcc16.2.0-binutils2.47-linux-aarch64-rv32-aros.tar.xz\",\"asset\":\"aros-toolchain-v2-gcc16.2.0-binutils2.47-linux-aarch64-rv32-aros.tar.xz\"",
        ),
        (
            "\"family\":\"gnu\"",
            "\"family\":\"gnu\",\"family\":\"gnu\"",
        ),
        (
            "\"gcc_version\":\"16.2.0\"",
            "\"gcc_version\":\"16.2.0\",\"gcc_version\":\"16.2.0\"",
        ),
        (
            "\"required_paths\":[\"bin/riscv-aros-gcc\",\"share/aros/toolchain-manifest-v2.json\"]",
            "\"required_paths\":[\"bin/riscv-aros-gcc\",\"share/aros/toolchain-manifest-v2.json\"],\"required_paths\":[\"bin/riscv-aros-gcc\",\"share/aros/toolchain-manifest-v2.json\"]",
        ),
    ];
    for (needle, duplicate) in duplicate_fields {
        let changed = encoded.replacen(needle, duplicate, 1);
        assert_ne!(
            changed, encoded,
            "duplicate-key probe did not match its field"
        );
        let error = NativeReleaseIndexV2::parse(changed.as_bytes(), &inputs).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerIndex
        );
    }

    let mut collision_fixture = fixture;
    let wire = index_value(&collision_fixture);
    let asset = wire["artifacts"][0]["asset"].as_str().unwrap();
    let colliding_name = format!("{asset}.manifest.json");
    let old_name = collision_fixture.collection["groups"][0]["recipe"]["file"]
        .as_str()
        .unwrap()
        .to_owned();
    let recipe = collision_fixture.documents.remove(&old_name).unwrap();
    collision_fixture
        .documents
        .insert(colliding_name.clone(), recipe.clone());
    collision_fixture.collection["groups"][0]["recipe"]["file"] = json!(colliding_name);
    collision_fixture.collection["groups"][0]["recipe"]["sha256"] = json!(sha256_bytes(&recipe));
    let collision_inputs = validated_inputs(&collision_fixture);
    let collision_index = index_value(&collision_fixture);
    assert_index_error(&collision_inputs, &collision_index);
}

fn qualification_policy(now: u64) -> Value {
    json!({
        "source_repository": "https://github.com/example/aros-toolchains",
        "source_workflow": ".github/workflows/qualification.yml",
        "signer_repository": "https://github.com/example/aros-toolchains",
        "signer_workflow": ".github/workflows/qualification.yml",
        "signer": "github-actions",
        "now": now
    })
}

fn qualification_evidence_value(
    inputs: &ReleaseInputs,
    index_bytes: &[u8],
    coverage: &str,
) -> Value {
    let index = NativeReleaseIndexV2::parse(index_bytes, inputs).unwrap();
    let mut report_id = 1_u64;
    let lanes = index
        .artifacts()
        .iter()
        .map(|artifact| {
            let group = inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .unwrap();
            let lane = json!({
                "group_id": artifact.group_id(),
                "asset": artifact.asset(),
                "host": artifact.host(),
                "target_profile": artifact.target_profile(),
                "target_triple": artifact.target_triple(),
                "source_commit": artifact.source_commit(),
                "compiler": artifact.compiler(),
                "recipe_sha256": group.recipe().sha256(),
                "source_lock_sha256": group.source_lock_reference().sha256(),
                "profiles_sha256": group.profiles_reference().sha256(),
                "archive_sha256": artifact.sha256(),
                "archive_size": artifact.size(),
                "tree_sha256": artifact.tree_sha256(),
                "build_a_report_sha256": format!("{:064x}", report_id),
                "build_b_report_sha256": format!("{:064x}", report_id + 1),
                "comparison_report_sha256": format!("{:064x}", report_id + 2),
                "compatibility_report_sha256": format!("{:064x}", report_id + 3)
            });
            report_id += 4;
            lane
        })
        .collect::<Vec<_>>();
    json!({
        "schema": "aros-toolchain-qualification-evidence-v2",
        "created_at": 100,
        "expires_at": 300,
        "source_run": {
            "repository": "https://github.com/example/aros-toolchains",
            "workflow": ".github/workflows/qualification.yml",
            "run_id": 42,
            "producer_commit": PRODUCER_COMMIT,
            "source_tag": "release-2026.10",
            "tag_object": "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a"
        },
        "release": {
            "release_id": index.release_id(),
            "base_url": index.base_url(),
            "inputs_sha256": inputs.collection_sha256(),
            "release_index_sha256": sha256_bytes(index_bytes),
            "pre_attestation_checksums_sha256": "c".repeat(64),
            "checksums_sha256": "d".repeat(64),
            "provenance_sha256": "d".repeat(64),
            "producer_commit": index.producer_commit(),
            "tools_commit": index.tools_commit()
        },
        "attestation": {
            "repository": "https://github.com/example/aros-toolchains",
            "workflow": ".github/workflows/qualification.yml",
            "signer": "github-actions",
            "subject_manifest_sha256": "c".repeat(64)
        },
        "lanes": lanes,
        "coverage": coverage
    })
}

fn qualification_setup(coverage: &str) -> (Fixture, ReleaseInputs, Vec<u8>, Value, Value) {
    let fixture = fixture();
    let inputs = validated_inputs(&fixture);
    let index_bytes = NativeReleaseIndexV2::parse(
        &serde_json::to_vec(&index_value(&fixture)).unwrap(),
        &inputs,
    )
    .unwrap()
    .to_json_bytes()
    .unwrap();
    let evidence = qualification_evidence_value(&inputs, &index_bytes, coverage);
    let policy = qualification_policy(150);
    (fixture, inputs, index_bytes, evidence, policy)
}

fn assert_qualification_invalid(
    evidence: &Value,
    index_bytes: &[u8],
    inputs: &ReleaseInputs,
    policy: &Value,
) {
    let bytes = serde_json::to_vec(evidence).unwrap();
    let Ok(parsed) =
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(&bytes)
    else {
        return;
    };
    assert!(parsed
        .validate_against_index(
            index_bytes,
            inputs,
            &serde_json::from_value(policy.clone()).unwrap()
        )
        .is_err());
}

#[test]
fn compiler_family_v2_candidate_baseline_requires_all_llvm_and_rv32_lanes_without_rv64() {
    // This synthetic collection models the next candidate's coverage. It is
    // not a production profile registry or evidence of actual compiler builds.
    let fixture = rv32_llvm_baseline_fixture();
    let inputs = validated_inputs(&fixture);
    assert_eq!(inputs.groups().len(), 2);

    let gnu_group = inputs
        .groups()
        .iter()
        .find(|group| group.id() == "gnu-rv32")
        .unwrap();
    let gnu_profiles = gnu_group
        .profiles()
        .entries()
        .iter()
        .map(aros_toolchain::profiles::Profile::name)
        .collect::<BTreeSet<_>>();
    assert_eq!(gnu_profiles, BTreeSet::from(["rv32-aros"]));

    let llvm_group = inputs
        .groups()
        .iter()
        .find(|group| group.id() == "llvm-baseline")
        .unwrap();
    let llvm_profiles = llvm_group
        .profiles()
        .entries()
        .iter()
        .map(aros_toolchain::profiles::Profile::name)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        llvm_profiles,
        LLVM_BASELINE_PROFILES
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>()
    );

    let index = parse_value(&inputs, &rv32_llvm_baseline_index_value(&fixture)).unwrap();
    assert_eq!(index.artifacts().len(), 12);
    assert_eq!(
        index
            .artifacts()
            .iter()
            .filter(|artifact| artifact.group_id() == "gnu-rv32")
            .count(),
        3
    );
    assert_eq!(
        index
            .artifacts()
            .iter()
            .filter(|artifact| artifact.group_id() == "llvm-baseline")
            .count(),
        9
    );
    assert!(!index
        .artifacts()
        .iter()
        .any(|artifact| artifact.target_profile() == "rv64-aros"));

    let selectors = index
        .artifacts()
        .iter()
        .map(|artifact| {
            (
                artifact.group_id(),
                artifact.host(),
                artifact.target_profile(),
            )
        })
        .collect::<BTreeSet<_>>();
    let expected_selectors = HOSTS
        .iter()
        .flat_map(|host| {
            std::iter::once(("gnu-rv32", *host, "rv32-aros")).chain(
                LLVM_BASELINE_PROFILES
                    .iter()
                    .map(move |(profile, _)| ("llvm-baseline", *host, *profile)),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(selectors.len(), 12);
    assert_eq!(selectors, expected_selectors);

    let mut expected_inventory = SUPPORT_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    for group in inputs.groups() {
        expected_inventory.insert(group.recipe_reference().file().to_owned());
        expected_inventory.insert(group.source_lock_reference().file().to_owned());
        expected_inventory.insert(group.profiles_reference().file().to_owned());
    }
    for artifact in index.artifacts() {
        let asset = artifact.asset();
        expected_inventory.insert(asset.to_owned());
        expected_inventory.insert(format!("{asset}.manifest.json"));
        expected_inventory.insert(format!("{asset}.sha256"));
        expected_inventory.insert(format!("{asset}.spdx.json"));
    }
    assert_eq!(index.expected_inventory(), &expected_inventory);
    assert_eq!(index.expected_inventory().len(), 60);
    let folded_inventory = index
        .expected_inventory()
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    assert_eq!(folded_inventory.len(), 60);

    let index_bytes = index.to_json_bytes().unwrap();
    let evidence = qualification_evidence_value(&inputs, &index_bytes, "release-candidate");
    let evidence_bytes = serde_json::to_vec(&evidence).unwrap();
    let parsed =
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(&evidence_bytes)
            .unwrap();
    assert_eq!(parsed.lanes.len(), 12);
    assert_ne!(
        parsed.release.pre_attestation_checksums_sha256,
        parsed.release.checksums_sha256
    );
    assert_eq!(
        parsed.attestation.subject_manifest_sha256,
        parsed.release.pre_attestation_checksums_sha256
    );
    let build_receipts = parsed
        .lanes
        .iter()
        .flat_map(|lane| {
            [
                lane.build_a_report_sha256.as_str(),
                lane.build_b_report_sha256.as_str(),
            ]
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(build_receipts.len(), 24);
    let compatibility_receipts = parsed
        .lanes
        .iter()
        .map(|lane| lane.compatibility_report_sha256.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(compatibility_receipts.len(), 12);
    let all_report_receipts = parsed
        .lanes
        .iter()
        .flat_map(|lane| {
            [
                lane.build_a_report_sha256.as_str(),
                lane.build_b_report_sha256.as_str(),
                lane.comparison_report_sha256.as_str(),
                lane.compatibility_report_sha256.as_str(),
            ]
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(all_report_receipts.len(), 48);

    let policy = serde_json::from_value(qualification_policy(150)).unwrap();
    parsed
        .validate_against_index(&index_bytes, &inputs, &policy)
        .unwrap();

    let mut missing_rv32_lane = evidence;
    let missing_index = missing_rv32_lane["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|lane| lane["group_id"] == "gnu-rv32" && lane["target_profile"] == "rv32-aros")
        .unwrap();
    missing_rv32_lane["lanes"]
        .as_array_mut()
        .unwrap()
        .remove(missing_index);
    assert_qualification_invalid(
        &missing_rv32_lane,
        &index_bytes,
        &inputs,
        &qualification_policy(150),
    );
}

#[test]
fn qualification_v2_binds_all_nine_compiler_family_lanes_and_exact_policy() {
    let (_fixture, inputs, index_bytes, evidence, policy) =
        qualification_setup("release-candidate");
    let bytes = serde_json::to_vec(&evidence).unwrap();
    let parsed =
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(&bytes).unwrap();
    assert_eq!(parsed.lanes.len(), 9);
    parsed
        .validate_against_index(
            &index_bytes,
            &inputs,
            &serde_json::from_value(policy).unwrap(),
        )
        .unwrap();

    let selectors = parsed
        .lanes
        .iter()
        .map(|lane| {
            (
                lane.group_id.as_str(),
                lane.host.as_str(),
                lane.target_profile.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(selectors.len(), 9);
    assert_eq!(
        parsed
            .lanes
            .iter()
            .filter(|lane| lane.group_id == "gnu-riscv")
            .count(),
        6
    );
    assert_eq!(
        parsed
            .lanes
            .iter()
            .filter(|lane| lane.group_id == "llvm-pc")
            .count(),
        3
    );

    // A v2 record is never silently interpreted by the independently versioned v1 parser.
    assert!(aros_toolchain::qualification_evidence::QualificationEvidence::parse(&bytes).is_err());
}

#[test]
fn qualification_v2_binds_attestation_manifest_separately_from_final_checksums() {
    let (_fixture, inputs, index_bytes, valid, policy) = qualification_setup("release-candidate");
    let parsed = aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
        &serde_json::to_vec(&valid).unwrap(),
    )
    .unwrap();
    assert_ne!(
        parsed.release.pre_attestation_checksums_sha256,
        parsed.release.checksums_sha256
    );
    assert_eq!(
        parsed.attestation.subject_manifest_sha256,
        parsed.release.pre_attestation_checksums_sha256
    );
    parsed
        .validate_against_index(
            &index_bytes,
            &inputs,
            &serde_json::from_value(policy.clone()).unwrap(),
        )
        .unwrap();

    let mut missing_manifest_digest = valid.clone();
    missing_manifest_digest["release"]
        .as_object_mut()
        .unwrap()
        .remove("pre_attestation_checksums_sha256");
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&missing_manifest_digest).unwrap()
        )
        .is_err()
    );

    let mut missing_manifest_claim = valid.clone();
    missing_manifest_claim["attestation"]
        .as_object_mut()
        .unwrap()
        .remove("subject_manifest_sha256");
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&missing_manifest_claim).unwrap()
        )
        .is_err()
    );

    let mut legacy_singular_subject = valid.clone();
    let attestation = legacy_singular_subject["attestation"]
        .as_object_mut()
        .unwrap();
    attestation.remove("subject_manifest_sha256");
    attestation.insert("subject_sha256".to_owned(), json!("c".repeat(64)));
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&legacy_singular_subject).unwrap()
        )
        .is_err()
    );

    let mut mismatched_manifest_claim = valid;
    mismatched_manifest_claim["attestation"]["subject_manifest_sha256"] = json!("e".repeat(64));
    assert_qualification_invalid(&mismatched_manifest_claim, &index_bytes, &inputs, &policy);
}

#[test]
fn qualification_v2_keeps_rv32_and_rv64_gnu_sources_independent() {
    let fixture = split_gnu_fixture();
    let inputs = validated_inputs(&fixture);
    let index_bytes = NativeReleaseIndexV2::parse(
        &serde_json::to_vec(&split_gnu_index_value(&fixture)).unwrap(),
        &inputs,
    )
    .unwrap()
    .to_json_bytes()
    .unwrap();
    let evidence = qualification_evidence_value(&inputs, &index_bytes, "release-candidate");
    let parsed = aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
        &serde_json::to_vec(&evidence).unwrap(),
    )
    .unwrap();
    parsed
        .validate_against_index(
            &index_bytes,
            &inputs,
            &serde_json::from_value(qualification_policy(150)).unwrap(),
        )
        .unwrap();

    for host in HOSTS {
        let rv32 = parsed
            .lanes
            .iter()
            .find(|lane| {
                lane.host == *host
                    && lane.group_id == "gnu-rv32"
                    && lane.target_profile == "rv32-aros"
            })
            .unwrap();
        let rv64 = parsed
            .lanes
            .iter()
            .find(|lane| {
                lane.host == *host
                    && lane.group_id == "gnu-rv64"
                    && lane.target_profile == "rv64-aros"
            })
            .unwrap();
        assert_eq!(rv32.source_commit.as_str(), GNU_RV32_SOURCE_COMMIT);
        assert_eq!(rv64.source_commit.as_str(), GNU_RV64_SOURCE_COMMIT);
        assert_ne!(rv32.source_commit.as_str(), rv64.source_commit.as_str());
    }
}

#[test]
fn qualification_v2_rejects_closed_schema_duplicate_keys_and_repeated_report_digests() {
    let (_fixture, _inputs, _index_bytes, valid, _policy) =
        qualification_setup("release-candidate");
    let encoded = String::from_utf8(serde_json::to_vec(&valid).unwrap()).unwrap();

    let mut unknown = valid.clone();
    unknown["private-evidence-field-marker"] = json!("private-evidence-value-marker");
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&unknown).unwrap()
        )
        .is_err()
    );
    let duplicate_schema = encoded.replacen(
        "\"schema\":\"aros-toolchain-qualification-evidence-v2\"",
        "\"schema\":\"aros-toolchain-qualification-evidence-v2\",\"schema\":\"aros-toolchain-qualification-evidence-v2\"",
        1,
    );
    assert_ne!(duplicate_schema, encoded);
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            duplicate_schema.as_bytes()
        )
        .is_err()
    );

    let duplicate_lane_key = encoded.replacen(
        "\"group_id\":\"gnu-riscv\"",
        "\"group_id\":\"gnu-riscv\",\"group_id\":\"gnu-riscv\"",
        1,
    );
    assert_ne!(duplicate_lane_key, encoded);
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            duplicate_lane_key.as_bytes()
        )
        .is_err()
    );

    let mut wrong_schema = valid.clone();
    wrong_schema["schema"] = json!("aros-toolchain-qualification-evidence-v1");
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&wrong_schema).unwrap()
        )
        .is_err()
    );

    let mut repeated_within_lane = valid.clone();
    repeated_within_lane["lanes"][0]["build_b_report_sha256"] =
        repeated_within_lane["lanes"][0]["build_a_report_sha256"].clone();
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&repeated_within_lane).unwrap()
        )
        .is_err()
    );

    let mut repeated_across_lanes = valid;
    repeated_across_lanes["lanes"][1]["comparison_report_sha256"] =
        repeated_across_lanes["lanes"][0]["comparison_report_sha256"].clone();
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&repeated_across_lanes).unwrap()
        )
        .is_err()
    );
}

#[test]
fn qualification_v2_rejects_each_cross_group_or_package_lane_swap() {
    let (_fixture, inputs, index_bytes, valid, policy) = qualification_setup("release-candidate");
    let lanes = valid["lanes"].as_array().unwrap();
    let base_index = lanes
        .iter()
        .position(|lane| {
            lane["group_id"] == "gnu-riscv"
                && lane["host"] == "linux-aarch64"
                && lane["target_profile"] == "rv32-aros"
        })
        .unwrap();
    let rv64_index = lanes
        .iter()
        .position(|lane| {
            lane["group_id"] == "gnu-riscv"
                && lane["host"] == "linux-aarch64"
                && lane["target_profile"] == "rv64-aros"
        })
        .unwrap();
    let llvm_index = lanes
        .iter()
        .position(|lane| lane["group_id"] == "llvm-pc" && lane["host"] == "linux-aarch64")
        .unwrap();
    let other_host_index = lanes
        .iter()
        .position(|lane| {
            lane["group_id"] == "gnu-riscv"
                && lane["host"] == "linux-x86_64"
                && lane["target_profile"] == "rv32-aros"
        })
        .unwrap();

    let mutations: &[(&str, usize, &str)] = &[
        ("group_id", llvm_index, "group_id"),
        ("compiler", llvm_index, "compiler"),
        ("source_commit", llvm_index, "source_commit"),
        ("target_profile", rv64_index, "target_profile"),
        ("target_triple", llvm_index, "target_triple"),
        ("host", other_host_index, "host"),
        ("recipe_sha256", llvm_index, "recipe_sha256"),
        ("source_lock_sha256", llvm_index, "source_lock_sha256"),
        ("profiles_sha256", llvm_index, "profiles_sha256"),
    ];
    for (_label, source_index, field) in mutations {
        let mut changed = valid.clone();
        changed["lanes"][base_index][*field] = valid["lanes"][*source_index][*field].clone();
        assert_qualification_invalid(&changed, &index_bytes, &inputs, &policy);
    }

    let mut changed_asset = valid.clone();
    changed_asset["lanes"][base_index]["asset"] = json!("a-not-indexed-asset.tar.xz");
    assert_qualification_invalid(&changed_asset, &index_bytes, &inputs, &policy);

    for (field, replacement) in [
        ("archive_sha256", json!("e".repeat(64))),
        ("archive_size", json!(4097)),
        ("tree_sha256", json!("f".repeat(64))),
    ] {
        let mut changed = valid.clone();
        changed["lanes"][base_index][field] = replacement;
        assert_qualification_invalid(&changed, &index_bytes, &inputs, &policy);
    }
}

#[test]
fn qualification_v2_distinguishes_diagnostic_subsets_from_complete_candidates() {
    let (_fixture, inputs, index_bytes, mut valid, policy) =
        qualification_setup("release-candidate");
    valid["lanes"].as_array_mut().unwrap().truncate(1);
    assert_qualification_invalid(&valid, &index_bytes, &inputs, &policy);

    let mut diagnostic = valid.clone();
    diagnostic["coverage"] = json!("diagnostic");
    let parsed = aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
        &serde_json::to_vec(&diagnostic).unwrap(),
    )
    .unwrap();
    parsed
        .validate_against_index(
            &index_bytes,
            &inputs,
            &serde_json::from_value(policy.clone()).unwrap(),
        )
        .unwrap();

    let mut extra = valid.clone();
    let mut extra_lane = extra["lanes"][0].clone();
    extra_lane["group_id"] = json!("extra-group");
    extra_lane["asset"] = json!("zz-extra.tar.xz");
    extra_lane["host"] = json!("extra-host");
    extra_lane["target_profile"] = json!("extra-profile");
    extra_lane["target_triple"] = json!("extra-triple");
    extra_lane["build_a_report_sha256"] = json!(format!("{:064x}", 100));
    extra_lane["build_b_report_sha256"] = json!(format!("{:064x}", 101));
    extra_lane["comparison_report_sha256"] = json!(format!("{:064x}", 102));
    extra_lane["compatibility_report_sha256"] = json!(format!("{:064x}", 103));
    extra["lanes"].as_array_mut().unwrap().push(extra_lane);
    assert_qualification_invalid(&extra, &index_bytes, &inputs, &policy);

    let mut duplicate = qualification_setup("release-candidate").3;
    let first = duplicate["lanes"][0].clone();
    duplicate["lanes"].as_array_mut().unwrap()[1] = first;
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&duplicate).unwrap()
        )
        .is_err()
    );

    let mut unsorted = qualification_setup("release-candidate").3;
    unsorted["lanes"].as_array_mut().unwrap().swap(0, 1);
    assert!(
        aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
            &serde_json::to_vec(&unsorted).unwrap()
        )
        .is_err()
    );
}

#[test]
fn qualification_v2_binds_exact_input_and_index_bytes_policy_and_time_window() {
    let (fixture, inputs, index_bytes, valid, policy) = qualification_setup("release-candidate");
    let parsed = aros_toolchain::qualification_evidence_v2::QualificationEvidenceV2::parse(
        &serde_json::to_vec(&valid).unwrap(),
    )
    .unwrap();

    let mut changed_index_bytes = index_bytes.clone();
    changed_index_bytes.push(b' ');
    assert!(parsed
        .validate_against_index(
            &changed_index_bytes,
            &inputs,
            &serde_json::from_value(policy.clone()).unwrap()
        )
        .is_err());

    let mut changed_collection_bytes = collection_bytes(&fixture);
    changed_collection_bytes.push(b' ');
    let changed_inputs =
        ReleaseInputs::parse(&changed_collection_bytes, &fixture.documents).unwrap();
    assert!(parsed
        .validate_against_index(
            &index_bytes,
            &changed_inputs,
            &serde_json::from_value(policy.clone()).unwrap()
        )
        .is_err());

    for field in [
        "source_repository",
        "source_workflow",
        "signer_repository",
        "signer_workflow",
        "signer",
    ] {
        let mut changed_policy = policy.clone();
        let replacement = if field.ends_with("repository") {
            json!("https://github.com/other/aros-toolchains")
        } else if field.ends_with("workflow") {
            json!(".github/workflows/other.yml")
        } else {
            json!("other-signer")
        };
        changed_policy[field] = replacement;
        assert_qualification_invalid(&valid, &index_bytes, &inputs, &changed_policy);
    }

    let mut future = valid.clone();
    future["created_at"] = json!(151);
    assert_qualification_invalid(&future, &index_bytes, &inputs, &policy);

    let mut expired_policy = policy;
    expired_policy["now"] = json!(300);
    assert_qualification_invalid(&valid, &index_bytes, &inputs, &expired_policy);
}

#[test]
fn llvm_family_v2_two_root_extraction_requires_explicit_format() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let candidate_root = root.join("llvm-v2-candidate");
    fs::create_dir(&candidate_root).unwrap();
    let payload = b"synthetic LLVM family-v2 relocation payload\n";
    fs::write(candidate_root.join("fixture-input"), payload).unwrap();

    let inputs = validated_inputs(&fixture());
    let llvm_group = inputs
        .groups()
        .iter()
        .find(|group| group.id() == "llvm-pc")
        .unwrap();
    let package_request = PackageRequest {
        candidate_root,
        output_dir: root.join("llvm-v2-package"),
        release_id: "release-2026.10".into(),
        host: "macos-aarch64".into(),
        recipe: llvm_group.recipe().clone(),
        source_lock: llvm_group.source_lock().clone(),
        profile: llvm_group.profiles().select("pc-x86_64").unwrap().clone(),
        build_environment: serde_json::Map::new(),
        forbidden_prefixes: Vec::new(),
    };
    let package = package_with_format(&package_request, PackageFormat::CompilerFamilyV2).unwrap();
    let verification = PackageVerificationRequest {
        package_dir: package.output_dir,
        release_id: package_request.release_id,
        host: package_request.host,
        recipe: package_request.recipe,
        source_lock: package_request.source_lock,
        profile: package_request.profile,
        build_environment: package_request.build_environment,
        forbidden_prefixes: package_request.forbidden_prefixes,
    };
    let relocation_request = |first_root, second_root| TwoRootRelocationRequest {
        verification: verification.clone(),
        first_root,
        second_root,
    };

    let default_request = relocation_request(
        root.join("default-legacy-first"),
        root.join("default-legacy-second"),
    );
    let error = extract_two_roots(&default_request).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerVerification
    );
    assert!(!default_request.first_root.exists());
    assert!(!default_request.second_root.exists());

    let wrong_format_request = relocation_request(
        root.join("explicit-legacy-first"),
        root.join("explicit-legacy-second"),
    );
    let error = extract_two_roots_with_format(&wrong_format_request, PackageFormat::LegacyLlvmV1)
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerVerification
    );
    assert!(!wrong_format_request.first_root.exists());
    assert!(!wrong_format_request.second_root.exists());

    let family_v2_request =
        relocation_request(root.join("family-v2-first"), root.join("family-v2-second"));
    let extracted =
        extract_two_roots_with_format(&family_v2_request, PackageFormat::CompilerFamilyV2).unwrap();

    assert_ne!(extracted.first.root, extracted.second.root);
    assert_eq!(
        extracted.first.verified.manifest,
        extracted.second.verified.manifest
    );
    assert_eq!(
        extracted.first.verified.archive_sha256,
        extracted.second.verified.archive_sha256
    );
    assert_eq!(
        extracted.first.verified.archive_size,
        extracted.second.verified.archive_size
    );
    assert_eq!(
        extracted.first.verified.archive_sha256,
        package.archive_sha256
    );
    assert_eq!(extracted.first.verified.archive_size, package.archive_size);
    assert_eq!(extracted.first.verified.manifest.schema, 2);
    for extracted_root in [&extracted.first.root, &extracted.second.root] {
        assert_eq!(
            fs::read(extracted_root.join("fixture-input")).unwrap(),
            payload
        );
    }
}
