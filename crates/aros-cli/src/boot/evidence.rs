//! Parse boot logs and resolve exception addresses against the bootstrap load layout.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use miette::{miette, Result};

use super::{BootReport, Fault, LlvmPipeJitProof, Milestone};

pub(super) fn apply_llvmpipe_jit_observation(
    report: &mut BootReport,
    serial: &str,
    timed_out: bool,
    timeout_seconds: u64,
) {
    match parse_llvmpipe_jit_proof(serial) {
        Ok(proof) => report.llvmpipe_jit_proof = Some(proof),
        Err(reason) => report.failures.push(format!(
            "required llvmpipe JIT proof missing or invalid: {reason}"
        )),
    }
    if timed_out {
        report.failures.push(format!(
            "QEMU exceeded the {timeout_seconds} second deadline; the llvmpipe JIT proof must be observed before timeout"
        ));
    }
}

pub(super) fn parse_llvmpipe_jit_proof(
    serial: &str,
) -> std::result::Result<LlvmPipeJitProof, String> {
    const EXPECTED_PIXEL: [u8; 4] = [64, 128, 191, 255];
    const PASS_MARKER: &str = "=== LLVMPipe LLVM 11 GLSL/JIT PROBE PASS ===";
    const FAIL_MARKER: &str = "=== LLVMPipe LLVM 11 GLSL/JIT PROBE FAIL ===";

    let lines = serial.lines().map(str::trim).collect::<Vec<_>>();
    if lines
        .iter()
        .any(|line| line.contains("[llvmpipe-jit] FAIL") || *line == FAIL_MARKER)
    {
        return Err("the serial log contains a llvmpipe probe FAIL marker".to_owned());
    }

    let renderer_lines = lines
        .iter()
        .filter_map(|line| line.strip_prefix("[llvmpipe-jit] GL_RENDERER:"))
        .map(str::trim)
        .collect::<Vec<_>>();
    if renderer_lines.is_empty()
        || renderer_lines
            .iter()
            .any(|renderer| !renderer.contains("llvmpipe") || !is_llvm_11_0_0(renderer))
    {
        return Err(
            "missing a GL_RENDERER line containing llvmpipe and the exact LLVM 11.0.0 version"
                .to_owned(),
        );
    }

    let mut pixel_readbacks = Vec::new();
    for line in &lines {
        let Some(values) = line.strip_prefix("[llvmpipe-jit] center RGBA:") else {
            continue;
        };
        let channels = values
            .split(';')
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::parse::<u8>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| "the center RGBA line contains an invalid channel".to_owned())?;
        let pixel: [u8; 4] = channels
            .try_into()
            .map_err(|_| "the center RGBA line does not contain four channels".to_owned())?;
        pixel_readbacks.push(pixel);
    }
    if pixel_readbacks.is_empty() {
        return Err("missing the center RGBA readback from the probe".to_owned());
    }
    if pixel_readbacks.iter().any(|pixel| {
        pixel
            .iter()
            .zip(EXPECTED_PIXEL)
            .any(|(actual, expected)| actual.abs_diff(expected) > 8)
    }) {
        return Err("the center RGBA readback is outside the probe's +/- 8 tolerance".to_owned());
    }

    let mut named_symbol = false;
    let mut valid_symbol = None;
    for line in &lines {
        let Some(fields) = line.strip_prefix("[llvmpipe-mcjit] ") else {
            continue;
        };
        let mut function = None;
        let mut address = None;
        for field in fields.split_whitespace() {
            if let Some((key, value)) = field.split_once('=') {
                match key {
                    "function" => function = Some(value),
                    "address" => address = Some(value),
                    _ => {}
                }
            }
        }
        let Some(function @ ("fs_variant_whole" | "fs_variant_partial")) = function else {
            continue;
        };
        named_symbol = true;
        if let Some(address) = address.filter(|address| is_nonzero_hex_pointer(address)) {
            valid_symbol = Some((function.to_owned(), address.to_owned()));
            break;
        }
    }
    let (function, address) = valid_symbol.ok_or_else(|| {
        if named_symbol {
            "the named llvmpipe MCJIT function has no nonzero hexadecimal address".to_owned()
        } else {
            "missing a named fs_variant_whole or fs_variant_partial MCJIT address".to_owned()
        }
    })?;

    let pass_line = lines
        .iter()
        .position(|line| *line == PASS_MARKER)
        .ok_or_else(|| {
            "missing the exact llvmpipe LLVM 11 GLSL/JIT probe PASS marker".to_owned()
        })?;
    if !lines.iter().enumerate().any(|(index, line)| {
        index > pass_line && *line == "=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ==="
    }) {
        return Err("missing successful probe return and ELF unload evidence".to_owned());
    }

    Ok(LlvmPipeJitProof {
        renderer: renderer_lines[0].to_owned(),
        function,
        address,
        pixel: pixel_readbacks[0],
    })
}

pub(super) fn is_nonzero_hex_pointer(address: &str) -> bool {
    let digits = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .unwrap_or(address);
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    u64::from_str_radix(digits, 16).is_ok_and(|value| value != 0)
}

fn is_llvm_11_0_0(renderer: &str) -> bool {
    renderer
        .match_indices("LLVM 11.0.0")
        .any(|(start, version)| {
            let after = start + version.len();
            renderer[after..].chars().next().is_none_or(|character| {
                character.is_ascii_whitespace() || matches!(character, ',' | ')')
            })
        })
}

pub(super) fn furthest_milestone(serial: &str, trace: &str) -> Option<Milestone> {
    let mut reached = None;
    for milestone in Milestone::ALL {
        // cpl=3 in an exception record is the only positive evidence of user
        // mode available without a debugger: the kernel prints nothing when
        // it drops privileges.
        let proved = milestone
            .serial_marker()
            .map_or_else(|| trace.contains("cpl=3"), |marker| serial.contains(marker));
        if proved {
            reached = Some(milestone);
        }
    }
    reached
}

/// Statements the logs make about the boot failing.
pub(super) fn read_failures(serial: &str, trace: &str) -> Vec<String> {
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
        if let Some(reason) = line.strip_prefix("[Kernel:TLSF] free-list corruption at ") {
            out.push(format!(
                "the kernel detected allocator corruption at {reason}"
            ));
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
/// `int 0xfe` from KrnSchedule, KrnSwitch and Supervisor. QEMU prefixes a
/// hardware interrupt record with `Servicing hardware INT=...`. Neither is a
/// CPU fault; counting them reports a working interrupt path as a defect.
pub(super) fn read_faults(trace: &str) -> Vec<Fault> {
    let mut seen: BTreeMap<(u8, u8, u64), usize> = BTreeMap::new();
    let mut next_record_is_hardware = false;
    for line in trace.lines() {
        if line.trim_start().starts_with("Servicing hardware INT=") {
            next_record_is_hardware = true;
            continue;
        }
        let Some(at) = line.find(" v=") else { continue };
        if std::mem::take(&mut next_record_is_hardware) {
            continue;
        }
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
pub(super) fn bootstrap_pointer_width(bootstrap: &Path) -> u64 {
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
pub(super) fn package_members(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
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
    let delta = match found.cmp(&computed) {
        std::cmp::Ordering::Greater => i64::try_from(found - computed).unwrap_or(i64::MAX),
        std::cmp::Ordering::Less => -i64::try_from(computed - found).unwrap_or(i64::MAX),
        std::cmp::Ordering::Equal => 0,
    };
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
pub(super) fn locate(
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
    let (found, how) = located_by_bytes(packed, traced, fault.ip).map_or_else(
        || {
            fault.ip.checked_sub(base).map_or_else(
                || (None, String::new()),
                |computed| {
                    let (offset, delta) = corrected_offset(packed, traced, fault.ip, computed);
                    let how = delta.map_or_else(
                        || "by arithmetic alone; its bytes are not unique nearby".to_owned(),
                        |delta| {
                            if delta == 0 {
                                "by arithmetic, confirmed by its bytes".to_owned()
                            } else {
                                format!(
                                "by its bytes, {delta:+#x} from where the load model computed it"
                            )
                            }
                        },
                    );
                    (Some(offset), how)
                },
            )
        },
        |offset| (Some(offset), "by its bytes".to_owned()),
    );
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
pub(super) fn traced_block(line: &str) -> Option<(u64, Vec<u8>)> {
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
