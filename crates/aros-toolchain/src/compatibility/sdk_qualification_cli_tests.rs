//! Actual producer CLI qualification round-trips for complete SDK matrices.
//!
//! Provider, tag and attestation observations are synthetic claims. These
//! process checks establish byte joins only, not job, signature, execution or
//! release authority.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::{sha256_bytes, sha256_reader, Sha256Digest};
use serde_json::{json, Value};

use crate::sdk_generation_matrix::{
    readback_sdk_generation_matrix, SdkGenerationMatrixReadbackRequest,
};

const SOURCE_REPOSITORY: &str = "https://github.com/example/aros-toolchains";
const SOURCE_WORKFLOW: &str = ".github/workflows/qualification.yml";
const SOURCE_TAG: &str = "release-2026.10";
const SOURCE_TAG_OBJECT: &str = "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a";

struct SelectedDocument {
    path: PathBuf,
    sha256: Sha256Digest,
}

struct ProbeFiles {
    policy: SelectedDocument,
    missing_selection: SelectedDocument,
    evidence_path: PathBuf,
    recovery: Option<SelectedDocument>,
}

struct CliProbe<'p, 'request, 'evidence> {
    binary: &'p Path,
    request: &'p SdkGenerationMatrixReadbackRequest<'request, 'evidence>,
    metadata_root: &'p Path,
    inspect_arguments: &'p [String],
}

/// Run record, verify and recovery through the configured production binary.
pub(super) fn probe(
    binary: &Path,
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    metadata_root: &Path,
    selection: &Value,
    inspect_arguments: &[String],
) {
    assert_eq!(request.generations.len(), 3);
    let context = CliProbe {
        binary,
        request,
        metadata_root,
        inspect_arguments,
    };
    let mut files = prepare_files(metadata_root, selection);
    let claims = expected_sdk_claims(request);
    context.reject_invalid_record(&files);
    let (evidence_sha256, evidence) = context.record(&files.evidence_path, &claims);
    context.verify(&files, &evidence_sha256, &claims);
    files.recovery = Some(context.write_recovery(&evidence, &evidence_sha256));
    context.verify_recovery(&files, &evidence_sha256, &claims);
    context.reject_missing_selection(&files, &evidence_sha256);
    context.reject_mutated_claim(&files, &evidence);
}

impl CliProbe<'_, '_, '_> {
    fn reject_invalid_record(&self, files: &ProbeFiles) {
        let arguments = self.record_arguments(&files.evidence_path, Some(&files.missing_selection));
        assert!(
            !files.evidence_path.exists(),
            "invalid recording output must start absent"
        );
        self.assert_failure(
            &arguments,
            "SDK generation selection must exactly cover the independently selected release lanes",
        );
        assert!(
            !files.evidence_path.exists(),
            "invalid recording must create no output file"
        );
    }

    fn record(&self, evidence_path: &Path, claims: &Value) -> (Sha256Digest, Value) {
        let release_before = self.snapshot_release();
        let arguments = self.record_arguments(evidence_path, None);
        let output = invoke(self.binary, &arguments);
        assert_success(
            &output,
            "record-qualification should join all SDK fixture rows",
        );
        assert_eq!(self.snapshot_release(), release_before);
        let result = json_stdout(&output);
        assert_operation_count(
            &result,
            "record-qualification",
            self.request.generations.len(),
        );
        assert_eq!(result["sdk_generations"], expected_cli_rows(claims));
        assert_no_authority_claims(&result);

        let bytes = fs::read(evidence_path).expect("recorded qualification should exist");
        let digest = sha256_bytes(&bytes);
        assert_eq!(result["qualification_sha256"], digest.as_str());
        let evidence: Value =
            serde_json::from_slice(&bytes).expect("recorded qualification should be valid JSON");
        assert_eq!(&evidence["sdk_generations"], claims);
        assert_no_authority_claims(&evidence);
        (digest, evidence)
    }

    fn verify(&self, files: &ProbeFiles, evidence_sha256: &Sha256Digest, claims: &Value) {
        let arguments = self.qualification_arguments(
            "verify-qualification",
            &files.evidence_path,
            evidence_sha256,
            &files.policy,
        );
        self.assert_successful_read_only(
            &arguments,
            "verify-qualification should reacquire all SDK fixture rows",
            "verify-qualification",
            claims,
        );
    }

    fn write_recovery(&self, evidence: &Value, evidence_sha256: &Sha256Digest) -> SelectedDocument {
        let value = recovery_request(evidence, evidence_sha256);
        write_document(
            self.metadata_root,
            "sdk-qualification-recovery.json",
            &value,
        )
    }

    fn verify_recovery(&self, files: &ProbeFiles, evidence_sha256: &Sha256Digest, claims: &Value) {
        let verify = self.qualification_arguments(
            "verify-qualification",
            &files.evidence_path,
            evidence_sha256,
            &files.policy,
        );
        let recovery = files
            .recovery
            .as_ref()
            .expect("recovery request is selected");
        let arguments = recovery_arguments(&verify, recovery);
        self.assert_successful_read_only(
            &arguments,
            "verify-recovery should reacquire all SDK fixture rows",
            "verify-recovery",
            claims,
        );
    }

    fn reject_missing_selection(&self, files: &ProbeFiles, evidence_sha256: &Sha256Digest) {
        let valid_verify = self.qualification_arguments(
            "verify-qualification",
            &files.evidence_path,
            evidence_sha256,
            &files.policy,
        );
        let mut missing_verify = valid_verify;
        replace_option_value(
            &mut missing_verify,
            "--selection",
            &files.missing_selection.path.to_string_lossy(),
        );
        replace_option_value(
            &mut missing_verify,
            "--selection-sha256",
            &files.missing_selection.sha256,
        );
        self.assert_failure(
            &missing_verify,
            "SDK generation selection must exactly cover the independently selected release lanes",
        );

        let recovery = files
            .recovery
            .as_ref()
            .expect("recovery request is selected");
        let missing_recovery = recovery_arguments(&missing_verify, recovery);
        self.assert_failure(
            &missing_recovery,
            "SDK generation selection must exactly cover the independently selected release lanes",
        );
    }

    fn reject_mutated_claim(&self, files: &ProbeFiles, evidence: &Value) {
        let mut mutated = evidence.clone();
        mutated["sdk_generations"][0]["generation_record_sha256"] =
            json!(sha256_bytes(b"synthetic substituted generation claim"));
        let changed = write_document(
            self.metadata_root,
            "sdk-qualification-mutated.json",
            &mutated,
        );
        let verify = self.qualification_arguments(
            "verify-qualification",
            &changed.path,
            &changed.sha256,
            &files.policy,
        );
        let diagnostic = "SDK qualification claim differs from measured generation bytes";
        self.assert_failure(&verify, diagnostic);

        let recovery_document = self.write_recovery(&mutated, &changed.sha256);
        let replay = recovery_arguments(&verify, &recovery_document);
        self.assert_failure(&replay, diagnostic);
    }

    fn record_arguments(
        &self,
        output_path: &Path,
        selection: Option<&SelectedDocument>,
    ) -> Vec<String> {
        let mut arguments = self.inspect_arguments.to_vec();
        replace_argument(
            &mut arguments,
            "verify-release-evidence",
            "record-qualification",
        );
        replace_argument(&mut arguments, "--directory", "--release-dir");
        for option in ["--release-id", "--base-url"] {
            let position = argument_position(&arguments, option);
            arguments.drain(position..position + 2);
        }
        set_or_append_option(&mut arguments, "--release-format", "family-v2");
        if let Some(selection) = selection {
            replace_option_value(
                &mut arguments,
                "--selection",
                &selection.path.to_string_lossy(),
            );
            replace_option_value(&mut arguments, "--selection-sha256", &selection.sha256);
        }
        let producer_commit = self
            .request
            .complete
            .builds
            .packages
            .index
            .producer_commit();
        arguments.extend([
            "--source-repository".into(),
            SOURCE_REPOSITORY.into(),
            "--source-workflow".into(),
            SOURCE_WORKFLOW.into(),
            "--source-run-id".into(),
            "42".into(),
            "--source-run-attempt".into(),
            "1".into(),
            "--source-tag".into(),
            SOURCE_TAG.into(),
            "--source-tag-object".into(),
            SOURCE_TAG_OBJECT.into(),
            "--source-tag-commit".into(),
            producer_commit.as_str().to_owned(),
            "--attestation-repository".into(),
            SOURCE_REPOSITORY.into(),
            "--attestation-workflow".into(),
            SOURCE_WORKFLOW.into(),
            "--attestation-signer".into(),
            "github-actions".into(),
            "--created-at".into(),
            "100".into(),
            "--expires-at".into(),
            "300".into(),
            "--output".into(),
            output_path.to_string_lossy().into_owned(),
        ]);
        arguments
    }

    fn qualification_arguments(
        &self,
        operation: &str,
        evidence_path: &Path,
        evidence_sha256: &Sha256Digest,
        policy: &SelectedDocument,
    ) -> Vec<String> {
        let mut arguments = self.inspect_arguments.to_vec();
        replace_argument(&mut arguments, "verify-release-evidence", operation);
        arguments.extend([
            "--qualification-evidence".into(),
            evidence_path.to_string_lossy().into_owned(),
            "--qualification-sha256".into(),
            evidence_sha256.to_string(),
            "--policy".into(),
            policy.path.to_string_lossy().into_owned(),
            "--policy-sha256".into(),
            policy.sha256.to_string(),
        ]);
        arguments
    }

    fn assert_successful_read_only(
        &self,
        arguments: &[String],
        message: &str,
        operation: &str,
        claims: &Value,
    ) {
        let before = self.snapshot_all();
        let output = invoke(self.binary, arguments);
        assert_success(&output, message);
        let result = json_stdout(&output);
        assert_operation_count(&result, operation, self.request.generations.len());
        assert_eq!(result["sdk_generations"], expected_cli_rows(claims));
        assert_no_authority_claims(&result);
        assert_eq!(self.snapshot_all(), before);
    }

    fn assert_failure(&self, arguments: &[String], expected: &str) {
        let before = self.snapshot_all();
        let output = invoke(self.binary, arguments);
        assert!(!output.status.success(), "invalid CLI invocation succeeded");
        assert!(
            output.stdout.is_empty(),
            "failed CLI invocation emitted stdout"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "expected {expected:?} in diagnostics:\n{stderr}"
        );
        assert_eq!(self.snapshot_all(), before);
    }

    fn snapshot_all(&self) -> BTreeMap<PathBuf, String> {
        let mut roots = self.selected_roots();
        roots.push(self.metadata_root);
        snapshot_roots(&roots)
    }

    fn snapshot_release(&self) -> BTreeMap<PathBuf, String> {
        snapshot_roots(&self.selected_roots())
    }

    fn selected_roots(&self) -> Vec<&Path> {
        let mut roots = vec![self.request.complete.builds.packages.directory.as_path()];
        for selected in self.request.generations.values() {
            roots.push(selected.package.package_dir.as_path());
            roots.push(selected.compatibility.directory.as_path());
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }
}

fn prepare_files(metadata_root: &Path, selection: &Value) -> ProbeFiles {
    let policy = json!({
        "source_repository": SOURCE_REPOSITORY,
        "source_workflow": SOURCE_WORKFLOW,
        "signer_repository": SOURCE_REPOSITORY,
        "signer_workflow": SOURCE_WORKFLOW,
        "signer": "github-actions",
        "now": 150,
    });
    let policy = write_document(metadata_root, "sdk-qualification-policy.json", &policy);
    let (missing_path, missing_sha256) = missing_generation_selection(selection, metadata_root);
    ProbeFiles {
        policy,
        missing_selection: SelectedDocument {
            path: missing_path,
            sha256: missing_sha256,
        },
        evidence_path: metadata_root.join("sdk-qualification-evidence.json"),
        recovery: None,
    }
}

fn expected_sdk_claims(request: &SdkGenerationMatrixReadbackRequest<'_, '_>) -> Value {
    let measured = readback_sdk_generation_matrix(request)
        .expect("selected SDK fixture bytes should have a complete measured matrix");
    Value::Array(
        measured
            .generations()
            .iter()
            .map(|joined| {
                let lane = joined.lane();
                let generation = joined.generation();
                json!({
                    "sdk_id": lane.sdk_id(),
                    "group_id": lane.group_id(),
                    "target_profile": lane.profile(),
                    "host": lane.host(),
                    "generation_record_sha256": generation.record_sha256(),
                    "compatibility_report_sha256": generation.compatibility_manifest_sha256(),
                    "compatibility_receipt_sha256": generation.compatibility_receipt_sha256(),
                    "sdk_link_receipt_sha256": generation.sdk_link_receipt_sha256(),
                })
            })
            .collect(),
    )
}

fn expected_cli_rows(claims: &Value) -> Value {
    Value::Array(
        claims
            .as_array()
            .expect("SDK claims should be a row array")
            .iter()
            .map(|claim| {
                let mut row = claim.clone();
                let report_sha256 = row
                    .as_object_mut()
                    .unwrap()
                    .remove("compatibility_report_sha256")
                    .unwrap();
                row["compatibility_manifest_sha256"] = report_sha256;
                row
            })
            .collect(),
    )
}

fn recovery_request(evidence: &Value, evidence_sha256: &Sha256Digest) -> Value {
    json!({
        "schema": crate::recovery_v2::RECOVERY_V2_SCHEMA,
        "qualification_sha256": evidence_sha256,
        "operation": "compatibility-replay",
        "failed_stage": "compatibility-harness",
        "observed_run": evidence["source_run"],
        "source_tag": {
            "name": evidence["source_run"]["source_tag"],
            "tag_object": evidence["source_run"]["tag_object"],
            "peeled_commit": evidence["source_run"]["producer_commit"],
        },
        "observed_attestation": evidence["attestation"],
        "handoff": null,
    })
}

fn recovery_arguments(verify_arguments: &[String], recovery: &SelectedDocument) -> Vec<String> {
    let mut arguments = verify_arguments.to_vec();
    replace_argument(&mut arguments, "verify-qualification", "verify-recovery");
    arguments.extend([
        "--recovery-request".into(),
        recovery.path.to_string_lossy().into_owned(),
        "--recovery-request-sha256".into(),
        recovery.sha256.to_string(),
    ]);
    arguments
}

fn missing_generation_selection(selection: &Value, root: &Path) -> (PathBuf, Sha256Digest) {
    let mut missing = selection.clone();
    missing["sdk_generations"].as_array_mut().unwrap().pop();
    let document = write_document(root, "sdk-selection-missing-generation.json", &missing);
    (document.path, document.sha256)
}

fn write_document(root: &Path, name: &str, value: &Value) -> SelectedDocument {
    let bytes = crate::canonical::bytes(value).expect("synthetic CLI metadata should serialize");
    let path = root.join(name);
    fs::write(&path, &bytes).expect("synthetic CLI metadata should be written");
    let path = fs::canonicalize(path).expect("synthetic CLI metadata should canonicalize");
    SelectedDocument {
        path,
        sha256: sha256_bytes(&bytes),
    }
}

fn replace_option_value(arguments: &mut [String], option: &str, value: &impl ToString) {
    let position = argument_position(arguments, option);
    arguments[position + 1] = value.to_string();
}

fn set_or_append_option(arguments: &mut Vec<String>, option: &str, value: &str) {
    if let Some(position) = arguments.iter().position(|argument| argument == option) {
        value.clone_into(&mut arguments[position + 1]);
    } else {
        arguments.extend([option.into(), value.into()]);
    }
}

fn replace_argument(arguments: &mut [String], old: &str, new: &str) {
    let position = argument_position(arguments, old);
    new.clone_into(&mut arguments[position]);
}

fn argument_position(arguments: &[String], name: &str) -> usize {
    arguments
        .iter()
        .position(|argument| argument == name)
        .unwrap_or_else(|| panic!("CLI arguments lack {name}"))
}

fn invoke(binary: &Path, arguments: &[String]) -> Output {
    Command::new(binary)
        .args(arguments)
        .output()
        .expect("configured SDK matrix CLI should execute")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("successful CLI output should be JSON")
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

fn assert_operation_count(result: &Value, operation: &str, count: usize) {
    assert_eq!(result["operation"], operation);
    assert_eq!(result["assurance"], "byte-consistency-only");
    assert_eq!(result["sdk_generation_count"], count);
    assert_eq!(result["sdk_generations"].as_array().unwrap().len(), count);
}

fn assert_no_authority_claims(value: &Value) {
    for field in [
        "execution_authenticated",
        "signature_verified",
        "job_origin_verified",
        "publication_authorized",
        "recovery_authorized",
    ] {
        assert!(
            value.get(field).is_none(),
            "byte-only output claimed {field}"
        );
    }
}

fn snapshot_roots(roots: &[&Path]) -> BTreeMap<PathBuf, String> {
    let mut snapshot = BTreeMap::new();
    for root in roots {
        snapshot_tree(root, &mut snapshot);
    }
    snapshot
}

fn snapshot_tree(path: &Path, snapshot: &mut BTreeMap<PathBuf, String>) {
    let metadata = fs::symlink_metadata(path).expect("snapshot root should exist");
    let kind = metadata.file_type();
    if kind.is_symlink() {
        snapshot.insert(
            path.to_path_buf(),
            format!("link:{}", fs::read_link(path).unwrap().display()),
        );
    } else if kind.is_dir() {
        snapshot.insert(path.to_path_buf(), "directory".into());
        for entry in fs::read_dir(path).expect("snapshot directory should remain readable") {
            snapshot_tree(
                &entry.expect("snapshot entry should be readable").path(),
                snapshot,
            );
        }
    } else if kind.is_file() {
        let measured = sha256_reader(&mut File::open(path).expect("snapshot file should open"))
            .expect("snapshot file should hash");
        snapshot.insert(
            path.to_path_buf(),
            format!("file:{}:{}", measured.size, measured.digest),
        );
    } else {
        snapshot.insert(path.to_path_buf(), "special".into());
    }
}
