//! Synthetic regression coverage for six-phase LLVM SDK compatibility.
//!
//! The local Git source, CMake binding, SDK bytes and compiler drivers in this
//! module are fixtures. They check selection and retained digest wiring; they
//! are not evidence that a real LLVM installation built or linked an SDK.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{
    sha256_bytes, toolchain_tree_inventory, ArosCompilerIdentity, ArosToolchainManifest,
    CancellationToken,
};
use serde_json::{json, Value};

use super::{
    execute_native_compatibility, export_retained_native_compatibility,
    readback_retained_native_compatibility, NativeCompatibilityRequest,
};
use crate::compatibility::{prepare, CompatibilityPreparationRequest};
use crate::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use crate::package_verify::VerifiedPackage;
use crate::profiles::Profiles;
use crate::recipe::GitObjectId;
use crate::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest};

const LLVM_VERSION: &str = "18.1.0";
const LLVM_SDK_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v6";
const PROFILE_NAME: &str = "pc-x86_64";
const SDK_RELATIVE_ROOT: &str = "products/ci-selected-sdk-layout";
const SDK_INCLUDE_RELATIVE: &str = "products/ci-selected-sdk-layout/include";
const SOURCE_EPOCH: u64 = 946_684_800;

#[derive(Clone, Copy)]
pub(super) enum BindingMode {
    Valid,
    Missing,
    Stale,
}

pub(super) struct Fixture {
    pub(super) request: NativeCompatibilityRequest,
    pub(super) profiles: Profiles,
    pub(super) cmake_log: PathBuf,
    pub(super) binding_path: PathBuf,
}

#[test]
fn executes_six_phases_and_four_llvm_sdk_links_with_schema_v6_digest_binding() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = fixture(temporary.path(), BindingMode::Valid, "x86_64", 62);

    let report = execute_native_compatibility(&fixture.request, &CancellationToken::default())
        .unwrap_or_else(|error| {
            let report_root = &fixture.request.reports_root;
            panic!(
                "LLVM SDK compatibility execution failed: {error}\nCMake argument log:\n{}\nCMake configure stdout:\n{}\nCMake configure stderr:\n{}",
                retained_text(&fixture.cmake_log),
                retained_text(&report_root.join("cmake-consumer.1.stdout.log")),
                retained_text(&report_root.join("cmake-consumer.1.stderr.log")),
            );
        });

    assert_eq!(report.probes.reports.len(), 6);
    assert_eq!(
        report.probes.reports[&crate::compatibility::CompatibilityPhase::CmakeConsumer]
            .commands
            .len(),
        2
    );
    let llvm_sdk = report.llvm_sdk.as_ref().expect("LLVM SDK report");
    assert_eq!(
        llvm_sdk.binding_sha256,
        sha256_bytes(&fs::read(&fixture.binding_path).unwrap())
    );
    assert!(llvm_sdk.sdk_entries > 0);

    let link_outputs = [
        "original-c.elf",
        "relocated-c.elf",
        "original-cxx.elf",
        "relocated-cxx.elf",
    ];
    for name in link_outputs {
        assert_eq!(
            find_named_files(&fixture.request.reports_root, name).len(),
            1,
            "{name}"
        );
    }
    assert!(fixture.cmake_log.is_file());
    let cmake_log = fs::read_to_string(&fixture.cmake_log).unwrap();
    assert_eq!(
        cmake_log
            .lines()
            .filter(|line| matches!(*line, "configure" | "build"))
            .count(),
        2
    );
    assert!(cmake_log.contains("-DAROS_TARGET_PROFILE=pc-x86_64"));
    assert!(cmake_log.contains("consumer.json"));
    assert!(cmake_log.contains("includes"));
    assert!(cmake_log.contains("linklibs"));

    let receipt_bytes = fs::read(&report.receipt.path).unwrap();
    let receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(receipt["schema"], LLVM_SDK_RECEIPT_SCHEMA);
    assert_eq!(
        receipt["llvm_sdk"]["binding_sha256"],
        llvm_sdk.binding_sha256.as_str()
    );
    assert_eq!(
        receipt["llvm_sdk"]["receipt_sha256"],
        llvm_sdk.receipt_sha256.as_str()
    );
    assert_eq!(
        receipt["llvm_sdk"]["sdk_inventory_sha256"],
        llvm_sdk.sdk_inventory_sha256.as_str()
    );
    assert_eq!(receipt["llvm_sdk"]["sdk_entries"], llvm_sdk.sdk_entries);
    assert_eq!(sha256_bytes(&receipt_bytes), report.receipt.sha256);

    let parsed: super::CompatibilityReceiptDocument =
        serde_json::from_slice(&receipt_bytes).unwrap();
    parsed.validate().unwrap();
    for (field, replacement) in [
        (
            "schema",
            json!("aros-toolchain-native-compatibility-receipt-v4"),
        ),
        ("llvm_sdk", Value::Null),
    ] {
        let mut invalid = receipt.clone();
        invalid[field] = replacement;
        let parsed: super::CompatibilityReceiptDocument = serde_json::from_value(invalid).unwrap();
        assert!(parsed.validate().is_err(), "accepted invalid {field}");
    }
    let mut foreign_source = receipt;
    foreign_source["llvm_sdk"]["identity"]["source"]["content_sha256"] = json!("f".repeat(64));
    let parsed: super::CompatibilityReceiptDocument =
        serde_json::from_value(foreign_source).unwrap();
    assert!(
        parsed.validate().is_err(),
        "accepted another SDK source tree"
    );

    let matching_link_receipts = find_files(&fixture.request.reports_root)
        .into_iter()
        .filter(|path| path != &report.receipt.path)
        .filter(|path| {
            fs::read(path).is_ok_and(|bytes| sha256_bytes(&bytes) == llvm_sdk.receipt_sha256)
        })
        .count();
    assert_eq!(matching_link_receipts, 1, "retained LLVM link receipt");
}

#[test]
fn rejects_source_package_target_mismatch_and_missing_or_mismatched_epoch_before_writes() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = fixture(temporary.path(), BindingMode::Valid, "aarch64", 183);
        let error = execute_native_compatibility(&fixture.request, &CancellationToken::default())
            .unwrap_err();
        assert!(error.diagnostics().diagnostics.iter().any(
            |diagnostic| diagnostic.code == aros_common::DiagnosticCode::ProducerCompatibility
        ));
        assert_no_execution_outputs(&fixture);
    }

    for epoch in [None, Some(SOURCE_EPOCH + 1)] {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = fixture(temporary.path(), BindingMode::Valid, "x86_64", 62);
        fixture.request.consumer_source_date_epoch = epoch;
        assert!(
            execute_native_compatibility(&fixture.request, &CancellationToken::default(),).is_err()
        );
        assert_no_execution_outputs(&fixture);
    }
}

#[test]
fn missing_or_stale_cmake_sdk_binding_prevents_the_aggregate_receipt() {
    for mode in [BindingMode::Missing, BindingMode::Stale] {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = fixture(temporary.path(), mode, "x86_64", 62);

        let error = execute_native_compatibility(&fixture.request, &CancellationToken::default())
            .unwrap_err();
        let expected = match mode {
            BindingMode::Missing => "cannot safely open LLVM SDK CMake binding",
            BindingMode::Stale => "LLVM SDK CMake binding differs from its exact source selection",
            BindingMode::Valid => unreachable!("valid binding is not a negative case"),
        };
        assert!(
            error.to_string().contains(expected),
            "expected binding failure {expected:?}, got {error}; CMake argument log:\n{}\nCMake configure stdout:\n{}\nCMake configure stderr:\n{}",
            retained_text(&fixture.cmake_log),
            retained_text(
                &fixture
                    .request
                    .reports_root
                    .join("cmake-consumer.1.stdout.log")
            ),
            retained_text(
                &fixture
                    .request
                    .reports_root
                    .join("cmake-consumer.1.stderr.log")
            ),
        );
        assert!(fixture.cmake_log.is_file());
        assert!(!fixture
            .request
            .reports_root
            .join("native-compatibility.receipt.json")
            .exists());
    }
}

#[test]
fn llvm_retained_readback_and_portable_export_fail_closed_before_execution_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = fixture(temporary.path(), BindingMode::Valid, "x86_64", 62);

    assert!(readback_retained_native_compatibility(&fixture.request, &fixture.profiles).is_err());
    assert!(export_retained_native_compatibility(&fixture.request, &fixture.profiles).is_err());
    assert_no_execution_outputs(&fixture);
}

#[test]
fn rejects_source_declared_compiler_profile_mismatch_before_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = fixture_with_toolchain_profile(
        temporary.path(),
        BindingMode::Valid,
        "x86_64",
        62,
        "another-compiler-profile",
    );
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("LLVM SDK source, preparation and compiler target/profile selection differ"));
    assert_no_execution_outputs(&fixture);
}

#[test]
fn rejects_every_output_root_nested_in_original_source_before_writes() {
    for output in 0..4 {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = fixture(temporary.path(), BindingMode::Valid, "x86_64", 62);
        let source_root = fixture
            .request
            .llvm_sdk_source
            .as_ref()
            .unwrap()
            .original_source_root()
            .to_path_buf();
        let status_before = git_text(&source_root, &["status", "--porcelain"]);
        let nested = source_root.join("forbidden-execution-output");
        match output {
            0 => fixture.request.cmake_build_root = nested,
            1 => fixture.request.upstream_build_root = nested,
            2 => fixture.request.standalone_output_root = nested,
            3 => fixture.request.reports_root = nested,
            _ => unreachable!(),
        }
        let error = execute_native_compatibility(&fixture.request, &CancellationToken::default())
            .unwrap_err();
        assert!(error.to_string().contains(
            "native compatibility output root overlaps a source, engine, package, or fixture input"
        ));
        assert_no_execution_outputs(&fixture);
        assert_eq!(
            git_text(&source_root, &["status", "--porcelain"]),
            status_before
        );
    }
}

pub(super) fn fixture(root: &Path, binding_mode: BindingMode, arch: &str, machine: u16) -> Fixture {
    fixture_with_toolchain_profile(root, binding_mode, arch, machine, PROFILE_NAME)
}

fn fixture_with_toolchain_profile(
    root: &Path,
    binding_mode: BindingMode,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
) -> Fixture {
    let mut llvm = super::llvm_v2_tests::llvm_v2_fixture(root);
    let compiler_root = root;
    install_link_capable_drivers(&mut llvm.request, compiler_root);

    let (source_root, materialized_source_root, identity, response_path) =
        generic_source(root, arch, machine, toolchain_profile);
    let helper_path = llvm.request.preparation.helpers["aros-transpiler"]
        .path
        .clone();
    let helpers_root = helper_path.parent().unwrap().to_path_buf();
    let response = source_binding_response(&materialized_source_root, &identity);
    // The real source validator emits sorted JSON with one final LF; CMake
    // retains those exact bytes. Keep the mock on that same boundary.
    fs::write(&response_path, crate::canonical::bytes(&response).unwrap()).unwrap();
    write_executable(
        &helper_path,
        format!(
            "#!/bin/sh\nset -eu\n[ \"$PATH\" = /nonexistent ]\n[ -z \"${{HOME+x}}\" ]\n[ \"$1\" = --validate-native-consumer-only ]\n/bin/cat {}\n",
            shell_quote(&response_path.to_string_lossy())
        )
        .as_bytes(),
    );
    let work_root = root.join("llvm-sdk-engine-work");
    fs::create_dir(&work_root).unwrap();
    llvm.request.preparation = prepare(&CompatibilityPreparationRequest {
        source_root: materialized_source_root.clone(),
        work_root,
        helpers_root,
    })
    .unwrap();
    let verified_source = verify_sdk_source(&SdkSourceVerificationRequest {
        source_root,
        materialized_source_root,
        source_preset: PROFILE_NAME.into(),
        identity: identity.clone(),
    })
    .unwrap();

    let binding_path = llvm
        .request
        .cmake_build_root
        .join("aros-native-consumer-binding.json");
    let cmake_log = llvm
        .request
        .reports_root
        .parent()
        .unwrap()
        .join("cmake-arguments.log");
    let sdk_root = llvm.request.cmake_build_root.join(SDK_RELATIVE_ROOT);
    let selected_response = match binding_mode {
        BindingMode::Valid | BindingMode::Missing => response_path,
        BindingMode::Stale => {
            let stale_path = root.join("stale-binding.json");
            let mut stale = response;
            stale["contract_sha256"] = json!("f".repeat(64));
            fs::write(&stale_path, crate::canonical::bytes(&stale).unwrap()).unwrap();
            stale_path
        }
    };
    let cmake_script = cmake_mock_script(
        &cmake_log,
        &binding_path,
        &sdk_root,
        &selected_response,
        binding_mode,
    );
    write_executable(&llvm.request.cmake_program, cmake_script.as_bytes());
    llvm.request.llvm_sdk_source = Some(verified_source);
    llvm.request.consumer_source_date_epoch = Some(identity.source_date_epoch);
    let profiles = super::tests::fixture_profiles(llvm.request.upstream_source_commit.as_str());

    Fixture {
        request: llvm.request,
        profiles,
        cmake_log,
        binding_path,
    }
}

fn generic_source(
    root: &Path,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
) -> (PathBuf, PathBuf, SdkIdentity, PathBuf) {
    let source_root = root.join("generic-sdk-source");
    fs::create_dir(&source_root).unwrap();
    git(&source_root, &["init", "-q"]);

    let source_cpu = arch;
    let target_triple = format!("{source_cpu}-unknown-aros");
    let profile = format!(
        "[[targets]]\nname='{PROFILE_NAME}'\ntoolchain_profile='{toolchain_profile}'\narch='{arch}'\nplatform='pc'\nbsp='generic'\nsdk_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='llvm'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n"
    );
    let abi = json!({
        "source_cpu": source_cpu,
        "target_triple": target_triple,
        "pointer_width": if arch == "riscv32" || arch == "arm" { 32 } else { 64 },
        "elf_machine": machine,
        "elf_flags_mask": 0,
        "elf_flags_value": 0,
        "use_mmu": true,
        "flavour": "native",
        "platform_smp": true,
    });
    for (path, bytes) in [
        ("aros-targets.toml", profile.as_bytes()),
        ("policy.json", b"{}".as_slice()),
        ("probe.c", b"/* synthetic SDK C probe */\n".as_slice()),
        ("probe.cpp", b"/* synthetic SDK C++ probe */\n".as_slice()),
    ] {
        fs::write(source_root.join(path), bytes).unwrap();
    }
    fs::create_dir(source_root.join("config")).unwrap();
    fs::write(
        source_root.join("config/make.cfg.in"),
        format!(
            "AROS_DEVELOPER = $(TOP)/{SDK_RELATIVE_ROOT}\nAROS_INCLUDES = $(AROS_DEVELOPER)/include\n"
        ),
    )
    .unwrap();

    let input_paths = [
        "aros-targets.toml",
        "policy.json",
        "probe.c",
        "probe.cpp",
        "config/make.cfg.in",
    ];
    let inputs = input_paths
        .iter()
        .map(|path| {
            json!({
                "path": path,
                "sha256": sha256_bytes(&fs::read(source_root.join(path)).unwrap()),
            })
        })
        .collect::<Vec<_>>();
    let target = SdkTarget {
        target_triple,
        abi: serde_json::from_value(abi).unwrap(),
    };
    let contract = json!({
        "schema": "aros-sdk-consumer-contract-v1",
        "profile": PROFILE_NAME,
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "inputs": inputs,
        "metamake_projection": "policy.json",
        "target": target,
        "native_sdk_link_probes": {
            "c": {"source": "probe.c", "libraries": ["fixture"]},
            "cxx": {"source": "probe.cpp", "libraries": ["fixture"]},
        },
    });
    let contract_bytes = serde_json::to_vec(&contract).unwrap();
    fs::write(source_root.join("consumer.json"), &contract_bytes).unwrap();
    fs::create_dir(source_root.join("cmake")).unwrap();
    fs::write(
        source_root.join("cmake/engine.cmake"),
        b"excluded engine material\n",
    )
    .unwrap();
    fs::create_dir(source_root.join("docs")).unwrap();
    fs::write(
        source_root.join("docs/source.txt"),
        b"committed synthetic source\n",
    )
    .unwrap();
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &["commit", "-qm", "synthetic generic SDK source"],
    );
    assert!(git_text(&source_root, &["status", "--porcelain"]).is_empty());

    let commit = GitObjectId::try_from(git_text(&source_root, &["rev-parse", "HEAD^0"])).unwrap();
    let tree = GitObjectId::try_from(git_text(&source_root, &["rev-parse", "HEAD:"])).unwrap();
    let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: source_root.clone(),
        expected_source: CommittedSourceIdentity { commit, tree },
        output_root: root.join("generic-sdk-materialized"),
    })
    .unwrap();
    assert!(!materialized.root.join("cmake").exists());
    let identity = SdkIdentity {
        sdk_id: PROFILE_NAME.into(),
        version: "1.0.0".into(),
        target,
        source: SdkSourceIdentity {
            commit: materialized.source_commit.clone(),
            git_tree: materialized.source_tree.clone(),
            content_sha256: materialized.source_tree_sha256.clone(),
            contract_sha256: sha256_bytes(&contract_bytes),
        },
        source_date_epoch: SOURCE_EPOCH,
    };
    let response_path = root.join("sdk-source-binding-response.json");
    (source_root, materialized.root, identity, response_path)
}

fn source_binding_response(materialized_root: &Path, identity: &SdkIdentity) -> Value {
    let contract_path = materialized_root.join("consumer.json");
    let contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    json!({
        "schema": "aros-sdk-consumer-validation-v1",
        "qualification": "source-binding-not-graph-or-build-proof",
        "source_dir": materialized_root,
        "contract_path": contract_path,
        "contract_sha256": identity.source.contract_sha256,
        "profile": PROFILE_NAME,
        "target": identity.target,
        "exec_smp": false,
        "input_paths": contract["inputs"].as_array().unwrap().iter().map(|input| input["path"].clone()).collect::<Vec<_>>(),
        "sdk_include_relative": SDK_INCLUDE_RELATIVE,
    })
}

fn cmake_mock_script(
    log: &Path,
    binding_path: &Path,
    sdk_root: &Path,
    response_path: &Path,
    binding_mode: BindingMode,
) -> String {
    let configure_binding = if matches!(binding_mode, BindingMode::Missing) {
        String::new()
    } else {
        format!(
            "/bin/cat {} > {}\n",
            shell_quote(&response_path.to_string_lossy()),
            shell_quote(&binding_path.to_string_lossy())
        )
    };
    // The measured host-command closure in this fixture uses exit-0 sentinels
    // for unrelated utilities. Use the real filesystem utility for SDK dirs.
    format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" >> {log}\nif [ \"$1\" = -S ]; then\n  printf '%s\\n' configure >> {log}\n  build=\n  while [ \"$#\" -gt 0 ]; do\n    if [ \"$1\" = -B ]; then shift; build=$1; fi\n    shift\n  done\n  mkdir -p {sdk_include} {sdk_lib}\n  printf '%s\\n' '/* synthetic public SDK header */' > {sdk_header}\n  printf '%s\\n' 'synthetic SDK archive' > {sdk_archive}\n  {configure_binding}else\n  printf '%s\\n' build >> {log}\n  [ \"$1\" = --build ]\n  case \" $* \" in *' --target includes linklibs '*) ;; *) exit 41 ;; esac\nfi\n",
        log = shell_quote(&log.to_string_lossy()),
        sdk_include = shell_quote(&sdk_root.join("include").to_string_lossy()),
        sdk_lib = shell_quote(&sdk_root.join("lib").to_string_lossy()),
        sdk_header = shell_quote(&sdk_root.join("include/public.h").to_string_lossy()),
        sdk_archive = shell_quote(&sdk_root.join("lib/libfixture.a").to_string_lossy()),
    )
    .replace("mkdir -p", "/bin/mkdir -p")
}

fn install_link_capable_drivers(request: &mut NativeCompatibilityRequest, root: &Path) {
    let c64 = root.join("c.elf");
    let cxx64 = root.join("cxx.elf");
    let c32 = root.join("c-i386.elf");
    let cxx32 = root.join("cxx-i386.elf");
    let application = root.join("llvm-sdk-application.elf");
    let mut application_bytes = super::tests::fixture_elf64("__TOOLCHAIN_LIST__");
    application_bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
    application_bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
    fs::write(&application, application_bytes).unwrap();

    for package_root in [
        request.relocation.first.root.as_path(),
        request.relocation.second.root.as_path(),
    ] {
        let runtime_root = package_root.join("lib");
        fs::create_dir_all(&runtime_root).unwrap();
        fs::write(
            runtime_root.join("libcompiler-rt.a"),
            b"synthetic compiler runtime\n",
        )
        .unwrap();
        let bin = package_root.join("bin");
        write_executable(
            &bin.join("clang"),
            compiler_script(&c64, &c32, &application).as_bytes(),
        );
        write_executable(
            &bin.join("clang++"),
            compiler_script(&cxx64, &cxx32, &application).as_bytes(),
        );
    }

    // Driver bytes and newly added runtime entries are part of each complete
    // package tree. Remeasure both independent roots after every driver edit.
    let compiler = ArosCompilerIdentity::Llvm {
        version: LLVM_VERSION.into(),
    };
    let first = reinventory_package(&request.relocation.first.root, request, &compiler);
    let second = reinventory_package(&request.relocation.second.root, request, &compiler);
    assert_eq!(first.manifest, second.manifest);
    request.relocation.first.verified = first;
    request.relocation.second.verified = second;
}

fn reinventory_package(
    package_root: &Path,
    request: &NativeCompatibilityRequest,
    compiler: &ArosCompilerIdentity,
) -> VerifiedPackage {
    let (tree_sha256, files) = toolchain_tree_inventory(package_root).unwrap();
    let old = if package_root == request.relocation.first.root {
        &request.relocation.first.verified
    } else {
        &request.relocation.second.verified
    };
    let manifest = ArosToolchainManifest {
        schema: 2,
        release_id: "synthetic-toolchain-v2-llvm-sdk-fixture".into(),
        host: request.host.clone(),
        target_profile: request.profile.name().into(),
        target_triple: request.profile.target_triple().into(),
        tree_sha256,
        llvm_version: None,
        compiler: Some(compiler.clone()),
        recipe_sha256: "a".repeat(64),
        source_lock_sha256: "b".repeat(64),
        profiles_sha256: request.profile.document_sha256().to_string(),
        source_commit: request
            .package_source_commit
            .as_ref()
            .unwrap()
            .as_str()
            .into(),
        producer_commit: "c".repeat(40),
        tools_commit: "d".repeat(40),
        source_date_epoch: 1,
        capabilities: request.profile.capabilities().to_vec(),
        build_environment: serde_json::Map::new(),
        files,
    };
    manifest.validate().unwrap();
    fs::write(
        package_root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    VerifiedPackage {
        manifest,
        archive_sha256: old.archive_sha256.clone(),
        archive_size: old.archive_size,
    }
}

fn compiler_script(standalone_64: &Path, standalone_32: &Path, application: &Path) -> String {
    let role_version = format!("clang version {LLVM_VERSION}");
    format!(
        "#!/bin/sh\nset -eu\nif [ \"${{1-}}\" = --version ]; then printf '%s\\n' {version}; exit 0; fi\nif [ \"${{2-}}\" = -dumpmachine ]; then printf '%s\\n' \"${{1#--target=}}\"; exit 0; fi\nout=\ntarget=\nsysroot=\nmap=\nfor argument in \"$@\"; do\n  case \"$argument\" in --target=*) target=${{argument#--target=}} ;; --sysroot=*) sysroot=${{argument#--sysroot=}} ;; -Wl,-Map=*) map=${{argument#-Wl,-Map=}}; map=${{map%%,*}} ;; esac\ndone\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -o ]; then shift; out=$1; fi\n  shift\ndone\n[ -n \"$out\" ] || exit 51\nif [ -n \"$map\" ]; then\n  /bin/cp {application} \"$out\"\n  printf '%s\\n' 'synthetic normal-link map' > \"$map\"\n  compiler_root=${{0%/bin/*}}\n  printf '%s\\n' \"$sysroot/lib/libfixture.a\" \"$compiler_root/lib/libcompiler-rt.a\" \"$TMPDIR/probe-Az09Qq.o\"\nelse\n  if [ \"$target\" = i386-unknown-aros ]; then fixture={standalone_32}; else fixture={standalone_64}; fi\n  /bin/cp \"$fixture\" \"$out\"\nfi\n",
        version = shell_quote(&role_version),
        application = shell_quote(&application.to_string_lossy()),
        standalone_32 = shell_quote(&standalone_32.to_string_lossy()),
        standalone_64 = shell_quote(&standalone_64.to_string_lossy()),
    )
}

fn assert_no_execution_outputs(fixture: &Fixture) {
    assert!(
        !fixture.cmake_log.exists(),
        "CMake ran before preflight completed"
    );
    for root in [
        &fixture.request.cmake_build_root,
        &fixture.request.upstream_build_root,
        &fixture.request.standalone_output_root,
        &fixture.request.reports_root,
    ] {
        assert!(!root.exists(), "preflight created {}", root.display());
    }
}

fn retained_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| format!("<unavailable: {error}>"))
        .trim()
        .to_owned()
}

fn find_named_files(root: &Path, name: &str) -> Vec<PathBuf> {
    find_files(root)
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|candidate| candidate == name))
        .collect()
}

fn find_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !root.exists() {
        return found;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=AROS LLVM SDK compatibility test",
            "-c",
            "user.email=aros-llvm-sdk-test@example.invalid",
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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
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

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
