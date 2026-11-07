//! What a native build phase leaves behind: its persisted logs and, when it
//! fails, the summary the person reading the job log needs first.

use std::fs::OpenOptions;
use std::path::Path;

use aros_common::{sha256_file, Sha256Digest};

use crate::ContractError;

/// How many trailing stderr lines a failed phase quotes in its diagnostic.
const FAILURE_TAIL_LINES: usize = 8;
/// Longest quoted stderr line, so one runaway line cannot flood a CI log.
const FAILURE_TAIL_LINE_CHARS: usize = 240;

/// Facts that decide most build failures and disappear with the runner: where
/// the full logs are, how much disk was left, and what the phase said last.
///
/// The retained logs stay the evidence of record. This only makes the first
/// look at a failed job conclusive, because a wrapped cause (a full disk
/// behind "failed to unpack") otherwise exists only in an artifact.
pub fn evidence(
    logs: &Path,
    log_name: &str,
    directories: &[(&str, &Path)],
    output: &aros_common::ProcessOutput,
) -> String {
    let mut parts = vec![format!(
        "logs: {}/{log_name}.stdout.log and {log_name}.stderr.log",
        logs.display()
    )];
    for (label, path) in directories {
        match fs2::available_space(path) {
            Ok(bytes) => parts.push(format!("free space {label}: {} MiB", bytes >> 20)),
            Err(error) => parts.push(format!("free space {label}: unavailable ({error})")),
        }
    }
    let rendered = output.stderr.rendered_lossy();
    let tail = rendered
        .lines()
        .map(|line| {
            line.chars()
                .filter(|character| !character.is_control())
                .take(FAILURE_TAIL_LINE_CHARS)
                .collect::<String>()
        })
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if tail.is_empty() {
        parts.push("stderr was empty".to_owned());
    } else {
        let start = tail.len().saturating_sub(FAILURE_TAIL_LINES);
        parts.push(format!("stderr tail: {}", tail[start..].join(" | ")));
    }
    parts.join("; ")
}

pub fn persist_process_output(
    logs: &Path,
    log_name: &str,
    output: &aros_common::ProcessOutput,
) -> Result<(Sha256Digest, Sha256Digest), ContractError> {
    let stdout_path = logs.join(format!("{log_name}.stdout.log"));
    let stderr_path = logs.join(format!("{log_name}.stderr.log"));
    let stdout = create_log(&stdout_path, &output.stdout)?;
    let stderr = create_log(&stderr_path, &output.stderr)?;
    Ok((stdout, stderr))
}

fn create_log(
    path: &Path,
    stream: &aros_common::CapturedStream,
) -> Result<Sha256Digest, ContractError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ContractError::state("cannot create native phase log"))?;
    stream
        .write_rendered(&mut file)
        .and_then(|()| file.sync_all())
        .map_err(|_| ContractError::state("cannot durably persist native phase log"))?;
    sha256_file(path)
        .map(|measured| measured.digest)
        .map_err(|_| ContractError::state("cannot hash native phase log"))
}
