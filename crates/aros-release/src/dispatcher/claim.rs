//! Dispatch claim payload (`compiler-sdk-dispatch-claim-v2.py`).
//!
//! The codec checks structural joins only. Authenticity of the signed start,
//! the dispatcher context and the native run is established elsewhere.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::context::{self, ContextExt};
use super::json::{canonical_line, exact, parse, positive};
use super::start;
use super::{fail, require, Result, MAIN_REF, MAX_BUNDLE, MAX_RECORD};

pub const CLAIM_NAME: &str = "compiler-sdk-dispatch-claim-v2.json";
pub const SCHEMA: &str = "aros-toolchains-dispatch-claim-v2";
pub const ASSURANCE: &str = "claim-payload-only-not-dispatch-evidence";
pub const WORKFLOW: &str = ".github/workflows/compiler-sdk-release-dispatch.yml";
pub const JOB: &str = "Dispatch compiler and SDK qualification";
pub const NATIVE_WORKFLOW: &str = ".github/workflows/toolchain-release.yml";

#[must_use]
pub fn sha256_hex(raw: &[u8]) -> String {
    super::json::hex(&Sha256::digest(raw))
}

/// The independently selected dispatcher side of a claim.
#[derive(Debug, Clone)]
pub struct Dispatcher<'a> {
    pub context: &'a Value,
    pub job: &'a Value,
    pub event: &'a str,
    pub trigger_context: &'a Value,
}

/// Encode the structural joins of one claim.
///
/// # Errors
///
/// Fails for unbounded inputs, an invalid start record or any dispatcher
/// selection that is not a distinct, original, protected-main workflow.
pub fn claim_bytes(
    record_bytes: &[u8],
    bundle_bytes: &[u8],
    source_context: &Value,
    dispatcher: &Dispatcher<'_>,
) -> Result<Vec<u8>> {
    require(
        !record_bytes.is_empty() && record_bytes.len() <= MAX_RECORD,
        "claim requires bounded original start bytes",
    )?;
    require(
        !bundle_bytes.is_empty() && bundle_bytes.len() <= MAX_BUNDLE,
        "claim requires bounded original signature bytes",
    )?;
    let record = start::validate(
        &parse(record_bytes, "claim start record", MAX_RECORD)?,
        source_context,
    )?;
    let dispatcher_context = context::validate(dispatcher.context)?;
    let source = record["context"].clone();
    let trigger = context::validate(dispatcher.trigger_context)?;
    require(
        dispatcher.event == "workflow_run" && trigger == source,
        "claim dispatcher event or triggering start context differs",
    )?;
    require(
        dispatcher_context.text("repository") == source.text("repository")
            && dispatcher_context.id("repository_id") == source.id("repository_id")
            && dispatcher_context.text("source_ref") == MAIN_REF
            && dispatcher_context.text("workflow") == WORKFLOW
            && dispatcher_context.id("run_attempt") == 1
            && dispatcher_context.id("run_id") != source.id("run_id"),
        "claim dispatcher is not a distinct original protected-main workflow",
    )?;
    let job = exact(
        dispatcher.job,
        &["id", "check_run_id", "name"],
        "claim dispatcher job",
    )?;
    positive(&job["id"], "claim dispatcher job id")?;
    positive(&job["check_run_id"], "claim dispatcher check id")?;
    require(
        job["name"] == JOB
            && job["id"] != record["job"]["id"]
            && job["check_run_id"] != record["job"]["check_run_id"],
        "claim dispatcher job differs or reuses the start owner",
    )?;
    let value = json!({
        "schema": SCHEMA, "assurance": ASSURANCE, "name": CLAIM_NAME,
        "start": {
            "context": source, "job": record["job"],
            "record_sha256": sha256_hex(record_bytes),
            "bundle_sha256": sha256_hex(bundle_bytes),
        },
        "candidate": {
            "tag": record["tag"], "draft": record["draft"],
            "inputs": record["admission"]["inputs"], "runtime": record["runtime"],
        },
        "dispatcher": {"context": dispatcher_context, "event": dispatcher.event,
                       "trigger_context": trigger, "job": dispatcher.job},
        "native": {"workflow": NATIVE_WORKFLOW, "event": "workflow_dispatch",
                   "source_ref": format!("refs/tags/{}", record["tag"]["name"].as_str().unwrap_or_default()),
                   "producer_commit": record["tag"]["commit"]},
    });
    let encoded = canonical_line(&value);
    require(
        encoded.len() <= MAX_RECORD,
        "encoded claim exceeds its metadata limit",
    )?;
    Ok(encoded)
}

/// Require exact canonical bytes and return the parsed claim.
///
/// # Errors
///
/// Fails when the bytes differ from the claim derived from the selections.
pub fn validate_claim(
    raw: &[u8],
    record_bytes: &[u8],
    bundle_bytes: &[u8],
    source_context: &Value,
    dispatcher: &Dispatcher<'_>,
) -> Result<Value> {
    require(
        !raw.is_empty() && raw.len() <= MAX_RECORD,
        "claim payload is not bounded bytes",
    )?;
    let expected = claim_bytes(record_bytes, bundle_bytes, source_context, dispatcher)?;
    if raw != expected {
        return fail("claim payload differs from the exact selected start and dispatcher");
    }
    parse(raw, "dispatch claim", MAX_RECORD)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{json, Value};

    use crate::dispatcher::context::fixtures::{dispatcher, source};
    use crate::dispatcher::json::canonical_line;
    use crate::dispatcher::start::fixtures::record;

    pub fn record_bytes() -> Vec<u8> {
        canonical_line(&record())
    }

    pub fn bundle_bytes() -> Vec<u8> {
        b"{\"mediaType\":\"sigstore-bundle\"}".to_vec()
    }

    pub fn job() -> Value {
        json!({"id": 21, "check_run_id": 22, "name": super::JOB})
    }

    pub struct Selection {
        pub source: Value,
        pub dispatcher: Value,
        pub job: Value,
    }

    impl Selection {
        pub fn new() -> Self {
            Self {
                source: source(),
                dispatcher: dispatcher(),
                job: job(),
            }
        }

        pub fn view(&self) -> super::Dispatcher<'_> {
            super::Dispatcher {
                context: &self.dispatcher,
                job: &self.job,
                event: "workflow_run",
                trigger_context: &self.source,
            }
        }

        pub fn claim(&self) -> Vec<u8> {
            super::claim_bytes(&record_bytes(), &bundle_bytes(), &self.source, &self.view())
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::fixtures::{bundle_bytes, record_bytes, Selection};
    use super::*;

    #[test]
    fn claim_round_trips_and_is_canonical() {
        let selection = Selection::new();
        let raw = selection.claim();
        assert!(raw.ends_with(b"\n") && raw.is_ascii());
        let claim = validate_claim(
            &raw,
            &record_bytes(),
            &bundle_bytes(),
            &selection.source,
            &selection.view(),
        )
        .unwrap();
        assert_eq!(claim["native"]["source_ref"], "refs/tags/v2.0.0");
        assert_eq!(claim["start"]["record_sha256"], sha256_hex(&record_bytes()));
    }

    #[test]
    fn counterprobes_the_dispatcher_selection() {
        let base = Selection::new();
        let attempt = |select: &dyn Fn(&mut Selection), event: &str| {
            let mut selection = Selection::new();
            select(&mut selection);
            let view = Dispatcher {
                event,
                ..selection.view()
            };
            claim_bytes(&record_bytes(), &bundle_bytes(), &selection.source, &view)
        };
        assert!(attempt(&|_| {}, "workflow_run").is_ok());
        assert!(attempt(&|_| {}, "push").is_err());
        assert!(attempt(&|s| s.dispatcher["run_id"] = 1000.into(), "workflow_run").is_err());
        assert!(attempt(&|s| s.dispatcher["run_attempt"] = 2.into(), "workflow_run").is_err());
        assert!(attempt(
            &|s| s.dispatcher["source_ref"] = "refs/heads/dev".into(),
            "workflow_run"
        )
        .is_err());
        assert!(attempt(
            &|s| s.dispatcher["workflow"] = ".github/workflows/other.yml".into(),
            "workflow_run"
        )
        .is_err());
        assert!(attempt(
            &|s| s.dispatcher["repository_id"] = 1.into(),
            "workflow_run"
        )
        .is_err());
        assert!(attempt(&|s| s.job["id"] = 11.into(), "workflow_run").is_err());
        assert!(attempt(&|s| s.job["check_run_id"] = 12.into(), "workflow_run").is_err());
        assert!(attempt(&|s| s.job["name"] = "x".into(), "workflow_run").is_err());
        assert!(attempt(&|s| s.job["extra"] = 1.into(), "workflow_run").is_err());
        // The trigger must be exactly the start context.
        let mut shifted = base.source.clone();
        shifted["run_id"] = 999.into();
        let view = Dispatcher {
            trigger_context: &shifted,
            ..base.view()
        };
        assert!(claim_bytes(&record_bytes(), &bundle_bytes(), &base.source, &view).is_err());
    }

    #[test]
    fn counterprobes_the_claim_bytes() {
        let selection = Selection::new();
        let good = selection.claim();
        let check = |raw: &[u8], record: &[u8], bundle: &[u8]| {
            validate_claim(raw, record, bundle, &selection.source, &selection.view()).is_err()
        };
        let mut no_newline = good.clone();
        no_newline.pop();
        assert!(check(&no_newline, &record_bytes(), &bundle_bytes()));
        assert!(check(&[], &record_bytes(), &bundle_bytes()));
        let mut flipped = good.clone();
        flipped[10] ^= 1;
        assert!(check(&flipped, &record_bytes(), &bundle_bytes()));
        assert!(check(&good, &record_bytes(), b"{\"other\":1}"));
        assert!(check(&good, b"{}", &bundle_bytes()));
        assert!(check(&good, &record_bytes(), &[]));
        assert!(check(&good, &record_bytes(), &vec![b'x'; MAX_BUNDLE + 1]));
        let mut padded = good;
        padded.extend_from_slice(b" ");
        assert!(check(&padded, &record_bytes(), &bundle_bytes()));
        let _ = json!(null);
    }
}
