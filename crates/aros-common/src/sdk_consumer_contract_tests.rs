//! Synthetic SDK source contracts test binding only, not native execution.
use super::*;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::fs;

const BASELINE: &str = "0123456789abcdef0123456789abcdef01234567";
const C_SOURCE: &[u8] = b"int sdk_c_probe(void) { return 0; }\n";
const CXX_SOURCE: &[u8] = b"int sdk_cxx_probe() { return 0; }\n";

struct Fixture {
    directory: tempfile::TempDir,
    profile: TargetProfile,
    document: Value,
}

#[derive(Clone, Copy)]
struct FixtureTarget<'a> {
    name: &'a str,
    arch: &'a str,
    source_cpu: &'a str,
    target_triple: &'a str,
    toolchain: &'a str,
    float_abi: Option<&'a str>,
    bootstrap: Option<(&'a str, bool)>,
    use_mmu: bool,
}

fn make_fixture(target: FixtureTarget<'_>) -> Fixture {
    let FixtureTarget {
        name,
        arch,
        source_cpu,
        target_triple,
        toolchain,
        float_abi,
        bootstrap,
        use_mmu,
    } = target;
    let directory = tempfile::tempdir().unwrap();
    let mut profile_text = format!(
        "[[targets]]\nname = {name:?}\narch = {arch:?}\nplatform = \"fixture\"\nbsp = \"fixture\"\nsdk_consumer_contract = \"sdk-consumer.json\"\n"
    );
    if let Some(float_abi) = float_abi {
        writeln!(profile_text, "float_abi = {float_abi:?}").unwrap();
    }
    write!(profile_text,
        "\n[targets.transpiler]\nfamily = \"\"\nvariant = \"\"\ntoolchain = {toolchain:?}\ncpu32 = \"\"\nuse_mmu = {use_mmu}\n"
    ).unwrap();
    if let Some((flavour, platform_smp)) = bootstrap {
        write!(
            profile_text,
            "\n[targets.bootstrap_abi]\nflavour = {flavour:?}\nplatform_smp = {platform_smp}\n"
        )
        .unwrap();
    }
    fs::write(directory.path().join("aros-targets.toml"), &profile_text).unwrap();
    fs::write(directory.path().join("policy.json"), b"{}").unwrap();
    fs::write(directory.path().join("probe.c"), C_SOURCE).unwrap();
    fs::write(directory.path().join("probe.cpp"), CXX_SOURCE).unwrap();
    let profile = TargetProfile::parse_config(&profile_text, "fixture")
        .unwrap()
        .targets
        .remove(0);

    let mut abi = serde_json::Map::new();
    abi.insert("source_cpu".into(), json!(source_cpu));
    abi.insert("pointer_width".into(), json!(profile.arch.pointer_width()));
    abi.insert("use_mmu".into(), json!(use_mmu));
    abi.insert("isa".into(), json!("source-declared-isa"));
    if let Some(float_abi) = float_abi {
        abi.insert("float_abi".into(), json!(float_abi));
    }
    if let Some((flavour, platform_smp)) = bootstrap {
        abi.insert("flavour".into(), json!(flavour));
        abi.insert("platform_smp".into(), json!(platform_smp));
    }
    let inputs = [
        (
            "aros-targets.toml",
            crate::sha256_bytes(profile_text.as_bytes()),
        ),
        ("policy.json", crate::sha256_bytes(b"{}")),
        ("probe.c", crate::sha256_bytes(C_SOURCE)),
        ("probe.cpp", crate::sha256_bytes(CXX_SOURCE)),
    ];
    let document = json!({
        "schema": SDK_CONSUMER_CONTRACT_SCHEMA,
        "profile": name,
        "source_baseline": BASELINE,
        "roots": ["includes", "linklibs"],
        "metamake_projection": "policy.json",
        "inputs": inputs.iter().map(|(path, sha256)| json!({"path": path, "sha256": sha256})).collect::<Vec<_>>(),
        "target": {
            "target_triple": target_triple,
            "abi": abi,
        },
        "native_sdk_link_probes": {
            "c": {"source": "probe.c", "libraries": ["c"]},
            "cxx": {"source": "probe.cpp", "libraries": ["stdc++"]}
        }
    });
    Fixture {
        directory,
        profile,
        document,
    }
}

fn load(fixture: &Fixture, document: &Value) -> Result<LoadedSdkConsumerContract> {
    let bytes = serde_json::to_vec(document).unwrap();
    load_bytes(fixture, &bytes)
}

fn load_bytes(fixture: &Fixture, bytes: &[u8]) -> Result<LoadedSdkConsumerContract> {
    fs::write(fixture.directory.path().join("sdk-consumer.json"), bytes).unwrap();
    load_bound_sdk_consumer_contract(
        fixture.directory.path(),
        Path::new("sdk-consumer.json"),
        &fixture.profile,
    )
}

#[test]
fn generic_sdk_contract_binds_four_source_owned_target_families() {
    let cases = [
        (
            "pc-x86_64",
            "x86_64",
            "x86_64",
            "x86_64-unknown-aros",
            "llvm",
            None,
            Some(("native", false)),
            true,
        ),
        (
            "arm-raspi",
            "arm",
            "arm",
            "arm-unknown-aros",
            "llvm",
            Some("hard"),
            Some(("native", false)),
            true,
        ),
        (
            "rpi-aarch64",
            "aarch64",
            "aarch64",
            "aarch64-unknown-aros",
            "llvm",
            None,
            Some(("native", true)),
            true,
        ),
        (
            "rv32-esp32p4",
            "riscv32",
            "riscv",
            "riscv-aros",
            "gnu",
            Some("ilp32f"),
            Some(("standalone", false)),
            false,
        ),
    ];
    for (name, arch, cpu, triple, toolchain, float_abi, bootstrap, use_mmu) in cases {
        let fixture = make_fixture(FixtureTarget {
            name,
            arch,
            source_cpu: cpu,
            target_triple: triple,
            toolchain,
            float_abi,
            bootstrap,
            use_mmu,
        });
        let loaded = load(&fixture, &fixture.document).unwrap_or_else(|error| {
            panic!("{name} source-owned SDK descriptor should validate: {error}")
        });
        assert_eq!(loaded.contract.schema, SDK_CONSUMER_CONTRACT_SCHEMA);
        assert_eq!(loaded.contract.target.target_triple, triple);
        assert_eq!(
            loaded.sha256,
            crate::sha256_file(&loaded.path).unwrap().digest
        );
        assert_eq!(
            loaded.contract.target.abi.get("isa"),
            Some(&SdkAbiValue::Text("source-declared-isa".into()))
        );
    }

    let riscv32_display = make_fixture(FixtureTarget {
        name: "rv32-esp32p4",
        arch: "riscv32",
        source_cpu: "riscv",
        target_triple: "riscv-unknown-aros",
        toolchain: "gnu",
        float_abi: Some("ilp32f"),
        bootstrap: Some(("standalone", false)),
        use_mmu: false,
    });
    let mut display_alias = riscv32_display.document.clone();
    display_alias["target"]["target_triple"] = json!("riscv32-vendor-aros");
    assert!(load(&riscv32_display, &display_alias).is_ok());
}

#[test]
fn sdk_target_rejects_profile_scalar_and_triple_disagreement() {
    let fixture = make_fixture(FixtureTarget {
        name: "arm-raspi",
        arch: "arm",
        source_cpu: "arm",
        target_triple: "arm-unknown-aros",
        toolchain: "llvm",
        float_abi: Some("hard"),
        bootstrap: Some(("native", false)),
        use_mmu: true,
    });
    let cases = [
        ("target_triple", json!("aarch64-unknown-aros")),
        ("target_triple", json!("arm--unknown-aros")),
        ("target_triple", json!("x86_64-unknown")),
        ("target_triple", json!("i386-unknown-aros")),
        ("source_cpu", json!("aarch64")),
        ("pointer_width", json!(64)),
        ("float_abi", json!("soft")),
        ("use_mmu", json!(false)),
        ("flavour", json!("standalone")),
        ("platform_smp", json!(true)),
    ];
    for (key, value) in cases {
        let mut changed = fixture.document.clone();
        if key == "target_triple" {
            changed["target"][key] = value;
        } else {
            changed["target"]["abi"][key] = value;
        }
        assert!(load(&fixture, &changed).is_err(), "accepted {key}");
    }

    let mut no_transpiler = fixture.profile.clone();
    no_transpiler.transpiler = None;
    assert!(load_bound_sdk_consumer_contract(
        fixture.directory.path(),
        Path::new("sdk-consumer.json"),
        &no_transpiler,
    )
    .is_err());
}

#[test]
fn sdk_contract_requires_closed_identity_target_and_link_probes() {
    let fixture = make_fixture(FixtureTarget {
        name: "pc-x86_64",
        arch: "x86_64",
        source_cpu: "x86_64",
        target_triple: "x86_64-unknown-aros",
        toolchain: "llvm",
        float_abi: None,
        bootstrap: None,
        use_mmu: true,
    });
    let mut absent_probes = fixture.document.clone();
    absent_probes
        .as_object_mut()
        .unwrap()
        .remove("native_sdk_link_probes");
    assert!(load(&fixture, &absent_probes).is_err());

    for (field, value) in [
        ("schema", json!("aros-sdk-consumer-contract-v2")),
        ("profile", json!("another-profile")),
    ] {
        let mut changed = fixture.document.clone();
        changed[field] = value;
        assert!(load(&fixture, &changed).is_err(), "accepted {field} drift");
    }
    let mut undeclared = fixture.profile.clone();
    undeclared.sdk_consumer_contract = None;
    assert!(load_bound_sdk_consumer_contract(
        fixture.directory.path(),
        Path::new("sdk-consumer.json"),
        &undeclared,
    )
    .is_err());

    for key in ["host", "board", "compiler"] {
        let mut forbidden = fixture.document.clone();
        forbidden["target"]["abi"][key] = json!("invented");
        assert!(load(&fixture, &forbidden).is_err(), "accepted ABI {key}");
    }
    let mut unknown = fixture.document.clone();
    unknown["target"]["unexpected"] = json!(true);
    assert!(load(&fixture, &unknown).is_err());
    unknown = fixture.document.clone();
    unknown["unexpected"] = json!(true);
    assert!(load(&fixture, &unknown).is_err());

    let mut missing_probe_file = fixture.document.clone();
    missing_probe_file["native_sdk_link_probes"]["c"]["source"] = json!("missing.c");
    assert!(load(&fixture, &missing_probe_file).is_err());
    let mut missing_input = fixture.document.clone();
    missing_input["inputs"] = json!([]);
    assert!(load(&fixture, &missing_input).is_err());
}

#[test]
fn sdk_contract_rejects_wrong_seals_changed_bytes_and_duplicate_abi_keys() {
    let fixture = make_fixture(FixtureTarget {
        name: "rpi-aarch64",
        arch: "aarch64",
        source_cpu: "aarch64",
        target_triple: "aarch64-unknown-aros",
        toolchain: "llvm",
        float_abi: None,
        bootstrap: None,
        use_mmu: true,
    });
    let mut wrong_digest = fixture.document.clone();
    wrong_digest["inputs"][0]["sha256"] = json!("0".repeat(64));
    assert!(load(&fixture, &wrong_digest).is_err());

    fs::write(
        fixture.directory.path().join("policy.json"),
        b"{\"changed\":true}",
    )
    .unwrap();
    assert!(load(&fixture, &fixture.document).is_err());

    let fresh = make_fixture(FixtureTarget {
        name: "rpi-aarch64",
        arch: "aarch64",
        source_cpu: "aarch64",
        target_triple: "aarch64-unknown-aros",
        toolchain: "llvm",
        float_abi: None,
        bootstrap: None,
        use_mmu: true,
    });
    let mut raw = serde_json::to_string(&fresh.document).unwrap();
    raw = raw.replace(
        "\"source_cpu\":\"aarch64\"",
        "\"source_cpu\":\"aarch64\",\"source_cpu\":\"aarch64\"",
    );
    assert!(load_bytes(&fresh, raw.as_bytes()).is_err());

    let mut malformed = fresh.document.clone();
    malformed["target"]["abi"]["isa"] = json!(["not", "a", "scalar"]);
    assert!(load(&fresh, &malformed).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let alias = fresh.directory.path().join("alias.c");
        symlink(fresh.directory.path().join("probe.c"), &alias).unwrap();
        let mut symlink_input = fresh.document.clone();
        symlink_input["inputs"][2]["path"] = json!("alias.c");
        assert!(load(&fresh, &symlink_input).is_err());
    }
}
