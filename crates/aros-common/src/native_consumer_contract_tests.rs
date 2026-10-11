//! Synthetic source contracts test admission, never real compiler execution.
use super::*;
use serde_json::{json, Value};
use std::fs;

const PROFILE: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";

fn fixture() -> (tempfile::TempDir, TargetProfile, Value) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("aros-targets.toml"), PROFILE).unwrap();
    fs::write(directory.path().join("policy.json"), "{}").unwrap();
    let profile = TargetProfile::parse_config(PROFILE, "fixture")
        .unwrap()
        .targets
        .remove(0);
    let document = json!({
        "schema": SCHEMA_V1, "profile": "example-sdk",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "metamake_projection": "policy.json",
        "inputs": [
            {"path": "aros-targets.toml", "sha256": crate::sha256_bytes(PROFILE.as_bytes())},
            {"path": "policy.json", "sha256": crate::sha256_bytes(b"{}")}
        ],
        "abi": { "source_cpu": "riscv64", "target_triple": "riscv64-aros",
            "isa": "rva22u64", "abi": "lp64d", "code_model": "medany",
            "flavour": "native", "platform_smp": true, "use_mmu": true }
    });
    (directory, profile, document)
}

fn v2_document(directory: &Path, document: &Value) -> Value {
    fs::write(
        directory.join("probe.c"),
        b"/* sealed C contract fixture */\n",
    )
    .unwrap();
    fs::write(
        directory.join("probe.cpp"),
        b"/* sealed C++ contract fixture */\n",
    )
    .unwrap();
    let mut v2 = document.clone();
    v2["schema"] = json!(SCHEMA_V2);
    v2["inputs"].as_array_mut().unwrap().extend([
        json!({
            "path": "probe.c",
            "sha256": crate::sha256_bytes(b"/* sealed C contract fixture */\n")
        }),
        json!({
            "path": "probe.cpp",
            "sha256": crate::sha256_bytes(b"/* sealed C++ contract fixture */\n")
        }),
    ]);
    v2["native_sdk_link_probes"] = json!({
        "c": { "source": "probe.c", "libraries": [] },
        "cxx": { "source": "probe.cpp", "libraries": [] }
    });
    v2
}

fn load(
    root: &Path,
    profile: &TargetProfile,
    document: &Value,
) -> Result<LoadedNativeConsumerContract> {
    fs::write(
        root.join("consumer.json"),
        serde_json::to_vec(document).unwrap(),
    )
    .unwrap();
    load_bound_native_consumer_contract(root, Path::new("consumer.json"), profile)
}

#[test]
fn consumer_contract_requires_no_core_package_or_media_and_binds_exact_bytes() {
    let (directory, profile, document) = fixture();
    let loaded = load(directory.path(), &profile, &document).unwrap();
    assert_eq!(loaded.contract.roots, ["includes", "linklibs"]);
    assert!(!serde_json::to_value(&loaded.contract)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("native_sdk_link_probes"));
    assert_eq!(
        loaded.sha256,
        crate::sha256_file(&loaded.path).unwrap().digest
    );
    assert_eq!(
        loaded
            .contract
            .make_variables_for_host("arbitrary-host")
            .unwrap(),
        BTreeMap::new()
    );
    assert!(loaded.contract.require_native_sdk_link_probes().is_err());
}

#[test]
fn consumer_v2_requires_sealed_c_and_cxx_sources_and_preserves_library_order() {
    let (directory, profile, document) = fixture();
    let default_driver = v2_document(directory.path(), &document);
    let loaded = load(directory.path(), &profile, &default_driver).unwrap();
    let probes = loaded.contract.require_native_sdk_link_probes().unwrap();
    assert_eq!(probes.c.source, "probe.c");
    assert!(probes.c.libraries.is_empty());
    assert_eq!(probes.cxx.source, "probe.cpp");
    assert!(probes.cxx.libraries.is_empty());

    let mut explicit = default_driver;
    explicit["native_sdk_link_probes"]["c"]["libraries"] = json!(["zeta", "alpha"]);
    let loaded = load(directory.path(), &profile, &explicit).unwrap();
    assert_eq!(
        loaded
            .contract
            .require_native_sdk_link_probes()
            .unwrap()
            .c
            .libraries,
        ["zeta".to_owned(), "alpha".to_owned()]
    );
}

#[test]
fn consumer_v2_probe_contract_rejects_missing_mismatched_and_unsafe_facts() {
    let (directory, profile, document) = fixture();
    let v2 = v2_document(directory.path(), &document);

    let mut missing = v2.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("native_sdk_link_probes");
    assert!(load(directory.path(), &profile, &missing).is_err());

    let mut schema_mismatch = v2.clone();
    schema_mismatch["schema"] = json!(SCHEMA_V1);
    assert!(load(directory.path(), &profile, &schema_mismatch).is_err());

    let mut unknown = v2.clone();
    unknown["native_sdk_link_probes"]["c"]["flags"] = json!(["-Wl,--as-needed"]);
    assert!(load(directory.path(), &profile, &unknown).is_err());
    let mut null_probes = v2.clone();
    null_probes["native_sdk_link_probes"] = Value::Null;
    assert!(load(directory.path(), &profile, &null_probes).is_err());

    let mut escaped = v2.clone();
    escaped["native_sdk_link_probes"]["c"]["source"] = json!("../outside.c");
    assert!(load(directory.path(), &profile, &escaped).is_err());

    let mut wrong_suffix = v2.clone();
    wrong_suffix["native_sdk_link_probes"]["c"]["source"] = json!("probe.cpp");
    assert!(load(directory.path(), &profile, &wrong_suffix).is_err());
    wrong_suffix["native_sdk_link_probes"]["c"]["source"] = json!("probe.c");
    wrong_suffix["native_sdk_link_probes"]["cxx"]["source"] = json!("probe.c");
    assert!(load(directory.path(), &profile, &wrong_suffix).is_err());

    fs::write(
        directory.path().join("unsealed.c"),
        b"/* present but unsealed */\n",
    )
    .unwrap();
    let mut unsealed = v2.clone();
    unsealed["native_sdk_link_probes"]["c"]["source"] = json!("unsealed.c");
    assert!(load(directory.path(), &profile, &unsealed).is_err());

    let mut duplicate_library = v2.clone();
    duplicate_library["native_sdk_link_probes"]["c"]["libraries"] = json!(["library", "library"]);
    assert!(load(directory.path(), &profile, &duplicate_library).is_err());

    for libraries in [
        json!(["-Wl,--as-needed"]),
        json!(["../library"]),
        json!(["$(LIBRARY)"]),
        json!((0..17)
            .map(|index| format!("library{index}"))
            .collect::<Vec<_>>()),
    ] {
        let mut unsafe_library = v2.clone();
        unsafe_library["native_sdk_link_probes"]["c"]["libraries"] = libraries;
        assert!(load(directory.path(), &profile, &unsafe_library).is_err());
    }

    let mut v1_with_probes = document;
    v1_with_probes["native_sdk_link_probes"] = v2["native_sdk_link_probes"].clone();
    assert!(load(directory.path(), &profile, &v1_with_probes).is_err());
}

#[test]
fn consumer_roots_are_nonempty_bounded_literal_sorted_and_unique() {
    let (directory, profile, document) = fixture();
    for roots in [
        json!([]),
        json!(["z", "a"]),
        json!(["a", "a"]),
        json!(["$(GUESS)"]),
        json!(["../outside"]),
        json!(["-flag"]),
        json!(["."]),
        json!([".."]),
        json!([""]),
        json!(["a b"]),
        json!(["a;command"]),
        json!(["a".repeat(257)]),
        json!((0..17).map(|i| format!("root-{i:02}")).collect::<Vec<_>>()),
    ] {
        let mut changed = document.clone();
        changed["roots"] = roots;
        assert!(
            load(directory.path(), &profile, &changed).is_err(),
            "{changed}"
        );
    }
}

#[test]
fn consumer_contract_rejects_changed_undeclared_or_aliased_source_inputs() {
    let (directory, profile, document) = fixture();
    for changed in [
        {
            let mut v = document.clone();
            v["metamake_projection"] = json!("missing.json");
            v
        },
        {
            let mut v = document.clone();
            v["inputs"] = json!([]);
            v
        },
        {
            let mut v = document.clone();
            v["inputs"][0]["sha256"] = json!("0".repeat(64));
            v
        },
        {
            let mut v = document.clone();
            v["inputs"][1]["path"] = json!("../outside");
            v
        },
        {
            let mut v = document.clone();
            v["inputs"] = json!([v["inputs"][1].clone()]);
            v
        },
        {
            let mut v = document.clone();
            let duplicate = v["inputs"][0].clone();
            v["inputs"].as_array_mut().unwrap().push(duplicate);
            v
        },
    ] {
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    fs::write(directory.path().join("policy.json"), "{\"changed\":true}").unwrap();
    assert!(load(directory.path(), &profile, &document).is_err());
}

#[test]
fn consumer_contract_rejects_identity_abi_and_profile_drift() {
    let (directory, profile, document) = fixture();
    for (field, value) in [
        ("schema", "unknown"),
        ("profile", "other"),
        ("source_baseline", "not-git"),
    ] {
        let mut changed = document.clone();
        changed[field] = json!(value);
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    for (field, value) in [
        ("source_cpu", "riscv"),
        ("target_triple", "riscv-aros"),
        ("isa", "rv32imafc"),
        ("abi", "lp64"),
        ("code_model", "unknown"),
        ("flavour", "standalone"),
    ] {
        let mut changed = document.clone();
        changed["abi"][field] = json!(value);
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    for field in ["platform_smp", "use_mmu"] {
        let mut changed = document.clone();
        changed["abi"][field] = json!(false);
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    let mut unbound = profile;
    unbound.features.push("invented".into());
    assert!(load(directory.path(), &unbound, &document).is_err());
}

#[test]
fn consumer_configuration_is_explicit_and_bounded_without_selector_overrides() {
    let (directory, profile, document) = fixture();
    for (name, value) in [
        ("AROS_TOOLCHAIN", "gnu"),
        ("AROS_HOST_CPU", "aarch64"),
        ("SOURCE_FLAGS", "$(SHELL)"),
        ("BAD", "a b"),
        ("bad", "yes"),
    ] {
        let mut changed = document.clone();
        changed["make_variables"] = json!({name: value});
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    let mut valid = document;
    valid["make_variables"] = json!({"AROS_TOOLCHAIN_RELEASE": "0"});
    valid["host_make_variables"] = json!({"macos-aarch64": {"AROS_HOST_CPU": "aarch64"}});
    let loaded = load(directory.path(), &profile, &valid).unwrap();
    assert_eq!(
        loaded
            .contract
            .make_variables_for_host("macos-aarch64")
            .unwrap()["AROS_HOST_CPU"],
        "aarch64"
    );
    assert!(loaded
        .contract
        .make_variables_for_host("unlisted-host")
        .is_err());
    valid["host_make_variables"]["macos-aarch64"]["AROS_TOOLCHAIN_RELEASE"] = json!("1");
    assert!(load(directory.path(), &profile, &valid).is_err());
}

#[test]
fn consumer_json_is_closed_duplicate_safe_and_size_bounded() {
    let (directory, profile, document) = fixture();
    for field in ["core", "package", "media", "unknown"] {
        let mut changed = document.clone();
        changed[field] = json!({});
        assert!(load(directory.path(), &profile, &changed).is_err());
    }
    let text = serde_json::to_string(&document).unwrap();
    let duplicate = text.replacen('{', "{\"schema\":\"duplicate\",", 1);
    fs::write(directory.path().join("consumer.json"), duplicate).unwrap();
    assert!(load_bound_native_consumer_contract(
        directory.path(),
        Path::new("consumer.json"),
        &profile
    )
    .is_err());
    fs::write(
        directory.path().join("consumer.json"),
        vec![b' '; MAX_BYTES as usize + 1],
    )
    .unwrap();
    assert!(load_bound_native_consumer_contract(
        directory.path(),
        Path::new("consumer.json"),
        &profile
    )
    .is_err());
}

#[test]
fn consumer_compiler_binding_rejects_other_families_and_target_contracts() {
    let (directory, profile, document) = fixture();
    let loaded = load(directory.path(), &profile, &document).unwrap();
    let compiler: ArosCompilerIdentity = serde_json::from_value(json!({
        "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
        "target": {"schema": "aros-riscv-target-v1", "isa": "rva22u64", "abi": "lp64d", "code_model": "medany",
            "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0}
    })).unwrap();
    validate_native_consumer_compiler(&loaded.contract, &compiler, "riscv64-aros").unwrap();
    assert!(validate_native_consumer_compiler(&loaded.contract, &compiler, "riscv-aros").is_err());
    let mut changed = loaded.contract.clone();
    changed.abi.isa = "rv64imafdc".into();
    assert!(validate_native_consumer_compiler(&changed, &compiler, "riscv64-aros").is_err());
    let llvm: ArosCompilerIdentity =
        serde_json::from_value(json!({"family": "llvm", "version": "23.0.0"})).unwrap();
    assert!(validate_native_consumer_compiler(&loaded.contract, &llvm, "riscv64-aros").is_err());
}

#[test]
fn consumer_contract_path_must_be_the_measured_profile_binding() {
    let (directory, profile, document) = fixture();
    fs::write(
        directory.path().join("other.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    assert!(load_bound_native_consumer_contract(
        directory.path(),
        Path::new("other.json"),
        &profile
    )
    .is_err());
    for relative in ["../consumer.json", "/consumer.json", "./consumer.json"] {
        assert!(load_bound_native_consumer_contract(
            directory.path(),
            Path::new(relative),
            &profile
        )
        .is_err());
    }
}

#[cfg(unix)]
#[test]
fn consumer_rejects_contract_and_input_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;
    let (directory, profile, document) = fixture();
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(directory.path().join("real.json"), &bytes).unwrap();
    symlink("real.json", directory.path().join("consumer.json")).unwrap();
    assert!(load_bound_native_consumer_contract(
        directory.path(),
        Path::new("consumer.json"),
        &profile
    )
    .is_err());
    fs::remove_file(directory.path().join("consumer.json")).unwrap();
    fs::rename(
        directory.path().join("policy.json"),
        directory.path().join("real-policy.json"),
    )
    .unwrap();
    symlink("real-policy.json", directory.path().join("policy.json")).unwrap();
    assert!(load(directory.path(), &profile, &document).is_err());
    fs::remove_file(directory.path().join("policy.json")).unwrap();
    fs::create_dir(directory.path().join("real-dir")).unwrap();
    fs::write(directory.path().join("real-dir/policy.json"), "{}").unwrap();
    symlink("real-dir", directory.path().join("alias-dir")).unwrap();
    let mut aliased = document;
    aliased["metamake_projection"] = json!("alias-dir/policy.json");
    aliased["inputs"][1]["path"] = json!("alias-dir/policy.json");
    assert!(load(directory.path(), &profile, &aliased).is_err());
}

#[test]
fn consumer_rejects_hardlinked_and_oversized_inputs() {
    let (directory, profile, document) = fixture();
    fs::hard_link(
        directory.path().join("policy.json"),
        directory.path().join("policy-alias.json"),
    )
    .unwrap();
    let mut aliased = document.clone();
    aliased["inputs"].as_array_mut().unwrap().push(json!({
        "path": "policy-alias.json", "sha256": crate::sha256_bytes(b"{}")
    }));
    assert!(load(directory.path(), &profile, &aliased).is_err());
    fs::write(
        directory.path().join("policy.json"),
        vec![b' '; MAX_INPUT_BYTES as usize + 1],
    )
    .unwrap();
    assert!(load(directory.path(), &profile, &document).is_err());
}
