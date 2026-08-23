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

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use miette::{miette, Result};

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
    /// Where the evidence is.
    pub evidence: PathBuf,
}

impl BootReport {
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failures.is_empty() && self.faults.is_empty()
    }
}

/// What to boot and for how long.
#[derive(Debug, Clone)]
pub struct BootRequest {
    pub build_dir: PathBuf,
    /// Multiboot modules. Empty means the kickstart alone.
    pub modules: Vec<PathBuf>,
    pub seconds: u64,
    pub evidence: PathBuf,
    pub memory_mb: u32,
}

/// Reads the serial log and the exception trace, and says what happened.
pub fn check(request: &BootRequest) -> Result<BootReport> {
    let boot = request.build_dir.join("SYS/boot");
    let bootstrap = boot.join("pc/bootstrap");
    let kickstart = boot.join("pc/kernel");
    for path in [&bootstrap, &kickstart] {
        if !path.exists() {
            return Err(miette!("missing {}", path.display()));
        }
    }

    std::fs::create_dir_all(&request.evidence)
        .map_err(|error| miette!("cannot create {}: {error}", request.evidence.display()))?;

    let mut modules = vec![kickstart.clone()];
    modules.extend(request.modules.iter().cloned());
    let mut report = BootReport {
        evidence: request.evidence.clone(),
        ..BootReport::default()
    };
    if request.modules.is_empty() {
        report.untested.push(
            "only the kickstart was passed as a multiboot module, so nothing in a \
             package was loaded"
                .to_owned(),
        );
    }

    let serial = request.evidence.join("serial.log");
    let trace = request.evidence.join("exceptions.log");
    run_qemu(request, &bootstrap, &modules, &serial, &trace, false)?;

    let serial_text = read_lossy(&serial);
    let trace_text = read_lossy(&trace);
    report.reached = furthest_milestone(&serial_text, &trace_text);
    report.failures = read_failures(&serial_text, &trace_text);
    report.faults = read_faults(&trace_text);

    // The second pass exists only to name the fault. An instruction trace is
    // large and slows the guest, so it is not paid for on a clean boot.
    if !report.faults.is_empty() {
        let asm = request.evidence.join("instructions.log");
        run_qemu(request, &bootstrap, &modules, &serial, &asm, true)?;
        let asm_text = read_lossy(&asm);
        match locate(&kickstart, &asm_text, &report.faults) {
            Ok(lines) => report.resolved = lines,
            Err(error) => report
                .untested
                .push(format!("the faulting address could not be resolved: {error}")),
        }
    }

    Ok(report)
}

fn read_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .replace('\u{0}', "")
                .to_string()
        })
        .unwrap_or_default()
}

/// Runs QEMU once, deterministically.
fn run_qemu(
    request: &BootRequest,
    bootstrap: &Path,
    modules: &[PathBuf],
    serial: &Path,
    trace: &Path,
    instructions: bool,
) -> Result<()> {
    let qemu = which::which("qemu-system-x86_64")
        .map_err(|_| miette!("qemu-system-x86_64 is not on PATH"))?;
    let list = modules
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(",");

    let mut command = Command::new(qemu);
    command
        // Fixed machine, so two runs are comparable. -no-reboot turns a triple
        // fault into an exit instead of an endless loop.
        .args(["-machine", "q35"])
        .args(["-cpu", "qemu64,+avx2"])
        .args(["-smp", "1"])
        .args(["-m", &request.memory_mb.to_string()])
        .args(["-rtc", "base=2020-01-01T00:00:00"])
        .arg("-no-reboot")
        .args(["-display", "none"])
        // Without the leading space this does nothing: the boot console reads
        // `strstr(cmdline, " debug")` (arch/all-native/bootconsole/common.c:79).
        .args(["-append", " debug=serial"])
        .arg("-serial")
        .arg(format!("file:{}", serial.display()))
        .arg("-kernel")
        .arg(bootstrap)
        .arg("-initrd")
        .arg(&list)
        .args(["-d", if instructions { "in_asm,int" } else { "int" }])
        .arg("-D")
        .arg(trace);

    let mut child = command
        .spawn()
        .map_err(|error| miette!("cannot start qemu: {error}"))?;
    // A guest that resets exits on its own; one that keeps running is stopped
    // when its time is up. Either way the logs are what is read afterwards.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(request.seconds);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(miette!("cannot wait for qemu: {error}")),
        }
    }
    Ok(())
}

/// The furthest milestone the evidence proves.
fn furthest_milestone(serial: &str, trace: &str) -> Option<Milestone> {
    let mut reached = None;
    for milestone in Milestone::ALL {
        let proved = match milestone.serial_marker() {
            Some(marker) => serial.contains(marker),
            // cpl=3 in an exception record is the only positive evidence of user
            // mode available without a debugger: the kernel prints nothing when
            // it drops privileges.
            None => trace.contains("cpl=3"),
        };
        if proved {
            reached = Some(milestone);
        }
    }
    reached
}

/// Statements the logs make about the boot failing.
fn read_failures(serial: &str, trace: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut undefined: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = serial.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[ELF Loader] Undefined symbol ") {
            *undefined.entry(rest.trim_matches('\'').to_owned()).or_default() += 1;
            continue;
        }
        if line.contains("Relocation error in section") {
            out.push(format!("the loader refused a module: {line}"));
            continue;
        }
        if line.contains("*** SYSTEM PANIC!!! ***") {
            out.push("the bootstrap panicked".to_owned());
            continue;
        }
        if line.contains("Critical boot failure") {
            // The reason is inside the box the kernel draws, on the next lines.
            let mut reason = String::new();
            while let Some(next) = lines.peek() {
                let text: String = next
                    .chars()
                    .filter(|character| character.is_ascii_graphic() || *character == ' ')
                    .collect();
                let text = text.trim().to_owned();
                lines.next();
                if text.is_empty() {
                    break;
                }
                if !reason.is_empty() {
                    reason.push_str(" / ");
                }
                reason.push_str(&text);
                if reason.len() > 200 {
                    break;
                }
            }
            out.push(format!("the kernel panicked: {reason}"));
            continue;
        }
        if let Some(rest) = line.strip_prefix("Exec Bootstrap Task: ") {
            out.push(format!("the boot task reported: {rest}"));
        }
    }
    for (symbol, count) in undefined {
        out.push(format!(
            "the loader found no definition of {symbol}{}",
            if count > 1 {
                format!(" ({count} times)")
            } else {
                String::new()
            }
        ));
    }
    if trace.contains("check_exception") {
        out.push(
            "an exception was taken while delivering another, so the guest \
             double-faulted"
                .to_owned(),
        );
    }
    out
}

/// Exception records, collapsed by (vector, address).
///
/// `i=1` marks a *software* interrupt, and AROS uses one as its supervisor entry:
/// `int 0xfe` from KrnSchedule, KrnSwitch and Supervisor. Counting those as
/// faults reported the system-call mechanism as a defect, five times per run.
fn read_faults(trace: &str) -> Vec<Fault> {
    let mut seen: BTreeMap<(u8, u8, u64), usize> = BTreeMap::new();
    for line in trace.lines() {
        let Some(at) = line.find(" v=") else { continue };
        let rest = &line[at + 3..];
        let Some(vector) = rest.get(..2).and_then(|v| u8::from_str_radix(v, 16).ok()) else {
            continue;
        };
        if line.contains(" i=1 ") {
            continue;
        }
        let cpl = line
            .find("cpl=")
            .and_then(|at| line[at + 4..].chars().next())
            .and_then(|character| character.to_digit(10))
            .unwrap_or(0) as u8;
        let ip = line
            .find("IP=")
            .and_then(|at| line[at + 3..].split_whitespace().next())
            .and_then(|field| field.rsplit(':').next())
            .and_then(|value| u64::from_str_radix(value, 16).ok())
            .unwrap_or(0);
        *seen.entry((vector, cpl, ip)).or_default() += 1;
    }
    seen.into_iter()
        .map(|((vector, cpl, ip), count)| Fault {
            vector,
            cpl,
            ip,
            count,
        })
        .collect()
}

/// The kickstart's read-only block as the bootstrap's loader lays it out.
///
/// `bootstrap/elfloader.c` walks the section headers in index order and packs
/// every allocatable one, plus the string and symbol tables, honouring each
/// section's own alignment. Writable sections go to a second block, which is not
/// modelled here: a faulting instruction pointer is in code.
fn packed_readonly(object: &aros_common::elf::Object, file: &[u8]) -> (Vec<(String, u64, u64)>, Vec<u8>) {
    let mut packed: Vec<u8> = Vec::new();
    let mut placed = Vec::new();
    for section in &object.sections {
        if section.size == 0 {
            continue;
        }
        let carried = section.is_alloc()
            || section.kind == aros_common::elf::SHT_STRTAB
            || section.kind == aros_common::elf::SHT_SYMTAB;
        if !carried || section.is_write() {
            continue;
        }
        let align = if section.align == 0 { 1 } else { section.align };
        let pad = (align - (packed.len() as u64 % align)) % align;
        packed.extend(std::iter::repeat_n(0u8, pad as usize));
        let start = packed.len() as u64;
        if section.is_nobits() {
            packed.extend(std::iter::repeat_n(0u8, section.size as usize));
        } else {
            let from = section.offset as usize;
            let to = from + section.size as usize;
            match file.get(from..to) {
                Some(bytes) => packed.extend_from_slice(bytes),
                None => packed.extend(std::iter::repeat_n(0u8, section.size as usize)),
            }
        }
        placed.push((section.name.clone(), start, section.size));
    }
    (placed, packed)
}

/// Turns each fault's address into `<section>+<offset> = <symbol>+<offset>`.
///
/// The load base is derived from the instruction trace rather than assumed: for
/// every traced block whose bytes occur exactly once in the packed image, the
/// address minus that offset is a candidate, and the majority wins. Deriving it
/// by hand is what went wrong before -- one attempt was 0x80 out, which named
/// the wrong function with complete confidence.
fn locate(kickstart: &Path, asm: &str, faults: &[Fault]) -> Result<Vec<String>> {
    let file = std::fs::read(kickstart)
        .map_err(|error| miette!("cannot read {}: {error}", kickstart.display()))?;
    let object = aros_common::elf::read(&file).map_err(|error| miette!("{error}"))?;
    let (placed, packed) = packed_readonly(&object, &file);

    let mut votes: BTreeMap<u64, usize> = BTreeMap::new();
    for line in asm.lines() {
        let Some((address, bytes)) = traced_block(line) else {
            continue;
        };
        if bytes.len() < 8 {
            continue;
        }
        if count_occurrences(&packed, &bytes) != 1 {
            continue;
        }
        let Some(offset) = find_subslice(&packed, &bytes) else {
            continue;
        };
        if address < offset as u64 {
            continue;
        }
        *votes.entry(address - offset as u64).or_default() += 1;
    }
    let Some((&base, &agree)) = votes.iter().max_by_key(|(_, count)| **count) else {
        return Err(miette!("no traced block matched the image"));
    };

    let mut out = vec![format!(
        "read-only block loaded at {base:#x} ({agree} traced blocks agree)"
    )];
    for fault in faults {
        out.push(describe(fault, base, &placed, &object));
    }
    Ok(out)
}

fn describe(
    fault: &Fault,
    base: u64,
    placed: &[(String, u64, u64)],
    object: &aros_common::elf::Object,
) -> String {
    let Some((name, start, _)) = placed
        .iter()
        .find(|(_, start, size)| fault.ip >= base + start && fault.ip < base + start + size)
    else {
        // Only the kickstart's read-only block is modelled. An address outside
        // it is either in its writable block or, more usually, inside one of the
        // package modules the loader placed after it -- each of those is its own
        // relocatable ELF and resolving into them means modelling the whole
        // load, which this does not do yet.
        return format!(
            "v={:02x} cpl={} IP={:#x}: outside the kickstart's read-only block, \
             so in its data or in a package module",
            fault.vector, fault.cpl, fault.ip
        );
    };
    let offset = fault.ip - base - start;
    let section_index = object
        .sections
        .iter()
        .find(|section| section.name == *name)
        .map(|section| section.index);
    let symbol = section_index.and_then(|index| {
        object
            .symbols
            .iter()
            .filter(|symbol| symbol.home == aros_common::elf::Home::Section(index))
            .filter(|symbol| {
                symbol.value <= offset && offset < symbol.value + symbol.size.max(1)
            })
            .min_by_key(|symbol| symbol.size)
    });
    let mut text = format!(
        "v={:02x} cpl={} IP={:#x} = {name}+{offset:#x}",
        fault.vector, fault.cpl, fault.ip
    );
    if let Some(symbol) = symbol {
        let _ = write!(
            text,
            " = {}+{:#x}",
            symbol.name,
            offset - symbol.value
        );
    } else {
        text.push_str(" (no symbol covers it)");
    }
    if fault.count > 1 {
        let _ = write!(text, ", {} times", fault.count);
    }
    text
}

/// `0x0139acac:  48 85 c0                 testq ...` from a `-d in_asm` trace.
fn traced_block(line: &str) -> Option<(u64, Vec<u8>)> {
    let rest = line.strip_prefix("0x")?;
    let (address, rest) = rest.split_once(':')?;
    let address = u64::from_str_radix(address.trim(), 16).ok()?;
    let mut bytes = Vec::new();
    for token in rest.split_whitespace() {
        if token.len() != 2 {
            break;
        }
        match u8::from_str_radix(token, 16) {
            Ok(byte) => bytes.push(byte),
            Err(_) => break,
        }
    }
    (!bytes.is_empty()).then_some((address, bytes))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// The report, as the user reads it.
#[must_use]
pub fn render(report: &BootReport) -> String {
    let mut out = String::new();
    match report.reached {
        Some(milestone) => {
            let _ = writeln!(out, "reached: {}", milestone.label());
        }
        None => out.push_str("reached: nothing; the kickstart never printed\n"),
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
            failures.iter().any(|line| line.contains("refused a module")),
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
            failures
                .iter()
                .any(|line| line.contains("APIC descriptor")),
            "{failures:?}"
        );
    }

    #[test]
    fn a_software_interrupt_is_not_a_fault() {
        // AROS enters supervisor mode with int 0xfe; that is the mechanism
        // working, not a defect.
        let trace = "     0: v=fe e=0000 i=1 cpl=3 IP=002b:00000000013a26a9 pc=x\n";
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
}
