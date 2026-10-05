//! Opt-in CLI and CMake process-boundary admission check for a measured local GNU compiler.
//!
//! This proves only that the CLI and the embedded CMake consumer accept a
//! byte-inventoried compiler prefix, bind its measured roles/runtimes, and
//! preserve the source-bound target options. It does not qualify code
//! generation, an SDK, a native build, or board/media behavior.

#![cfg(unix)]

use aros_common::{
    local_toolchain::{LocalToolchainDescriptor, LOCAL_TOOLCHAIN_DESCRIPTOR_FILE},
    sha256_bytes,
    target::TargetProfile,
    toolchain_inventory::{toolchain_tree_inventory, toolchain_tree_inventory_excluding},
    toolchain_layout::{ToolchainToolLayout, TOOLCHAIN_TOOLS_FILE},
    ArosCompilerIdentity, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::{json, Value};
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

const PRESET: &str = "esp32p4-d1001";
const TOOLCHAIN_PROFILE: &str = "rv32-p4-local";
const TRIPLE: &str = "riscv-aros";
const GCC_VERSION: &str = "16.2.0";
const BINUTILS_VERSION: &str = "2.47";
const NATIVE_CONTRACT_RELATIVE: &str = "arch/riscv-esp32p4/native-build-v1.json";
const EXPECTED_NATIVE_CONTRACT_SHA256: &str =
    "92e0d3afc76450b70706d2b3446c189eb691afbf1ada659bf7278602e351fc09";
const EXPECTED_TARGET_CONTRACT_SHA256: &str =
    "4cc304cfd3d7ace734a846f080e52c295db869b972ccff01db0c74326a74dd64";

#[test]
#[ignore = "requires the retained rv32 compiler prefix, P4 source, and evidence parent in AROS_RV3_* env vars"]
fn local_rv32_compiler_admission_is_byte_verified_and_not_release_qualified() {
    let source_root = required_path("AROS_TEST_P4_SOURCE");
    let source_root = canonical_real_directory(&source_root, "P4 source");
    let original_prefix = required_path("AROS_RV3_LOCAL_RV32_COMPILER");
    let original_prefix = canonical_real_directory(&original_prefix, "historical compiler prefix");
    let target_contract_path = canonical_regular_file(
        &required_path("AROS_RV3_TARGET_CONTRACT"),
        "exact RV32 target contract",
    );
    let expected_native_sha = required_env("AROS_TEST_NATIVE_CONTRACT_SHA256");
    let evidence_parent = required_path("AROS_RV3_COMPILER_EVIDENCE_PARENT");
    let evidence_parent = canonical_real_directory(&evidence_parent, "evidence parent");

    assert_eq!(
        expected_native_sha, EXPECTED_NATIVE_CONTRACT_SHA256,
        "this opt-in probe is pinned to the supplied 77-input P4 native contract"
    );
    let source = SourceSnapshot::capture(&source_root, &expected_native_sha);
    let target_bytes = fs::read(&target_contract_path).expect("read exact RV32 target JSON");
    assert_eq!(
        sha256_bytes(&target_bytes).to_string(),
        EXPECTED_TARGET_CONTRACT_SHA256,
        "target JSON must be the exact independently supplied RV32 contract"
    );
    let target_contract = aros_common::elf::riscv::TargetContract::parse(&target_bytes)
        .expect("parse exact RV32 target JSON");
    let target_document: Value =
        serde_json::from_slice(&target_bytes).expect("parse exact RV32 target JSON fields");
    assert_eq!(target_contract.abi(), "ilp32f");
    assert_eq!(
        target_contract.isa(),
        source.contract["abi"]["isa"].as_str().unwrap()
    );
    assert_eq!(
        target_contract.code_model(),
        source.contract["abi"]["code_model"].as_str().unwrap()
    );
    assert_eq!(source.contract["abi"]["target_triple"], TRIPLE);
    assert_eq!(source.contract["abi"]["source_cpu"], "riscv");
    assert_eq!(source.contract["abi"]["flavour"], "standalone");
    assert_eq!(source.contract["qualification"], "experimental-unqualified");
    let profile = TargetProfile::load_from_file(&source_root.join("aros-targets.toml"))
        .expect("load source target profiles")
        .into_iter()
        .find(|profile| profile.name == PRESET)
        .expect("P4 preset is present in source target configuration");
    assert_eq!(profile.toolchain_profile(), TOOLCHAIN_PROFILE);
    assert_eq!(profile.arch.to_string(), "riscv32");
    assert_eq!(profile.float_abi.as_deref(), Some("ilp32f"));
    assert_eq!(
        profile.native_build_contract.as_deref(),
        Some(NATIVE_CONTRACT_RELATIVE)
    );
    assert_eq!(
        profile
            .transpiler
            .as_ref()
            .map(|value| value.toolchain.as_str()),
        Some("gnu")
    );

    let evidence_root = create_retained_directory(&evidence_parent);
    fs::create_dir(evidence_root.join("logs")).expect("create evidence log directory");
    fs::create_dir(evidence_root.join("probes")).expect("create compiler probe directory");
    write_exact(
        &evidence_root.join("native-build-v1.json"),
        &source.contract_bytes,
    );
    write_exact(&evidence_root.join("rv32-target-v1.json"), &target_bytes);
    write_exact(
        &evidence_root.join("aros-targets.toml"),
        &source.targets_bytes,
    );
    write_json(
        &evidence_root.join("source-input-digests.json"),
        &json!({
            "source_root": source_root,
            "native_contract_sha256": expected_native_sha,
            "input_count": source.inputs.len(),
            "inputs": source.inputs.iter().map(|(path, digest)| json!({
                "path": path,
                "sha256": digest,
            })).collect::<Vec<_>>(),
        }),
    );
    let copy_prefix = evidence_root.join("compiler");
    let mut evidence = json!({
        "schema": "aros-local-compiler-admission-evidence-v1",
        "scope": "CLI and CMake local-byte admission only; no codegen, SDK, native build, ABI, or media qualification",
        "preset": PRESET,
        "toolchain_profile": TOOLCHAIN_PROFILE,
        "target_triple": TRIPLE,
        "source_root": source_root,
        "historical_prefix_read_only": original_prefix,
        "retained_copy": copy_prefix,
        "native_contract_sha256": expected_native_sha,
        "target_contract_path": target_contract_path,
        "target_contract_sha256": sha256_bytes(&target_bytes).to_string(),
        "gcc_expected_version": GCC_VERSION,
        "binutils_expected_version": BINUTILS_VERSION,
        "commands": [],
        "probes": [],
    });
    persist_evidence(&evidence_root, &evidence);

    assert_absent_metadata(&original_prefix);
    let original_inventory =
        toolchain_tree_inventory(&original_prefix).expect("inventory original compiler prefix");
    let copy_output = Command::new("/bin/cp")
        .arg("-R")
        .arg(&original_prefix)
        .arg(&copy_prefix)
        .output()
        .expect("run cp -R through Rust without a shell");
    record_process(&evidence_root, &mut evidence, "copy-prefix", &copy_output);
    assert_process_success(&copy_output, "copy historical compiler prefix");
    assert!(
        copy_prefix.is_dir(),
        "cp -R did not create the retained copy"
    );
    let copied_inventory =
        toolchain_tree_inventory(&copy_prefix).expect("inventory copied compiler prefix");
    assert_eq!(
        copied_inventory, original_inventory,
        "copied compiler tree must exactly match input bytes and modes before metadata is added"
    );
    assert_absent_metadata(&copy_prefix);
    write_json(
        &evidence_root.join("pre-metadata-inventory.json"),
        &json!({
            "original_tree_sha256": original_inventory.0,
            "original_files": original_inventory.1,
            "copied_tree_sha256": copied_inventory.0,
            "copied_files": copied_inventory.1,
            "equal_before_metadata": true,
        }),
    );
    evidence["pre_metadata_inventory_equal"] = json!(true);
    persist_evidence(&evidence_root, &evidence);

    let gcc = copy_prefix.join("riscv-aros-gcc");
    let linker = copy_prefix.join("riscv-aros-ld");
    let collector_document_path = copy_prefix.join("aros-collector-tools.json");
    let collector_document_bytes = fs::read(&collector_document_path)
        .expect("read compiler-bound collector metadata from the copied prefix");
    let collector_document: Value = serde_json::from_slice(&collector_document_bytes)
        .expect("parse compiler-bound collector metadata");
    assert_eq!(collector_document["schema"], "aros-collector-tools-v1");
    assert_eq!(collector_document["family"], "gnu");
    assert_eq!(collector_document["linker"], "riscv-aros-ld");
    assert_eq!(collector_document["strip"], "riscv-aros-strip");
    assert_eq!(collector_document["emulation"], "riscvelf_aros");
    let collector_relative = safe_relative_tool_path(
        collector_document["invocation"]
            .as_str()
            .expect("collector invocation path"),
    );
    let collector = checked_executable(&copy_prefix, &collector_relative);
    let collector_file_sha256 = sha256_file_text(&collector);
    write_exact(
        &evidence_root.join("collector-tools.json"),
        &collector_document_bytes,
    );
    evidence["collector_invocation"] = json!(collector_relative);
    evidence["collector_sha256"] = json!(collector_file_sha256);
    evidence["collector_metadata_sha256"] =
        json!(sha256_bytes(&collector_document_bytes).to_string());
    persist_evidence(&evidence_root, &evidence);

    let gcc_version_output = run_probe(
        &evidence_root,
        &mut evidence,
        "gcc-dumpfullversion",
        Command::new(&gcc).arg("-dumpfullversion"),
    );
    let observed_gcc_version = stdout_text(&gcc_version_output).trim().to_owned();
    assert_eq!(observed_gcc_version, GCC_VERSION);
    let target_output = run_probe(
        &evidence_root,
        &mut evidence,
        "gcc-dumpmachine",
        Command::new(&gcc).arg("-dumpmachine"),
    );
    assert_eq!(stdout_text(&target_output).trim(), TRIPLE);
    let binutils_output = run_probe(
        &evidence_root,
        &mut evidence,
        "ld-version",
        Command::new(&linker).arg("--version"),
    );
    let binutils_banner = stdout_text(&binutils_output);
    assert!(
        binutils_banner.starts_with("GNU ld "),
        "unexpected binutils banner: {binutils_banner}"
    );
    let observed_binutils_version = version_token(&binutils_banner)
        .expect("GNU ld --version contains a numeric version")
        .to_owned();
    aros_common::validate_gnu_compiler_versions(GCC_VERSION, &observed_binutils_version)
        .expect("measured binutils version has valid numeric components");
    let observed_binutils_major_minor = observed_binutils_version
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".");
    assert_eq!(
        observed_binutils_major_minor, BINUTILS_VERSION,
        "observed GNU binutils version {observed_binutils_version}, expected {BINUTILS_VERSION}.x"
    );
    evidence["measured_gcc_version"] = json!(observed_gcc_version);
    evidence["measured_binutils_version"] = json!(observed_binutils_version);
    evidence["measured_target_triple"] = json!(stdout_text(&target_output).trim());
    persist_evidence(&evidence_root, &evidence);

    let compiler = ArosCompilerIdentity::Gnu {
        gcc_version: observed_gcc_version,
        binutils_version: observed_binutils_version,
        target: target_contract.clone(),
    };
    let tools = json!({
        "c": "riscv-aros-gcc",
        "cxx": "riscv-aros-g++",
        "assembler": "riscv-aros-as",
        "linker": "riscv-aros-ld",
        "archive": "riscv-aros-ar",
        "ranlib": "riscv-aros-ranlib",
        "strip": "riscv-aros-strip",
        "collector": collector_relative,
        "nm": "riscv-aros-nm",
        "objcopy": "riscv-aros-objcopy",
        "objdump": "riscv-aros-objdump",
    });
    for role in [
        "c",
        "cxx",
        "assembler",
        "linker",
        "archive",
        "ranlib",
        "strip",
        "nm",
        "objcopy",
        "objdump",
    ] {
        let executable = checked_executable(
            &copy_prefix,
            Path::new(tools[role].as_str().expect("tool path")),
        );
        let output = run_probe(
            &evidence_root,
            &mut evidence,
            &format!("{role}-version"),
            Command::new(executable).arg("--version"),
        );
        assert_process_success(&output, &format!("{role} --version"));
    }
    let collector_version = run_probe(
        &evidence_root,
        &mut evidence,
        "collector-version",
        Command::new(&collector).arg("--version"),
    );
    assert_process_success(&collector_version, "collector --version");
    assert_eq!(
        toolchain_tree_inventory(&copy_prefix).expect("remeasure compiler after read-only probes"),
        original_inventory,
        "version probes must leave the copied input prefix unchanged before metadata is added"
    );
    let descriptor_compiler = compiler.clone();
    let layout_bytes = serde_json::to_vec_pretty(&json!({
        "schema": "aros-toolchain-tools-v3",
        "compiler": compiler,
        "target_triple": TRIPLE,
        "tools": tools,
    }))
    .expect("serialize measured v3 GNU tool layout");
    let layout =
        ToolchainToolLayout::parse(&layout_bytes).expect("validate measured v3 GNU layout");
    assert!(layout.has_objdump_role());
    write_exact(&copy_prefix.join(TOOLCHAIN_TOOLS_FILE), &layout_bytes);
    write_exact(&evidence_root.join("toolchain-tools.json"), &layout_bytes);

    let descriptor = LocalToolchainDescriptor::capture(
        &copy_prefix,
        aros_common::target::native_host_key().expect("supported native host"),
        TOOLCHAIN_PROFILE,
        TRIPLE,
        descriptor_compiler,
    )
    .expect("capture local compiler-only descriptor");
    assert_eq!(descriptor.qualification, "local-byte-verified");
    let descriptor_bytes =
        serde_json::to_vec_pretty(&descriptor).expect("serialize local descriptor");
    assert!(!serde_json::from_slice::<Value>(&descriptor_bytes)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("release_id"));
    write_exact(
        &copy_prefix.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
        &descriptor_bytes,
    );
    write_exact(
        &evidence_root.join("toolchain-local.json"),
        &descriptor_bytes,
    );
    evidence["descriptor_qualification"] = json!(descriptor.qualification);
    evidence["descriptor_tree_sha256"] = json!(descriptor.tree_sha256);
    evidence["release_id"] = Value::Null;
    persist_evidence(&evidence_root, &evidence);

    let marker = evidence_root.join("unexpected-tool-execution");
    let isolated = IsolatedCliState::new(&evidence_root, &marker);
    let install = isolated
        .cli(&source_root, "install", &copy_prefix)
        .output()
        .expect("run CLI toolchain install");
    record_process(&evidence_root, &mut evidence, "cli-install", &install);
    assert_process_success(&install, "CLI local toolchain install");
    assert!(stdout_text(&install).contains("without copying it"));
    assert!(stdout_text(&install).contains(copy_prefix.to_str().unwrap()));

    let path = isolated
        .cli(&source_root, "path", &copy_prefix)
        .output()
        .expect("run CLI toolchain path");
    record_process(&evidence_root, &mut evidence, "cli-path", &path);
    assert_process_success(&path, "CLI local toolchain path");
    assert_eq!(
        stdout_text(&path).trim(),
        copy_prefix.canonicalize().unwrap().to_str().unwrap()
    );

    let verify = isolated
        .cli(&source_root, "verify", &copy_prefix)
        .output()
        .expect("run CLI toolchain verify");
    record_process(&evidence_root, &mut evidence, "cli-verify", &verify);
    assert_process_success(&verify, "CLI local toolchain verify");
    assert!(stdout_text(&verify).contains("Verified"));
    assert!(stdout_text(&verify).contains(copy_prefix.to_str().unwrap()));

    assert!(!marker.exists());
    let gcc_original = fs::read(&gcc).expect("snapshot copied GCC tool bytes");
    let gcc_mode = fs::symlink_metadata(&gcc).unwrap().mode() & 0o7777;
    let mut restore_gcc = RestoreFile::new(&gcc, &gcc_original, gcc_mode);
    install_execution_marker_tool(&gcc, &marker);
    let changed_tool = isolated
        .cli(&source_root, "verify", &copy_prefix)
        .output()
        .expect("run changed-tool rejection");
    record_process(
        &evidence_root,
        &mut evidence,
        "changed-tool-rejected",
        &changed_tool,
    );
    assert_process_failure_contains(
        &changed_tool,
        "local toolchain payload differs from its descriptor",
    );
    assert!(
        !marker.exists(),
        "changed GCC must be rejected before execution"
    );
    restore_gcc
        .restore()
        .expect("restore copied GCC bytes and mode");

    let fake_release_manifest = copy_prefix.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    assert!(is_absent_entry(&fake_release_manifest));
    let mut remove_fake_manifest = RemoveFile::new(&fake_release_manifest);
    write_exact(
        &fake_release_manifest,
        b"intentionally malformed release manifest\n",
    );
    let gcc_original = fs::read(&gcc).expect("snapshot copied GCC bytes for manifest rejection");
    let gcc_mode = fs::symlink_metadata(&gcc).unwrap().mode() & 0o7777;
    let mut restore_gcc = RestoreFile::new(&gcc, &gcc_original, gcc_mode);
    install_execution_marker_tool(&gcc, &marker);
    let fake_manifest = isolated
        .cli(&source_root, "verify", &copy_prefix)
        .output()
        .expect("run release-ambiguity rejection");
    record_process(
        &evidence_root,
        &mut evidence,
        "coexisting-release-rejected",
        &fake_manifest,
    );
    assert_process_failure_contains(&fake_manifest, "coexisting release manifest");
    assert!(
        !marker.exists(),
        "coexisting release metadata must be rejected before execution"
    );
    restore_gcc
        .restore()
        .expect("restore copied GCC after release rejection");
    remove_fake_manifest
        .remove()
        .expect("remove fake release manifest from retained copy");

    let final_verify = isolated
        .cli(&source_root, "verify", &copy_prefix)
        .output()
        .expect("verify restored retained copy");
    record_process(
        &evidence_root,
        &mut evidence,
        "cli-verify-after-restoration",
        &final_verify,
    );
    assert_process_success(&final_verify, "CLI verify after restoring copied compiler");
    assert!(is_absent_entry(
        &copy_prefix.join(AROS_TOOLCHAIN_MANIFEST_FILE)
    ));
    assert_eq!(
        fs::read(copy_prefix.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE)).unwrap(),
        descriptor_bytes,
        "CLI admission must not rewrite the local descriptor"
    );

    run_cmake_consumer_admission(
        &evidence_root,
        &mut evidence,
        &copy_prefix,
        &descriptor,
        &descriptor_bytes,
        (&profile, &target_contract, &target_document),
    );

    descriptor
        .verify(&copy_prefix)
        .expect("restored local inventory verifies");
    assert_eq!(
        toolchain_tree_inventory(&original_prefix).expect("remeasure historical input prefix"),
        original_inventory,
        "historical compiler prefix must remain unchanged"
    );
    let retained_inventory =
        toolchain_tree_inventory_excluding(&copy_prefix, &[LOCAL_TOOLCHAIN_DESCRIPTOR_FILE])
            .expect("remeasure retained copied prefix");
    assert_eq!(
        retained_inventory.0, descriptor.tree_sha256,
        "retained copy tree must match its measured descriptor after CLI checks"
    );
    assert_eq!(
        retained_inventory.1, descriptor.files,
        "retained copy file inventory must match its descriptor after CLI checks"
    );
    assert_cli_state_unmanaged(&isolated);
    source.assert_unchanged(&source_root);
    assert_eq!(fs::read(&target_contract_path).unwrap(), target_bytes);
    evidence["historical_prefix_unchanged"] = json!(true);
    evidence["source_contract_and_76_inputs_unchanged"] = json!(true);
    evidence["managed_store_unchanged"] = json!(true);
    evidence["copy_restored_after_negative_probes"] = json!(true);
    evidence["result"] = json!(
        "passed; local byte admission and CMake identity/role binding only, no ABI/build/media qualification"
    );
    persist_evidence(&evidence_root, &evidence);
}

fn run_cmake_consumer_admission(
    evidence_root: &Path,
    evidence: &mut Value,
    copy_prefix: &Path,
    descriptor: &LocalToolchainDescriptor,
    descriptor_bytes: &[u8],
    target: (
        &TargetProfile,
        &aros_common::elf::riscv::TargetContract,
        &Value,
    ),
) {
    let (profile, target_contract, target_document) = target;
    let engine_root = evidence_root.join("embedded-cmake-engine");
    let placement = aros_cmake_engine::materialize(&engine_root)
        .expect("materialize exact embedded engine under retained evidence");
    assert_eq!(placement.digest, aros_cmake_engine::digest());
    assert_eq!(
        placement.written,
        aros_cmake_engine::file_count(),
        "fresh evidence directory must receive every embedded engine file"
    );
    write_json(
        &evidence_root.join("embedded-cmake-engine.json"),
        &json!({
            "root": engine_root,
            "digest": placement.digest,
            "file_count": aros_cmake_engine::file_count(),
            "written": placement.written,
            "removed": placement.removed,
            "reused": placement.reused,
        }),
    );
    evidence["embedded_cmake_engine_digest"] = json!(placement.digest);
    evidence["embedded_cmake_engine_file_count"] = json!(aros_cmake_engine::file_count());
    persist_evidence(evidence_root, evidence);

    let project_root = evidence_root.join("cmake-consumer-project");
    fs::create_dir(&project_root).expect("create retained CMake consumer project");
    let project = r#"cmake_minimum_required(VERSION 3.22)
project(local_gnu_consumer_admission NONE)

# Admission only: no language is enabled, so this fixture never builds code.
include("${ENGINE_ROOT}/ToolchainIdentity.cmake")
include("${ENGINE_ROOT}/toolchains/AROS.cmake")
aros_lock_build_tree_toolchain()

if(NOT AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified")
    message(FATAL_ERROR "Local compiler qualification was lost")
endif()
if(NOT AROS_CROSS_TOOLCHAIN_RELEASE_ID STREQUAL "")
    message(FATAL_ERROR "Local compiler was given a release identity")
endif()

set(_role_variables
    CMAKE_C_COMPILER CMAKE_CXX_COMPILER CMAKE_ASM_COMPILER CMAKE_AR
    CMAKE_RANLIB CMAKE_STRIP CMAKE_NM CMAKE_OBJCOPY CMAKE_OBJDUMP
    AROS_AS_BIN AROS_LINKER_BIN AROS_COLLECT_BIN)
foreach(_role IN LISTS _role_variables)
    if(NOT DEFINED ${_role} OR "${${_role}}" STREQUAL "")
        message(FATAL_ERROR "CMake consumer left ${_role} unbound")
    endif()
    cmake_path(IS_PREFIX AROS_CROSS_TOOLCHAIN_ROOT "${${_role}}"
        NORMALIZE _role_inside)
    if(NOT _role_inside)
        message(FATAL_ERROR "CMake consumer bound ${_role} outside its prefix")
    endif()
endforeach()

set(_runtime_archives
    "${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE};${AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES}")
if(NOT _runtime_archives)
    message(FATAL_ERROR "CMake consumer left compiler runtimes unbound")
endif()
foreach(_runtime IN LISTS _runtime_archives)
    if(NOT _runtime)
        message(FATAL_ERROR "CMake consumer returned an empty runtime path")
    endif()
    cmake_path(IS_PREFIX AROS_CROSS_TOOLCHAIN_ROOT "${_runtime}"
        NORMALIZE _runtime_inside)
    if(NOT _runtime_inside)
        message(FATAL_ERROR "CMake consumer bound a runtime outside its prefix")
    endif()
endforeach()

set(_expected_options
    "-march=${EXPECTED_ISA};-mabi=${EXPECTED_ABI};-mcmodel=${EXPECTED_CODE_MODEL}")
if(EXPECTED_UNALIGNED_ACCESS)
    list(APPEND _expected_options "-mno-strict-align")
else()
    list(APPEND _expected_options "-mstrict-align")
endif()
if(NOT "${AROS_GNU_TARGET_COMPILE_OPTIONS}" STREQUAL "${_expected_options}")
    message(FATAL_ERROR
        "CMake target switches do not match the pinned target contract: "
        "${AROS_GNU_TARGET_COMPILE_OPTIONS} != ${_expected_options}")
endif()

string(JOIN "\n" _admission_report
    "qualification=${AROS_CROSS_TOOLCHAIN_QUALIFICATION}"
    "release_id=${AROS_CROSS_TOOLCHAIN_RELEASE_ID}"
    "descriptor_sha256=${AROS_CROSS_TOOLCHAIN_LOCAL_SHA256}"
    "tree_sha256=${AROS_CROSS_TOOLCHAIN_TREE_SHA256}"
    "target_profile=${AROS_TARGET_PROFILE}"
    "target_triple=${AROS_TARGET_TRIPLE}"
    "c=${CMAKE_C_COMPILER}"
    "cxx=${CMAKE_CXX_COMPILER}"
    "asm=${CMAKE_ASM_COMPILER}"
    "objdump=${CMAKE_OBJDUMP}"
    "collector=${AROS_COLLECT_BIN}"
    "builtins=${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}"
    "cxx_runtime=${AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES}"
    "target_options=${AROS_GNU_TARGET_COMPILE_OPTIONS}")
file(WRITE "${CMAKE_BINARY_DIR}/consumer-admission.txt" "${_admission_report}\n")
"#;
    write_exact(&project_root.join("CMakeLists.txt"), project.as_bytes());

    let root = copy_prefix
        .canonicalize()
        .expect("canonical retained compiler prefix");
    let descriptor_sha = sha256_bytes(descriptor_bytes).to_string();
    let target_isa = target_contract.isa();
    let target_abi = target_contract.abi();
    let target_code_model = target_contract.code_model();
    let unaligned_access = target_document["unaligned_access"]
        .as_bool()
        .expect("target contract unaligned_access boolean");
    assert_eq!(target_isa, target_document["isa"].as_str().unwrap());
    assert_eq!(target_abi, target_document["abi"].as_str().unwrap());
    assert_eq!(
        target_code_model,
        target_document["code_model"].as_str().unwrap()
    );
    let cmake_profile = profile.toolchain_profile();

    let build_root = evidence_root.join("cmake-consumer-build");
    let args = vec![
        "-S".to_owned(),
        project_root.display().to_string(),
        "-B".to_owned(),
        build_root.display().to_string(),
        format!("-DENGINE_ROOT={}", engine_root.display()),
        format!("-DAROS_CROSS_TOOLCHAIN_ROOT={}", root.display()),
        format!("-DAROS_CROSS_TOOLCHAIN_LOCAL_SHA256={descriptor_sha}"),
        "-DAROS_CROSS_TOOLCHAIN_QUALIFICATION=local-byte-verified".to_owned(),
        "-DAROS_TOOLCHAIN=gnu".to_owned(),
        format!("-DAROS_TARGET_CPU={}", profile.arch),
        format!("-DAROS_TARGET_PLATFORM={}", profile.platform),
        format!("-DAROS_TARGET_PROFILE={cmake_profile}"),
        format!("-DAROS_TARGET_TRIPLE={}", descriptor.target_triple),
        format!("-DEXPECTED_ISA={target_isa}"),
        format!("-DEXPECTED_ABI={target_abi}"),
        format!("-DEXPECTED_CODE_MODEL={target_code_model}"),
        format!("-DEXPECTED_UNALIGNED_ACCESS={unaligned_access}"),
    ];
    evidence["cmake_consumer_command"] = json!({
        "program": "cmake",
        "args": args,
        "project_file": project_root.join("CMakeLists.txt"),
        "build_root": build_root,
    });
    persist_evidence(evidence_root, evidence);

    let mut command = Command::new("cmake");
    command.args(&args).current_dir(&project_root);
    for inherited in [
        "CMAKE_TOOLCHAIN_FILE",
        "AROS_CROSS_TOOLCHAIN_ROOT",
        "AROS_CROSS_TOOLCHAIN_QUALIFICATION",
        "AROS_CROSS_TOOLCHAIN_LOCAL_SHA256",
        "AROS_TOOLCHAIN",
        "AROS_TARGET_CPU",
        "AROS_TARGET_PLATFORM",
        "AROS_TARGET_PROFILE",
        "AROS_TARGET_TRIPLE",
    ] {
        command.env_remove(inherited);
    }
    let output = command.output().expect("run real CMake consumer admission");
    record_process(evidence_root, evidence, "cmake-consumer-admission", &output);
    assert_process_success(&output, "embedded CMake local GNU consumer admission");

    let admission_report = fs::read_to_string(build_root.join("consumer-admission.txt"))
        .expect("read CMake consumer admission report");
    let expected_options = format!(
        "-march={target_isa};-mabi={target_abi};-mcmodel={target_code_model};{}",
        if unaligned_access {
            "-mno-strict-align"
        } else {
            "-mstrict-align"
        }
    );
    for expected in [
        "qualification=local-byte-verified".to_owned(),
        "release_id=\n".to_owned(),
        format!("descriptor_sha256={descriptor_sha}\n"),
        format!("tree_sha256={}\n", descriptor.tree_sha256),
        format!("target_profile={cmake_profile}\n"),
        format!("target_triple={}\n", descriptor.target_triple),
        format!("c={}/riscv-aros-gcc\n", root.display()),
        format!("cxx={}/riscv-aros-g++\n", root.display()),
        format!("target_options={expected_options}\n"),
    ] {
        assert!(
            admission_report.contains(&expected),
            "CMake admission report omitted {expected:?}: {admission_report}"
        );
    }

    let stamp = fs::read_to_string(build_root.join(".aros-toolchain-id"))
        .expect("read schema-2 CMake toolchain identity stamp");
    let expected_stamp = format!(
        "schema=2\nqualification=local-byte-verified\nroot={}\ndescriptor_sha256={}\ntarget_profile={}\ntarget_triple={}\ntree_sha256={}\n",
        root.display(),
        descriptor_sha,
        cmake_profile,
        descriptor.target_triple,
        descriptor.tree_sha256,
    );
    assert_eq!(stamp, expected_stamp, "CMake schema-2 identity stamp");
    write_exact(
        &evidence_root.join("cmake-consumer-admission.txt"),
        admission_report.as_bytes(),
    );
    write_exact(
        &evidence_root.join("cmake-toolchain-identity-stamp.txt"),
        stamp.as_bytes(),
    );
    evidence["cmake_consumer_admission"] = json!({
        "passed": true,
        "qualification": "local-byte-verified",
        "release_id": null,
        "descriptor_sha256": descriptor_sha,
        "tree_sha256": descriptor.tree_sha256,
        "preset": profile.name,
        "target_profile": cmake_profile,
        "target_triple": descriptor.target_triple,
        "source_target_options": expected_options,
        "identity_stamp_schema": 2,
        "identity_stamp_file": "cmake-toolchain-identity-stamp.txt",
        "report_file": "cmake-consumer-admission.txt",
        "scope": "CMake admission only; project(NONE), no compiler build or SDK qualification",
    });
    persist_evidence(evidence_root, evidence);
}

struct SourceSnapshot {
    contract_bytes: Vec<u8>,
    contract: Value,
    targets_bytes: Vec<u8>,
    inputs: Vec<(PathBuf, String)>,
}

impl SourceSnapshot {
    fn capture(source_root: &Path, expected_sha256: &str) -> Self {
        let contract_path = source_root.join(NATIVE_CONTRACT_RELATIVE);
        let contract_bytes = fs::read(&contract_path).expect("read pinned P4 native contract");
        assert_eq!(sha256_bytes(&contract_bytes).to_string(), expected_sha256);
        let contract: Value =
            serde_json::from_slice(&contract_bytes).expect("parse P4 native contract");
        assert_eq!(contract["schema_version"], 1);
        assert_eq!(contract["profile"], PRESET);
        let declared_inputs = contract["inputs"]
            .as_array()
            .expect("native contract inputs");
        assert_eq!(
            declared_inputs.len(),
            76,
            "expected exact current source contract input closure"
        );
        let mut inputs = Vec::with_capacity(declared_inputs.len());
        for declaration in declared_inputs {
            let relative =
                safe_source_relative(declaration["path"].as_str().expect("source input path"));
            let path = source_root.join(&relative);
            let metadata = fs::symlink_metadata(&path).expect("inspect declared source input");
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            assert!(path.canonicalize().unwrap().starts_with(source_root));
            let bytes = fs::read(&path).expect("read declared source input");
            let digest = sha256_bytes(&bytes).to_string();
            assert_eq!(
                digest,
                declaration["sha256"].as_str().unwrap(),
                "source input {}",
                relative.display()
            );
            inputs.push((relative, digest));
        }
        Self {
            contract_bytes,
            contract,
            targets_bytes: fs::read(source_root.join("aros-targets.toml"))
                .expect("read source target profile configuration"),
            inputs,
        }
    }

    fn assert_unchanged(&self, source_root: &Path) {
        assert_eq!(
            fs::read(source_root.join(NATIVE_CONTRACT_RELATIVE)).unwrap(),
            self.contract_bytes,
            "CLI changed the source native contract"
        );
        assert_eq!(
            fs::read(source_root.join("aros-targets.toml")).unwrap(),
            self.targets_bytes,
            "CLI changed source target selection"
        );
        for (relative, expected) in &self.inputs {
            let actual = sha256_file_text(&source_root.join(relative));
            assert_eq!(
                &actual,
                expected,
                "CLI changed declared source input {}",
                relative.display()
            );
        }
    }
}

struct IsolatedCliState {
    home: PathBuf,
    cache: PathBuf,
    store: PathBuf,
    execution_marker: PathBuf,
}

impl IsolatedCliState {
    fn new(evidence_root: &Path, execution_marker: &Path) -> Self {
        Self {
            home: evidence_root.join("isolated-home"),
            cache: evidence_root.join("isolated-cache"),
            store: evidence_root.join("isolated-managed-store"),
            execution_marker: execution_marker.to_path_buf(),
        }
    }

    fn cli(&self, source_root: &Path, verb: &str, local: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(source_root)
            .env("AROS_HOME", &self.home)
            .env("AROS_CACHE_DIR", &self.cache)
            .env("AROS_CROSS_TOOLCHAINS_DIR", &self.store)
            .env("AROS_OFFLINE", "true")
            .env("AROS_TEST_EXEC_MARKER", &self.execution_marker)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .args([
                "--diagnostic-format",
                "json",
                "toolchain",
                verb,
                "--preset",
                PRESET,
                "--local",
            ])
            .arg(local);
        command
    }
}

fn assert_cli_state_unmanaged(state: &IsolatedCliState) {
    for path in [&state.home, &state.cache, &state.store] {
        assert!(
            is_absent_entry(path),
            "local byte admission unexpectedly created isolated state at {}",
            path.display()
        );
    }
}

struct RestoreFile {
    path: PathBuf,
    bytes: Vec<u8>,
    mode: u32,
    restored: bool,
}

impl RestoreFile {
    fn new(path: &Path, bytes: &[u8], mode: u32) -> Self {
        Self {
            path: path.to_path_buf(),
            bytes: bytes.to_vec(),
            mode,
            restored: false,
        }
    }

    fn restore(&mut self) -> std::io::Result<()> {
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "copied compiler role has a non-UTF-8 filename",
                )
            })?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let sibling = self.path.with_file_name(format!(
            ".{file_name}.restore-{}-{nonce}",
            std::process::id()
        ));
        let result = (|| {
            let mut replacement = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sibling)?;
            replacement.write_all(&self.bytes)?;
            replacement.sync_all()?;
            drop(replacement);
            fs::set_permissions(&sibling, fs::Permissions::from_mode(self.mode))?;
            // Replacing the copied Mach-O via a fresh inode avoids carrying
            // macOS's invalidated code-signature state from the test sentinel.
            fs::rename(&sibling, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&sibling);
        }
        result?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for RestoreFile {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.restore();
        }
    }
}

struct RemoveFile {
    path: PathBuf,
    removed: bool,
}

impl RemoveFile {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            removed: false,
        }
    }

    fn remove(&mut self) -> std::io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => {
                self.removed = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.removed = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for RemoveFile {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}

fn required_env(name: &str) -> String {
    env::var(name)
        .unwrap_or_else(|_| panic!("{name} is required when this ignored test is explicitly run"))
}

fn required_path(name: &str) -> PathBuf {
    let path = PathBuf::from(required_env(name));
    assert!(path.is_absolute(), "{name} must be an absolute path");
    path
}

fn canonical_real_directory(path: &Path, description: &str) -> PathBuf {
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("cannot inspect {description} {}: {error}", path.display()));
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{description} must be a real directory"
    );
    path.canonicalize()
        .unwrap_or_else(|error| panic!("cannot canonicalize {description}: {error}"))
}

fn canonical_regular_file(path: &Path, description: &str) -> PathBuf {
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("cannot inspect {description} {}: {error}", path.display()));
    assert!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{description} must be a real regular file"
    );
    path.canonicalize()
        .unwrap_or_else(|error| panic!("cannot canonicalize {description}: {error}"))
}

fn create_retained_directory(parent: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    let path = parent.join(format!(
        "rv3-local-compiler-admission-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&path).expect("create fresh retained evidence directory");
    path
}

fn assert_absent_metadata(root: &Path) {
    for file in [
        TOOLCHAIN_TOOLS_FILE,
        LOCAL_TOOLCHAIN_DESCRIPTOR_FILE,
        AROS_TOOLCHAIN_MANIFEST_FILE,
    ] {
        assert!(
            is_absent_entry(&root.join(file)),
            "historical input prefix unexpectedly already contains {file}"
        );
    }
}

fn is_absent_entry(path: &Path) -> bool {
    matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

fn safe_source_relative(value: &str) -> PathBuf {
    let path = Path::new(value);
    assert!(
        !path.as_os_str().is_empty()
            && path.is_relative()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "unsafe native-contract source path {value:?}"
    );
    path.to_path_buf()
}

fn safe_relative_tool_path(value: &str) -> PathBuf {
    let path = Path::new(value);
    assert!(
        !path.as_os_str().is_empty()
            && path.is_relative()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "unsafe collector invocation path {value:?}"
    );
    path.to_path_buf()
}

fn checked_executable(root: &Path, relative: &Path) -> PathBuf {
    assert!(
        relative.is_relative()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "tool path must remain source-relative"
    );
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .unwrap_or_else(|error| panic!("missing compiler role {}: {error}", path.display()));
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_ne!(
        metadata.mode() & 0o111,
        0,
        "tool is not executable: {}",
        path.display()
    );
    assert!(path
        .canonicalize()
        .unwrap()
        .starts_with(root.canonicalize().unwrap()));
    path
}

fn sha256_file_text(path: &Path) -> String {
    sha256_bytes(&fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())))
        .to_string()
}

fn version_token(banner: &str) -> Option<&str> {
    banner
        .lines()
        .next()?
        .split_whitespace()
        .rev()
        .find(|word| word.as_bytes().first().is_some_and(u8::is_ascii_digit))
}

fn run_probe(
    evidence_root: &Path,
    evidence: &mut Value,
    label: &str,
    command: &mut Command,
) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {label}: {error}"));
    record_probe(evidence_root, evidence, label, &output);
    assert_process_success(&output, label);
    output
}

fn record_probe(evidence_root: &Path, evidence: &mut Value, label: &str, output: &Output) {
    let directory = evidence_root.join("probes");
    write_exact(&directory.join(format!("{label}.stdout")), &output.stdout);
    write_exact(&directory.join(format!("{label}.stderr")), &output.stderr);
    evidence["probes"].as_array_mut().unwrap().push(json!({
        "label": label,
        "success": output.status.success(),
        "status_code": output.status.code(),
        "stdout_file": format!("probes/{label}.stdout"),
        "stderr_file": format!("probes/{label}.stderr"),
    }));
    persist_evidence(evidence_root, evidence);
}

fn record_process(evidence_root: &Path, evidence: &mut Value, label: &str, output: &Output) {
    let directory = evidence_root.join("logs");
    write_exact(&directory.join(format!("{label}.stdout")), &output.stdout);
    write_exact(&directory.join(format!("{label}.stderr")), &output.stderr);
    evidence["commands"].as_array_mut().unwrap().push(json!({
        "label": label,
        "success": output.status.success(),
        "status_code": output.status.code(),
        "stdout_file": format!("logs/{label}.stdout"),
        "stderr_file": format!("logs/{label}.stderr"),
    }));
    persist_evidence(evidence_root, evidence);
}

fn assert_process_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_process_failure_contains(output: &Output, needle: &str) {
    assert!(
        !output.status.success(),
        "expected failure containing {needle:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(needle),
        "expected {needle:?} in diagnostic: {stderr}"
    );
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn install_execution_marker_tool(path: &Path, marker: &Path) {
    fs::write(
        path,
        b"#!/bin/sh\nprintf x > \"$AROS_TEST_EXEC_MARKER\"\nexit 0\n",
    )
    .expect("replace copied GCC with a sentinel executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make sentinel executable");
    assert!(!marker.exists());
}

fn write_json(path: &Path, value: &Value) {
    write_exact(
        path,
        &serde_json::to_vec_pretty(value).expect("serialize evidence JSON"),
    );
}

fn write_exact(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn persist_evidence(root: &Path, evidence: &Value) {
    write_json(&root.join("evidence.json"), evidence);
}
