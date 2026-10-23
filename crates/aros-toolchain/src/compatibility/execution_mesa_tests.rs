//! Synthetic owner regressions for the configured Mesa 26 execution path.
//!
//! The configure script, feature script, CMake consumer, SDK, and compiler
//! drivers are test fixtures. These checks cover runtime ownership and receipt
//! binding; they do not build a real AROS SDK or qualify a compiler.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use crate::compatibility::{
    execute_native_compatibility_with_export, readback_portable_native_compatibility,
    NativeCompatibilityInputClaims, PortableNativeCompatibilityRequest,
};
use aros_common::{sha256_bytes, CancellationToken, Sha256Digest};
use serde_json::Value;

use super::*;

const MESA_RECEIPT_SCHEMA: &str = "aros-mesa-preflight-v7";
const NATIVE_RECEIPT_V7: &str = "aros-toolchain-native-compatibility-receipt-v7";

#[test]
fn configured_mesa_26_runs_source_preflight_and_binds_its_receipt_to_cmake_and_v7() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = configured_mesa_fixture(temporary.path(), false, false);
    let source_root = &fixture.request.preparation.source_root;
    let contract: Value =
        serde_json::from_slice(&fs::read(source_root.join("consumer.json")).unwrap()).unwrap();
    let input_paths = contract["inputs"].as_array().unwrap();
    for expected in ["configure", "config/features"] {
        assert!(
            input_paths.iter().any(|input| input["path"] == expected),
            "Mesa 26 fixture did not seal {expected} into consumer inputs"
        );
        assert!(source_root.join(expected).is_file());
    }
    assert!(fs::read_to_string(source_root.join("aros-targets.toml"))
        .unwrap()
        .contains("mesa_version='26.0.0'"));

    let independent = super::super::retained::ExpectedRetainedEvidence::prepare(
        &fixture.request,
        &fixture.profiles,
    )
    .unwrap();
    let execution = execute_native_compatibility_with_export(
        &fixture.request, &fixture.profiles, &CancellationToken::default(),
    )
        .unwrap_or_else(|error| {
            panic!(
                "synthetic Mesa owner execution failed: {error}\nCMake arguments:\n{}\nconfigure stderr:\n{}\nfeatures stderr:\n{}\nfeature-inputs stderr:\n{}\nconfigured-values stderr:\n{}",
                fs::read_to_string(&fixture.cmake_log).unwrap_or_default(),
                fs::read_to_string(fixture.request.reports_root.join("mesa-preflight/configure.stderr"))
                    .unwrap_or_default(),
                fs::read_to_string(fixture.request.reports_root.join("mesa-preflight/features.stderr"))
                    .unwrap_or_default(),
                fs::read_to_string(fixture.request.reports_root.join("mesa-preflight/feature-inputs.stderr"))
                    .unwrap_or_default(),
                fs::read_to_string(fixture.request.reports_root.join("mesa-preflight/configured-values.stderr"))
                    .unwrap_or_default(),
            )
        });
    let report = execution.report();

    let preflight_root = fixture.request.reports_root.join("mesa-preflight");
    let mut expected_stages: Vec<String> = [
        "configure",
        "feature-inputs",
        "features",
        "configured-values",
        "genmodule-inventory",
        "genmodule-build",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for owner in ["mesa3dgl-owner-a", "mesa3dgl-owner-b"] {
        expected_stages.push(
            aros_common::mesa_genmodule_collection::mesa_genmodule_scope_stage(owner).unwrap(),
        );
        expected_stages.push(format!("genmodule-make-{owner}"));
    }
    for stage in &expected_stages {
        assert!(
            preflight_root.join(format!("{stage}.stdout")).is_file(),
            "{stage}"
        );
        assert!(
            preflight_root.join(format!("{stage}.stderr")).is_file(),
            "{stage}"
        );
    }
    let preflight_receipt_bytes = fs::read(preflight_root.join("receipt.json")).unwrap();
    let preflight_receipt_sha256 = sha256_bytes(&preflight_receipt_bytes);
    let preflight_receipt: Value = serde_json::from_slice(&preflight_receipt_bytes).unwrap();
    assert_eq!(preflight_receipt["schema"], MESA_RECEIPT_SCHEMA);
    assert_eq!(preflight_receipt["qualification"], "local-configure-only");
    let stages = preflight_receipt["stages"].as_array().unwrap();
    assert_eq!(stages.len(), expected_stages.len());
    for (stage, name) in stages.iter().zip(&expected_stages) {
        for stream in ["stdout", "stderr"] {
            assert_eq!(
                stage[stream],
                sha256_bytes(&fs::read(preflight_root.join(format!("{name}.{stream}"))).unwrap())
                    .as_str(),
                "selected stage {name} {stream} is bound in execution order",
            );
        }
    }
    let collection = &preflight_receipt["genmodule_collection"];
    let outputs = collection["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 2, "two distinct source producer owners");
    let measured_helper = &fixture.request.preparation.helpers["aros-transpiler"].sha256;
    assert_eq!(stages[4]["program"], measured_helper.as_str());
    for (scope, generated) in [(6, 7), (8, 9)] {
        assert_eq!(stages[scope]["program"], stages[3]["program"]);
        assert_eq!(stages[scope]["environment"], stages[3]["environment"]);
        assert_eq!(stages[generated]["program"], collection["producer_sha256"]);
    }
    assert_eq!(outputs[0]["producer"]["owner"], "mesa3dgl-owner-a");
    assert_eq!(outputs[1]["producer"]["owner"], "mesa3dgl-owner-b");
    assert_eq!(
        preflight_receipt["configured_paths"]["AROS_TARGET_CPU"],
        "x86_64"
    );
    assert_eq!(
        preflight_receipt["configured_paths"]["AROS_TARGET_ARCH"],
        "pc"
    );
    assert_eq!(
        preflight_receipt["make_values"]["CFLAGS_GNU11"],
        "-std=gnu11"
    );
    assert_eq!(
        preflight_receipt["make_values"]["CXXFLAGS_CXX17"],
        "-std=c++17"
    );
    for name in MESA_BASELINE_VARIABLES {
        assert!(
            preflight_receipt["make_values"].get(*name).is_some(),
            "fixture omitted measured baseline {name}"
        );
    }

    let cmake_arguments = fs::read_to_string(&fixture.cmake_log).unwrap();
    assert!(cmake_arguments.contains("-DAROS_MESA_CONFIGURATION_MODE=bound"));
    let input_argument = cmake_arguments
        .lines()
        .find_map(|line| line.strip_prefix("-DAROS_MESA_CONFIGURATION_INPUT="))
        .expect("fake CMake received the bound configuration input");
    let input_path = PathBuf::from(input_argument);
    let input_bytes = fs::read(&input_path).unwrap();
    let input: Value = serde_json::from_slice(&input_bytes).unwrap();
    assert_eq!(input["version"], 7);
    assert_eq!(
        input["genmodule_collection"]["outputs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(input["receipt_sha256"], preflight_receipt_sha256.as_str());
    assert!(cmake_arguments.contains(&format!(
        "-DAROS_MESA_CONFIGURATION_INPUT_SHA256={}",
        sha256_bytes(&input_bytes)
    )));

    assert_eq!(
        report.mesa_preflight_receipt_sha256.as_ref(),
        Some(&preflight_receipt_sha256)
    );
    let aggregate_bytes = fs::read(&report.receipt.path).unwrap();
    let aggregate: Value = serde_json::from_slice(&aggregate_bytes).unwrap();
    assert_eq!(aggregate["schema"], NATIVE_RECEIPT_V7);
    assert_eq!(
        aggregate["mesa_preflight_receipt_sha256"],
        preflight_receipt_sha256.as_str()
    );
    assert_eq!(sha256_bytes(&aggregate_bytes), report.receipt.sha256);
    let retained = readback_retained_native_compatibility(&fixture.request, &fixture.profiles)
        .expect("selected v7 evidence must pass retained consistency readback");
    assert_eq!(retained.receipt_sha256, report.receipt.sha256);
    let exported = execution.evidence();
    assert_eq!(exported.receipt_sha256(), &report.receipt.sha256);
    let selected = independent
        .portable_expectations(&fixture.request, &fixture.profiles)
        .unwrap();
    let identity = selected.llvm_sdk.unwrap().identity();
    let llvm_selection = selected.llvm_sdk.unwrap();
    let mut changed_root = serde_json::to_value(selected.mesa_preflight.unwrap()).unwrap();
    changed_root["compiler_root"] =
        serde_json::to_value(llvm_selection.compiler_root().parent().unwrap()).unwrap();
    let changed_root: crate::compatibility::MesaPreflightSelection =
        serde_json::from_value(changed_root).unwrap();
    changed_root.validate().unwrap();
    assert!(
        changed_root
            .validate_binding(
                &fixture.request.profile,
                selected.package.compiler,
                llvm_selection.compiler_tree_sha256(),
                llvm_selection.compiler_root(),
                &fixture.request.preparation.source_tree_sha256,
                identity,
            )
            .is_err(),
        "a structurally valid rewritten root must not replace the independent LLVM selection"
    );
    let claims = NativeCompatibilityInputClaims::parse(
        execution.inputs().bytes(),
        execution.inputs().sha256(),
    )
    .unwrap();
    let input_document: Value = serde_json::from_slice(claims.bytes()).unwrap();
    assert_eq!(
        input_document["schema"],
        "aros-toolchain-compatibility-inputs-v4"
    );
    assert!(
        claims
            .expectations_with_llvm_sdk(
                selected.package.clone(),
                &fixture.profiles,
                &fixture.request.profile,
                identity,
            )
            .is_err(),
        "legacy policy must not infer Mesa requirements from claims"
    );
    assert!(
        claims
            .expectations_with_llvm_sdk_policy(
                selected.package.clone(),
                &fixture.profiles,
                &fixture.request.profile,
                identity,
                true,
            )
            .is_err(),
        "portable expectations must not adopt a collection binding from claims"
    );
    let source = fixture.request.llvm_sdk_source.as_ref().unwrap();
    let source_profile = crate::compatibility::SourceMesaPreflightProfile::load(
        &fixture.request.preparation.source_root,
        source.source_profile(),
    )
    .unwrap();
    let binding = source_profile.genmodule_inventory_binding(
        aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::ContractRoots,
    );
    let helper_sha256 = fixture.request.preparation.helpers["aros-transpiler"]
        .sha256
        .clone();
    let expected = claims
        .expectations_with_llvm_sdk_mesa_selection(
            selected.package.clone(),
            &fixture.profiles,
            &fixture.request.profile,
            identity,
            (&binding, &helper_sha256),
        )
        .unwrap();
    let mut wrong_binding = binding.clone();
    wrong_binding.contract_sha256 = Sha256Digest::parse(&"f".repeat(64)).unwrap();
    assert!(claims
        .expectations_with_llvm_sdk_mesa_selection(
            selected.package.clone(),
            &fixture.profiles,
            &fixture.request.profile,
            identity,
            (&wrong_binding, &helper_sha256),
        )
        .is_err());
    let wrong_helper = Sha256Digest::parse(&"e".repeat(64)).unwrap();
    assert!(claims
        .expectations_with_llvm_sdk_mesa_selection(
            selected.package.clone(),
            &fixture.profiles,
            &fixture.request.profile,
            identity,
            (&binding, &wrong_helper),
        )
        .is_err());
    let download = tempfile::tempdir().unwrap();
    let mut mesa_members = std::collections::BTreeSet::new();
    for (name, bytes) in exported.files() {
        fs::write(download.path().join(name), bytes).unwrap();
        if name.starts_with("mesa-preflight-") {
            mesa_members.insert(name.to_owned());
        }
    }
    let expected_members = crate::compatibility::mesa_preflight_evidence::expected_members(
        &preflight_root.canonicalize().unwrap(),
        &preflight_receipt_bytes,
    )
    .unwrap();
    assert_eq!(mesa_members, expected_members.into_keys().collect());
    let downloaded = PortableNativeCompatibilityRequest {
        directory: download.path().canonicalize().unwrap(),
        manifest_sha256: exported.manifest_sha256().clone(),
        expected,
    };
    readback_portable_native_compatibility(&downloaded).unwrap();
    let mut missing_policy = downloaded.clone();
    missing_policy.expected.mesa_preflight = None;
    assert!(readback_portable_native_compatibility(&missing_policy).is_err());
    let path = download.path().join("mesa-preflight-features.stdout");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"corrupt stage log").unwrap();
    assert!(readback_portable_native_compatibility(&downloaded).is_err());
    fs::write(&path, original).unwrap();
    readback_portable_native_compatibility(&downloaded).unwrap();
    let mut wrong_manifest = selected.package.manifest.clone();
    wrong_manifest.tree_sha256 = "1".repeat(64);
    let mut wrong_package = downloaded.clone();
    wrong_package.expected.package.manifest = &wrong_manifest;
    assert!(readback_portable_native_compatibility(&wrong_package).is_err());
}

#[test]
fn inventory_helper_mutation_by_fake_cmake_stops_before_upstream_configure_and_aggregate() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = configured_mesa_fixture_mutating_inventory_helper(temporary.path());
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("compatibility helper changed after preparation"),
        "unexpected failure after explicit inventory-helper mutation: {error}"
    );
    assert!(fixture.cmake_log.is_file(), "fake CMake did not run");
    let helper = &fixture.request.preparation.helpers["aros-transpiler"].path;
    assert!(String::from_utf8_lossy(&fs::read(helper).unwrap())
        .contains("synthetic inventory helper mutation"));
    assert!(
        !fixture
            .request
            .reports_root
            .join("upstream-configure.report.json")
            .exists(),
        "upstream configure ran after selected helper revalidation failed"
    );
    assert!(
        !fixture
            .request
            .reports_root
            .join("native-compatibility.receipt.json")
            .exists(),
        "published an aggregate receipt after selected helper revalidation failed"
    );
}

#[test]
fn preflight_receipt_mutation_by_fake_cmake_stops_before_upstream_configure_and_aggregate() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = configured_mesa_fixture(temporary.path(), false, true);
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();

    assert!(
        error.to_string().contains("Mesa preflight output changed"),
        "unexpected failure after explicit fake-CMake mutation: {error}"
    );
    assert!(fixture.cmake_log.is_file(), "fake CMake did not run");
    let preflight_root = fixture.request.reports_root.join("mesa-preflight");
    let receipt_bytes = fs::read(preflight_root.join("receipt.json")).unwrap();
    assert!(
        String::from_utf8_lossy(&receipt_bytes).contains("synthetic preflight receipt mutation")
    );
    assert!(
        !fixture
            .request
            .reports_root
            .join("upstream-configure.report.json")
            .exists(),
        "upstream configure ran after owner revalidation failed"
    );
    assert!(
        !fixture
            .request
            .reports_root
            .join("native-compatibility.receipt.json")
            .exists(),
        "published an aggregate receipt after owner revalidation failed"
    );
}

#[test]
fn missing_mesa_configure_fails_during_preflight_before_cmake() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = configured_mesa_fixture(temporary.path(), true, false);
    assert!(!fixture
        .request
        .preparation
        .source_root
        .join("configure")
        .exists());
    assert!(fixture
        .request
        .preparation
        .source_root
        .join("config/features")
        .is_file());

    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(
        error.to_string().contains("Mesa preflight child failed"),
        "missing configure did not fail at the source preflight boundary: {error}"
    );
    let preflight_root = fixture.request.reports_root.join("mesa-preflight");
    assert!(
        fs::read_to_string(preflight_root.join("configure.stderr"))
            .unwrap()
            .contains("No such file or directory"),
        "missing configure failure was not captured in the preflight stage log"
    );
    assert!(
        !fixture.cmake_log.exists(),
        "CMake ran without configure input"
    );
    assert!(!fixture
        .request
        .reports_root
        .join("native-compatibility.receipt.json")
        .exists());
}
