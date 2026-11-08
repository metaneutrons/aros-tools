//! `dispatch`: the one native `workflow_dispatch` (aros-toolchains#115).
//!
//! This step only fixes the command surface, the input contract and the
//! fail-closed behaviour. The HTTP dispatch, the bounded HTTP 200 parsing of
//! the real response, the post-dispatch binding signature and the durable
//! once-only consumption (START-A7) are implemented in #115. Until then the
//! command validates its inputs with the same closed codecs and then refuses:
//! it performs no provider call of any kind, so a mutation can neither happen
//! nor be retried.

use serde_json::Value;

use super::claim::sha256_hex;
use super::claim::{validate_claim, Dispatcher};
use super::context;
use super::json::{digest, positive};
use super::{fail, require, Result, MAX_BUNDLE, MAX_RECORD};

pub const NOT_IMPLEMENTED: &str =
    "dispatch is not implemented in this release: the single native dispatch, its binding and \
     once-only consumption land with aros-toolchains#115; nothing was dispatched";

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

/// Validate the input contract, then refuse.
///
/// # Errors
///
/// Always: an invalid contract is a contract violation, and a valid one ends
/// in the documented not-implemented rejection. No provider is contacted.
pub fn dispatch(request: &Request<'_>) -> Result<std::convert::Infallible> {
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
    let asset = super::json::exact(
        request.claim_asset,
        &["id", "name", "size", "sha256"],
        "dispatch claim asset",
    )?;
    positive(&asset["id"], "dispatch claim asset ID")?;
    require(
        asset["sha256"].as_str() == Some(&sha256_hex(request.claim_raw))
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
    fail(NOT_IMPLEMENTED)
}
