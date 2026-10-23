//! Supervised execution of an explicitly selected private GNU query phase.
//!
//! This is a process/IO mechanism, not a Source-effect provider or authority
//! derived from an observation. The consuming original owner must independently
//! bind query scope, complete engine/environment semantics, runtime and external
//! inputs. In particular, exported Source values alone do not establish those
//! semantics. No default transpiler path selects this runner.
//! Executable/input namespaces must remain exclusively owned and immutable
//! during execution. Hash checks before/after do not defeat a hostile same-UID
//! process swapping a path temporarily; this is not descriptor-pinned exec.

use aros_common::{
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_startup_arguments::LiteralMakeStartupArguments,
    open_regular_file_nofollow, run_output_with_control, sha256_bytes, CancellationToken,
    Sha256Digest,
};
use aros_transpiler::make_shell_effects::gnu_query::{
    GnuShellQuery, GnuShellQueryResult, QUERY_GOAL,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    ffi::OsStr,
    io::Read as _,
    os::unix::{fs::MetadataExt as _, process::CommandExt as _},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const MAX_RESULT_BYTES: usize = 1024 * 1024 + 1;
const STREAM_BYTES: usize = 64 * 1024;
/// Private diagnostic record names, distinct from the result/status protocol.
pub const QUERY_STDOUT_RECORD: &str = "process-stdout.txt";
pub const QUERY_STDERR_RECORD: &str = "process-stderr.txt";
const OUTCOME_RECORD: &str = "process-outcome.txt";

/// Process/IO selection for one fresh query, not execution authentication.
pub struct GnuShellQueryExecution<'a> {
    pub program: &'a Path,
    pub program_sha256: &'a Sha256Digest,
    /// Exact original argv0/argv/cwd/incoming environment. Explicit Source
    /// inputs and goals are replaced by the selected private query phase.
    pub original: &'a NativeMakeInvocation,
    pub bundle: &'a GnuShellQuery,
    /// Held fresh private directory outside immutable original inventories.
    pub directory: &'a PrivateNativeCaptureDirectory,
}

/// Shared attempt/output accounting across every child of one original owner.
///
/// A refused attempt poisons this budget permanently; it is not a retry token.
/// Output accounting includes process streams and generated result/status bytes.
/// The aggregate budget is checked after a process returns; it is NOT an active
/// pipe-drain quota. Stream retention and result-file reads are separately
/// bounded. Deadline/cancellation stop noisy processes, not this byte counter.
/// Make's memory, file growth or arbitrary descendants escaping its supervised
/// process group remain outside these bounds. This is not a sandbox.
pub struct GnuShellQueryBudget {
    maximum_queries: usize,
    maximum_output_bytes: u64,
    queries: Cell<usize>,
    output_bytes: Cell<u64>,
    failed: Cell<bool>,
    deadline: Cell<Option<Instant>>,
}

impl GnuShellQueryBudget {
    /// Select finite aggregate bounds before the original owner's queries.
    /// # Errors
    /// Refuses zero or excessive counts and byte limits.
    pub fn new(maximum_queries: usize, maximum_output_bytes: u64) -> Result<Self, String> {
        if !(1..=65_536).contains(&maximum_queries)
            || !(1..=64 * 1024 * 1024).contains(&maximum_output_bytes)
        {
            return Err("GNU query aggregate limits are outside their admitted range".into());
        }
        Ok(Self {
            maximum_queries,
            maximum_output_bytes,
            queries: Cell::new(0),
            output_bytes: Cell::new(0),
            failed: Cell::new(false),
            deadline: Cell::new(None),
        })
    }

    #[must_use]
    pub const fn attempted_queries(&self) -> usize {
        self.queries.get()
    }

    #[must_use]
    pub const fn observed_output_bytes(&self) -> u64 {
        self.output_bytes.get()
    }

    /// Execute once under the caller's ORIGINAL remaining clock and policy.
    ///
    /// `check_owner` must revalidate the original consuming owner and return its
    /// remaining total time, never a renewed per-query timeout. It runs before
    /// preparation, immediately before spawn and after every prepared attempt,
    /// including process/IO failures. Shell nonzero status is data, whereas a
    /// failed Make query, cancellation, timeout or truncated stream refuses.
    /// Bounded process diagnostics are retained privately, not mistaken for the
    /// GNU-folded result protocol. Original quiet flags are never added/removed.
    /// Failed files remain untouched; no output path is overwritten or removed.
    /// The caller must retain/revalidate completed bundles until owner finish.
    ///
    /// # Errors
    /// Refuses stale policy/runtime, reused/linked IO, unsafe startup options,
    /// exhausted shared budgets, process failure or invalid generated framing.
    pub fn execute(
        &self,
        request: &GnuShellQueryExecution<'_>,
        cancellation: &CancellationToken,
        check_owner: impl Fn() -> Result<Duration, String>,
    ) -> Result<GnuShellQueryResult, String> {
        if self.failed.get() {
            return Err("GNU query owner already refused".into());
        }
        self.failed.set(true);
        if self.queries.get() >= self.maximum_queries {
            return Err("GNU query aggregate count exhausted".into());
        }
        self.queries.set(self.queries.get() + 1);
        self.check_time(cancellation, &check_owner)?;
        let result = self.execute_prepared(request, cancellation, &check_owner);
        // Do not short-circuit the original owner's postcheck on any IO or
        // query failure. A postcheck failure also discards a successful value.
        let runtime_after = validate_runtime(request);
        let owner_after = self.check_time(cancellation, &check_owner);
        let result = match (result, owner_after, runtime_after) {
            (Ok(value), Ok(_), Ok(())) => Ok(value),
            (Err(error), _, _) | (Ok(_), Err(error), _) | (Ok(_), Ok(_), Err(error)) => Err(error),
        };
        if result.is_ok() {
            self.failed.set(false);
        }
        result
    }

    fn execute_prepared(
        &self,
        request: &GnuShellQueryExecution<'_>,
        cancellation: &CancellationToken,
        check_owner: &impl Fn() -> Result<Duration, String>,
    ) -> Result<GnuShellQueryResult, String> {
        validate_runtime(request)?;
        let arguments = query_arguments(request.original, &request.bundle.makefile())?;
        for (name, bytes) in request.bundle.files() {
            request
                .directory
                .write_new(OsStr::new(name), bytes)
                .map_err(|_| "cannot create exclusive GNU query payload")?;
        }
        for output in [
            request.bundle.result_file(),
            request.bundle.status_file(),
            request.directory.path().join(QUERY_STDOUT_RECORD),
            request.directory.path().join(QUERY_STDERR_RECORD),
            request.directory.path().join(OUTCOME_RECORD),
        ] {
            match std::fs::symlink_metadata(output) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("GNU query output is not absent".into()),
            }
        }
        validate_payloads(request)?;
        let mut command = Command::new(request.program);
        if let Some(argument_zero) = request.original.argument_zero() {
            command.arg0(argument_zero);
        }
        command
            .args(arguments)
            .current_dir(request.original.working_directory())
            .env_clear()
            .envs(request.original.environment());
        validate_runtime(request)?;
        // Runtime remeasurement consumes the same clock; do not launch with a
        // remaining duration sampled before that work.
        let remaining = self.check_time(cancellation, check_owner)?;
        let output = run_output_with_control(&mut command, STREAM_BYTES, remaining, cancellation)
            .map_err(|_| "cannot supervise selected GNU query")?;
        retain_process_diagnostics(request.directory, &output)?;
        self.charge(
            output
                .stdout
                .total_bytes()
                .saturating_add(output.stderr.total_bytes()),
        )?;
        if output.cancelled
            || output.timed_out
            || !output.status.success()
            || output.stdout.is_truncated()
            || output.stderr.is_truncated()
        {
            return Err("selected GNU query did not complete cleanly".into());
        }
        validate_payloads(request)?;
        let value = self.read_output(request, &request.bundle.result_file(), MAX_RESULT_BYTES)?;
        let status = self.read_output(request, &request.bundle.status_file(), 4)?;
        validate_payloads(request)?;
        GnuShellQueryResult::decode(&value, &status)
    }

    fn charge(&self, bytes: u64) -> Result<(), String> {
        let total = self.output_bytes.get().saturating_add(bytes);
        self.output_bytes.set(total);
        if total > self.maximum_output_bytes {
            return Err("GNU query aggregate output exhausted".into());
        }
        Ok(())
    }

    fn check_time(
        &self,
        cancellation: &CancellationToken,
        check_owner: &impl Fn() -> Result<Duration, String>,
    ) -> Result<Duration, String> {
        // Always invoke the owner even after cancellation, including postcheck.
        let remaining = check_owner().map_err(|_| "GNU query original owner refused")?;
        if cancellation.is_cancelled() {
            return Err("GNU query cancelled".into());
        }
        let now = Instant::now();
        let selected = now
            .checked_add(remaining)
            .ok_or("GNU query original deadline is invalid")?;
        let deadline = self
            .deadline
            .get()
            .map_or(selected, |prior| prior.min(selected));
        self.deadline.set(Some(deadline));
        let remaining = deadline
            .checked_duration_since(now)
            .filter(|remaining| !remaining.is_zero())
            .ok_or("GNU query original deadline expired")?;
        Ok(remaining)
    }

    fn read_output(
        &self,
        request: &GnuShellQueryExecution<'_>,
        path: &Path,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        request
            .directory
            .revalidate()
            .map_err(|_| "GNU query directory changed")?;
        let mut file = open_regular_file_nofollow(path)
            .map_err(|_| "GNU query output is not a nonlinked regular file")?;
        let before = file
            .metadata()
            .map_err(|_| "cannot inspect GNU query output")?;
        self.charge(before.len())?;
        if before.nlink() != 1
            || before.len() > limit as u64
            || before.uid() != rustix::process::geteuid().as_raw()
            || before.mode() & 0o022 != 0
        {
            return Err("GNU query output is not bounded and privately owned".into());
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read bounded GNU query output")?;
        let after = file
            .metadata()
            .map_err(|_| "cannot recheck GNU query output")?;
        request
            .directory
            .revalidate()
            .map_err(|_| "GNU query directory changed")?;
        let named = std::fs::symlink_metadata(path).map_err(|_| "GNU query output disappeared")?;
        if bytes.len() > limit
            || bytes.len() as u64 != before.len()
            || metadata_identity(&before) != metadata_identity(&after)
            || metadata_identity(&after) != metadata_identity(&named)
        {
            return Err("GNU query output changed during readback".into());
        }
        Ok(bytes)
    }
}

fn retain_process_diagnostics(
    directory: &PrivateNativeCaptureDirectory,
    output: &aros_common::ProcessOutput,
) -> Result<(), String> {
    for (name, stream) in [
        (QUERY_STDOUT_RECORD, &output.stdout),
        (QUERY_STDERR_RECORD, &output.stderr),
    ] {
        let mut bytes = Vec::new();
        stream
            .write_rendered(&mut bytes)
            .map_err(|_| "cannot frame GNU query diagnostics")?;
        directory
            .write_new(OsStr::new(name), &bytes)
            .map_err(|_| "cannot retain exclusive GNU query diagnostics")?;
    }
    let outcome = format!(
        "exit={:?}\nsignal={:?}\ncancelled={}\ntimed_out={}\nstdout_bytes={}\nstderr_bytes={}\nstdout_truncated={}\nstderr_truncated={}\n",
        output.status.code(),
        aros_common::exit_signal(output.status),
        output.cancelled,
        output.timed_out,
        output.stdout.total_bytes(),
        output.stderr.total_bytes(),
        output.stdout.is_truncated(),
        output.stderr.is_truncated()
    );
    directory
        .write_new(OsStr::new(OUTCOME_RECORD), outcome.as_bytes())
        .map_err(|_| "cannot retain exclusive GNU query outcome".to_owned())
}

fn metadata_identity(
    metadata: &std::fs::Metadata,
) -> (u64, u64, u64, u32, u32, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.nlink(),
        metadata.mode(),
        metadata.uid(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn validate_runtime(request: &GnuShellQueryExecution<'_>) -> Result<(), String> {
    request
        .directory
        .revalidate()
        .map_err(|_| "GNU query directory changed")?;
    if request.bundle.makefile().parent() != Some(request.directory.path()) {
        return Err("GNU query bundle differs from selected directory".into());
    }
    let executable = super::checked_executable(request.program)
        .map_err(|_| "GNU query executable is unavailable")?;
    if super::measure_executable(&executable)
        .map_err(|_| "cannot remeasure GNU query executable")?
        .0
        != *request.program_sha256
    {
        return Err("GNU query executable changed".into());
    }
    Ok(())
}

fn validate_payloads(request: &GnuShellQueryExecution<'_>) -> Result<(), String> {
    for (name, bytes) in request.bundle.files() {
        request
            .directory
            .read_selected(OsStr::new(name), &sha256_bytes(bytes), bytes.len().max(1))
            .map_err(|_| "GNU query input payload changed")?;
    }
    Ok(())
}

fn query_arguments(
    original: &NativeMakeInvocation,
    makefile: &Path,
) -> Result<Vec<String>, String> {
    LiteralMakeStartupArguments::admit(original.arguments())
        .map_err(|_| "GNU query original startup is unsupported")?;
    let makefile = makefile.to_str().ok_or("GNU query filename is not UTF-8")?;
    let mut result = Vec::new();
    let mut command_values = BTreeMap::new();
    let mut arguments = original.arguments().iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-f" => {
                arguments
                    .next()
                    .ok_or("GNU query lost original file operand")?;
            }
            "-o" => {
                result.push(argument.clone());
                result.push(
                    arguments
                        .next()
                        .ok_or("GNU query lost original old-file operand")?
                        .clone(),
                );
            }
            _ if argument.starts_with("--file=") => {}
            _ if argument.starts_with('-') => result.push(argument.clone()),
            _ => {
                // File operands were consumed above. An '=' in a selected
                // filename must not be mistaken for a command-line assignment.
                if let Some((name, value)) = argument.split_once('=') {
                    command_values.insert(name.to_owned(), value.to_owned());
                    result.push(argument.clone());
                }
                // Other admitted words are original-phase goals, not queries.
            }
        }
    }
    aros_common::make_observer::validate_source_prelude_inputs(
        original.environment(),
        &command_values,
    )
    .map_err(|_| "GNU query inherited preload/options are unsupported")?;
    result.extend(["-f".to_owned(), makefile.to_owned(), QUERY_GOAL.to_owned()]);
    Ok(result)
}

#[cfg(test)]
mod tests;
