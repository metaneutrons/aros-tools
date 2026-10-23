//! Synthetic regression coverage for six-phase LLVM SDK compatibility.
//!
//! The local Git source, CMake binding, SDK bytes and compiler drivers in this
//! module are fixtures. They check selection and retained digest wiring; they
//! are not evidence that a real LLVM installation built or linked an SDK.

#![cfg(unix)]

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aros_common::mesa_configuration::{MESA_BASELINE_VARIABLES, MESA_COMPILER_OPTION_VARIABLES};
use aros_common::mesa_configuration_input::{
    MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_OVERRIDE_CONFIG,
};
use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleInventory, MesaGenmoduleInventoryRecord, MesaGenmoduleInventoryScope,
    MesaGenmoduleSelectionBinding, MesaGenmoduleSelectionDocument, MESA_GENMODULE_INVENTORY_SCHEMA,
    MESA_GENMODULE_SELECTION_SCHEMA,
};
use aros_common::mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES;
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
use crate::compatibility::{
    native_compatibility_host_tools, prepare, prepare_host_tool_closure, CompatibilityHostTool,
    CompatibilityPreparationRequest, HostToolClosureRequest,
};
use crate::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use crate::package_verify::VerifiedPackage;
use crate::profiles::Profiles;
use crate::recipe::GitObjectId;
use crate::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest, VerifiedSdkSource};

const LLVM_VERSION: &str = "18.1.0";
const LLVM_SDK_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v6";
const PROFILE_NAME: &str = "pc-x86_64";
const SDK_RELATIVE_ROOT: &str = "products/ci-selected-sdk-layout";
const SDK_INCLUDE_RELATIVE: &str = "products/ci-selected-sdk-layout/include";
const SOURCE_EPOCH: u64 = 946_684_800;
const HISTORICAL_V6_MESA_VERSION: &str = "20.0.0";
const CONFIGURED_MESA_VERSION: &str = "26.0.0";

#[path = "execution_mesa_tests.rs"]
mod mesa_tests;

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

#[derive(Clone, Copy)]
struct MesaFixtureOptions<'a> {
    version: &'a str,
    include_configure: bool,
    mutate_preflight_receipt: Option<&'a Path>,
    mutate_inventory_helper: bool,
}

#[derive(Clone, Copy)]
struct MesaSourceOptions<'a> {
    version: &'a str,
    include_configure: bool,
    clang_path: &'a Path,
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
fn llvm_sdk_binding_exactly_checks_source_declared_companion_roles() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let mut fixture = fixture(root, BindingMode::Valid, "x86_64", 62);

    let role_source_root = root.join("companion-role-fixture");
    fs::create_dir(&role_source_root).unwrap();
    let (source_root, materialized_source_root, identity, _) = generic_source(
        &role_source_root,
        "x86_64",
        62,
        PROFILE_NAME,
        true,
        MesaSourceOptions {
            version: HISTORICAL_V6_MESA_VERSION,
            include_configure: false,
            clang_path: &fixture.request.relocation.first.root.join("bin/clang"),
        },
    );
    let source = verify_sdk_source(&SdkSourceVerificationRequest {
        source_root,
        materialized_source_root,
        source_preset: PROFILE_NAME.into(),
        identity,
    })
    .unwrap();
    bind_fixture_to_shared_source(&mut fixture, &source);

    let request = crate::compatibility::LlvmSdkGeneratedRootRequest {
        source: source.clone(),
        preparation: fixture.request.preparation.clone(),
        cmake_build_root: fixture.request.cmake_build_root.clone(),
        timeout: Duration::from_secs(30),
    };
    let sdk_root = request.cmake_build_root.join(SDK_RELATIVE_ROOT);
    fs::create_dir_all(sdk_root.join("include")).unwrap();
    fs::write(sdk_root.join("include/public.h"), b"/* synthetic SDK */\n").unwrap();

    let expected = source_binding_response(source.materialized_source_root(), source.identity());
    fs::write(
        &fixture.binding_path,
        crate::canonical::bytes(&expected).unwrap(),
    )
    .unwrap();
    let bound =
        crate::compatibility::bind_llvm_sdk_generated_root(&request, &CancellationToken::default())
            .unwrap();
    assert_eq!(bound.sdk_root(), sdk_root.canonicalize().unwrap());

    let helper_response_path = fixture
        .cmake_log
        .parent()
        .unwrap()
        .join("sdk-source-binding-response.json");
    let mut fresh_missing = expected.clone();
    fresh_missing
        .as_object_mut()
        .unwrap()
        .remove("companion_roles");
    fs::write(
        &helper_response_path,
        crate::canonical::bytes(&fresh_missing).unwrap(),
    )
    .unwrap();
    assert!(
        crate::compatibility::bind_llvm_sdk_generated_root(
            &request,
            &CancellationToken::default(),
        )
        .is_err(),
        "accepted an absent source-declared role from fresh validation"
    );
    fs::write(
        &helper_response_path,
        crate::canonical::bytes(&expected).unwrap(),
    )
    .unwrap();

    let mut absent = expected.clone();
    absent.as_object_mut().unwrap().remove("companion_roles");
    assert_rejects_companion_role_binding(&fixture, &request, &absent, "absent roles");

    let mut changed = expected.clone();
    changed["companion_roles"][0]["library_dir"] = json!("lib64");
    assert_rejects_companion_role_binding(&fixture, &request, &changed, "changed role");

    let mut extra = expected.clone();
    let role = extra["companion_roles"][0].clone();
    extra["companion_roles"].as_array_mut().unwrap().push(role);
    assert_rejects_companion_role_binding(&fixture, &request, &extra, "extra role");

    let mut unknown = expected;
    unknown["companion_roles"][0]["inferred_role"] = json!(true);
    assert_rejects_companion_role_binding(&fixture, &request, &unknown, "unknown role field");
}

fn assert_rejects_companion_role_binding(
    fixture: &Fixture,
    request: &crate::compatibility::LlvmSdkGeneratedRootRequest,
    binding: &Value,
    probe: &str,
) {
    fs::write(
        &fixture.binding_path,
        crate::canonical::bytes(binding).unwrap(),
    )
    .unwrap();
    assert!(
        crate::compatibility::bind_llvm_sdk_generated_root(request, &CancellationToken::default(),)
            .is_err(),
        "accepted {probe} in the retained source binding"
    );
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

pub(super) fn fixture_for_host(
    root: &Path,
    binding_mode: BindingMode,
    arch: &str,
    machine: u16,
    host: &'static str,
) -> Fixture {
    fixture_for_host_with_toolchain_profile(root, binding_mode, arch, machine, PROFILE_NAME, host)
}

fn fixture_with_toolchain_profile(
    root: &Path,
    binding_mode: BindingMode,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
) -> Fixture {
    fixture_for_host_with_toolchain_profile(
        root,
        binding_mode,
        arch,
        machine,
        toolchain_profile,
        "linux-x86_64",
    )
}

fn fixture_for_host_with_toolchain_profile(
    root: &Path,
    binding_mode: BindingMode,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
    host: &'static str,
) -> Fixture {
    fixture_for_host_with_mesa_options(
        root,
        binding_mode,
        arch,
        machine,
        toolchain_profile,
        host,
        MesaFixtureOptions {
            version: HISTORICAL_V6_MESA_VERSION,
            include_configure: false,
            mutate_preflight_receipt: None,
            mutate_inventory_helper: false,
        },
    )
}

pub(super) fn configured_mesa_fixture(
    root: &Path,
    missing_configure: bool,
    mutate_preflight_receipt: bool,
) -> Fixture {
    let mutation_path = root.join("reports/mesa-preflight/receipt.json");
    fixture_for_host_with_mesa_options(
        root,
        BindingMode::Valid,
        "x86_64",
        62,
        PROFILE_NAME,
        "linux-x86_64",
        MesaFixtureOptions {
            version: CONFIGURED_MESA_VERSION,
            include_configure: !missing_configure,
            mutate_preflight_receipt: mutate_preflight_receipt.then_some(mutation_path.as_path()),
            mutate_inventory_helper: false,
        },
    )
}

pub(super) fn configured_mesa_fixture_mutating_inventory_helper(root: &Path) -> Fixture {
    fixture_for_host_with_mesa_options(
        root,
        BindingMode::Valid,
        "x86_64",
        62,
        PROFILE_NAME,
        "linux-x86_64",
        MesaFixtureOptions {
            version: CONFIGURED_MESA_VERSION,
            include_configure: true,
            mutate_preflight_receipt: None,
            mutate_inventory_helper: true,
        },
    )
}

fn fixture_for_host_with_mesa_options(
    root: &Path,
    binding_mode: BindingMode,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
    host: &'static str,
    mesa_options: MesaFixtureOptions<'_>,
) -> Fixture {
    let mut llvm = super::llvm_v2_tests::llvm_v2_fixture(root);
    llvm.request.host = host.to_owned();
    let host_roles = native_compatibility_host_tools(host).unwrap();
    let fallback = llvm.request.host_tools.tools["true"].program.clone();
    llvm.request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: root.join(format!("host-tools-{host}")),
        tools: host_roles
            .into_iter()
            .map(|name| CompatibilityHostTool {
                name: name.into(),
                program: llvm
                    .request
                    .host_tools
                    .tools
                    .get(name)
                    .map_or_else(|| fallback.clone(), |identity| identity.program.clone()),
            })
            .collect(),
    })
    .unwrap();
    let compiler_root = root;
    install_link_capable_drivers(&mut llvm.request, compiler_root);

    let clang_path = llvm
        .request
        .relocation
        .first
        .root
        .canonicalize()
        .unwrap()
        .join("bin/clang");
    let (source_root, materialized_source_root, identity, response_path) = generic_source(
        root,
        arch,
        machine,
        toolchain_profile,
        false,
        MesaSourceOptions {
            version: mesa_options.version,
            include_configure: mesa_options.include_configure,
            clang_path: &clang_path,
        },
    );
    if mesa_options.version == CONFIGURED_MESA_VERSION {
        install_mesa_preflight_host_tools(&mut llvm.request, root);
    }
    let helper_path = llvm.request.preparation.helpers["aros-transpiler"]
        .path
        .clone();
    let helpers_root = helper_path.parent().unwrap().to_path_buf();
    let response = source_binding_response(&materialized_source_root, &identity);
    // The real source validator emits sorted JSON with one final LF; CMake
    // retains those exact bytes. Keep the mock on that same boundary.
    fs::write(&response_path, crate::canonical::bytes(&response).unwrap()).unwrap();
    let inventory_path = root.join("mesa-genmodule-selection.json");
    if mesa_options.version == CONFIGURED_MESA_VERSION {
        fs::write(&inventory_path, configured_mesa_inventory(&identity)).unwrap();
    }
    write_executable(
        &helper_path,
        mesa_transpiler_helper(
            &response_path,
            &inventory_path,
            &materialized_source_root,
            &identity,
            mesa_options.version == CONFIGURED_MESA_VERSION,
            &llvm.request.host_tools.root,
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
        mesa_options.mutate_preflight_receipt,
        mesa_options
            .mutate_inventory_helper
            .then_some(helper_path.as_path()),
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

pub(super) fn bind_fixture_to_shared_source(fixture: &mut Fixture, source: &VerifiedSdkSource) {
    let response_path = fixture
        .cmake_log
        .parent()
        .expect("fixture CMake log has a root")
        .join("sdk-source-binding-response.json");
    let response = source_binding_response(source.materialized_source_root(), source.identity());
    fs::write(&response_path, crate::canonical::bytes(&response).unwrap()).unwrap();
    let helper_path = fixture.request.preparation.helpers["aros-transpiler"]
        .path
        .clone();
    write_executable(
        &helper_path,
        mesa_transpiler_helper(
            &response_path,
            &fixture
                .cmake_log
                .parent()
                .unwrap()
                .join("mesa-genmodule-selection.json"),
            source.materialized_source_root(),
            source.identity(),
            false,
            &fixture.request.host_tools.root,
        )
        .as_bytes(),
    );
    fixture.request.preparation.source_root = source.materialized_source_root().to_path_buf();
    fixture.request.preparation.source_tree_sha256 =
        source.identity().source.content_sha256.clone();
    fixture.request.llvm_sdk_source = Some(source.clone());
    fixture.request.consumer_source_date_epoch = Some(source.identity().source_date_epoch);
}

fn generic_source(
    root: &Path,
    arch: &str,
    machine: u16,
    toolchain_profile: &str,
    with_companion_role: bool,
    mesa_options: MesaSourceOptions<'_>,
) -> (PathBuf, PathBuf, SdkIdentity, PathBuf) {
    let source_root = root.join("generic-sdk-source");
    fs::create_dir(&source_root).unwrap();
    git(&source_root, &["init", "-q"]);

    let source_cpu = arch;
    let target_triple = format!("{source_cpu}-unknown-aros");
    let cpu32 = if with_companion_role { "i386" } else { "" };
    let profile = format!(
        "[[targets]]\nname='{PROFILE_NAME}'\ntoolchain_profile='{toolchain_profile}'\narch='{arch}'\nplatform='pc'\nbsp='generic'\nsdk_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='llvm'\ncpu32='{cpu32}'\nuse_mmu=true\nmesa_version='{mesa_version}'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n",
        mesa_version = mesa_options.version,
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

    // Historical v6 compatibility coverage deliberately declares Mesa 20 and
    // has no configure/features inputs. Configured Mesa 26 fixtures add both
    // source files before sealing the consumer input list or committing.
    if mesa_options.version == CONFIGURED_MESA_VERSION {
        if mesa_options.include_configure {
            write_executable(
                &source_root.join("configure"),
                mesa_configure_script(mesa_options.clang_path).as_bytes(),
            );
        }
        write_executable(
            &source_root.join("config/features"),
            mesa_features_script(arch, mesa_options.clang_path).as_bytes(),
        );
        write_source_genmodule_fixture(&source_root);
    }

    let mut input_paths = vec![
        "aros-targets.toml",
        "policy.json",
        "probe.c",
        "probe.cpp",
        "config/make.cfg.in",
    ];
    if mesa_options.version == CONFIGURED_MESA_VERSION {
        if mesa_options.include_configure {
            input_paths.push("configure");
        }
        input_paths.push("config/features");
        input_paths.extend([
            "tools/genmodule/Makefile",
            "tools/genmodule/Makefile.deps",
            aros_common::mesa_configuration_input::MESA_GENMODULE_BASE_CONFIG,
            aros_common::mesa_configuration_input::MESA_GENMODULE_OVERRIDE_CONFIG,
        ]);
    }
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
    let mut contract = json!({
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
    if mesa_options.version == CONFIGURED_MESA_VERSION {
        contract["mesa_configure_target"] = json!(toolchain_profile);
    }
    if with_companion_role {
        contract["companion_roles"] = json!([i386_companion_role()]);
    }
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
    let mut response = json!({
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
    });
    if let Some(companion_roles) = contract.get("companion_roles") {
        response["companion_roles"] = companion_roles.clone();
    }
    response
}

fn i386_companion_role() -> Value {
    json!({
        "selector_cpu": "i386",
        "target": {
            "target_triple": "i386-unknown-aros",
            "abi": {
                "source_cpu": "i386",
                "pointer_width": 32,
                "elf_class": 1,
                "elf_machine": 3,
                "endian": "little",
            },
        },
        "compiler_options": ["--target=i386-unknown-aros"],
        "library_dir": "lib32",
        "required_runtime_library": "builtins-i386",
    })
}

fn cmake_mock_script(
    log: &Path,
    binding_path: &Path,
    sdk_root: &Path,
    response_path: &Path,
    binding_mode: BindingMode,
    mutate_preflight_receipt: Option<&Path>,
    mutate_inventory_helper: Option<&Path>,
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
    let mutation = mutate_preflight_receipt.map_or_else(String::new, |path| {
        format!(
            "printf '%s' 'synthetic preflight receipt mutation' >> {}\n",
            shell_quote(&path.to_string_lossy())
        )
    });
    let helper_mutation = mutate_inventory_helper.map_or_else(String::new, |path| {
        format!(
            "printf '%s' 'synthetic inventory helper mutation' >> {}\n",
            shell_quote(&path.to_string_lossy())
        )
    });
    // The measured host-command closure in this fixture uses exit-0 sentinels
    // for unrelated utilities. Use the real filesystem utility for SDK dirs.
    format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" >> {log}\nif [ \"$1\" = -S ]; then\n  printf '%s\\n' configure >> {log}\n  build=\n  while [ \"$#\" -gt 0 ]; do\n    if [ \"$1\" = -B ]; then shift; build=$1; fi\n    shift\n  done\n  mkdir -p {sdk_include} {sdk_lib}\n  printf '%s\\n' '/* synthetic public SDK header */' > {sdk_header}\n  printf '%s\\n' 'synthetic SDK archive' > {sdk_archive}\n  {configure_binding}{mutation}{helper_mutation}else\n  printf '%s\\n' build >> {log}\n  [ \"$1\" = --build ]\n  case \" $* \" in *' --target includes linklibs '*) ;; *) exit 41 ;; esac\nfi\n",
        log = shell_quote(&log.to_string_lossy()),
        sdk_include = shell_quote(&sdk_root.join("include").to_string_lossy()),
        sdk_lib = shell_quote(&sdk_root.join("lib").to_string_lossy()),
        sdk_header = shell_quote(&sdk_root.join("include/public.h").to_string_lossy()),
        sdk_archive = shell_quote(&sdk_root.join("lib/libfixture.a").to_string_lossy()),
    )
    .replace("mkdir -p", "/bin/mkdir -p")
}

fn install_mesa_preflight_host_tools(request: &mut NativeCompatibilityRequest, root: &Path) {
    let fake_make = request.host_tools.tools["make"].program.clone();
    let gnu_make = gnu_make();
    let dispatch = root.join("mesa-preflight-make-dispatch");
    write_executable(
        &dispatch,
        format!(
            "#!/bin/sh\nset -eu\nif [ \"${{1-}}\" = --silent ] && [ \"${{2-}}\" = --no-builtin-rules ]; then\n  exec {} \"$@\"\nfi\nif [ \"${{1-}}\" = --no-builtin-rules ]; then\n  exec {} \"$@\"\nfi\nexec {} \"$@\"\n",
            shell_quote(&gnu_make.to_string_lossy()),
            shell_quote(&gnu_make.to_string_lossy()),
            shell_quote(&fake_make.to_string_lossy()),
        )
        .as_bytes(),
    );

    let tools = request
        .host_tools
        .tools
        .iter()
        .map(|(name, identity)| CompatibilityHostTool {
            name: name.clone(),
            program: match name.as_str() {
                "bash" => host_program("bash"),
                "make" => dispatch.clone(),
                "mkdir" => host_program("mkdir"),
                _ => identity.program.clone(),
            },
        })
        .collect();
    request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: root.join("mesa-preflight-host-tools"),
        tools,
    })
    .unwrap();
}

fn configured_mesa_inventory(identity: &SdkIdentity) -> Vec<u8> {
    let binding = MesaGenmoduleSelectionBinding {
        profile: PROFILE_NAME.into(),
        kind: "sdk-consumer".into(),
        scope: MesaGenmoduleInventoryScope::ContractRoots,
        contract_sha256: identity.source.contract_sha256.clone(),
    };
    let records = ["mesa3dgl-owner-a", "mesa3dgl-owner-b"]
        .into_iter()
        .map(|owner| MesaGenmoduleInventoryRecord {
            owner: owner.into(),
            declaration_directory: "workbench/libs/mesa".into(),
            module: "mesa3dgl26-0".into(),
            module_type: "library".into(),
            base_configuration: MESA_GENMODULE_BASE_CONFIG.into(),
            override_configuration: Some(MESA_GENMODULE_OVERRIDE_CONFIG.into()),
            consumer_path: format!("runtime/{owner}"),
            components: Vec::new(),
        })
        .collect();
    let document = MesaGenmoduleSelectionDocument {
        schema: MESA_GENMODULE_SELECTION_SCHEMA.into(),
        selection: binding,
        inventory: MesaGenmoduleInventory {
            schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
            records,
        },
    };
    crate::canonical::bytes(&serde_json::to_value(document).unwrap()).unwrap()
}

fn mesa_transpiler_helper(
    response_path: &Path,
    inventory_path: &Path,
    materialized_source_root: &Path,
    identity: &SdkIdentity,
    allow_inventory: bool,
    inventory_host_tools: &Path,
) -> String {
    let mut script = format!(
        "#!/bin/sh\nset -eu\n[ -z \"${{HOME+x}}\" ]\nif [ \"${{1-}}\" = --validate-native-consumer-only ]; then\n  [ \"$PATH\" = /nonexistent ]\n  /bin/cat {}\n  exit\nfi\n[ \"$PATH\" = {} ] || exit 50\n",
        shell_quote(&response_path.to_string_lossy()),
        shell_quote(&inventory_host_tools.to_string_lossy()),
    );
    if allow_inventory {
        let binding = identity.source.contract_sha256.to_string();
        let source = shell_quote(&materialized_source_root.to_string_lossy());
        let inventory = shell_quote(&inventory_path.to_string_lossy());
        write!(script,
            "[ \"${{1-}}\" = --mesa-genmodule-inventory-only ] || exit 51\nshift\n[ \"${{1-}}\" = --native-consumer-profile ] || exit 52\nshift\n[ \"${{1-}}\" = {PROFILE_NAME} ] || exit 53\nshift\n[ \"${{1-}}\" = --native-consumer-contract-sha256 ] || exit 54\nshift\n[ \"${{1-}}\" = {binding} ] || exit 55\nshift\noutput=\nports=\nwhile [ \"$#\" -gt 0 ]; do\n  option=$1\n  shift\n  [ \"$#\" -gt 0 ] || exit 56\n  value=$1\n  shift\n  case \"$option\" in\n    --source-dir) [ \"$value\" = {source} ] || exit 57 ;;\n    --output) output=$value ;;\n    --ports-dir) ports=$value ;;\n    --cpu) [ \"$value\" = x86_64 ] || exit 58 ;;\n    --platform) [ \"$value\" = pc ] || exit 59 ;;\n    --family|--variant|--cpu32|--float-abi) [ -z \"$value\" ] || exit 60 ;;\n    --toolchain) [ \"$value\" = llvm ] || exit 61 ;;\n    --use-mmu) [ \"$value\" = 1 ] || exit 62 ;;\n    --mesa-version) [ \"$value\" = {CONFIGURED_MESA_VERSION} ] || exit 63 ;;\n    *) exit 64 ;;\n  esac\ndone\n[ -n \"$output\" ] && [ -n \"$ports\" ] || exit 65\n/bin/cat {inventory} > \"$output\"\nexit\n"
        ).unwrap();
    }
    script
}

fn write_source_genmodule_fixture(source: &Path) {
    let directory = source.join("tools/genmodule");
    fs::create_dir_all(&directory).unwrap();
    for (relative, bytes) in [
        (
            MESA_GENMODULE_BASE_CONFIG,
            "fixture genmodule base configuration\n",
        ),
        (
            MESA_GENMODULE_OVERRIDE_CONFIG,
            "fixture genmodule Mesa override\n",
        ),
    ] {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let generated_lines = MESA_GENMODULE_MAKE_SUFFIXES
        .iter()
        .map(|suffix| shell_quote(&format!("mesa3dgl26-0_{suffix} += ")))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "#!/bin/sh\nset -eu\nout=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -d ]; then shift; out=$1; fi\n  shift\ndone\n[ -n \"$out\" ] || exit 42\nprintf '%s\\n' {generated_lines} > \"$out/Makefile.mesa3dgl26-0library\"\n"
    );
    let quoted_lines = script
        .lines()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
        .replace('$', "$$");
    fs::write(
        directory.join("Makefile"),
        format!(
            ".PHONY: all\nall: $(GENMODULE)\n$(GENMODULE):\n\t@printf '%s\\n' {quoted_lines} > \"$@\"\n\t@/bin/chmod 755 \"$@\"\n"
        ),
    )
    .unwrap();
    fs::write(directory.join("Makefile.deps"), "# fixture inventory\n").unwrap();
}

fn mesa_configure_script(clang_path: &Path) -> String {
    let compiler_root = clang_path.parent().and_then(Path::parent).unwrap();
    let clangxx_path = clang_path.with_file_name("clang++");
    let mut script = format!(
        "#!/bin/bash\nset -eu\n\
         [ \"$#\" -eq 7 ] || exit 90\n\
         [ \"$1\" = --target=pc-x86_64 ] || exit 91\n\
         [ \"$2\" = --with-toolchain=llvm ] || exit 92\n\
         [ \"$3\" = --with-llvm-version={LLVM_VERSION} ] || exit 93\n\
         [ \"$4\" = --with-aros-toolchain=yes ] || exit 94\n\
         [ \"$5\" = --disable-crosstools ] || exit 95\n\
         [ \"$6\" = {} ] || exit 96\n\
         [ \"$7\" = --with-mesagl={CONFIGURED_MESA_VERSION} ] || exit 97\n\
         mkdir -p config gen/config\n",
        shell_quote(&format!(
            "--with-aros-toolchain-install={}",
            compiler_root.display()
        )),
    );
    script.push_str(
        "source_root=\"${0%/*}\"\n\
         printf 'default: all\\nTOP := %s\\nSRCDIR := %s\\ninclude $(TOP)/config/make.cfg\\nall:\\n\\t@: > poison-target-ran\\n' \"$PWD\" \"$source_root\" > Makefile\n",
    );

    let clang = clang_path.to_string_lossy();
    let clangxx = clangxx_path.to_string_lossy();
    for line in [
        "AROS_TARGET_CPU := x86_64".to_owned(),
        "AROS_TARGET_ARCH := pc".to_owned(),
        format!("TARGET_CC := {clang}"),
        "TARGET_SYSROOT :=".into(),
        "TARGET_CFLAGS := -target-c".into(),
        "DEBUG_CFLAGS := -g".into(),
        "OPTIMIZATION_CFLAGS := -O2".into(),
        format!("TARGET_CPP := {clang} -E"),
        format!("TARGET_CXX := {clangxx}"),
        format!("AROS_CXX := {clangxx}"),
        "TARGET_LTO := no".into(),
        "LTO_CFLAGS :=".into(),
        "CFLAGS_IQUOTE := -iquote".into(),
        "CFLAGS_IQUOTE_END :=".into(),
        "TARGET_CXXFLAGS := -target-cxx".into(),
        "AROS_BUILD_HOST := x86_64-unknown-linux-gnu".into(),
        "TARGET_ISA_CFLAGS := -march=x86-64".into(),
        "TARGET_ISA_CXXFLAGS := -march=x86-64".into(),
        "CONFIG_CFLAGS := -Dconfig-c".into(),
        "CONFIG_CXXFLAGS := -Dconfig-cxx".into(),
        "CONFIG_CPPFLAGS := -Dconfig-cpp".into(),
        "TARGET_CPPFLAGS := -target-cpp".into(),
        "DEBUG_CXXFLAGS := -g".into(),
        "LLVM_NOWARN = $(NOWARN_IMPLICIT_INT)".into(),
        "MESA_COMPILEOPTIONS := -Dmesa-feature".into(),
    ] {
        writeln!(
            script,
            "printf '%s\\n' {} >> config/make.cfg",
            shell_quote(&line)
        )
        .unwrap();
    }
    script.push_str(
        "printf '%s\\n' \"AROS_DEVELOPER := $PWD/AROS/Developer\" >> config/make.cfg\n\
         printf '%s\\n' \"GENDIR := $PWD/gen\" >> config/make.cfg\n\
         printf '%s\\n' \"PORTSDIR := $PWD/Ports\" >> config/make.cfg\n",
    );
    script
}

fn mesa_features_script(cpu: &str, clang_path: &Path) -> String {
    let clang = clang_path.to_string_lossy();
    let clangxx_path = clang_path.with_file_name("clang++");
    let clangxx = clangxx_path.to_string_lossy();
    let mut script = format!(
        "#!/bin/bash\nset -eu\n\
         [ \"$1\" = --host=x86_64-unknown-linux-gnu ] || exit 30\n\
         [ \"$AROS_TARGET_CPU\" = {cpu} ] || exit 31\n\
         [ \"$CC\" = {} ] || exit 32\n\
         [ \"$CONFIG_STATUS\" = features.status ] || exit 33\n\
         [ \"$CPP\" = {} ] || exit 35\n\
         [ \"$CXX\" = {} ] || exit 36\n\
         [ \"$CFLAGS\" = \"-target-c -g -O2\" ] || exit 37\n\
         config_root=\"${{PWD%/config}}\"\n\
         [ \"$PWD\" = \"$config_root/config\" ] || exit 39\n\
         [ \"$AROS_DEVELOPER\" = \"$config_root/AROS/Developer\" ] || exit 38\n\
         [ \"$2\" = \"--with-config-dir=$config_root/gen/config\" ] || exit 34\n\
         config_dir=\"$config_root/gen/config\"\n\
         mkdir -p \"$config_dir\"\n\
         : > \"$config_dir/compiler.cfg\"\n",
        shell_quote(&format!("{clang} ")),
        shell_quote(&format!("{clang} -E ")),
        shell_quote(&format!("{clangxx} ")),
    );
    for name in MESA_COMPILER_OPTION_VARIABLES {
        let value = match *name {
            "CFLAGS_GNU11" => "-std=gnu11",
            "CXXFLAGS_CXX17" => "-std=c++17",
            _ => "",
        };
        script.push_str(&mesa_assignment_command(name, value));
    }
    for (index, (name, value)) in [
        ("CFLAGS_NO_BUILTIN_FFS", "-fno-builtin-ffs"),
        ("CFLAGS_NO_BUILTIN_FFSLL", "-fno-builtin-ffsll"),
        ("CFLAGS_NO_BUILTIN_BSWAP32", "-fno-builtin-bswap32"),
        ("CFLAGS_NO_BUILTIN_BSWAP64", "-fno-builtin-bswap64"),
        ("CFLAGS_NO_BUILTIN_CLZ", "-fno-builtin-clz"),
        ("CFLAGS_NO_BUILTIN_CLZLL", "-fno-builtin-clzll"),
        ("CFLAGS_NO_BUILTIN_CTZ", "-fno-builtin-ctz"),
        ("CFLAGS_NO_BUILTIN_EXPECT", "-fno-builtin-expect"),
        ("CFLAGS_NO_BUILTIN_POPCOUNT", "-fno-builtin-popcount"),
        ("CFLAGS_NO_BUILTIN_POPCOUNTLL", "-fno-builtin-popcountll"),
        ("CFLAGS_NO_BUILTIN_UNREACHABLE", "-fno-builtin-unreachable"),
    ]
    .into_iter()
    .enumerate()
    {
        script.push_str(&mesa_assignment_command(
            name,
            if index % 2 == 0 { value } else { "" },
        ));
    }
    script.push_str(&mesa_assignment_command(
        "NOWARN_IMPLICIT_INT",
        "-Wno-implicit-int",
    ));
    script
}

fn mesa_assignment_command(name: &str, value: &str) -> String {
    format!(
        "printf '%s\\n' {} >> \"$config_dir/compiler.cfg\"\n",
        shell_quote(&format!("{name} := {value}"))
    )
}

fn host_program(name: &str) -> PathBuf {
    let output = Command::new("which")
        .arg(name)
        .output()
        .unwrap_or_else(|error| panic!("cannot locate host {name}: {error}"));
    assert!(
        output.status.success(),
        "the synthetic Mesa execution regression requires host {name}"
    );
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        .canonicalize()
        .unwrap()
}

fn gnu_make() -> PathBuf {
    for name in ["gmake", "make"] {
        let output = Command::new("which").arg(name).output().unwrap();
        if !output.status.success() {
            continue;
        }
        let path = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let version = Command::new(&path).arg("--version").output().unwrap();
        if version.status.success()
            && String::from_utf8_lossy(&version.stdout)
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("GNU Make "))
        {
            return path;
        }
    }
    panic!("the synthetic Mesa execution regression requires GNU Make")
}

fn install_link_capable_drivers(request: &mut NativeCompatibilityRequest, root: &Path) {
    let c64 = root.join("c.elf");
    let cxx64 = root.join("cxx.elf");
    let c32 = root.join("c-i386.elf");
    let cxx32 = root.join("cxx-i386.elf");
    let application = root.join("llvm-sdk-application.elf");
    let mut application_bytes = super::tests::fixture_elf64("__PROGRAM_ENTRIES_LIST__");
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
