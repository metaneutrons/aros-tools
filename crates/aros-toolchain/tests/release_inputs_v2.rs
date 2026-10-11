//! Synthetic compiler-family input fixtures exercise binding only, not release evidence.

use std::collections::BTreeMap;

use aros_common::sdk_manifest::SdkIdentity;
use aros_common::{sha256_bytes, DiagnosticCode};
use aros_toolchain::{canonical, release_inputs::ReleaseInputs, ContractError};
use serde_json::{json, Value};

const GNU_SOURCE_COMMIT: &str = "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";
const LLVM_SOURCE_COMMIT: &str = "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2";
const PRODUCER_COMMIT: &str = "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3";
const TOOLS_COMMIT: &str = "b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4";
const PRODUCER_TREE: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
const TOOLS_TREE: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";

struct Fixture {
    collection: Value,
    documents: BTreeMap<String, Vec<u8>>,
}

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

fn gnu_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": GNU_SOURCE_COMMIT,
        "profiles": [gnu_profile(32), gnu_profile(64)]
    }))
    .unwrap()
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

fn recipe_bytes(
    source_lock: &[u8],
    profiles: &[u8],
    source_commit: &str,
    producer_commit: &str,
    tools_commit: &str,
    producer_tree: &str,
    tools_tree: &str,
) -> Vec<u8> {
    let mut recipe = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": source_commit,
        "source_tree": "7".repeat(40),
        "producer_commit": producer_commit,
        "producer_tree": producer_tree,
        "tools_commit": tools_commit,
        "tools_tree": tools_tree,
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
    let recipe = recipe_bytes(
        &source_lock,
        &profiles,
        source_commit,
        PRODUCER_COMMIT,
        TOOLS_COMMIT,
        PRODUCER_TREE,
        TOOLS_TREE,
    );
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
    let mut fixture = Fixture {
        collection: json!({
            "schema": "aros-toolchain-release-inputs-v2",
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "hosts": ["linux-aarch64", "linux-x86_64", "macos-aarch64"],
            "groups": [gnu, llvm]
        }),
        documents,
    };
    mutate_recipe(&mut fixture, "llvm-pc", |recipe| {
        recipe["source_tree"] = json!("8".repeat(40));
    });
    fixture
}

fn collection_bytes(fixture: &Fixture) -> Vec<u8> {
    serde_json::to_vec(&fixture.collection).unwrap()
}

fn parse_error(fixture: &Fixture) -> ContractError {
    ReleaseInputs::parse(&collection_bytes(fixture), &fixture.documents).unwrap_err()
}

fn assert_parse_rejected(fixture: &Fixture) {
    assert!(ReleaseInputs::parse(&collection_bytes(fixture), &fixture.documents).is_err());
}

fn assert_code(error: &ContractError, expected: DiagnosticCode) {
    assert_eq!(error.diagnostics().diagnostics[0].code, expected);
}

fn assert_message(error: &ContractError, expected: &str) {
    assert!(error.to_string().contains(expected), "{error}");
}

fn group_index(fixture: &Fixture, id: &str) -> usize {
    fixture.collection["groups"]
        .as_array()
        .unwrap()
        .iter()
        .position(|group| group["id"] == id)
        .unwrap()
}

fn mutate_recipe(fixture: &mut Fixture, group_id: &str, mutation: impl FnOnce(&mut Value)) {
    let index = group_index(fixture, group_id);
    let file = fixture.collection["groups"][index]["recipe"]["file"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut recipe: Value = serde_json::from_slice(&fixture.documents[&file]).unwrap();
    mutation(&mut recipe);
    recipe.as_object_mut().unwrap().remove("recipe_sha256");
    let digest = sha256_bytes(&canonical::bytes(&recipe).unwrap());
    recipe["recipe_sha256"] = json!(digest);
    let bytes = serde_json::to_vec(&recipe).unwrap();
    fixture.documents.insert(file, bytes.clone());
    fixture.collection["groups"][index]["recipe"]["sha256"] = json!(sha256_bytes(&bytes));
}

fn mutate_profiles(fixture: &mut Fixture, group_id: &str, mutation: impl FnOnce(&mut Value)) {
    let index = group_index(fixture, group_id);
    let file = fixture.collection["groups"][index]["profiles"]["file"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut profiles: Value = serde_json::from_slice(&fixture.documents[&file]).unwrap();
    mutation(&mut profiles);
    let bytes = serde_json::to_vec(&profiles).unwrap();
    fixture.documents.insert(file, bytes.clone());
    fixture.collection["groups"][index]["profiles"]["sha256"] = json!(sha256_bytes(&bytes));
    let profiles_digest = sha256_bytes(&bytes);
    mutate_recipe(fixture, group_id, |recipe| {
        recipe["profiles_sha256"] = json!(profiles_digest);
    });
}

fn mutate_source_lock(fixture: &mut Fixture, group_id: &str, mutation: impl FnOnce(&mut Value)) {
    let index = group_index(fixture, group_id);
    let file = fixture.collection["groups"][index]["source_lock"]["file"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut source_lock: Value = serde_json::from_slice(&fixture.documents[&file]).unwrap();
    mutation(&mut source_lock);
    let bytes = serde_json::to_vec(&source_lock).unwrap();
    fixture.documents.insert(file, bytes.clone());
    fixture.collection["groups"][index]["source_lock"]["sha256"] = json!(sha256_bytes(&bytes));
    let lock_digest = sha256_bytes(&bytes);
    mutate_recipe(fixture, group_id, |recipe| {
        recipe["source_lock_sha256"] = json!(lock_digest);
    });
}

fn sdk_identity_value(sdk_id: &str) -> Value {
    json!({
        "sdk_id": sdk_id,
        "version": "1.2.3",
        "target": {
            "target_triple": "i686-aros",
            "abi": {
                "data_model": "ilp32",
                "pointer_width": 32
            }
        },
        "source": {
            "commit": "9".repeat(40),
            "git_tree": "8".repeat(40),
            "content_sha256": "a".repeat(64),
            "contract_sha256": "b".repeat(64)
        },
        "source_date_epoch": 946_684_800
    })
}

fn add_sdk(
    fixture: &mut Fixture,
    id: &str,
    identity_file: &str,
    source_preset: &str,
    compiler_profiles: Value,
) -> Value {
    let identity = serde_json::to_vec(&sdk_identity_value(id)).unwrap();
    fixture
        .documents
        .insert(identity_file.to_owned(), identity.clone());
    let mut selection = json!({
        "id": id,
        "identity": reference(identity_file, &identity),
        "source_preset": source_preset
    });
    selection
        .as_object_mut()
        .unwrap()
        .insert("compiler_profiles".to_owned(), compiler_profiles);
    selection
}

fn sdk_fixture() -> Fixture {
    let mut fixture = fixture();
    let sdk = add_sdk(
        &mut fixture,
        "core-sdk",
        "core-sdk-identity.json",
        "aros-core",
        json!([
            {"group_id": "gnu-riscv", "profile": "rv32-aros"},
            {"group_id": "llvm-pc", "profile": "pc-x86_64"}
        ]),
    );
    fixture.collection["sdks"] = json!([sdk]);
    fixture
}

fn sdk_identity_file(fixture: &Fixture, index: usize) -> String {
    fixture.collection["sdks"][index]["identity"]["file"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn replace_sdk_identity_bytes(fixture: &mut Fixture, index: usize, bytes: Vec<u8>) {
    let file = sdk_identity_file(fixture, index);
    let digest = sha256_bytes(&bytes);
    fixture.documents.insert(file, bytes);
    fixture.collection["sdks"][index]["identity"]["sha256"] = json!(digest);
}

fn mutate_sdk_identity(fixture: &mut Fixture, index: usize, mutation: impl FnOnce(&mut Value)) {
    let file = sdk_identity_file(fixture, index);
    let mut identity: Value = serde_json::from_slice(&fixture.documents[&file]).unwrap();
    mutation(&mut identity);
    replace_sdk_identity_bytes(fixture, index, serde_json::to_vec(&identity).unwrap());
}

fn rename_sdk_identity_file(fixture: &mut Fixture, index: usize, new_file: &str) {
    let old_file = sdk_identity_file(fixture, index);
    let bytes = fixture.documents.remove(&old_file).unwrap();
    fixture.documents.insert(new_file.to_owned(), bytes.clone());
    fixture.collection["sdks"][index]["identity"]["file"] = json!(new_file);
    fixture.collection["sdks"][index]["identity"]["sha256"] = json!(sha256_bytes(&bytes));
}

#[test]
fn binds_independent_source_commits_and_derives_every_active_lane() {
    let fixture = fixture();
    let collection = collection_bytes(&fixture);
    let validated = ReleaseInputs::parse(&collection, &fixture.documents).unwrap();

    assert_eq!(validated.collection_sha256(), &sha256_bytes(&collection));
    assert_eq!(validated.groups().len(), 2);
    assert_eq!(validated.groups()[0].profiles().entries().len(), 2);
    assert_eq!(
        validated.groups()[0].recipe_bytes(),
        fixture.documents["gnu-riscv-recipe.json"].as_slice()
    );
    assert_ne!(
        validated.groups()[0].recipe().source().0.as_str(),
        validated.groups()[1].recipe().source().0.as_str()
    );
    assert_ne!(
        validated.groups()[0].recipe().source().1.as_str(),
        validated.groups()[1].recipe().source().1.as_str()
    );
    assert_eq!(validated.expected_lanes().len(), 9);

    let actual = validated
        .expected_lanes()
        .iter()
        .map(|lane| (lane.group_id(), lane.profile(), lane.host()))
        .collect::<std::collections::BTreeSet<_>>();
    let expected = ["rv32-aros", "rv64-aros"]
        .into_iter()
        .map(|profile| ("gnu-riscv", profile))
        .chain(std::iter::once(("llvm-pc", "pc-x86_64")))
        .flat_map(|(group, profile)| {
            ["linux-aarch64", "linux-x86_64", "macos-aarch64"]
                .into_iter()
                .map(move |host| (group, profile, host))
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn accepts_one_source_commit_and_tree_shared_across_groups() {
    let mut fixture = fixture();
    mutate_profiles(&mut fixture, "llvm-pc", |profiles| {
        profiles["upstream_commit"] = json!(GNU_SOURCE_COMMIT);
    });
    mutate_recipe(&mut fixture, "llvm-pc", |recipe| {
        recipe["source_commit"] = json!(GNU_SOURCE_COMMIT);
        recipe["source_tree"] = json!("7".repeat(40));
    });

    let validated = ReleaseInputs::parse(&collection_bytes(&fixture), &fixture.documents).unwrap();
    assert_eq!(
        validated.groups()[0].recipe().source(),
        validated.groups()[1].recipe().source()
    );
    assert_eq!(validated.expected_lanes().len(), 9);
}

#[test]
fn rejects_different_source_trees_for_the_same_source_commit() {
    let mut fixture = fixture();
    mutate_profiles(&mut fixture, "llvm-pc", |profiles| {
        profiles["upstream_commit"] = json!(GNU_SOURCE_COMMIT);
    });
    mutate_recipe(&mut fixture, "llvm-pc", |recipe| {
        recipe["source_commit"] = json!(GNU_SOURCE_COMMIT);
        recipe["source_tree"] = json!("9".repeat(40));
    });

    let error = parse_error(&fixture);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "source tree differs for a repeated source commit");
}

#[test]
fn rejects_missing_and_extra_document_map_entries_before_parsing() {
    let mut missing = fixture();
    missing.documents.remove("gnu-riscv-profiles.json");
    let error = parse_error(&missing);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "exactly the referenced filenames");

    let mut extra = fixture();
    extra
        .documents
        .insert("private-extra-marker.json".into(), b"not-json".to_vec());
    let error = parse_error(&extra);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "exactly the referenced filenames");
    assert!(!error.to_string().contains("private-extra-marker"));
}

#[test]
fn rejects_raw_document_hash_mismatch_without_parsing_tampered_bytes() {
    let mut fixture = fixture();
    fixture
        .documents
        .get_mut("gnu-riscv-source-lock.json")
        .unwrap()
        .push(b' ');
    let error = parse_error(&fixture);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "do not match their declared SHA-256");
}

#[test]
fn binds_recipe_commits_and_shared_producer_and_tools_trees() {
    let mut producer = fixture();
    mutate_recipe(&mut producer, "gnu-riscv", |recipe| {
        recipe["producer_commit"] = json!("9".repeat(40));
    });
    let error = parse_error(&producer);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "producer commit differs");

    let mut tools = fixture();
    mutate_recipe(&mut tools, "gnu-riscv", |recipe| {
        recipe["tools_commit"] = json!("8".repeat(40));
    });
    let error = parse_error(&tools);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "tools commit differs");

    let mut producer_tree = fixture();
    mutate_recipe(&mut producer_tree, "llvm-pc", |recipe| {
        recipe["producer_tree"] = json!("9".repeat(40));
    });
    let error = parse_error(&producer_tree);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "producer tree differs across");

    let mut tools_tree = fixture();
    mutate_recipe(&mut tools_tree, "llvm-pc", |recipe| {
        recipe["tools_tree"] = json!("8".repeat(40));
    });
    let error = parse_error(&tools_tree);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "tools tree differs across");
}

#[test]
fn binds_source_commit_profiles_family_recipe_digests_and_patch_closure() {
    let mut source_mismatch = fixture();
    mutate_profiles(&mut source_mismatch, "gnu-riscv", |profiles| {
        profiles["upstream_commit"] = json!("9".repeat(40));
    });
    let error = parse_error(&source_mismatch);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(
        &error,
        "source commit differs from profiles upstream commit",
    );

    let mut empty_profiles = fixture();
    mutate_profiles(&mut empty_profiles, "gnu-riscv", |profiles| {
        profiles["profiles"] = json!([]);
    });
    let error = parse_error(&empty_profiles);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "1..128 entries");

    let mut lock_digest_mismatch = fixture();
    mutate_recipe(&mut lock_digest_mismatch, "gnu-riscv", |recipe| {
        recipe["source_lock_sha256"] = json!("9".repeat(64));
    });
    let error = parse_error(&lock_digest_mismatch);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "exact source lock, profiles and patch closure");

    let mut patch_mismatch = fixture();
    mutate_source_lock(&mut patch_mismatch, "gnu-riscv", |lock| {
        lock["sources"][0]["patch"] = json!("tools/crosstools/gnu/gcc-aros.diff");
    });
    let error = parse_error(&patch_mismatch);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "exact source lock, profiles and patch closure");

    let mut profiles_digest_mismatch = fixture();
    mutate_recipe(&mut profiles_digest_mismatch, "gnu-riscv", |recipe| {
        recipe["profiles_sha256"] = json!("8".repeat(64));
    });
    let error = parse_error(&profiles_digest_mismatch);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "exact source lock, profiles and patch closure");

    let mut mixed_family = fixture();
    let llvm_lock: Value =
        serde_json::from_slice(&mixed_family.documents["llvm-pc-source-lock.json"]).unwrap();
    mutate_source_lock(&mut mixed_family, "gnu-riscv", |lock| *lock = llvm_lock);
    let error = parse_error(&mixed_family);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "different compiler families");

    let mut duplicate_profile = fixture();
    mutate_profiles(&mut duplicate_profile, "llvm-pc", |profiles| {
        profiles["profiles"][0]["name"] = json!("rv32-aros");
    });
    let error = parse_error(&duplicate_profile);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "globally unique");
}

#[test]
fn rejects_duplicate_json_keys_and_unknown_closed_schema_fields() {
    let valid_fixture = fixture();
    let collection = String::from_utf8(collection_bytes(&valid_fixture)).unwrap();
    let duplicate = collection.replacen(
        &format!("\"producer_commit\":\"{PRODUCER_COMMIT}\""),
        &format!(
            "\"producer_commit\":\"{PRODUCER_COMMIT}\",\"producer_commit\":\"{PRODUCER_COMMIT}\""
        ),
        1,
    );
    let error = ReleaseInputs::parse(duplicate.as_bytes(), &valid_fixture.documents).unwrap_err();
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains(PRODUCER_COMMIT));

    let duplicate_reference = collection.replacen(
        "\"file\":\"gnu-riscv-recipe.json\"",
        "\"file\":\"gnu-riscv-recipe.json\",\"file\":\"gnu-riscv-recipe.json\"",
        1,
    );
    let error =
        ReleaseInputs::parse(duplicate_reference.as_bytes(), &valid_fixture.documents).unwrap_err();
    assert_code(&error, DiagnosticCode::ProducerContract);

    let mut unknown = valid_fixture;
    unknown.collection["untrusted-field-marker"] = json!("secret-value-marker");
    let error = parse_error(&unknown);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains("untrusted-field-marker"));
    assert!(!error.to_string().contains("secret-value-marker"));

    let mut unknown_reference = fixture();
    unknown_reference.collection["groups"][0]["recipe"]["private-field-marker"] =
        json!("secret-value-marker");
    let error = parse_error(&unknown_reference);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains("private-field-marker"));
    assert!(!error.to_string().contains("secret-value-marker"));
}

#[test]
fn rejects_unsafe_reserved_and_colliding_document_filenames() {
    for file in [
        "../private.json",
        "folder/input.json",
        "folder\\input.json",
        ".hidden.json",
        "json",
        "Recipe.json",
        "recipe.JSON",
        "toolchain-index-v1.json",
        "toolchain-release-inputs-v2.json",
        "toolchain-manifest-v2.schema.json",
        "toolchain-provenance.sigstore.json",
    ] {
        let mut fixture = fixture();
        fixture.collection["groups"][0]["recipe"]["file"] = json!(file);
        let error = parse_error(&fixture);
        assert_code(&error, DiagnosticCode::ProducerContract);
        assert_message(&error, "unsafe or reserved document filename");
        assert!(!error.to_string().contains(file));
    }

    let mut too_long = fixture();
    let file = format!("{}.json", "x".repeat(124));
    too_long.collection["groups"][0]["recipe"]["file"] = json!(file);
    let error = parse_error(&too_long);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "unsafe or reserved document filename");

    let mut collision = fixture();
    collision.collection["groups"][0]["profiles"]["file"] =
        collision.collection["groups"][0]["recipe"]["file"].clone();
    let error = parse_error(&collision);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "globally unique");
}

#[test]
fn rejects_bad_group_ids_and_duplicate_or_unsorted_groups() {
    let mut bad_id = fixture();
    bad_id.collection["groups"][0]["id"] = json!("Upper-case");
    let error = parse_error(&bad_id);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "sorted, unique lower-case kebab IDs");

    let mut duplicate = fixture();
    duplicate.collection["groups"][1]["id"] = json!("gnu-riscv");
    let error = parse_error(&duplicate);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "sorted, unique lower-case kebab IDs");

    let mut unsorted = fixture();
    let groups = unsorted.collection["groups"].as_array_mut().unwrap();
    groups.swap(0, 1);
    let error = parse_error(&unsorted);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "sorted, unique lower-case kebab IDs");
}

#[test]
fn requires_the_exact_sorted_active_host_set() {
    for hosts in [
        json!(["linux-aarch64", "macos-aarch64"]),
        json!([
            "linux-aarch64",
            "linux-x86_64",
            "macos-aarch64",
            "macos-x86_64"
        ]),
        json!(["linux-x86_64", "linux-aarch64", "macos-aarch64"]),
        json!(["linux-aarch64", "linux-x86_64", "macos-x86_64"]),
    ] {
        let mut fixture = fixture();
        fixture.collection["hosts"] = hosts;
        let error = parse_error(&fixture);
        assert_code(&error, DiagnosticCode::ProducerContract);
        assert_message(&error, "sorted active host set");
    }
}

#[test]
fn enforces_collection_group_and_document_size_bounds() {
    let valid_fixture = fixture();
    let bytes = vec![b' '; 1024 * 1024 + 1];
    let error = ReleaseInputs::parse(&bytes, &valid_fixture.documents).unwrap_err();
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "collection exceeds 1 MiB");

    let mut no_groups = fixture();
    no_groups.collection["groups"] = json!([]);
    let error = parse_error(&no_groups);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "requires 1..32 input groups");

    let mut too_many_groups = fixture();
    let group = too_many_groups.collection["groups"][0].clone();
    too_many_groups.collection["groups"] = Value::Array(vec![group; 33]);
    let error = parse_error(&too_many_groups);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "requires 1..32 input groups");

    let mut oversized_document = fixture();
    oversized_document
        .documents
        .get_mut("gnu-riscv-recipe.json")
        .unwrap()
        .resize(1024 * 1024 + 1, b' ');
    let error = parse_error(&oversized_document);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "referenced release input document exceeds 1 MiB");

    let mut oversized_id = fixture();
    oversized_id.collection["groups"][0]["id"] = json!(format!("{}a", "g".repeat(64)));
    let error = parse_error(&oversized_id);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "sorted, unique lower-case kebab IDs");
}

#[test]
fn rejects_noncanonical_reference_digests_and_git_object_ids() {
    let mut bad_digest = fixture();
    let uppercase_digest = "A".repeat(64);
    bad_digest.collection["groups"][0]["recipe"]["sha256"] = json!(uppercase_digest);
    let error = parse_error(&bad_digest);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains(uppercase_digest.as_str()));

    let mut bad_commit = fixture();
    let uppercase_commit = "A".repeat(40);
    bad_commit.collection["producer_commit"] = json!(uppercase_commit);
    let error = parse_error(&bad_commit);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains(uppercase_commit.as_str()));
}

#[test]
fn optionally_binds_closed_sdk_identity_bytes_without_changing_compiler_lanes() {
    let compiler_only = fixture();
    let compiler_inputs =
        ReleaseInputs::parse(&collection_bytes(&compiler_only), &compiler_only.documents).unwrap();
    assert!(compiler_inputs.sdks().is_empty());
    assert_eq!(compiler_inputs.expected_lanes().len(), 9);

    let fixture = sdk_fixture();
    let identity_file = sdk_identity_file(&fixture, 0);
    let identity_bytes = fixture.documents[&identity_file].as_slice();
    let expected_identity: SdkIdentity = serde_json::from_slice(identity_bytes).unwrap();
    let inputs = ReleaseInputs::parse(&collection_bytes(&fixture), &fixture.documents).unwrap();

    assert_eq!(inputs.sdks().len(), 1);
    assert_eq!(
        inputs.expected_lanes().len(),
        compiler_inputs.expected_lanes().len()
    );
    let sdk = &inputs.sdks()[0];
    assert_eq!(sdk.id(), "core-sdk");
    assert_eq!(sdk.identity_reference().file(), "core-sdk-identity.json");
    assert_eq!(
        sdk.identity_reference().sha256(),
        &sha256_bytes(identity_bytes)
    );
    assert_eq!(sdk.identity(), &expected_identity);
    assert_eq!(sdk.identity_bytes(), identity_bytes);
    assert_eq!(sdk.source_preset(), "aros-core");
    assert_eq!(sdk.compiler_profiles().len(), 2);
    assert_eq!(
        (
            sdk.compiler_profiles()[0].group_id(),
            sdk.compiler_profiles()[0].profile()
        ),
        ("gnu-riscv", "rv32-aros")
    );
    assert_eq!(
        (
            sdk.compiler_profiles()[1].group_id(),
            sdk.compiler_profiles()[1].profile()
        ),
        ("llvm-pc", "pc-x86_64")
    );

    let gnu = inputs
        .groups()
        .iter()
        .find(|group| group.id() == "gnu-riscv")
        .unwrap();
    let rv32 = gnu
        .profiles()
        .entries()
        .iter()
        .find(|profile| profile.name() == "rv32-aros")
        .unwrap();
    assert_ne!(sdk.identity().version, gnu.source_lock().version());
    assert_ne!(
        sdk.identity().source.commit,
        gnu.recipe().source().0.as_str()
    );
    assert_ne!(sdk.identity().target.target_triple, rv32.target_triple());
}

#[test]
fn sdk_identity_documents_must_be_exactly_selected_and_raw_hash_bound() {
    let mut missing = sdk_fixture();
    let identity_file = sdk_identity_file(&missing, 0);
    missing.documents.remove(&identity_file);
    assert!(parse_error(&missing)
        .to_string()
        .contains("exactly the referenced filenames"));

    let mut extra = sdk_fixture();
    extra
        .documents
        .insert("unselected-identity.json".into(), b"{}".to_vec());
    assert!(parse_error(&extra)
        .to_string()
        .contains("exactly the referenced filenames"));

    let mut changed = sdk_fixture();
    let identity_file = sdk_identity_file(&changed, 0);
    changed
        .documents
        .get_mut(&identity_file)
        .unwrap()
        .push(b' ');
    let error = parse_error(&changed);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "do not match their declared SHA-256");

    let mut unselected = sdk_fixture();
    unselected.collection["sdks"][0]["id"] = json!("different-sdk");
    assert_parse_rejected(&unselected);
}

#[test]
fn sdk_ids_and_identity_filenames_are_safe_unique_and_case_distinct() {
    let mut unsafe_id = sdk_fixture();
    unsafe_id.collection["sdks"][0]["id"] = json!("Core-SDK");
    assert_parse_rejected(&unsafe_id);

    let mut duplicate_id = fixture();
    let first = add_sdk(
        &mut duplicate_id,
        "core-sdk",
        "core-sdk-identity-a.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut duplicate_id,
        "core-sdk",
        "core-sdk-identity-b.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    duplicate_id.collection["sdks"] = json!([first, second]);
    assert_parse_rejected(&duplicate_id);

    let mut unsorted_ids = fixture();
    let later = add_sdk(
        &mut unsorted_ids,
        "sdk-z",
        "sdk-z-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let earlier = add_sdk(
        &mut unsorted_ids,
        "sdk-a",
        "sdk-a-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    unsorted_ids.collection["sdks"] = json!([later, earlier]);
    assert_parse_rejected(&unsorted_ids);

    let mut unsafe_file = sdk_fixture();
    rename_sdk_identity_file(&mut unsafe_file, 0, "../identity.json");
    assert_parse_rejected(&unsafe_file);

    let mut duplicate_file = sdk_fixture();
    rename_sdk_identity_file(&mut duplicate_file, 0, "gnu-riscv-recipe.json");
    assert_parse_rejected(&duplicate_file);

    let mut case_collision = sdk_fixture();
    rename_sdk_identity_file(&mut case_collision, 0, "Gnu-riscv-recipe.json");
    assert_parse_rejected(&case_collision);

    let mut duplicate_sdk_file = fixture();
    let first = add_sdk(
        &mut duplicate_sdk_file,
        "core-sdk",
        "shared-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut duplicate_sdk_file,
        "other-sdk",
        "shared-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    duplicate_sdk_file.collection["sdks"] = json!([first, second]);
    assert_parse_rejected(&duplicate_sdk_file);
}

#[test]
fn sdk_selection_fields_are_closed_and_sdk_identity_is_semantically_valid() {
    let mut unknown_selection = sdk_fixture();
    unknown_selection.collection["sdks"][0]["private-field-marker"] = json!("private-value-marker");
    let error = parse_error(&unknown_selection);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains("private-field-marker"));
    assert!(!error.to_string().contains("private-value-marker"));

    let mut unknown_identity = sdk_fixture();
    let identity_file = sdk_identity_file(&unknown_identity, 0);
    let mut identity: Value =
        serde_json::from_slice(&unknown_identity.documents[&identity_file]).unwrap();
    identity["untrusted-field-marker"] = json!("secret-value-marker");
    replace_sdk_identity_bytes(
        &mut unknown_identity,
        0,
        serde_json::to_vec(&identity).unwrap(),
    );
    let error = parse_error(&unknown_identity);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert!(!error.to_string().contains("untrusted-field-marker"));
    assert!(!error.to_string().contains("secret-value-marker"));

    let mut duplicate_identity_key = sdk_fixture();
    let identity_file = sdk_identity_file(&duplicate_identity_key, 0);
    let identity =
        String::from_utf8(duplicate_identity_key.documents[&identity_file].clone()).unwrap();
    let duplicate = identity.replacen(
        "\"sdk_id\":\"core-sdk\"",
        "\"sdk_id\":\"core-sdk\",\"sdk_id\":\"core-sdk\"",
        1,
    );
    assert_ne!(duplicate, identity);
    replace_sdk_identity_bytes(&mut duplicate_identity_key, 0, duplicate.into_bytes());
    assert_code(
        &parse_error(&duplicate_identity_key),
        DiagnosticCode::ProducerContract,
    );

    let duplicate_selection_key = sdk_fixture();
    let collection = String::from_utf8(collection_bytes(&duplicate_selection_key)).unwrap();
    let duplicate = collection.replacen(
        "\"id\":\"core-sdk\"",
        "\"id\":\"core-sdk\",\"id\":\"core-sdk\"",
        1,
    );
    assert_ne!(duplicate, collection);
    assert_code(
        &ReleaseInputs::parse(duplicate.as_bytes(), &duplicate_selection_key.documents)
            .unwrap_err(),
        DiagnosticCode::ProducerContract,
    );

    let mut invalid_identity = sdk_fixture();
    let identity_file = sdk_identity_file(&invalid_identity, 0);
    let mut identity: Value =
        serde_json::from_slice(&invalid_identity.documents[&identity_file]).unwrap();
    identity["version"] = json!("not-semver");
    replace_sdk_identity_bytes(
        &mut invalid_identity,
        0,
        serde_json::to_vec(&identity).unwrap(),
    );
    assert_parse_rejected(&invalid_identity);

    let mut invalid_source_preset = sdk_fixture();
    invalid_source_preset.collection["sdks"][0]["source_preset"] = json!("../unsafe");
    assert_parse_rejected(&invalid_source_preset);
}

#[test]
fn sdk_compiler_profile_mappings_must_reference_sorted_unique_bound_profiles() {
    let mut unknown_group = sdk_fixture();
    unknown_group.collection["sdks"][0]["compiler_profiles"][0]["group_id"] =
        json!("missing-group");
    assert_parse_rejected(&unknown_group);

    let mut unknown_profile = sdk_fixture();
    unknown_profile.collection["sdks"][0]["compiler_profiles"][0]["profile"] =
        json!("missing-profile");
    assert_parse_rejected(&unknown_profile);

    let mut unsorted = sdk_fixture();
    unsorted.collection["sdks"][0]["compiler_profiles"] = json!([
        {"group_id": "llvm-pc", "profile": "pc-x86_64"},
        {"group_id": "gnu-riscv", "profile": "rv32-aros"}
    ]);
    assert_parse_rejected(&unsorted);

    let mut duplicate = sdk_fixture();
    duplicate.collection["sdks"][0]["compiler_profiles"] = json!([
        {"group_id": "gnu-riscv", "profile": "rv32-aros"},
        {"group_id": "gnu-riscv", "profile": "rv32-aros"}
    ]);
    assert_parse_rejected(&duplicate);
}

#[test]
fn bounds_the_number_of_sdk_selections() {
    let mut fixture = fixture();
    let mut sdks = Vec::new();
    for index in 0..33 {
        let id = format!("sdk-{index:02}");
        let identity_file = format!("{id}-identity.json");
        sdks.push(add_sdk(
            &mut fixture,
            &id,
            &identity_file,
            "aros-core",
            json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
        ));
    }
    fixture.collection["sdks"] = Value::Array(sdks);
    assert_parse_rejected(&fixture);
}

#[test]
fn rejects_distinct_sdk_products_that_derive_the_same_archive_name() {
    let mut fixture = fixture();
    let first = add_sdk(
        &mut fixture,
        "a",
        "a-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut fixture,
        "a-1.0.0",
        "a-versioned-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    fixture.collection["sdks"] = json!([first, second]);
    mutate_sdk_identity(&mut fixture, 0, |identity| {
        identity["version"] = json!("1.0.0-1.0.0");
    });
    mutate_sdk_identity(&mut fixture, 1, |identity| {
        identity["version"] = json!("1.0.0");
    });

    let error = parse_error(&fixture);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(
        &error,
        "SDK selections derive colliding canonical asset names",
    );
}

#[test]
fn rejects_empty_or_excessive_sdk_compiler_profile_references_before_binding() {
    let mut empty = sdk_fixture();
    empty.collection["sdks"][0]["compiler_profiles"] = json!([]);
    let error = parse_error(&empty);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "requires 1..64 explicit compiler profiles");

    let mut excessive = sdk_fixture();
    excessive.collection["sdks"][0]["compiler_profiles"] = Value::Array(vec![
        json!({"group_id": "unbound-group", "profile": "unbound-profile"});
        65
    ]);
    let error = parse_error(&excessive);
    assert_code(&error, DiagnosticCode::ProducerContract);
    assert_message(&error, "requires 1..64 explicit compiler profiles");
}

#[test]
fn rejects_contradictory_source_identities_for_repeated_sdk_commits() {
    let mut different_trees = fixture();
    let first = add_sdk(
        &mut different_trees,
        "core-sdk",
        "core-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut different_trees,
        "other-sdk",
        "other-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    different_trees.collection["sdks"] = json!([first, second]);
    mutate_sdk_identity(&mut different_trees, 0, |identity| {
        identity["source"]["commit"] = json!("6".repeat(40));
        identity["source"]["git_tree"] = json!("7".repeat(40));
    });
    mutate_sdk_identity(&mut different_trees, 1, |identity| {
        identity["source"]["commit"] = json!("6".repeat(40));
        identity["source"]["git_tree"] = json!("8".repeat(40));
    });
    let error = parse_error(&different_trees);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "SDK source Git tree contradicts another selection");

    let mut different_contents = fixture();
    let first = add_sdk(
        &mut different_contents,
        "core-sdk",
        "core-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut different_contents,
        "other-sdk",
        "other-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    different_contents.collection["sdks"] = json!([first, second]);
    mutate_sdk_identity(&mut different_contents, 0, |identity| {
        identity["source"]["commit"] = json!("6".repeat(40));
        identity["source"]["git_tree"] = json!("7".repeat(40));
        identity["source"]["content_sha256"] = json!("a".repeat(64));
    });
    mutate_sdk_identity(&mut different_contents, 1, |identity| {
        identity["source"]["commit"] = json!("6".repeat(40));
        identity["source"]["git_tree"] = json!("7".repeat(40));
        identity["source"]["content_sha256"] = json!("c".repeat(64));
    });
    let error = parse_error(&different_contents);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "SDK source content contradicts another selection");
}

#[test]
fn binds_one_projection_for_the_same_sdk_git_tree_across_distinct_commits() {
    let mut fixture = fixture();
    let first = add_sdk(
        &mut fixture,
        "core-sdk",
        "core-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    let second = add_sdk(
        &mut fixture,
        "other-sdk",
        "other-sdk-identity.json",
        "aros-core",
        json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
    );
    fixture.collection["sdks"] = json!([first, second]);
    mutate_sdk_identity(&mut fixture, 1, |identity| {
        identity["source"]["commit"] = json!("6".repeat(40));
    });
    // Different commits may legitimately select the same immutable tree.
    let valid = ReleaseInputs::parse(&collection_bytes(&fixture), &fixture.documents).unwrap();
    assert_eq!(valid.sdks().len(), 2);
    assert_ne!(
        valid.sdks()[0].identity().source.commit,
        valid.sdks()[1].identity().source.commit
    );
    assert_eq!(
        valid.sdks()[0].identity().source.git_tree,
        valid.sdks()[1].identity().source.git_tree
    );
    mutate_sdk_identity(&mut fixture, 1, |identity| {
        identity["source"]["content_sha256"] = json!("c".repeat(64));
    });
    let error = parse_error(&fixture);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(
        &error,
        "SDK source content contradicts another selection of the same Git tree",
    );
}

#[test]
fn rejects_an_sdk_tree_that_contradicts_a_bound_compiler_recipe_for_the_same_commit() {
    let mut fixture = sdk_fixture();
    mutate_sdk_identity(&mut fixture, 0, |identity| {
        identity["source"]["commit"] = json!(GNU_SOURCE_COMMIT);
        identity["source"]["git_tree"] = json!("9".repeat(40));
    });

    let error = parse_error(&fixture);
    assert_code(&error, DiagnosticCode::ProducerIdentity);
    assert_message(&error, "SDK source Git tree contradicts another selection");
}

#[test]
fn sdk_selection_ids_preserve_the_shared_sdk_product_id_grammar() {
    for id in ["sdk_v2", "sdk.v2+abi", &"a".repeat(96)] {
        let mut fixture = fixture();
        let sdk = add_sdk(
            &mut fixture,
            id,
            "sdk-identity.json",
            "aros-core",
            json!([{"group_id": "gnu-riscv", "profile": "rv32-aros"}]),
        );
        fixture.collection["sdks"] = json!([sdk]);
        let inputs = ReleaseInputs::parse(&collection_bytes(&fixture), &fixture.documents).unwrap();
        assert_eq!(inputs.sdks()[0].id(), id);
    }
}

#[test]
fn sdk_selections_cannot_parse_an_incomplete_release_index() {
    let fixture = sdk_fixture();
    let inputs = ReleaseInputs::parse(&collection_bytes(&fixture), &fixture.documents).unwrap();
    let error =
        aros_toolchain::release_index_v2::NativeReleaseIndexV2::parse(b"{}", &inputs).unwrap_err();
    assert!(error.to_string().contains("closed JSON document"));
}

#[test]
fn filesystem_loading_binds_the_selected_sdk_identity_document() {
    let fixture = sdk_fixture();
    let temporary = tempfile::tempdir().unwrap();
    // macOS exposes its temporary directory through /var, a symlink. The
    // production loader intentionally admits only real absolute ancestors.
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("toolchain-release-inputs-v2.json"),
        collection_bytes(&fixture),
    )
    .unwrap();
    for (name, bytes) in &fixture.documents {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    let inputs = ReleaseInputs::load(&root).unwrap();
    assert_eq!(
        inputs.sdks()[0].identity_bytes(),
        fixture.documents["core-sdk-identity.json"]
    );
    std::fs::write(root.join("core-sdk-identity.json"), b"{}").unwrap();
    assert!(ReleaseInputs::load(&root).is_err());
}
