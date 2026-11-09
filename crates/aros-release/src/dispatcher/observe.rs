//! Shared provider observations (`artifact-origin.py` job/run joins,
//! `compiler-sdk-dispatcher-auth-v2.py`, `compiler-sdk-release-start-v2.py`).
//!
//! All reads go through [`Provider::get`], which is bound to the fixed
//! repository and to HTTP 200 JSON.

use serde_json::Value;

use super::claim::{JOB, WORKFLOW as DISPATCH_WORKFLOW};
use super::context::{self, ContextExt};
use super::json::{is_hex, positive, provider_id};
use super::provider::Provider;
use super::start::WORKFLOW as START_WORKFLOW;
use super::{
    fail, integrity, require, require_integrity, Result, MAIN_REF, MAX_PAGES, PAGE_SIZE, REPOSITORY,
};

pub const DISPATCHER_SCHEMA: &str = "aros-toolchains-dispatcher-identity-v2";
pub const DISPATCHER_ASSURANCE: &str = "dispatcher-identity-only-not-start-or-upload-authorization";
const SUPPORTED_EVENTS: [&str; 3] = ["push", "workflow_dispatch", "workflow_run"];

fn run_path(context: &Value) -> String {
    format!(
        "repos/{}/actions/runs/{}",
        context.text("repository"),
        context.id("run_id")
    )
}

/// Bind a context to the exact run attempt and return its actual event.
///
/// # Errors
///
/// Rejects any identity, commit, ref, workflow path or repository difference.
pub fn observe_run_event(provider: &dyn Provider, selected: &Value) -> Result<String> {
    let selected = context::validate(selected)?;
    let run = provider.get(&format!(
        "{}/attempts/{}",
        run_path(&selected),
        selected.id("run_attempt")
    ))?;
    require_integrity(
        run["id"].as_u64() == Some(selected.id("run_id"))
            && run["run_attempt"].as_u64() == Some(selected.id("run_attempt")),
        "provider run attempt identity differs",
    )?;
    require_integrity(
        run["head_sha"].as_str() == Some(selected.text("producer_commit")),
        "provider run commit differs",
    )?;
    let branch = selected
        .text("source_ref")
        .splitn(3, '/')
        .nth(2)
        .unwrap_or_default();
    require_integrity(
        run["head_branch"].as_str() == Some(branch),
        "provider run branch/tag differs",
    )?;
    let path = run["path"].as_str().unwrap_or_default();
    require_integrity(
        path == selected.text("workflow")
            || path
                == format!(
                    "{}@{}",
                    selected.text("workflow"),
                    selected.text("source_ref")
                ),
        "provider run workflow path differs",
    )?;
    let head = &run["head_repository"];
    require_integrity(
        head["id"].as_u64() == Some(selected.id("repository_id"))
            && head["full_name"].as_str() == Some(selected.text("repository")),
        "provider run repository differs",
    )?;
    match run["event"].as_str() {
        Some(event) if SUPPORTED_EVENTS.contains(&event) => Ok(event.to_owned()),
        _ => integrity("provider run event is unsupported"),
    }
}

/// All job pages of one attempt (`provider_json` pagination with its bounds).
///
/// # Errors
///
/// Rejects malformed, short, repeated or unbounded pagination.
pub fn jobs_for(provider: &dyn Provider, context: &Value) -> Result<Vec<Value>> {
    let base = format!(
        "{}/attempts/{}/jobs?per_page=100",
        run_path(context),
        context.id("run_attempt")
    );
    let mut pages = Vec::new();
    let mut observed = 0usize;
    for page in 1..=MAX_PAGES {
        let value = provider.get(&format!("{base}&page={page}"))?;
        let jobs = value["jobs"].as_array().map(Vec::len);
        let Some(count) = jobs.filter(|count| *count <= PAGE_SIZE) else {
            return integrity("dispatcher job page is malformed");
        };
        let total = positive(&value["total_count"], "dispatcher total jobs")? as usize;
        pages.push(value);
        observed += count;
        if observed >= total || count < PAGE_SIZE {
            complete_jobs(&pages)?;
            return Ok(pages);
        }
    }
    integrity("dispatcher jobs exceed the bounded page limit")
}

/// Merge pages and require a complete, duplicate-free job list.
///
/// # Errors
///
/// Rejects incomplete or repeating pages.
pub fn complete_jobs(pages: &[Value]) -> Result<Vec<Value>> {
    require_integrity(
        !pages.is_empty() && pages.len() <= 100,
        "job pages are invalid",
    )?;
    let mut totals = Vec::new();
    let mut jobs = Vec::new();
    for page in pages {
        let Some(list) = page["jobs"].as_array() else {
            return integrity("job page is invalid");
        };
        totals.push(provider_id(&page["total_count"], "job page total")?);
        jobs.extend(list.iter().cloned());
    }
    require_integrity(
        totals.iter().all(|t| *t == totals[0]) && jobs.len() as u64 == totals[0],
        "job pages are incomplete",
    )?;
    require_integrity(jobs.iter().all(Value::is_object), "job metadata is invalid")?;
    let mut ids = Vec::new();
    for job in &jobs {
        ids.push(provider_id(&job["id"], "job id")?);
    }
    ids.sort_unstable();
    require_integrity(
        ids.windows(2).all(|w| w[0] != w[1]),
        "job pages repeat an identity",
    )?;
    Ok(jobs)
}

/// Select the one owning job by check run (`select_job`).
///
/// # Errors
///
/// Rejects any ambiguity, identity, attempt or execution-state difference.
pub fn select_job(
    pages: &[Value],
    context: &Value,
    check_id: u64,
    name: &str,
    status: &str,
) -> Result<Value> {
    let jobs = complete_jobs(pages)?;
    let url = format!(
        "https://api.github.com/repos/{}/check-runs/{check_id}",
        context.text("repository")
    );
    let selected: Vec<&Value> = jobs
        .iter()
        .filter(|job| job["check_run_url"].as_str() == Some(&url))
        .collect();
    require_integrity(
        selected.len() == 1,
        "OIDC check ID does not select exactly one owning job",
    )?;
    let job = selected[0];
    require_integrity(
        job["name"].as_str() == Some(name)
            && job["head_sha"].as_str() == Some(context.text("producer_commit"))
            && provider_id(&job["run_id"], "job run")? == context.id("run_id"),
        "owning job identity differs",
    )?;
    if let Some(attempt) = job.get("run_attempt") {
        require_integrity(
            provider_id(attempt, "job attempt")? == context.id("run_attempt"),
            "owning job attempt differs",
        )?;
    }
    require_integrity(
        job["status"].as_str() == Some(status),
        "owning job has the wrong execution state",
    )?;
    let conclusion_ok = if status == "completed" {
        job["conclusion"].as_str() == Some("success")
    } else {
        job["conclusion"].is_null()
    };
    require_integrity(
        conclusion_ok,
        "owning job did not succeed or is not currently executing",
    )?;
    Ok(
        serde_json::json!({"id": provider_id(&job["id"], "job id")?, "check_run_id": check_id, "name": name}),
    )
}

/// Select a completed source job by id/name, then bind its check URL.
///
/// # Errors
///
/// Rejects an unknown job, a malformed check URL or any `select_job` failure.
pub fn select_source_job(pages: &[Value], context: &Value, expected: &Value) -> Result<Value> {
    let jobs = complete_jobs(pages)?;
    let id = expected["id"].as_u64().unwrap_or_default();
    let selected: Vec<&Value> = jobs
        .iter()
        .filter(|j| j["id"].as_u64() == Some(id))
        .collect();
    require_integrity(
        selected.len() == 1 && selected[0]["name"] == expected["name"],
        "source job ID/name does not select one provider job",
    )?;
    let prefix = format!(
        "https://api.github.com/repos/{}/check-runs/",
        context.text("repository")
    );
    let suffix = selected[0]["check_run_url"]
        .as_str()
        .and_then(|url| url.strip_prefix(&prefix))
        .filter(|rest| {
            !rest.is_empty() && !rest.starts_with('0') && rest.bytes().all(|b| b.is_ascii_digit())
        });
    let Some(check_id) = suffix.and_then(|rest| rest.parse::<u64>().ok()) else {
        return integrity("source job check URL is invalid");
    };
    select_job(
        pages,
        context,
        check_id,
        expected["name"].as_str().unwrap_or_default(),
        "completed",
    )
}

/// Observe the completed, successful, original start attempt.
///
/// # Errors
///
/// Rejects incomplete, unsuccessful, rerun or changed runs.
pub fn observe_start_run(provider: &dyn Provider, context: &Value) -> Result<()> {
    let run = provider.get(&format!(
        "{}/attempts/{}",
        run_path(context),
        context.id("run_attempt")
    ))?;
    require_integrity(
        run["status"] == "completed" && run["conclusion"] == "success",
        "original start attempt is incomplete or unsuccessful",
    )?;
    require_integrity(
        observe_run_event(provider, context)? == "workflow_run",
        "provider start event differs",
    )?;
    let latest = provider.get(&run_path(context))?;
    require_integrity(
        latest["id"].as_u64() == Some(context.id("run_id"))
            && latest["run_attempt"].as_u64() == Some(1)
            && latest["head_sha"].as_str() == Some(context.text("producer_commit"))
            && latest["status"] == "completed"
            && latest["conclusion"] == "success",
        "start run was rerun, changed or did not complete successfully",
    )
}

/// Observe the executing dispatcher run (still the first attempt, running).
///
/// # Errors
///
/// Rejects a reran, finished or foreign run.
pub fn observe_dispatcher(provider: &dyn Provider, context: &Value) -> Result<()> {
    require_integrity(
        observe_run_event(provider, context)? == "workflow_run",
        "dispatcher provider event differs",
    )?;
    let latest = provider.get(&run_path(context))?;
    require_integrity(
        latest["id"].as_u64() == Some(context.id("run_id"))
            && latest["run_attempt"].as_u64() == Some(1)
            && latest["head_sha"].as_str() == Some(context.text("producer_commit"))
            && latest["status"] == "in_progress"
            && latest["conclusion"].is_null(),
        "dispatcher run changed, was rerun or is not executing",
    )
}

/// Reobserve the exact annotated ref/object of the start tag.
///
/// # Errors
///
/// Rejects any ref, object, target or timestamp difference.
pub fn observe_tag(provider: &dyn Provider, tag: &Value) -> Result<()> {
    let name = tag["name"].as_str().unwrap_or_default();
    let object = tag["object"].as_str().unwrap_or_default();
    let reference = provider.get(&format!("repos/{REPOSITORY}/git/ref/tags/{name}"))?;
    require_integrity(
        reference["ref"].as_str() == Some(&format!("refs/tags/{name}"))
            && reference["object"]
                == serde_json::json!({"type": "tag", "sha": object,
                    "url": format!("https://api.github.com/repos/{REPOSITORY}/git/tags/{object}")}),
        "remote ref differs from the exact annotated start tag",
    )?;
    let annotated = provider.get(&format!("repos/{REPOSITORY}/git/tags/{object}"))?;
    require_integrity(
        annotated["sha"].as_str() == Some(object) && annotated["tag"].as_str() == Some(name),
        "remote annotated tag identity differs",
    )?;
    let target = &annotated["object"];
    require_integrity(
        target["type"] == "commit" && target["sha"] == tag["commit"],
        "remote annotated tag does not directly select the admitted commit",
    )?;
    let Some(date) = annotated["tagger"]["date"].as_str() else {
        return integrity("remote tag timestamp is absent");
    };
    let epoch = parse_utc_second(date);
    require_integrity(
        epoch.is_some() && epoch == tag["epoch"].as_i64(),
        "remote tag timestamp differs from the start or is malformed",
    )
}

/// `YYYY-MM-DDTHH:MM:SS` plus `Z` or `+HH:MM`/`-HH:MM`, exact seconds only.
fn parse_utc_second(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.is_ascii() {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = text.get(from..to)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if [bytes[4], bytes[7], bytes[10], bytes[13], bytes[16]] != *b"--T::" {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let zone = &text[19..];
    let offset = if zone == "Z" {
        0
    } else if zone.len() == 6
        && zone.as_bytes()[3] == b':'
        && matches!(zone.as_bytes()[0], b'+' | b'-')
    {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        sign * (num(20, 22)? * 3600 + num(23, 25)? * 60)
    } else {
        return None;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Check the trigger edge: the actual successful original start completion.
///
/// # Errors
///
/// Rejects any event, run or head-repository difference.
pub fn validate_trigger(event: &Value, source: &Value, dispatcher: &Value) -> Result<()> {
    require(
        event["action"] == "completed",
        "dispatcher requires the actual start-completion event",
    )?;
    let repository = &event["repository"];
    require(
        repository["id"].as_u64() == Some(dispatcher.id("repository_id"))
            && repository["id"].is_u64()
            && repository["full_name"].as_str() == Some(dispatcher.text("repository")),
        "dispatcher event repository differs",
    )?;
    let run = &event["workflow_run"];
    require(
        run["id"].as_u64() == Some(source.id("run_id"))
            && run["run_attempt"].as_u64() == Some(source.id("run_attempt"))
            && run["head_sha"].as_str() == Some(source.text("producer_commit"))
            && run["head_branch"] == "main"
            && run["event"] == "workflow_run"
            && run["status"] == "completed"
            && run["conclusion"] == "success",
        "dispatcher trigger does not select the exact successful original start",
    )?;
    let head = &run["head_repository"];
    require(
        head["id"].as_u64() == Some(source.id("repository_id"))
            && head["full_name"].as_str() == Some(source.text("repository")),
        "dispatcher trigger head repository differs",
    )
}

/// Validate OIDC claims against an expected context (`require_oidc_claims`).
///
/// # Errors
///
/// Rejects any issuer, audience, event, binding, identity or time difference.
pub fn require_oidc_claims(
    claims: &Value,
    context: &Value,
    now: i64,
    expected_event: &str,
) -> Result<u64> {
    require_integrity(
        claims["iss"] == "https://token.actions.githubusercontent.com"
            && claims["aud"] == "aros-toolchain-artifact-origin",
        "OIDC issuer or audience differs",
    )?;
    require_integrity(
        SUPPORTED_EVENTS.contains(&expected_event) && claims["event_name"] == expected_event,
        "OIDC event differs from the observed provider event",
    )?;
    let workflow_ref = format!(
        "{}/{}@{}",
        context.text("repository"),
        context.text("workflow"),
        context.text("source_ref")
    );
    let bound = [
        ("repository", context.text("repository")),
        ("sha", context.text("producer_commit")),
        ("ref", context.text("source_ref")),
        ("workflow_ref", workflow_ref.as_str()),
        ("workflow_sha", context.text("producer_commit")),
        ("runner_environment", "github-hosted"),
    ];
    for (key, expected) in bound {
        require_integrity(
            claims[key].as_str() == Some(expected),
            "OIDC workflow/source/runner binding differs",
        )?;
    }
    for key in ["repository_id", "run_id", "run_attempt"] {
        let value = oidc_identity(&claims[key]);
        require_integrity(
            value == Some(context.id(key)),
            "OIDC provider identity differs",
        )?;
    }
    if claims.get("job_workflow_ref").is_some() || claims.get("job_workflow_sha").is_some() {
        require_integrity(
            claims["job_workflow_ref"] == claims["workflow_ref"]
                && claims["job_workflow_sha"].as_str() == Some(context.text("producer_commit")),
            "unapproved job workflow identity",
        )?;
    }
    let epochs: Vec<Option<i64>> = ["iat", "nbf", "exp"]
        .iter()
        .map(|k| claims[*k].as_i64())
        .collect();
    let (Some(issued), Some(starts), Some(expires)) = (epochs[0], epochs[1], epochs[2]) else {
        return integrity("OIDC token times are invalid");
    };
    require_integrity(
        0 < issued
            && issued <= now + 30
            && 0 < starts
            && starts <= now + 30
            && now + 30 < expires
            && issued <= expires
            && expires <= issued + 3600,
        "OIDC token is expired or outside its time bound",
    )?;
    oidc_identity(&claims["check_run_id"])
        .map_or_else(|| integrity("OIDC check run identity is invalid"), Ok)
}

/// Provider identity as a canonical integer or canonical decimal string.
fn oidc_identity(value: &Value) -> Option<u64> {
    let text = match value {
        Value::Number(number) if number.is_u64() => number.to_string(),
        Value::String(text) => text.clone(),
        _ => return None,
    };
    let canonical = !text.is_empty()
        && text.len() <= 16
        && !text.starts_with('0')
        && text.bytes().all(|b| b.is_ascii_digit());
    let number: u64 = text.parse().ok().filter(|_| canonical)?;
    (number <= 9_007_199_254_740_991).then_some(number)
}

/// Authenticate the current dispatcher identity and triggering edge.
///
/// Returns the identity record `{schema, assurance, context, job, event,
/// trigger_context}`; it is not start, upload or dispatch authority.
///
/// # Errors
///
/// Rejects anything but a distinct original main-based start-completion reader
/// whose provider run, event file and owning job are consistent before and
/// after the observation.
pub fn authenticate_dispatcher(
    provider: &dyn Provider,
    source_context: &Value,
    event_path: &std::path::Path,
) -> Result<Value> {
    let source = context::validate(source_context)?;
    let env = |name: &str| provider.var(name);
    let context = context::from_environment(&env, DISPATCH_WORKFLOW)?;
    require(
        source.text("repository") == REPOSITORY
            && source.text("source_ref") == MAIN_REF
            && source.text("workflow") == START_WORKFLOW
            && source.id("run_attempt") == 1
            && context.text("repository") == source.text("repository")
            && context.id("repository_id") == source.id("repository_id")
            && context.text("source_ref") == MAIN_REF
            && context.id("run_attempt") == 1
            && context.id("run_id") != source.id("run_id")
            && provider.var("GITHUB_EVENT_NAME").as_deref() == Some("workflow_run"),
        "dispatcher is not a distinct original main-based start-completion reader",
    )?;
    let raw = super::files::read_regular(event_path, "dispatcher event", super::MAX_EVENT_BYTES)?;
    validate_trigger(
        &super::json::parse(&raw, "dispatcher event", super::MAX_EVENT_BYTES)?,
        &source,
        &context,
    )?;
    observe_dispatcher(provider, &context)?;
    observe_start_run(provider, &source)?;
    let check_id = provider.oidc_check_id(&context, "workflow_run")?;
    let job = select_job(
        &jobs_for(provider, &context)?,
        &context,
        check_id,
        JOB,
        "in_progress",
    )?;
    observe_dispatcher(provider, &context)?;
    observe_start_run(provider, &source)?;
    require_integrity(
        select_job(
            &jobs_for(provider, &context)?,
            &context,
            check_id,
            JOB,
            "in_progress",
        )? == job,
        "dispatcher owning job changed during authentication",
    )?;
    require_integrity(
        super::files::read_regular(event_path, "dispatcher event", super::MAX_EVENT_BYTES)? == raw,
        "dispatcher event changed during authentication",
    )?;
    Ok(serde_json::json!({
        "schema": DISPATCHER_SCHEMA, "assurance": DISPATCHER_ASSURANCE,
        "context": context, "job": job, "event": "workflow_run", "trigger_context": source,
    }))
}

/// Compare a commit-pinned input byte-for-byte with its admitted digest.
///
/// # Errors
///
/// Rejects any digest difference.
pub fn require_frozen_input(
    provider: &dyn Provider,
    commit: &str,
    path: &str,
    expected: &str,
) -> Result<()> {
    require(
        is_hex(commit, 40) && is_hex(expected, 64),
        "frozen input selection is malformed",
    )?;
    let bytes = provider.committed_file(commit, path, super::MAX_API_BYTES)?;
    if super::claim::sha256_hex(&bytes) != expected {
        return fail("frozen start input bytes differ from the signed admission");
    }
    Ok(())
}

/// The fixed repository exists, is active and has `main` as default branch.
///
/// # Errors
///
/// Rejects any identity, archive, disabled or default-branch difference.
pub fn require_active_repository(provider: &dyn Provider) -> Result<()> {
    let repository = provider.get(&format!("repos/{REPOSITORY}"))?;
    require_integrity(
        repository["full_name"] == REPOSITORY
            && repository["archived"] == false
            && repository["disabled"] == false
            && repository["default_branch"] == "main",
        "provider repository identity, flags or default branch differ",
    )
}
