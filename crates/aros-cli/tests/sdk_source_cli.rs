//! Actual read-only SDK source-binding CLI boundary; no compiler or source program runs.

#![cfg(unix)]

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sha256_bytes;
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use aros_toolchain::recipe::GitObjectId;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PRESET: &str = "example-sdk";
const PROFILE: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const POLICY: &[u8] = b"{}";
const C_PROBE: &[u8] = b"/* sealed SDK C probe */\n";
const CXX_PROBE: &[u8] = b"/* sealed SDK C++ probe */\n";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source_root: PathBuf,
    materialized_source_root: PathBuf,
    identity_path: PathBuf,
    identity_sha256: String,
    identity: SdkIdentity,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source_root = root.join("source");
        fs::create_dir(&source_root).unwrap();
        git(&source_root, &["init", "-q"]);

        fs::write(source_root.join("aros-targets.toml"), PROFILE).unwrap();
        fs::write(source_root.join("policy.json"), POLICY).unwrap();
        fs::write(source_root.join("probe.c"), C_PROBE).unwrap();
        fs::write(source_root.join("probe.cpp"), CXX_PROBE).unwrap();
        let abi = abi_value();
        let contract = json!({
            "schema":"aros-native-consumer-contract-v2",
            "profile":PRESET,
            "source_baseline":"0123456789abcdef0123456789abcdef01234567",
            "roots":["includes","linklibs"],
            "metamake_projection":"policy.json",
            "inputs":[
                {"path":"aros-targets.toml","sha256":sha256_bytes(PROFILE.as_bytes())},
                {"path":"policy.json","sha256":sha256_bytes(POLICY)},
                {"path":"probe.c","sha256":sha256_bytes(C_PROBE)},
                {"path":"probe.cpp","sha256":sha256_bytes(CXX_PROBE)}
            ],
            "abi":abi,
            "native_sdk_link_probes":{
                "c":{"source":"probe.c","libraries":[]},
                "cxx":{"source":"probe.cpp","libraries":[]}
            }
        });
        let contract_bytes = serde_json::to_vec(&contract).unwrap();
        fs::write(source_root.join("consumer.json"), &contract_bytes).unwrap();

        // The committed cmake subtree is deliberately excluded from the engine-free
        // projection. This synthetic fixture is not compiler or SDK qualification.
        fs::create_dir(source_root.join("cmake")).unwrap();
        fs::write(
            source_root.join("cmake/engine.cmake"),
            b"excluded engine input\n",
        )
        .unwrap();
        fs::create_dir(source_root.join("docs")).unwrap();
        fs::write(
            source_root.join("docs/source.txt"),
            b"committed source fixture\n",
        )
        .unwrap();
        git(&source_root, &["add", "-A"]);
        git(
            &source_root,
            &["commit", "-qm", "sealed SDK source fixture"],
        );

        let committed = CommittedSourceIdentity {
            commit: GitObjectId::try_from(git_text(&source_root, &["rev-parse", "HEAD^0"]))
                .unwrap(),
            tree: GitObjectId::try_from(git_text(&source_root, &["rev-parse", "HEAD:"])).unwrap(),
        };
        let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
            source_root: source_root.clone(),
            expected_source: committed,
            output_root: root.join("materialized"),
        })
        .unwrap();

        let abi: BTreeMap<String, SdkAbiValue> = serde_json::from_value(abi_value()).unwrap();
        let identity = SdkIdentity {
            sdk_id: "example-sdk".to_owned(),
            version: "1.0.0".to_owned(),
            target: SdkTarget {
                target_triple: "riscv64-aros".to_owned(),
                abi,
            },
            source: SdkSourceIdentity {
                commit: materialized.source_commit,
                git_tree: materialized.source_tree,
                content_sha256: materialized.source_tree_sha256,
                contract_sha256: sha256_bytes(&contract_bytes),
            },
            // Explicit identity metadata, independent of the fixture commit timestamp.
            source_date_epoch: 946_684_800,
        };
        let identity_path = root.join("identity.json");
        let identity_sha256 = write_identity(&identity_path, &identity);

        let isolated_home = root.join("home");
        fs::create_dir(&isolated_home).unwrap();

        Self {
            _temporary: temporary,
            root,
            source_root,
            materialized_source_root: materialized.root,
            identity_path,
            identity_sha256,
            identity,
        }
    }

    fn write_identity(&mut self) {
        self.identity_sha256 = write_identity(&self.identity_path, &self.identity);
    }

    fn command(&self) -> Command {
        self.command_with_selections(&self.identity_sha256, Some(PRESET))
    }

    fn command_with_source_preset(&self, preset: Option<&str>) -> Command {
        self.command_with_selections(&self.identity_sha256, preset)
    }

    fn command_with_selections(&self, identity_sha256: &str, preset: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .env("AROS_HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("home/cache"))
            .env("XDG_CONFIG_HOME", self.root.join("home/config"))
            .env("XDG_DATA_HOME", self.root.join("home/data"))
            .env("XDG_STATE_HOME", self.root.join("home/state"))
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .env_remove("AROS_DIAGNOSTIC_FORMAT")
            .env_remove("AROS_UPSTREAM_URL")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .args(["--diagnostic-format", "json", "sdk", "verify-source"])
            .arg("--identity")
            .arg(&self.identity_path)
            .arg("--identity-sha256")
            .arg(identity_sha256)
            .arg("--source-dir")
            .arg(&self.source_root)
            .arg("--materialized-source-dir")
            .arg(&self.materialized_source_root);
        if let Some(preset) = preset {
            command.arg("--source-preset").arg(preset);
        }
        command
    }
}

fn write_identity(path: &Path, identity: &SdkIdentity) -> String {
    let bytes = serde_json::to_vec(identity).unwrap();
    fs::write(path, &bytes).unwrap();
    sha256_bytes(&bytes).to_string()
}

fn abi_value() -> Value {
    json!({
        "source_cpu":"riscv64",
        "target_triple":"riscv64-aros",
        "isa":"rva22u64",
        "abi":"lp64d",
        "code_model":"medany",
        "flavour":"native",
        "platform_smp":true,
        "use_mmu":true
    })
}

fn git(root: &Path, arguments: &[&str]) {
    let output = git_command(root, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = git_command(root, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_command(root: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args([
            "-c",
            "user.name=AROS SDK source CLI test",
            "-c",
            "user.email=aros-sdk-source-cli-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(arguments);
    command
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotKind {
    Directory,
    Regular(Vec<u8>),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    mode: u32,
    kind: SnapshotKind,
}

fn collect_snapshot(root: &Path, directory: &Path, entries: &mut Vec<SnapshotEntry>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            SnapshotKind::Symlink(fs::read_link(&path).unwrap())
        } else if file_type.is_dir() {
            SnapshotKind::Directory
        } else if file_type.is_file() {
            SnapshotKind::Regular(fs::read(&path).unwrap())
        } else {
            SnapshotKind::Other
        };
        entries.push(SnapshotEntry {
            path: relative,
            mode: metadata.permissions().mode() & 0o7777,
            kind,
        });
        if file_type.is_dir() {
            collect_snapshot(root, &path, entries);
        }
    }
}

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    collect_snapshot(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn assert_rejected_without_writes(mut command: Command, root: &Path) -> Output {
    let before = snapshot(root);
    let output = command.output().unwrap();
    assert!(!output.status.success(), "invalid SDK source was accepted");
    assert!(
        output.stdout.is_empty(),
        "failed command emitted a success result"
    );
    assert_eq!(
        snapshot(root),
        before,
        "rejected command changed fixture state"
    );
    output
}

fn assert_json_diagnostic(output: &Output, expected_reason: &str) {
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"][0]["severity"], "error");
    let message = diagnostic["diagnostics"][0]["message"]
        .as_str()
        .expect("JSON diagnostic message");
    assert!(
        message.contains(expected_reason),
        "command failed for an unexpected reason: expected {expected_reason:?}, got {message:?}"
    );
}

#[test]
fn sdk_verify_source_cli_binds_selected_identity_to_committed_source_and_abi() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.root);
    let output = fixture.command().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["schema"], "aros-sdk-source-binding-v2");
    assert_eq!(result["operation"], "sdk.verify-source");
    assert_eq!(
        result["assurance"],
        "local-source-binding-not-execution-or-release-admission"
    );
    assert_eq!(
        result["identity"],
        serde_json::to_value(&fixture.identity).unwrap()
    );
    assert_eq!(result["source_preset"], PRESET);
    assert_eq!(
        result["materialized_source_dir"].as_str(),
        fixture.materialized_source_root.to_str()
    );
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "successful command changed fixture state"
    );
}

#[test]
fn sdk_verify_source_cli_rejects_wrong_identity_hash_without_writes() {
    let fixture = Fixture::new();
    let wrong_hash = if fixture.identity_sha256.starts_with('0') {
        format!("1{}", &fixture.identity_sha256[1..])
    } else {
        format!("0{}", &fixture.identity_sha256[1..])
    };
    let command = fixture.command_with_selections(&wrong_hash, Some(PRESET));
    let output = assert_rejected_without_writes(command, &fixture.root);
    assert_json_diagnostic(
        &output,
        "SDK identity document differs from its independently selected SHA-256",
    );
}

#[test]
fn sdk_verify_source_cli_rejects_changed_source_and_projection_without_writes() {
    let fixture = Fixture::new();
    fs::write(
        fixture.source_root.join("docs/source.txt"),
        b"working tree no longer matches its commit\n",
    )
    .unwrap();
    let output = assert_rejected_without_writes(fixture.command(), &fixture.root);
    assert_json_diagnostic(
        &output,
        "source file size or executable bit differs from its committed blob",
    );

    let fixture = Fixture::new();
    fs::write(
        fixture.materialized_source_root.join("docs/source.txt"),
        b"materialized projection changed\n",
    )
    .unwrap();
    let output = assert_rejected_without_writes(fixture.command(), &fixture.root);
    assert_json_diagnostic(
        &output,
        "source projection differs from raw committed material",
    );
}

#[test]
fn sdk_verify_source_cli_rejects_target_and_abi_divergence_without_writes() {
    let mut fixture = Fixture::new();
    fixture.identity.target.target_triple = "riscv64-other-aros".to_owned();
    fixture.identity.target.abi.insert(
        "target_triple".to_owned(),
        SdkAbiValue::Text("riscv64-other-aros".to_owned()),
    );
    fixture.write_identity();
    let output = assert_rejected_without_writes(fixture.command(), &fixture.root);
    assert_json_diagnostic(
        &output,
        "SDK target or ABI differs from its source contract",
    );

    let mut fixture = Fixture::new();
    fixture
        .identity
        .target
        .abi
        .insert("abi".to_owned(), SdkAbiValue::Text("lp64".to_owned()));
    fixture.write_identity();
    let output = assert_rejected_without_writes(fixture.command(), &fixture.root);
    assert_json_diagnostic(
        &output,
        "SDK target or ABI differs from its source contract",
    );
}

#[test]
fn sdk_verify_source_cli_rejects_missing_required_selection_without_writes() {
    let fixture = Fixture::new();
    let mut missing = fixture.command_with_source_preset(None);
    let before = snapshot(&fixture.root);
    let output = missing.output().unwrap();
    assert!(
        !output.status.success(),
        "missing required argument was accepted"
    );
    assert_json_diagnostic(&output, "--source-preset <NAME>");
    assert_eq!(
        snapshot(&fixture.root),
        before,
        "argument error changed fixture state"
    );
}
