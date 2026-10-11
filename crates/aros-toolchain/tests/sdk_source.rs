#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sha256_bytes;
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceOutput,
    EngineFreeSourceRequest,
};
use aros_toolchain::recipe::GitObjectId;
use aros_toolchain::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest};
use serde_json::{json, Value};
use tempfile::TempDir;

const PROFILE: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const POLICY: &[u8] = b"{}";
const C_PROBE: &[u8] = b"/* sealed SDK C probe */\n";
const CXX_PROBE: &[u8] = b"/* sealed SDK C++ probe */\n";

struct Fixture {
    temporary: TempDir,
    source_root: PathBuf,
    materialized_source_root: PathBuf,
    materialized: EngineFreeSourceOutput,
    identity: SdkIdentity,
    consumer_bytes: Vec<u8>,
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=AROS SDK source test",
            "-c",
            "user.email=aros-sdk-source-test@example.invalid",
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
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn source_identity(source_root: &Path) -> CommittedSourceIdentity {
    CommittedSourceIdentity {
        commit: GitObjectId::try_from(git_text(source_root, &["rev-parse", "HEAD^0"])).unwrap(),
        tree: GitObjectId::try_from(git_text(source_root, &["rev-parse", "HEAD:"])).unwrap(),
    }
}

fn abi_value() -> Value {
    json!({
        "source_cpu": "riscv64",
        "target_triple": "riscv64-aros",
        "isa": "rva22u64",
        "abi": "lp64d",
        "code_model": "medany",
        "flavour": "native",
        "platform_smp": true,
        "use_mmu": true,
    })
}

fn write_consumer_contract(root: &Path, v2: bool) -> Vec<u8> {
    fs::write(root.join("aros-targets.toml"), PROFILE).unwrap();
    fs::write(root.join("policy.json"), POLICY).unwrap();

    let mut inputs = vec![
        json!({
            "path": "aros-targets.toml",
            "sha256": sha256_bytes(PROFILE.as_bytes()),
        }),
        json!({
            "path": "policy.json",
            "sha256": sha256_bytes(POLICY),
        }),
    ];
    let mut document = json!({
        "schema": if v2 {
            "aros-native-consumer-contract-v2"
        } else {
            "aros-native-consumer-contract-v1"
        },
        "profile": "example-sdk",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "metamake_projection": "policy.json",
        "inputs": inputs,
        "abi": abi_value(),
    });
    if v2 {
        fs::write(root.join("probe.c"), C_PROBE).unwrap();
        fs::write(root.join("probe.cpp"), CXX_PROBE).unwrap();
        inputs.clone_from(document["inputs"].as_array().unwrap());
        inputs.extend([
            json!({"path": "probe.c", "sha256": sha256_bytes(C_PROBE)}),
            json!({"path": "probe.cpp", "sha256": sha256_bytes(CXX_PROBE)}),
        ]);
        document["inputs"] = Value::Array(inputs);
        document["native_sdk_link_probes"] = json!({
            "c": {"source": "probe.c", "libraries": []},
            "cxx": {"source": "probe.cpp", "libraries": []},
        });
    }
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(root.join("consumer.json"), &bytes).unwrap();
    bytes
}

fn create_fixture(v2: bool) -> Fixture {
    create_source_fixture(
        |root| write_consumer_contract(root, v2),
        SdkTarget {
            target_triple: "riscv64-aros".to_owned(),
            abi: serde_json::from_value(abi_value()).unwrap(),
        },
    )
}

fn create_source_fixture(
    write_contract: impl FnOnce(&Path) -> Vec<u8>,
    target: SdkTarget,
) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    fs::create_dir(&source_root).unwrap();
    git(&source_root, &["init", "-q"]);

    let consumer_bytes = write_contract(&source_root);
    fs::create_dir(source_root.join("cmake")).unwrap();
    fs::write(
        source_root.join("cmake/engine.cmake"),
        "excluded source engine\n",
    )
    .unwrap();
    fs::create_dir(source_root.join("docs")).unwrap();
    fs::write(
        source_root.join("docs/source.txt"),
        "committed SDK source\n",
    )
    .unwrap();
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &["commit", "-qm", "sealed SDK source fixture"],
    );

    let committed = source_identity(&source_root);
    let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: source_root.clone(),
        expected_source: committed,
        output_root: temporary.path().join("materialized"),
    })
    .unwrap();
    let contract_digest = sha256_bytes(&consumer_bytes);
    let identity = SdkIdentity {
        sdk_id: "example-sdk".to_owned(),
        version: "1.0.0".to_owned(),
        target,
        source: SdkSourceIdentity {
            commit: materialized.source_commit.clone(),
            git_tree: materialized.source_tree.clone(),
            content_sha256: materialized.source_tree_sha256.clone(),
            contract_sha256: contract_digest,
        },
        // Deliberately explicit and independent of the fixture commit timestamp.
        source_date_epoch: 946_684_800,
    };

    Fixture {
        temporary,
        source_root,
        materialized_source_root: materialized.root.clone(),
        materialized,
        identity,
        consumer_bytes,
    }
}

fn create_generic_fixture(arch: &str, family: &str, float_abi: Option<&str>) -> Fixture {
    create_generic_fixture_with_elf(arch, family, float_abi, None)
}

fn create_generic_fixture_with_elf(
    arch: &str,
    family: &str,
    float_abi: Option<&str>,
    elf_machine: Option<u16>,
) -> Fixture {
    let source_cpu = if arch == "riscv32" { "riscv" } else { arch };
    let width = if matches!(arch, "arm" | "riscv32") {
        32
    } else {
        64
    };
    let mut abi = BTreeMap::from([
        ("source_cpu".into(), SdkAbiValue::Text(source_cpu.into())),
        ("pointer_width".into(), SdkAbiValue::Unsigned(width)),
        ("use_mmu".into(), SdkAbiValue::Boolean(true)),
        ("flavour".into(), SdkAbiValue::Text("native".into())),
        ("platform_smp".into(), SdkAbiValue::Boolean(true)),
    ]);
    if let Some(float) = float_abi {
        abi.insert("float_abi".into(), SdkAbiValue::Text(float.into()));
    }
    if let Some(machine) = elf_machine {
        abi.extend([
            (
                "elf_machine".into(),
                SdkAbiValue::Unsigned(u64::from(machine)),
            ),
            ("elf_flags_mask".into(), SdkAbiValue::Unsigned(0)),
            ("elf_flags_value".into(), SdkAbiValue::Unsigned(0)),
        ]);
    }
    let target = SdkTarget {
        target_triple: format!("{source_cpu}-unknown-aros"),
        abi,
    };
    let selected = target.clone();
    create_source_fixture(
        |root| {
            let float =
                float_abi.map_or_else(String::new, |float| format!("float_abi='{float}'\n"));
            let profile = format!(
                "[[targets]]\nname='example-sdk'\narch='{arch}'\nplatform='generic'\nbsp='example'\nsdk_consumer_contract='consumer.json'\n{float}[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='{family}'\ncpu32=''\nuse_mmu=true\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n"
            );
            for (path, bytes) in [
                ("aros-targets.toml", profile.as_bytes()),
                ("policy.json", POLICY),
                ("probe.c", C_PROBE),
                ("probe.cpp", CXX_PROBE),
            ] {
                fs::write(root.join(path), bytes).unwrap();
            }
            let inputs: Vec<Value> = ["aros-targets.toml", "policy.json", "probe.c", "probe.cpp"]
                .into_iter()
                .map(|path| json!({"path": path, "sha256": sha256_bytes(&fs::read(root.join(path)).unwrap())}))
                .collect();
            let document = json!({
                "schema": "aros-sdk-consumer-contract-v1",
                "profile": "example-sdk",
                "source_baseline": "0123456789abcdef0123456789abcdef01234567",
                "roots": ["includes", "linklibs"],
                "inputs": inputs,
                "metamake_projection": "policy.json",
                "target": selected,
                "native_sdk_link_probes": {
                    "c": {"source": "probe.c", "libraries": []},
                    "cxx": {"source": "probe.cpp", "libraries": []},
                },
            });
            let bytes = serde_json::to_vec(&document).unwrap();
            fs::write(root.join("consumer.json"), &bytes).unwrap();
            bytes
        },
        target,
    )
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    Regular(Vec<u8>),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    mode: u32,
    kind: EntryKind,
}

fn collect_snapshot(root: &Path, directory: &Path, entries: &mut Vec<SnapshotEntry>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).unwrap())
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::Regular(fs::read(&path).unwrap())
        } else {
            EntryKind::Other
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

fn top_level_entries(root: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn request(fixture: &Fixture, identity: SdkIdentity, preset: &str) -> SdkSourceVerificationRequest {
    SdkSourceVerificationRequest {
        source_root: fixture.source_root.clone(),
        materialized_source_root: fixture.materialized_source_root.clone(),
        source_preset: preset.to_owned(),
        identity,
    }
}

fn verify_rejects_without_writes(
    fixture: &Fixture,
    request: &SdkSourceVerificationRequest,
    expected_reason: &str,
) {
    let source_before = snapshot(&fixture.source_root);
    let materialized_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());
    let error = verify_sdk_source(request).expect_err("invalid SDK source was accepted");
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(
        message.contains(expected_reason),
        "rejected SDK source for an unexpected reason: expected {expected_reason:?}, got {message:?}"
    );
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        materialized_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);
}

fn changed_object_id(value: &str) -> String {
    let mut changed = value.to_owned();
    let replacement = if changed.starts_with('0') { '1' } else { '0' };
    changed.replace_range(0..1, &replacement.to_string());
    changed
}

#[test]
fn binds_git_engine_free_cas_sealed_contract_abi_and_revalidates_read_only() {
    let fixture = create_fixture(true);
    let request = request(&fixture, fixture.identity.clone(), "example-sdk");
    let source_before = snapshot(&fixture.source_root);
    let projection_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());

    let verified = verify_sdk_source(&request).unwrap();
    assert_eq!(verified.identity(), &fixture.identity);
    assert_eq!(verified.source_profile().name, "example-sdk");
    assert_eq!(
        verified.consumer_contract_schema(),
        "aros-native-consumer-contract-v2"
    );
    aros_toolchain::sdk_generation::validate_source_executor(&verified).unwrap();
    assert_eq!(
        fixture.identity.source.commit,
        fixture.materialized.source_commit
    );
    assert_eq!(
        fixture.identity.source.git_tree,
        fixture.materialized.source_tree
    );
    assert_eq!(
        fixture.identity.source.content_sha256,
        fixture.materialized.source_tree_sha256
    );
    assert_eq!(
        fixture.identity.source.contract_sha256,
        sha256_bytes(&fixture.consumer_bytes)
    );
    assert_eq!(verified.identity().source_date_epoch, 946_684_800);
    assert_eq!(
        verified.materialized_source_root(),
        fixture.materialized_source_root.as_path()
    );
    assert!(!fixture.materialized_source_root.join("cmake").exists());
    verified.revalidate().unwrap();

    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        projection_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);
}

#[test]
fn generic_sdk_contract_binds_four_target_selections_without_executing_them() {
    for (arch, family, float) in [
        ("x86_64", "llvm", None),
        ("arm", "llvm", Some("hard")),
        ("aarch64", "llvm", None),
        ("riscv32", "gnu", Some("ilp32f")),
    ] {
        let fixture = create_generic_fixture(arch, family, float);
        let source_before = snapshot(&fixture.source_root);
        let projection_before = snapshot(&fixture.materialized_source_root);
        let siblings_before = top_level_entries(fixture.temporary.path());
        let verified =
            verify_sdk_source(&request(&fixture, fixture.identity.clone(), "example-sdk")).unwrap();
        assert_eq!(verified.identity().target, fixture.identity.target);
        assert_eq!(
            verified.consumer_contract_schema(),
            "aros-sdk-consumer-contract-v1"
        );
        let unsupported =
            aros_toolchain::sdk_generation::validate_source_executor(&verified).unwrap_err();
        assert!(unsupported
            .to_string()
            .contains("does not yet support the selected generic source contract"));
        verified.revalidate().unwrap();
        assert_eq!(snapshot(&fixture.source_root), source_before);
        assert_eq!(
            snapshot(&fixture.materialized_source_root),
            projection_before
        );
        assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);

        let mut incompatible = fixture.identity.clone();
        incompatible
            .target
            .abi
            .insert("extra_fact".into(), SdkAbiValue::Boolean(true));
        verify_rejects_without_writes(
            &fixture,
            &request(&fixture, incompatible, "example-sdk"),
            "SDK target or ABI differs from its source contract",
        );
        let mut wrong_digest = fixture.identity.clone();
        wrong_digest.source.contract_sha256 = sha256_bytes(b"unselected generic ABI bytes");
        verify_rejects_without_writes(
            &fixture,
            &request(&fixture, wrong_digest, "example-sdk"),
            "SDK source contract differs from its selected digest",
        );
    }
}

#[test]
fn generic_sdk_source_binding_rejects_changed_sealed_input() {
    let fixture = create_generic_fixture("arm", "llvm", Some("hard"));
    let verified =
        verify_sdk_source(&request(&fixture, fixture.identity.clone(), "example-sdk")).unwrap();
    fs::write(
        fixture.materialized_source_root.join("probe.c"),
        b"/* changed sealed C probe */\n",
    )
    .unwrap();
    assert!(verified.revalidate().is_err());
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source consumer contract is not sealed to its profile and inputs",
    );
}

#[test]
fn sdk_source_binding_rejects_ambiguous_contract_paths_without_fallback() {
    let fixture = create_generic_fixture("x86_64", "llvm", None);
    verify_sdk_source(&request(&fixture, fixture.identity.clone(), "example-sdk")).unwrap();
    let path = fixture.materialized_source_root.join("aros-targets.toml");
    let original = fs::read_to_string(&path).unwrap();
    let ambiguous = original.replace(
        "sdk_consumer_contract='consumer.json'",
        "sdk_consumer_contract='consumer.json'\nnative_consumer_contract='absent-native-consumer.json'",
    );
    assert_ne!(ambiguous, original);
    fs::write(path, ambiguous).unwrap();
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source preset has ambiguous consumer contracts",
    );
}

#[test]
fn rejects_each_selected_identity_mismatch_without_writes() {
    let mut changed = create_fixture(true);
    changed.identity.source.commit = changed_object_id(&changed.identity.source.commit);
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "compatibility source checkout differs from the selected source identity",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.git_tree = changed_object_id(&changed.identity.source.git_tree);
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "compatibility source checkout differs from the selected source identity",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.content_sha256 = sha256_bytes(b"wrong engine-free content");
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK source content differs from its selected engine-free digest",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.contract_sha256 = sha256_bytes(b"wrong contract bytes");
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK source contract differs from its selected digest",
    );
}

#[test]
fn rejects_target_triple_and_abi_divergence_without_writes() {
    let mut changed = create_fixture(true);
    changed.identity.target.target_triple = "riscv64-other".to_owned();
    changed.identity.target.abi.insert(
        "target_triple".to_owned(),
        SdkAbiValue::Text("riscv64-other".to_owned()),
    );
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut changed = create_fixture(true);
    changed
        .identity
        .target
        .abi
        .insert("abi".to_owned(), SdkAbiValue::Text("lp64".to_owned()));
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut missing = create_fixture(true);
    missing.identity.target.abi.remove("use_mmu");
    verify_rejects_without_writes(
        &missing,
        &request(&missing, missing.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut extra = create_fixture(true);
    extra
        .identity
        .target
        .abi
        .insert("extra_fact".to_owned(), SdkAbiValue::Boolean(true));
    verify_rejects_without_writes(
        &extra,
        &request(&extra, extra.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut scalar = create_fixture(true);
    scalar.identity.target.abi.insert(
        "platform_smp".to_owned(),
        SdkAbiValue::Text("true".to_owned()),
    );
    verify_rejects_without_writes(
        &scalar,
        &request(&scalar, scalar.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );
}

#[test]
fn rejects_unknown_preset_and_legacy_v1_contract_without_writes() {
    let fixture = create_fixture(true);
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "not-declared"),
        "SDK source preset is absent",
    );

    let fixture = create_fixture(false);
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source contract must declare v2 ordinary link probes",
    );
}

#[test]
fn rejects_changed_source_or_projection_and_revalidation_writes_nothing() {
    let fixture = create_fixture(true);
    let verified =
        verify_sdk_source(&request(&fixture, fixture.identity.clone(), "example-sdk")).unwrap();
    fs::write(
        fixture.source_root.join("docs/source.txt"),
        "changed original source\n",
    )
    .unwrap();
    let source_before = snapshot(&fixture.source_root);
    let projection_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());
    let error = verified
        .revalidate()
        .expect_err("changed original source was accepted during revalidation");
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(
        message.contains("source file size or executable bit differs from its committed blob"),
        "revalidation rejected the changed original source for an unexpected reason: {message:?}"
    );
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        projection_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);

    let fixture = create_fixture(true);
    fs::write(
        fixture.materialized_source_root.join("docs/source.txt"),
        "changed projection\n",
    )
    .unwrap();
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "source projection differs from raw committed material",
    );

    let fixture = create_fixture(true);
    fs::write(
        fixture.materialized_source_root.join("probe.c"),
        b"changed sealed probe\n",
    )
    .unwrap();
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source consumer contract is not sealed to its profile and inputs",
    );
}

fn application_selection(
    fixture: &Fixture,
) -> aros_toolchain::compatibility::LlvmSdkApplicationPlanRequest {
    use aros_common::{ArosCompilerIdentity, CancellationToken};
    use aros_toolchain::compatibility::{
        inspect_llvm_sdk_drivers, LlvmSdkApplicationPlanRequest, LlvmSdkDriverRequest,
    };
    let source =
        verify_sdk_source(&request(fixture, fixture.identity.clone(), "example-sdk")).unwrap();
    let compiler_root = fixture.temporary.path().join("compiler");
    fs::create_dir_all(compiler_root.join("bin")).unwrap();
    let driver = compiler_root.join("bin/clang");
    fs::write(&driver, format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'clang version 11.0.0\\n'\nelse\n  printf '{}\\n'\nfi\n",
            fixture.identity.target.target_triple,
        )).unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("clang", compiler_root.join("bin/clang++")).unwrap();
    let drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root,
            compiler: ArosCompilerIdentity::Llvm {
                version: "11.0.0".into(),
            },
            target: fixture.identity.target.clone(),
            timeout: std::time::Duration::from_secs(2),
        },
        &CancellationToken::default(),
    )
    .unwrap();
    // These deliberately incomplete fixture trees prove planning only;
    // neither tree is a generated SDK or a link-success claim.
    let original_sdk = fixture.temporary.path().join("original-sdk");
    let relocated_sdk = fixture.temporary.path().join("relocated-sdk");
    for sdk in [&original_sdk, &relocated_sdk] {
        fs::create_dir(sdk).unwrap();
        fs::write(sdk.join("fixture.txt"), "unqualified SDK-tree fixture\n").unwrap();
    }
    let output_root = fixture.temporary.path().join("link-proof");
    LlvmSdkApplicationPlanRequest {
        source,
        drivers,
        original_sdk,
        relocated_sdk,
        output_root,
    }
}

#[test]
fn plans_four_ordinary_llvm_links_from_sealed_source_without_writes() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::plan_llvm_sdk_application_links;

    for (arch, machine, float) in [
        ("x86_64", 62, None),
        ("arm", 40, Some("hard")),
        ("aarch64", 183, None),
    ] {
        let fixture = create_generic_fixture_with_elf(arch, "llvm", float, Some(machine));
        let selection = application_selection(&fixture);
        let original_sdk = &selection.original_sdk;
        let relocated_sdk = &selection.relocated_sdk;
        let output_root = &selection.output_root;
        let source_before = snapshot(&fixture.source_root);
        let projection_before = snapshot(&fixture.materialized_source_root);
        let siblings_before = top_level_entries(fixture.temporary.path());
        let plan =
            plan_llvm_sdk_application_links(&selection, &CancellationToken::default()).unwrap();
        assert_eq!(plan.commands().len(), 4);
        for (index, command) in plan.commands().iter().enumerate() {
            let cpp = index >= 2;
            let sdk = if index % 2 == 0 {
                original_sdk
            } else {
                relocated_sdk
            };
            assert_eq!(
                command.program.file_name().unwrap(),
                if cpp { "clang++" } else { "clang" }
            );
            assert!(command.arguments.contains(&format!(
                "--target={}",
                fixture.identity.target.target_triple
            )));
            assert!(command.arguments.contains(&format!(
                "--sysroot={}",
                sdk.canonicalize().unwrap().display()
            )));
            for forbidden in ["-c", "-r", "-nostdlib", "-nostartfiles", "-nodefaultlibs"] {
                assert!(!command
                    .arguments
                    .iter()
                    .any(|argument| argument == forbidden));
            }
            assert_eq!(command.arguments.contains(&"-std=c++17".into()), cpp);
            assert_eq!(
                command.arguments.contains(&"-mfloat-abi=hard".into()),
                float.is_some()
            );
        }
        plan.revalidate(&CancellationToken::default()).unwrap();
        assert!(!output_root.exists());
        assert_eq!(snapshot(&fixture.source_root), source_before);
        assert_eq!(
            snapshot(&fixture.materialized_source_root),
            projection_before
        );
        assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);

        let mut invalid = selection.clone();
        invalid.relocated_sdk.clone_from(&invalid.original_sdk);
        let error =
            plan_llvm_sdk_application_links(&invalid, &CancellationToken::default()).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].message,
            "LLVM SDK application roots must be distinct and non-overlapping"
        );
        fs::write(relocated_sdk.join("fixture.txt"), "changed fixture\n").unwrap();
        let error = plan.revalidate(&CancellationToken::default()).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].message,
            "LLVM SDK application trees differ before linking"
        );
        assert!(!output_root.exists());
    }
}

#[test]
fn rejects_unbound_llvm_plan_selections_before_writing() {
    use aros_common::{ArosCompilerIdentity, CancellationToken};
    use aros_toolchain::compatibility::{
        inspect_llvm_sdk_drivers, plan_llvm_sdk_application_links, LlvmSdkDriverRequest,
    };

    let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
    let selection = application_selection(&fixture);
    let assert_rejected = |selection: &_, expected: &str, cancellation: &CancellationToken| {
        let before = snapshot(fixture.temporary.path());
        let error = plan_llvm_sdk_application_links(selection, cancellation).unwrap_err();
        assert_eq!(error.diagnostics().diagnostics[0].message, expected);
        assert_eq!(snapshot(fixture.temporary.path()), before);
    };
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert_rejected(
        &selection,
        "LLVM SDK application planning was cancelled",
        &cancelled,
    );

    let mut invalid = selection.clone();
    invalid.output_root = selection.original_sdk.join("would-mutate-sdk");
    assert_rejected(
        &invalid,
        "LLVM SDK application output overlaps an input",
        &CancellationToken::default(),
    );
    let mut target = fixture.identity.target.clone();
    target
        .abi
        .insert("extra_fact".into(), SdkAbiValue::Boolean(true));
    invalid = selection.clone();
    invalid.drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: fixture.temporary.path().join("compiler"),
            compiler: ArosCompilerIdentity::Llvm {
                version: "11.0.0".into(),
            },
            target,
            timeout: std::time::Duration::from_secs(2),
        },
        &CancellationToken::default(),
    )
    .unwrap();
    assert_rejected(
        &invalid,
        "LLVM SDK drivers differ from the complete source-selected target",
        &CancellationToken::default(),
    );
    fs::write(&selection.output_root, "occupied output must remain\n").unwrap();
    assert_rejected(
        &selection,
        "LLVM SDK application output already exists or cannot be inspected",
        &CancellationToken::default(),
    );

    let fixture = create_generic_fixture("x86_64", "llvm", None);
    let selection = application_selection(&fixture);
    let before = snapshot(fixture.temporary.path());
    let error =
        plan_llvm_sdk_application_links(&selection, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].message,
        "SDK target is missing required unsigned ABI fact elf_machine"
    );
    assert_eq!(snapshot(fixture.temporary.path()), before);
}

// Synthetic drivers qualify the harness and rejection paths, never a real SDK.
fn llvm_link_selection(
    fixture: &Fixture,
    mode: &str,
) -> aros_toolchain::compatibility::LlvmSdkLinkRequest {
    use aros_common::{ArosCompilerIdentity, CancellationToken};
    use aros_toolchain::compatibility::{
        inspect_llvm_sdk_drivers, prepare, CompatibilityPreparationRequest, LlvmSdkDriverRequest,
        LlvmSdkLinkRequest, REQUIRED_HELPERS,
    };
    let selection = application_selection(fixture);
    let compiler = fixture.temporary.path().join("compiler");
    fs::write(compiler.join("fixture.elf"), synthetic_application_elf()).unwrap();
    fs::write(
        selection.original_sdk.join("libclient.a"),
        b"synthetic SDK archive",
    )
    .unwrap();
    let driver = compiler.join("bin/clang");
    fs::write(&driver, format!(
        "#!/bin/sh\nset -eu\nif [ \"$1\" = --version ]; then printf 'clang version 11.0.0\\n'; exit 0; fi\nif [ \"$2\" = -dumpmachine ]; then printf '{}\\n'; exit 0; fi\n[ \"$PATH\" = /nonexistent ]\n[ -z \"${{HOME+x}}\" ]\nsdk=\nmap=\nout=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --sysroot=*) sdk=${{1#--sysroot=}} ;;\n    -Wl,-Map=*) map=${{1#-Wl,-Map=}}; map=${{map%%,*}} ;;\n    -o) shift; out=$1 ;;\n    -nostdlib|-nostartfiles|-nodefaultlibs|-c|-r) exit 88 ;;\n  esac\n  shift\ndone\ncase '{mode}' in\n  failed) printf 'simulated link failure\\n' >&2; exit 29 ;;\n  timeout) /bin/sleep 2 ;;\nesac\n/bin/cp '{}/fixture.elf' \"$out\"\nprintf 'synthetic link map\\n' > \"$map\"\nprintf '%s/libclient.a\\n' \"$sdk\"\ncase '{mode}' in\n  foreign) printf '/etc/passwd\\n' ;;\n  extra) printf 'extra\\n' > \"$out.extra\" ;;\n  different) case \"$out\" in *relocated*) printf X >> \"$out\" ;; esac ;;\nesac\n",
        fixture.identity.target.target_triple, compiler.display(),
    )).unwrap();
    let drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: compiler,
            compiler: ArosCompilerIdentity::Llvm {
                version: "11.0.0".into(),
            },
            target: fixture.identity.target.clone(),
            timeout: std::time::Duration::from_secs(2),
        },
        &CancellationToken::default(),
    )
    .unwrap();
    let work = fixture.temporary.path().join("preparation");
    let helpers = fixture.temporary.path().join("helpers");
    fs::create_dir(&work).unwrap();
    fs::create_dir(&helpers).unwrap();
    for name in REQUIRED_HELPERS {
        let path = helpers.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let preparation = prepare(&CompatibilityPreparationRequest {
        source_root: fixture.materialized_source_root.clone(),
        work_root: work,
        helpers_root: helpers,
    })
    .unwrap();
    LlvmSdkLinkRequest {
        source: selection.source,
        drivers,
        preparation,
        sdk_root: selection.original_sdk,
        output_root: selection.output_root,
        timeout: std::time::Duration::from_secs(10),
    }
}

fn synthetic_application_elf() -> Vec<u8> {
    let names = b"\0__TOOLCHAIN_LIST__\0";
    let strings = 64 + 3 * 64;
    let symbols = strings + names.len();
    let mut bytes = vec![0; symbols + 48];
    bytes[..9].copy_from_slice(b"\x7fELF\x02\x01\x01\x0f\x01");
    for (offset, value) in [(16, 1_u16), (18, 62), (52, 64), (58, 64), (60, 3)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (20, 1_u32),
        (128 + 4, 3),
        (192 + 4, 2),
        (192 + 40, 1),
        (symbols + 24, 1),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (40, 64_u64),
        (128 + 24, strings as u64),
        (128 + 32, names.len() as u64),
        (128 + 48, 1),
        (192 + 24, symbols as u64),
        (192 + 32, 48),
        (192 + 48, 8),
        (192 + 56, 24),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes[strings..strings + names.len()].copy_from_slice(names);
    bytes[symbols + 24 + 4] = 0x10;
    bytes[symbols + 24 + 6..symbols + 24 + 8].copy_from_slice(&1_u16.to_le_bytes());
    bytes
}

#[test]
fn executes_and_revalidates_four_llvm_links_with_closed_retained_evidence() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::execute_llvm_sdk_links;
    let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
    let selection = llvm_link_selection(&fixture, "success");
    let source_before = snapshot(&fixture.source_root);
    let sdk_before = snapshot(&selection.sdk_root);
    let proof = execute_llvm_sdk_links(&selection, &CancellationToken::default()).unwrap();
    proof.revalidate(&CancellationToken::default()).unwrap();
    assert_eq!(
        sha256_bytes(&fs::read(proof.receipt_path()).unwrap()),
        *proof.receipt_sha256()
    );
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(snapshot(&selection.sdk_root), sdk_before);
    for member in [
        "links/original-c.elf",
        "links/relocated-c.elf",
        "links/original-cxx.elf",
        "links/relocated-cxx.elf",
        "links/reports/standalone-c.1.stdout.log",
        "links/reports/standalone-cxx.2.stderr.log",
        "sdk-inventory.json",
        "relocated-sdk/libclient.a",
    ] {
        assert!(selection.output_root.join(member).is_file());
    }
    let log = selection
        .output_root
        .join("links/reports/standalone-c.1.stdout.log");
    fs::write(log, b"tampered retained log\n").unwrap();
    assert!(proof.revalidate(&CancellationToken::default()).is_err());
}

#[test]
fn llvm_link_failure_foreign_inputs_extra_members_and_relocation_drift_never_finish() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::execute_llvm_sdk_links;
    for (mode, expected) in [
        ("failed", "exited unsuccessfully"),
        ("foreign", "outside the SDK or compiler roots"),
        ("extra", "proof directory cannot be bounded"),
        ("different", "differs after relocation"),
    ] {
        let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
        let selection = llvm_link_selection(&fixture, mode);
        let failure =
            execute_llvm_sdk_links(&selection, &CancellationToken::default()).unwrap_err();
        assert!(
            failure.to_string().contains(expected),
            "unexpected {mode} failure: {failure}"
        );
        assert!(!selection
            .output_root
            .join("receipt/llvm-sdk-links.receipt.json")
            .exists());
        assert!(selection
            .output_root
            .join("links/reports/standalone-c.1.stdout.log")
            .exists());
    }
}

#[test]
fn llvm_execution_rejects_cancelled_mixed_or_reused_selection_before_writes() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::execute_llvm_sdk_links;
    let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
    let selection = llvm_link_selection(&fixture, "success");
    let before = snapshot(fixture.temporary.path());
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(execute_llvm_sdk_links(&selection, &cancelled).is_err());
    let mut mixed = selection.clone();
    mixed.timeout = std::time::Duration::ZERO;
    assert!(execute_llvm_sdk_links(&mixed, &CancellationToken::default()).is_err());
    mixed = selection.clone();
    mixed.preparation.source_tree_sha256 = sha256_bytes(b"other source");
    assert!(execute_llvm_sdk_links(&mixed, &CancellationToken::default()).is_err());
    mixed = selection.clone();
    mixed.output_root = selection.sdk_root.join("overlapping");
    assert!(execute_llvm_sdk_links(&mixed, &CancellationToken::default()).is_err());
    assert_eq!(snapshot(fixture.temporary.path()), before);
    fs::create_dir(&selection.output_root).unwrap();
    fs::write(selection.output_root.join("owned.txt"), b"existing owner\n").unwrap();
    let before = snapshot(fixture.temporary.path());
    assert!(execute_llvm_sdk_links(&selection, &CancellationToken::default()).is_err());
    assert_eq!(snapshot(fixture.temporary.path()), before);
}

#[test]
fn llvm_link_deadline_rejects_slow_driver_without_completed_evidence() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::execute_llvm_sdk_links;
    let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
    let mut selection = llvm_link_selection(&fixture, "timeout");
    selection.timeout = std::time::Duration::from_secs(1);
    let failure = execute_llvm_sdk_links(&selection, &CancellationToken::default()).unwrap_err();
    assert!(
        failure.to_string().contains("deadline") || failure.to_string().contains("timed out"),
        "unexpected deadline failure: {failure}"
    );
    assert!(!selection
        .output_root
        .join("receipt/llvm-sdk-links.receipt.json")
        .exists());
}

#[test]
fn llvm_link_readback_rejects_changed_receipt_sdk_and_extra_members() {
    use aros_common::CancellationToken;
    use aros_toolchain::compatibility::execute_llvm_sdk_links;
    for mutation in ["receipt", "sdk", "extra"] {
        let fixture = create_generic_fixture_with_elf("x86_64", "llvm", None, Some(62));
        let selection = llvm_link_selection(&fixture, "success");
        let proof = execute_llvm_sdk_links(&selection, &CancellationToken::default()).unwrap();
        let path = match mutation {
            "receipt" => proof.receipt_path(),
            "sdk" => selection.output_root.join("relocated-sdk/libclient.a"),
            "extra" => selection.output_root.join("unselected.txt"),
            _ => unreachable!(),
        };
        fs::write(path, b"modified retained evidence\n").unwrap();
        let before = snapshot(&selection.output_root);
        assert!(proof.revalidate(&CancellationToken::default()).is_err());
        assert_eq!(snapshot(&selection.output_root), before);
    }
}
