//! `select`: bounded start-archive bytes for an independently observed start
//! (`select-compiler-sdk-start-v2.py`).
//!
//! Read-only provider observations only. The command does not verify the start
//! signature, establish native qualification or grant upload, dispatch or
//! publication authority: those stay separate gates (`verify-claim`,
//! `verify-binding`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::claim::sha256_hex;
use super::context::{self, ContextExt};
use super::files;
use super::json::{canonical_line, is_hex, parse, positive, provider_id};
use super::observe::{authenticate_dispatcher, validate_trigger};
use super::provider::Provider;
use super::start::WORKFLOW as START_WORKFLOW;
use super::zip_reader::{read_start_archive, BUNDLE_BASENAME, MAX_ARCHIVE, RECORD_BASENAME};
use super::{
    fail, integrity, require, require_integrity, Result, MAIN_REF, MAX_EVENT_BYTES, MAX_PAGES,
    PAGE_SIZE, REPOSITORY,
};

pub const ASSURANCE: &str = "transport-selection-only-not-signature-or-native-authority";
pub const ARTIFACT_PREFIX: &str = "compiler-sdk-release-start-v2-";
pub const SELECTION_NAME: &str = "compiler-sdk-start-selection-v2.json";
pub const SELECTION_SCHEMA: &str = "aros-toolchains-start-selection-v2";

/// The selected transport bytes plus the identities they were joined to.
#[derive(Debug)]
pub struct Selected {
    pub source_context: Value,
    pub dispatcher_identity: Value,
    pub artifact_selector: Value,
    pub record_bytes: Vec<u8>,
    pub bundle_bytes: Vec<u8>,
}

fn require_environment(provider: &dyn Provider) -> Result<()> {
    require(
        provider.var("GH_HOST").as_deref() == Some("github.com"),
        "start transport requires the fixed github.com host",
    )?;
    require(
        provider
            .var("GH_TOKEN")
            .is_some_and(|token| !token.trim().is_empty()),
        "start transport requires the GitHub job token",
    )?;
    require(
        provider.var("GITHUB_EVENT_NAME").as_deref() == Some("workflow_run"),
        "start transport requires the actual workflow_run event",
    )
}

fn event_file(provider: &dyn Provider) -> Result<(PathBuf, Vec<u8>, Value)> {
    let selected = provider.var("GITHUB_EVENT_PATH").unwrap_or_default();
    let path = PathBuf::from(&selected);
    require(
        !selected.is_empty() && path.is_absolute(),
        "start transport requires the runner event file",
    )?;
    let Ok(canonical) = path.canonicalize() else {
        return integrity("runner event file is unavailable");
    };
    require(
        canonical == path && !path.components().any(|c| c.as_os_str() == ".."),
        "runner event path is not canonical",
    )?;
    let raw = files::read_regular(&path, "dispatcher event", MAX_EVENT_BYTES)?;
    let value = parse(&raw, "dispatcher event", MAX_EVENT_BYTES)?;
    require(value.is_object(), "dispatcher event is not an object")?;
    Ok((path, raw, value))
}

/// Derive the source selector only from the bounded runner event.
///
/// # Errors
///
/// Rejects anything but the successful original attempt-one `main` start.
pub fn source_context_from_event(event: &Value) -> Result<Value> {
    let (repository, run) = (&event["repository"], &event["workflow_run"]);
    require(
        event["action"] == "completed" && repository.is_object() && run.is_object(),
        "runner event is not a workflow_run completion",
    )?;
    require(
        repository["full_name"] == REPOSITORY,
        "runner event repository differs from the fixed source repository",
    )?;
    let (head_sha, head) = (&run["head_sha"], &run["head_repository"]);
    require(
        repository["id"].is_u64() && run["id"].is_u64() && run["run_attempt"].is_u64(),
        "runner event identities must be typed integers",
    )?;
    let repository_id = positive(&repository["id"], "event repository ID")?;
    let run_id = positive(&run["id"], "event workflow-run ID")?;
    require(
        run["run_attempt"] == 1,
        "start source must be original attempt one",
    )?;
    require(
        head_sha.as_str().is_some_and(|sha| is_hex(sha, 40)),
        "runner event source commit is not an exact SHA",
    )?;
    require(
        head["id"].is_u64(),
        "runner event head repository identity must be a typed integer",
    )?;
    require(
        positive(&head["id"], "event head repository ID")? == repository_id
            && head["full_name"] == REPOSITORY
            && run["head_branch"] == "main"
            && run["event"] == "workflow_run"
            && run["status"] == "completed"
            && run["conclusion"] == "success",
        "runner event does not select the successful original main start",
    )?;
    context::validate(&json!({
        "repository": REPOSITORY, "repository_id": repository_id,
        "producer_commit": head_sha, "source_ref": MAIN_REF, "workflow": START_WORKFLOW,
        "run_id": run_id, "run_attempt": 1,
    }))
}

/// Validate one provider artifact row against the source context.
///
/// # Errors
///
/// Rejects any shape, identity, size, name, expiry, digest or run difference.
pub fn validate_artifact_metadata(
    value: &Value,
    context: &Value,
    expected_name: &str,
) -> Result<Value> {
    require(
        value.is_object(),
        "start artifact metadata is not an object",
    )?;
    require(
        value["id"].is_u64() && value["size_in_bytes"].is_u64(),
        "start artifact ID and size must be typed integers",
    )?;
    let id = positive(&value["id"], "start artifact ID")?;
    provider_id(&value["id"], "start artifact ID")?;
    let size = value["size_in_bytes"].as_u64().unwrap_or_default();
    require(
        0 < size && size <= MAX_ARCHIVE as u64,
        "start artifact size is empty or exceeds the archive limit",
    )?;
    require(
        value["name"] == expected_name && value["expired"] == false,
        "start artifact name or expiry state differs",
    )?;
    let digest = value["digest"]
        .as_str()
        .and_then(|text| text.strip_prefix("sha256:"))
        .filter(|hex| is_hex(hex, 64));
    let Some(digest) = digest else {
        return fail("start artifact digest is not canonical SHA-256");
    };
    let run = &value["workflow_run"];
    require(
        run.is_object(),
        "start artifact has no workflow-run identity",
    )?;
    for key in ["id", "repository_id", "head_repository_id"] {
        require(
            run[key].is_u64(),
            "start artifact workflow identities must be typed integers",
        )?;
    }
    require(
        run["head_sha"].is_string(),
        "start artifact workflow commit must be a string",
    )?;
    require(
        run["id"].as_u64() == Some(context.id("run_id"))
            && run["head_sha"].as_str() == Some(context.text("producer_commit"))
            && run["repository_id"].as_u64() == Some(context.id("repository_id"))
            && run["head_repository_id"].as_u64() == Some(context.id("repository_id")),
        "start artifact workflow run or repository differs from the source",
    )?;
    for key in ["id", "repository_id", "head_repository_id"] {
        provider_id(&run[key], "start artifact workflow identity")?;
    }
    Ok(json!({"id": id, "name": expected_name, "sha256": digest, "size_in_bytes": size}))
}

/// Complete, paginated artifact list of the source run.
fn list_source_artifacts(provider: &dyn Provider, context: &Value) -> Result<Vec<Value>> {
    let endpoint = format!(
        "repos/{}/actions/runs/{}/artifacts?per_page=100&page=",
        context.text("repository"),
        context.id("run_id")
    );
    let mut rows: Vec<Value> = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    let mut total_count: Option<usize> = None;
    let mut pages = 0;
    for page in 1..=MAX_PAGES {
        let value = provider.get(&format!("{endpoint}{page}"))?;
        let (Some(listed), true) = (value["artifacts"].as_array(), value["total_count"].is_u64())
        else {
            return integrity("start artifact page has an invalid shape");
        };
        let page_total = value["total_count"].as_u64().unwrap_or_default() as usize;
        require_integrity(
            page_total <= MAX_PAGES * PAGE_SIZE,
            "start artifact count exceeds the bounded page limit",
        )?;
        let total = *total_count.get_or_insert(page_total);
        require_integrity(
            page_total == total,
            "start artifact total count changed during pagination",
        )?;
        require_integrity(
            listed.len() <= PAGE_SIZE,
            "start artifact page exceeds its row limit",
        )?;
        let expected_rows = PAGE_SIZE.min(total.saturating_sub(rows.len()));
        require_integrity(
            listed.len() == expected_rows,
            "start artifact pages are short, truncated, or exceed total_count",
        )?;
        for artifact in listed {
            require_integrity(
                artifact.is_object(),
                "start artifact page contains a non-object row",
            )?;
            require_integrity(
                artifact["id"].is_u64(),
                "start artifact page ID must be a typed integer",
            )?;
            let id = positive(&artifact["id"], "start artifact page ID")?;
            require_integrity(
                identities.insert(id),
                "start artifact pages contain a duplicate ID",
            )?;
            rows.push(artifact.clone());
        }
        pages = page;
        if total == 0 || rows.len() == total {
            let expected_pages = total.div_ceil(PAGE_SIZE).max(1);
            require_integrity(
                pages == expected_pages,
                "start artifact pagination ended before the complete list",
            )?;
            return Ok(rows);
        }
    }
    let _ = pages;
    integrity("start artifact list exceeds the bounded page limit")
}

fn select_from_list(rows: &[Value], context: &Value) -> Result<Value> {
    let name = format!("{ARTIFACT_PREFIX}{}-1", context.id("run_id"));
    let selected: Vec<&Value> = rows
        .iter()
        .filter(|row| row["name"] == name.as_str())
        .collect();
    require_integrity(
        selected.len() == 1,
        "source run does not contain exactly one original start artifact",
    )?;
    validate_artifact_metadata(selected[0], context, &name)
}

fn download(provider: &dyn Provider, artifact: &Value) -> Result<Vec<u8>> {
    let id = artifact["id"].as_u64().unwrap_or_default();
    let size = artifact["size_in_bytes"].as_u64().unwrap_or_default() as usize;
    let raw = provider.download_artifact(id, size)?;
    require_integrity(
        raw.len() == size,
        "downloaded start artifact size differs from provider metadata",
    )?;
    require_integrity(
        Some(sha256_hex(&raw).as_str()) == artifact["sha256"].as_str(),
        "downloaded start artifact digest differs from provider metadata",
    )?;
    Ok(raw)
}

/// Select the record and bundle bytes for the independently observed start.
///
/// # Errors
///
/// Rejects on any provider, event, identity, pagination, size, digest, ZIP or
/// consistency failure; every selector surface is reobserved after the one
/// download and must be unchanged.
pub fn select_start_transport(provider: &dyn Provider) -> Result<Selected> {
    require_environment(provider)?;
    let (event_path, event_raw, event) = event_file(provider)?;
    let source = source_context_from_event(&event)?;
    let dispatcher_context =
        context::from_environment(&|name| provider.var(name), super::claim::WORKFLOW)?;
    validate_trigger(&event, &source, &dispatcher_context)?;
    let identity = authenticate_dispatcher(provider, &source, &event_path)?;
    require_integrity(
        identity["context"] == dispatcher_context && identity["trigger_context"] == source,
        "authenticated dispatcher identity differs from the executing context",
    )?;
    let unchanged = |label: &str| -> Result<()> {
        require_integrity(
            files::read_regular(&event_path, "dispatcher event", MAX_EVENT_BYTES)? == event_raw,
            label,
        )
    };
    unchanged("runner event changed during dispatcher authentication")?;

    let expected_name = format!("{ARTIFACT_PREFIX}{}-1", source.id("run_id"));
    let listed_rows = list_source_artifacts(provider, &source)?;
    let listed = select_from_list(&listed_rows, &source)?;
    let direct_path = format!(
        "repos/{}/actions/artifacts/{}",
        source.text("repository"),
        listed["id"]
    );
    let direct = validate_artifact_metadata(&provider.get(&direct_path)?, &source, &expected_name)?;
    require_integrity(
        direct == listed,
        "direct start artifact metadata differs from the complete run listing",
    )?;

    let archive = download(provider, &listed)?;
    let members = read_start_archive(
        &archive,
        listed["sha256"].as_str().unwrap_or_default(),
        listed["size_in_bytes"].as_u64().unwrap_or_default() as usize,
    )?;

    require_integrity(
        list_source_artifacts(provider, &source)? == listed_rows,
        "complete start artifact metadata changed during transport",
    )?;
    require_integrity(
        select_from_list(&listed_rows, &source)? == listed,
        "complete start artifact listing changed during transport",
    )?;
    require_integrity(
        validate_artifact_metadata(&provider.get(&direct_path)?, &source, &expected_name)?
            == listed,
        "direct start artifact metadata changed during transport",
    )?;
    require_integrity(
        authenticate_dispatcher(provider, &source, &event_path)? == identity,
        "authenticated dispatcher identity changed during transport",
    )?;
    unchanged("runner event bytes changed during start transport selection")?;

    Ok(Selected {
        source_context: source,
        dispatcher_identity: identity,
        artifact_selector: listed,
        record_bytes: members.record,
        bundle_bytes: members.bundle,
    })
}

/// Output file names inside the private directory of `select`.
#[must_use]
pub fn output_paths(directory: &Path) -> [PathBuf; 3] {
    [
        directory.join(RECORD_BASENAME),
        directory.join(BUNDLE_BASENAME),
        directory.join(SELECTION_NAME),
    ]
}

/// Retain a selection as three exclusive private files.
///
/// # Errors
///
/// Rejects an occupied destination, a non-private parent or a readback
/// difference. Partial files from a failed run are unusable.
pub fn retain(directory: &Path, selected: &Selected) -> Result<()> {
    let [record, bundle, selection] = output_paths(directory);
    for path in [&record, &bundle, &selection] {
        files::require_absent(path)?;
        files::require_private_parent(path)?;
    }
    let report = json!({
        "schema": SELECTION_SCHEMA, "assurance": ASSURANCE,
        "source_context": selected.source_context,
        "dispatcher_identity": selected.dispatcher_identity,
        "artifact_selector": selected.artifact_selector,
        "record_sha256": sha256_hex(&selected.record_bytes),
        "bundle_sha256": sha256_hex(&selected.bundle_bytes),
        "limitations": [
            "the start signature is not verified",
            "no native qualification or release authority is established",
            "the source context must be re-selected independently by verify-claim",
        ],
    });
    files::write_exclusive(&record, &selected.record_bytes, "retained start record")?;
    files::write_exclusive(&bundle, &selected.bundle_bytes, "retained start bundle")?;
    files::write_exclusive(
        &selection,
        &canonical_line(&report),
        "retained start selection",
    )
}
