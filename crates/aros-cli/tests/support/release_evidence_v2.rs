//! Synthetic process fixture for the full family-v2 evidence collector.
//!
//! These bytes are deliberately constructed declarations. No compiler,
//! producer command, compatibility probe, authentication or qualification is
//! performed by this fixture.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use aros_common::{
    measure_tree_content_cas, sha256_bytes, ArosCompilerIdentity, Sha256Digest,
    AROS_TOOLCHAIN_MANIFEST_FILE,
};
use aros_toolchain::compatibility::{
    native_compatibility_host_tools, verify_standalone_outputs_with_compilers, CompatibilityPhase,
    StandaloneOutputRequest, StandaloneTargetArtifacts,
};
use aros_toolchain::native_candidate::{
    export_finished_package_measurement, package_finished_candidate,
    readback_finished_build_result, FinishedBuildResultRequest,
};
use aros_toolchain::package::{PackageOutput, PackageRequest};
use aros_toolchain::plan::{Executor, Identity};
use aros_toolchain::profiles::{Profile, Profiles};
use aros_toolchain::recipe::Recipe;
use aros_toolchain::release_attestation_manifest_v2::write_attestation_manifest_v2;
use aros_toolchain::release_checksums_v2_writer::write_final_checksums_v2;
use aros_toolchain::release_index::{compare_package_sets, write_package_comparison_report};
use aros_toolchain::release_index_v2::PROVENANCE_NAME;
use aros_toolchain::release_index_v2_builder::MeasuredReleaseIndexRequestV2;
use aros_toolchain::release_index_v2_readback::{
    readback_indexed_packages, IndexedPackageReadbackRequestV2,
};
use aros_toolchain::release_index_v2_writer::write_measured_index_v2;
use aros_toolchain::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

pub const RELEASE_ID: &str = "release-evidence-v2-synthetic";
pub const BASE_URL: &str = "https://example.invalid/releases/release-evidence-v2-synthetic";

#[derive(Clone)]
pub struct SideSelection {
    pub package_dir: PathBuf,
    pub measurement: PathBuf,
    pub measurement_sha256: Sha256Digest,
    pub contract_sha256: Sha256Digest,
    pub binary_sha256: Sha256Digest,
}

pub struct LaneSelection {
    pub host: String,
    pub build_environment: Map<String, Value>,
    pub required_paths: Vec<String>,
    pub builds: [SideSelection; 2],
    pub comparison: PathBuf,
    pub comparison_sha256: Sha256Digest,
    pub compatibility_directory: PathBuf,
    pub compatibility_inputs: PathBuf,
    pub compatibility_inputs_sha256: Sha256Digest,
    pub compatibility_manifest_sha256: Sha256Digest,
}

pub struct Fixture {
    _release_root: TempDir,
    _transport_root: TempDir,
    pub release_directory: PathBuf,
    pub transport_directory: PathBuf,
    pub inputs_sha256: Sha256Digest,
    pub index_sha256: Sha256Digest,
    pub subject_manifest: PathBuf,
    pub subject_manifest_sha256: Sha256Digest,
    pub lanes: BTreeMap<String, LaneSelection>,
    pub selection: Value,
}

struct InputFixture {
    inputs: ReleaseInputs,
    collection: Vec<u8>,
    documents: BTreeMap<String, Vec<u8>>,
    profiles: Profiles,
    profile: Profile,
}

struct PackageSide {
    output: PackageOutput,
    measurement: PathBuf,
    measurement_sha256: Sha256Digest,
    contract_sha256: Sha256Digest,
    binary_sha256: Sha256Digest,
    _package_root: TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let input = input_fixture();
        let release_root = tempfile::tempdir().unwrap();
        let release_directory = fs::canonicalize(release_root.path()).unwrap();
        write_release_inputs(&release_directory, &input);

        let transport_root = tempfile::tempdir().unwrap();
        let transport_directory = fs::canonicalize(transport_root.path()).unwrap();
        let package_downloads = transport_directory.join("packages");
        let measurements = transport_directory.join("measurements");
        let comparisons = transport_directory.join("comparisons");
        let compatibility_roots = transport_directory.join("compatibility");
        let compatibility_inputs = transport_directory.join("compatibility-inputs");
        for path in [
            &package_downloads,
            &measurements,
            &comparisons,
            &compatibility_roots,
            &compatibility_inputs,
        ] {
            fs::create_dir(path).unwrap();
        }

        let mut build_environments = BTreeMap::new();
        let mut required_paths = BTreeMap::new();
        let mut sides_by_asset = BTreeMap::new();
        let mut lane_environment = BTreeMap::new();

        for (host_index, host) in ACTIVE_HOSTS.iter().copied().enumerate() {
            let environment = Map::from_iter([(
                "fixture-observation".to_owned(),
                json!(format!("independently-selected-{host}")),
            )]);
            let left = package_side(
                host,
                environment.clone(),
                host_index,
                0,
                &input,
                &measurements,
            );
            let right = package_side(
                host,
                environment.clone(),
                host_index,
                1,
                &input,
                &measurements,
            );
            let asset = left
                .output
                .archive
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                right.output.archive.file_name().unwrap(),
                asset.as_str(),
                "the two synthetic sides must select the same indexed asset"
            );

            let left_download = package_downloads.join(format!("{host_index}-a"));
            let right_download = package_downloads.join(format!("{host_index}-b"));
            copy_package(left.output.output_dir.as_path(), &left_download);
            copy_package(right.output.output_dir.as_path(), &right_download);
            let left_download = fs::canonicalize(left_download).unwrap();
            let right_download = fs::canonicalize(right_download).unwrap();
            copy_flat_package(left.output.output_dir.as_path(), &release_directory);

            let comparison_path = comparisons.join(format!("{asset}.json"));
            let compared =
                compare_package_sets(&left.output.output_dir, &right.output.output_dir).unwrap();
            let comparison = write_package_comparison_report(&comparison_path, &compared).unwrap();
            let comparison_path = fs::canonicalize(comparison.path).unwrap();
            let left_measurement = left.measurement.clone();
            let right_measurement = right.measurement.clone();
            let left_measurement_sha256 = left.measurement_sha256.clone();
            let right_measurement_sha256 = right.measurement_sha256.clone();
            let left_contract = left.contract_sha256.clone();
            let right_contract = right.contract_sha256.clone();
            let left_binary = left.binary_sha256.clone();
            let right_binary = right.binary_sha256.clone();
            let asset_sides = [
                SideSelection {
                    package_dir: left_download,
                    measurement: fs::canonicalize(left_measurement).unwrap(),
                    measurement_sha256: left_measurement_sha256,
                    contract_sha256: left_contract,
                    binary_sha256: left_binary,
                },
                SideSelection {
                    package_dir: right_download,
                    measurement: fs::canonicalize(right_measurement).unwrap(),
                    measurement_sha256: right_measurement_sha256,
                    contract_sha256: right_contract,
                    binary_sha256: right_binary,
                },
            ];

            build_environments.insert(asset.clone(), environment.clone());
            required_paths.insert(asset.clone(), vec!["bin/clang".to_owned()]);
            lane_environment.insert(asset.clone(), environment);
            sides_by_asset.insert(asset, (asset_sides, comparison_path, comparison.sha256));
        }

        let written_index = write_measured_index_v2(&MeasuredReleaseIndexRequestV2 {
            directory: release_directory.clone(),
            inputs: input.inputs.clone(),
            release_id: RELEASE_ID.into(),
            base_url: BASE_URL.into(),
            build_environments: build_environments.clone(),
            required_paths: required_paths.clone(),
            forbidden_prefixes: Vec::new(),
        })
        .unwrap();
        let index_sha256 = written_index.sha256().clone();
        let index = written_index.index().clone();
        let packages = IndexedPackageReadbackRequestV2 {
            directory: release_directory.clone(),
            inputs: input.inputs.clone(),
            index: index.clone(),
            build_environments,
            forbidden_prefixes: Vec::new(),
        };
        let subject_manifest = transport_directory.join("subjects.sha256");
        let subject = write_attestation_manifest_v2(&packages, &subject_manifest).unwrap();
        let subject_manifest = fs::canonicalize(subject_manifest).unwrap();
        fs::write(
            release_directory.join(PROVENANCE_NAME),
            b"synthetic unsigned provenance bytes; not authenticated\n",
        )
        .unwrap();
        write_final_checksums_v2(&packages).unwrap();
        let measured_packages = readback_indexed_packages(&packages).unwrap();

        let mut lanes = BTreeMap::new();
        for artifact in index.artifacts() {
            let (builds, comparison_path, comparison_sha256) =
                sides_by_asset.remove(artifact.asset()).unwrap();
            let compatibility_directory = compatibility_roots.join(artifact.asset());
            fs::create_dir(&compatibility_directory).unwrap();
            let (manifest_sha256, input_bytes) = write_compatibility_lane(
                &compatibility_directory,
                artifact.host(),
                &input,
                measured_packages
                    .packages()
                    .iter()
                    .find(|package| package.asset() == artifact.asset())
                    .unwrap()
                    .package(),
            );
            let compatibility_inputs =
                compatibility_inputs.join(format!("{}.json", artifact.asset()));
            fs::write(&compatibility_inputs, &input_bytes).unwrap();
            let compatibility_inputs = fs::canonicalize(compatibility_inputs).unwrap();
            let compatibility_directory = fs::canonicalize(compatibility_directory).unwrap();
            lanes.insert(
                artifact.asset().to_owned(),
                LaneSelection {
                    host: artifact.host().to_owned(),
                    build_environment: lane_environment.remove(artifact.asset()).unwrap(),
                    required_paths: artifact.required_paths().to_vec(),
                    builds,
                    comparison: comparison_path,
                    comparison_sha256,
                    compatibility_directory,
                    compatibility_inputs_sha256: sha256_bytes(&input_bytes),
                    compatibility_inputs,
                    compatibility_manifest_sha256: manifest_sha256,
                },
            );
        }
        let selection = selection_document(&lanes);
        Self {
            _release_root: release_root,
            _transport_root: transport_root,
            release_directory,
            transport_directory,
            inputs_sha256: input.inputs.collection_sha256().clone(),
            index_sha256,
            subject_manifest,
            subject_manifest_sha256: subject.sha256().clone(),
            lanes,
            selection,
        }
    }

    pub fn write_selection(&self, name: &str, selection: &Value) -> (PathBuf, Sha256Digest) {
        let path = self.transport_directory.join(format!("selection-{name}"));
        fs::write(&path, serde_json::to_vec(selection).unwrap()).unwrap();
        let path = fs::canonicalize(path).unwrap();
        let bytes = fs::read(&path).unwrap();
        (path, sha256_bytes(&bytes))
    }

    pub fn selection_path(&self) -> (PathBuf, Sha256Digest) {
        self.write_selection("selection.json", &self.selection)
    }
}

fn input_fixture() -> InputFixture {
    let source_lock = fixture_source_lock();
    let profiles_bytes = fixture_profiles();
    let recipe = fixture_recipe(&source_lock, &profiles_bytes);
    let mut documents = BTreeMap::new();
    documents.insert("recipe.json".to_owned(), recipe);
    documents.insert("source-lock.json".to_owned(), source_lock);
    documents.insert("profiles.json".to_owned(), profiles_bytes.clone());
    let collection = serde_json::to_vec(&json!({
        "schema":"aros-toolchain-release-inputs-v2",
        "producer_commit":PRODUCER_COMMIT,
        "tools_commit":TOOLS_COMMIT,
        "hosts":ACTIVE_HOSTS,
        "groups":[{
            "id":"llvm-pc",
            "recipe":{"file":"recipe.json","sha256":sha256_bytes(&documents["recipe.json"]).as_str()},
            "source_lock":{"file":"source-lock.json","sha256":sha256_bytes(&documents["source-lock.json"]).as_str()},
            "profiles":{"file":"profiles.json","sha256":sha256_bytes(&documents["profiles.json"]).as_str()}
        }]
    }))
    .unwrap();
    let inputs = ReleaseInputs::parse(&collection, &documents).unwrap();
    let profiles = Profiles::parse(&profiles_bytes).unwrap();
    let profile = profiles.select("pc-x86_64").unwrap().clone();
    InputFixture {
        inputs,
        collection,
        documents,
        profiles,
        profile,
    }
}

const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
const PRODUCER_COMMIT: &str = "3333333333333333333333333333333333333333";
const TOOLS_COMMIT: &str = "5555555555555555555555555555555555555555";

fn fixture_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-source-lock-v2",
        "family":"llvm",
        "version":"17.0.6",
        "sources":[{"component":"llvm","version":"17.0.6","purpose":"toolchain-component","filename":"llvm-17.0.6.tar.xz","url":"https://example.invalid/llvm-17.0.6.tar.xz","sha256":"a".repeat(64),"size":1}],
        "host_python_packages":[{"name":"wheel","version":"0.43.0","filename":"wheel-0.43.0.tar.gz","url":"https://example.invalid/wheel-0.43.0.tar.gz","sha256":"b".repeat(64),"size":1,"source_root":"wheel-0.43.0","python_path":"."}]
    }))
    .unwrap()
}

fn fixture_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":"aros-toolchain-profiles-v1",
        "upstream_commit":SOURCE_COMMIT,
        "profiles":[{"name":"pc-x86_64","configure_target":"pc-x86_64","upstream_output_target":"pc-x86_64","target_triple":"x86_64-unknown-aros","cpu":"x86_64","platform":"pc","float_abi":"","capabilities":["c","cxx","standalone-collector"]}]
    }))
    .unwrap()
}

fn fixture_recipe(source_lock: &[u8], profiles: &[u8]) -> Vec<u8> {
    let mut value = json!({
        "schema":"aros-toolchain-recipe-v2",
        "source_commit":SOURCE_COMMIT,
        "source_tree":"2222222222222222222222222222222222222222",
        "producer_commit":PRODUCER_COMMIT,
        "producer_tree":"4444444444444444444444444444444444444444",
        "tools_commit":TOOLS_COMMIT,
        "tools_tree":"6666666666666666666666666666666666666666",
        "source_date_epoch":946_684_800_u64,
        "source_lock_sha256":sha256_bytes(source_lock).as_str(),
        "profiles_sha256":sha256_bytes(profiles).as_str(),
        "patches":[]
    });
    let self_digest = sha256_bytes(&aros_toolchain::canonical::bytes(&value).unwrap());
    value["recipe_sha256"] = json!(self_digest.as_str());
    serde_json::to_vec(&value).unwrap()
}

fn write_release_inputs(directory: &Path, input: &InputFixture) {
    for (name, bytes) in &input.documents {
        fs::write(directory.join(name), bytes).unwrap();
    }
    fs::write(
        directory.join("toolchain-release-inputs-v2.json"),
        &input.collection,
    )
    .unwrap();
    fs::write(
        directory.join("toolchain-manifest-v2.schema.json"),
        include_bytes!("../../../aros-common/tests/fixtures/toolchain-manifest-v2.schema.json"),
    )
    .unwrap();
    fs::write(
        directory.join("tree-digest-v1.fixture.json"),
        include_bytes!("../../../aros-common/tests/fixtures/tree-digest-v1.fixture.json"),
    )
    .unwrap();
}

fn package_side(
    host: &'static str,
    build_environment: Map<String, Value>,
    host_index: usize,
    side_index: usize,
    input: &InputFixture,
    measurements: &Path,
) -> PackageSide {
    let work_root = tempfile::tempdir().unwrap();
    let output_root = tempfile::tempdir().unwrap();
    let work = create_directory(&work_root.path().join("work"));
    let output = create_directory(&output_root.path().join("output"));
    let lifecycle = work.join("native-lifecycle");
    let receipts = lifecycle.join("receipts");
    let stage = output.join(".aros-native-toolchain-stage");
    let candidate = output.join("toolchain");
    fs::create_dir_all(&receipts).unwrap();
    fs::create_dir_all(stage.join("toolchain/bin")).unwrap();
    fs::write(
        stage.join("toolchain/bin/llvm-config"),
        b"synthetic old checkpoint\n",
    )
    .unwrap();
    write_candidate(&candidate);

    let recipe = Recipe::parse(&input.documents["recipe.json"]).unwrap();
    let source_lock =
        aros_toolchain::source_lock::SourceLock::parse(&input.documents["source-lock.json"])
            .unwrap();
    let identity = Identity {
        recipe_sha256: recipe.sha256().clone(),
        source_commit: recipe.source().0.clone(),
        producer_commit: recipe.producer().0.clone(),
        tools_commit: recipe.tools().0.clone(),
        host,
        target_profile: input.profile.name().to_owned(),
        executor: Executor {
            contract_id: Some("aros-toolchain-producer-v1"),
            contract_sha256: Some(sha256_bytes(b"synthetic producer contract")),
            tools_commit: Some(recipe.tools().0.clone()),
            binary_sha256: sha256_bytes(b"synthetic CLI binary observation"),
            origin_evidence_sha256: None,
        },
    };
    let phase_digests = write_phase_chain(&receipts, &work, &output, &identity, &candidate);
    let payload = measure_tree_content_cas(&candidate).unwrap();
    let mut finished = json!({
        "schema":"aros-toolchain-finished-candidate-v2",
        "identity":identity,
        "compiler":{"family":"llvm","version":"17.0.6"},
        "target_triple":input.profile.target_triple(),
        "source_lock_sha256":source_lock.sha256(),
        "profiles_sha256":input.profile.document_sha256(),
        "candidate_root":candidate,
        "phase_receipt_digests":phase_digests,
        "payload_sha256":payload.payload_digest_excluding(None),
        "entry_count":u64::try_from(payload.entry_count()).unwrap(),
        "regular_file_bytes":payload.regular_file_bytes().unwrap(),
        "receipt_sha256":sha256_bytes(b"placeholder")
    });
    let finished_digest = replace_self_digest(&mut finished);
    fs::write(
        receipts.join("finished-candidate.json"),
        aros_toolchain::canonical::bytes(&finished).unwrap(),
    )
    .unwrap();
    let evidence = phase_digests
        .iter()
        .enumerate()
        .map(|(index, digest)| {
            json!({"check":PHASES[index],"status":"passed","report_sha256":digest})
        })
        .chain(std::iter::once(json!({
            "check":"finished-candidate","status":"passed","report_sha256":finished_digest
        })))
        .chain(std::iter::once(json!({
            "check":"origin","status":"not-run","report_sha256":null
        })))
        .collect::<Vec<_>>();
    let publish: Value =
        serde_json::from_slice(&fs::read(receipts.join("publish.json")).unwrap()).unwrap();
    let build_result = json!({
        "schema":"aros-toolchain-result-v1",
        "operation":"build",
        "identity":identity,
        "output_root":output,
        "outputs":publish["outputs"],
        "evidence":evidence,
        "qualification":"local-only",
        "commit_state":"committed"
    });
    let mut build_result_bytes = serde_json::to_vec(&build_result).unwrap();
    build_result_bytes.push(b'\n');
    let result_path = work_root.path().join("build-result.json");
    fs::write(&result_path, &build_result_bytes).unwrap();
    let result_sha256 = sha256_bytes(&build_result_bytes);
    let candidate_proof = readback_finished_build_result(&FinishedBuildResultRequest {
        work_dir: &work,
        output_dir: &output,
        recipe: &recipe,
        source_lock: &source_lock,
        profile: &input.profile,
        host,
        build_result: &result_path,
        build_result_sha256: &result_sha256,
    })
    .unwrap();

    let package_root = tempfile::tempdir().unwrap();
    let request = PackageRequest {
        candidate_root: candidate,
        output_dir: package_root.path().join("package"),
        release_id: RELEASE_ID.into(),
        host: host.into(),
        recipe,
        source_lock,
        profile: input.profile.clone(),
        build_environment,
        forbidden_prefixes: vec![work, output],
    };
    let package = package_finished_candidate(&request, &candidate_proof).unwrap();
    let measurement = export_finished_package_measurement(&candidate_proof, &package).unwrap();
    let measurement_path = measurements.join(format!("{host_index}-{side_index}.json"));
    fs::write(&measurement_path, measurement.bytes()).unwrap();
    PackageSide {
        output: package.package_output().clone(),
        measurement: fs::canonicalize(measurement_path).unwrap(),
        measurement_sha256: measurement.sha256().clone(),
        contract_sha256: identity.executor.contract_sha256.clone().unwrap(),
        binary_sha256: identity.executor.binary_sha256,
        _package_root: package_root,
    }
}

const PHASES: [&str; 6] = [
    "preflight",
    "environment",
    "configure",
    "compiler",
    "collector",
    "publish",
];

fn write_phase_chain(
    receipts: &Path,
    work: &Path,
    output: &Path,
    identity: &Identity,
    candidate: &Path,
) -> [Sha256Digest; 6] {
    let collector = fs::read(candidate.join("bin/aros-collect")).unwrap();
    let old_checkpoint = b"synthetic old checkpoint\n";
    let mut result = std::array::from_fn(|_| sha256_bytes(b"unset"));
    for (index, phase) in PHASES.iter().enumerate() {
        let output_root = if index < 3 {
            work.join("native-lifecycle")
        } else if index == 5 {
            candidate.to_path_buf()
        } else {
            output.join(".aros-native-toolchain-stage")
        };
        let outputs = if index == 3 {
            vec![
                json!({"path":"toolchain/bin/llvm-config","kind":"file","sha256":sha256_bytes(old_checkpoint),"size":old_checkpoint.len()}),
            ]
        } else if index == 5 {
            vec![
                json!({"path":"toolchain/bin/aros-collect","kind":"file","sha256":sha256_bytes(&collector),"size":collector.len()}),
            ]
        } else {
            Vec::new()
        };
        let value = json!({
            "schema":"aros-toolchain-receipt-v1",
            "identity":identity,
            "phase":phase,
            "input_sha256":sha256_bytes(phase.as_bytes()),
            "output_root":output_root,
            "outputs":outputs,
            "previous_receipt_sha256":index.checked_sub(1).map(|prior| result[prior].clone()),
            "receipt_sha256":sha256_bytes(b"placeholder")
        });
        let mut value = value;
        result[index] = replace_self_digest(&mut value);
        fs::write(
            receipts.join(format!("{phase}.json")),
            aros_toolchain::canonical::bytes(&value).unwrap(),
        )
        .unwrap();
    }
    result
}

fn write_candidate(candidate: &Path) {
    for relative in ["bin", "libexec", "target/empty", "include/empty"] {
        fs::create_dir_all(candidate.join(relative)).unwrap();
    }
    write_executable(
        candidate.join("bin/clang"),
        b"synthetic compiler payload\n",
        0o751,
    );
    write_executable(
        candidate.join("bin/aros-collect"),
        b"synthetic published collector\n",
        0o755,
    );
    fs::write(
        candidate.join("libexec/llvm-ar.real"),
        b"synthetic archiver\n",
    )
    .unwrap();
    fs::write(
        candidate.join(".installflag-fixture"),
        b"normalization marker\n",
    )
    .unwrap();
    fs::write(
        candidate.join(AROS_TOOLCHAIN_MANIFEST_FILE),
        b"synthetic prior embedded manifest\n",
    )
    .unwrap();
    symlink("clang", candidate.join("bin/clang++")).unwrap();
    symlink("../libexec/llvm-ar.real", candidate.join("bin/llvm-ar")).unwrap();
    symlink("aros-collect", candidate.join("bin/collect-aros")).unwrap();
    symlink("aros-collect", candidate.join("bin/collect-aros32")).unwrap();
}

fn write_executable(path: PathBuf, bytes: &[u8], mode: u32) {
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn create_directory(path: &Path) -> PathBuf {
    fs::create_dir(path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn replace_self_digest(value: &mut Value) -> Sha256Digest {
    value
        .as_object_mut()
        .unwrap()
        .remove("receipt_sha256")
        .unwrap();
    let digest = sha256_bytes(&aros_toolchain::canonical::bytes(value).unwrap());
    value["receipt_sha256"] = json!(digest.as_str());
    digest
}

fn copy_package(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    copy_flat_package(source, destination);
}

fn copy_flat_package(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        let output = destination.join(entry.file_name());
        assert!(
            !output.exists(),
            "duplicate package member {}",
            output.display()
        );
        fs::copy(entry.path(), output).unwrap();
    }
}

fn write_compatibility_lane(
    directory: &Path,
    host: &str,
    input: &InputFixture,
    package_manifest: &aros_toolchain::package_verify::VerifiedPackage,
) -> (Sha256Digest, Vec<u8>) {
    let group = input
        .inputs
        .groups()
        .iter()
        .find(|item| item.id() == "llvm-pc")
        .unwrap();
    let profile = group.profiles().select("pc-x86_64").unwrap();
    let compiler = package_manifest.manifest.compiler_identity().unwrap();
    let sdk_tree = sha256_bytes(b"synthetic engine-free compatibility source tree");
    let engine = sha256_bytes(b"synthetic embedded compatibility engine");
    let sdk_environment = sha256_bytes(b"synthetic selected sdk environment");
    let standalone_environment = sha256_bytes(b"synthetic selected standalone environment");
    let helpers = json!(aros_toolchain::compatibility::REQUIRED_HELPERS
        .iter()
        .map(|name| (
            (*name).to_owned(),
            json!({"sha256":sha256_bytes(format!("helper-{name}").as_bytes()),"size":16})
        ))
        .collect::<BTreeMap<_, _>>());
    let host_tools = json!(native_compatibility_host_tools(host)
        .unwrap()
        .into_iter()
        .map(|name| (
            name.to_owned(),
            json!({"sha256":sha256_bytes(format!("host-tool-{name}").as_bytes()),"size":20})
        ))
        .collect::<BTreeMap<_, _>>());
    let port = json!({
        "id":"bzip2",
        "cache_filename":"bzip2-1.0.8.tar.gz",
        "relative_path":"ports/bzip2-1.0.8.tar.gz",
        "fetch_marker":".bzip2-1.0.8-fetched",
        "sha256":sha256_bytes(b"synthetic locked port"),
        "size":128
    });

    let standalone_temp = tempfile::tempdir().unwrap();
    let standalone_root = fs::canonicalize(standalone_temp.path()).unwrap();
    let targets = BTreeMap::from([
        (
            "x86_64-unknown-aros".to_owned(),
            StandaloneTargetArtifacts {
                c: standalone_root.join("c-x86_64.o"),
                cxx: standalone_root.join("cxx-x86_64.o"),
            },
        ),
        (
            "i386-unknown-aros".to_owned(),
            StandaloneTargetArtifacts {
                c: standalone_root.join("c-i386.o"),
                cxx: standalone_root.join("cxx-i386.o"),
            },
        ),
    ]);
    let compiler_map = targets
        .keys()
        .map(|triple| (triple.clone(), compiler.clone()))
        .collect::<BTreeMap<_, _>>();
    fs::write(
        &targets["x86_64-unknown-aros"].c,
        fixture_elf64("__TOOLCHAIN_LIST__", 62),
    )
    .unwrap();
    fs::write(
        &targets["x86_64-unknown-aros"].cxx,
        fixture_elf64("__INIT_ARRAY_LIST__", 62),
    )
    .unwrap();
    fs::write(
        &targets["i386-unknown-aros"].c,
        fixture_elf32("__TOOLCHAIN_LIST__", 3),
    )
    .unwrap();
    fs::write(
        &targets["i386-unknown-aros"].cxx,
        fixture_elf32("__INIT_ARRAY_LIST__", 3),
    )
    .unwrap();
    let standalone = verify_standalone_outputs_with_compilers(
        &StandaloneOutputRequest {
            output_root: standalone_root,
            targets,
        },
        &compiler_map,
    )
    .unwrap();
    let standalone_targets = standalone.targets.iter().map(|(triple, report)| {
        (
            triple.clone(),
            json!({
                "c":{"sha256":report.c.sha256,"size":report.c.size,"class":class_name(report.c.class)},
                "cxx":{"sha256":report.cxx.sha256,"size":report.cxx.size,"class":class_name(report.cxx.class)}
            }),
        )
    }).collect::<BTreeMap<_, _>>();

    let phase_specs = [
        (
            CompatibilityPhase::CmakeConsumer,
            "cmake-consumer",
            1,
            false,
        ),
        (
            CompatibilityPhase::UpstreamConfigure,
            "upstream-configure",
            1,
            false,
        ),
        (
            CompatibilityPhase::UpstreamIncludes,
            "upstream-includes",
            1,
            false,
        ),
        (
            CompatibilityPhase::UpstreamLinklibs,
            "upstream-linklibs",
            1,
            false,
        ),
        (CompatibilityPhase::StandaloneC, "standalone-c", 2, true),
        (CompatibilityPhase::StandaloneCxx, "standalone-cxx", 2, true),
    ];
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    let mut phase_reports = Vec::new();
    for (phase, stem, count, standalone_phase) in phase_specs {
        let mut commands = Vec::new();
        for command_index in 0..count {
            let stdout = format!("synthetic {stem} stdout {command_index}\n").into_bytes();
            let stderr = format!("synthetic {stem} stderr {command_index}\n").into_bytes();
            commands.push(json!({
                "program_sha256":sha256_bytes(format!("program-{stem}-{command_index}").as_bytes()),
                "command_sha256":sha256_bytes(format!("command-{stem}-{command_index}").as_bytes()),
                "stdout_sha256":sha256_bytes(&stdout),
                "stderr_sha256":sha256_bytes(&stderr)
            }));
            let command_stem = if count == 1 {
                stem.to_owned()
            } else {
                format!("{stem}.{}", command_index + 1)
            };
            files.insert(format!("{command_stem}.stdout.log"), stdout);
            files.insert(format!("{command_stem}.stderr.log"), stderr);
        }
        let report = json!({
            "schema":"aros-toolchain-compatibility-report-v5",
            "phase":phase,
            "engine_api_version":3,
            "engine_sha256":engine,
            "source_tree_sha256":sdk_tree,
            "helpers":helpers,
            "host_tools":if standalone_phase { json!({}) } else { host_tools.clone() },
            "environment_sha256":if standalone_phase { &standalone_environment } else { &sdk_environment },
            "commands":commands
        });
        let bytes = aros_toolchain::canonical::bytes(&report).unwrap();
        phase_reports.push(json!({"phase":phase,"report_sha256":sha256_bytes(&bytes)}));
        files.insert(format!("{stem}.report.json"), bytes);
    }
    for (name, bytes) in [
        ("c-x86_64.o", fixture_elf64("__TOOLCHAIN_LIST__", 62)),
        ("cxx-x86_64.o", fixture_elf64("__INIT_ARRAY_LIST__", 62)),
        ("c-i386.o", fixture_elf32("__TOOLCHAIN_LIST__", 3)),
        ("cxx-i386.o", fixture_elf32("__INIT_ARRAY_LIST__", 3)),
    ] {
        files.insert(name.to_owned(), bytes);
    }
    let manifest_value = serde_json::to_value(&package_manifest.manifest).unwrap();
    let manifest_bytes = aros_toolchain::canonical::bytes(&manifest_value).unwrap();
    let receipt = json!({
        "schema":"aros-toolchain-native-compatibility-receipt-v4",
        "operation":"native-compatibility",
        "upstream_source_commit":input.profiles.upstream_commit().as_str(),
        "upstream_source_tree":"8888888888888888888888888888888888888888",
        "ports_sources":[port],
        "phase_reports":phase_reports,
        "standalone_targets":standalone_targets,
        "package":{
            "compiler":compiler,
            "host":host,
            "target_profile":profile.name(),
            "target_triple":profile.target_triple(),
            "archive_sha256":package_manifest.archive_sha256,
            "archive_size":package_manifest.archive_size,
            "manifest_sha256":sha256_bytes(&manifest_bytes),
            "tree_sha256":package_manifest.manifest.tree_sha256,
            "source_tree_sha256":sdk_tree
        }
    });
    files.insert(
        "native-compatibility.receipt.json".into(),
        aros_toolchain::canonical::bytes(&receipt).unwrap(),
    );
    let manifest = compatibility_manifest(&files);
    let manifest_sha256 = sha256_bytes(&manifest);
    files.insert("compatibility-measurement.json".into(), manifest);
    for (name, bytes) in &files {
        fs::write(directory.join(name), bytes).unwrap();
    }

    let inputs = compatibility_inputs_document(CompatibilityInputs {
        host,
        compiler: &compiler,
        package: package_manifest,
        profile,
        profiles: &input.profiles,
        sdk_tree: &sdk_tree,
        engine: &engine,
        sdk_environment: &sdk_environment,
        standalone_environment: &standalone_environment,
        helpers: &helpers,
        host_tools: &host_tools,
        port: &port,
    });
    (manifest_sha256, inputs)
}

#[derive(Clone, Copy)]
struct CompatibilityInputs<'a> {
    host: &'a str,
    compiler: &'a ArosCompilerIdentity,
    package: &'a aros_toolchain::package_verify::VerifiedPackage,
    profile: &'a Profile,
    profiles: &'a Profiles,
    sdk_tree: &'a Sha256Digest,
    engine: &'a Sha256Digest,
    sdk_environment: &'a Sha256Digest,
    standalone_environment: &'a Sha256Digest,
    helpers: &'a Value,
    host_tools: &'a Value,
    port: &'a Value,
}

fn compatibility_inputs_document(request: CompatibilityInputs<'_>) -> Vec<u8> {
    let CompatibilityInputs {
        host,
        compiler,
        package,
        profile,
        profiles,
        sdk_tree,
        engine,
        sdk_environment,
        standalone_environment,
        helpers,
        host_tools,
        port,
    } = request;
    let manifest =
        aros_toolchain::canonical::bytes(&serde_json::to_value(&package.manifest).unwrap())
            .unwrap();
    let value = json!({
        "schema":"aros-toolchain-compatibility-inputs-v1",
        "package_manifest_sha256":sha256_bytes(&manifest),
        "archive_sha256":package.archive_sha256,
        "archive_size":package.archive_size,
        "compiler":compiler,
        "package_source_commit":package.manifest.source_commit,
        "host":host,
        "profiles_sha256":profile.document_sha256(),
        "target_profile":profile.name(),
        "target_triple":profile.target_triple(),
        "source_preset":null,
        "cmake_build_required":false,
        "sdk_source_tree_sha256":sdk_tree,
        "engine_api_version":3,
        "engine_sha256":engine,
        "helpers":helpers,
        "host_tools":host_tools,
        "standalone_c_fixture":{"sha256":sha256_bytes(b"synthetic c source fixture"),"size":27},
        "standalone_cxx_fixture":{"sha256":sha256_bytes(b"synthetic cxx source fixture"),"size":29},
        "sdk_environment_sha256":sdk_environment,
        "standalone_environment_sha256":standalone_environment,
        "upstream_source_commit":profiles.upstream_commit().as_str(),
        "upstream_source_tree":"8888888888888888888888888888888888888888",
        "ports_sources":[port]
    });
    aros_toolchain::canonical::bytes(&value).unwrap()
}

fn compatibility_manifest(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let identities = files
        .iter()
        .map(|(name, bytes)| {
            (
                name.clone(),
                json!({"sha256":sha256_bytes(bytes),"size":u64::try_from(bytes.len()).unwrap()}),
            )
        })
        .collect::<Map<_, _>>();
    aros_toolchain::canonical::bytes(&json!({
        "schema":"aros-toolchain-compatibility-measurement-v2",
        "files":identities
    }))
    .unwrap()
}

const fn class_name(class: aros_common::elf::Class) -> &'static str {
    match class {
        aros_common::elf::Class::Elf32 => "elf32",
        aros_common::elf::Class::Elf64 => "elf64",
    }
}

fn fixture_elf64(symbol: &str, machine: u16) -> Vec<u8> {
    let mut names = vec![0_u8];
    names.extend_from_slice(symbol.as_bytes());
    names.push(0);
    let section_offset = 64_usize;
    let section_size = 64_usize;
    let strtab_offset = section_offset + 3 * section_size;
    let symtab_offset = strtab_offset + names.len();
    let mut object = vec![0_u8; symtab_offset + 2 * 24];
    object[..4].copy_from_slice(b"\x7fELF");
    object[4] = 2;
    object[5] = 1;
    object[6] = 1;
    object[7] = aros_common::elf::OS_ABI_AROS;
    object[8] = aros_common::elf::AROS_ABI_VERSION;
    write_u16(&mut object, 0x10, 1);
    write_u16(&mut object, 0x12, machine);
    write_u32(&mut object, 0x14, 1);
    write_u64(&mut object, 0x28, section_offset as u64);
    write_u16(&mut object, 0x34, 64);
    write_u16(&mut object, 0x3a, section_size as u16);
    write_u16(&mut object, 0x3c, 3);
    let strtab = section_offset + section_size;
    write_u32(&mut object, strtab + 4, 3);
    write_u64(&mut object, strtab + 24, strtab_offset as u64);
    write_u64(&mut object, strtab + 32, names.len() as u64);
    write_u64(&mut object, strtab + 48, 1);
    let symtab = strtab + section_size;
    write_u32(&mut object, symtab + 4, 2);
    write_u64(&mut object, symtab + 24, symtab_offset as u64);
    write_u64(&mut object, symtab + 32, 48);
    write_u32(&mut object, symtab + 40, 1);
    write_u64(&mut object, symtab + 48, 8);
    write_u64(&mut object, symtab + 56, 24);
    object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
    let symbol_entry = symtab_offset + 24;
    write_u32(&mut object, symbol_entry, 1);
    object[symbol_entry + 4] = 0x10;
    write_u16(&mut object, symbol_entry + 6, 1);
    object
}

fn fixture_elf32(symbol: &str, machine: u16) -> Vec<u8> {
    let mut names = vec![0_u8];
    names.extend_from_slice(symbol.as_bytes());
    names.push(0);
    let section_offset = 52_usize;
    let section_size = 40_usize;
    let strtab_offset = section_offset + 3 * section_size;
    let symtab_offset = strtab_offset + names.len();
    let mut object = vec![0_u8; symtab_offset + 2 * 16];
    object[..4].copy_from_slice(b"\x7fELF");
    object[4] = 1;
    object[5] = 1;
    object[6] = 1;
    object[7] = aros_common::elf::OS_ABI_AROS;
    object[8] = aros_common::elf::AROS_ABI_VERSION;
    write_u16(&mut object, 0x10, 1);
    write_u16(&mut object, 0x12, machine);
    write_u32(&mut object, 0x14, 1);
    write_u32(&mut object, 0x20, section_offset as u32);
    write_u16(&mut object, 0x28, 52);
    write_u16(&mut object, 0x2e, section_size as u16);
    write_u16(&mut object, 0x30, 3);
    let strtab = section_offset + section_size;
    write_u32(&mut object, strtab + 4, 3);
    write_u32(&mut object, strtab + 16, strtab_offset as u32);
    write_u32(&mut object, strtab + 20, names.len() as u32);
    write_u32(&mut object, strtab + 32, 1);
    let symtab = strtab + section_size;
    write_u32(&mut object, symtab + 4, 2);
    write_u32(&mut object, symtab + 16, symtab_offset as u32);
    write_u32(&mut object, symtab + 20, 32);
    write_u32(&mut object, symtab + 24, 1);
    write_u32(&mut object, symtab + 36, 16);
    object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
    let symbol_entry = symtab_offset + 16;
    write_u32(&mut object, symbol_entry, 1);
    object[symbol_entry + 12] = 0x10;
    write_u16(&mut object, symbol_entry + 14, 1);
    object
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn selection_document(lanes: &BTreeMap<String, LaneSelection>) -> Value {
    let lanes = lanes
        .iter()
        .map(|(asset, lane)| {
            let build = |side: &SideSelection| {
                json!({
                    "package_dir":side.package_dir,
                    "measurement":{"path":side.measurement,"sha256":side.measurement_sha256},
                    "executor":{"contract_sha256":side.contract_sha256,"binary_sha256":side.binary_sha256}
                })
            };
            (
                asset.clone(),
                json!({
                    "build_environment":lane.build_environment,
                    "required_paths":lane.required_paths,
                    "builds":[build(&lane.builds[0]),build(&lane.builds[1])],
                    "comparison":{"path":lane.comparison,"sha256":lane.comparison_sha256},
                    "compatibility":{
                        "directory":lane.compatibility_directory,
                        "inputs":{"path":lane.compatibility_inputs,"sha256":lane.compatibility_inputs_sha256},
                        "manifest_sha256":lane.compatibility_manifest_sha256
                    }
                }),
            )
        })
        .collect::<Map<_, _>>();
    json!({"schema":"aros-toolchain-release-evidence-selection-v2","lanes":lanes})
}

pub fn update_compatibility_manifest(directory: &Path) -> Sha256Digest {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name == "compatibility-measurement.json" {
            continue;
        }
        files.insert(name, fs::read(entry.path()).unwrap());
    }
    let bytes = compatibility_manifest(&files);
    let digest = sha256_bytes(&bytes);
    fs::write(directory.join("compatibility-measurement.json"), bytes).unwrap();
    digest
}

pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let output = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &output);
        } else {
            fs::copy(entry.path(), output).unwrap();
        }
    }
}

pub fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                result.insert(relative.clone(), b"<directory>".to_vec());
                visit(root, &path, result);
            } else if kind.is_symlink() {
                result.insert(
                    relative,
                    fs::read_link(path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                );
            } else {
                result.insert(relative, fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
