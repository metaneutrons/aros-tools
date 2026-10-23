//! Mesa policy derivation and SDK-policy readback tests.
//!
//! The synthetic source is only a clean, pinned input fixture; these tests do
//! not run a compiler or authenticate where the selected policy came from.

use super::{
    inspect_native_source_policy, readback_sdk_mesa_policy, NativeConsumerPolicy,
    SourcePolicyRequest,
};
use crate::compatibility_source::CommittedSourceIdentity;
use crate::recipe::GitObjectId;
use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sha256_bytes;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SOURCE_PRESET: &str = "mesa-policy-fixture";
const COMPILER_PROFILE: &str = "llvm-x86_64-fixture";
const SDK_CONTRACT_PATH: &str = "contracts/sdk-consumer.json";
const C_PROBE: &[u8] = b"int sdk_c_probe(void) { return 0; }\n";
const CXX_PROBE: &[u8] = b"int sdk_cxx_probe() { return 0; }\n";

struct SourceFixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    identity: CommittedSourceIdentity,
}

impl SourceFixture {
    fn new(mesa_version: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("source");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);

        let profile = format!(
            "[[targets]]\nname = '{SOURCE_PRESET}'\ntoolchain_profile = '{COMPILER_PROFILE}'\narch = 'x86_64'\nplatform = 'pc'\nbsp = 'generic'\nfloat_abi = 'fixture-float'\nsdk_consumer_contract = '{SDK_CONTRACT_PATH}'\n[targets.transpiler]\nfamily = 'fixture-family'\nvariant = 'fixture-variant'\ntoolchain = 'llvm'\ncpu32 = 'i686'\nuse_mmu = true\nmesa_version = '{mesa_version}'\n[targets.bootstrap_abi]\nflavour = 'native'\nplatform_smp = true\n"
        );
        fs::write(root.join("aros-targets.toml"), profile.as_bytes()).unwrap();
        fs::write(root.join("policy.json"), b"{}\n").unwrap();
        fs::write(root.join("probe.c"), C_PROBE).unwrap();
        fs::write(root.join("probe.cpp"), CXX_PROBE).unwrap();
        fs::create_dir_all(root.join("contracts")).unwrap();

        let contract = json!({
            "schema": "aros-sdk-consumer-contract-v1",
            "profile": SOURCE_PRESET,
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes", "linklibs"],
            "inputs": [
                {"path": "aros-targets.toml", "sha256": sha256_bytes(profile.as_bytes())},
                {"path": "policy.json", "sha256": sha256_bytes(b"{}\n")},
                {"path": "probe.c", "sha256": sha256_bytes(C_PROBE)},
                {"path": "probe.cpp", "sha256": sha256_bytes(CXX_PROBE)}
            ],
            "target": {
                "target_triple": "x86_64-unknown-aros",
                "abi": {
                    "source_cpu": "x86_64",
                    "target_triple": "x86_64-unknown-aros",
                    "pointer_width": 64,
                    "float_abi": "fixture-float",
                    "use_mmu": true,
                    "isa": "fixture-source-isa",
                    "flavour": "native",
                    "platform_smp": true
                }
            },
            "metamake_projection": "policy.json",
            "native_sdk_link_probes": {
                "c": {"source": "probe.c", "libraries": []},
                "cxx": {"source": "probe.cpp", "libraries": []}
            }
        });
        fs::write(
            root.join(SDK_CONTRACT_PATH),
            serde_json::to_vec(&contract).unwrap(),
        )
        .unwrap();

        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "synthetic sealed Mesa source"]);
        let commit = GitObjectId::try_from(git(&root, &["rev-parse", "HEAD"])).unwrap();
        let tree = GitObjectId::try_from(git(&root, &["rev-parse", "HEAD:"])).unwrap();
        Self {
            _temporary: temporary,
            root: root.canonicalize().unwrap(),
            identity: CommittedSourceIdentity { commit, tree },
        }
    }

    fn request(&self) -> SourcePolicyRequest {
        SourcePolicyRequest {
            source_root: self.root.clone(),
            expected_source: self.identity.clone(),
            source_preset: SOURCE_PRESET.to_owned(),
            compiler_profile: COMPILER_PROFILE.to_owned(),
        }
    }
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Mesa policy fixture",
            "-c",
            "user.email=mesa-policy-fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments);
    let output =
        aros_common::run_output_with_timeout(&mut command, 64 * 1024, Duration::from_secs(10))
            .unwrap();
    assert!(
        output.status.success() && !output.timed_out,
        "git {arguments:?} failed: {output:?}"
    );
    String::from_utf8(output.stdout.exact_bytes().unwrap().to_vec())
        .unwrap()
        .trim()
        .to_owned()
}

fn sdk_identity() -> SdkIdentity {
    SdkIdentity {
        sdk_id: "mesa-policy-fixture".into(),
        version: "1.0.0".into(),
        target: SdkTarget {
            target_triple: "x86_64-unknown-aros".into(),
            abi: BTreeMap::from([
                ("source_cpu".into(), SdkAbiValue::Text("x86_64".into())),
                (
                    "target_triple".into(),
                    SdkAbiValue::Text("x86_64-unknown-aros".into()),
                ),
                ("pointer_width".into(), SdkAbiValue::Unsigned(64)),
            ]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(b"selected source tree"),
            contract_sha256: sha256_bytes(b"selected generic SDK contract"),
        },
        source_date_epoch: 1,
    }
}

fn policy_value(identity: &SdkIdentity, mesa: bool, native_sdk_required: bool) -> Value {
    json!({
        "source_commit": identity.source.commit,
        "source_tree": identity.source.git_tree,
        "source_preset": SOURCE_PRESET,
        "compiler_profile": COMPILER_PROFILE,
        "profiles_sha256": sha256_bytes(b"selected source profiles").to_string(),
        "native_consumer": {
            "file": SDK_CONTRACT_PATH,
            "sha256": identity.source.contract_sha256,
            "schema": "aros-sdk-consumer-contract-v1"
        },
        "native_sdk_required": native_sdk_required,
        "mesa_preflight_required": mesa
    })
}

fn policy_bytes(identity: &SdkIdentity, mesa: bool, native_sdk_required: bool) -> Vec<u8> {
    serde_json::to_vec(&policy_value(identity, mesa, native_sdk_required)).unwrap()
}

#[test]
fn source_mesa_preflight_binds_declared_host_python_lock_bytes() {
    use crate::compatibility::SourceMesaPreflightProfile;
    let fixture = SourceFixture::new("26.0.0");
    let profile = aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    let path = fixture.root.join(SDK_CONTRACT_PATH);
    let mut contract: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let lock = serde_json::to_vec(&json!({
        "schema": "aros-host-python-lock-v1",
        "packages": [{
            "name": "mako", "version": "1.3.10", "filename": "mako-1.3.10.tar.gz",
            "url": "https://example.invalid/mako-1.3.10.tar.gz", "size": 1,
            "sha256": sha256_bytes(b"fixture archive"), "source_root": "mako-1.3.10", "python_path": "."
        }]
    })).unwrap();
    fs::write(fixture.root.join("host-python.json"), &lock).unwrap();
    contract["mesa_configure_target"] = json!("pc-x86_64");
    contract["mesa_host_python_lock"] = json!("host-python.json");
    fs::write(&path, serde_json::to_vec(&contract).unwrap()).unwrap();
    assert!(
        SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err(),
        "unsealed lock"
    );
    contract["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path": "host-python.json", "sha256": sha256_bytes(&lock)}));
    fs::write(&path, serde_json::to_vec(&contract).unwrap()).unwrap();
    let selected = SourceMesaPreflightProfile::load(&fixture.root, &profile).unwrap();
    assert_eq!(
        selected.python_lock().unwrap().sha256(),
        &sha256_bytes(&lock)
    );
    fs::write(fixture.root.join("host-python.json"), b"{}\n").unwrap();
    assert!(
        SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err(),
        "changed sealed lock"
    );
    contract["inputs"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["sha256"] = json!(sha256_bytes(b"{}\n"));
    fs::write(&path, serde_json::to_vec(&contract).unwrap()).unwrap();
    assert!(
        SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err(),
        "sealed invalid lock"
    );
}

#[test]
fn source_mesa_preflight_uses_explicit_sealed_configure_target_and_complete_sdk_abi() {
    use crate::compatibility::SourceMesaPreflightProfile;
    let fixture = SourceFixture::new("26.0.0");
    let profile = aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    let contract_path = fixture.root.join(SDK_CONTRACT_PATH);
    let mut document: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    assert!(SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err());
    document["mesa_configure_target"] = json!("explicit-not-the-board-name");
    fs::write(&contract_path, serde_json::to_vec(&document).unwrap()).unwrap();
    let selected = SourceMesaPreflightProfile::load(&fixture.root, &profile).unwrap();
    assert_eq!(selected.configure_target(), "explicit-not-the-board-name");
    assert_eq!(selected.target().target_triple, "x86_64-unknown-aros");
    assert_eq!(selected.target().abi["use_mmu"], SdkAbiValue::Boolean(true));
    assert_eq!(
        selected.target().abi["flavour"],
        SdkAbiValue::Text("native".into())
    );
    let mut different = profile.clone();
    different.transpiler.as_mut().unwrap().use_mmu = false;
    assert!(SourceMesaPreflightProfile::load(&fixture.root, &different).is_err());
    document["target"]["abi"]["use_mmu"] = json!(false);
    fs::write(&contract_path, serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err());
}

#[test]
fn source_mesa_genmodule_requests_bind_the_source_preset_and_all_selectors() {
    use crate::compatibility::SourceMesaPreflightProfile;
    use aros_common::mesa_genmodule_inventory::{
        MesaGenmoduleInventoryScope, MesaGenmoduleSelectionBinding,
    };

    let fixture = SourceFixture::new("26.0.0");
    let profile = aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    let contract_path = fixture.root.join(SDK_CONTRACT_PATH);
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["mesa_configure_target"] = json!("fixture-configure-target");
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let selected = SourceMesaPreflightProfile::load(&fixture.root, &profile).unwrap();
    let contract_sha256 = sha256_bytes(&fs::read(&contract_path).unwrap());
    assert_ne!(SOURCE_PRESET, COMPILER_PROFILE);

    let contract_roots_binding =
        selected.genmodule_inventory_binding(MesaGenmoduleInventoryScope::ContractRoots);
    assert_eq!(
        contract_roots_binding,
        MesaGenmoduleSelectionBinding {
            profile: SOURCE_PRESET.into(),
            kind: "sdk-consumer".into(),
            scope: MesaGenmoduleInventoryScope::ContractRoots,
            contract_sha256: contract_sha256.clone(),
        }
    );
    let full_target_binding =
        selected.genmodule_inventory_binding(MesaGenmoduleInventoryScope::FullTarget);
    assert_eq!(full_target_binding.profile, SOURCE_PRESET);
    assert_eq!(full_target_binding.kind, "sdk-consumer");
    assert_eq!(
        full_target_binding.scope,
        MesaGenmoduleInventoryScope::FullTarget
    );
    assert_eq!(full_target_binding.contract_sha256, contract_sha256);

    let common_selectors = [
        "--mesa-genmodule-inventory-only",
        "--cpu",
        "x86_64",
        "--platform",
        "pc",
        "--family",
        "fixture-family",
        "--variant",
        "fixture-variant",
        "--toolchain",
        "llvm",
        "--cpu32",
        "i686",
        "--use-mmu",
        "1",
        "--float-abi",
        "fixture-float",
        "--mesa-version",
        "26.0.0",
    ];
    let mut expected_contract_roots = vec![
        "--mesa-genmodule-inventory-only".to_owned(),
        "--native-consumer-profile".into(),
        SOURCE_PRESET.into(),
        "--native-consumer-contract-sha256".into(),
        contract_sha256.to_string(),
    ];
    expected_contract_roots.extend(common_selectors[1..].iter().map(|value| (*value).into()));
    assert_eq!(
        selected
            .genmodule_inventory_arguments(MesaGenmoduleInventoryScope::ContractRoots)
            .unwrap(),
        expected_contract_roots
    );

    let mut expected_full_target = vec![
        "--mesa-genmodule-inventory-only".to_owned(),
        "--mesa-genmodule-full-inventory-profile".into(),
        SOURCE_PRESET.into(),
        "--mesa-genmodule-full-contract-sha256".into(),
        contract_sha256.to_string(),
    ];
    expected_full_target.extend(common_selectors[1..].iter().map(|value| (*value).into()));
    assert_eq!(
        selected
            .genmodule_inventory_arguments(MesaGenmoduleInventoryScope::FullTarget)
            .unwrap(),
        expected_full_target
    );
}

#[test]
fn source_mesa_preflight_refuses_unsafe_targets_missing_source_and_changed_seals() {
    use crate::compatibility::SourceMesaPreflightProfile;
    for target in ["", "with space", "x;command", "x\ncommand", "x/../y"] {
        let fixture = SourceFixture::new("26.0.0");
        let profile =
            aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
                .unwrap()
                .targets
                .remove(0);
        let path = fixture.root.join(SDK_CONTRACT_PATH);
        let mut contract: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        contract["mesa_configure_target"] = json!(target);
        fs::write(path, serde_json::to_vec(&contract).unwrap()).unwrap();
        assert!(
            SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err(),
            "{target:?}"
        );
    }
    let fixture = SourceFixture::new("26.0.0");
    let profile = aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    let path = fixture.root.join(SDK_CONTRACT_PATH);
    let mut contract: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    contract["mesa_configure_target"] = json!("pc-x86_64");
    fs::write(path, serde_json::to_vec(&contract).unwrap()).unwrap();
    fs::write(fixture.root.join("policy.json"), b"changed sealed input").unwrap();
    assert!(SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err());
    let missing = tempfile::tempdir().unwrap();
    assert!(SourceMesaPreflightProfile::load(missing.path(), &profile).is_err());
}

#[test]
fn source_mesa_preflight_does_not_enable_the_mesa20_path() {
    let fixture = SourceFixture::new("20.0.8");
    let profile = aros_common::TargetProfile::load_config(&fixture.root.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    assert!(
        crate::compatibility::SourceMesaPreflightProfile::load(&fixture.root, &profile).is_err()
    );
}

#[test]
#[ignore = "requires explicit selected real AROS-NX source root; selection-only, no compiler execution"]
fn actual_source_mesa_profiles_load_all_three_sealed_sdk_selections() {
    let root = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_SOURCE_SELECTION_ROOT")
            .expect("select source root explicitly"),
    );
    assert!(root.is_absolute());
    let profiles =
        aros_common::TargetProfile::load_config(&root.join("aros-targets.toml")).unwrap();
    for (name, configure, triple) in [
        ("pc-x86_64", "pc-x86_64", "x86_64-unknown-aros"),
        ("arm-raspi", "raspi-armhf", "arm-unknown-aros"),
        ("rpi-aarch64", "raspi-aarch64", "aarch64-unknown-aros"),
    ] {
        let profile = profiles
            .targets
            .iter()
            .find(|profile| profile.name == name)
            .unwrap();
        let selected =
            crate::compatibility::SourceMesaPreflightProfile::load(&root, profile).unwrap();
        assert_eq!(selected.configure_target(), configure);
        assert_eq!(selected.target().target_triple, triple);
    }
}

#[test]
fn derives_mesa26_and_mesa20_from_clean_pinned_sealed_sdk_sources() {
    let mesa26 = SourceFixture::new("26.0.0");
    let mesa26_policy = inspect_native_source_policy(&mesa26.request()).unwrap();
    assert!(mesa26_policy.mesa_preflight_required);
    assert_eq!(
        serde_json::to_value(&mesa26_policy).unwrap()["mesa_preflight_required"],
        json!(true)
    );
    assert_eq!(
        mesa26_policy.native_consumer,
        Some(NativeConsumerPolicy {
            file: SDK_CONTRACT_PATH.into(),
            sha256: sha256_bytes(&fs::read(mesa26.root.join(SDK_CONTRACT_PATH)).unwrap()),
            schema: "aros-sdk-consumer-contract-v1".into(),
        })
    );
    assert!(mesa26_policy.native_sdk_required);

    let mesa20 = SourceFixture::new("20.0.0");
    let mesa20_policy = inspect_native_source_policy(&mesa20.request()).unwrap();
    assert!(!mesa20_policy.mesa_preflight_required);
    assert!(mesa20_policy.native_sdk_required);
}

#[test]
fn refuses_dirty_or_stale_pinned_source() {
    let dirty = SourceFixture::new("26.0.0");
    fs::write(dirty.root.join("untracked.txt"), b"not committed\n").unwrap();
    assert!(inspect_native_source_policy(&dirty.request()).is_err());

    let stale = SourceFixture::new("26.0.0");
    fs::write(stale.root.join("policy.json"), b"changed\n").unwrap();
    git(&stale.root, &["add", "policy.json"]);
    git(&stale.root, &["commit", "-qm", "advance synthetic source"]);
    // The checkout is clean, but the independently selected commit and tree
    // still refer to the earlier snapshot.
    assert!(inspect_native_source_policy(&stale.request()).is_err());
}

#[test]
fn sdk_policy_readback_returns_both_selected_boolean_values() {
    let identity = sdk_identity();
    for mesa in [true, false] {
        assert_eq!(
            readback_sdk_mesa_policy(
                &policy_bytes(&identity, mesa, true),
                &identity,
                SOURCE_PRESET,
                COMPILER_PROFILE,
            )
            .unwrap(),
            mesa
        );
    }
}

#[test]
fn sdk_policy_readback_requires_bounded_closed_unambiguous_metadata() {
    let identity = sdk_identity();
    let mut missing = policy_value(&identity, true, true);
    missing
        .as_object_mut()
        .unwrap()
        .remove("mesa_preflight_required");
    assert!(readback_sdk_mesa_policy(
        &serde_json::to_vec(&missing).unwrap(),
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());

    let mut unknown = policy_value(&identity, true, true);
    unknown["unrecognized"] = json!(true);
    assert!(readback_sdk_mesa_policy(
        &serde_json::to_vec(&unknown).unwrap(),
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());

    let mut wrong_schema = policy_value(&identity, true, true);
    wrong_schema["native_consumer"]["schema"] = json!("aros-sdk-consumer-contract-v99");
    assert!(readback_sdk_mesa_policy(
        &serde_json::to_vec(&wrong_schema).unwrap(),
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());

    let duplicate = String::from_utf8(policy_bytes(&identity, true, true))
        .unwrap()
        .replace(
            "\"mesa_preflight_required\":true",
            "\"mesa_preflight_required\":true,\"mesa_preflight_required\":true",
        );
    assert!(readback_sdk_mesa_policy(
        duplicate.as_bytes(),
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());

    assert!(readback_sdk_mesa_policy(
        &vec![b' '; crate::canonical::MAX_DOCUMENT_BYTES + 1],
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());
}

#[test]
fn sdk_policy_readback_binds_every_independently_selected_identity_field() {
    let identity = sdk_identity();
    let bytes = policy_bytes(&identity, true, true);

    let mut wrong_commit = identity.clone();
    wrong_commit.source.commit = "3".repeat(40);
    assert!(
        readback_sdk_mesa_policy(&bytes, &wrong_commit, SOURCE_PRESET, COMPILER_PROFILE,).is_err()
    );

    let mut wrong_tree = identity.clone();
    wrong_tree.source.git_tree = "4".repeat(40);
    assert!(
        readback_sdk_mesa_policy(&bytes, &wrong_tree, SOURCE_PRESET, COMPILER_PROFILE,).is_err()
    );

    let mut wrong_contract = identity.clone();
    wrong_contract.source.contract_sha256 = sha256_bytes(b"another contract");
    assert!(
        readback_sdk_mesa_policy(&bytes, &wrong_contract, SOURCE_PRESET, COMPILER_PROFILE,)
            .is_err()
    );

    assert!(
        readback_sdk_mesa_policy(&bytes, &identity, "another-source-preset", COMPILER_PROFILE,)
            .is_err()
    );
    assert!(
        readback_sdk_mesa_policy(&bytes, &identity, SOURCE_PRESET, "another-compiler-profile",)
            .is_err()
    );

    let no_ordinary_links = policy_bytes(&identity, true, false);
    assert!(readback_sdk_mesa_policy(
        &no_ordinary_links,
        &identity,
        SOURCE_PRESET,
        COMPILER_PROFILE,
    )
    .is_err());
}
