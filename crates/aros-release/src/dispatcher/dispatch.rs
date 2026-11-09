//! `dispatch`: the one native `workflow_dispatch` (aros-toolchains#115).
//!
//! The dispatcher has already authenticated the signed start and attested the
//! canonical claim. This step sends exactly one `workflow_dispatch` at the
//! immutable tag ref with `return_run_details`, parses the bounded HTTP 200
//! answer, reads the named run back and emits the canonical binding subject
//! that the dispatcher workflow then attests (aros-toolchains#116).
//!
//! Fail-closed rules: a previous native dispatch at this tag ref stops before
//! any mutation; one attempt only, never a retry; HTTP 204, any other status,
//! an unknown or unbounded answer stops without searching for a convenient
//! recent run, and a native run that differs from the claim stops after the
//! mutation with the outcome left for diagnosis.
//!
//! Durable once-only consumption of the start (START-A7) is not decided here:
//! the read-based guard below is point-in-time and cannot exclude two racing
//! dispatchers. The marker mechanism is an open design decision.

use serde_json::Value;

use super::binding::{self, Selection};
use super::claim::{sha256_hex, validate_claim, Dispatcher, CLAIM_NAME, NATIVE_WORKFLOW};
use super::context;
use super::json::{digest, exact, positive};
use super::provider::Provider;
use super::{require, require_integrity, Result, MAX_BUNDLE, MAX_RECORD, REPOSITORY};

/// Native workflow file name inside the fixed repository.
pub const NATIVE_FILE: &str = "toolchain-release.yml";
/// Frozen native inputs: the only dispatch call this step can make.
pub const NATIVE_INPUTS: [(&str, &str); 3] = [
    ("mode", "release-v2"),
    ("scope", "full"),
    ("diagnostic_group", ""),
];

/// The closed input contract of `dispatch`.
#[derive(Debug)]
pub struct Request<'a> {
    pub claim_raw: &'a [u8],
    pub record_bytes: &'a [u8],
    pub start_bundle_bytes: &'a [u8],
    pub source_context: &'a Value,
    pub dispatcher_context: &'a Value,
    pub dispatcher_job: &'a Value,
    pub claim_asset: &'a Value,
    pub claim_bundle_sha256: &'a str,
}

fn tag_of(claim: &Value) -> Result<(String, String)> {
    let tag = claim["candidate"]["tag"]["name"]
        .as_str()
        .unwrap_or_default();
    let commit = claim["native"]["producer_commit"]
        .as_str()
        .unwrap_or_default();
    require(
        !tag.is_empty() && claim["native"]["source_ref"] == format!("refs/tags/{tag}").as_str(),
        "dispatch claim does not select the immutable tag ref",
    )?;
    require(
        super::json::is_hex(commit, 40) && claim["candidate"]["tag"]["commit"] == commit,
        "dispatch claim commit differs from the annotated tag commit",
    )?;
    Ok((tag.to_owned(), commit.to_owned()))
}

/// No native run may exist at this tag ref before the dispatch.
fn require_no_previous_dispatch(provider: &dyn Provider, tag: &str) -> Result<()> {
    let listing = provider.get(&format!(
        "repos/{REPOSITORY}/actions/workflows/{NATIVE_FILE}/runs?event=workflow_dispatch&branch={tag}&per_page=100"
    ))?;
    let total = listing["total_count"].as_u64();
    let runs = listing["workflow_runs"].as_array();
    require_integrity(
        total == Some(0) && runs.is_some_and(Vec::is_empty),
        "a native dispatch already exists for this tag ref, or the listing is uncertain; nothing was dispatched",
    )
}

/// Read the named run back and require it to be the claim's native run.
fn observe_native_run(
    provider: &dyn Provider,
    source: &Value,
    tag: &str,
    commit: &str,
    run_id: u64,
) -> Result<()> {
    let run = provider.get(&format!("repos/{REPOSITORY}/actions/runs/{run_id}"))?;
    let path_tagged = format!("{NATIVE_WORKFLOW}@refs/tags/{tag}");
    require_integrity(
        run["id"].as_u64() == Some(run_id)
            && run["run_attempt"] == 1
            && run["event"] == "workflow_dispatch"
            && run["head_sha"] == commit
            && run["head_branch"] == tag
            && (run["path"] == NATIVE_WORKFLOW || run["path"] == path_tagged.as_str())
            && run["repository"]["full_name"] == REPOSITORY
            && run["repository"]["id"] == source["repository_id"]
            && run["head_repository"]["id"] == source["repository_id"],
        "dispatched native run differs from the claim; stop and diagnose, never retry",
    )
}

/// Send the one native dispatch and return the canonical binding subject.
///
/// # Errors
///
/// Fails on an invalid contract, a previous native dispatch, an unconfirmed or
/// unknown dispatch answer, or a native run that differs from the claim. The
/// dispatch is attempted at most once; a failure after the mutation is never
/// retried.
pub fn dispatch(provider: &dyn Provider, request: &Request<'_>) -> Result<Vec<u8>> {
    require(
        !request.claim_raw.is_empty()
            && request.claim_raw.len() <= MAX_RECORD
            && request.start_bundle_bytes.len() <= MAX_BUNDLE,
        "dispatch inputs are not bounded bytes",
    )?;
    let source = context::validate(request.source_context)?;
    let dispatcher = Dispatcher {
        context: request.dispatcher_context,
        job: request.dispatcher_job,
        event: "workflow_run",
        trigger_context: &source,
    };
    let claim = validate_claim(
        request.claim_raw,
        request.record_bytes,
        request.start_bundle_bytes,
        &source,
        &dispatcher,
    )?;
    let asset = exact(
        request.claim_asset,
        &["id", "name", "size", "sha256"],
        "dispatch claim asset",
    )?;
    positive(&asset["id"], "dispatch claim asset ID")?;
    require(
        asset["name"] == CLAIM_NAME
            && asset["sha256"].as_str() == Some(&sha256_hex(request.claim_raw))
            && asset["size"].as_u64() == Some(request.claim_raw.len() as u64),
        "dispatch claim asset differs from the exact selected claim bytes",
    )?;
    digest(
        &Value::from(request.claim_bundle_sha256),
        "dispatch claim signature",
    )?;
    require(
        claim["native"]["event"] == "workflow_dispatch",
        "dispatch claim does not select a workflow_dispatch native run",
    )?;
    let (tag, commit) = tag_of(&claim)?;

    require_no_previous_dispatch(provider, &tag)?;
    // The single mutation. No retry follows, whatever the answer.
    let (status, body) =
        provider.dispatch_workflow(NATIVE_FILE, &format!("refs/tags/{tag}"), &NATIVE_INPUTS)?;
    let confirmed = binding::confirmed_run(status, &body)?;
    let run_id = confirmed["workflow_run_id"].as_u64().unwrap_or_default();
    observe_native_run(provider, &source, &tag, &commit, run_id)?;
    let selection = Selection {
        claim_raw: request.claim_raw,
        record_bytes: request.record_bytes,
        start_bundle_bytes: request.start_bundle_bytes,
        source_context: &source,
        dispatcher,
        claim_asset: request.claim_asset,
        expected_claim_bundle_sha256: request.claim_bundle_sha256,
    };
    let subject = binding::binding_bytes(&selection, status, &body)?;
    // The run must still be the same one when the subject is returned.
    observe_native_run(provider, &source, &tag, &commit, run_id)?;
    Ok(subject)
}
