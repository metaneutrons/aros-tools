//! Post-dispatch run binding (`compiler-sdk-dispatch-binding-v2.py`).
//!
//! The payload joins one confirmed-response run selector to the exact sole
//! claim asset, claim signature bytes, original start and dispatcher. A 204,
//! a timeout or any uncertain response selects no run and never permits a
//! mutation retry. The codec checks supplied joins, not their authenticity;
//! `actual_native_context` must never be derived from the binding itself.

use serde_json::{json, Value};

use super::claim::{self, sha256_hex, Dispatcher, CLAIM_NAME, NATIVE_WORKFLOW};
use super::context::{self, ContextExt};
use super::json::{canonical_line, digest, exact, parse, positive, provider_id};
use super::{fail, require, Result, MAX_RECORD, REPOSITORY};

pub const BINDING_NAME: &str = "compiler-sdk-dispatch-binding-v2.json";
pub const SCHEMA: &str = "aros-toolchains-dispatch-binding-v2";
pub const ASSURANCE: &str = "dispatch-run-binding-payload-only-not-native-authority";
pub const MAX_RESPONSE: usize = 4096;

fn run_urls(run_id: u64) -> Value {
    json!({
        "workflow_run_id": run_id,
        "run_url": format!("https://api.github.com/repos/{REPOSITORY}/actions/runs/{run_id}"),
        "html_url": format!("https://github.com/{REPOSITORY}/actions/runs/{run_id}"),
    })
}

/// Parse one closed confirmed dispatch response (HTTP 200 with run details).
///
/// This does not observe that run. An empty legacy response supplies no
/// selector: the caller must stop rather than search recent runs, retarget or
/// retry the dispatch.
///
/// # Errors
///
/// Fails for any status other than 200, an empty, unbounded or non-canonical
/// body, or run URLs that differ from the fixed repository.
pub fn confirmed_run(status: u16, raw: &[u8]) -> Result<Value> {
    require(
        status == 200,
        "dispatch is not confirmed HTTP 200; no mutation retry",
    )?;
    require(
        !raw.is_empty() && raw.len() <= MAX_RESPONSE,
        "dispatch response is not bounded bytes; no mutation retry",
    )?;
    let value = parse(raw, "confirmed dispatch response", MAX_RESPONSE)?;
    let fields = exact(
        &value,
        &["workflow_run_id", "run_url", "html_url"],
        "confirmed dispatch response",
    )?;
    let run_id = positive(&fields["workflow_run_id"], "confirmed dispatch run ID")?;
    provider_id(&fields["workflow_run_id"], "confirmed dispatch run ID")?;
    if value != run_urls(run_id) {
        return fail("confirmed dispatch run URLs differ; no mutation retry");
    }
    Ok(value)
}

/// Everything the binding is joined to.
#[derive(Debug, Clone)]
pub struct Selection<'a> {
    pub claim_raw: &'a [u8],
    pub record_bytes: &'a [u8],
    pub start_bundle_bytes: &'a [u8],
    pub source_context: &'a Value,
    pub dispatcher: Dispatcher<'a>,
    pub claim_asset: &'a Value,
    pub expected_claim_bundle_sha256: &'a str,
}

impl Selection<'_> {
    fn claim(&self) -> Result<Value> {
        claim::validate_claim(
            self.claim_raw,
            self.record_bytes,
            self.start_bundle_bytes,
            self.source_context,
            &self.dispatcher,
        )
    }

    fn value(&self, actual_native_context: &Value, actual_native_event: &str) -> Result<Value> {
        let claim = self.claim()?;
        let asset = exact(
            self.claim_asset,
            &["id", "name", "size", "sha256"],
            "binding claim asset",
        )?;
        positive(&asset["id"], "binding claim asset ID")?;
        require(
            asset["name"] == CLAIM_NAME
                && asset["size"].as_u64() == Some(self.claim_raw.len() as u64)
                && asset["size"].is_u64()
                && asset["sha256"].as_str() == Some(&sha256_hex(self.claim_raw)),
            "binding claim asset differs from the exact selected bytes",
        )?;
        let pin_value = Value::from(self.expected_claim_bundle_sha256);
        let bundle_pin = digest(&pin_value, "binding claim signature")?;
        let native = context::validate(actual_native_context)?;
        let source = &claim["start"]["context"];
        let dispatcher = &claim["dispatcher"]["context"];
        let expected = json!({
            "repository": source["repository"], "repository_id": source["repository_id"],
            "producer_commit": claim["native"]["producer_commit"],
            "source_ref": claim["native"]["source_ref"],
            "workflow": NATIVE_WORKFLOW, "run_id": native["run_id"], "run_attempt": 1,
        });
        require(
            actual_native_event == "workflow_dispatch"
                && native == expected
                && native.id("run_id") != source.id("run_id")
                && native.id("run_id") != dispatcher.id("run_id"),
            "binding native event or independently selected original context differs",
        )?;
        Ok(json!({
            "schema": SCHEMA, "assurance": ASSURANCE, "name": BINDING_NAME,
            "start": claim["start"], "dispatcher": claim["dispatcher"],
            "candidate": {"tag": claim["candidate"]["tag"], "draft": claim["candidate"]["draft"]},
            "claim": self.claim_asset, "claim_bundle_sha256": bundle_pin,
            "native": {"context": native, "event": "workflow_dispatch",
                       "response": run_urls(native.id("run_id"))},
        }))
    }
}

fn encode(value: &Value) -> Result<Vec<u8>> {
    let raw = canonical_line(value);
    require(
        !raw.is_empty() && raw.len() <= MAX_RECORD,
        "encoded dispatch binding exceeds its metadata limit",
    )?;
    Ok(raw)
}

/// Construct a structural signing subject after a confirmed response.
///
/// Being able to construct the payload grants no authority.
///
/// # Errors
///
/// Fails when the response is not a confirmed run or any join differs.
pub fn binding_bytes(
    selection: &Selection<'_>,
    response_status: u16,
    response_body: &[u8],
) -> Result<Vec<u8>> {
    let response = confirmed_run(response_status, response_body)?;
    let claim = selection.claim()?;
    let source = &claim["start"]["context"];
    let native = json!({
        "repository": source["repository"], "repository_id": source["repository_id"],
        "producer_commit": claim["native"]["producer_commit"],
        "source_ref": claim["native"]["source_ref"],
        "workflow": NATIVE_WORKFLOW, "run_id": response["workflow_run_id"], "run_attempt": 1,
    });
    encode(&selection.value(&native, "workflow_dispatch")?)
}

/// Match canonical bytes to the native caller's independent actual context.
///
/// # Errors
///
/// Fails for unbounded bytes or any join that differs; a binding copied from
/// another native run cannot pass.
pub fn validate_binding(
    raw: &[u8],
    selection: &Selection<'_>,
    actual_native_context: &Value,
    actual_native_event: &str,
) -> Result<Value> {
    require(
        !raw.is_empty() && raw.len() <= MAX_RECORD,
        "dispatch binding is not bounded bytes",
    )?;
    let value = selection.value(actual_native_context, actual_native_event)?;
    if raw != encode(&value)? {
        return fail("dispatch binding differs from the independent native run, claim or owners");
    }
    parse(raw, "dispatch binding", MAX_RECORD)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{json, Value};

    use crate::dispatcher::claim::fixtures::{
        bundle_bytes, record_bytes, Selection as ClaimSelection,
    };
    use crate::dispatcher::claim::{sha256_hex, CLAIM_NAME, NATIVE_WORKFLOW};
    use crate::dispatcher::context::fixtures::COMMIT;

    pub const NATIVE_RUN: u64 = 3000;

    pub struct Owned {
        pub claim: ClaimSelection,
        pub claim_raw: Vec<u8>,
        pub asset: Value,
        pub bundle_pin: String,
    }

    impl Owned {
        pub fn new() -> Self {
            let claim = ClaimSelection::new();
            let claim_raw = claim.claim();
            let asset = json!({"id": 41, "name": CLAIM_NAME, "size": claim_raw.len(),
                               "sha256": sha256_hex(&claim_raw)});
            Self {
                claim,
                claim_raw,
                asset,
                bundle_pin: sha256_hex(b"claim-signature"),
            }
        }

        pub fn selection<'a>(&'a self, record: &'a [u8], bundle: &'a [u8]) -> super::Selection<'a> {
            super::Selection {
                claim_raw: &self.claim_raw,
                record_bytes: record,
                start_bundle_bytes: bundle,
                source_context: &self.claim.source,
                dispatcher: self.claim.view(),
                claim_asset: &self.asset,
                expected_claim_bundle_sha256: &self.bundle_pin,
            }
        }

        pub fn bytes() -> (Vec<u8>, Vec<u8>) {
            (record_bytes(), bundle_bytes())
        }
    }

    pub fn native(run_id: u64) -> Value {
        json!({"repository": "metaneutrons/aros-toolchains", "repository_id": 4242,
               "producer_commit": COMMIT, "source_ref": "refs/tags/v2.0.0",
               "workflow": NATIVE_WORKFLOW, "run_id": run_id, "run_attempt": 1})
    }

    pub fn response(run_id: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "workflow_run_id": run_id,
            "run_url": format!("https://api.github.com/repos/metaneutrons/aros-toolchains/actions/runs/{run_id}"),
            "html_url": format!("https://github.com/metaneutrons/aros-toolchains/actions/runs/{run_id}"),
        }))
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::fixtures::{native, response, Owned, NATIVE_RUN};
    use super::*;

    #[test]
    fn confirmed_response_counterprobes() {
        assert!(confirmed_run(200, &response(NATIVE_RUN)).is_ok());
        // 204 and every other status select no run.
        for status in [201, 202, 204, 302, 404, 500] {
            assert!(
                confirmed_run(status, &response(NATIVE_RUN)).is_err(),
                "{status}"
            );
        }
        assert!(confirmed_run(200, b"").is_err());
        assert!(confirmed_run(200, &vec![b' '; MAX_RESPONSE + 1]).is_err());
        assert!(confirmed_run(200, b"[]").is_err());
        assert!(confirmed_run(200, b"{\"workflow_run_id\":1}").is_err());
        let wrong = |edit: &dyn Fn(&mut Value)| {
            let mut value: Value = serde_json::from_slice(&response(NATIVE_RUN)).unwrap();
            edit(&mut value);
            confirmed_run(200, &serde_json::to_vec(&value).unwrap()).is_err()
        };
        assert!(wrong(&|v| v["workflow_run_id"] = "3000".into()));
        assert!(wrong(&|v| v["workflow_run_id"] = 0.into()));
        assert!(wrong(&|v| v["workflow_run_id"] = true.into()));
        assert!(wrong(
            &|v| v["workflow_run_id"] = 9_007_199_254_740_992_u64.into()
        ));
        assert!(wrong(
            &|v| v["run_url"] = "https://api.github.com/repos/evil/x/actions/runs/3000".into()
        ));
        assert!(wrong(&|v| {
            v["html_url"] =
                "https://github.com/metaneutrons/aros-toolchains/actions/runs/3001".into();
        }));
        assert!(wrong(&|v| v["extra"] = 1.into()));
        // Duplicate keys and floats never parse.
        assert!(confirmed_run(
            200,
            br#"{"workflow_run_id":1,"workflow_run_id":1,"run_url":"","html_url":""}"#
        )
        .is_err());
        assert!(confirmed_run(
            200,
            br#"{"workflow_run_id":3000.0,"run_url":"","html_url":""}"#
        )
        .is_err());
    }

    #[test]
    fn binding_round_trips_against_the_independent_native_context() {
        let owned = Owned::new();
        let (record, bundle) = Owned::bytes();
        let selection = owned.selection(&record, &bundle);
        let raw = binding_bytes(&selection, 200, &response(NATIVE_RUN)).unwrap();
        let value =
            validate_binding(&raw, &selection, &native(NATIVE_RUN), "workflow_dispatch").unwrap();
        assert_eq!(value["native"]["response"]["workflow_run_id"], NATIVE_RUN);
        assert_eq!(value["claim_bundle_sha256"], owned.bundle_pin);
    }

    #[test]
    fn binding_counterprobes() {
        let owned = Owned::new();
        let (record, bundle) = Owned::bytes();
        let selection = owned.selection(&record, &bundle);
        let raw = binding_bytes(&selection, 200, &response(NATIVE_RUN)).unwrap();
        let rejects = |raw: &[u8], context: &Value, event: &str| {
            validate_binding(raw, &selection, context, event).is_err()
        };
        // Copied binding from another native run.
        assert!(rejects(&raw, &native(NATIVE_RUN + 1), "workflow_dispatch"));
        assert!(rejects(&raw, &native(NATIVE_RUN), "push"));
        assert!(rejects(&raw, &native(NATIVE_RUN), "workflow_run"));
        // The native run must be distinct from the start and dispatcher runs.
        for reused in [1000, 2000] {
            assert!(binding_bytes(&selection, 200, &response(reused)).is_err());
            assert!(rejects(&raw, &native(reused), "workflow_dispatch"));
        }
        for edit in [
            (|c: &mut Value| c["run_attempt"] = 2.into()) as fn(&mut Value),
            |c| c["workflow"] = ".github/workflows/other.yml".into(),
            |c| c["source_ref"] = "refs/tags/v9.9.9".into(),
            |c| c["producer_commit"] = "f".repeat(40).into(),
            |c| c["repository_id"] = 1.into(),
        ] {
            let mut context = native(NATIVE_RUN);
            edit(&mut context);
            assert!(rejects(&raw, &context, "workflow_dispatch"));
        }
        let mut flipped = raw.clone();
        flipped[5] ^= 1;
        assert!(rejects(&flipped, &native(NATIVE_RUN), "workflow_dispatch"));
        assert!(rejects(&[], &native(NATIVE_RUN), "workflow_dispatch"));
        assert!(rejects(
            &raw[..raw.len() - 1],
            &native(NATIVE_RUN),
            "workflow_dispatch"
        ));
        // No binding without a confirmed run.
        for status in [204, 201, 500] {
            assert!(binding_bytes(&selection, status, &response(NATIVE_RUN)).is_err());
        }
        assert!(binding_bytes(&selection, 200, b"").is_err());
    }

    #[test]
    fn claim_asset_and_signature_pin_counterprobes() {
        let owned = Owned::new();
        let (record, bundle) = Owned::bytes();
        let build = |asset: &Value, pin: &str| {
            let selection = Selection {
                claim_asset: asset,
                expected_claim_bundle_sha256: pin,
                ..owned.selection(&record, &bundle)
            };
            binding_bytes(&selection, 200, &response(NATIVE_RUN))
        };
        assert!(build(&owned.asset, &owned.bundle_pin).is_ok());
        let tweak = |key: &str, value: Value| {
            let mut asset = owned.asset.clone();
            asset[key] = value;
            build(&asset, &owned.bundle_pin).is_err()
        };
        assert!(tweak("id", 0.into()));
        assert!(tweak("name", "other.json".into()));
        assert!(tweak("size", 1.into()));
        assert!(tweak("sha256", "0".repeat(64).into()));
        let mut extra = owned.asset.clone();
        extra["state"] = "uploaded".into();
        assert!(build(&extra, &owned.bundle_pin).is_err());
        assert!(build(&owned.asset, "NOT-A-DIGEST").is_err());
        let _ = json!(null);
    }
}
