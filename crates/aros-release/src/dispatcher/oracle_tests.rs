//! Differential tests against the Python adapters of aros-toolchains.
//!
//! Enabled by `AROS_TOOLCHAINS_CHECKOUT` (a checkout of
//! `metaneutrons/aros-toolchains`); skipped otherwise because CI has no such
//! checkout. The inputs are offline fixtures; Python is run in isolated mode
//! and only ever loads the checked-out scripts.
#![allow(
    clippy::type_complexity,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls,
    reason = "scripted fixtures favour one flat, readable probe table per command"
)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use super::binding::binding_bytes;
use super::binding::fixtures::{native, response, Owned, NATIVE_RUN};
use super::claim::fixtures::{bundle_bytes, record_bytes};
use super::json::{canonical, parse};

const DRIVER: &str = r#"
import importlib.util, json, sys, pathlib
root = pathlib.Path(sys.argv[1]) / "scripts" / "toolchain"
def load(name, file):
    spec = importlib.util.spec_from_file_location(name, root / file)
    mod = importlib.util.module_from_spec(spec); spec.loader.exec_module(mod); return mod
job = json.loads(sys.stdin.read())
binding = load("oracle_binding", "compiler-sdk-dispatch-binding-v2.py")
claim = binding.CLAIM
record, bundle = bytes.fromhex(job["record"]), bytes.fromhex(job["bundle"])
out = {}
out["canonical"] = [claim.START.ADMISSION.canonical_json(v) for v in job["canonical"]]
raw = claim.claim_bytes(record, bundle, source_context=job["source"], dispatcher_context=job["dispatcher"],
    dispatcher_job=job["job"], dispatcher_event="workflow_run", trigger_context=job["source"])
out["claim"] = raw.hex()
out["binding"] = binding.binding_bytes(raw, record, bundle, source_context=job["source"],
    dispatcher_context=job["dispatcher"], dispatcher_job=job["job"], claim_asset=job["asset"],
    expected_claim_bundle_sha256=job["pin"], response_status=200,
    response_body=bytes.fromhex(job["response"])).hex()
for status in (204, 201, 500):
    try:
        binding.confirmed_run(status, bytes.fromhex(job["response"])); out["accepted_%d" % status] = True
    except Exception:
        out["accepted_%d" % status] = False
print(json.dumps(out))
"#;

fn checkout() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("AROS_TOOLCHAINS_CHECKOUT")?);
    path.join("scripts/toolchain/compiler-sdk-dispatch-binding-v2.py")
        .is_file()
        .then_some(path)
}

#[test]
fn claim_binding_and_canonical_json_match_the_python_oracle() {
    let Some(root) = checkout() else {
        eprintln!("AROS_TOOLCHAINS_CHECKOUT unset: Python oracle comparison skipped");
        return;
    };
    let owned = Owned::new();
    let (record, bundle) = (record_bytes(), bundle_bytes());
    let selection = owned.selection(&record, &bundle);
    let canonical_samples = json!([
        {"b": [1, "é\u{e9}", "\u{1F600}", "\u{7f}", "\n\t\"\\"], "a": null, "c": {"z": true, "y": false}},
        [], {}, "plain", 0, 9_007_199_254_740_991_u64
    ]);
    let job = json!({
        "record": hex(&record), "bundle": hex(&bundle), "source": owned.claim.source,
        "dispatcher": owned.claim.dispatcher, "job": owned.claim.job, "asset": owned.asset,
        "pin": owned.bundle_pin, "response": hex(&response(NATIVE_RUN)),
        "canonical": canonical_samples,
    });
    let mut child = Command::new("python3")
        .args(["-I", "-c", DRIVER])
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3 is required for the oracle comparison");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(job.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "oracle driver failed");
    let oracle: Value = serde_json::from_slice(&output.stdout).unwrap();

    for (index, sample) in canonical_samples.as_array().unwrap().iter().enumerate() {
        assert_eq!(
            oracle["canonical"][index].as_str().unwrap(),
            canonical(sample),
            "{sample}"
        );
    }
    assert_eq!(oracle["claim"].as_str().unwrap(), hex(&owned.claim_raw));
    let ours = binding_bytes(&selection, 200, &response(NATIVE_RUN)).unwrap();
    assert_eq!(oracle["binding"].as_str().unwrap(), hex(&ours));
    for status in [204, 201, 500] {
        assert_eq!(oracle[format!("accepted_{status}")], false);
    }
    // The independent native context built here must also validate.
    assert!(super::binding::validate_binding(
        &ours,
        &selection,
        &native(NATIVE_RUN),
        "workflow_dispatch"
    )
    .is_ok());
    let _ = parse(&ours, "x", 1 << 20).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    super::json::hex(bytes)
}
