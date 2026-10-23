//! Optional process-level byte-consistency probe for complete SDK matrices.
//!
//! The indexed release and compatibility lanes are synthetic local fixtures.
//! This probe makes no owning-job, origin, native-host-CI, authentication or
//! release-readiness claim. SDK-generation input observations come from the
//! fixture executions; ordinary compiler-lane input documents are explicitly
//! synthetic claims assembled from the independently selected fixture request.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::{sha256_bytes, Sha256Digest};
use serde_json::{json, Map, Value};

use crate::compatibility::NativeCompatibilityReceiptExpectations;
use crate::release_index_v2::INDEX_NAME;
use crate::sdk_generation_lanes::ExpectedSdkGenerationLane;
use crate::sdk_generation_matrix::SdkGenerationMatrixReadbackRequest;

/// Actual input-observation bytes and digest retained by one fixture execution.
pub(super) type GenerationCompatibilityInput<'a> = (&'a [u8], &'a Sha256Digest);

/// Run the production CLI only when an explicit absolute binary path is set.
/// All paths selected by this probe point at existing fixture bytes or small
/// metadata files beneath one new temporary directory. It never copies package
/// trees, generation trees, evidence directories or build outputs.
pub(super) fn probe_if_configured(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    generation_inputs: &BTreeMap<ExpectedSdkGenerationLane, GenerationCompatibilityInput<'_>>,
) {
    let Some(binary) = std::env::var_os("AROS_TEST_SDK_MATRIX_CLI") else {
        return;
    };
    let binary = PathBuf::from(binary);
    assert!(
        binary.is_absolute(),
        "AROS_TEST_SDK_MATRIX_CLI must name an absolute executable path"
    );
    let binary = fs::canonicalize(&binary)
        .expect("AROS_TEST_SDK_MATRIX_CLI should name an existing CLI binary");
    assert_eq!(
        generation_inputs.len(),
        request.generations.len(),
        "one actual compatibility input observation is required for each SDK fixture row"
    );
    assert!(request.generations.keys().eq(generation_inputs.keys()));

    let metadata = tempfile::tempdir().expect("CLI probe metadata directory should be created");
    let metadata_root = fs::canonicalize(metadata.path()).unwrap();
    let mut selection = selection_document(request, generation_inputs, &metadata_root);
    let (selection_path, selection_sha256) =
        write_selection(&metadata_root, "selection.json", &selection);
    let arguments = cli_arguments(request, &selection_path, &selection_sha256);

    let verified = invoke_cli(&binary, &arguments);
    assert_success(
        &verified,
        "valid complete SDK-generation selection should verify",
    );
    let result: Value =
        serde_json::from_slice(&verified.stdout).expect("JSON CLI output should be valid JSON");
    assert_eq!(result["operation"], "verify-release-evidence");
    assert_eq!(result["assurance"], "byte-consistency-only");
    assert_eq!(
        result["sdk_generation_count"],
        request.generations.len(),
        "CLI should report the exact complete fixture matrix count"
    );
    probe_source_policy_selection(&binary, request, generation_inputs, &metadata_root);

    super::sdk_qualification_cli_tests::probe(
        &binary,
        request,
        &metadata_root,
        &selection,
        &arguments,
    );

    let generation_rows = selection["sdk_generations"]
        .as_array_mut()
        .expect("selection should contain generation rows");
    generation_rows.pop();
    let (missing_path, missing_sha256) = write_selection(
        &metadata_root,
        "selection-missing-generation.json",
        &selection,
    );
    let missing = invoke_cli(
        &binary,
        &cli_arguments(request, &missing_path, &missing_sha256),
    );
    assert_failure_contains(
        &missing,
        "SDK generation selection must exactly cover the independently selected release lanes",
        "CLI should reject an omitted SDK host tuple after rehashing selection metadata",
    );

    let mut wrong_sdk_size = selection_document(request, generation_inputs, &metadata_root);
    let sdk_id = request
        .complete
        .builds
        .packages
        .sdk_packages
        .keys()
        .next()
        .expect("matrix fixture should select an SDK product");
    let archive_size = wrong_sdk_size["sdk_packages"][sdk_id]["archive_size"]
        .as_u64()
        .expect("selected SDK archive size should be an unsigned integer");
    wrong_sdk_size["sdk_packages"][sdk_id]["archive_size"] = json!(archive_size
        .checked_add(1)
        .expect("fixture SDK archive size should admit an increment"));
    let (wrong_size_path, wrong_size_sha256) = write_selection(
        &metadata_root,
        "selection-wrong-sdk-size.json",
        &wrong_sdk_size,
    );
    let wrong_size = invoke_cli(
        &binary,
        &cli_arguments(request, &wrong_size_path, &wrong_size_sha256),
    );
    assert_failure_contains(
        &wrong_size,
        "selected SDK package failed shared release measurement",
        "CLI should bind selected SDK byte expectations to the indexed package",
    );
    assert!(String::from_utf8_lossy(&wrong_size.stderr).contains("AX0701"));

    let mut wrong_policy = selection_document(request, generation_inputs, &metadata_root);
    let first_policy =
        &mut wrong_policy["sdk_generations"][0]["compatibility"]["native_sdk_required"];
    *first_policy = json!(!first_policy.as_bool().unwrap());
    let (wrong_policy_path, wrong_policy_sha256) = write_selection(
        &metadata_root,
        "selection-wrong-sdk-policy.json",
        &wrong_policy,
    );
    let wrong_policy_result = invoke_cli(
        &binary,
        &cli_arguments(request, &wrong_policy_path, &wrong_policy_sha256),
    );
    assert_failure_contains(
        &wrong_policy_result,
        "SDK generation native policy differs from its independently selected compiler family",
        "SDK family policy cannot be substituted through selection metadata",
    );
}

/// Synthetic independent policy declarations exercise collector byte joins.
/// This does not claim inspection of a real source or authenticate its origin.
fn probe_source_policy_selection(
    binary: &Path,
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    generation_inputs: &BTreeMap<ExpectedSdkGenerationLane, GenerationCompatibilityInput<'_>>,
    metadata_root: &Path,
) {
    let inputs = &request.complete.builds.packages.inputs;
    let mut selection = selection_document(request, generation_inputs, metadata_root);
    selection["schema"] = json!("aros-toolchain-release-evidence-selection-v3");
    let mut llvm_row = None;
    for (index, row) in selection["sdk_generations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        let group = inputs
            .groups()
            .iter()
            .find(|group| group.id() == row["group_id"].as_str().unwrap())
            .unwrap();
        if group.source_lock().family() != crate::source_lock::CompilerFamily::Llvm {
            continue;
        }
        let sdk = inputs
            .sdks()
            .iter()
            .find(|sdk| sdk.id() == row["sdk_id"].as_str().unwrap())
            .unwrap();
        // Existing matrix fixtures explicitly select historical Mesa20, not
        // configured Mesa26. This constant is fixture policy, not a marker read.
        let policy = json!({
            "source_commit":sdk.identity().source.commit,
            "source_tree":sdk.identity().source.git_tree,
            "source_preset":sdk.source_preset(),
            "compiler_profile":row["target_profile"],
            "profiles_sha256":sha256_bytes(b"synthetic independently selected source profiles"),
            "native_consumer":{"file":"synthetic-sdk-consumer.json","sha256":sdk.identity().source.contract_sha256,"schema":"aros-sdk-consumer-contract-v1"},
            "native_sdk_required":true,"mesa_preflight_required":false
        });
        let bytes = crate::canonical::bytes(&policy).unwrap();
        let path = write_bytes(
            metadata_root,
            &format!("source-policy-{index}.json"),
            &bytes,
        );
        row["compatibility"]["source_policy"] = json!({"path":path,"sha256":sha256_bytes(&bytes)});
        llvm_row = Some(index);
    }
    let Some(row_index) = llvm_row else {
        return;
    };
    let (path, digest) = write_selection(metadata_root, "selection-source-policy.json", &selection);
    assert_success(
        &invoke_cli(binary, &cli_arguments(request, &path, &digest)),
        "v3 LLVM SDK policy selection should verify",
    );
    probe_generation_policy_cli(
        binary,
        request,
        &selection["sdk_generations"][row_index],
        metadata_root,
    );
    let mut missing = selection.clone();
    missing["sdk_generations"][row_index]["compatibility"]
        .as_object_mut()
        .unwrap()
        .remove("source_policy");
    let (path, digest) = write_selection(
        metadata_root,
        "selection-missing-source-policy.json",
        &missing,
    );
    assert_failure_contains(
        &invoke_cli(binary, &cli_arguments(request, &path, &digest)),
        "requires a separately selected source policy",
        "v3 cannot infer absent source policy from input markers",
    );
    let selected_policy =
        &mut selection["sdk_generations"][row_index]["compatibility"]["source_policy"];
    let policy_path = PathBuf::from(selected_policy["path"].as_str().unwrap());
    let mut policy: Value = serde_json::from_slice(&fs::read(&policy_path).unwrap()).unwrap();
    policy["mesa_preflight_required"] = json!(true);
    let bytes = crate::canonical::bytes(&policy).unwrap();
    fs::write(&policy_path, &bytes).unwrap();
    selected_policy["sha256"] = json!(sha256_bytes(&bytes));
    let (path, digest) = write_selection(metadata_root, "selection-required-mesa.json", &selection);
    assert_failure_contains(
        &invoke_cli(binary, &cli_arguments(request, &path, &digest)),
        "configured Mesa requirement differs",
        "independent Mesa policy must reject historical input observations",
    );
}

fn probe_generation_policy_cli(
    binary: &Path,
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    row: &Value,
    root: &Path,
) {
    let packages = &request.complete.builds.packages;
    let (lane, generation) = request
        .generations
        .iter()
        .find(|(lane, _)| {
            lane.sdk_id() == row["sdk_id"].as_str().unwrap()
                && lane.group_id() == row["group_id"].as_str().unwrap()
                && lane.profile() == row["target_profile"].as_str().unwrap()
                && lane.host() == row["host"].as_str().unwrap()
        })
        .unwrap();
    let expected_lane = (lane.group_id(), lane.profile(), lane.host());
    let artifact = packages
        .index
        .artifacts()
        .iter()
        .find(|artifact| {
            (
                artifact.group_id(),
                artifact.target_profile(),
                artifact.host(),
            ) == expected_lane
        })
        .unwrap();
    let compiler = &generation.compatibility.expected.package;
    let environment = crate::canonical::bytes(&Value::Object(
        packages.build_environments[artifact.asset()].clone(),
    ))
    .unwrap();
    let environment_path =
        write_bytes(root, "standalone-generation-environment.json", &environment);
    let mut compatibility = row["compatibility"].clone();
    compatibility
        .as_object_mut()
        .unwrap()
        .remove("native_sdk_required");
    let mut selected = json!({
        "schema":"aros-sdk-generation-readback-selection-v2",
        "compiler":{
            "directory":request.complete.builds.lanes[artifact.asset()].builds[0].package_dir,
            "archive_sha256":compiler.archive_sha256,"archive_size":compiler.archive_size,
            "environment":{"path":environment_path,"sha256":sha256_bytes(&environment)}
        },
        "sdk":{
            "directory":generation.package.package_dir,
            "tree_sha256":generation.package.expected_tree_sha256,
            "archive_sha256":generation.package.expected_archive_sha256,
            "archive_size":generation.package.expected_archive_size
        },
        "generation":row["record"],"compatibility":compatibility
    });
    let mut arguments = vec![
        "sdk".into(),
        "verify-generation".into(),
        "--inputs-dir".into(),
        packages.directory.to_string_lossy().into_owned(),
        "--inputs-sha256".into(),
        packages.inputs.collection_sha256().to_string(),
        "--sdk-id".into(),
        lane.sdk_id().to_owned(),
        "--compiler-group".into(),
        lane.group_id().to_owned(),
        "--compiler-profile".into(),
        lane.profile().to_owned(),
        "--host".into(),
        lane.host().to_owned(),
        "--release-id".into(),
        packages.index.release_id().to_owned(),
    ];
    for prefix in &packages.forbidden_prefixes {
        arguments.extend([
            "--forbidden-prefix".into(),
            prefix.to_string_lossy().into_owned(),
        ]);
    }
    if packages.forbidden_prefixes.is_empty() {
        // The standalone command requires an explicit forbidden root. Use the
        // actual compiler extraction in this synthetic fixture, not a dummy
        // path or a relaxation of its production argument contract.
        arguments.extend([
            "--forbidden-prefix".into(),
            request.complete.builds.lanes[artifact.asset()].builds[0]
                .package_dir
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    let (path, digest) = write_selection(root, "standalone-generation-policy.json", &selected);
    let mut valid = arguments.clone();
    valid.extend([
        "--selection".into(),
        path.to_string_lossy().into_owned(),
        "--selection-sha256".into(),
        digest.to_string(),
    ]);
    assert_success(
        &invoke_cli(binary, &valid),
        "standalone v2 generation must read the independent source policy",
    );
    selected["compatibility"]
        .as_object_mut()
        .unwrap()
        .remove("source_policy");
    let (path, digest) =
        write_selection(root, "standalone-generation-missing-policy.json", &selected);
    arguments.extend([
        "--selection".into(),
        path.to_string_lossy().into_owned(),
        "--selection-sha256".into(),
        digest.to_string(),
    ]);
    assert_failure_contains(
        &invoke_cli(binary, &arguments),
        "requires a separately selected source policy",
        "standalone v2 must not infer missing policy from results",
    );
}

fn selection_document(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    generation_inputs: &BTreeMap<ExpectedSdkGenerationLane, GenerationCompatibilityInput<'_>>,
    metadata_root: &Path,
) -> Value {
    let packages = &request.complete.builds.packages;
    let mut lanes = Map::new();
    for (asset, build_lane) in &request.complete.builds.lanes {
        let artifact = packages
            .index
            .artifacts()
            .iter()
            .find(|artifact| artifact.asset() == asset)
            .expect("complete fixture build lane should match one indexed artifact");
        let compatibility = request
            .complete
            .lanes
            .get(asset)
            .expect("complete fixture should select each compatibility lane");
        let inputs_path = write_bytes(
            metadata_root,
            &format!("compiler-compatibility-{asset}.inputs.json"),
            &synthetic_input_document(&compatibility.expected),
        );
        let builds = build_lane
            .builds
            .iter()
            .map(|side| {
                let contract_sha256 = side
                    .identity
                    .executor
                    .contract_sha256
                    .as_ref()
                    .expect("fixture executor should retain its native contract digest");
                json!({
                    "package_dir": side.package_dir,
                    "measurement": {
                        "path": side.measurement,
                        "sha256": side.measurement_sha256,
                    },
                    "executor": {
                        "contract_sha256": contract_sha256,
                        "binary_sha256": side.identity.executor.binary_sha256,
                    },
                })
            })
            .collect::<Vec<_>>();
        let environment = packages
            .build_environments
            .get(asset)
            .expect("fixture build environment should cover every indexed lane");
        lanes.insert(
            asset.clone(),
            json!({
                "build_environment": environment,
                "required_paths": artifact.required_paths(),
                "builds": builds,
                "comparison": {
                    "path": build_lane.comparison,
                    "sha256": build_lane.comparison_sha256,
                },
                "compatibility": {
                    "directory": compatibility.directory,
                    "inputs": {
                        "path": inputs_path,
                        "sha256": sha256_bytes(&fs::read(&inputs_path).unwrap()),
                    },
                    "manifest_sha256": compatibility.manifest_sha256,
                    "native_sdk_required": compatibility.expected.native_sdk.is_some(),
                },
            }),
        );
    }

    let sdk_packages = packages
        .sdk_packages
        .iter()
        .map(|(sdk_id, selected)| {
            (
                sdk_id.clone(),
                json!({
                    "tree_sha256": selected.expected_tree_sha256,
                    "archive_sha256": selected.expected_archive_sha256,
                    "archive_size": selected.expected_archive_size,
                }),
            )
        })
        .collect::<Map<_, _>>();

    let sdk_generations = request
        .generations
        .iter()
        .map(|(lane, selected)| {
            let record_path = write_bytes(
                metadata_root,
                &format!(
                    "sdk-generation-{}-{}.record.json",
                    lane.group_id(),
                    lane.host()
                ),
                selected.record_bytes,
            );
            assert_eq!(
                &sha256_bytes(selected.record_bytes),
                selected.record_sha256,
                "fixture record bytes should match their independent raw digest"
            );
            let (input_bytes, input_sha256) = generation_inputs
                .get(lane)
                .expect("fixture execution should retain this lane's input observation");
            assert_eq!(&sha256_bytes(input_bytes), *input_sha256);
            let inputs_path = write_bytes(
                metadata_root,
                &format!(
                    "sdk-generation-{}-{}.inputs.json",
                    lane.group_id(),
                    lane.host()
                ),
                input_bytes,
            );
            let compiler_family = request
                .complete
                .builds
                .packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == lane.group_id())
                .expect("generation lane should have an input-derived compiler group")
                .source_lock()
                .family();
            let native_sdk_required = compiler_family == crate::source_lock::CompilerFamily::Gnu;
            json!({
                "sdk_id": lane.sdk_id(),
                "group_id": lane.group_id(),
                "target_profile": lane.profile(),
                "host": lane.host(),
                "package_dir": selected.package.package_dir,
                "record": {
                    "path": record_path,
                    "sha256": selected.record_sha256,
                },
                "compatibility": {
                    "directory": selected.compatibility.directory,
                    "inputs": {
                        "path": inputs_path,
                        "sha256": input_sha256,
                    },
                    "manifest_sha256": selected.compatibility.manifest_sha256,
                    "native_sdk_required": native_sdk_required,
                },
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schema": "aros-toolchain-release-evidence-selection-v2",
        "lanes": lanes,
        "sdk_packages": sdk_packages,
        "sdk_generations": sdk_generations,
    })
}

/// Build only ordinary compiler-lane fixture inputs, from independently
/// selected test expectations. These local claims do not describe execution.
fn synthetic_input_document(expected: &NativeCompatibilityReceiptExpectations<'_>) -> Vec<u8> {
    let manifest =
        crate::canonical::bytes(&serde_json::to_value(expected.package.manifest).unwrap()).unwrap();
    let c_fixture = b"synthetic compiler-lane C fixture";
    let cxx_fixture = b"synthetic compiler-lane C++ fixture";
    let mut document = json!({
        "schema": if expected.mesa_preflight.is_some() {
            "aros-toolchain-compatibility-inputs-v4"
        } else if expected.llvm_sdk.is_some() {
            "aros-toolchain-compatibility-inputs-v3"
        } else if expected.native_sdk.is_some() {
            "aros-toolchain-compatibility-inputs-v2"
        } else {
            "aros-toolchain-compatibility-inputs-v1"
        },
        "package_manifest_sha256": sha256_bytes(&manifest),
        "archive_sha256": expected.package.archive_sha256,
        "archive_size": expected.package.archive_size,
        "compiler": expected.package.compiler,
        "package_source_commit": expected.package.source_commit,
        "host": expected.package.host,
        "profiles_sha256": expected.profile.document_sha256(),
        "target_profile": expected.profile.name(),
        "target_triple": expected.profile.target_triple(),
        "source_preset": expected.gnu_source_preset,
        "cmake_build_required": expected.cmake_build_required,
        "sdk_source_tree_sha256": expected.sdk_consumer_source_tree_sha256,
        "engine_api_version": expected.engine_api_version,
        "engine_sha256": expected.engine_sha256,
        "helpers": expected.helpers,
        "host_tools": expected.host_tools,
        "standalone_c_fixture": {
            "sha256": sha256_bytes(c_fixture),
            "size": c_fixture.len() as u64,
        },
        "standalone_cxx_fixture": {
            "sha256": sha256_bytes(cxx_fixture),
            "size": cxx_fixture.len() as u64,
        },
        "sdk_environment_sha256": expected.sdk_environment_sha256,
        "standalone_environment_sha256": expected.standalone_environment_sha256,
        "upstream_source_commit": expected.upstream_source_commit,
        "upstream_source_tree": expected.upstream_source_tree,
        "ports_sources": expected.ports_sources,
    });
    let object = document
        .as_object_mut()
        .expect("fixture input document should be a JSON object");
    if let Some(selection) = expected.native_sdk {
        object.insert(
            "native_sdk".into(),
            serde_json::to_value(selection).unwrap(),
        );
    }
    if let Some(selection) = expected.llvm_sdk {
        object.insert("llvm_sdk".into(), serde_json::to_value(selection).unwrap());
    }
    if let Some(selection) = expected.mesa_preflight {
        object.insert(
            "mesa_preflight".into(),
            serde_json::to_value(selection).unwrap(),
        );
    }
    crate::canonical::bytes(&document).unwrap()
}

fn cli_arguments(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    selection_path: &Path,
    selection_sha256: &Sha256Digest,
) -> Vec<String> {
    let packages = &request.complete.builds.packages;
    let index_path = packages.directory.join(INDEX_NAME);
    let index_bytes = fs::read(&index_path).expect("fixture release index should exist");
    assert_eq!(
        packages.index.to_json_bytes().unwrap(),
        index_bytes,
        "fixture index bytes should match the selected validated index"
    );
    let index_sha256 = sha256_bytes(&index_bytes);
    let mut arguments = vec![
        "toolchain".to_owned(),
        "producer".to_owned(),
        "verify-release-evidence".to_owned(),
        "--directory".to_owned(),
        packages.directory.to_string_lossy().into_owned(),
        "--release-id".to_owned(),
        packages.index.release_id().to_owned(),
        "--base-url".to_owned(),
        packages.index.base_url().to_owned(),
        "--inputs-sha256".to_owned(),
        packages.inputs.collection_sha256().to_string(),
        "--index-sha256".to_owned(),
        index_sha256.to_string(),
        "--selection".to_owned(),
        selection_path.to_string_lossy().into_owned(),
        "--selection-sha256".to_owned(),
        selection_sha256.to_string(),
        "--subject-manifest".to_owned(),
        request
            .complete
            .builds
            .subject_manifest
            .to_string_lossy()
            .into_owned(),
        "--subject-manifest-sha256".to_owned(),
        request.complete.builds.subject_manifest_sha256.to_string(),
        "--format".to_owned(),
        "json".to_owned(),
    ];
    for forbidden_prefix in &packages.forbidden_prefixes {
        arguments.push("--forbidden-prefix".into());
        arguments.push(forbidden_prefix.to_string_lossy().into_owned());
    }
    arguments
}

fn invoke_cli(binary: &Path, arguments: &[String]) -> Output {
    Command::new(binary)
        .args(arguments)
        .output()
        .expect("configured AROS_TEST_SDK_MATRIX_CLI should execute")
}

fn write_selection(root: &Path, name: &str, value: &Value) -> (PathBuf, Sha256Digest) {
    let bytes = crate::canonical::bytes(value).unwrap();
    let path = write_bytes(root, name, &bytes);
    (path, sha256_bytes(&bytes))
}

fn write_bytes(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, bytes).expect("CLI probe metadata should be written");
    fs::canonicalize(path).expect("CLI probe metadata path should be canonical")
}

fn assert_success(output: &Output, message: &str) {
    assert!(
        output.status.success(),
        "{message}: status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure_contains(output: &Output, expected: &str, message: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{message}: command unexpectedly succeeded"
    );
    assert!(
        stdout.contains(expected) || stderr.contains(expected),
        "{message}: expected {expected:?} in stdout/stderr\nstdout={stdout}\nstderr={stderr}"
    );
}
