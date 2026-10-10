//! Command-boundary tests for source-owned native consumer selection.

use super::*;
use clap::Parser;
use serde_json::json;
use std::fs;

const PROFILE: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const ROOTS: [&str; 2] = ["includes", "linklibs"];
type SelectorMutation = (&'static str, fn(&mut TargetContext));

struct Fixture {
    directory: tempfile::TempDir,
    digest: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("aros-targets.toml"), PROFILE).unwrap();
        fs::write(directory.path().join("policy.json"), "{}").unwrap();
        let document = json!({
            "schema": "aros-native-consumer-contract-v1",
            "profile": "example-sdk",
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ROOTS,
            "metamake_projection": "policy.json",
            "inputs": [
                { "path": "aros-targets.toml", "sha256": aros_common::sha256_bytes(PROFILE.as_bytes()) },
                { "path": "policy.json", "sha256": aros_common::sha256_bytes(b"{}") }
            ],
            "abi": {
                "source_cpu": "riscv64",
                "target_triple": "riscv64-aros",
                "isa": "rva22u64",
                "abi": "lp64d",
                "code_model": "medany",
                "flavour": "native",
                "platform_smp": true,
                "use_mmu": true
            }
        });
        let contract_path = directory.path().join("consumer.json");
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(&contract_path, &bytes).unwrap();
        let digest = aros_common::sha256_bytes(&bytes).to_string();
        Self { directory, digest }
    }

    fn args(&self, profile: &str, digest: Option<&str>) -> Args {
        let mut values = vec![
            "aros-transpiler".to_owned(),
            "--source-dir".to_owned(),
            self.directory.path().display().to_string(),
            "--source-inventory-only".to_owned(),
            "--native-consumer-profile".to_owned(),
            profile.to_owned(),
            "--cpu".to_owned(),
            "riscv64".to_owned(),
            "--platform".to_owned(),
            "opensbi".to_owned(),
            "--family".to_owned(),
            String::new(),
            "--variant".to_owned(),
            String::new(),
            "--toolchain".to_owned(),
            "gnu".to_owned(),
            "--cpu32".to_owned(),
            String::new(),
            "--use-mmu".to_owned(),
            "1".to_owned(),
            "--float-abi".to_owned(),
            "lp64d".to_owned(),
            "--mesa-version".to_owned(),
            "26.0.0".to_owned(),
        ];
        if let Some(digest) = digest {
            values.extend([
                "--native-consumer-contract-sha256".to_owned(),
                digest.to_owned(),
            ]);
        }
        Args::try_parse_from(values).unwrap()
    }

    fn selection(args: &Args) -> Result<Option<LoadedNativeSelection>> {
        let target = context(args);
        load_native_selection(args, Some(&target))
    }

    fn without_consumer_binding() -> Self {
        let fixture = Self::new();
        let profile = PROFILE.replace("native_consumer_contract='consumer.json'\n", "");
        fs::write(fixture.directory.path().join("aros-targets.toml"), profile).unwrap();
        fixture
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
fn cli_rejects_conflicting_modes_and_cross_mode_digests() {
    let digest = "0".repeat(64);
    for argv in [
        vec![
            "aros-transpiler",
            "--native-profile",
            "example-sdk",
            "--native-consumer-profile",
            "example-sdk",
        ],
        vec![
            "aros-transpiler",
            "--native-profile",
            "example-sdk",
            "--native-consumer-contract-sha256",
            digest.as_str(),
        ],
        vec![
            "aros-transpiler",
            "--native-consumer-profile",
            "example-sdk",
            "--native-contract-sha256",
            digest.as_str(),
        ],
        vec!["aros-transpiler", "--source-inventory-only"],
        vec!["aros-transpiler", "--validate-native-consumer-only"],
        vec![
            "aros-transpiler",
            "--native-profile",
            "example-sdk",
            "--validate-native-consumer-only",
        ],
        vec![
            "aros-transpiler",
            "--native-consumer-profile",
            "example-sdk",
            "--validate-native-consumer-only",
            "--source-inventory-only",
        ],
    ] {
        assert!(Args::try_parse_from(argv).is_err());
    }
}

#[test]
fn consumer_selection_requires_a_source_profile_and_consumer_binding() {
    let fixture = Fixture::new();
    let unknown = fixture.args("missing-sdk", Some(&fixture.digest));
    let error = Fixture::selection(&unknown)
        .err()
        .expect("unknown profile rejected")
        .to_string();
    assert!(error.contains("native profile is not declared"), "{error}");

    let fixture = Fixture::without_consumer_binding();
    let args = fixture.args("example-sdk", Some(&fixture.digest));
    let error = Fixture::selection(&args)
        .err()
        .expect("unbound consumer rejected")
        .to_string();
    assert!(error.contains("no native consumer contract"), "{error}");
}

#[test]
fn consumer_selection_checks_digest_and_every_selector() {
    let fixture = Fixture::new();
    let args = fixture.args("example-sdk", Some(&fixture.digest));
    assert!(Fixture::selection(&args).unwrap().is_some());

    let wrong_digest = fixture.args("example-sdk", Some(&"0".repeat(64)));
    assert!(Fixture::selection(&wrong_digest).is_err());

    let mismatches: [SelectorMutation; 9] = [
        ("cpu", |target| target.cpu = Some("riscv".into())),
        ("platform", |target| target.platform = Some("pc".into())),
        ("family", |target| target.family = Some("gnu".into())),
        ("variant", |target| target.variant = Some("smp".into())),
        ("toolchain", |target| target.toolchain = Some("llvm".into())),
        ("cpu32", |target| target.cpu32 = Some("i386".into())),
        ("use_mmu", |target| target.use_mmu = Some("0".into())),
        ("float_abi", |target| target.float_abi = Some("hard".into())),
        ("mesa_version", |target| {
            target.mesa_version = Some("25.0.0".into());
        }),
    ];
    for (name, change) in mismatches {
        let mut target = context(&args);
        change(&mut target);
        assert!(
            load_native_selection(&args, Some(&target)).is_err(),
            "changed {name} was accepted"
        );
    }
}

#[test]
fn explicit_consumer_keeps_literal_roots_and_has_no_build_contract() {
    let fixture = Fixture::new();
    let args = fixture.args("example-sdk", Some(&fixture.digest));
    let target = context(&args);
    let selection = Fixture::selection(&args).unwrap().unwrap();
    assert_eq!(selection.kind(), "consumer");
    assert!(selection.build_contract().is_none());
    assert_eq!(
        selection.roots(&DependencyGraph::new(), &target).unwrap(),
        ROOTS.map(str::to_owned)
    );
    assert_eq!(selection.invocation().profile, "example-sdk");
    assert_eq!(selection.sha256().as_str(), fixture.digest);
}

#[test]
fn explicitly_empty_selectors_are_distinct_from_omitted_values() {
    let fixture = Fixture::new();
    let args = fixture.args("example-sdk", Some(&fixture.digest));
    assert_eq!(args.family.as_deref(), Some(""));
    assert_eq!(args.variant.as_deref(), Some(""));
    assert_eq!(args.cpu32.as_deref(), Some(""));
    assert!(Fixture::selection(&args).unwrap().is_some());

    for (name, selector) in [("family", 0_u8), ("variant", 1_u8), ("cpu32", 2_u8)] {
        let mut target = context(&args);
        match selector {
            0 => target.family = None,
            1 => target.variant = None,
            _ => target.cpu32 = None,
        }
        assert!(
            load_native_selection(&args, Some(&target)).is_err(),
            "omitted {name} was treated as an explicit empty selector"
        );
    }
}
