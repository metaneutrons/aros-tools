//! `claim`: build the canonical claim and upload it once as the sole asset of
//! the exact empty private draft (aros-toolchains#116, port of
//! `compiler-sdk-claim-upload-v2.py`).
//!
//! The start is authenticated first, in the unclaimed phase: the draft must
//! still be exactly the empty private candidate. One upload attempt follows;
//! it needs a confirmed HTTP 201, an asset row that matches the exact bytes, a
//! numeric-ID byte readback and the exact one-asset closure of the draft.
//! Nothing is retried, replaced, deleted or dispatched. A successful claim is
//! not dispatch authority: the dispatcher still attests the canonical claim
//! under its own workflow and the native entry authenticates that signature.

use std::path::Path;

use serde_json::{json, Value};

use super::claim::{claim_bytes, sha256_hex, validate_claim, Dispatcher, CLAIM_NAME};
use super::context;
use super::files;
use super::json::{digest, parse};
use super::observe::{observe_tag, require_active_repository};
use super::provider::Provider;
use super::verify_claim::{selected_asset, validate_asset, verify_start, Request as StartRequest};
use super::{require, require_integrity, Result, MAX_BUNDLE, MAX_RECORD, REPOSITORY};

/// Everything the trusted caller selects independently.
#[derive(Debug)]
pub struct Request<'a> {
    pub record_path: &'a Path,
    pub bundle_path: &'a Path,
    pub source_context: &'a Value,
    pub dispatcher_context: &'a Value,
    pub dispatcher_job: &'a Value,
    pub expected_record_sha256: &'a str,
    pub expected_bundle_sha256: &'a str,
    pub runtime_lock: &'a Path,
    pub runtime_directory: &'a Path,
    pub runtime_interface: &'a Path,
    pub runtime_host: &'a str,
}

/// The uploaded claim: its canonical bytes and the asset selector.
#[derive(Debug)]
pub struct Uploaded {
    pub claim: Vec<u8>,
    pub asset: Value,
}

/// The unclaimed candidate: exactly the empty private draft of the start.
fn observe_empty_candidate(provider: &dyn Provider, record: &Value) -> Result<()> {
    let (draft, tag) = (&record["draft"], &record["tag"]);
    require_active_repository(provider)?;
    observe_tag(provider, tag)?;
    let release = provider.get(&format!("repos/{REPOSITORY}/releases/{}", draft["id"]))?;
    require_integrity(
        release["id"] == draft["id"]
            && release["tag_name"] == tag["name"]
            && release["target_commitish"] == tag["commit"]
            && release["draft"] == true
            && release["prerelease"] == false
            && release["immutable"] == false
            && release["assets"].as_array().is_some_and(Vec::is_empty),
        "claim upload requires the exact empty private draft",
    )
}

/// Authenticate the start, then upload the claim exactly once.
///
/// # Errors
///
/// Fails on any pin, join, provider, signature, runtime or consistency
/// difference, on a draft that is not empty, and on any upload answer other
/// than a confirmed HTTP 201 with a matching asset and byte readback.
pub fn upload_claim(provider: &dyn Provider, request: &Request<'_>) -> Result<Uploaded> {
    require(
        provider.var("GH_HOST").as_deref() == Some("github.com")
            && provider
                .var("GH_TOKEN")
                .is_some_and(|token| !token.trim().is_empty()),
        "claim upload requires the fixed-host job token",
    )?;
    let mut captured = Vec::new();
    for (path, maximum, pin, label) in [
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
    ] {
        digest(&Value::from(pin), label)?;
        let raw = files::read_regular(path, label, maximum)?;
        require_integrity(
            !raw.is_empty() && sha256_hex(&raw) == pin,
            "start input differs from its independent byte selection",
        )?;
        captured.push(raw);
    }
    let (record_raw, bundle_raw) = (&captured[0], &captured[1]);
    let source = context::validate(request.source_context)?;
    let dispatcher = Dispatcher {
        context: request.dispatcher_context,
        job: request.dispatcher_job,
        event: "workflow_run",
        trigger_context: &source,
    };
    let claim = claim_bytes(record_raw, bundle_raw, &source, &dispatcher)?;
    // The unclaimed phase shares the signed-start verification of verify-claim.
    // Its claim fields are not read there; they stay inert placeholders.
    let claim_name = Path::new(CLAIM_NAME);
    let start_request = StartRequest {
        record_path: request.record_path,
        bundle_path: request.bundle_path,
        claim_path: claim_name,
        claim_asset_id: 0,
        source_context: request.source_context,
        dispatcher_context: request.dispatcher_context,
        dispatcher_job: request.dispatcher_job,
        expected_record_sha256: request.expected_record_sha256,
        expected_bundle_sha256: request.expected_bundle_sha256,
        expected_claim_sha256: "",
        runtime_lock: request.runtime_lock,
        runtime_directory: request.runtime_directory,
        runtime_interface: request.runtime_interface,
        runtime_host: request.runtime_host,
    };
    let record = verify_start(provider, &start_request, &|record| {
        observe_empty_candidate(provider, record)
    })?;
    // The inputs may not change between the claim construction and the upload.
    require_integrity(
        validate_claim(&claim, record_raw, bundle_raw, &source, &dispatcher).is_ok()
            && claim == claim_bytes(record_raw, bundle_raw, &source, &dispatcher)?,
        "claim differs from its canonical construction",
    )?;

    let release_id = record["draft"]["id"].as_u64().unwrap_or_default();
    // The single mutation: one attempt, no retry whatever the answer.
    let (status, body) = provider.upload_claim(release_id, CLAIM_NAME, &claim)?;
    require_integrity(
        status == 201,
        "claim upload is not confirmed HTTP 201; no retry or dispatch",
    )?;
    let row = parse(&body, "claim upload asset", MAX_RECORD)?;
    let asset = validate_asset(&row, &claim)?;
    let asset_id = asset["id"].as_u64().unwrap_or_default();
    require_integrity(
        provider.download_asset(asset_id, MAX_RECORD)? == claim,
        "claim numeric readback differs; no retry or dispatch",
    )?;
    require_integrity(
        selected_asset(provider, &record, asset_id, &claim)? == asset,
        "claim does not belong to the exact selected draft; no dispatch",
    )?;
    require_active_repository(provider)?;
    observe_tag(provider, &record["tag"])?;
    require_integrity(
        files::read_regular(request.record_path, "signed start record", MAX_RECORD)? == *record_raw
            && files::read_regular(request.bundle_path, "start signature bundle", MAX_BUNDLE)?
                == *bundle_raw,
        "start record or signature changed during the claim upload",
    )?;
    Ok(Uploaded {
        claim,
        asset: json!({"id": asset["id"], "name": asset["name"], "size": asset["size"],
                      "sha256": asset["sha256"]}),
    })
}
