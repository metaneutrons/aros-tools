//! Synthetic process-boundary policy probes; no compiler or release is qualified.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::sha256_bytes;
use serde_json::{json, Value};

const PRESET: &str = "source-rv32";
const COMPILER: &str = "compiler-rv32";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    recipe: Value,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q"]);
        let profiles = format!(
            "[[targets]]\nname='{PRESET}'\ntoolchain_profile='{COMPILER}'\narch='riscv32'\nplatform='esp32p4'\nbsp='esp32p4'\nfloat_abi='ilp32f'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily='riscv'\nvariant='esp32p4'\ntoolchain='gnu'\ncpu32='riscv'\nuse_mmu=false\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n"
        );
        let probe = b"int main(void) { return 0; }\n";
        for (name, bytes) in [
            ("aros-targets.toml", profiles.as_bytes()),
            ("policy.json", b"{}\n".as_slice()),
            ("probe.c", probe.as_slice()),
            ("probe.cpp", probe.as_slice()),
        ] {
            fs::write(source.join(name), bytes).unwrap();
        }
        let consumer = json!({
            "schema":"aros-native-consumer-contract-v2", "profile":PRESET,
            "source_baseline":"1".repeat(40), "roots":["includes","linklibs"],
            "inputs":[
                {"path":"aros-targets.toml","sha256":sha256_bytes(profiles.as_bytes())},
                {"path":"policy.json","sha256":sha256_bytes(b"{}\n")},
                {"path":"probe.c","sha256":sha256_bytes(probe)},
                {"path":"probe.cpp","sha256":sha256_bytes(probe)}
            ],
            "abi":{"source_cpu":"riscv","target_triple":"riscv-aros",
                "isa":"rv32imafc_zicsr_zifencei_zaamo_zalrsc","abi":"ilp32f",
                "code_model":"medany","flavour":"native","platform_smp":true,"use_mmu":false},
            "metamake_projection":"policy.json",
            "native_sdk_link_probes":{
                "c":{"source":"probe.c","libraries":[]},
                "cxx":{"source":"probe.cpp","libraries":[]}}
        });
        fs::write(
            source.join("consumer.json"),
            serde_json::to_vec(&consumer).unwrap(),
        )
        .unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-qm", "test: source policy fixture"]);
        let producer_profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2", "family":"gnu", "upstream_commit":"2".repeat(40),
            "profiles":[{"name":COMPILER,"configure_target":"esp32p4-riscv",
                "upstream_output_target":"esp32p4-riscv-smp","target_triple":"riscv-aros",
                "cpu":"riscv","platform":"esp32p4","float_abi":"ilp32f",
                "capabilities":["c","cxx","libgcc","libstdcxx","libsupcxx","standalone-collector"],
                "target":{"schema":"aros-riscv-target-v1",
                    "isa":"rv32imafc_zicsr_zifencei_zaamo_zalrsc","abi":"ilp32f",
                    "code_model":"medany",
                    "architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0_zca1p0_zcf1p0",
                    "unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0}}]
        })).unwrap();
        fs::write(root.join("profiles.json"), &producer_profiles).unwrap();
        let recipe = json!({
            "schema":"aros-toolchain-recipe-v2",
            "source_commit":git(&source, &["rev-parse","HEAD"]),
            "source_tree":git(&source, &["rev-parse","HEAD:"]),
            "producer_commit":"3".repeat(40),"producer_tree":"4".repeat(40),
            "tools_commit":"5".repeat(40),"tools_tree":"6".repeat(40),
            "source_date_epoch":0,"source_lock_sha256":sha256_bytes(b"synthetic lock"),
            "profiles_sha256":sha256_bytes(&producer_profiles),"patches":[]
        });
        let mut fixture = Self {
            _temporary: temporary,
            root,
            source,
            recipe,
        };
        fixture.save_recipe();
        fixture
    }

    fn save_recipe(&mut self) {
        self.recipe.as_object_mut().unwrap().remove("recipe_sha256");
        self.recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&self.recipe).unwrap()
        ));
        fs::write(
            self.root.join("recipe.json"),
            serde_json::to_vec(&self.recipe).unwrap(),
        )
        .unwrap();
    }

    fn run(&self, source_policy: bool, preset: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .args([
                "--diagnostic-format=json",
                "toolchain",
                "producer",
                "profile",
                "--recipe",
                "recipe.json",
                "--profiles",
                "profiles.json",
                "--preset",
                COMPILER,
                "--format",
                "json",
            ]);
        if source_policy {
            command.args(["--source-dir", "source", "--source-preset", preset]);
        }
        command.output().unwrap()
    }
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Policy fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn assert_diagnostic(output: &Output, reason: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    let diagnostics = diagnostic["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["severity"], "error");
    assert!(
        diagnostics[0]["message"].as_str().unwrap().contains(reason),
        "{diagnostic}"
    );
}

#[test]
fn profile_preserves_legacy_output_and_adds_explicit_source_bound_sdk_policy() {
    let fixture = Fixture::new();
    let legacy = fixture.run(false, PRESET);
    assert!(
        legacy.status.success(),
        "{}",
        String::from_utf8_lossy(&legacy.stderr)
    );
    let legacy: Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert!(legacy.get("source_policy").is_none());

    let selected = fixture.run(true, PRESET);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    assert!(selected.stderr.is_empty());
    let mut selected: Value = serde_json::from_slice(&selected.stdout).unwrap();
    let policy = selected
        .as_object_mut()
        .unwrap()
        .remove("source_policy")
        .unwrap();
    assert_eq!(selected, legacy);
    assert_eq!(policy["source_commit"], fixture.recipe["source_commit"]);
    assert_eq!(policy["source_tree"], fixture.recipe["source_tree"]);
    assert_eq!(policy["source_preset"], PRESET);
    assert_eq!(policy["compiler_profile"], COMPILER);
    assert_eq!(policy["native_sdk_required"], true);
    assert_eq!(
        policy["native_consumer"]["schema"],
        "aros-native-consumer-contract-v2"
    );
}

#[test]
fn invalid_source_selection_cannot_emit_an_sdk_policy() {
    let mut fixture = Fixture::new();
    let unknown = fixture.run(true, "missing");
    assert_diagnostic(&unknown, "selected source preset is not declared");

    fixture.recipe["source_commit"] = json!("a".repeat(40));
    fixture.save_recipe();
    let wrong = fixture.run(true, PRESET);
    assert_diagnostic(&wrong, "independently selected commit and tree");
}

#[test]
fn uncommitted_sdk_probe_is_rejected_without_source_writes() {
    let fixture = Fixture::new();
    let changed = b"uncommitted probe substitution\n";
    fs::write(fixture.source.join("probe.c"), changed).unwrap();
    let status_before = git(&fixture.source, &["status", "--porcelain"]);
    let output = fixture.run(true, PRESET);
    assert_diagnostic(&output, "sealed inputs are invalid");
    assert_eq!(fs::read(fixture.source.join("probe.c")).unwrap(), changed);
    assert_eq!(
        git(&fixture.source, &["status", "--porcelain"]),
        status_before
    );
}
