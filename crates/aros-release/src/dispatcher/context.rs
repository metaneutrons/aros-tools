//! Provider run contexts (`artifact-origin.py::validate_context`).

use serde_json::Value;

use super::json::{exact, is_hex, provider_id, string};
use super::{fail, require, Result};

const KEYS: [&str; 7] = [
    "repository",
    "repository_id",
    "producer_commit",
    "source_ref",
    "workflow",
    "run_id",
    "run_attempt",
];

fn name_chars(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A syntactically valid `owner/name` repository.
#[must_use]
pub fn is_repository(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, name)| name_chars(owner) && name_chars(name))
}

fn is_source_ref(value: &str) -> bool {
    ["refs/heads/", "refs/tags/"].iter().any(|prefix| {
        value.strip_prefix(prefix).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
        })
    })
}

fn is_workflow(value: &str) -> bool {
    value
        .strip_prefix(".github/workflows/")
        .and_then(|rest| rest.strip_suffix(".yml"))
        .is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
}

/// Validate one context object and return it unchanged.
///
/// # Errors
///
/// Fails for an unexpected field set, an invalid selector or a non-integer
/// provider identity.
pub fn validate(value: &Value) -> Result<Value> {
    let ctx = exact(value, &KEYS, "context")?;
    let repository = string(ctx, "repository", "context")?;
    let commit = string(ctx, "producer_commit", "context")?;
    let reference = string(ctx, "source_ref", "context")?;
    let workflow = string(ctx, "workflow", "context")?;
    require(is_repository(repository), "repository is invalid")?;
    require(is_hex(commit, 40), "producer commit is invalid")?;
    require(is_source_ref(reference), "source ref is invalid")?;
    require(is_workflow(workflow), "workflow path is invalid")?;
    for key in ["repository_id", "run_id", "run_attempt"] {
        if !ctx[key].is_u64() {
            return fail("selected provider identity is not an integer");
        }
        provider_id(&ctx[key], "selected provider identity")?;
    }
    Ok(value.clone())
}

/// Build a context from the runner environment (`context_from_environment`).
///
/// # Errors
///
/// Fails when a variable is absent or not a canonical identity.
pub fn from_environment(
    environment: &dyn Fn(&str) -> Option<String>,
    workflow: &str,
) -> Result<Value> {
    let number = |name: &str| -> Result<u64> {
        let text = environment(name).unwrap_or_default();
        let canonical = !text.is_empty()
            && text.len() <= 16
            && text.bytes().all(|b| b.is_ascii_digit())
            && !text.starts_with('0');
        require(canonical, "provider identity is not canonical")?;
        let value: u64 = text.parse().unwrap_or(0);
        require(
            (1..=9_007_199_254_740_991).contains(&value),
            "provider identity exceeds the exact integer limit",
        )?;
        Ok(value)
    };
    validate(&serde_json::json!({
        "repository": environment("GITHUB_REPOSITORY").unwrap_or_default(),
        "repository_id": number("GITHUB_REPOSITORY_ID")?,
        "producer_commit": environment("GITHUB_SHA").unwrap_or_default(),
        "source_ref": environment("GITHUB_REF").unwrap_or_default(),
        "workflow": workflow,
        "run_id": number("GITHUB_RUN_ID")?,
        "run_attempt": number("GITHUB_RUN_ATTEMPT")?,
    }))
}

/// Typed accessors on an already validated context.
pub trait ContextExt {
    fn text(&self, key: &str) -> &str;
    fn id(&self, key: &str) -> u64;
}

impl ContextExt for Value {
    fn text(&self, key: &str) -> &str {
        self[key].as_str().unwrap_or_default()
    }

    fn id(&self, key: &str) -> u64 {
        self[key].as_u64().unwrap_or_default()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{json, Value};

    pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    pub fn source() -> Value {
        json!({"repository": "metaneutrons/aros-toolchains", "repository_id": 4242,
               "producer_commit": COMMIT, "source_ref": "refs/heads/main",
               "workflow": ".github/workflows/compiler-sdk-release-start.yml",
               "run_id": 1000, "run_attempt": 1})
    }

    pub fn dispatcher() -> Value {
        json!({"repository": "metaneutrons/aros-toolchains", "repository_id": 4242,
               "producer_commit": COMMIT, "source_ref": "refs/heads/main",
               "workflow": ".github/workflows/compiler-sdk-release-dispatch.yml",
               "run_id": 2000, "run_attempt": 1})
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::source;
    use super::*;

    #[test]
    fn accepts_the_fixed_shape_and_rejects_every_deviation() {
        assert_eq!(validate(&source()).unwrap(), source());
        let mutate = |key: &str, value: Value| {
            let mut context = source();
            context[key] = value;
            validate(&context)
        };
        assert!(mutate("repository", "no-slash".into()).is_err());
        assert!(mutate("repository", "a/b/c".into()).is_err());
        assert!(mutate("producer_commit", "ABCDEF".into()).is_err());
        assert!(mutate("source_ref", "refs/pull/1/head".into()).is_err());
        assert!(mutate("workflow", ".github/workflows/x.yaml".into()).is_err());
        assert!(mutate("repository_id", "4242".into()).is_err());
        assert!(mutate("run_id", Value::Bool(true)).is_err());
        assert!(mutate("run_attempt", 0.into()).is_err());
        let mut extra = source();
        extra["extra"] = 1.into();
        assert!(validate(&extra).is_err());
        let mut missing = source();
        missing.as_object_mut().unwrap().remove("run_id");
        assert!(validate(&missing).is_err());
    }

    #[test]
    fn environment_identities_must_be_canonical_decimals() {
        let build = |run: &'static str| {
            from_environment(
                &move |name| {
                    Some(
                        match name {
                            "GITHUB_REPOSITORY" => "metaneutrons/aros-toolchains",
                            "GITHUB_REPOSITORY_ID" => "4242",
                            "GITHUB_SHA" => "0123456789abcdef0123456789abcdef01234567",
                            "GITHUB_REF" => "refs/heads/main",
                            "GITHUB_RUN_ID" => run,
                            _ => "1",
                        }
                        .to_owned(),
                    )
                },
                ".github/workflows/compiler-sdk-release-dispatch.yml",
            )
        };
        assert!(build("2000").is_ok());
        assert!(build("02000").is_err());
        assert!(build("2000 ").is_err());
        assert!(build("").is_err());
        assert!(build("9007199254740992").is_err());
    }
}
