//! Shared subprocess execution primitives and bounded output rendering.
//!
//! Domain crates remain responsible for deciding whether a status is an error
//! and for assigning stable diagnostic codes. This module owns process
//! mechanics: program identity, elapsed time, exit status, concurrent pipe
//! draining, input delivery, capture limits, timeouts, termination and reaping.

use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Default maximum retained bytes for each captured child stream.
pub const DEFAULT_CAPTURE_LIMIT: usize = 64 * 1024;

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

/// Completed child process with a hard memory bound on both captured streams.
#[derive(Debug)]
pub struct ProcessOutput {
    /// Executed program as configured on [`Command`].
    pub tool: String,
    /// Wall-clock time spent waiting for the child.
    pub elapsed: Duration,
    /// Operating-system exit status.
    pub status: ExitStatus,
    /// Whether this runner terminated the process after its deadline.
    pub timed_out: bool,
    /// Bounded standard output.
    pub stdout: CapturedStream,
    /// Bounded standard error.
    pub stderr: CapturedStream,
}

/// One fully drained stream retaining either all bytes or a bounded head/tail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedStream {
    bytes: Vec<u8>,
    head_len: usize,
    omitted_bytes: u64,
    total_bytes: u64,
}

impl CapturedStream {
    /// Return the complete bytes only when no truncation occurred.
    #[must_use]
    pub fn exact_bytes(&self) -> Option<&[u8]> {
        (!self.is_truncated()).then_some(self.bytes.as_slice())
    }

    /// Return whether bytes between the retained head and tail were omitted.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.omitted_bytes != 0
    }

    /// Total bytes drained from the child stream.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Number of bytes omitted from the in-memory representation.
    #[must_use]
    pub const fn omitted_bytes(&self) -> u64 {
        self.omitted_bytes
    }

    /// Write the retained head and tail with an explicit omission marker.
    ///
    /// # Errors
    ///
    /// Returns a destination write error.
    pub fn write_rendered(&self, destination: &mut impl Write) -> io::Result<()> {
        if !self.is_truncated() {
            return destination.write_all(&self.bytes);
        }
        destination.write_all(&self.bytes[..self.head_len])?;
        write!(
            destination,
            "\n[{} bytes omitted by aros]\n",
            self.omitted_bytes
        )?;
        destination.write_all(&self.bytes[self.head_len..])
    }

    fn render_lossy(&self) -> String {
        if !self.is_truncated() {
            return String::from_utf8_lossy(&self.bytes).into_owned();
        }
        format!(
            "{}\n[{} bytes omitted by aros]\n{}",
            String::from_utf8_lossy(&self.bytes[..self.head_len]),
            self.omitted_bytes,
            String::from_utf8_lossy(&self.bytes[self.head_len..])
        )
    }
}

/// Completed child process stopped by a caller-supplied deadline or by itself.
#[derive(Debug)]
pub struct TimedProcessStatus {
    /// Executed program as configured on [`Command`].
    pub tool: String,
    /// Wall-clock time spent waiting for and, when needed, terminating the child.
    pub elapsed: Duration,
    /// Final, reaped operating-system status.
    pub status: ExitStatus,
    /// Whether this runner terminated the process after the deadline.
    pub timed_out: bool,
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

/// Run a command and capture both streams with [`DEFAULT_CAPTURE_LIMIT`].
///
/// The pipes are drained concurrently even after their retention limit is
/// reached, so a noisy child cannot deadlock or cause unbounded memory growth.
///
/// # Errors
///
/// Returns an I/O error when the child cannot be spawned, drained, or waited
/// for.
pub fn run_output(command: &mut Command) -> io::Result<ProcessOutput> {
    run_output_with_limit(command, DEFAULT_CAPTURE_LIMIT)
}

/// Run a command and capture each stream with an explicit retention limit.
///
/// # Errors
///
/// Returns an I/O error for a zero limit or when the child cannot be spawned,
/// drained, or waited for.
pub fn run_output_with_limit(
    command: &mut Command,
    per_stream_limit: usize,
) -> io::Result<ProcessOutput> {
    run_output_inner(command, None, per_stream_limit, None)
}

/// Run a command with bounded streams and a hard process-group deadline.
///
/// # Errors
///
/// Returns an I/O error for a zero limit or when the child process group cannot
/// be spawned, drained, terminated, or reaped.
pub fn run_output_with_timeout(
    command: &mut Command,
    per_stream_limit: usize,
    timeout: Duration,
) -> io::Result<ProcessOutput> {
    run_output_inner(command, None, per_stream_limit, Some(timeout))
}

/// Run a command with exact standard input and bounded captured streams.
///
/// Input and both output streams are handled concurrently, avoiding the
/// classic pipe deadlock where a child writes before consuming all input.
///
/// # Errors
///
/// Returns an I/O error for a zero limit, incomplete input delivery, or when
/// the child cannot be spawned, drained, or waited for.
pub fn run_output_with_input(
    command: &mut Command,
    input: &[u8],
    per_stream_limit: usize,
) -> io::Result<ProcessOutput> {
    run_output_inner(command, Some(input), per_stream_limit, None)
}

fn run_output_inner(
    command: &mut Command,
    input: Option<&[u8]>,
    per_stream_limit: usize,
    timeout: Option<Duration>,
) -> io::Result<ProcessOutput> {
    if per_stream_limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "captured stream limit must be greater than zero",
        ));
    }
    let tool = command.get_program().to_string_lossy().into_owned();
    configure_process_group(command);
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| abort_after_setup_error(&mut child, "child stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| abort_after_setup_error(&mut child, "child stderr pipe was unavailable"))?;
    let stdin = if input.is_some() {
        Some(child.stdin.take().ok_or_else(|| {
            abort_after_setup_error(&mut child, "child stdin pipe was unavailable")
        })?)
    } else {
        None
    };

    let stdout_reader = thread::spawn(move || drain_bounded(stdout, per_stream_limit));
    let stderr_reader = thread::spawn(move || drain_bounded(stderr, per_stream_limit));
    let input_writer = stdin.zip(input).map(|(mut stdin, input)| {
        let input = input.to_vec();
        thread::spawn(move || {
            stdin.write_all(&input)?;
            stdin.flush()
        })
    });

    let (status, timed_out) = match wait_for_child(&mut child, timeout) {
        Ok(result) => result,
        Err(primary) => {
            let cleanup = terminate_and_reap(&mut child);
            // A failed wait/termination path cannot prove that descendants
            // closed inherited descriptors. Detach workers instead of risking
            // a second, unbounded wait while reporting the primary failure.
            drop(stdout_reader);
            drop(stderr_reader);
            drop(input_writer);
            return Err(combine_process_errors(primary, &cleanup));
        }
    };
    if !timed_out {
        if let Err(primary) = kill_remaining_process_group(&mut child) {
            // Do not join pipe workers after group termination failed: a surviving
            // descendant may still own those descriptors and would make cleanup
            // block forever. Dropping the handles keeps this failure path bounded.
            drop(stdout_reader);
            drop(stderr_reader);
            drop(input_writer);
            return Err(primary);
        }
    }
    let (stdout, stderr) = join_workers(stdout_reader, stderr_reader, input_writer)?;
    let elapsed = started.elapsed();
    tracing::debug!(
        tool,
        elapsed_ms = elapsed.as_millis(),
        success = status.success(),
        timed_out,
        stdout_bytes = stdout.total_bytes(),
        stderr_bytes = stderr.total_bytes(),
        stdout_omitted = stdout.omitted_bytes(),
        stderr_omitted = stderr.omitted_bytes(),
        "captured child process completed"
    );
    Ok(ProcessOutput {
        tool,
        elapsed,
        status,
        timed_out,
        stdout,
        stderr,
    })
}

/// Run a command until it exits or `timeout` expires, then terminate and reap
/// it before returning.
///
/// # Errors
///
/// Returns an I/O error when spawn, polling, termination, or reaping fails.
pub fn run_status_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<TimedProcessStatus> {
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    let tool = command.get_program().to_string_lossy().into_owned();
    configure_process_group(command);
    let started = Instant::now();
    let deadline = started.checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process timeout deadline overflowed",
        )
    })?;
    let mut child = command.spawn()?;
    let (status, timed_out) = match wait_until(&mut child, deadline, POLL_INTERVAL) {
        Ok(result) => result,
        Err(primary) => {
            let cleanup = terminate_and_reap(&mut child);
            return Err(combine_process_errors(primary, &cleanup));
        }
    };
    if !timed_out {
        kill_remaining_process_group(&mut child)?;
    }
    Ok(timed_result(tool, started, status, timed_out))
}

fn wait_for_child(child: &mut Child, timeout: Option<Duration>) -> io::Result<(ExitStatus, bool)> {
    let Some(timeout) = timeout else {
        return child.wait().map(|status| (status, false));
    };
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process timeout deadline overflowed",
        )
    })?;
    wait_until(child, deadline, Duration::from_millis(20))
}

fn wait_until(
    child: &mut Child,
    deadline: Instant,
    poll_interval: Duration,
) -> io::Result<(ExitStatus, bool)> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        let now = Instant::now();
        if now >= deadline {
            if let Some(status) = child.try_wait().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("deadline expired and final child-state inspection failed: {error}"),
                )
            })? {
                return Ok((status, false));
            }
            if let Err(kill_error) = kill_process_group(child) {
                if let Some(status) = child.try_wait().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "deadline expired, process-group termination failed ({kill_error}), and final child-state inspection failed: {error}"
                        ),
                    )
                })? {
                    return Ok((status, false));
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("could not terminate timed-out process group: {kill_error}"),
                ));
            }
            return child.wait().map(|status| (status, true)).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "timed-out process group was terminated but could not be reaped: {error}"
                    ),
                )
            });
        }
        thread::sleep(poll_interval.min(deadline.saturating_duration_since(now)));
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_process_group(child: &Child) -> io::Result<()> {
    use rustix::process::{kill_process_group, Pid, Signal};

    match kill_process_group(Pid::from_child(child), Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut Child) -> io::Result<()> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg_attr(
    unix,
    allow(
        clippy::needless_pass_by_ref_mut,
        reason = "the cross-platform contract needs mutable Child for Child::kill on non-Unix"
    )
)]
fn kill_remaining_process_group(child: &mut Child) -> io::Result<()> {
    kill_process_group(child).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not terminate remaining child process group: {error}"),
        )
    })
}

fn timed_result(
    tool: String,
    started: Instant,
    status: ExitStatus,
    timed_out: bool,
) -> TimedProcessStatus {
    let elapsed = started.elapsed();
    tracing::debug!(
        tool,
        elapsed_ms = elapsed.as_millis(),
        success = status.success(),
        timed_out,
        "timed child process completed"
    );
    TimedProcessStatus {
        tool,
        elapsed,
        status,
        timed_out,
    }
}

fn drain_bounded(mut stream: impl Read, limit: usize) -> io::Result<CapturedStream> {
    let head_capacity = limit.div_ceil(2);
    let tail_capacity = limit - head_capacity;
    let mut head = Vec::with_capacity(head_capacity);
    let mut tail = Vec::with_capacity(tail_capacity);
    let mut total_bytes = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(u64::try_from(read).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("captured byte count overflowed"))?;
        let mut remaining = &buffer[..read];
        let head_remaining = head_capacity.saturating_sub(head.len());
        let retained_head = head_remaining.min(remaining.len());
        head.extend_from_slice(&remaining[..retained_head]);
        remaining = &remaining[retained_head..];
        if tail_capacity == 0 || remaining.is_empty() {
            continue;
        }
        if remaining.len() >= tail_capacity {
            tail.clear();
            tail.extend_from_slice(&remaining[remaining.len() - tail_capacity..]);
        } else {
            let overflow = tail
                .len()
                .saturating_add(remaining.len())
                .saturating_sub(tail_capacity);
            if overflow != 0 {
                tail.drain(..overflow);
            }
            tail.extend_from_slice(remaining);
        }
    }
    let retained = head
        .len()
        .checked_add(tail.len())
        .ok_or_else(|| io::Error::other("retained byte count overflowed"))?;
    let retained_u64 = u64::try_from(retained).map_err(io::Error::other)?;
    let omitted_bytes = total_bytes.saturating_sub(retained_u64);
    let head_len = head.len();
    head.extend_from_slice(&tail);
    Ok(CapturedStream {
        bytes: head,
        head_len,
        omitted_bytes,
        total_bytes,
    })
}

fn join_stream(
    reader: thread::JoinHandle<io::Result<CapturedStream>>,
    label: &str,
) -> io::Result<CapturedStream> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("{label} reader thread panicked")))?
}

fn join_writer(writer: thread::JoinHandle<io::Result<()>>) -> io::Result<()> {
    writer
        .join()
        .map_err(|_| io::Error::other("stdin writer thread panicked"))?
}

fn join_workers(
    stdout_reader: thread::JoinHandle<io::Result<CapturedStream>>,
    stderr_reader: thread::JoinHandle<io::Result<CapturedStream>>,
    input_writer: Option<thread::JoinHandle<io::Result<()>>>,
) -> io::Result<(CapturedStream, CapturedStream)> {
    let stdout = join_stream(stdout_reader, "stdout");
    let stderr = join_stream(stderr_reader, "stderr");
    let input = input_writer.map_or(Ok(()), join_writer);

    let mut primary = None;
    let mut additional = Vec::new();
    let mut record = |label: &str, error: &io::Error| {
        if primary.is_none() {
            primary = Some(io::Error::new(error.kind(), format!("{label}: {error}")));
        } else {
            additional.push(format!("{label}: {error}"));
        }
    };
    if let Err(error) = &stdout {
        record("drain stdout", error);
    }
    if let Err(error) = &stderr {
        record("drain stderr", error);
    }
    if let Err(error) = &input {
        record("write stdin", error);
    }
    if let Some(primary) = primary {
        return Err(combine_process_errors(primary, &additional));
    }

    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Ok((stdout, stderr)),
        _ => Err(io::Error::other(
            "process worker result disappeared after successful joins",
        )),
    }
}

fn abort_after_setup_error(child: &mut Child, message: &str) -> io::Error {
    let cleanup = terminate_and_reap(child);
    let primary = io::Error::other(message);
    combine_process_errors(primary, &cleanup)
}

fn terminate_and_reap(child: &mut Child) -> Vec<String> {
    let mut cleanup = Vec::new();
    if let Err(error) = kill_process_group(child) {
        cleanup.push(format!("terminate child process group: {error}"));
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => cleanup.push(
                "child could not be terminated and remains unreaped; manual process cleanup may be required"
                    .to_owned(),
            ),
            Err(error) => cleanup.push(format!("inspect child after failed termination: {error}")),
        }
        return cleanup;
    }
    if let Err(error) = child.wait() {
        cleanup.push(format!("reap terminated child: {error}"));
    }
    cleanup
}

fn combine_process_errors(primary: io::Error, cleanup: &[String]) -> io::Error {
    if cleanup.is_empty() {
        primary
    } else {
        io::Error::new(
            primary.kind(),
            format!("{primary}; cleanup also failed: {}", cleanup.join("; ")),
        )
    }
}

/// Render captured output with explicit per-stream labels and omission markers.
#[must_use]
pub fn bounded_output_detail(stdout: &CapturedStream, stderr: &CapturedStream) -> String {
    fn part(stream: &CapturedStream, label: &str) -> String {
        let text = stream.render_lossy().trim().to_owned();
        if text.is_empty() {
            String::new()
        } else {
            format!("{label}:\n{text}")
        }
    }

    let stdout = part(stdout, "stdout");
    let stderr = part(stderr, "stderr");
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

    #[cfg(unix)]
    #[test]
    fn capture_keeps_a_bounded_head_and_tail_while_draining_both_streams() {
        let observed = run_output_with_limit(
            Command::new("sh").args([
                "-c",
                "printf HEAD; yes x | head -c 131072; printf TAIL; printf ERROR >&2; exit 7",
            ]),
            1024,
        )
        .expect("run fixture");
        assert_eq!(observed.tool, "sh");
        assert_eq!(observed.status.code(), Some(7));
        assert_eq!(observed.stdout.total_bytes(), 131_080);
        assert!(observed.stdout.is_truncated());
        assert!(observed.stdout.omitted_bytes() > 100_000);
        let rendered = bounded_output_detail(&observed.stdout, &observed.stderr);
        assert!(rendered.contains("HEAD"));
        assert!(rendered.contains("TAIL"));
        assert!(rendered.contains("bytes omitted by aros"));
        assert!(rendered.contains("ERROR"));
    }

    #[cfg(unix)]
    #[test]
    fn exact_input_and_output_are_delivered_without_deadlock() {
        let input = vec![b'a'; 256 * 1024];
        let observed =
            run_output_with_input(Command::new("sh").args(["-c", "cat"]), &input, input.len())
                .expect("run fixture");
        assert!(observed.status.success());
        assert_eq!(observed.stdout.exact_bytes(), Some(input.as_slice()));
        assert_eq!(observed.stderr.exact_bytes(), Some([].as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_and_reaps_the_child() {
        let observed = run_status_with_timeout(
            Command::new("sh").args(["-c", "sleep 10"]),
            Duration::from_millis(30),
        )
        .expect("run fixture");
        assert!(observed.timed_out);
        assert!(!observed.status.success());
        assert!(observed.elapsed < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn captured_command_cleans_up_descendants_that_keep_pipes_open() {
        let started = Instant::now();
        let observed = run_output_with_limit(
            Command::new("sh").args(["-c", "sleep 10 & printf done"]),
            1024,
        )
        .expect("run fixture");
        assert!(observed.status.success());
        assert_eq!(observed.stdout.exact_bytes(), Some(b"done".as_slice()));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn zero_capture_limit_is_rejected_before_spawn() {
        let error = run_output_with_limit(&mut Command::new("definitely-not-a-command"), 0)
            .expect_err("zero limit must fail first");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
