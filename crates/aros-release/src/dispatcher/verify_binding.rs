//! `verify-binding`: original-dispatcher signature reader for a native-run
//! binding (`verify-compiler-sdk-dispatch-binding-v2.py`).
//!
//! The pure codec checks joins, not authenticity. This reader captures
//! independently pinned binding and signature bytes and authenticates the
//! original successful dispatcher, the exact signing workflow, commit, ref and
//! attempt, and the fixed binding subject. The actual native context and event
//! and the start, claim and candidate authenticity must be established
//! independently. It never signs, dispatches, consumes or publishes.

use std::path::Path;

use serde_json::{json, Value};

use super::binding::{validate_binding, Selection, BINDING_NAME};
use super::claim::{sha256_hex, Dispatcher, JOB, WORKFLOW as DISPATCH_WORKFLOW};
use super::files;
use super::json::{digest, parse, provider_id};
use super::observe::{complete_jobs, jobs_for, observe_start_run, select_source_job};
use super::provider::Provider;
use super::verify_claim::attest_private;
use super::{require, require_integrity, Result, MAX_BUNDLE, MAX_RECORD};

pub const SCHEMA: &str = "aros-toolchains-authenticated-dispatch-binding-v2";
pub const ASSURANCE: &str = "dispatcher-signature-only-not-native-consumption-or-qualification";
pub const LIMITATIONS: [&str; 3] = [
    "actual native execution identity must be authenticated independently",
    "start, claim and candidate authenticity must be established independently",
    "does not establish durable consumption, authorize fan-out or qualify a release",
];

/// Everything the caller selects independently.
#[derive(Debug)]
pub struct Request<'a> {
    pub binding_path: &'a Path,
    pub signature_path: &'a Path,
    pub expected_binding_sha256: &'a str,
    pub expected_signature_sha256: &'a str,
    pub claim_raw: &'a [u8],
    pub record_bytes: &'a [u8],
    pub start_bundle_bytes: &'a [u8],
    pub source_context: &'a Value,
    pub dispatcher_context: &'a Value,
    pub dispatcher_job: &'a Value,
    pub claim_asset: &'a Value,
    pub expected_claim_bundle_sha256: &'a str,
    pub actual_native_context: &'a Value,
    pub actual_native_event: &'a str,
}

/// Authenticate one selected binding signature, not native authorization.
///
/// # Errors
///
/// Rejects on any pin, join, provider owner, signature or consistency
/// failure, and when a selected input changes during authentication.
pub fn verify_binding_signature(provider: &dyn Provider, request: &Request<'_>) -> Result<Value> {
    require(
        provider.var("GH_HOST").as_deref() == Some("github.com")
            && provider
                .var("GH_TOKEN")
                .is_some_and(|t| !t.trim().is_empty()),
        "binding verification requires the fixed-host job token",
    )?;
    require(
        request.binding_path.file_name().and_then(|n| n.to_str()) == Some(BINDING_NAME)
            && files::absolute(request.binding_path)? != files::absolute(request.signature_path)?,
        "binding subject name or separate signature path differs",
    )?;
    digest(
        &Value::from(request.expected_binding_sha256),
        "selected dispatch binding",
    )?;
    digest(
        &Value::from(request.expected_signature_sha256),
        "selected binding signature",
    )?;
    let raw = files::read_regular(
        request.binding_path,
        "selected dispatch binding",
        MAX_RECORD,
    )?;
    let signature = files::read_regular(
        request.signature_path,
        "selected binding signature",
        MAX_BUNDLE,
    )?;
    require_integrity(
        !raw.is_empty()
            && !signature.is_empty()
            && sha256_hex(&raw) == request.expected_binding_sha256
            && sha256_hex(&signature) == request.expected_signature_sha256,
        "binding or signature differs from the independent byte selection",
    )?;
    let selection = Selection {
        claim_raw: request.claim_raw,
        record_bytes: request.record_bytes,
        start_bundle_bytes: request.start_bundle_bytes,
        source_context: request.source_context,
        dispatcher: Dispatcher {
            context: request.dispatcher_context,
            job: request.dispatcher_job,
            event: "workflow_run",
            trigger_context: request.source_context,
        },
        claim_asset: request.claim_asset,
        expected_claim_bundle_sha256: request.expected_claim_bundle_sha256,
    };
    let selected = || {
        validate_binding(
            &raw,
            &selection,
            request.actual_native_context,
            request.actual_native_event,
        )
    };
    let binding = selected()?;
    let context = binding["dispatcher"]["context"].clone();
    let job = binding["dispatcher"]["job"].clone();
    for id in [
        &binding["start"]["job"]["id"],
        &binding["start"]["job"]["check_run_id"],
        &job["id"],
        &job["check_run_id"],
        &binding["candidate"]["draft"]["id"],
        &binding["claim"]["id"],
    ] {
        provider_id(id, "binding selected identity")?;
    }
    let observe_owner = || -> Result<()> {
        observe_start_run(provider, &context)?;
        let pages = jobs_for(provider, &context)?;
        complete_jobs(&pages)?;
        let observed = select_source_job(&pages, &context, &json!({"id": job["id"], "name": JOB}))?;
        require_integrity(observed == job, "binding original dispatcher job differs")
    };
    observe_owner()?;
    attest_private(
        provider,
        BINDING_NAME,
        &raw,
        &signature,
        &context,
        DISPATCH_WORKFLOW,
    )?;
    observe_owner()?;
    require_integrity(
        files::read_regular(
            request.binding_path,
            "selected dispatch binding",
            MAX_RECORD,
        )? == raw
            && files::read_regular(
                request.signature_path,
                "selected binding signature",
                MAX_BUNDLE,
            )? == signature,
        "binding or signature changed during authentication",
    )?;
    require_integrity(
        selected()? == binding,
        "binding selectors changed during authentication",
    )?;
    let parsed = parse(&raw, "authenticated dispatch binding", MAX_RECORD)?;
    Ok(
        json!({"schema": SCHEMA, "assurance": ASSURANCE, "binding": parsed,
              "limitations": LIMITATIONS}),
    )
}
