//! Shared subprocess execution primitives and bounded output rendering.
//!
//! Domain crates remain responsible for deciding whether a status is an error
//! and for assigning stable diagnostic codes. This module owns the mechanics:
//! program identity, elapsed time, exit status, captured bytes, and one bounded
//! representation of child output.

use std::io;
use std::process::{Command, ExitStatus, Output};
use std::time::{Duration, Instant};

/// Completed child process whose standard streams were inherited or redirected
/// by the caller.
#[derive(Debug)]
pub struct ProcessStatus {
    /// Executed program as configured on [`Command`].
    pub tool: String,
    /// Wall-clock time spent waiting for the child.
    pub elapsed: Duration,
    /// Operating-system exit status.
    pub status: ExitStatus,
}

/// Completed child process with captured standard output and error streams.
#[derive(Debug)]
pub struct ProcessOutput {
    /// Executed program as configured on [`Command`].
    pub tool: String,
    /// Wall-clock time spent waiting for the child.
    pub elapsed: Duration,
    /// Captured status and byte streams.
    pub output: Output,
}

/// Run a command with the caller's configured stream handling.
///
/// # Errors
///
/// Returns an I/O error when the child cannot be spawned or waited for.
pub fn run_status(command: &mut Command) -> io::Result<ProcessStatus> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let started = Instant::now();
    let status = command.status()?;
    let elapsed = started.elapsed();
    tracing::debug!(
        tool,
        elapsed_ms = elapsed.as_millis(),
        success = status.success(),
        "child process completed"
    );
    Ok(ProcessStatus {
        tool,
        elapsed,
        status,
    })
}

/// Run a command and capture both byte streams.
///
/// # Errors
///
/// Returns an I/O error when the child cannot be spawned, read, or waited for.
pub fn run_output(command: &mut Command) -> io::Result<ProcessOutput> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let started = Instant::now();
    let output = command.output()?;
    let elapsed = started.elapsed();
    tracing::debug!(
        tool,
        elapsed_ms = elapsed.as_millis(),
        success = output.status.success(),
        "captured child process completed"
    );
    Ok(ProcessOutput {
        tool,
        elapsed,
        output,
    })
}

/// Render captured output with a deterministic per-stream byte limit.
#[must_use]
pub fn bounded_output_detail(stdout: &[u8], stderr: &[u8], limit: usize) -> String {
    fn part(bytes: &[u8], label: &str, limit: usize) -> String {
        let truncated = bytes.len() > limit;
        let mut text = String::from_utf8_lossy(&bytes[..bytes.len().min(limit)])
            .trim()
            .to_owned();
        if truncated {
            text.push_str("\n[output truncated by aros]");
        }
        if text.is_empty() {
            String::new()
        } else {
            format!("{label}:\n{text}")
        }
    }

    let stdout = part(stdout, "stdout", limit);
    let stderr = part(stderr, "stderr", limit);
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

/// Return the terminating Unix signal, when the platform exposes one.
#[must_use]
#[cfg(unix)]
pub fn exit_signal(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

/// Non-Unix platforms do not expose a Unix terminating signal.
#[must_use]
#[cfg(not(unix))]
pub const fn exit_signal(_status: ExitStatus) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_detail_labels_and_truncates_each_stream() {
        assert_eq!(
            bounded_output_detail(b"abcdef", b"problem", 4),
            "stdout:\nabcd\n[output truncated by aros]\nstderr:\nprob\n[output truncated by aros]"
        );
    }

    #[test]
    fn output_observes_program_status_and_streams() {
        let observed =
            run_output(Command::new("sh").args(["-c", "printf output; printf error >&2; exit 7"]))
                .expect("run fixture");
        assert_eq!(observed.tool, "sh");
        assert_eq!(observed.output.status.code(), Some(7));
        assert_eq!(observed.output.stdout, b"output");
        assert_eq!(observed.output.stderr, b"error");
    }
}
