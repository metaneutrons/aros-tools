//! Bounded GenMF expansion, dependency discovery, and cache freshness.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use aros_common::read_source;
use rayon::prelude::*;

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

/// Runs genmf over each mmakefile, caching the result.
///
/// genmf is quick (about 20 ms per file) but there are over a thousand files,
/// so the expansions are kept and only redone on request.
pub fn expand_all(
    root: &Path,
    cache: &Path,
    files: &[PathBuf],
    refresh: bool,
    timeout: Duration,
) -> ExpansionResult {
    let tmpl = root.join("config/make.tmpl");
    let genmf = root.join("tools/genmf/genmf.py");
    let genmf_dependencies = genmf_dependency_files(root);

    let outcomes: Vec<std::result::Result<(String, PathBuf), ExpansionFailure>> = files
        .par_iter()
        .map(|f| {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(f)
                .to_string_lossy()
                .to_string();
            let out = cache.join(format!("{}.mk", rel.replace('/', "%")));
            let failure = |detail: String| ExpansionFailure {
                file: rel.clone(),
                message: format!("{rel}: {detail}"),
                timed_out: false,
                timeout_ms: None,
            };
            let mut inputs = Vec::with_capacity(genmf_dependencies.len() + 1);
            inputs.push(f.as_path());
            inputs.extend(genmf_dependencies.iter().map(PathBuf::as_path));
            if refresh || !cache_is_fresh(&out, &inputs) {
                // Never let a failed regeneration make a stale or partial
                // output look fresh on the next run.
                let _ = fs::remove_file(&out);
                let mut command = Command::new("python3");
                command.arg(&genmf).arg(&tmpl).arg(f).arg(&out);
                let result = aros_common::run_output_with_timeout(
                    &mut command,
                    aros_common::DEFAULT_CAPTURE_LIMIT,
                    timeout,
                );

                let command_output =
                    result.map_err(|error| failure(format!("could not start genmf: {error}")))?;
                if command_output.timed_out {
                    let _ = fs::remove_file(&out);
                    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
                    return Err(ExpansionFailure {
                        file: rel.clone(),
                        message: format!(
                            "{rel}: genmf timed out after {timeout_ms} ms and its process group was terminated"
                        ),
                        timed_out: true,
                        timeout_ms: Some(timeout_ms),
                    });
                }
                if !command_output.status.success() {
                    let _ = fs::remove_file(&out);
                    let detail = aros_common::bounded_output_detail(
                        &command_output.stdout,
                        &command_output.stderr,
                    )
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                    let detail = if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    };
                    return Err(failure(format!(
                        "genmf exited with {}{detail}",
                        command_output.status
                    )));
                }
                if !out.is_file() {
                    return Err(failure(
                        "genmf succeeded without producing cache output".to_owned(),
                    ));
                }
            }
            Ok((rel, out))
        })
        .collect();

    let mut expanded = Vec::new();
    let mut failures = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(expansion) => expanded.push(expansion),
            Err(failure) => failures.push(failure),
        }
    }
    expanded.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    failures.sort_unstable_by(|left, right| left.message.cmp(&right.message));
    failures.dedup_by(|left, right| left.message == right.message);
    ExpansionResult { expanded, failures }
}

/// Files whose contents affect every genmf expansion.
///
/// MetaMake's `genmakefiledeps` names the main template and its three current
/// includes. Discover the includes from the template itself so adding another
/// one cannot leave a previously cached reference expansion looking fresh.
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

fn cache_is_fresh(output: &Path, inputs: &[&Path]) -> bool {
    let Ok(output_modified) = fs::metadata(output).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let mut input_modified = Vec::with_capacity(inputs.len());
    for input in inputs {
        let Ok(modified) = fs::metadata(input).and_then(|metadata| metadata.modified()) else {
            return false;
        };
        input_modified.push(modified);
    }
    timestamps_are_fresh(output_modified, &input_modified)
}

pub fn timestamps_are_fresh(output: SystemTime, inputs: &[SystemTime]) -> bool {
    inputs.iter().all(|input| output > *input)
}
