//! A boot check that reads what happened instead of waiting.
//!
//! The command this replaces built an ISO, started QEMU, slept, killed it and
//! printed "VERIFIED: QEMU boot execution finished cleanly without crashes!".
//! Nothing between the sleep and that line read anything, so the message
//! appeared for a guest that triple-faulted in the first millisecond exactly as
//! for one that reached Workbench. It also booted from three paths that do not
//! exist and passed no `" debug=serial"`, without which the boot console prints
//! nothing at all.
//!
//! What this does instead:
//!
//!   * a verdict from the serial log and the QEMU exception trace, never from a
//!     timer;
//!   * named milestones, so "how far does it boot" is a number that can be
//!     compared across commits rather than an impression;
//!   * a faulting instruction pointer resolved to a symbol, which means
//!     modelling how the bootstrap's loader placed the kickstart -- work that
//!     was done by hand three times while getting here, and got the load base
//!     wrong once;
//!   * one evidence directory per run;
//!   * and a statement of what it did not test.
//!
//! The bootstrap is a multiboot image, so QEMU loads it the way GRUB would and
//! no bootable ISO is needed.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use miette::{miette, Result};

mod evidence;
use evidence::{
    apply_llvmpipe_jit_observation, furthest_milestone, locate, parse_llvmpipe_jit_proof,
    read_failures, read_faults,
};
#[cfg(test)]
use evidence::{bootstrap_pointer_width, is_nonzero_hex_pointer, package_members, traced_block};

/// How far a boot got. Ordered, and each variant is proved by something the
/// build actually prints or the trace actually records.
///
/// The list ends where the boot currently ends. It is meant to grow: a new
/// milestone is a new line in this enum plus the evidence that proves it, and
/// the check then reports the further one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Milestone {
    /// The ELF loader handed over and the kickstart's boot console works.
    KickstartRunning,
    /// The interrupt controller is up, so `ictl_Initialize` passed.
    InterruptController,
    /// Privileges dropped, so ExecBase exists and a task is running.
    UserMode,
    /// A module's LIBS symbol set is being walked, so autoinit runs.
    LibraryOpen,
}

impl Milestone {
    pub const fn label(self) -> &'static str {
        match self {
            Self::KickstartRunning => "kickstart running",
            Self::InterruptController => "interrupt controller up",
            Self::UserMode => "user mode reached",
            Self::LibraryOpen => "libraries being opened",
        }
    }

    /// The serial-log substring that proves it, where a string proves it.
    const fn serial_marker(self) -> Option<&'static str> {
        match self {
            Self::KickstartRunning => Some("The AROS Research OS"),
            Self::InterruptController => Some("[Kernel:APIC-IA32]"),
            // Proved by the trace, not by a message.
            Self::UserMode => None,
            Self::LibraryOpen => Some("Could not open version"),
        }
    }

    pub const ALL: [Self; 4] = [
        Self::KickstartRunning,
        Self::InterruptController,
        Self::UserMode,
        Self::LibraryOpen,
    ];
}

/// One CPU exception the trace recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// Exception vector, as QEMU prints it.
    pub vector: u8,
    /// Privilege level the fault was taken at.
    pub cpl: u8,
    /// Faulting instruction pointer.
    pub ip: u64,
    /// How many times this (vector, ip) pair occurred.
    pub count: usize,
}

/// Identity of an ISO image supplied to QEMU and retained with the evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IsoImageEvidence {
    /// Canonical source path supplied by the caller.
    pub canonical_path: PathBuf,
    /// Private retained snapshot that QEMU was asked to boot.
    pub snapshot_path: PathBuf,
    /// SHA-256 of the source and independently checked snapshot bytes.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedIsoImage {
    canonical_path: PathBuf,
    sha256: String,
}

/// Renderer, generated shader, pixel and successful unload evidence must agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmPipeJitProof {
    /// Renderer reported by GL_RENDERER.
    pub renderer: String,
    /// Named MCJIT function reported by the audit wrapper.
    pub function: String,
    /// Nonzero MCJIT code address reported by the audit wrapper.
    pub address: String,
    /// Center pixel read back from the rendered shader.
    pub pixel: [u8; 4],
}

/// The result of one boot.
#[derive(Debug, Default)]
pub struct BootReport {
    pub reached: Option<Milestone>,
    /// Statements read out of the logs, most important first.
    pub failures: Vec<String>,
    pub faults: Vec<Fault>,
    /// Faulting addresses resolved to a symbol.
    pub resolved: Vec<String>,
    /// What this run did not cover.
    pub untested: Vec<String>,
    /// ISO image identity, when QEMU booted from a CD-ROM image.
    pub iso_image: Option<IsoImageEvidence>,
    /// Whether the strict llvmpipe LLVM 11 GLSL/JIT proof was requested.
    pub require_llvmpipe_jit: bool,
    /// Proof parsed from this run's serial log.
    pub llvmpipe_jit_proof: Option<LlvmPipeJitProof>,
    /// Where the evidence is.
    pub evidence: PathBuf,
}

impl BootReport {
    #[must_use]
    pub const fn is_success(&self) -> bool {
        let positive_evidence = if self.require_llvmpipe_jit {
            self.llvmpipe_jit_proof.is_some()
        } else {
            self.reached.is_some()
        };
        positive_evidence && self.failures.is_empty() && self.faults.is_empty()
    }
}

/// What to boot and for how long.
#[derive(Debug, Clone)]
pub struct BootRequest {
    pub build_dir: PathBuf,
    /// Multiboot modules. Empty means the kickstart alone.
    pub modules: Vec<PathBuf>,
    /// Existing ISO image to boot instead of the build's multiboot image.
    pub iso_image: Option<PathBuf>,
    /// Require the llvmpipe LLVM 11 GLSL/JIT probe proof in the serial log.
    pub require_llvmpipe_jit: bool,
    pub seconds: u64,
    /// Root below which this invocation creates one private retained run.
    pub evidence: PathBuf,
    pub memory_mb: u32,
}

/// Reads the serial log and the exception trace, and says what happened.
pub fn check(request: &BootRequest) -> Result<BootReport> {
    if request.require_llvmpipe_jit && request.iso_image.is_none() {
        return Err(miette!("llvmpipe JIT proof requires an ISO image"));
    }
    if request.iso_image.is_some() && !request.modules.is_empty() {
        return Err(miette!(
            "ISO boot cannot be combined with multiboot modules"
        ));
    }

    let iso_source = request
        .iso_image
        .as_deref()
        .map(canonical_iso_image)
        .transpose()?;
    let direct_boot = if iso_source.is_none() {
        let boot = request.build_dir.join("SYS/boot");
        let bootstrap = boot.join("pc/bootstrap");
        let kickstart = boot.join("pc/kernel");
        for path in [&bootstrap, &kickstart] {
            if !path.exists() {
                return Err(miette!("missing {}", path.display()));
            }
        }
        Some((bootstrap, kickstart))
    } else {
        None
    };

    std::fs::create_dir_all(&request.evidence).map_err(|error| {
        miette!(
            "cannot create boot-evidence root {}: {error}",
            request.evidence.display()
        )
    })?;
    let run_directory = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(&request.evidence)
        .map_err(|error| {
            miette!(
                "cannot create a private run below boot-evidence root {}: {error}",
                request.evidence.display()
            )
        })?
        .keep();

    let modules = direct_boot
        .as_ref()
        .map_or_else(Vec::new, |(_, kickstart)| {
            let mut modules = vec![kickstart.clone()];
            modules.extend(request.modules.iter().cloned());
            modules
        });
    let mut report = BootReport {
        evidence: run_directory,
        require_llvmpipe_jit: request.require_llvmpipe_jit,
        ..BootReport::default()
    };
    if iso_source.is_none() && request.modules.is_empty() {
        report.untested.push(
            "only the kickstart was passed as a multiboot module, so nothing in a \
             package was loaded"
                .to_owned(),
        );
    }

    let serial = report.evidence.join("serial.log");
    let trace = report.evidence.join("exceptions.log");
    if let Some(source) = iso_source {
        let image = snapshot_iso_image(&report.evidence, source)?;
        write_iso_image_evidence(&report.evidence, &image)?;
        report.iso_image = Some(image);
    }
    create_evidence_file(&serial)?;
    create_evidence_file(&trace)?;
    let invocation = QemuInvocation {
        evidence: &report.evidence,
        bootstrap: direct_boot
            .as_ref()
            .map(|(bootstrap, _)| bootstrap.as_path()),
        modules: &modules,
        iso_image: report
            .iso_image
            .as_ref()
            .map(|image| image.snapshot_path.as_path()),
        serial: &serial,
        trace: &trace,
        instructions: false,
    };
    let observed = if request.require_llvmpipe_jit {
        run_qemu_until_llvmpipe_jit_proof(request, &invocation)?
    } else {
        run_qemu(request, &invocation)?.into()
    };

    let serial_text = read_evidence(&serial)?;
    let trace_text = read_evidence(&trace)?;
    report.reached = furthest_milestone(&serial_text, &trace_text);
    report.failures = read_failures(&serial_text, &trace_text);
    report.faults = read_faults(&trace_text);
    if observed.evidence_incomplete {
        report.failures.push(
            "the incremental serial/exception monitor could not classify a complete bounded log line"
                .to_owned(),
        );
    }
    if report.require_llvmpipe_jit {
        apply_llvmpipe_jit_observation(
            &mut report,
            &serial_text,
            observed.timed_out,
            request.seconds,
        );
    }
    if !observed.timed_out
        && !observed.cancelled
        && !observed.status.success()
        && report.failures.is_empty()
        && report.faults.is_empty()
    {
        return Err(crate::observability::unexpected_observed_exit(
            observed.into_timed(),
            &format!(
                "QEMU exited before the evidence deadline without producing a classified guest failure; retained evidence: {}",
                report.evidence.display()
            ),
        )
        .into());
    }

    // The second pass exists only to name the fault. An instruction trace is
    // large and slows the guest, so it is not paid for on a clean boot.
    if let Some((bootstrap, kickstart)) = direct_boot.as_ref().filter(|_| !report.faults.is_empty())
    {
        let asm_serial = report.evidence.join("instructions-serial.log");
        let asm = report.evidence.join("instructions.log");
        create_evidence_file(&asm_serial)?;
        create_evidence_file(&asm)?;
        run_qemu(
            request,
            &QemuInvocation {
                evidence: &report.evidence,
                bootstrap: Some(bootstrap),
                modules: &modules,
                iso_image: None,
                serial: &asm_serial,
                trace: &asm,
                instructions: true,
            },
        )?;
        let asm_text = read_evidence(&asm)?;
        match locate(
            kickstart,
            bootstrap,
            &request.modules,
            &asm_text,
            &report.faults,
        ) {
            Ok(lines) => report.resolved = lines,
            Err(error) => report.untested.push(format!(
                "the faulting address could not be resolved: {error}"
            )),
        }
    }

    Ok(report)
}

fn canonical_iso_image(path: &Path) -> Result<ValidatedIsoImage> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| miette!("cannot inspect ISO image {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(miette!(
            "ISO image must be an existing regular nonsymlink file: {}",
            path.display()
        ));
    }
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| miette!("cannot canonicalize ISO image {}: {error}", path.display()))?;
    let canonical_metadata = std::fs::symlink_metadata(&canonical_path).map_err(|error| {
        miette!(
            "cannot inspect canonical ISO image {}: {error}",
            canonical_path.display()
        )
    })?;
    if canonical_metadata.file_type().is_symlink() || !canonical_metadata.is_file() {
        return Err(miette!(
            "canonical ISO image is not a regular nonsymlink file: {}",
            canonical_path.display()
        ));
    }
    let mut source = aros_common::open_regular_file_nofollow(&canonical_path).map_err(|error| {
        miette!(
            "cannot open ISO image without following symlinks {}: {error}",
            canonical_path.display()
        )
    })?;
    let digest = aros_common::sha256_reader(&mut source).map_err(|error| {
        miette!(
            "cannot hash ISO image {}: {error}",
            canonical_path.display()
        )
    })?;
    Ok(ValidatedIsoImage {
        canonical_path,
        sha256: digest.digest.to_string(),
    })
}

fn snapshot_iso_image(directory: &Path, source: ValidatedIsoImage) -> Result<IsoImageEvidence> {
    let snapshot_path = directory.join("boot.iso");
    let mut input =
        aros_common::open_regular_file_nofollow(&source.canonical_path).map_err(|error| {
            miette!(
                "cannot open ISO source without following symlinks {}: {error}",
                source.canonical_path.display()
            )
        })?;
    let mut snapshot = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&snapshot_path)
        .map_err(|error| {
            miette!(
                "cannot create retained ISO snapshot {}: {error}",
                snapshot_path.display()
            )
        })?;
    std::io::copy(&mut input, &mut snapshot).map_err(|error| {
        miette!(
            "cannot copy ISO source {} to retained snapshot {}: {error}",
            source.canonical_path.display(),
            snapshot_path.display()
        )
    })?;
    snapshot.sync_all().map_err(|error| {
        miette!(
            "cannot flush retained ISO snapshot {}: {error}",
            snapshot_path.display()
        )
    })?;
    drop(snapshot);

    let mut snapshot_reader =
        aros_common::open_regular_file_nofollow(&snapshot_path).map_err(|error| {
            miette!(
                "cannot reopen retained ISO snapshot {}: {error}",
                snapshot_path.display()
            )
        })?;
    let snapshot_digest = aros_common::sha256_reader(&mut snapshot_reader).map_err(|error| {
        miette!(
            "cannot hash retained ISO snapshot {}: {error}",
            snapshot_path.display()
        )
    })?;
    let mut permissions = snapshot_reader
        .metadata()
        .map_err(|error| {
            miette!(
                "cannot inspect retained ISO snapshot {}: {error}",
                snapshot_path.display()
            )
        })?
        .permissions();
    permissions.set_readonly(true);
    snapshot_reader
        .set_permissions(permissions)
        .map_err(|error| {
            miette!(
                "cannot make retained ISO snapshot read-only {}: {error}",
                snapshot_path.display()
            )
        })?;

    let snapshot_sha256 = snapshot_digest.digest.to_string();
    if snapshot_sha256 != source.sha256 {
        return Err(miette!(
            "ISO source changed while it was being snapshotted; original SHA-256 {} differs from snapshot SHA-256 {} (retained at {})",
            source.sha256,
            snapshot_sha256,
            snapshot_path.display()
        ));
    }

    Ok(IsoImageEvidence {
        canonical_path: source.canonical_path,
        snapshot_path,
        sha256: snapshot_sha256,
    })
}

fn write_iso_image_evidence(directory: &Path, image: &IsoImageEvidence) -> Result<()> {
    let path = directory.join("iso-image.json");
    let serialized = serde_json::to_vec_pretty(image)
        .map_err(|error| miette!("cannot encode ISO evidence: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            miette!(
                "cannot create boot-evidence file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(&serialized).map_err(|error| {
        miette!(
            "cannot write boot-evidence file {}: {error}",
            path.display()
        )
    })?;
    file.write_all(b"\n").map_err(|error| {
        miette!(
            "cannot finish boot-evidence file {}: {error}",
            path.display()
        )
    })
}

fn create_evidence_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| {
            miette!(
                "cannot create boot-evidence file {}: {error}",
                path.display()
            )
        })
}

fn read_evidence(path: &Path) -> Result<String> {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\u{0}', ""))
        .map_err(|error| miette!("cannot read boot-evidence file {}: {error}", path.display()))
}

struct QemuInvocation<'a> {
    evidence: &'a Path,
    bootstrap: Option<&'a Path>,
    modules: &'a [PathBuf],
    iso_image: Option<&'a Path>,
    serial: &'a Path,
    trace: &'a Path,
    instructions: bool,
}

/// Process result shared by the ordinary observer and the proof-gated runner.
#[derive(Debug)]
struct QemuObservation {
    tool: String,
    elapsed: Duration,
    status: ExitStatus,
    timed_out: bool,
    cancelled: bool,
    evidence_incomplete: bool,
}

impl From<aros_common::TimedProcessStatus> for QemuObservation {
    fn from(observed: aros_common::TimedProcessStatus) -> Self {
        Self {
            tool: observed.tool,
            elapsed: observed.elapsed,
            status: observed.status,
            timed_out: observed.timed_out,
            cancelled: false,
            evidence_incomplete: false,
        }
    }
}

impl From<ControlledQemuOutput> for QemuObservation {
    fn from(observed: ControlledQemuOutput) -> Self {
        Self {
            tool: observed.output.tool,
            elapsed: observed.output.elapsed,
            status: observed.output.status,
            timed_out: observed.output.timed_out,
            cancelled: observed.output.cancelled,
            evidence_incomplete: observed.evidence_incomplete,
        }
    }
}

#[derive(Debug)]
struct ControlledQemuOutput {
    output: aros_common::ProcessOutput,
    evidence_incomplete: bool,
}

impl QemuObservation {
    fn into_timed(self) -> aros_common::TimedProcessStatus {
        aros_common::TimedProcessStatus {
            tool: self.tool,
            elapsed: self.elapsed,
            status: self.status,
            timed_out: self.timed_out,
        }
    }
}

/// Runs QEMU once, deterministically.
fn run_qemu(
    request: &BootRequest,
    invocation: &QemuInvocation<'_>,
) -> Result<aros_common::TimedProcessStatus> {
    let qemu = which::which("qemu-system-x86_64")
        .map_err(|_| miette!("qemu-system-x86_64 is not on PATH"))?;
    let mut command = qemu_command(&qemu, request, invocation);

    // A guest that resets exits on its own; one that keeps running is stopped
    // with its complete process group when time is up. Either way, the reaped
    // process status is only execution metadata: the logs determine the boot
    // verdict below.
    crate::observability::observe_until_timeout(
        &mut command,
        "bounded QEMU boot evidence run",
        std::time::Duration::from_secs(request.seconds),
    )
    .map_err(Into::into)
}

/// Runs strict ISO mode until the complete proof arrives, a definitive guest
/// failure appears, or the configured deadline expires. Only the first two
/// cases request cancellation; the controlled process runner reports deadline
/// expiry separately so it cannot be mistaken for proof-triggered shutdown.
fn run_qemu_until_llvmpipe_jit_proof(
    request: &BootRequest,
    invocation: &QemuInvocation<'_>,
) -> Result<QemuObservation> {
    let qemu = which::which("qemu-system-x86_64")
        .map_err(|_| miette!("qemu-system-x86_64 is not on PATH"))?;
    let command = qemu_command(&qemu, request, invocation);
    let output = run_controlled_qemu_until_jit_evidence(
        command,
        Duration::from_secs(request.seconds),
        invocation.serial,
        invocation.trace,
    )?;
    persist_qemu_output(invocation.evidence, &output.output)?;
    Ok(output.into())
}

const JIT_LOG_BYTES_PER_POLL: usize = 8 * 1024 * 1024;
const JIT_LOG_LINE_LIMIT: usize = 64 * 1024;
const JIT_SERIAL_WINDOW_LIMIT: usize = 256 * 1024;

struct IncrementalLogReader {
    file: File,
    pending: Vec<u8>,
    discarding_long_line: bool,
    invalid: bool,
}

impl IncrementalLogReader {
    fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            pending: Vec::new(),
            discarding_long_line: false,
            invalid: false,
        })
    }

    /// Read at most `byte_limit` new bytes and pass only complete lines to the
    /// visitor. A giant or truncated log line is retained as an invalid-evidence
    /// condition rather than buffered without limit or silently ignored.
    fn read_new_lines(
        &mut self,
        byte_limit: usize,
        mut visit: impl FnMut(&str),
    ) -> std::io::Result<bool> {
        let initial_position = self.file.stream_position()?;
        if self.file.metadata()?.len() < initial_position {
            self.invalid = true;
            self.pending.clear();
            self.discarding_long_line = true;
            return Ok(false);
        }

        let mut remaining = byte_limit;
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining > 0 {
            let read_limit = remaining.min(buffer.len());
            let count = self.file.read(&mut buffer[..read_limit])?;
            if count == 0 {
                break;
            }
            for byte in &buffer[..count] {
                if self.discarding_long_line {
                    if *byte == b'\n' {
                        self.discarding_long_line = false;
                    }
                    continue;
                }
                if *byte == b'\n' {
                    if self.pending.last() == Some(&b'\r') {
                        self.pending.pop();
                    }
                    let line = String::from_utf8_lossy(&self.pending);
                    visit(line.as_ref());
                    self.pending.clear();
                } else if self.pending.len() == JIT_LOG_LINE_LIMIT {
                    self.invalid = true;
                    self.pending.clear();
                    self.discarding_long_line = true;
                } else {
                    self.pending.push(*byte);
                }
            }
            remaining -= count;
        }

        let position = self.file.stream_position()?;
        let file_len = self.file.metadata()?.len();
        if file_len < position {
            self.invalid = true;
            self.pending.clear();
            self.discarding_long_line = true;
            return Ok(false);
        }
        Ok(position >= file_len)
    }

    const fn at_line_boundary(&self) -> bool {
        self.pending.is_empty() && !self.discarding_long_line
    }
}

struct BoundedSerialWindow {
    bytes: VecDeque<u8>,
}

impl BoundedSerialWindow {
    fn new() -> Self {
        Self {
            bytes: VecDeque::with_capacity(JIT_SERIAL_WINDOW_LIMIT),
        }
    }

    fn push_line(&mut self, line: &str) {
        for byte in line.bytes().chain(std::iter::once(b'\n')) {
            if self.bytes.len() == JIT_SERIAL_WINDOW_LIMIT {
                self.bytes.pop_front();
            }
            self.bytes.push_back(byte);
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<_>>()).into_owned()
    }
}

#[derive(Default)]
struct IncrementalTraceScan {
    next_record_is_hardware: bool,
    saw_fault: bool,
    saw_double_fault: bool,
}

impl IncrementalTraceScan {
    fn observe_line(&mut self, line: &str) {
        if line.trim_start().starts_with("Servicing hardware INT=") {
            self.next_record_is_hardware = true;
            return;
        }
        if line.contains("check_exception") {
            self.saw_double_fault = true;
        }
        let Some(at) = line.find(" v=") else {
            return;
        };
        if std::mem::take(&mut self.next_record_is_hardware) || line.contains(" i=1 ") {
            return;
        }
        let rest = &line[at + 3..];
        if rest
            .get(..2)
            .is_some_and(|vector| u8::from_str_radix(vector, 16).is_ok())
        {
            self.saw_fault = true;
        }
    }

    const fn has_failure(&self) -> bool {
        self.saw_fault || self.saw_double_fault
    }
}

struct IncrementalProofEvidence {
    serial: IncrementalLogReader,
    trace: IncrementalLogReader,
    serial_window: BoundedSerialWindow,
    pass_marker_seen: bool,
    probe_failure_seen: bool,
    guest_failure_seen: bool,
    trace_scan: IncrementalTraceScan,
}

struct IncrementalProofPoll {
    completed_proof: bool,
    definitive_failure: bool,
}

impl IncrementalProofEvidence {
    fn open(serial: &Path, trace: &Path) -> std::io::Result<Self> {
        Ok(Self {
            serial: IncrementalLogReader::open(serial)?,
            trace: IncrementalLogReader::open(trace)?,
            serial_window: BoundedSerialWindow::new(),
            pass_marker_seen: false,
            probe_failure_seen: false,
            guest_failure_seen: false,
            trace_scan: IncrementalTraceScan::default(),
        })
    }

    fn poll(&mut self) -> std::io::Result<IncrementalProofPoll> {
        let serial_caught_up = {
            let Self {
                serial,
                serial_window,
                pass_marker_seen,
                probe_failure_seen,
                guest_failure_seen,
                ..
            } = self;
            serial.read_new_lines(JIT_LOG_BYTES_PER_POLL, |line| {
                serial_window.push_line(line);
                let trimmed = line.trim();
                if trimmed == "=== LLVMPipe LLVM 11 GLSL/JIT PROBE PASS ===" {
                    *pass_marker_seen = true;
                }
                if has_llvmpipe_jit_fail_line(trimmed) {
                    *probe_failure_seen = true;
                }
                if has_guest_failure_line(trimmed) {
                    *guest_failure_seen = true;
                }
            })?
        };
        let trace_caught_up = {
            let Self {
                trace, trace_scan, ..
            } = self;
            trace.read_new_lines(JIT_LOG_BYTES_PER_POLL, |line| {
                trace_scan.observe_line(line);
            })?
        };

        let evidence_incomplete = self.serial.invalid || self.trace.invalid;
        let definitive_failure = evidence_incomplete
            || self.probe_failure_seen
            || self.guest_failure_seen
            || self.trace_scan.has_failure();
        let complete_lines = self.serial.at_line_boundary() && self.trace.at_line_boundary();
        let completed_proof = self.pass_marker_seen
            && serial_caught_up
            && trace_caught_up
            && complete_lines
            && !definitive_failure
            && parse_llvmpipe_jit_proof(&self.serial_window.text()).is_ok();

        Ok(IncrementalProofPoll {
            completed_proof,
            definitive_failure,
        })
    }
}

fn has_guest_failure_line(line: &str) -> bool {
    line.starts_with("[ELF Loader] Undefined symbol ")
        || line.starts_with("[Kernel:TLSF] free-list corruption at ")
        || line.contains("Relocation error in section")
        || line.contains("*** SYSTEM PANIC!!! ***")
        || line.contains("Critical boot failure")
        || line.starts_with("Exec Bootstrap Task: ")
        || line.contains("check_exception")
}

fn has_llvmpipe_jit_fail_line(line: &str) -> bool {
    line.contains("[llvmpipe-jit] FAIL") || line == "=== LLVMPipe LLVM 11 GLSL/JIT PROBE FAIL ==="
}

fn run_controlled_qemu_until_jit_evidence(
    mut command: Command,
    timeout: Duration,
    serial: &Path,
    trace: &Path,
) -> Result<ControlledQemuOutput> {
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    let started = Instant::now();
    let mut evidence = IncrementalProofEvidence::open(serial, trace)
        .map_err(|error| miette!("could not open incremental QEMU evidence logs: {error}"))?;
    let cancellation = aros_common::CancellationToken::default();
    let worker_cancellation = cancellation.clone();
    let worker = thread::spawn(move || {
        let remaining = timeout.saturating_sub(started.elapsed());
        aros_common::run_output_with_control(
            &mut command,
            aros_common::DEFAULT_CAPTURE_LIMIT,
            remaining,
            &worker_cancellation,
        )
    });

    loop {
        if worker.is_finished() {
            break;
        }

        let poll = match evidence.poll() {
            Ok(poll) => poll,
            Err(error) => {
                cancellation.cancel();
                let _ = worker.join();
                return Err(miette!(
                    "could not incrementally read QEMU evidence: {error}"
                ));
            }
        };
        if started.elapsed() < timeout && (poll.completed_proof || poll.definitive_failure) {
            cancellation.cancel();
            break;
        }

        thread::sleep(POLL_INTERVAL);
    }

    let output = worker
        .join()
        .map_err(|_| miette!("the controlled QEMU observer thread panicked"))?
        .map_err(|error| miette!("could not observe controlled QEMU run: {error}"))?;
    Ok(ControlledQemuOutput {
        output,
        evidence_incomplete: evidence.serial.invalid || evidence.trace.invalid,
    })
}

fn persist_qemu_output(evidence: &Path, output: &aros_common::ProcessOutput) -> Result<()> {
    for (name, stream) in [
        ("qemu.stdout.log", &output.stdout),
        ("qemu.stderr.log", &output.stderr),
    ] {
        let path = evidence.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                miette!(
                    "cannot create captured QEMU output {}: {error}",
                    path.display()
                )
            })?;
        stream.write_rendered(&mut file).map_err(|error| {
            miette!(
                "cannot write captured QEMU output {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn qemu_command(qemu: &Path, request: &BootRequest, invocation: &QemuInvocation<'_>) -> Command {
    let ram = invocation.evidence.join(if invocation.instructions {
        "instructions-guest-ram.bin"
    } else {
        "guest-ram.bin"
    });
    let list = invocation
        .modules
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(",");

    let mut command = Command::new(qemu);
    command
        // Fixed machine, so two runs are comparable. -no-reboot turns a triple
        // fault into an exit instead of an endless loop.
        // The guest's RAM is backed by a file in the evidence directory, so a
        // fault can be read rather than guessed at. Point 27g stalled on
        // exactly this: `FindMem` walks SysBase->MemList and dereferences a
        // successor holding x86 code, and saying which node and where it came
        // from needs the memory, not the trace.
        .args([
            "-object",
            &format!(
                "memory-backend-file,id=guest-ram,size={}M,mem-path={},share=on",
                request.memory_mb,
                ram.display()
            ),
        ])
        .args(["-machine", "q35,memory-backend=guest-ram"])
        .args([
            "-cpu",
            if invocation.iso_image.is_some() {
                "qemu64"
            } else {
                "qemu64,+avx2"
            },
        ])
        .args(["-smp", "1"])
        .args(["-m", &request.memory_mb.to_string()])
        .args(["-rtc", "base=2020-01-01T00:00:00"])
        .arg("-no-reboot")
        .args(["-display", "none"])
        .arg("-serial")
        .arg(format!("file:{}", invocation.serial.display()))
        .args([
            "-d",
            if invocation.instructions {
                "in_asm,int"
            } else {
                "int"
            },
        ])
        .arg("-D")
        .arg(invocation.trace);

    if let Some(iso_image) = invocation.iso_image {
        command
            .args(["-accel", "tcg"])
            .arg("-cdrom")
            .arg(iso_image)
            .args(["-boot", "order=d"])
            .args(["-monitor", "none"]);
    } else {
        // Without the leading space this does nothing: the boot console reads
        // `strstr(cmdline, " debug")` (arch/all-native/bootconsole/common.c:79).
        command
            .args(["-append", " debug=serial"])
            .arg("-kernel")
            .arg(
                invocation
                    .bootstrap
                    .expect("direct boot requires a bootstrap"),
            )
            .arg("-initrd")
            .arg(&list);
    }
    command
}

/// The furthest milestone the evidence proves.
/// The report, as the user reads it.
#[must_use]
pub fn render(report: &BootReport) -> String {
    let mut out = String::new();
    match report.reached {
        Some(milestone) => {
            let _ = writeln!(out, "reached: {}", milestone.label());
        }
        None if report.iso_image.is_some() => {
            out.push_str("reached: no ordinary boot milestone was observed\n");
        }
        None => out.push_str("reached: nothing; the kickstart never printed\n"),
    }
    if let Some(image) = &report.iso_image {
        let _ = writeln!(out, "  ISO: {}", image.canonical_path.display());
        let _ = writeln!(out, "  ISO snapshot: {}", image.snapshot_path.display());
        let _ = writeln!(out, "  ISO SHA-256: {}", image.sha256);
    }
    if report.require_llvmpipe_jit {
        if let Some(proof) = &report.llvmpipe_jit_proof {
            let _ = writeln!(out, "  llvmpipe renderer: {}", proof.renderer);
            let _ = writeln!(
                out,
                "  llvmpipe MCJIT: function={} address={}",
                proof.function, proof.address
            );
            let _ = writeln!(
                out,
                "  llvmpipe pixel: {},{},{},{}",
                proof.pixel[0], proof.pixel[1], proof.pixel[2], proof.pixel[3]
            );
            out.push_str("  llvmpipe probe lifecycle: returned and ELF unloaded\n");
        } else {
            out.push_str("  llvmpipe JIT proof: missing or invalid\n");
        }
    }
    for failure in &report.failures {
        let _ = writeln!(out, "  failure: {failure}");
    }
    for line in &report.resolved {
        let _ = writeln!(out, "  {line}");
    }
    for fault in &report.faults {
        if report.resolved.is_empty() {
            let _ = writeln!(
                out,
                "  fault: v={:02x} cpl={} IP={:#x} ({} times)",
                fault.vector, fault.cpl, fault.ip, fault.count
            );
        }
    }
    for note in &report.untested {
        let _ = writeln!(out, "  not tested: {note}");
    }
    let _ = writeln!(out, "  evidence: {}", report.evidence.display());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The alignment step before each descriptor uses the *bootstrap's*
    /// pointer width. On PC that is 4 while the descriptor it writes is the
    /// 64-bit one, and taking the module's width instead put every module after
    /// the first out by 4 bytes, growing to 80 by the fortieth.
    #[test]
    fn the_bootstrap_pointer_width_comes_from_the_bootstrap() {
        let scratch = tempfile::tempdir().unwrap();
        let elf32 = scratch.path().join("boot32");
        let elf64 = scratch.path().join("boot64");
        let mut header = vec![0x7f, b'E', b'L', b'F', 1];
        header.resize(64, 0);
        std::fs::write(&elf32, &header).unwrap();
        header[4] = 2;
        std::fs::write(&elf64, &header).unwrap();
        assert_eq!(bootstrap_pointer_width(&elf32), 4);
        assert_eq!(bootstrap_pointer_width(&elf64), 8);
        // A bootstrap that cannot be read must not silently pick the narrow
        // width: that would shift every module and look plausible.
        assert_eq!(bootstrap_pointer_width(&scratch.path().join("absent")), 8);
    }

    /// The package format, including the part that is easy to get wrong: the
    /// declared name length is a field width and may or may not count the
    /// terminator, while the name itself ends at the first NUL.
    #[test]
    fn package_members_read_names_as_c_strings_and_skip_by_field_width() {
        let mut package = b"PKG\x01\x00\x00\x00\x00".to_vec();
        // First member: length 19 for an 18-character name plus its NUL, the
        // shape the real bootkeyboard.class entry has.
        package.extend(19u32.to_be_bytes());
        package.extend(b"bootkeyboard.class\x00");
        package.push(0);
        package.extend(4u32.to_be_bytes());
        package.extend(b"AAAA");
        // Second member: length 15 for 15 characters, no NUL counted.
        package.extend(15u32.to_be_bytes());
        package.extend(b"bootmouse.class");
        package.push(0);
        package.extend(2u32.to_be_bytes());
        package.extend(b"BB");

        let members = package_members(&package);
        assert_eq!(members.len(), 2, "{members:?}");
        assert_eq!(members[0].0, "bootkeyboard.class");
        assert_eq!(members[0].1, b"AAAA");
        assert_eq!(members[1].0, "bootmouse.class");
        assert_eq!(members[1].1, b"BB");
    }

    /// A member name keeps only its basename, because that is what the loader
    /// stores and what the descriptor's `strlen` then measures.
    #[test]
    fn package_member_names_lose_their_path() {
        let mut package = b"PKG\x01\x00\x00\x00\x00".to_vec();
        let name = b"Devs/USB/hub.class";
        package.extend((name.len() as u32).to_be_bytes());
        package.extend(name);
        package.push(0);
        package.extend(1u32.to_be_bytes());
        package.push(b'X');
        let members = package_members(&package);
        assert_eq!(members[0].0, "hub.class");
    }

    /// The descriptor advance moves an already-aligned pointer on by a full
    /// word: `(p + 8) & ~7` is 16 for p = 8, not 8. Getting that wrong shifts
    /// every module after the first, which is exactly the failure this
    /// modelling exists to avoid.
    #[test]
    fn the_descriptor_advance_moves_an_aligned_pointer() {
        let advance = |p: u64, word: u64| (p + word) & !(word - 1);
        assert_eq!(advance(8, 8), 16);
        assert_eq!(advance(9, 8), 16);
        assert_eq!(advance(15, 8), 16);
        assert_eq!(advance(16, 8), 24);
    }

    #[test]
    fn a_clean_serial_log_proves_the_milestones_it_shows() {
        let serial = "AROS64 - The AROS Research OS\n[Kernel:APIC-IA32] MSI\n";
        assert_eq!(
            furthest_milestone(serial, ""),
            Some(Milestone::InterruptController)
        );
    }

    #[test]
    fn user_mode_is_proved_by_the_trace_because_nothing_prints_it() {
        let serial = "AROS64 - The AROS Research OS\n";
        let trace = "     0: v=0d e=0000 i=0 cpl=3 IP=002b:00000000013adfa3\n";
        assert_eq!(furthest_milestone(serial, trace), Some(Milestone::UserMode));
    }

    #[test]
    fn an_undefined_symbol_is_named_once_however_often_it_repeats() {
        let serial = "[ELF Loader] Undefined symbol 'con_LibName'\n\
                      [ELF Loader] Undefined symbol 'con_LibName'\n\
                      con-handler: Relocation error in section 3!\n";
        let failures = read_failures(serial, "");
        assert!(
            failures
                .iter()
                .any(|line| line.contains("con_LibName") && line.contains("2 times")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|line| line.contains("refused a module")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_kernel_panic_carries_its_reason_out_of_the_box() {
        let serial = "+-------+\n\
                      | Critical boot failure |\n\
                      | Failed to allocate APIC descriptor. |\n\
                      \n";
        let failures = read_failures(serial, "");
        assert!(
            failures.iter().any(|line| line.contains("APIC descriptor")),
            "{failures:?}"
        );
    }

    #[test]
    fn allocator_corruption_is_a_failure_before_its_deliberate_trap() {
        let line = "[Kernel:TLSF] free-list corruption at REMOVE_HEADER: bucket=17/16 block=0x6b2fa60 size=0 flags=0x0";
        assert!(has_guest_failure_line(line));
        let failures = read_failures(&format!("{line}\r\n"), "");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("allocator corruption at REMOVE_HEADER"));
        assert!(failures[0].contains("block=0x6b2fa60"));
        let report = BootReport {
            reached: Some(Milestone::UserMode),
            failures,
            ..BootReport::default()
        };
        assert!(!report.is_success());
        assert!(!has_guest_failure_line("[Kernel:TLSF] initialized pool"));
        assert!(read_failures("[Kernel:TLSF] initialized pool\n", "").is_empty());
    }

    #[test]
    fn a_software_interrupt_is_not_a_fault() {
        // AROS enters supervisor mode with int 0xfe; that is the mechanism
        // working, not a defect.
        let trace = "     0: v=fe e=0000 i=1 cpl=3 IP=002b:00000000013a26a9 pc=x\n";
        assert!(read_faults(trace).is_empty());
    }

    #[test]
    fn a_hardware_interrupt_is_not_a_fault() {
        let trace = "Servicing hardware INT=0xf6\n\
                         43: v=f6 e=0000 i=0 cpl=3 IP=002b:0000000001ab7820 pc=x\n";
        assert!(read_faults(trace).is_empty());
    }

    #[test]
    fn faults_are_collapsed_by_vector_and_address() {
        let trace = "     0: v=0d e=0000 i=0 cpl=0 IP=0008:00000000013ae003 pc=x\n\
                          1: v=0d e=0000 i=0 cpl=0 IP=0008:00000000013ae003 pc=x\n\
                          2: v=08 e=0000 i=0 cpl=0 IP=0008:00000000013ae003 pc=x\n";
        let faults = read_faults(trace);
        assert_eq!(faults.len(), 2, "{faults:?}");
        let gp = faults.iter().find(|fault| fault.vector == 0x0d).unwrap();
        assert_eq!(gp.count, 2);
        assert_eq!(gp.ip, 0x013a_e003);
    }

    #[test]
    fn a_traced_block_yields_its_address_and_bytes() {
        let line = "0x013942a7:  48 85 c0                 testq    %rax, %rax";
        let (address, bytes) = traced_block(line).unwrap();
        assert_eq!(address, 0x0139_42a7);
        assert_eq!(bytes, [0x48, 0x85, 0xc0]);
    }

    #[test]
    fn a_run_with_a_fault_is_not_a_success() {
        let report = BootReport {
            faults: vec![Fault {
                vector: 14,
                cpl: 0,
                ip: 1,
                count: 1,
            }],
            ..BootReport::default()
        };
        assert!(!report.is_success());
    }

    #[test]
    fn an_empty_evidence_set_is_not_a_success() {
        assert!(!BootReport::default().is_success());
    }

    #[test]
    fn success_requires_a_positive_milestone_without_findings() {
        let report = BootReport {
            reached: Some(Milestone::KickstartRunning),
            ..BootReport::default()
        };
        assert!(report.is_success());
    }

    #[test]
    fn required_jit_proof_is_positive_evidence_even_without_a_smoke_milestone() {
        let proof = parse_llvmpipe_jit_proof(&valid_jit_serial()).unwrap();
        let report = BootReport {
            require_llvmpipe_jit: true,
            llvmpipe_jit_proof: Some(proof),
            ..BootReport::default()
        };
        assert!(report.is_success());
        assert!(!BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        }
        .is_success());
    }

    #[test]
    fn jit_parser_requires_renderer_symbol_pixel_and_exact_pass_marker() {
        let proof = parse_llvmpipe_jit_proof(&valid_jit_serial()).unwrap();
        assert_eq!(proof.renderer, "llvmpipe (LLVM 11.0.0, 256 bits)");
        assert_eq!(proof.function, "fs_variant_whole");
        assert_eq!(proof.address, "FFEEDDCCBBAA9988");
        assert_eq!(proof.pixel, [64, 128, 191, 255]);
        let partial = valid_jit_serial()
            .replace("fs_variant_whole", "fs_variant_partial")
            .replace("FFEEDDCCBBAA9988", "0xdeadbeef");
        assert_eq!(
            parse_llvmpipe_jit_proof(&partial).unwrap().function,
            "fs_variant_partial"
        );

        for serial in [
            "=== LLVMPipe LLVM 11 GLSL/JIT PROBE PASS ===\n",
            "[llvmpipe-mcjit] function=fs_variant_whole address=FFEEDDCCBBAA9988\n",
            "[llvmpipe-jit] GL_RENDERER: llvmpipe (LLVM 11.0.0, 256 bits)\n\
             [llvmpipe-mcjit] function=fs_variant_whole address=FFEEDDCCBBAA9988\n\
             [llvmpipe-jit] center RGBA: 64 128 191 255\n",
        ] {
            assert!(
                parse_llvmpipe_jit_proof(serial).is_err(),
                "incomplete evidence must fail: {serial:?}"
            );
        }
    }

    #[test]
    fn jit_parser_rejects_wrong_renderer_symbols_null_addresses_pixels_and_fail_markers() {
        let good = valid_jit_serial();
        for serial in [
            good.replace("LLVM 11.0.0", "LLVM 110.0.0"),
            good.replace("LLVM 11.0.0", "LLVM 11.0.01"),
            good.replace("LLVM 11.0.0", "LLVM 11.0.0-dev"),
            good.replace("function=fs_variant_whole", "function=<unnamed>"),
            good.replace("function=fs_variant_whole", "function=other_variant"),
            good.replace("address=FFEEDDCCBBAA9988", "address=0000000000000000"),
            good.replace("64 128 191 255", "64 128 181 255"),
            good.replace("=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===", ""),
            format!(
                "=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===\n{}",
                good.replace("=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===", "")
            ),
            format!("{good}\n[llvmpipe-jit] FAIL: shader compilation failed\n"),
            good.replace("address=FFEEDDCCBBAA9988", "address=0000000000000000"),
        ] {
            assert!(
                parse_llvmpipe_jit_proof(&serial).is_err(),
                "invalid evidence must fail: {serial:?}"
            );
        }
    }

    #[test]
    fn mcjit_pointer_parser_accepts_aros_bare_hex_and_optional_prefixes() {
        assert!(is_nonzero_hex_pointer("FFEEDDCCBBAA9988"));
        assert!(is_nonzero_hex_pointer("0xdeadbeef"));
        assert!(is_nonzero_hex_pointer("0Xdeadbeef"));
        assert!(!is_nonzero_hex_pointer("0000000000000000"));
        assert!(!is_nonzero_hex_pointer("(nil)"));
        assert!(!is_nonzero_hex_pointer("0x10000000000000000"));
    }

    #[test]
    fn iso_identity_is_canonical_hashed_and_requires_a_regular_nonsymlink_file() {
        let scratch = tempfile::tempdir().unwrap();
        let image_path = scratch.path().join("boot.iso");
        let contents = b"test ISO bytes";
        std::fs::write(&image_path, contents).unwrap();
        let image = canonical_iso_image(&image_path).unwrap();
        assert_eq!(
            image.canonical_path,
            std::fs::canonicalize(&image_path).unwrap()
        );
        assert_eq!(
            image.sha256,
            aros_common::sha256_bytes(contents).to_string()
        );
        assert_eq!(std::fs::read(&image_path).unwrap(), contents);
        assert!(canonical_iso_image(scratch.path()).is_err());
        assert!(canonical_iso_image(&scratch.path().join("missing.iso")).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = scratch.path().join("boot-link.iso");
            symlink(&image_path, &link).unwrap();
            assert!(canonical_iso_image(&link).is_err());
        }
    }

    #[test]
    fn iso_snapshot_is_hashed_read_only_and_does_not_modify_the_source() {
        let scratch = tempfile::tempdir().unwrap();
        let source_path = scratch.path().join("source.iso");
        let evidence = scratch.path().join("run");
        std::fs::create_dir(&evidence).unwrap();
        let contents = b"immutable test image";
        std::fs::write(&source_path, contents).unwrap();
        let validated = canonical_iso_image(&source_path).unwrap();
        let image = snapshot_iso_image(&evidence, validated).unwrap();

        assert_eq!(
            image.canonical_path,
            std::fs::canonicalize(&source_path).unwrap()
        );
        assert_eq!(image.snapshot_path, evidence.join("boot.iso"));
        assert_eq!(std::fs::read(&source_path).unwrap(), contents);
        assert_eq!(std::fs::read(&image.snapshot_path).unwrap(), contents);
        assert_eq!(
            image.sha256,
            aros_common::sha256_bytes(contents).to_string()
        );
        assert!(std::fs::metadata(&image.snapshot_path)
            .unwrap()
            .permissions()
            .readonly());
    }

    #[test]
    fn iso_snapshot_rejects_source_changes_after_initial_hashing() {
        let scratch = tempfile::tempdir().unwrap();
        let source_path = scratch.path().join("source.iso");
        let evidence = scratch.path().join("run");
        std::fs::create_dir(&evidence).unwrap();
        std::fs::write(&source_path, b"original image").unwrap();
        let validated = canonical_iso_image(&source_path).unwrap();
        std::fs::write(&source_path, b"replacement image").unwrap();

        let error = snapshot_iso_image(&evidence, validated)
            .expect_err("snapshot must not accept bytes that changed after validation");
        assert!(error.to_string().contains("ISO source changed"));
        assert!(std::fs::metadata(evidence.join("boot.iso"))
            .unwrap()
            .permissions()
            .readonly());
    }

    #[test]
    fn iso_hash_and_canonical_path_are_saved_and_rendered() {
        let scratch = tempfile::tempdir().unwrap();
        let image = IsoImageEvidence {
            canonical_path: scratch.path().join("boot.iso"),
            snapshot_path: scratch.path().join("run-1/boot.iso"),
            sha256: "ab".repeat(32),
        };
        write_iso_image_evidence(scratch.path(), &image).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(scratch.path().join("iso-image.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved["canonical_path"],
            image.canonical_path.display().to_string()
        );
        assert_eq!(
            saved["snapshot_path"],
            image.snapshot_path.display().to_string()
        );
        assert_eq!(saved["sha256"], image.sha256);

        let report = BootReport {
            iso_image: Some(image.clone()),
            require_llvmpipe_jit: true,
            evidence: scratch.path().to_path_buf(),
            ..BootReport::default()
        };
        let rendered = render(&report);
        assert!(rendered.contains(&image.canonical_path.display().to_string()));
        assert!(rendered.contains(&image.snapshot_path.display().to_string()));
        assert!(rendered.contains(&image.sha256));
        assert!(rendered.contains("llvmpipe JIT proof: missing or invalid"));
    }

    #[test]
    fn iso_qemu_command_uses_cdrom_without_multiboot_or_append_arguments() {
        let scratch = tempfile::tempdir().unwrap();
        let image = scratch.path().join("boot.iso");
        let request = BootRequest {
            build_dir: scratch.path().join("missing-build-tree"),
            modules: Vec::new(),
            iso_image: Some(image.clone()),
            require_llvmpipe_jit: true,
            seconds: 1,
            evidence: scratch.path().join("evidence"),
            memory_mb: 64,
        };
        let command = qemu_command(
            Path::new("fake-qemu"),
            &request,
            &QemuInvocation {
                evidence: scratch.path(),
                bootstrap: None,
                modules: &[],
                iso_image: Some(&image),
                serial: &scratch.path().join("serial.log"),
                trace: &scratch.path().join("exceptions.log"),
                instructions: false,
            },
        );
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(arguments.windows(2).any(|pair| pair == ["-cpu", "qemu64"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-accel", "tcg"]));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-boot", "order=d"]));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-monitor", "none"]));
        assert!(arguments.contains(&"-cdrom".to_owned()));
        assert!(arguments.contains(&image.display().to_string()));
        for forbidden in ["-append", "-kernel", "-initrd"] {
            assert!(!arguments.contains(&forbidden.to_owned()), "{arguments:?}");
        }

        let bootstrap = scratch.path().join("bootstrap");
        let modules = [scratch.path().join("module.pkg")];
        let direct = qemu_command(
            Path::new("fake-qemu"),
            &request,
            &QemuInvocation {
                evidence: scratch.path(),
                bootstrap: Some(&bootstrap),
                modules: &modules,
                iso_image: None,
                serial: &scratch.path().join("direct-serial.log"),
                trace: &scratch.path().join("direct-exceptions.log"),
                instructions: false,
            },
        );
        let direct_arguments = direct
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(direct_arguments
            .windows(2)
            .any(|pair| pair == ["-cpu", "qemu64,+avx2"]));
        assert!(direct_arguments.contains(&"-append".to_owned()));
        assert!(direct_arguments.contains(&"-kernel".to_owned()));
        assert!(direct_arguments.contains(&"-initrd".to_owned()));
        assert!(!direct_arguments.contains(&"-cdrom".to_owned()));
        assert!(!direct_arguments.contains(&"-accel".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn fake_qemu_exit_status_remains_observation_metadata() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("fake-qemu");
        let recorded_arguments = scratch.path().join("arguments.txt");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_QEMU_ARGUMENTS\"\nexit 23\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let image = scratch.path().join("boot.iso");
        let request = BootRequest {
            build_dir: scratch.path().to_path_buf(),
            modules: Vec::new(),
            iso_image: Some(image.clone()),
            require_llvmpipe_jit: false,
            seconds: 1,
            evidence: scratch.path().to_path_buf(),
            memory_mb: 64,
        };
        let mut command = qemu_command(
            &script,
            &request,
            &QemuInvocation {
                evidence: scratch.path(),
                bootstrap: None,
                modules: &[],
                iso_image: Some(&image),
                serial: &scratch.path().join("serial.log"),
                trace: &scratch.path().join("exceptions.log"),
                instructions: false,
            },
        );
        command.env("FAKE_QEMU_ARGUMENTS", &recorded_arguments);
        let observed = crate::observability::observe_until_timeout(
            &mut command,
            "fake QEMU argument test",
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert!(!observed.timed_out);
        assert_eq!(observed.status.code(), Some(23));
        let arguments = std::fs::read_to_string(recorded_arguments).unwrap();
        assert!(arguments.lines().any(|line| line == "-cdrom"));
        assert!(arguments.lines().any(|line| line == "-accel"));
        assert!(!arguments.lines().any(|line| line == "-kernel"));
    }

    #[test]
    fn incremental_exception_reader_carries_records_split_across_chunks() {
        let scratch = tempfile::tempdir().unwrap();
        let trace_path = scratch.path().join("exceptions.log");
        std::fs::write(
            &trace_path,
            "0: v=0e e=0000 IP=0008:0000000000001234 cpl=3\n",
        )
        .unwrap();

        let mut reader = IncrementalLogReader::open(&trace_path).unwrap();
        let mut scan = IncrementalTraceScan::default();
        let mut caught_up = reader
            .read_new_lines(12, |line| scan.observe_line(line))
            .unwrap();
        assert!(!caught_up);
        assert!(!scan.has_failure());

        while !caught_up {
            caught_up = reader
                .read_new_lines(12, |line| scan.observe_line(line))
                .unwrap();
        }
        assert!(scan.has_failure());
        assert!(!reader.invalid);
    }

    #[test]
    fn incremental_exception_reader_scans_large_logs_once_in_bounded_chunks() {
        const LINE_COUNT: usize = 180_000;

        let scratch = tempfile::tempdir().unwrap();
        let trace_path = scratch.path().join("exceptions.log");
        let content = "harmless trace record\n".repeat(LINE_COUNT);
        std::fs::write(&trace_path, content).unwrap();

        let mut reader = IncrementalLogReader::open(&trace_path).unwrap();
        let mut line_count = 0;
        let mut poll_count = 0;
        let mut caught_up = false;
        while !caught_up {
            caught_up = reader
                .read_new_lines(64 * 1024, |_| line_count += 1)
                .unwrap();
            poll_count += 1;
        }

        assert_eq!(line_count, LINE_COUNT);
        assert!(poll_count > 1);
        assert!(!reader.invalid);
    }

    #[cfg(unix)]
    #[test]
    fn strict_iso_runner_cancels_qemu_after_a_complete_timely_proof() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("fake-qemu");
        let serial = scratch.path().join("serial.log");
        let trace = scratch.path().join("exceptions.log");
        std::fs::write(&serial, "").unwrap();
        std::fs::write(&trace, "").unwrap();
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$FAKE_SERIAL_CONTENT\" > \"$FAKE_SERIAL\"\nexec sleep 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(&script);
        command
            .env("FAKE_SERIAL", &serial)
            .env("FAKE_SERIAL_CONTENT", valid_jit_serial());
        let observed = run_controlled_qemu_until_jit_evidence(
            command,
            Duration::from_secs(3),
            &serial,
            &trace,
        )
        .unwrap();

        assert!(observed.output.cancelled);
        assert!(!observed.output.timed_out);
        assert!(!observed.evidence_incomplete);
        assert!(observed.output.elapsed < Duration::from_secs(3));
        let mut report = BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(
            &mut report,
            &read_evidence(&serial).unwrap(),
            observed.output.timed_out,
            3,
        );
        assert!(report.is_success());
    }

    #[cfg(unix)]
    #[test]
    fn strict_iso_runner_reports_a_real_deadline_without_proof_as_timeout() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("fake-qemu");
        let serial = scratch.path().join("serial.log");
        let trace = scratch.path().join("exceptions.log");
        std::fs::write(&serial, "").unwrap();
        std::fs::write(&trace, "").unwrap();
        std::fs::write(&script, "#!/bin/sh\nexec sleep 5\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let observed = run_controlled_qemu_until_jit_evidence(
            Command::new(&script),
            Duration::from_millis(200),
            &serial,
            &trace,
        )
        .unwrap();

        assert!(observed.output.timed_out);
        assert!(!observed.output.cancelled);
        let mut report = BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(
            &mut report,
            &read_evidence(&serial).unwrap(),
            observed.output.timed_out,
            1,
        );
        assert!(!report.is_success());
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("deadline")));

        let late_script = scratch.path().join("fake-qemu-late-proof");
        let late_serial = scratch.path().join("late-serial.log");
        let late_trace = scratch.path().join("late-exceptions.log");
        std::fs::write(&late_serial, valid_jit_serial()).unwrap();
        std::fs::write(&late_trace, "").unwrap();
        std::fs::write(&late_script, "#!/bin/sh\nexec sleep 5\n").unwrap();
        std::fs::set_permissions(&late_script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let late = run_controlled_qemu_until_jit_evidence(
            Command::new(&late_script),
            Duration::ZERO,
            &late_serial,
            &late_trace,
        )
        .unwrap();
        assert!(late.output.timed_out);
        let mut late_report = BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(
            &mut late_report,
            &read_evidence(&late_serial).unwrap(),
            late.output.timed_out,
            0,
        );
        assert!(late_report.llvmpipe_jit_proof.is_some());
        assert!(!late_report.is_success());
    }

    #[cfg(unix)]
    #[test]
    fn strict_iso_runner_cancels_on_probe_failure_and_early_exit_stays_distinct() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("fake-qemu-failure");
        let serial = scratch.path().join("failure-serial.log");
        let trace = scratch.path().join("failure-exceptions.log");
        std::fs::write(&serial, "").unwrap();
        std::fs::write(&trace, "").unwrap();
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '[llvmpipe-jit] FAIL: shader compilation failed' > \"$FAKE_SERIAL\"\nexec sleep 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(&script);
        command.env("FAKE_SERIAL", &serial);
        let observed = run_controlled_qemu_until_jit_evidence(
            command,
            Duration::from_secs(3),
            &serial,
            &trace,
        )
        .unwrap();
        assert!(observed.output.cancelled);
        assert!(!observed.output.timed_out);
        let failure_serial = read_evidence(&serial).unwrap();
        assert!(failure_serial.contains("[llvmpipe-jit] FAIL"));
        let mut failure_report = BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(
            &mut failure_report,
            &failure_serial,
            observed.output.timed_out,
            3,
        );
        assert!(!failure_report.is_success());

        let exit_script = scratch.path().join("fake-qemu-early-exit");
        let exit_serial = scratch.path().join("exit-serial.log");
        let exit_trace = scratch.path().join("exit-exceptions.log");
        std::fs::write(&exit_serial, "").unwrap();
        std::fs::write(&exit_trace, "").unwrap();
        std::fs::write(&exit_script, "#!/bin/sh\nexit 23\n").unwrap();
        std::fs::set_permissions(&exit_script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let exited = run_controlled_qemu_until_jit_evidence(
            Command::new(&exit_script),
            Duration::from_secs(3),
            &exit_serial,
            &exit_trace,
        )
        .unwrap();
        assert!(!exited.output.cancelled);
        assert!(!exited.output.timed_out);
        assert_eq!(exited.output.status.code(), Some(23));
        let mut exit_report = BootReport {
            require_llvmpipe_jit: true,
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(
            &mut exit_report,
            &read_evidence(&exit_serial).unwrap(),
            exited.output.timed_out,
            3,
        );
        assert!(!exit_report.is_success());
    }

    #[cfg(unix)]
    #[test]
    fn strict_iso_runner_rechecks_faults_after_reaping_even_with_a_complete_proof() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = tempfile::tempdir().unwrap();
        let script = scratch.path().join("fake-qemu-fault-after-proof");
        let serial = scratch.path().join("serial.log");
        let trace = scratch.path().join("exceptions.log");
        std::fs::write(&serial, "").unwrap();
        std::fs::write(&trace, "").unwrap();
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '0: v=0e e=0000 IP=0008:0000000000001234 cpl=3' > \"$FAKE_TRACE\"\nprintf '%s' \"$FAKE_SERIAL_CONTENT\" > \"$FAKE_SERIAL\"\nexec sleep 5\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(&script);
        command
            .env("FAKE_SERIAL", &serial)
            .env("FAKE_TRACE", &trace)
            .env("FAKE_SERIAL_CONTENT", valid_jit_serial());
        let observed = run_controlled_qemu_until_jit_evidence(
            command,
            Duration::from_secs(3),
            &serial,
            &trace,
        )
        .unwrap();
        assert!(observed.output.cancelled);
        assert!(!observed.output.timed_out);

        let serial_text = read_evidence(&serial).unwrap();
        let trace_text = read_evidence(&trace).unwrap();
        let mut report = BootReport {
            require_llvmpipe_jit: true,
            faults: read_faults(&trace_text),
            ..BootReport::default()
        };
        apply_llvmpipe_jit_observation(&mut report, &serial_text, observed.output.timed_out, 3);
        assert!(report.llvmpipe_jit_proof.is_some());
        assert_eq!(report.faults.len(), 1);
        assert!(!report.is_success());
    }

    fn valid_jit_serial() -> String {
        "[llvmpipe-jit] GL_RENDERER: llvmpipe (LLVM 11.0.0, 256 bits)\n\
         [llvmpipe-mcjit] function=fs_variant_whole address=FFEEDDCCBBAA9988\n\
         [llvmpipe-jit] center RGBA: 64 128 191 255; expected 64 128 191 255 +/- 8\n\
         === LLVMPipe LLVM 11 GLSL/JIT PROBE PASS ===\n\
         === LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===\n"
            .to_owned()
    }
}
