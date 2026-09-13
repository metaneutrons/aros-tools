//! Compatibility adapter from verifier reference expansions to the GenMF cache.

#[cfg(test)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(test)]
use std::time::SystemTime;

use crate::genmf_cache::{materialize, GenmfCacheRequest};
#[cfg(test)]
use aros_common::read_source;
use aros_common::CancellationToken;

#[derive(Debug)]
pub struct ExpansionResult {
    pub expanded: Vec<(String, PathBuf)>,
    pub failures: Vec<ExpansionFailure>,
}

#[derive(Debug)]
pub struct ExpansionFailure {
    pub file: String,
    pub message: String,
    pub timed_out: bool,
    pub timeout_ms: Option<u64>,
}

/// Materialize exact immutable GenMF expansions for verifier comparison.
///
/// The legacy flat mtime cache is deliberately neither read nor repaired. The
/// cache root owns only the versioned content-addressed `genmf/v1` namespace.
pub fn expand_all(root: &Path, cache: &Path, refresh: bool, timeout: Duration) -> ExpansionResult {
    let python = match which::which("python3") {
        Ok(path) => path,
        Err(error) => {
            return ExpansionResult {
                expanded: Vec::new(),
                failures: vec![ExpansionFailure {
                    file: "<python3>".to_owned(),
                    message: format!("cannot resolve the required python3 interpreter: {error}"),
                    timed_out: false,
                    timeout_ms: None,
                }],
            }
        }
    };
    let request = GenmfCacheRequest {
        source_dir: root.to_path_buf(),
        cache_dir: cache.to_path_buf(),
        python,
        timeout,
    };
    let result = match materialize(&request, refresh, &CancellationToken::default()) {
        Ok(result) => result,
        Err(error) => {
            return ExpansionResult {
                expanded: Vec::new(),
                failures: vec![ExpansionFailure {
                    file: "<genmf-cache>".to_owned(),
                    message: error.to_string(),
                    timed_out: error.timed_out(),
                    timeout_ms: error
                        .timed_out()
                        .then(|| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
                }],
            }
        }
    };
    let mut expanded = Vec::new();
    let mut failures = Vec::new();
    for entry in result.entries {
        match entry.result {
            Ok(generation) => expanded.push((
                entry.selection.source_relative_path,
                generation.generation_dir.join("expansion.mk"),
            )),
            Err(error) => failures.push(ExpansionFailure {
                file: entry.selection.source_relative_path.clone(),
                message: materialization_failure_message(
                    &entry.selection.source_relative_path,
                    &error.message,
                    error.timed_out,
                    timeout,
                ),
                timed_out: error.timed_out,
                timeout_ms: error
                    .timed_out
                    .then(|| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
            }),
        }
    }
    expanded.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    failures.sort_unstable_by(|left, right| left.message.cmp(&right.message));
    failures.dedup_by(|left, right| left.message == right.message);
    ExpansionResult { expanded, failures }
}

fn materialization_failure_message(
    relative_path: &str,
    detail: &str,
    timed_out: bool,
    timeout: Duration,
) -> String {
    if timed_out {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        return format!(
            "{relative_path}: genmf timed out after {timeout_ms} ms and its process group was terminated: {detail}"
        );
    }
    format!("{relative_path}: {detail}")
}

/// Files whose contents affect every genmf expansion.
///
/// MetaMake's `genmakefiledeps` names the main template and its three current
/// includes. Discover the includes from the template itself so adding another
/// one cannot leave a previously cached reference expansion looking fresh.
#[cfg(test)]
pub fn genmf_dependency_files(root: &Path) -> Vec<PathBuf> {
    let mut dependencies = BTreeSet::from([root.join("tools/genmf/genmf.py")]);
    let mut pending = vec![root.join("config/make.tmpl")];

    while let Some(template) = pending.pop() {
        if !dependencies.insert(template.clone()) {
            continue;
        }
        let Ok(text) = read_source(&template) else {
            continue;
        };
        let parent = template.parent().unwrap_or(root);
        for line in text.lines() {
            let Some(raw_include) = line.strip_prefix("%include") else {
                continue;
            };
            if !raw_include.chars().next().is_some_and(char::is_whitespace) {
                continue;
            }
            let mut include = raw_include.trim();
            if include.len() > 1 && include.starts_with('"') && include.ends_with('"') {
                include = &include[1..include.len() - 1];
            }
            if !include.is_empty() {
                let include = Path::new(include);
                pending.push(if include.is_absolute() {
                    include.to_path_buf()
                } else {
                    parent.join(include)
                });
            }
        }
    }

    dependencies.into_iter().collect()
}

#[cfg(test)]
pub fn timestamps_are_fresh(output: SystemTime, inputs: &[SystemTime]) -> bool {
    inputs.iter().all(|input| output > *input)
}
