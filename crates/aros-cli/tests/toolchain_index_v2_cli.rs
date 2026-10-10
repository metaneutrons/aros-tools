//! Process-boundary coverage for the explicit compiler-family-v2 index stages.
//!
//! All packages are generated from small synthetic inputs. These cases test
//! CLI binding and mutation boundaries; they are not compiler or release proof.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use aros_common::{sha256_bytes, ArosCompilerIdentity};
use aros_toolchain::canonical;
use aros_toolchain::package::{package_with_format, PackageFormat, PackageRequest};
use aros_toolchain::profiles::Profile;
use aros_toolchain::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};
use aros_toolchain::source_lock::{CompilerFamily, SourceLock};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

const RELEASE_ID: &str = "release-2026.10-cli-fixture";
const GNU_SOURCE_COMMIT: &str = "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";
const LLVM_SOURCE_COMMIT: &str = "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2";
const PRODUCER_COMMIT: &str = "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3";
const TOOLS_COMMIT: &str = "b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4";
const PRODUCER_TREE: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
const TOOLS_TREE: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";
const PRIVATE_MARKER: &str = "synthetic-private-index-cli-marker";
const SYNTHETIC_PROVENANCE: &[u8] =
    b"{\"schema\":\"synthetic-unverified-provenance-fixture-v1\"}\n";

#[derive(Clone, Debug, PartialEq, Eq)]
enum MemberSnapshot {
    File {
        bytes: Vec<u8>,
        mode: u32,
        inode: u64,
        links: u64,
        modified: Option<SystemTime>,
    },
    Symlink {
        target: PathBuf,
        mode: u32,
        inode: u64,
    },
    Directory {
        mode: u32,
        inode: u64,
        modified: Option<SystemTime>,
    },
    Other {
        mode: u32,
        inode: u64,
        size: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectorySnapshot {
    modified: Option<SystemTime>,
    members: BTreeMap<String, MemberSnapshot>,
}

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    base_preindex: PathBuf,
    lanes: Value,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let base_preindex = root.join("base-preindex");
        fs::create_dir(&base_preindex).unwrap();

        let (collection, documents) = target_scope_input_documents();
        let collection_bytes = serde_json::to_vec(&collection).unwrap();
        let inputs = ReleaseInputs::parse(&collection_bytes, &documents).unwrap();
        let actual_profiles = inputs
            .groups()
            .iter()
            .flat_map(|group| {
                group
                    .profiles()
                    .entries()
                    .iter()
                    .map(|profile| (group.id().to_owned(), profile.name().to_owned()))
            })
            .collect::<BTreeSet<_>>();
        let expected_profiles = [
            ("gnu-riscv", "rv32-esp32p4"),
            ("llvm-pc", "pc-x86_64"),
            ("llvm-pc", "arm-raspi"),
            ("llvm-pc", "rpi-aarch64"),
        ]
        .into_iter()
        .map(|(group, profile)| (group.to_owned(), profile.to_owned()))
        .collect::<BTreeSet<_>>();
        assert_eq!(actual_profiles, expected_profiles);
        assert_eq!(ACTIVE_HOSTS.len(), 3);
        fs::write(
            base_preindex.join("toolchain-release-inputs-v2.json"),
            &collection_bytes,
        )
        .unwrap();
        for (name, bytes) in &documents {
            fs::write(base_preindex.join(name), bytes).unwrap();
        }
        fs::write(
            base_preindex.join("toolchain-manifest-v2.schema.json"),
            include_bytes!("../../aros-common/tests/fixtures/toolchain-manifest-v2.schema.json"),
        )
        .unwrap();
        fs::write(
            base_preindex.join("tree-digest-v1.fixture.json"),
            include_bytes!("../../aros-common/tests/fixtures/tree-digest-v1.fixture.json"),
        )
        .unwrap();

        let mut lanes = BTreeMap::new();
        for group in inputs.groups() {
            for profile in group.profiles().entries() {
                for host in ACTIVE_HOSTS {
                    let asset = asset_name(group.source_lock(), profile, host);
                    let candidate = root.join(format!(
                        "candidate-{}-{host}-{}",
                        group.id(),
                        profile.name()
                    ));
                    fs::create_dir_all(&candidate).unwrap();
                    fs::write(
                        candidate.join("fixture-input"),
                        b"synthetic package payload; never execute\n",
                    )
                    .unwrap();
                    if group.source_lock().family() == CompilerFamily::Gnu {
                        write_gnu_tool_layout(&candidate, group.source_lock(), profile);
                    }

                    let environment = build_environment(host, &asset);
                    let request = PackageRequest {
                        candidate_root: candidate,
                        output_dir: root.join(format!(
                            "package-{}-{host}-{}",
                            group.id(),
                            profile.name()
                        )),
                        release_id: RELEASE_ID.to_owned(),
                        host: (*host).to_owned(),
                        recipe: group.recipe().clone(),
                        source_lock: group.source_lock().clone(),
                        profile: profile.clone(),
                        build_environment: environment.clone(),
                        forbidden_prefixes: Vec::new(),
                    };
                    let package = package_with_format(&request, PackageFormat::CompilerFamilyV2)
                        .expect("synthetic compiler-family package is valid");
                    assert_eq!(
                        package.archive.file_name().unwrap().to_str().unwrap(),
                        asset.as_str()
                    );
                    for source in [
                        &package.archive,
                        &package.manifest,
                        &package.checksum,
                        &package.sbom,
                    ] {
                        fs::copy(source, base_preindex.join(source.file_name().unwrap())).unwrap();
                    }
                    lanes.insert(
                        asset,
                        json!({
                            "build_environment": environment,
                            "required_paths": ["fixture-input"]
                        }),
                    );
                }
            }
        }
        assert_eq!(lanes.len(), 12);
        assert_eq!(fs::read_dir(&base_preindex).unwrap().count(), 57);
        let lanes = json!({"schema":"aros-toolchain-index-lanes-v1", "lanes":lanes});
        Self {
            _temporary: temporary,
            root,
            base_preindex,
            lanes,
        }
    }

    fn lane_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(format!("{name}-lanes.json"));
        fs::write(&path, bytes).unwrap();
        path
    }

    fn lane_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.lanes).unwrap()
    }

    fn copy_preindex(&self, name: &str) -> PathBuf {
        copy_directory(&self.base_preindex, &self.root.join(name))
    }

    fn command(
        &self,
        directory: &Path,
        lane_inputs: &Path,
        subject_manifest: &Path,
        stage: &str,
        release_format: Option<&str>,
        subject_manifest_sha256: Option<&str>,
    ) -> Command {
        let mut command = Command::new(aros());
        command
            .current_dir(&self.root)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .env_remove("AROS_OFFLINE")
            .env_remove("AROS_DIAGNOSTIC_FORMAT")
            .args([
                "--diagnostic-format=json",
                "toolchain",
                "producer",
                "index",
                "--directory",
            ])
            .arg(directory)
            .args([
                "--release-id",
                RELEASE_ID,
                "--base-url",
                "https://example.invalid/aros/releases/2026.10",
            ]);
        if let Some(release_format) = release_format {
            command.args(["--release-format", release_format]);
        }
        command
            .arg("--lane-inputs")
            .arg(lane_inputs)
            .arg("--subject-manifest")
            .arg(subject_manifest)
            .args(["--stage", stage, "--format", "json"]);
        if let Some(subject_manifest_sha256) = subject_manifest_sha256 {
            command
                .arg("--subject-manifest-sha256")
                .arg(subject_manifest_sha256);
        }
        command
    }

    fn invoke(
        &self,
        directory: &Path,
        lane_inputs: &Path,
        subject_manifest: &Path,
        stage: &str,
        release_format: Option<&str>,
        subject_manifest_sha256: Option<&str>,
    ) -> Output {
        self.command(
            directory,
            lane_inputs,
            subject_manifest,
            stage,
            release_format,
            subject_manifest_sha256,
        )
        .output()
        .expect("aros producer index command executes")
    }
}

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn copy_directory(source: &Path, destination: &Path) -> PathBuf {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).unwrap();
        if metadata.file_type().is_symlink() {
            symlink(fs::read_link(source).unwrap(), target).unwrap();
        } else if metadata.is_file() {
            fs::copy(source, target).unwrap();
        } else {
            panic!("fixture contains unsupported nested inventory entry");
        }
    }
    destination.canonicalize().unwrap()
}

fn invoke_with_timeout(mut command: Command, timeout: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("aros process starts");
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait_with_output();
            panic!("aros hung while reading a FIFO lane-input file");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn successful_json(output: &Output, operation: &str) -> Value {
    assert!(
        output.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{operation} wrote diagnostics on success: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("successful CLI stage emits JSON")
}

fn assert_failure_is_sanitized(
    output: &Output,
    directory: &Path,
    expected_reason: &str,
    marker: Option<&str>,
) {
    assert!(
        !output.status.success(),
        "invalid CLI input unexpectedly succeeded"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        stdout.is_empty(),
        "failure emitted success output: {stdout}"
    );
    let diagnostic: Value = serde_json::from_slice(&output.stderr)
        .expect("failure emits the requested JSON diagnostic format");
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    let messages = diagnostic["diagnostics"]
        .as_array()
        .expect("diagnostic set contains an array")
        .iter()
        .filter_map(|item| item["message"].as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains(expected_reason)),
        "failure had an unexpected reason; expected {expected_reason:?}, got {messages:?}"
    );
    assert!(
        !combined.contains(directory.to_string_lossy().as_ref()),
        "failure diagnostic disclosed the release path: {combined}"
    );
    if let Some(marker) = marker {
        assert!(
            !combined.contains(marker),
            "failure diagnostic disclosed private fixture bytes: {combined}"
        );
    }
}

fn snapshot(directory: &Path) -> DirectorySnapshot {
    let mut members = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = entry.file_name().into_string().unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let state = if metadata.file_type().is_symlink() {
            MemberSnapshot::Symlink {
                target: fs::read_link(&path).unwrap(),
                mode: metadata.mode(),
                inode: metadata.ino(),
            }
        } else if metadata.is_file() {
            MemberSnapshot::File {
                bytes: fs::read(&path).unwrap(),
                mode: metadata.mode(),
                inode: metadata.ino(),
                links: metadata.nlink(),
                modified: metadata.modified().ok(),
            }
        } else if metadata.is_dir() {
            MemberSnapshot::Directory {
                mode: metadata.mode(),
                inode: metadata.ino(),
                modified: metadata.modified().ok(),
            }
        } else {
            MemberSnapshot::Other {
                mode: metadata.mode(),
                inode: metadata.ino(),
                size: metadata.len(),
            }
        };
        members.insert(name, state);
    }
    DirectorySnapshot {
        modified: fs::symlink_metadata(directory).unwrap().modified().ok(),
        members,
    }
}

fn target(abi: &str, architecture: &str, isa: &str) -> Value {
    json!({
        "schema":"aros-riscv-target-v1", "isa":isa, "abi":abi,
        "code_model":"medany", "architecture":architecture,
        "unaligned_access":false, "atomic_abi":0, "x3_reg_usage":0
    })
}

fn gnu_profiles() -> Vec<u8> {
    let profile = json!({
        "name":"rv32-esp32p4", "configure_target":"esp32p4",
        "upstream_output_target":"esp32p4", "target_triple":"riscv-aros",
        "cpu":"riscv", "platform":"esp32p4", "float_abi":"ilp32f",
        "capabilities":["c", "libgcc", "standalone-collector"],
        "target":target("ilp32f", "rv32i2p1_m2p0_a2p1_f2p2_c2p0", "rv32imafc")
    });
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-profiles-v2", "family":"gnu",
        "upstream_commit":GNU_SOURCE_COMMIT, "profiles":[profile]
    }))
    .unwrap()
}

fn gnu_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-source-lock-v3", "family":"gnu", "version":"16.2.0",
        "sources":[
            {"component":"gcc","version":"16.2.0","purpose":"toolchain-component","filename":"gcc.tar.xz","url":"https://example.invalid/gcc.tar.xz","sha256":"1".repeat(64),"size":1},
            {"component":"binutils","version":"2.47","purpose":"toolchain-component","filename":"binutils.tar.xz","url":"https://example.invalid/binutils.tar.xz","sha256":"2".repeat(64),"size":1}
        ],
        "host_python_packages":[{"name":"mako","version":"1.3.10","filename":"mako.tar.gz","url":"https://example.invalid/mako.tar.gz","sha256":"3".repeat(64),"size":1,"source_root":"mako","python_path":"."}]
    }))
    .unwrap()
}

fn llvm_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-source-lock-v2", "family":"llvm", "version":"17.0.6",
        "sources":[{"component":"llvm","version":"17.0.6","purpose":"toolchain-component","filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz","sha256":"4".repeat(64),"size":1}],
        "host_python_packages":[{"name":"wheel","version":"0.43.0","filename":"wheel.tar.gz","url":"https://example.invalid/wheel.tar.gz","sha256":"5".repeat(64),"size":1,"source_root":"wheel","python_path":"."}]
    }))
    .unwrap()
}

fn llvm_profiles() -> Vec<u8> {
    let pc = json!({
        "name":"pc-x86_64", "configure_target":"pc-x86_64",
        "upstream_output_target":"pc-x86_64", "target_triple":"x86_64-unknown-aros",
        "cpu":"x86_64", "platform":"pc", "float_abi":"",
        "capabilities":["c", "cxx", "standalone-collector"]
    });
    let mut arm = pc.clone();
    arm["name"] = json!("arm-raspi");
    arm["configure_target"] = json!("arm-raspi");
    arm["upstream_output_target"] = json!("arm-raspi");
    arm["target_triple"] = json!("arm-unknown-aros");
    arm["cpu"] = json!("arm");
    arm["platform"] = json!("raspi");
    let mut aarch64 = pc.clone();
    aarch64["name"] = json!("rpi-aarch64");
    aarch64["configure_target"] = json!("rpi-aarch64");
    aarch64["upstream_output_target"] = json!("rpi-aarch64");
    aarch64["target_triple"] = json!("aarch64-unknown-aros");
    aarch64["cpu"] = json!("aarch64");
    aarch64["platform"] = json!("raspi");
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-profiles-v2", "family":"llvm",
        "upstream_commit":LLVM_SOURCE_COMMIT, "profiles":[pc, arm, aarch64]
    }))
    .unwrap()
}

fn recipe_bytes(source_lock: &[u8], profiles: &[u8], source_commit: &str) -> Vec<u8> {
    let mut recipe = json!({
        "schema":"aros-toolchain-recipe-v2", "source_commit":source_commit,
        "source_tree":"7".repeat(40), "producer_commit":PRODUCER_COMMIT,
        "producer_tree":PRODUCER_TREE, "tools_commit":TOOLS_COMMIT,
        "tools_tree":TOOLS_TREE, "source_date_epoch":0,
        "source_lock_sha256":sha256_bytes(source_lock),
        "profiles_sha256":sha256_bytes(profiles), "patches":[]
    });
    recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
    serde_json::to_vec(&recipe).unwrap()
}

fn add_group(
    documents: &mut BTreeMap<String, Vec<u8>>,
    id: &str,
    source_commit: &str,
    source_lock: Vec<u8>,
    profiles: Vec<u8>,
) -> Value {
    let entries = [
        (
            format!("{id}-recipe.json"),
            recipe_bytes(&source_lock, &profiles, source_commit),
        ),
        (format!("{id}-source-lock.json"), source_lock),
        (format!("{id}-profiles.json"), profiles),
    ];
    let refs = entries
        .iter()
        .map(|(name, bytes)| json!({"file":name,"sha256":sha256_bytes(bytes)}))
        .collect::<Vec<_>>();
    for (name, bytes) in entries {
        documents.insert(name, bytes);
    }
    json!({"id":id,"recipe":refs[0],"source_lock":refs[1],"profiles":refs[2]})
}

fn target_scope_input_documents() -> (Value, BTreeMap<String, Vec<u8>>) {
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
    (
        json!({
            "schema":"aros-toolchain-release-inputs-v2", "producer_commit":PRODUCER_COMMIT,
            "tools_commit":TOOLS_COMMIT, "hosts":ACTIVE_HOSTS, "groups":[gnu, llvm]
        }),
        documents,
    )
}

fn build_environment(host: &str, asset: &str) -> Map<String, Value> {
    json!({
        "fixture":"synthetic-integration-package-input", "host":host,
        "asset":asset, "toolchain_id":"fixture-toolchain"
    })
    .as_object()
    .unwrap()
    .clone()
}

fn write_gnu_tool_layout(root: &Path, lock: &SourceLock, profile: &Profile) {
    let bin = root.join("bin");
    let libexec = root.join("libexec");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&libexec).unwrap();
    for relative in [
        "bin/fixture-c",
        "bin/fixture-cxx",
        "bin/fixture-ld",
        "bin/fixture-ar",
        "bin/fixture-ranlib",
        "bin/fixture-strip",
        "libexec/fixture-collect",
    ] {
        let path = root.join(relative);
        fs::write(&path, b"synthetic GNU executable fixture; never execute\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    symlink("fixture-c", bin.join("fixture-as")).unwrap();
    let binutils = lock
        .source_components()
        .find(|source| source.component() == "binutils")
        .unwrap();
    let compiler = ArosCompilerIdentity::Gnu {
        gcc_version: lock.version().to_owned(),
        binutils_version: binutils.version().to_owned(),
        target: profile.target().unwrap().clone(),
    };
    let tools = json!({
        "schema":"aros-toolchain-tools-v1", "compiler":compiler,
        "target_triple":profile.target_triple(),
        "tools":{
            "c":"bin/fixture-c", "cxx":"bin/fixture-cxx", "assembler":"bin/fixture-as",
            "linker":"bin/fixture-ld", "archive":"bin/fixture-ar", "ranlib":"bin/fixture-ranlib",
            "strip":"bin/fixture-strip", "collector":"libexec/fixture-collect"
        }
    });
    fs::write(
        root.join("toolchain-tools.json"),
        serde_json::to_vec(&tools).unwrap(),
    )
    .unwrap();
}

fn asset_name(lock: &SourceLock, profile: &Profile, host: &str) -> String {
    match lock.family() {
        CompilerFamily::Gnu => {
            let binutils = lock
                .source_components()
                .find(|source| source.component() == "binutils")
                .unwrap();
            format!(
                "aros-toolchain-v2-gcc{}-binutils{}-{host}-{}.tar.xz",
                lock.version(),
                binutils.version(),
                profile.name()
            )
        }
        CompilerFamily::Llvm => format!(
            "aros-toolchain-v2-llvm{}-{host}-{}.tar.xz",
            lock.version(),
            profile.name()
        ),
    }
}

fn path_snapshot(path: &Path) -> Option<MemberSnapshot> {
    let metadata = fs::symlink_metadata(path).ok()?;
    Some(if metadata.file_type().is_symlink() {
        MemberSnapshot::Symlink {
            target: fs::read_link(path).unwrap(),
            mode: metadata.mode(),
            inode: metadata.ino(),
        }
    } else if metadata.is_file() {
        MemberSnapshot::File {
            bytes: fs::read(path).unwrap(),
            mode: metadata.mode(),
            inode: metadata.ino(),
            links: metadata.nlink(),
            modified: metadata.modified().ok(),
        }
    } else if metadata.is_dir() {
        MemberSnapshot::Directory {
            mode: metadata.mode(),
            inode: metadata.ino(),
            modified: metadata.modified().ok(),
        }
    } else {
        MemberSnapshot::Other {
            mode: metadata.mode(),
            inode: metadata.ino(),
            size: metadata.len(),
        }
    })
}

fn assert_pre_rejected(
    fixture: &Fixture,
    directory: &Path,
    lane_inputs: &Path,
    subject_manifest: &Path,
    release_format: Option<&str>,
    expected_reason: &str,
    private_marker: Option<&str>,
) {
    let release_before = snapshot(directory);
    let lanes_before = path_snapshot(lane_inputs);
    let manifest_before = path_snapshot(subject_manifest);
    let output = fixture.invoke(
        directory,
        lane_inputs,
        subject_manifest,
        "pre-attestation",
        release_format,
        None,
    );
    assert_failure_is_sanitized(&output, directory, expected_reason, private_marker);
    assert_eq!(snapshot(directory), release_before);
    assert_eq!(path_snapshot(lane_inputs), lanes_before);
    assert_eq!(path_snapshot(subject_manifest), manifest_before);
    assert_eq!(
        directory.join("toolchain-index-v2.json").exists(),
        release_before
            .members
            .contains_key("toolchain-index-v2.json")
    );
    assert_eq!(
        directory.join("SHA256SUMS").exists(),
        release_before.members.contains_key("SHA256SUMS")
    );
}

fn assert_pre_hash_rejected(
    fixture: &Fixture,
    directory: &Path,
    lane_inputs: &Path,
    subject_manifest: &Path,
) {
    let release_before = snapshot(directory);
    let lanes_before = path_snapshot(lane_inputs);
    let manifest_before = path_snapshot(subject_manifest);
    let output = fixture.invoke(
        directory,
        lane_inputs,
        subject_manifest,
        "pre-attestation",
        Some("family-v2"),
        Some(&"a".repeat(64)),
    );
    assert_failure_is_sanitized(
        &output,
        directory,
        "does not accept --subject-manifest-sha256",
        None,
    );
    assert_eq!(snapshot(directory), release_before);
    assert_eq!(path_snapshot(lane_inputs), lanes_before);
    assert_eq!(path_snapshot(subject_manifest), manifest_before);
}

fn assert_final_rejected(
    fixture: &Fixture,
    directory: &Path,
    lane_inputs: &Path,
    subject_manifest: &Path,
    subject_hash: Option<&str>,
    expected_reason: &str,
    private_marker: Option<&str>,
) {
    let release_before = snapshot(directory);
    let manifest_before = path_snapshot(subject_manifest);
    let lanes_before = path_snapshot(lane_inputs);
    let output = fixture.invoke(
        directory,
        lane_inputs,
        subject_manifest,
        "final",
        Some("family-v2"),
        subject_hash,
    );
    assert_failure_is_sanitized(&output, directory, expected_reason, private_marker);
    assert_eq!(snapshot(directory), release_before);
    assert_eq!(path_snapshot(subject_manifest), manifest_before);
    assert_eq!(path_snapshot(lane_inputs), lanes_before);
}

fn lane_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn duplicate_lane_key_bytes(lanes: &Value) -> Vec<u8> {
    let entries = lanes["lanes"].as_object().unwrap();
    let mut pairs = Vec::new();
    for (index, (key, value)) in entries.iter().enumerate() {
        let encoded_key = serde_json::to_string(key).unwrap();
        let encoded_value = serde_json::to_string(value).unwrap();
        pairs.push(format!("{encoded_key}:{encoded_value}"));
        if index == 0 {
            pairs.push(format!("{encoded_key}:{encoded_value}"));
        }
    }
    format!(
        "{{\"schema\":\"aros-toolchain-index-lanes-v1\",\"lanes\":{{{}}}}}",
        pairs.join(",")
    )
    .into_bytes()
}

fn duplicate_environment_key_bytes(lanes: &Value) -> Vec<u8> {
    let entries = lanes["lanes"].as_object().unwrap();
    let duplicate_lane_key = entries.keys().next().unwrap();
    let mut lane_pairs = Vec::new();
    for (key, lane) in entries {
        let lane_json = if key == duplicate_lane_key {
            let environment = lane["build_environment"].as_object().unwrap();
            let mut environment_pairs = Vec::new();
            for (index, (environment_key, value)) in environment.iter().enumerate() {
                let encoded_key = serde_json::to_string(environment_key).unwrap();
                let encoded_value = serde_json::to_string(value).unwrap();
                environment_pairs.push(format!("{encoded_key}:{encoded_value}"));
                if index == 0 {
                    environment_pairs.push(format!("{encoded_key}:{encoded_value}"));
                }
            }
            let paths = serde_json::to_string(&lane["required_paths"]).unwrap();
            format!(
                "{{\"build_environment\":{{{}}},\"required_paths\":{paths}}}",
                environment_pairs.join(",")
            )
        } else {
            serde_json::to_string(lane).unwrap()
        };
        lane_pairs.push(format!(
            "{}:{lane_json}",
            serde_json::to_string(key).unwrap()
        ));
    }
    format!(
        "{{\"schema\":\"aros-toolchain-index-lanes-v1\",\"lanes\":{{{}}}}}",
        lane_pairs.join(",")
    )
    .into_bytes()
}

fn expected_manifest_digest(result: &Value, manifest: &Path) -> String {
    let digest = sha256_bytes(&fs::read(manifest).unwrap())
        .as_str()
        .to_owned();
    assert_eq!(
        result["subject_manifest_sha256"].as_str(),
        Some(digest.as_str())
    );
    digest
}

fn pre_stage_case(
    fixture: &Fixture,
    case: &str,
    lane_contents: &[u8],
) -> (PathBuf, PathBuf, PathBuf) {
    let directory = fixture.copy_preindex(case);
    let lane_inputs = fixture.lane_file(case, lane_contents);
    let subject_manifest = fixture.root.join(format!("{case}-subjects.sha256"));
    (directory, lane_inputs, subject_manifest)
}

fn inventory_count(directory: &Path) -> usize {
    fs::read_dir(directory).unwrap().count()
}

#[test]
fn family_v2_index_cli_binds_staged_outputs_and_rejects_invalid_process_inputs() {
    let fixture = Fixture::new();
    let canonical_lanes = fixture.lane_bytes();

    let pre_directory = fixture.copy_preindex("valid-preindex");
    let lane_inputs = fixture.lane_file("valid", &canonical_lanes);
    let subject_manifest = fixture.root.join("valid-subjects.sha256");
    assert_eq!(inventory_count(&pre_directory), 57);
    let pre_output = fixture.invoke(
        &pre_directory,
        &lane_inputs,
        &subject_manifest,
        "pre-attestation",
        Some("family-v2"),
        None,
    );
    let pre_result = successful_json(&pre_output, "pre-attestation family-v2 index");
    assert_eq!(pre_result["schema"], "aros-toolchain-producer-stage-v2");
    assert_eq!(pre_result["operation"], "index");
    assert_eq!(pre_result["inventory_count"], 60);
    assert_eq!(pre_result["subject_count"], 58);
    assert!(pre_directory.join("toolchain-index-v2.json").is_file());
    assert!(subject_manifest.is_file());
    assert!(!pre_directory.join("SHA256SUMS").exists());
    assert!(!pre_directory
        .join("toolchain-provenance.sigstore.json")
        .exists());
    assert_eq!(inventory_count(&pre_directory), 58);
    let subject_text = fs::read_to_string(&subject_manifest).unwrap();
    assert_eq!(subject_text.lines().count(), 58);
    assert!(!subject_text.contains("SHA256SUMS"));
    assert!(!subject_text.contains("toolchain-provenance.sigstore.json"));
    let subject_hash = expected_manifest_digest(&pre_result, &subject_manifest);
    assert_eq!(
        pre_result["index_sha256"].as_str(),
        Some(
            sha256_bytes(&fs::read(pre_directory.join("toolchain-index-v2.json")).unwrap())
                .as_str()
        )
    );
    let indexed_template = copy_directory(&pre_directory, &fixture.root.join("indexed-template"));

    // A final stage is explicitly bound to the pre-attestation manifest hash.
    let final_directory = copy_directory(&indexed_template, &fixture.root.join("valid-final"));
    fs::write(
        final_directory.join("toolchain-provenance.sigstore.json"),
        SYNTHETIC_PROVENANCE,
    )
    .unwrap();
    let final_output = fixture.invoke(
        &final_directory,
        &lane_inputs,
        &subject_manifest,
        "final",
        Some("family-v2"),
        Some(&subject_hash),
    );
    let final_result = successful_json(&final_output, "final family-v2 index");
    assert_eq!(final_result["schema"], "aros-toolchain-producer-stage-v2");
    assert_eq!(final_result["operation"], "index");
    assert_eq!(final_result["subject_manifest_sha256"], subject_hash);
    assert!(final_result["checksums_sha256"].as_str().is_some());
    assert_eq!(
        final_result["provenance_sha256"].as_str(),
        Some(
            sha256_bytes(
                &fs::read(final_directory.join("toolchain-provenance.sigstore.json")).unwrap()
            )
            .as_str()
        )
    );
    let checksums = fs::read_to_string(final_directory.join("SHA256SUMS")).unwrap();
    assert_eq!(checksums.lines().count(), 59);
    assert_eq!(inventory_count(&final_directory), 60);

    // Omitting the format must not route this v2 lane request through the old v1 path.
    let (directory, lanes, manifest) = pre_stage_case(&fixture, "missing-format", &canonical_lanes);
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        None,
        "--source-lock-filename",
        None,
    );

    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "hash-on-pre-stage", &canonical_lanes);
    assert_pre_hash_rejected(&fixture, &directory, &lanes, &manifest);

    let mut unknown_field = fixture.lanes.clone();
    unknown_field["unexpected"] = json!(PRIVATE_MARKER);
    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "unknown-lane-field", &lane_bytes(&unknown_field));
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "closed lane schema",
        Some(PRIVATE_MARKER),
    );

    let (directory, lanes, manifest) = pre_stage_case(
        &fixture,
        "duplicate-lane-key",
        &duplicate_lane_key_bytes(&fixture.lanes),
    );
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "malformed or contain duplicate keys",
        None,
    );

    let (directory, lanes, manifest) = pre_stage_case(
        &fixture,
        "duplicate-environment-key",
        &duplicate_environment_key_bytes(&fixture.lanes),
    );
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "malformed or contain duplicate keys",
        None,
    );

    let mut wrong_environment = fixture.lanes.clone();
    let first_lane = wrong_environment["lanes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    first_lane["build_environment"]["fixture"] = json!(PRIVATE_MARKER);
    let (directory, lanes, manifest) = pre_stage_case(
        &fixture,
        "wrong-environment",
        &lane_bytes(&wrong_environment),
    );
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "measured native package failed read-back verification",
        Some(PRIVATE_MARKER),
    );

    let mut wrong_required_paths = fixture.lanes.clone();
    wrong_required_paths["lanes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["required_paths"] = json!([PRIVATE_MARKER]);
    let (directory, lanes, manifest) = pre_stage_case(
        &fixture,
        "wrong-required-paths",
        &lane_bytes(&wrong_required_paths),
    );
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "indexed required path is absent or is not a file or symlink",
        Some(PRIVATE_MARKER),
    );

    let mut missing_lane = fixture.lanes.clone();
    let missing_key = missing_lane["lanes"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    missing_lane["lanes"]
        .as_object_mut()
        .unwrap()
        .remove(&missing_key);
    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "missing-lane", &lane_bytes(&missing_lane));
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "measured release lane maps must exactly cover",
        None,
    );

    let mut extra_lane = fixture.lanes.clone();
    let sample_lane = extra_lane["lanes"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    extra_lane["lanes"]["unexpected.archive"] = sample_lane;
    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "extra-lane", &lane_bytes(&extra_lane));
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "measured release lane maps must exactly cover",
        None,
    );

    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "corrupt-archive", &canonical_lanes);
    let archive = directory.join(
        fixture.lanes["lanes"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap(),
    );
    let mut corrupt = fs::read(&archive).unwrap();
    corrupt.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(&archive, corrupt).unwrap();
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "measured native package failed read-back verification",
        Some(PRIVATE_MARKER),
    );

    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "existing-subject-manifest", &canonical_lanes);
    fs::write(&manifest, PRIVATE_MARKER.as_bytes()).unwrap();
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "subject manifest output already exists or is unavailable",
        Some(PRIVATE_MARKER),
    );

    let (directory, lanes, _) =
        pre_stage_case(&fixture, "inside-subject-manifest", &canonical_lanes);
    let inside_manifest = directory.join("subject-manifest.sha256");
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &inside_manifest,
        Some("family-v2"),
        "subject manifest must be absolute and outside the release directory",
        None,
    );

    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "symlink-lane-input", &canonical_lanes);
    let linked_lanes = fixture.root.join("symlink-lanes.json");
    symlink(&lanes, &linked_lanes).unwrap();
    assert_pre_rejected(
        &fixture,
        &directory,
        &linked_lanes,
        &manifest,
        Some("family-v2"),
        "safely opened regular file",
        None,
    );

    let (directory, _, manifest) = pre_stage_case(&fixture, "fifo-lane-input", &canonical_lanes);
    let fifo_lanes = fixture.root.join("fifo-lanes.json");
    assert!(Command::new("mkfifo")
        .arg(&fifo_lanes)
        .status()
        .unwrap()
        .success());
    let fifo_before = path_snapshot(&fifo_lanes);
    let fifo_release_before = snapshot(&directory);
    let fifo_output = invoke_with_timeout(
        fixture.command(
            &directory,
            &fifo_lanes,
            &manifest,
            "pre-attestation",
            Some("family-v2"),
            None,
        ),
        Duration::from_secs(5),
    );
    assert_failure_is_sanitized(&fifo_output, &directory, "safely opened regular file", None);
    assert_eq!(path_snapshot(&fifo_lanes), fifo_before);
    assert_eq!(snapshot(&directory), fifo_release_before);
    assert!(!directory.join("toolchain-index-v2.json").exists());
    assert!(!manifest.exists());

    let (directory, lanes, manifest) =
        pre_stage_case(&fixture, "oversized-lane-input", &canonical_lanes);
    fs::write(&lanes, vec![b'x'; 16 * 1024 * 1024 + 1]).unwrap();
    assert_pre_rejected(
        &fixture,
        &directory,
        &lanes,
        &manifest,
        Some("family-v2"),
        "exceeds the metadata limit",
        None,
    );

    // Final hash omission and mismatch are rejected without replacing outputs.
    let missing_hash_dir =
        copy_directory(&indexed_template, &fixture.root.join("missing-final-hash"));
    fs::write(
        missing_hash_dir.join("toolchain-provenance.sigstore.json"),
        SYNTHETIC_PROVENANCE,
    )
    .unwrap();
    assert_final_rejected(
        &fixture,
        &missing_hash_dir,
        &lane_inputs,
        &subject_manifest,
        None,
        "--subject-manifest-sha256",
        None,
    );

    let wrong_hash_dir = copy_directory(&indexed_template, &fixture.root.join("wrong-final-hash"));
    fs::write(
        wrong_hash_dir.join("toolchain-provenance.sigstore.json"),
        SYNTHETIC_PROVENANCE,
    )
    .unwrap();
    let wrong_hash = "0".repeat(64);
    assert_ne!(wrong_hash, subject_hash);
    assert_final_rejected(
        &fixture,
        &wrong_hash_dir,
        &lane_inputs,
        &subject_manifest,
        Some(&wrong_hash),
        "subject manifest differs from the pre-attestation digest",
        None,
    );

    let changed_subject_dir = copy_directory(
        &indexed_template,
        &fixture.root.join("changed-final-subject"),
    );
    fs::write(
        changed_subject_dir.join("toolchain-provenance.sigstore.json"),
        SYNTHETIC_PROVENANCE,
    )
    .unwrap();
    let changed_archive = changed_subject_dir.join(
        fixture.lanes["lanes"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap(),
    );
    let mut changed_bytes = fs::read(&changed_archive).unwrap();
    changed_bytes.extend_from_slice(PRIVATE_MARKER.as_bytes());
    fs::write(&changed_archive, changed_bytes).unwrap();
    assert_final_rejected(
        &fixture,
        &changed_subject_dir,
        &lane_inputs,
        &subject_manifest,
        Some(&subject_hash),
        "native compiler-family package failed read-back verification",
        Some(PRIVATE_MARKER),
    );

    let existing_checksums_dir = copy_directory(
        &indexed_template,
        &fixture.root.join("existing-final-checksums"),
    );
    fs::write(
        existing_checksums_dir.join("toolchain-provenance.sigstore.json"),
        SYNTHETIC_PROVENANCE,
    )
    .unwrap();
    fs::write(
        existing_checksums_dir.join("SHA256SUMS"),
        PRIVATE_MARKER.as_bytes(),
    )
    .unwrap();
    assert_final_rejected(
        &fixture,
        &existing_checksums_dir,
        &lane_inputs,
        &subject_manifest,
        Some(&subject_hash),
        "release directory does not contain the exact indexed inventory",
        Some(PRIVATE_MARKER),
    );
}
