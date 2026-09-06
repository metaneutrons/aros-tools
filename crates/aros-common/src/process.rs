//! Shared subprocess execution primitives and bounded output rendering.
//!
//! Domain crates remain responsible for deciding whether a status is an error
//! and for assigning stable diagnostic codes. This module owns process
//! mechanics: program identity, elapsed time, exit status, concurrent pipe
//! draining, input delivery, capture limits, timeouts, termination and reaping.

use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Default maximum retained bytes for each captured child stream.
pub const DEFAULT_CAPTURE_LIMIT: usize = 64 * 1024;

/// Cooperative, one-way cancellation shared by an operation and its children.
///
/// Clones share the same flag. This token installs no process-wide signal
/// handler; the frontend owns signal policy and requests cancellation here.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Request cancellation. The token deliberately cannot be reset or reused
    /// to authorize a later phase after an operation has been cancelled.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Return whether this operation was cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

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
    /// Whether the controlled runner terminated this process on cancellation.
    /// Mutually exclusive with `timed_out`; older entry points always set false.
    pub cancelled: bool,
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
    run_output_inner(command, None, per_stream_limit, None, None)
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
    run_output_inner(command, None, per_stream_limit, Some(timeout), None)
}

/// Run a command with bounded capture, a deadline, and cooperative cancellation.
///
/// A cancellation observed before spawn returns [`io::ErrorKind::Interrupted`]
/// without starting a child. After spawn the result retains captured output and
/// the final reaped status; `cancelled` is distinct from `timed_out`. A child
/// already observed to have exited wins over a concurrent cancellation. Otherwise
/// cancellation wins over a simultaneously observed deadline. Unix cleanup covers
/// the process group, not descendants that deliberately escape it or a sandbox.
///
/// # Errors
/// Returns an I/O error for invalid limits/deadlines, pre-spawn cancellation,
/// spawn, capture or cleanup failure. Cancellation never retries a command.
pub fn run_output_with_control(
    command: &mut Command,
    per_stream_limit: usize,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> io::Result<ProcessOutput> {
    run_output_inner(
        command,
        None,
        per_stream_limit,
        Some(timeout),
        Some(cancellation),
    )
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
    run_output_inner(command, Some(input), per_stream_limit, None, None)
}

fn run_output_inner(
    command: &mut Command,
    input: Option<&[u8]>,
    per_stream_limit: usize,
    timeout: Option<Duration>,
    cancellation: Option<&CancellationToken>,
) -> io::Result<ProcessOutput> {
    if per_stream_limit == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "captured stream limit must be greater than zero",
        ));
    }
    let tool = command.get_program().to_string_lossy().into_owned();
    let started = Instant::now();
    let deadline = timeout
        .map(|value| process_deadline(started, value))
        .transpose()?;
    configure_process_group(command);
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    check_cancellation(cancellation)?;
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

    if let Err(error) = prepare_capture_pipe(&stdout)
        .and_then(|()| prepare_capture_pipe(&stderr))
        .and_then(|()| stdin.as_ref().map_or(Ok(()), prepare_capture_pipe))
    {
        return Err(combine_process_errors(
            error,
            &terminate_and_reap(&mut child),
        ));
    }
    let finished = Arc::new(AtomicBool::new(false));
    let stdout_finished = Arc::clone(&finished);
    let stderr_finished = Arc::clone(&finished);
    let stdout_reader =
        thread::spawn(move || drain_bounded(stdout, per_stream_limit, &stdout_finished));
    let stderr_reader =
        thread::spawn(move || drain_bounded(stderr, per_stream_limit, &stderr_finished));
    let input_writer = stdin.zip(input).map(|(stdin, input)| {
        let input = input.to_vec();
        let input_finished = Arc::clone(&finished);
        thread::spawn(move || write_complete(stdin, &input, &input_finished))
    });

    let (status, completion) = match wait_for_child(&mut child, deadline, cancellation) {
        Ok(result) => result,
        Err(primary) => {
            let cleanup = terminate_and_reap(&mut child);
            finished.store(true, Ordering::Release);
            // Report the primary failure promptly. On Unix, pipe workers see
            // the stop flag and close their descriptors within the drain bound.
            drop(stdout_reader);
            drop(stderr_reader);
            drop(input_writer);
            return Err(combine_process_errors(primary, &cleanup));
        }
    };
    if completion == Completion::Exited {
        if let Err(primary) = kill_remaining_process_group(&mut child) {
            finished.store(true, Ordering::Release);
            // A failed group termination is not complete cleanup. Unix pipe
            // workers stop independently; do not delay the primary failure.
            drop(stdout_reader);
            drop(stderr_reader);
            drop(input_writer);
            return Err(primary);
        }
    }
    finished.store(true, Ordering::Release);
    let (stdout, stderr) = join_workers(stdout_reader, stderr_reader, input_writer).map_err(
        |error| match completion {
            Completion::Cancelled => io::Error::new(
                io::ErrorKind::Interrupted,
                format!("operation cancelled; pipe cleanup also failed: {error}"),
            ),
            Completion::TimedOut => io::Error::new(
                io::ErrorKind::TimedOut,
                format!("process deadline expired; pipe cleanup also failed: {error}"),
            ),
            Completion::Exited => error,
        },
    )?;
    let elapsed = started.elapsed();
    let timed_out = completion == Completion::TimedOut;
    let cancelled = completion == Completion::Cancelled;
    tracing::debug!(
        tool,
        elapsed_ms = elapsed.as_millis(),
        success = status.success(),
        timed_out,
        cancelled,
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
        cancelled,
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
    let deadline = process_deadline(started, timeout)?;
    let mut child = command.spawn()?;
    let (status, completion) = match wait_until(&mut child, deadline, POLL_INTERVAL, None) {
        Ok(result) => result,
        Err(primary) => {
            let cleanup = terminate_and_reap(&mut child);
            return Err(combine_process_errors(primary, &cleanup));
        }
    };
    let timed_out = completion == Completion::TimedOut;
    if !timed_out {
        kill_remaining_process_group(&mut child)?;
    }
    Ok(timed_result(tool, started, status, timed_out))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Completion {
    Exited,
    TimedOut,
    Cancelled,
}

fn process_deadline(started: Instant, timeout: Duration) -> io::Result<Instant> {
    started.checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "process timeout deadline overflowed",
        )
    })
}

fn check_cancellation(cancellation: Option<&CancellationToken>) -> io::Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation cancelled before child spawn",
        ));
    }
    Ok(())
}

fn wait_for_child(
    child: &mut Child,
    deadline: Option<Instant>,
    cancellation: Option<&CancellationToken>,
) -> io::Result<(ExitStatus, Completion)> {
    let Some(deadline) = deadline else {
        return child.wait().map(|status| (status, Completion::Exited));
    };
    wait_until(child, deadline, Duration::from_millis(20), cancellation)
}

fn wait_until(
    child: &mut Child,
    deadline: Instant,
    poll_interval: Duration,
    cancellation: Option<&CancellationToken>,
) -> io::Result<(ExitStatus, Completion)> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, Completion::Exited));
        }
        let now = Instant::now();
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            let cleanup = terminate_and_reap(child);
            if !cleanup.is_empty() {
                return Err(combine_process_errors(
                    io::Error::new(io::ErrorKind::Interrupted, "cancelled child cleanup failed"),
                    &cleanup,
                ));
            }
            return child.wait().map(|status| (status, Completion::Cancelled));
        }
        if now >= deadline {
            if let Some(status) = child.try_wait().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("deadline expired and final child-state inspection failed: {error}"),
                )
            })? {
                return Ok((status, Completion::Exited));
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
                    return Ok((status, Completion::Exited));
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("could not terminate timed-out process group: {kill_error}"),
                ));
            }
            return child
                .wait()
                .map(|status| (status, Completion::TimedOut))
                .map_err(|error| {
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

#[cfg(unix)]
fn prepare_capture_pipe(stream: &impl std::os::fd::AsFd) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(stream)?;
    rustix::fs::fcntl_setfl(stream, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_capture_pipe<T>(_stream: &T) -> io::Result<()> {
    Ok(())
}

fn drain_bounded(
    mut stream: impl Read,
    limit: usize,
    finished: &AtomicBool,
) -> io::Result<CapturedStream> {
    let head_capacity = limit.div_ceil(2);
    let tail_capacity = limit - head_capacity;
    let mut head = Vec::with_capacity(head_capacity);
    let mut tail = Vec::with_capacity(tail_capacity);
    let mut total_bytes = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    let mut finished_at = None;
    loop {
        check_pipe_deadline(finished, &mut finished_at)?;
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error),
        };
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

fn check_pipe_deadline(finished: &AtomicBool, finished_at: &mut Option<Instant>) -> io::Result<()> {
    if finished.load(Ordering::Acquire) {
        let since = *finished_at.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(1) {
            return Err(io::Error::new(io::ErrorKind::TimedOut,
                "pipe remained open after child cleanup; a descendant may have escaped the process group and require separate cleanup"));
        }
    }
    Ok(())
}

fn write_complete(
    mut stream: impl Write,
    mut input: &[u8],
    finished: &AtomicBool,
) -> io::Result<()> {
    let mut finished_at = None;
    while !input.is_empty() {
        check_pipe_deadline(finished, &mut finished_at)?;
        match stream.write(input) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "child stdin accepted no bytes",
                ))
            }
            Ok(written) => input = &input[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    stream.flush()
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

    #[cfg(unix)]
    #[test]
    fn already_observed_exit_wins_over_late_cancellation() {
        let mut command = Command::new("sh");
        configure_process_group(&mut command);
        let mut child = command.args(["-c", "exit 7"]).spawn().unwrap();
        child.wait().unwrap();
        let token = CancellationToken::default();
        token.cancel();
        let (status, completion) = wait_until(
            &mut child,
            Instant::now(),
            Duration::from_millis(1),
            Some(&token),
        )
        .unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(completion == Completion::Exited);
    }
}
