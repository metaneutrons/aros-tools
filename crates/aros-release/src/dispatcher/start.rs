//! Signed start record joins (`release-admission-v2.py::validate_receipt`,
//! `compiler-sdk-release-start-v2.py::validate_start`).
//!
//! Validation here checks joins only; it confers no authenticity.

use serde_json::Value;

use super::context::{self, ContextExt};
use super::json::{digest, exact, is_hex, positive, string};
use super::{fail, require, Result, MAIN_REF, REPOSITORY};

pub const WORKFLOW: &str = ".github/workflows/compiler-sdk-release-start.yml";
pub const JOB: &str = "Create protected compiler and SDK start";
pub const SCHEMA: &str = "aros-toolchains-release-start-v2";
pub const ASSURANCE: &str = "protected-start-only-not-native-qualification";
pub const RUNTIME_LOCK_INPUT: &str = "toolchains/aros-tools-runtime-v1.json";

/// Frozen input paths of the admission receipt, in the oracle's order.
pub const INPUT_PATHS: [&str; 7] = [
    ".release-please-manifest.json",
    "version.txt",
    "CHANGELOG.md",
    "toolchains/aros-tools-runtime-v1.json",
    "toolchains/compiler-sdk-ci-selection-v2.json",
    ".github/workflows/ci.yml",
    "scripts/toolchain/materialize-aros-tools-runtime.py",
];

fn is_semver(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0'))
        })
}

/// Validate the embedded admission receipt.
///
/// # Errors
///
/// Fails for any malformed or over-claiming receipt.
pub fn validate_receipt(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    let receipt = exact(
        value,
        &[
            "schema",
            "assurance",
            "repository",
            "commit",
            "version",
            "tag",
            "pull_request",
            "runs",
            "governance",
            "inputs",
            "release_absence",
        ],
        "admission receipt",
    )?;
    require(
        receipt["schema"] == "aros-toolchains-release-admission-v2"
            && receipt["assurance"] == "metadata-only"
            && receipt["repository"] == REPOSITORY,
        "admission receipt identity is malformed",
    )?;
    require(
        receipt["commit"].as_str().is_some_and(|c| is_hex(c, 40)),
        "admission receipt commit is malformed",
    )?;
    let version = receipt["version"].as_str().unwrap_or_default();
    require(
        is_semver(version) && receipt["tag"].as_str() == Some(&format!("v{version}")),
        "admission receipt version or tag is malformed",
    )?;
    positive(&receipt["pull_request"], "admission receipt pull request")?;
    let runs = exact(
        &receipt["runs"],
        &["producer_contracts"],
        "admission receipt runs",
    )?;
    let ci = exact(
        &runs["producer_contracts"],
        &["id", "attempt"],
        "producer contracts receipt",
    )?;
    positive(&ci["id"], "admission receipt CI run id")?;
    positive(&ci["attempt"], "admission receipt CI attempt")?;
    let governance = exact(
        &receipt["governance"],
        &["ruleset_id", "policy_sha256", "checked_at"],
        "admission receipt governance",
    )?;
    positive(&governance["ruleset_id"], "admission receipt ruleset id")?;
    digest(
        &governance["policy_sha256"],
        "admission receipt governance digest",
    )?;
    require(
        governance["checked_at"].is_i64() || governance["checked_at"].is_u64(),
        "admission receipt governance time is malformed",
    )?;
    let inputs = exact(
        &receipt["inputs"],
        &INPUT_PATHS,
        "admission receipt input hashes",
    )?;
    for hash in inputs.values() {
        digest(hash, "admission receipt input hash")?;
    }
    let absence = exact(
        &receipt["release_absence"],
        &[
            "candidate_tag_ref",
            "visible_release",
            "private_draft_absence",
            "required_start_recheck",
        ],
        "admission receipt release absence",
    )?;
    let recheck = exact(
        &absence["required_start_recheck"],
        &["method", "endpoint", "permission", "absence_status"],
        "privileged release absence recheck",
    )?;
    require(
        absence["candidate_tag_ref"] == "absent"
            && absence["visible_release"] == "absent"
            && absence["private_draft_absence"] == "unproven"
            && recheck["method"] == "GET"
            && recheck["endpoint"].as_str()
                == Some(&format!("repos/{REPOSITORY}/releases/tags/v{version}"))
            && recheck["permission"] == "contents:write"
            && recheck["absence_status"] == 404,
        "admission receipt overstates release absence or omits its privileged start recheck",
    )?;
    Ok(receipt)
}

/// Validate a start record against the independently selected source context.
///
/// # Errors
///
/// Fails for any deviation from the closed start record.
pub fn validate(value: &Value, source_context: &Value) -> Result<Value> {
    let ctx = context::validate(source_context)?;
    require(
        ctx.text("repository") == REPOSITORY
            && ctx.text("workflow") == WORKFLOW
            && ctx.text("source_ref") == MAIN_REF
            && ctx.id("run_attempt") == 1,
        "start source is not the original protected-main workflow",
    )?;
    let record = exact(
        value,
        &[
            "schema",
            "assurance",
            "context",
            "event",
            "job",
            "admission",
            "tag",
            "draft",
            "runtime",
        ],
        "start record",
    )?;
    require(
        record["schema"] == SCHEMA && record["assurance"] == ASSURANCE,
        "start record schema or assurance differs",
    )?;
    require(
        context::validate(&record["context"])? == ctx,
        "start record context differs from independent source selection",
    )?;
    require(
        record["event"] == "workflow_run",
        "start record requires the actual main-CI completion event",
    )?;
    let admission = validate_receipt(&record["admission"])?;
    require(
        admission["commit"].as_str() == Some(ctx.text("producer_commit")),
        "admission selects another producer commit",
    )?;
    let job = exact(&record["job"], &["id", "check_run_id", "name"], "start job")?;
    positive(&job["id"], "start job id")?;
    positive(&job["check_run_id"], "start check id")?;
    require(job["name"] == JOB, "start job name differs")?;
    let tag = exact(
        &record["tag"],
        &["name", "object", "commit", "epoch"],
        "start tag",
    )?;
    require(
        tag["name"] == admission["tag"] && tag["commit"] == admission["commit"],
        "start tag differs from admitted version and producer commit",
    )?;
    require(
        tag["object"].as_str().is_some_and(|o| is_hex(o, 40)),
        "start tag object is invalid",
    )?;
    positive(&tag["epoch"], "start tag epoch")?;
    let draft = exact(
        &record["draft"],
        &["id", "tag_name", "target_commitish"],
        "start draft",
    )?;
    positive(&draft["id"], "start draft id")?;
    require(
        draft["tag_name"] == tag["name"] && draft["target_commitish"] == tag["commit"],
        "start draft selects another tag or commit",
    )?;
    let runtime = exact(
        &record["runtime"],
        &[
            "lock_sha256",
            "receipt_sha256",
            "interface_sha256",
            "executable_sha256",
        ],
        "start runtime",
    )?;
    for (key, hash) in runtime {
        digest(hash, &format!("start runtime {key}"))?;
    }
    let locked = admission["inputs"].get(RUNTIME_LOCK_INPUT);
    if locked != Some(&runtime["lock_sha256"]) {
        return fail("start runtime lock differs from admitted input bytes");
    }
    let _ = string(tag, "name", "start tag")?;
    Ok(value.clone())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    use crate::dispatcher::context::fixtures::{source, COMMIT};

    pub fn hex(label: &str) -> String {
        crate::dispatcher::json::hex(&Sha256::digest(label.as_bytes()))
    }

    pub fn record() -> Value {
        let mut inputs = serde_json::Map::new();
        for path in super::INPUT_PATHS {
            inputs.insert(path.to_owned(), hex(path).into());
        }
        let lock = hex(super::RUNTIME_LOCK_INPUT);
        json!({
            "schema": super::SCHEMA, "assurance": super::ASSURANCE,
            "context": source(), "event": "workflow_run",
            "job": {"id": 11, "check_run_id": 12, "name": super::JOB},
            "admission": {
                "schema": "aros-toolchains-release-admission-v2", "assurance": "metadata-only",
                "repository": "metaneutrons/aros-toolchains", "commit": COMMIT,
                "version": "2.0.0", "tag": "v2.0.0", "pull_request": 7,
                "runs": {"producer_contracts": {"id": 9, "attempt": 1}},
                "governance": {"ruleset_id": 5, "policy_sha256": hex("policy"), "checked_at": 1_700_000_000},
                "inputs": inputs,
                "release_absence": {
                    "candidate_tag_ref": "absent", "visible_release": "absent",
                    "private_draft_absence": "unproven",
                    "required_start_recheck": {
                        "method": "GET",
                        "endpoint": "repos/metaneutrons/aros-toolchains/releases/tags/v2.0.0",
                        "permission": "contents:write", "absence_status": 404}}},
            "tag": {"name": "v2.0.0", "object": "a".repeat(40), "commit": COMMIT, "epoch": 1_700_000_100},
            "draft": {"id": 31, "tag_name": "v2.0.0", "target_commitish": COMMIT},
            "runtime": {"lock_sha256": lock, "receipt_sha256": hex("receipt"),
                        "interface_sha256": hex("interface"), "executable_sha256": hex("exe")},
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::fixtures::{hex, record};
    use super::*;
    use crate::dispatcher::context::fixtures::source;

    fn rejects(mutate: impl FnOnce(&mut Value)) {
        let mut value = record();
        mutate(&mut value);
        assert!(validate(&value, &source()).is_err(), "{value}");
    }

    #[test]
    fn accepts_the_closed_record() {
        assert_eq!(validate(&record(), &source()).unwrap(), record());
    }

    #[test]
    fn counterprobes_every_join() {
        rejects(|v| v["extra"] = 1.into());
        rejects(|v| v["schema"] = "other".into());
        rejects(|v| v["assurance"] = "native".into());
        rejects(|v| v["event"] = "push".into());
        rejects(|v| v["context"]["run_id"] = 1001.into());
        rejects(|v| v["job"]["name"] = "x".into());
        rejects(|v| v["job"]["id"] = 0.into());
        rejects(|v| v["tag"]["name"] = "v2.0.1".into());
        rejects(|v| v["tag"]["commit"] = "b".repeat(40).into());
        rejects(|v| v["tag"]["object"] = "A".repeat(40).into());
        rejects(|v| v["tag"]["epoch"] = 0.into());
        rejects(|v| v["draft"]["tag_name"] = "v9.9.9".into());
        rejects(|v| v["draft"]["target_commitish"] = "c".repeat(40).into());
        rejects(|v| v["runtime"]["lock_sha256"] = hex("other").into());
        rejects(|v| v["runtime"]["executable_sha256"] = "XYZ".into());
        rejects(|v| v["admission"]["commit"] = "d".repeat(40).into());
        rejects(|v| v["admission"]["version"] = "02.0.0".into());
        rejects(|v| v["admission"]["assurance"] = "full".into());
        rejects(|v| v["admission"]["release_absence"]["private_draft_absence"] = "absent".into());
        rejects(|v| {
            v["admission"]["release_absence"]["required_start_recheck"]["absence_status"] =
                403.into();
        });
        rejects(|v| {
            v["admission"]["inputs"]
                .as_object_mut()
                .unwrap()
                .remove("version.txt");
        });
        rejects(|v| v["admission"]["inputs"]["version.txt"] = "short".into());
    }

    #[test]
    fn rejects_a_foreign_or_rerun_source_context() {
        for (key, value) in [
            ("workflow", json!(".github/workflows/other.yml")),
            ("source_ref", json!("refs/heads/dev")),
            ("run_attempt", json!(2)),
            ("repository", json!("evil/aros-toolchains")),
        ] {
            let mut context = source();
            context[key] = value;
            assert!(validate(&record(), &context).is_err(), "{key}");
        }
    }
}
