//! `verify-claim`: read-only verification of the signed start in the closed
//! phase after the sole claim asset exists
//! (`verify-claimed-compiler-sdk-start-v2.py`).
//!
//! The original start verification still demands an empty draft before the
//! claim upload; this phase authenticates the same signed start while requiring
//! exactly one independently selected claim asset and its exact numeric-ID
//! readback. Returning the authenticated start is NOT native or dispatch
//! authority: the claim signature, the executing native run and the actual
//! native identity are separate mandatory gates.
//!
//! Runtime evidence: the lock, receipt, interface and `bin/aros` digest are
//! joined to the signed start by byte pins, and the held runtime closure is
//! re-derived from the lock and the signed manifest ([`super::runtime`], the
//! port of `RUNTIME.verify_extracted_frozen`). Deviation from the oracle: the
//! interface `checks` list is joined by its byte pin and not re-derived, and
//! the interface is not re-executed.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::claim::{sha256_hex, validate_claim, Dispatcher, CLAIM_NAME};
use super::context::{self, ContextExt};
use super::files;
use super::json::{canonical_line, digest, parse, positive, provider_id};
use super::observe::{
    jobs_for, observe_start_run, observe_tag, require_active_repository, require_frozen_input,
    select_source_job,
};
use super::provider::{Attestation, Provider};
use super::runtime;
use super::start::{self, JOB, WORKFLOW};
use super::{integrity, require, require_integrity, Result, MAX_BUNDLE, MAX_RECORD, REPOSITORY};

pub const SCHEMA: &str = "aros-toolchains-claimed-start-verification-v2";
pub const ASSURANCE: &str = "signed-start-in-claim-phase-only-not-native-or-dispatch-authority";
pub const RUNTIME_RECEIPT: &str = ".aros-tools-runtime-receipt-v1.json";
pub const HOSTS: [&str; 1] = ["linux-x86_64"];
pub const LIMITATIONS: [&str; 4] = [
    "the claim signature, the executing native run and its actual identity remain separate gates",
    "the runtime closure is re-derived from the lock and manifest; the interface checks list is joined by byte pins only",
    "no upload, dispatch, publication or fan-out authority is granted",
    "provider observations are point-in-time, not an atomic snapshot",
];

/// Everything the caller selects independently.
#[derive(Debug)]
pub struct Request<'a> {
    pub record_path: &'a Path,
    pub bundle_path: &'a Path,
    pub claim_path: &'a Path,
    pub claim_asset_id: u64,
    pub source_context: &'a Value,
    pub dispatcher_context: &'a Value,
    pub dispatcher_job: &'a Value,
    pub expected_record_sha256: &'a str,
    pub expected_bundle_sha256: &'a str,
    pub expected_claim_sha256: &'a str,
    pub runtime_lock: &'a Path,
    pub runtime_directory: &'a Path,
    pub runtime_interface: &'a Path,
    pub runtime_host: &'a str,
}

/// Verify a signature through private copies (gh reopens inputs by path).
///
/// Returns the single verified entry's `verificationResult`.
///
/// # Errors
///
/// Rejects anything but exactly one verified signature of the exact subject
/// from the original run attempt.
pub fn attest_private(
    provider: &dyn Provider,
    subject_name: &str,
    subject: &[u8],
    bundle: &[u8],
    context: &Value,
    signer_workflow: &str,
) -> Result<Value> {
    let verified = {
        let Ok(root) = tempfile::Builder::new().prefix("aros-signature-").tempdir() else {
            return integrity("private signature directory is unavailable");
        };
        let subject_dir = root.path().join("subject");
        let bundle_dir = root.path().join("signature");
        let (subject_path, bundle_path) = (
            subject_dir.join(subject_name),
            bundle_dir.join("bundle.json"),
        );
        for (dir, path, data) in [
            (&subject_dir, &subject_path, subject),
            (&bundle_dir, &bundle_path, bundle),
        ] {
            let written = std::fs::create_dir(dir).and_then(|()| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?
                    .write_all(data)
            });
            require_integrity(written.is_ok(), "private signature input cannot be written")?;
        }
        provider.attest(&Attestation {
            subject: &subject_path,
            bundle: &bundle_path,
            repository: context.text("repository"),
            signer_workflow,
            commit: context.text("producer_commit"),
        })?
    };
    let Some([entry]) = verified.as_array().map(Vec::as_slice) else {
        return integrity("signature authentication requires exactly one verified signature");
    };
    let result = &entry["verificationResult"];
    let invocation = format!(
        "https://github.com/{}/actions/runs/{}/attempts/1",
        context.text("repository"),
        context.id("run_id")
    );
    require_integrity(
        result["signature"]["certificate"]["runInvocationURI"].as_str() == Some(&invocation),
        "signature belongs to another original run attempt",
    )?;
    require_integrity(
        result["statement"]["subject"]
            == json!([{"name": subject_name, "digest": {"sha256": sha256_hex(subject)}}]),
        "signature subject differs from the selected bytes",
    )?;
    Ok(result.clone())
}

fn runtime_evidence(record: &Value, request: &Request<'_>) -> Result<()> {
    let selected = &record["runtime"];
    require(
        HOSTS.contains(&request.runtime_host),
        "start runtime host is not independently supported",
    )?;
    let receipt = request.runtime_directory.join(RUNTIME_RECEIPT);
    let paths = [
        ("lock_sha256", request.runtime_lock.to_path_buf()),
        ("receipt_sha256", receipt),
        ("interface_sha256", request.runtime_interface.to_path_buf()),
    ];
    let absolute: Result<Vec<PathBuf>> = paths.iter().map(|(_, p)| files::absolute(p)).collect();
    let mut distinct = absolute?;
    distinct.sort();
    distinct.dedup();
    require(
        distinct.len() == paths.len(),
        "runtime evidence must use separate files",
    )?;
    let mut raw = Vec::new();
    for (key, path) in &paths {
        let bytes = files::read_regular(path, "start runtime evidence", MAX_RECORD)?;
        require_integrity(
            Some(sha256_hex(&bytes).as_str()) == selected[*key].as_str(),
            &format!("start runtime {key} differs from retained bytes"),
        )?;
        raw.push(bytes);
    }
    let interface = parse(&raw[2], "start runtime interface", MAX_RECORD)?;
    require_integrity(
        interface["schema"] == "aros-tools-runtime-interface-preflight-v1"
            && interface["assurance"] == "interface-only-exact-selected-binary"
            && interface["runtime"]["sha256"] == selected["executable_sha256"],
        "start runtime interface schema, assurance or executable differs",
    )?;
    // Re-derive the runtime evidence from the lock and the held closure: the
    // lock, the signed manifest, the exact file set, every byte and mode and
    // the materialization receipt (Python `verify_extracted_frozen`).
    let lock = parse(&raw[0], "start runtime lock", MAX_RECORD)?;
    runtime::validate_lock(&lock)?;
    require(
        lock["repository"] == runtime::RUNTIME_REPOSITORY,
        "start runtime repository differs from the selected runtime",
    )?;
    let lock_sha256 = selected["lock_sha256"].as_str().unwrap_or_default();
    let manifest = runtime::verify_extracted_frozen(
        &lock,
        lock_sha256,
        request.runtime_host,
        request.runtime_directory,
    )?;
    let materialization = parse(&raw[1], "start materialization receipt", MAX_RECORD)?;
    let manifest_sha256 = lock["hosts"][request.runtime_host]["manifest"]["sha256"]
        .as_str()
        .unwrap_or_default();
    require_integrity(
        materialization
            == runtime::receipt(
                &lock,
                request.runtime_host,
                &manifest,
                manifest_sha256,
                lock_sha256,
            ),
        "start materialization receipt differs from the selected locked host",
    )?;
    let executable = manifest["files"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["path"] == "bin/aros"))
        .cloned()
        .unwrap_or(Value::Null);
    require_integrity(
        executable["sha256"] == selected["executable_sha256"],
        "start executable digest differs from the locked manifest",
    )?;
    let observed = &interface["runtime"];
    require_integrity(
        observed["version"] == lock["version"]
            && observed["file_identity"]["size"] == executable["size"]
            && observed["file_identity"]["mode"]
                .as_u64()
                .zip(executable["mode"].as_str())
                .is_some_and(|(mode, locked)| {
                    u64::from_str_radix(locked, 8).is_ok_and(|expected| mode & 0o777 == expected)
                }),
        "start runtime interface version or executable identity differs from the locked manifest",
    )
}

/// Full signed-start verification with a caller-supplied candidate phase.
fn verify_start(
    provider: &dyn Provider,
    request: &Request<'_>,
    candidate: &dyn Fn(&Value) -> Result<()>,
) -> Result<Value> {
    require(
        files::absolute(request.record_path)? != files::absolute(request.bundle_path)?,
        "start record and signature bundle must be separate files",
    )?;
    let raw = files::read_regular(request.record_path, "signed start record", MAX_RECORD)?;
    let bundle = files::read_regular(request.bundle_path, "start signature bundle", MAX_BUNDLE)?;
    let record = start::validate(
        &parse(&raw, "signed start", MAX_RECORD)?,
        request.source_context,
    )?;
    let ctx = record["context"].clone();
    observe_start_run(provider, &ctx)?;
    let source_job = || -> Result<Value> {
        let pages = jobs_for(provider, &ctx)?;
        let job = select_source_job(
            &pages,
            &ctx,
            &json!({"id": record["job"]["id"], "name": JOB}),
        )?;
        let listed = super::observe::complete_jobs(&pages)?;
        let provider_job = listed.iter().find(|item| item["id"] == job["id"]);
        require_integrity(
            provider_job.is_some_and(|item| item["labels"] == json!(["ubuntu-24.04"])),
            "signed start provider runner label differs",
        )?;
        Ok(job)
    };
    let expected = source_job()?;
    require_integrity(
        expected == record["job"],
        "completed provider job differs from the signed start",
    )?;
    let name = request
        .record_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    attest_private(provider, name, &raw, &bundle, &ctx, WORKFLOW)?;
    let inputs = record["admission"]["inputs"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (path, expected_digest) in &inputs {
        require_frozen_input(
            provider,
            ctx.text("producer_commit"),
            path,
            expected_digest.as_str().unwrap_or_default(),
        )?;
    }
    runtime_evidence(&record, request)?;
    candidate(&record)?;
    observe_start_run(provider, &ctx)?;
    require_integrity(
        source_job()? == expected,
        "provider start job changed during authentication",
    )?;
    candidate(&record)?;
    runtime_evidence(&record, request)?;
    require_integrity(
        files::read_regular(request.record_path, "signed start record", MAX_RECORD)? == raw
            && files::read_regular(request.bundle_path, "start signature bundle", MAX_BUNDLE)?
                == bundle,
        "start record or signature changed during authentication",
    )?;
    Ok(record)
}

fn selected_asset(
    provider: &dyn Provider,
    record: &Value,
    asset_id: u64,
    claim_raw: &[u8],
) -> Result<Value> {
    let (draft, tag) = (&record["draft"], &record["tag"]);
    let release = provider.get(&format!("repos/{REPOSITORY}/releases/{}", draft["id"]))?;
    let assets = release["assets"].as_array();
    require_integrity(
        release["id"] == draft["id"]
            && release["tag_name"] == tag["name"]
            && release["target_commitish"] == tag["commit"]
            && release["draft"] == true
            && release["prerelease"] == false
            && release["immutable"] == false
            && assets.is_some_and(|list| list.len() == 1),
        "claimed start requires the exact private sole-claim candidate",
    )?;
    let asset = validate_asset(&release["assets"][0], claim_raw)?;
    require_integrity(
        asset["id"].as_u64() == Some(asset_id),
        "candidate claim differs from the independent numeric selection",
    )?;
    Ok(asset)
}

/// Validate one claim asset row against the exact claim bytes.
///
/// # Errors
///
/// Rejects any name, state, content type, size, digest or URL difference.
pub fn validate_asset(value: &Value, raw: &[u8]) -> Result<Value> {
    require_integrity(value.is_object(), "claim upload returned no asset object")?;
    let id = positive(&value["id"], "claim asset ID")?;
    let digest_text = format!("sha256:{}", sha256_hex(raw));
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/{id}");
    require_integrity(
        value["size"].is_u64()
            && value["name"] == CLAIM_NAME
            && value["state"] == "uploaded"
            && value["content_type"] == "application/json"
            && value["size"].as_u64() == Some(raw.len() as u64)
            && value["digest"] == digest_text.as_str()
            && value["url"] == url.as_str(),
        "claim asset identity, bytes or upload state differs",
    )?;
    Ok(json!({"id": id, "name": CLAIM_NAME, "size": raw.len(), "sha256": sha256_hex(raw)}))
}

fn observe_claimed_candidate(
    provider: &dyn Provider,
    record: &Value,
    asset_id: u64,
    claim_raw: &[u8],
) -> Result<()> {
    let tag = &record["tag"];
    let by_tag_path = format!(
        "repos/{REPOSITORY}/releases/tags/{}",
        tag["name"].as_str().unwrap_or_default()
    );
    require_active_repository(provider)?;
    observe_tag(provider, tag)?;
    let before = selected_asset(provider, record, asset_id, claim_raw)?;
    let by_tag = provider.get(&by_tag_path)?;
    require_integrity(
        by_tag["id"].is_u64() && by_tag["id"] == record["draft"]["id"],
        "claimed start draft ID and tag lookup differ",
    )?;
    require_integrity(
        provider.download_asset(asset_id, MAX_RECORD)? == claim_raw,
        "selected claim numeric readback differs from the captured bytes",
    )?;
    // A GET may change download counters: compare the frozen selection and
    // the exact one-asset closure, not unrelated mutable provider fields.
    require_integrity(
        selected_asset(provider, record, asset_id, claim_raw)? == before,
        "candidate claim changed during numeric readback",
    )?;
    let after = provider.get(&by_tag_path)?;
    require_integrity(
        after["id"].is_u64() && after["id"] == record["draft"]["id"],
        "claimed start tag lookup changed during numeric readback",
    )?;
    observe_tag(provider, tag)?;
    require_active_repository(provider)
}

/// Authenticate the signed start in the closed selected-claim phase.
///
/// All selectors must be independently chosen by the trusted caller. The
/// claim signature, the executing native run and upload/publication authority
/// are not established here.
///
/// # Errors
///
/// Rejects on any pin, join, provider, signature, runtime or consistency
/// failure, and when any selected input changes during verification.
pub fn verify_claimed_start(provider: &dyn Provider, request: &Request<'_>) -> Result<Value> {
    require(
        provider.var("GH_HOST").as_deref() == Some("github.com")
            && provider
                .var("GH_TOKEN")
                .is_some_and(|t| !t.trim().is_empty()),
        "claimed start requires the fixed-host job token",
    )?;
    provider_id(&json!(request.claim_asset_id), "selected claim asset ID")?;
    require(
        request.claim_path.file_name().and_then(|n| n.to_str()) == Some(CLAIM_NAME),
        "selected claim filename differs",
    )?;
    let selections = [
        (
            request.record_path,
            MAX_RECORD,
            request.expected_record_sha256,
            "selected start record",
        ),
        (
            request.bundle_path,
            MAX_BUNDLE,
            request.expected_bundle_sha256,
            "selected start signature",
        ),
        (
            request.claim_path,
            MAX_RECORD,
            request.expected_claim_sha256,
            "selected claim",
        ),
    ];
    let mut captured = Vec::new();
    for (path, maximum, pin, label) in selections {
        digest(&Value::from(pin), label)?;
        let raw = files::read_regular(path, label, maximum)?;
        require_integrity(
            !raw.is_empty() && sha256_hex(&raw) == pin,
            "claimed start input differs from its independent byte selection",
        )?;
        captured.push(raw);
    }
    let (record_raw, bundle_raw, claim_raw) = (&captured[0], &captured[1], &captured[2]);
    let source = context::validate(request.source_context)?;
    let dispatcher = Dispatcher {
        context: request.dispatcher_context,
        job: request.dispatcher_job,
        event: "workflow_run",
        trigger_context: &source,
    };
    let claim_check = || validate_claim(claim_raw, record_raw, bundle_raw, &source, &dispatcher);
    claim_check()?;
    let record = verify_start(provider, request, &|record| {
        observe_claimed_candidate(provider, record, request.claim_asset_id, claim_raw)
    })?;
    for ((path, maximum, _, label), raw) in selections.iter().zip(&captured) {
        require_integrity(
            &files::read_regular(path, label, *maximum)? == raw,
            "selected start, signature or claim changed during verification",
        )?;
    }
    claim_check()?;
    Ok(record)
}

/// Canonical receipt of a successful verification.
#[must_use]
pub fn report(record: &Value, request: &Request<'_>) -> Vec<u8> {
    canonical_line(&json!({
        "schema": SCHEMA, "assurance": ASSURANCE,
        "start": {"record_sha256": request.expected_record_sha256,
                  "bundle_sha256": request.expected_bundle_sha256,
                  "context": record["context"], "tag": record["tag"], "draft": record["draft"]},
        "claim": {"asset_id": request.claim_asset_id, "sha256": request.expected_claim_sha256},
        "limitations": LIMITATIONS,
    }))
}
