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
        match locate(
            &kickstart,
            &bootstrap,
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
    let ram = request.evidence.join("guest-ram.bin");
    let list = modules
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
            *undefined
                .entry(rest.trim_matches('\'').to_owned())
                .or_default() += 1;
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

/// `sizeof(void *)` in the bootstrap that will do the loading.
///
/// Read from the bootstrap's own ELF class rather than assumed, because the two
/// widths differ on PC: 32-bit loader code, 64-bit structures.
fn bootstrap_pointer_width(bootstrap: &Path) -> u64 {
    std::fs::read(bootstrap).map_or(8, |bytes| if bytes.get(4) == Some(&1) { 4 } else { 8 })
}

/// One image the loader places: the kickstart, or one member of a package.
struct Image {
    name: String,
    bytes: Vec<u8>,
    object: aros_common::elf::Object,
}

/// Where one section of one image ended up in the shared read-only block.
struct Placement {
    image: usize,
    section: String,
    section_index: u16,
    start: u64,
    size: u64,
}

/// The members of a `PKG\x01` archive, in package order.
///
/// The format is `arch/all-pc/bootstrap/bootstrap.c:315`: an eight-byte header,
/// then per member a big-endian name length, the name and its terminator, a
/// big-endian image length, and the image.
fn package_members(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut at = 8usize;
    while at + 4 <= bytes.len() {
        let name_len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let name_start = at + 4;
        let name_end = name_start + name_len;
        if name_end + 4 > bytes.len() {
            break;
        }
        // The declared length is the field width, and it is not consistent
        // about the terminator: in one package the first member declares 19 for
        // an 18-character name, the next declares 15 for 15 characters. The
        // loader does not care, because `__bs_remove_path(file + 4)` reads a C
        // string; so the name ends at the first NUL, while the field length
        // still drives the skip below. That distinction also decides the
        // descriptor size, which uses `strlen(Name) + 1`.
        let field = &bytes[name_start..name_end];
        let name = String::from_utf8_lossy(
            &field[..field
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(field.len())],
        )
        .into_owned();
        // `file += 5 + len` skips the terminator the length does not count.
        let size_at = at + 5 + name_len;
        if size_at + 4 > bytes.len() {
            break;
        }
        let image_len =
            u32::from_be_bytes(bytes[size_at..size_at + 4].try_into().unwrap()) as usize;
        let image_start = size_at + 4;
        let image_end = image_start + image_len;
        if image_end > bytes.len() {
            break;
        }
        // The loader keeps the basename only (__bs_remove_path).
        let name = name.rsplit('/').next().unwrap_or(&name).to_owned();
        found.push((name, bytes[image_start..image_end].to_vec()));
        at = image_end;
    }
    found
}

/// Every image the loader will place, in the order it places them.
///
/// The kickstart first, then each multiboot module: a bare ELF as one image, a
/// package as one image per member. A name already seen is skipped, which is
/// what `module_prepare` (bootstrap.c:177) does -- "if some file is specified in
/// both PKG file and list of separate modules, the copy in PKG will be skipped".
fn images(kickstart: &Path, modules: &[PathBuf]) -> Result<Vec<Image>> {
    let mut raw: Vec<(String, Vec<u8>)> = Vec::new();
    let kickstart_bytes = std::fs::read(kickstart)
        .map_err(|error| miette!("cannot read {}: {error}", kickstart.display()))?;
    raw.push(("Kickstart ELF".to_owned(), kickstart_bytes));
    for path in modules {
        let bytes = std::fs::read(path)
            .map_err(|error| miette!("cannot read {}: {error}", path.display()))?;
        if bytes.starts_with(b"\x7fELF") {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            raw.push((name, bytes));
        } else if bytes.starts_with(b"PKG\x01") {
            raw.extend(package_members(&bytes));
        }
        // Anything else the loader ignores too, and says so itself.
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (name, bytes) in raw {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Ok(object) = aros_common::elf::read(&bytes) else {
            continue;
        };
        out.push(Image {
            name,
            bytes,
            object,
        });
    }
    Ok(out)
}

/// How many bytes the loader spends on one image's debug descriptor.
///
/// After an image's sections, `LoadKernel` (bootstrap/elfloader.c:702) advances
/// the read-only pointer by `(p + sizeof(void*)) & ~(sizeof(void*) - 1)` -- note
/// that this moves an already-aligned pointer on by a full word -- then writes
/// the module descriptor, the ELF header, the section header table and the name
/// with its terminator, none of which are aligned individually.
fn descriptor_bytes(image: &Image, bootstrap_word: u64) -> (u64, u64) {
    let sixty_four = matches!(image.object.class, aros_common::elf::Class::Elf64);
    // The alignment step is `sizeof(void *)` in the *bootstrap*, not in the
    // module: on PC the bootstrap is 32-bit code building 64-bit structures
    // (it links gen/lib32/libbootstrap.a), so it advances by 4 while the
    // descriptor it writes is the 64-bit one. Assuming the module's own width
    // here put every module after the first out by 4, growing to 80 bytes by
    // the fortieth -- see OPEN-POINTS 49 for how that was measured.
    let word: u64 = bootstrap_word;
    // struct ELF_ModuleInfo_t: Next, Name, Type, Pad0, [Pad1], eh, sh.
    let descriptor: u64 = if sixty_four { 40 } else { 20 };
    let header: u64 = if sixty_four { 64 } else { 52 };
    let (shentsize, shnum) = section_header_shape(&image.bytes, sixty_four);
    (
        word,
        descriptor + header + u64::from(shentsize) * u64::from(shnum) + image.name.len() as u64 + 1,
    )
}

/// `e_shentsize` and `e_shnum`, read from the file rather than recomputed: the
/// loader copies exactly `shnum * shentsize` bytes of section header.
fn section_header_shape(bytes: &[u8], sixty_four: bool) -> (u16, u16) {
    let at = if sixty_four { 0x3a } else { 0x2e };
    let read = |offset: usize| -> u16 {
        bytes
            .get(offset..offset + 2)
            .map_or(0, |slice| u16::from_le_bytes(slice.try_into().unwrap()))
    };
    (read(at), read(at + 2))
}

/// The shared read-only block, packed the way the loader packs it.
///
/// Every image contributes its non-writable allocated sections plus its string
/// and symbol tables, in section-index order, each aligned to its own
/// `sh_addralign`; then the loader's per-image debug descriptor advances the
/// pointer further. There is one block for all images, not one per image, which
/// is why an address in a package module can be resolved at all.
///
/// The bytes matter as well as the offsets: the load base is derived by finding
/// traced instruction bytes in this image, so the descriptor gaps are filled
/// with zeroes rather than skipped.
fn place_readonly(images: &[Image], bootstrap_word: u64) -> (Vec<Placement>, Vec<u8>) {
    let mut packed: Vec<u8> = Vec::new();
    let mut placed = Vec::new();
    for (index, image) in images.iter().enumerate() {
        for section in &image.object.sections {
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
                match image.bytes.get(from..to) {
                    Some(bytes) => packed.extend_from_slice(bytes),
                    None => packed.extend(std::iter::repeat_n(0u8, section.size as usize)),
                }
            }
            placed.push(Placement {
                image: index,
                section: section.name.clone(),
                section_index: section.index,
                start,
                size: section.size,
            });
        }
        let (word, descriptor) = descriptor_bytes(image, bootstrap_word);
        let aligned = (packed.len() as u64 + word) & !(word - 1);
        packed.extend(std::iter::repeat_n(
            0u8,
            (aligned - packed.len() as u64) as usize,
        ));
        packed.extend(std::iter::repeat_n(0u8, descriptor as usize));
    }
    (placed, packed)
}

/// Every traced instruction, by address.
///
/// The trace prints one instruction per line, so consecutive entries can be
/// stitched back into a run of bytes long enough to be unique in the image.
fn traced_instructions(asm: &str) -> BTreeMap<u64, Vec<u8>> {
    let mut found = BTreeMap::new();
    for line in asm.lines() {
        if let Some((address, bytes)) = traced_block(line) {
            if !bytes.is_empty() {
                found.entry(address).or_insert(bytes);
            }
        }
    }
    found
}

/// A run of at least `want` bytes starting at `address`, stitched from
/// consecutive traced instructions.
fn stitched_from(traced: &BTreeMap<u64, Vec<u8>>, address: u64, want: usize) -> Option<Vec<u8>> {
    let mut at = address;
    let mut run = Vec::new();
    while run.len() < want {
        let bytes = traced.get(&at)?;
        run.extend_from_slice(bytes);
        at += bytes.len() as u64;
    }
    Some(run)
}

/// A run of traced bytes that *ends* with the instruction at `ip`, and the
/// distance from the run's start to `ip`.
///
/// Needed because the faulting instruction is usually the last one traced --
/// nothing after it executed -- so a forward run from the fault has only those
/// few bytes to be unique with. Runs are tried shortest first: the packed image
/// holds unrelocated bytes, so a longer run is more likely to reach back into an
/// instruction carrying an absolute address that the loader filled in later, and
/// such a run cannot match at all.
fn runs_ending_at(traced: &BTreeMap<u64, Vec<u8>>, ip: u64) -> Vec<(Vec<u8>, u64)> {
    let Some(at_fault) = traced.get(&ip) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut start = ip;
    let mut prefix: Vec<u8> = Vec::new();
    // Walk back over instructions that abut, newest first.
    for (&address, bytes) in traced.range(..ip).rev().take(8) {
        if address + bytes.len() as u64 != start {
            break;
        }
        let mut run = bytes.clone();
        run.extend_from_slice(&prefix);
        run.extend_from_slice(at_fault);
        prefix = {
            let mut carried = bytes.clone();
            carried.extend_from_slice(&prefix);
            carried
        };
        start = address;
        out.push((run, ip - address));
    }
    out
}

/// The offset the arithmetic gives, corrected by the fault's own bytes.
///
/// The arithmetic models the loader's packing, and the model can be off: the
/// first version of it put this fault 0x50 past the truth and named a
/// neighbouring function without hesitating. A global byte search cannot always
/// settle it either, because the packed image holds *unrelocated* bytes -- the
/// unique part of a library-base stub is the absolute address the loader fills
/// in later, and what remains (`movq (%r11), %r11; jmpq *-<lvo>(%r11)`) occurs
/// once per module that calls into the same library.
///
/// So: start from the arithmetic, then look for the faulting instruction near
/// it. One match in the window is the answer, and the distance from the computed
/// offset is reported, because a non-zero distance is a defect in the model
/// rather than a detail.
fn corrected_offset(
    packed: &[u8],
    traced: &BTreeMap<u64, Vec<u8>>,
    ip: u64,
    computed: u64,
) -> (u64, Option<i64>) {
    const WINDOW: u64 = 1 << 16;
    let Some(bytes) = traced.get(&ip) else {
        return (computed, None);
    };
    if bytes.len() < 4 {
        return (computed, None);
    }
    let low = computed.saturating_sub(WINDOW) as usize;
    let high = ((computed + WINDOW) as usize).min(packed.len());
    if low >= high {
        return (computed, None);
    }
    let window = &packed[low..high];
    let mut hits = Vec::new();
    let mut at = 0usize;
    while let Some(index) = find_subslice(&window[at..], bytes) {
        hits.push(at + index);
        at += index + 1;
        if hits.len() > 1 {
            break;
        }
    }
    if hits.len() != 1 {
        return (computed, None);
    }
    let found = low as u64 + hits[0] as u64;
    let delta = found as i64 - computed as i64;
    (found, Some(delta))
}

/// Where a faulting address really is, found by its own bytes.
///
/// The address arithmetic below models the loader's packing, and a model can be
/// wrong: the first version of it put this fault 0x50 past the truth and named a
/// neighbouring function with complete confidence. The bytes cannot be wrong in
/// that way. When the instruction run at the fault occurs exactly once in the
/// packed image, its offset is the answer and no arithmetic is involved.
fn located_by_bytes(packed: &[u8], traced: &BTreeMap<u64, Vec<u8>>, ip: u64) -> Option<u64> {
    // Forward first, for a fault that was executed past.
    for want in [24usize, 16, 12, 8] {
        let Some(run) = stitched_from(traced, ip, want) else {
            continue;
        };
        if count_occurrences(packed, &run) == 1 {
            return find_subslice(packed, &run).map(|offset| offset as u64);
        }
    }
    // Then runs ending at the fault, which is the usual case.
    for (run, lead) in runs_ending_at(traced, ip) {
        if run.len() < 8 {
            continue;
        }
        if count_occurrences(packed, &run) == 1 {
            return find_subslice(packed, &run).map(|offset| offset as u64 + lead);
        }
    }
    None
}

/// Turns each fault's address into `<module> <section>+<offset> = <symbol>+<offset>`.
///
/// The load base is derived from the instruction trace rather than assumed: for
/// every traced block whose bytes occur exactly once in the packed image, the
/// address minus that offset is a candidate, and the majority wins. Deriving it
/// by hand is what went wrong before -- one attempt was 0x80 out, which named
/// the wrong function with complete confidence.
fn locate(
    kickstart: &Path,
    bootstrap: &Path,
    modules: &[PathBuf],
    asm: &str,
    faults: &[Fault],
) -> Result<Vec<String>> {
    let images = images(kickstart, modules)?;
    let bootstrap_word = bootstrap_pointer_width(bootstrap);
    let (placed, packed) = place_readonly(&images, bootstrap_word);

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

    let traced = traced_instructions(asm);
    let mut out = vec![format!(
        "read-only block loaded at {base:#x} ({agree} traced blocks agree, \
         {} images modelled)",
        images.len()
    )];
    for fault in faults {
        out.push(describe(fault, base, &placed, &images, &packed, &traced));
    }
    Ok(out)
}

fn describe(
    fault: &Fault,
    base: u64,
    placed: &[Placement],
    images: &[Image],
    packed: &[u8],
    traced: &BTreeMap<u64, Vec<u8>>,
) -> String {
    // The bytes first, the arithmetic only as a fallback: a wrong layout model
    // names a neighbouring function without hesitating, and this one did.
    let (found, how) = match located_by_bytes(packed, traced, fault.ip) {
        Some(offset) => (Some(offset), "by its bytes".to_owned()),
        None => match fault.ip.checked_sub(base) {
            None => (None, String::new()),
            Some(computed) => {
                let (offset, delta) = corrected_offset(packed, traced, fault.ip, computed);
                let how = match delta {
                    None => "by arithmetic alone; its bytes are not unique nearby".to_owned(),
                    Some(0) => "by arithmetic, confirmed by its bytes".to_owned(),
                    Some(delta) => {
                        format!("by its bytes, {delta:+#x} from where the load model computed it")
                    }
                };
                (Some(offset), how)
            }
        },
    };
    let Some(offset_in_block) = found else {
        return format!(
            "v={:02x} cpl={} IP={:#x}: below the load base, so not in the read-only block",
            fault.vector, fault.cpl, fault.ip
        );
    };
    let Some(place) = placed
        .iter()
        .find(|place| offset_in_block >= place.start && offset_in_block < place.start + place.size)
    else {
        // Every image the loader was given is modelled, so an address outside
        // all of them is in a writable block -- which this does not model,
        // because a faulting instruction pointer is in code.
        return format!(
            "v={:02x} cpl={} IP={:#x}: outside every modelled read-only section, \
             so in a writable block",
            fault.vector, fault.cpl, fault.ip
        );
    };
    let image = &images[place.image];
    let offset = offset_in_block - place.start;
    let symbol = image
        .object
        .symbols
        .iter()
        .filter(|symbol| symbol.home == aros_common::elf::Home::Section(place.section_index))
        .filter(|symbol| symbol.value <= offset && offset < symbol.value + symbol.size.max(1))
        .min_by_key(|symbol| symbol.size);
    let mut text = format!(
        "v={:02x} cpl={} IP={:#x} = {} {}+{offset:#x} ({how})",
        fault.vector, fault.cpl, fault.ip, image.name, place.section
    );
    if let Some(symbol) = symbol {
        let _ = write!(text, " = {}+{:#x}", symbol.name, offset - symbol.value);
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
