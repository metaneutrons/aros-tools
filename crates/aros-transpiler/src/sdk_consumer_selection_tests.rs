//! Command-boundary tests for generic source-owned SDK consumer selection.

use super::*;
use clap::Parser;
use serde_json::json;
use std::{collections::BTreeSet, fmt::Write as _, fs};

#[derive(Clone, Copy)]
struct Descriptor<'a> {
    name: &'a str,
    arch: &'a str,
    source_cpu: &'a str,
    target_triple: &'a str,
    toolchain: &'a str,
    float_abi: Option<&'a str>,
    flavour: &'a str,
    platform_smp: bool,
    use_mmu: bool,
}

struct Fixture {
    directory: tempfile::TempDir,
    digest: String,
}

impl Fixture {
    fn new(descriptor: Descriptor<'_>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut profile = format!(
            "[[targets]]\nname={:?}\narch={:?}\nplatform='fixture'\nbsp='fixture'\nsdk_consumer_contract='sdk-consumer.json'\n",
            descriptor.name, descriptor.arch
        );
        if let Some(float_abi) = descriptor.float_abi {
            writeln!(profile, "float_abi={float_abi:?}").unwrap();
        }
        write!(profile,
            "[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain={:?}\ncpu32=''\nuse_mmu={}\n[targets.bootstrap_abi]\nflavour={:?}\nplatform_smp={}\n",
            descriptor.toolchain,
            descriptor.use_mmu,
            descriptor.flavour,
            descriptor.platform_smp
        ).unwrap();
        fs::write(directory.path().join("aros-targets.toml"), &profile).unwrap();
        fs::write(directory.path().join("policy.json"), b"{}").unwrap();
        fs::write(
            directory.path().join("probe.c"),
            b"int c_probe(void) { return 0; }\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("probe.cpp"),
            b"int cxx_probe() { return 0; }\n",
        )
        .unwrap();

        let mut abi = serde_json::Map::new();
        abi.insert("source_cpu".into(), json!(descriptor.source_cpu));
        let pointer_width = match descriptor.arch {
            "riscv32" | "arm" => 32,
            _ => 64,
        };
        abi.insert("pointer_width".into(), json!(pointer_width));
        abi.insert("use_mmu".into(), json!(descriptor.use_mmu));
        abi.insert("isa".into(), json!("source-declared-isa"));
        if let Some(float_abi) = descriptor.float_abi {
            abi.insert("float_abi".into(), json!(float_abi));
        }
        abi.insert("flavour".into(), json!(descriptor.flavour));
        abi.insert("platform_smp".into(), json!(descriptor.platform_smp));
        let inputs = [
            (
                "aros-targets.toml",
                aros_common::sha256_bytes(profile.as_bytes()),
            ),
            ("policy.json", aros_common::sha256_bytes(b"{}")),
            (
                "probe.c",
                aros_common::sha256_bytes(b"int c_probe(void) { return 0; }\n"),
            ),
            (
                "probe.cpp",
                aros_common::sha256_bytes(b"int cxx_probe() { return 0; }\n"),
            ),
        ];
        let contract = json!({
            "schema": "aros-sdk-consumer-contract-v1",
            "profile": descriptor.name,
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes", "linklibs"],
            "metamake_projection": "policy.json",
            "inputs": inputs.iter().map(|(path, sha256)| json!({"path": path, "sha256": sha256})).collect::<Vec<_>>(),
            "target": {"target_triple": descriptor.target_triple, "abi": abi},
            "native_sdk_link_probes": {
                "c": {"source": "probe.c", "libraries": ["c"]},
                "cxx": {"source": "probe.cpp", "libraries": ["stdc++"]}
            }
        });
        let contract_bytes = serde_json::to_vec(&contract).unwrap();
        fs::write(directory.path().join("sdk-consumer.json"), &contract_bytes).unwrap();
        Self {
            directory,
            digest: aros_common::sha256_bytes(&contract_bytes).to_string(),
        }
    }

    fn args(&self, descriptor: Descriptor<'_>, digest: Option<&str>) -> Args {
        let mut values = vec![
            "aros-transpiler".to_owned(),
            "--source-dir".to_owned(),
            self.directory.path().display().to_string(),
            "--source-inventory-only".to_owned(),
            "--native-consumer-profile".to_owned(),
            descriptor.name.to_owned(),
            "--cpu".to_owned(),
            descriptor.source_cpu.to_owned(),
            "--platform".to_owned(),
            "fixture".to_owned(),
            "--family".to_owned(),
            String::new(),
            "--variant".to_owned(),
            String::new(),
            "--toolchain".to_owned(),
            descriptor.toolchain.to_owned(),
            "--cpu32".to_owned(),
            String::new(),
            "--use-mmu".to_owned(),
            (if descriptor.use_mmu { "1" } else { "0" }).to_owned(),
        ];
        values.extend([
            "--float-abi".to_owned(),
            descriptor.float_abi.unwrap_or("").to_owned(),
        ]);
        if let Some(digest) = digest {
            values.extend([
                "--native-consumer-contract-sha256".to_owned(),
                digest.to_owned(),
            ]);
        }
        Args::try_parse_from(values).unwrap()
    }
}

fn context(args: &Args) -> TargetContext {
    TargetContext {
        cpu: args.cpu.clone(),
        platform: args.platform.clone(),
        family: args.family.clone(),
        variant: args.variant.clone(),
        toolchain: args.toolchain.clone(),
        cpu32: args.cpu32.clone(),
        use_mmu: args.use_mmu.clone(),
        float_abi: args.float_abi.clone(),
        mesa_version: args.mesa_version.clone(),
        ..TargetContext::default()
    }
}

#[test]
fn generic_sdk_profiles_select_pc_arm_aarch64_and_gnu_descriptors() {
    let descriptors = [
        Descriptor {
            name: "pc-x86_64",
            arch: "x86_64",
            source_cpu: "x86_64",
            target_triple: "x86_64-unknown-aros",
            toolchain: "llvm",
            float_abi: None,
            flavour: "native",
            platform_smp: false,
            use_mmu: true,
        },
        Descriptor {
            name: "arm-raspi",
            arch: "arm",
            source_cpu: "arm",
            target_triple: "arm-unknown-aros",
            toolchain: "llvm",
            float_abi: Some("hard"),
            flavour: "native",
            platform_smp: false,
            use_mmu: true,
        },
        Descriptor {
            name: "rpi-aarch64",
            arch: "aarch64",
            source_cpu: "aarch64",
            target_triple: "aarch64-unknown-aros",
            toolchain: "llvm",
            float_abi: None,
            flavour: "native",
            platform_smp: true,
            use_mmu: true,
        },
        Descriptor {
            name: "rv32-esp32p4",
            arch: "riscv32",
            source_cpu: "riscv",
            target_triple: "riscv-aros",
            toolchain: "gnu",
            float_abi: Some("ilp32f"),
            flavour: "standalone",
            platform_smp: false,
            use_mmu: false,
        },
    ];

    for descriptor in descriptors {
        let fixture = Fixture::new(descriptor);
        let args = fixture.args(descriptor, Some(&fixture.digest));
        let target = context(&args);
        let selection = load_native_selection(&args, Some(&target))
            .unwrap_or_else(|error| panic!("{}: {error}", descriptor.name))
            .expect("SDK consumer selection");
        let LoadedNativeSelection::SdkConsumer(loaded) = &selection else {
            panic!("{} selected the wrong consumer family", descriptor.name);
        };
        assert_eq!(selection.kind(), "sdk-consumer");
        assert!(selection.build_contract().is_none());
        assert_eq!(selection.invocation().source_cpu, descriptor.source_cpu);
        assert_eq!(
            loaded.contract.target.target_triple,
            descriptor.target_triple
        );
        assert_eq!(selection.sha256().as_str(), fixture.digest);
        assert_eq!(
            selection.roots(&DependencyGraph::new(), &target).unwrap(),
            vec!["includes".to_owned(), "linklibs".to_owned()]
        );
    }
}

#[test]
fn sdk_consumer_selection_rejects_digest_selector_and_binding_drift() {
    let descriptor = Descriptor {
        name: "pc-x86_64",
        arch: "x86_64",
        source_cpu: "x86_64",
        target_triple: "x86_64-unknown-aros",
        toolchain: "llvm",
        float_abi: None,
        flavour: "native",
        platform_smp: false,
        use_mmu: true,
    };
    let fixture = Fixture::new(descriptor);
    let args = fixture.args(descriptor, Some(&fixture.digest));
    assert!(load_native_selection(&args, Some(&context(&args))).is_ok());

    let wrong_digest = fixture.args(descriptor, Some(&"0".repeat(64)));
    assert!(load_native_selection(&wrong_digest, Some(&context(&wrong_digest))).is_err());

    let mut changed_cpu = context(&args);
    changed_cpu.cpu = Some("arm".into());
    assert!(load_native_selection(&args, Some(&changed_cpu)).is_err());
    let mut changed_toolchain = context(&args);
    changed_toolchain.toolchain = Some("gnu".into());
    assert!(load_native_selection(&args, Some(&changed_toolchain)).is_err());
    let mut changed_float_abi = context(&args);
    changed_float_abi.float_abi = Some("hard".into());
    assert!(load_native_selection(&args, Some(&changed_float_abi)).is_err());
    let mut omitted_float_abi = context(&args);
    omitted_float_abi.float_abi = None;
    assert!(load_native_selection(&args, Some(&omitted_float_abi)).is_ok());

    let profile_path = fixture.directory.path().join("aros-targets.toml");
    let original = fs::read_to_string(&profile_path).unwrap();
    fs::write(
        &profile_path,
        original.replace(
            "sdk_consumer_contract='sdk-consumer.json'",
            "native_consumer_contract='native.json'\nsdk_consumer_contract='sdk-consumer.json'",
        ),
    )
    .unwrap();
    let error = load_native_selection(&args, Some(&context(&args)))
        .err()
        .expect("dual consumer bindings must be rejected")
        .to_string();
    assert!(error.contains("exactly one"), "{error}");

    fs::write(
        &profile_path,
        original.replace("sdk_consumer_contract='sdk-consumer.json'\n", ""),
    )
    .unwrap();
    let error = load_native_selection(&args, Some(&context(&args)))
        .err()
        .expect("missing consumer binding must be rejected")
        .to_string();
    assert!(error.contains("exactly one"), "{error}");
}

#[test]
fn consumer_validation_json_keeps_common_keys_and_selects_abi_or_target() {
    let legacy: aros_common::native_consumer_contract::NativeConsumerContract =
        serde_json::from_value(json!({
            "schema": "aros-native-consumer-contract-v1",
            "profile": "legacy-profile",
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes"],
            "inputs": [{"path": "policy.json", "sha256": "0".repeat(64)}],
            "metamake_projection": "policy.json",
            "abi": {
                "source_cpu": "arm",
                "target_triple": "arm-unknown-aros",
                "isa": "armv7-a",
                "abi": "hard",
                "code_model": "small",
                "flavour": "native",
                "platform_smp": false,
                "use_mmu": true
            }
        }))
        .unwrap();
    let legacy = LoadedNativeSelection::Consumer(Box::new(
        aros_common::native_consumer_contract::LoadedNativeConsumerContract {
            path: "legacy.json".into(),
            sha256: aros_common::Sha256Digest::parse(&"1".repeat(64)).unwrap(),
            contract: legacy,
        },
    ));
    let generic: aros_common::sdk_consumer_contract::SdkConsumerContract =
        serde_json::from_value(json!({
            "schema": "aros-sdk-consumer-contract-v1",
            "profile": "generic-profile",
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes"],
            "inputs": [{"path": "policy.json", "sha256": "0".repeat(64)}],
            "metamake_projection": "policy.json",
            "target": {
                "target_triple": "x86_64-unknown-aros",
                "abi": {
                    "source_cpu": "x86_64",
                    "pointer_width": 64,
                    "isa": "x86_64-v3"
                }
            },
            "native_sdk_link_probes": {
                "c": {"source": "probe.c", "libraries": []},
                "cxx": {"source": "probe.cpp", "libraries": []}
            }
        }))
        .unwrap();
    let generic = LoadedNativeSelection::SdkConsumer(Box::new(
        aros_common::sdk_consumer_contract::LoadedSdkConsumerContract {
            path: "generic.json".into(),
            sha256: aros_common::Sha256Digest::parse(&"2".repeat(64)).unwrap(),
            contract: generic,
        },
    ));

    let common = |selection: &LoadedNativeSelection, path: &str| {
        crate::consumer_validation::validation_document(
            selection,
            "/source",
            path,
            "build/sdk/include",
            false,
        )
        .unwrap()
    };
    let legacy_document = common(&legacy, "legacy.json");
    let generic_document = common(&generic, "generic.json");
    let common_keys = [
        "qualification",
        "source_dir",
        "contract_path",
        "contract_sha256",
        "profile",
        "sdk_include_relative",
        "exec_smp",
        "input_paths",
    ];
    for key in common_keys {
        let legacy_value = &legacy_document[key];
        let generic_value = &generic_document[key];
        if key == "contract_path" || key == "contract_sha256" || key == "profile" {
            continue;
        }
        assert_eq!(legacy_value, generic_value, "common key {key}");
    }
    assert_eq!(
        legacy_document["schema"].as_str(),
        Some("aros-native-consumer-validation-v1")
    );
    assert_eq!(
        generic_document["schema"].as_str(),
        Some("aros-sdk-consumer-validation-v1")
    );
    assert_eq!(
        legacy_document["abi"],
        json!({
            "source_cpu": "arm",
            "target_triple": "arm-unknown-aros",
            "isa": "armv7-a",
            "abi": "hard",
            "code_model": "small",
            "flavour": "native",
            "platform_smp": false,
            "use_mmu": true
        })
    );
    assert_eq!(
        generic_document["target"],
        json!({
            "target_triple": "x86_64-unknown-aros",
            "abi": {
                "source_cpu": "x86_64",
                "pointer_width": 64,
                "isa": "x86_64-v3"
            }
        })
    );
    let legacy_keys = legacy_document
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let generic_keys = generic_document
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected_common_keys = BTreeSet::from([
        "qualification".to_owned(),
        "source_dir".to_owned(),
        "contract_path".to_owned(),
        "contract_sha256".to_owned(),
        "profile".to_owned(),
        "sdk_include_relative".to_owned(),
        "exec_smp".to_owned(),
        "input_paths".to_owned(),
        "schema".to_owned(),
    ]);
    let mut legacy_common_keys = legacy_keys.clone();
    let mut generic_common_keys = generic_keys.clone();
    legacy_common_keys.remove("abi");
    generic_common_keys.remove("target");
    assert_eq!(legacy_common_keys, expected_common_keys);
    assert_eq!(generic_common_keys, expected_common_keys);
    assert!(legacy_keys.contains("abi") && !legacy_keys.contains("target"));
    assert!(generic_keys.contains("target") && !generic_keys.contains("abi"));
    assert_eq!(legacy_keys.len(), 10);
    assert_eq!(generic_keys.len(), 10);
}
