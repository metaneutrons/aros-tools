//! A bounded, linear disassembly check for code that must remain resident in
//! internal SRAM while external flash is unavailable.
//!
//! This deliberately models the named-reference lint in
//! `arch/riscv-esp32p4/kernel/check-sramtext.sh`. It is not a control-flow or
//! whole-program data-flow proof.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

const RV32_LIMIT: u64 = 1u64 << 32;
const MAX_DISASSEMBLY_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Half-open RV32 address interval `[start, end)`.
///
/// `u64` is used so the exclusive end of the entire RV32 address space can be
/// represented as `0x1_0000_0000`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressRange {
    pub start: u64,
    pub end: u64,
}

impl AddressRange {
    pub fn new(start: u64, end: u64) -> Result<Self, AddressRangeError> {
        let range = Self { start, end };
        range.validate()?;
        Ok(range)
    }

    const fn validate(self) -> Result<(), AddressRangeError> {
        if self.start >= self.end {
            return Err(AddressRangeError::EmptyOrReversed {
                start: self.start,
                end: self.end,
            });
        }
        if self.end > RV32_LIMIT {
            return Err(AddressRangeError::OutsideRv32 {
                start: self.start,
                end: self.end,
            });
        }
        Ok(())
    }

    fn contains(self, address: u32) -> bool {
        let address = u64::from(address);
        self.start <= address && address < self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddressRangeError {
    EmptyOrReversed { start: u64, end: u64 },
    OutsideRv32 { start: u64, end: u64 },
}

impl fmt::Display for AddressRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOrReversed { start, end } => {
                write!(
                    f,
                    "invalid empty or reversed address range {start:#x}..{end:#x}"
                )
            }
            Self::OutsideRv32 { start, end } => write!(
                f,
                "address range {start:#x}..{end:#x} is outside the RV32 address space"
            ),
        }
    }
}

impl Error for AddressRangeError {}

/// Version of the intentionally bounded analysis performed here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidencyAlgorithm {
    Riscv32XipV1,
}

pub const ALGORITHM_VERSION: ResidencyAlgorithm = ResidencyAlgorithm::Riscv32XipV1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResidencyError {
    EmptyOutput,
    InputTooLarge {
        actual: usize,
        limit: usize,
    },
    LineTooLong {
        line_number: usize,
        actual: usize,
        limit: usize,
    },
    MissingFileFormatBanner,
    UnsupportedFileFormat {
        line_number: usize,
        line: String,
        format: String,
    },
    MissingSramTextSection,
    NoInstructions,
    InvalidRange {
        region: &'static str,
        source: AddressRangeError,
    },
    MalformedLine {
        line_number: usize,
        line: String,
        reason: &'static str,
    },
}

impl fmt::Display for ResidencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOutput => f.write_str("disassembly output is empty"),
            Self::InputTooLarge { actual, limit } => {
                write!(f, "disassembly is {actual} bytes; limit is {limit} bytes")
            }
            Self::LineTooLong {
                line_number,
                actual,
                limit,
            } => write!(
                f,
                "disassembly line {line_number} is {actual} bytes; limit is {limit} bytes"
            ),
            Self::MissingFileFormatBanner => {
                f.write_str("GNU objdump ELF32 RISC-V file-format banner is missing")
            }
            Self::UnsupportedFileFormat {
                line_number,
                line,
                format,
            } => write!(
                f,
                "unsupported GNU objdump file format on line {line_number} ({format}): {line}"
            ),
            Self::MissingSramTextSection => {
                f.write_str("GNU objdump .sramtext section heading is missing")
            }
            Self::NoInstructions => f.write_str("disassembly contains no instructions"),
            Self::InvalidRange { region, source } => {
                write!(f, "invalid {region} address range: {source}")
            }
            Self::MalformedLine {
                line_number,
                line,
                reason,
            } => write!(
                f,
                "malformed disassembly line {line_number}: {reason}: {line}"
            ),
        }
    }
}

impl Error for ResidencyError {}

#[derive(Clone, Copy, Debug, Default)]
struct RegisterValue {
    address: u32,
    auipc_origin: bool,
    auipc_addi: bool,
    auipc_mv: bool,
}

#[derive(Clone, Debug)]
struct Instruction<'a> {
    pc: u32,
    mnemonic: &'a str,
    operands: &'a str,
    comment: &'a str,
}

#[derive(Clone, Copy, Debug)]
struct NamedReference {
    address: u32,
}

/// Return every original line containing a named reference into `flash`.
///
/// The input must be complete GNU `objdump -dr --section=.sramtext` output for
/// an ELF32 little-endian RISC-V image, including its file-format banner and
/// `.sramtext` section heading.
/// The checker follows the source script's linear register tracking for LUI,
/// AUIPC, ADDI and MV, and invalidates caller-saved registers at call-like
/// instructions. It clears state at distinct named function labels, while
/// preserving it at local labels and `symbol+0xoffset` labels.
pub fn check_sram_residency(
    disassembly: &str,
    flash: AddressRange,
    sram: AddressRange,
) -> Result<Vec<String>, ResidencyError> {
    flash
        .validate()
        .map_err(|source| ResidencyError::InvalidRange {
            region: "flash",
            source,
        })?;
    sram.validate()
        .map_err(|source| ResidencyError::InvalidRange {
            region: "SRAM",
            source,
        })?;

    if disassembly.trim().is_empty() {
        return Err(ResidencyError::EmptyOutput);
    }
    if disassembly.len() > MAX_DISASSEMBLY_BYTES {
        return Err(ResidencyError::InputTooLarge {
            actual: disassembly.len(),
            limit: MAX_DISASSEMBLY_BYTES,
        });
    }

    let mut known: HashMap<&str, RegisterValue> = HashMap::new();
    let mut offending = Vec::new();
    let mut instruction_count = 0usize;
    let mut saw_file_format = false;
    let mut saw_sramtext_section = false;

    for (zero_based_line, original) in disassembly.lines().enumerate() {
        let line_number = zero_based_line + 1;
        if original.len() > MAX_LINE_BYTES {
            return Err(ResidencyError::LineTooLong {
                line_number,
                actual: original.len(),
                limit: MAX_LINE_BYTES,
            });
        }
        let line = original.trim();
        if line.is_empty() {
            continue;
        }

        // This also checks references on labels, comments, and other records,
        // matching the source script's scan of each complete input line.
        let named_references =
            named_references(original).map_err(|reason| ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason,
            })?;

        if let Some(format) = file_format_banner(line) {
            if saw_file_format || saw_sramtext_section {
                return Err(ResidencyError::MalformedLine {
                    line_number,
                    line: original.to_owned(),
                    reason: "duplicate or misplaced GNU objdump file-format banner",
                });
            }
            if format != "elf32-littleriscv" {
                return Err(ResidencyError::UnsupportedFileFormat {
                    line_number,
                    line: original.to_owned(),
                    format: format.to_owned(),
                });
            }
            saw_file_format = true;
            continue;
        }

        if is_section_heading(line) {
            if line != "Disassembly of section .sramtext:" {
                return Err(ResidencyError::MalformedLine {
                    line_number,
                    line: original.to_owned(),
                    reason: "expected the .sramtext disassembly section",
                });
            }
            if saw_sramtext_section {
                return Err(ResidencyError::MalformedLine {
                    line_number,
                    line: original.to_owned(),
                    reason: "duplicate .sramtext section heading",
                });
            }
            saw_sramtext_section = true;
            continue;
        }

        if line.contains("file format") {
            return Err(ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason: "malformed GNU objdump file-format banner",
            });
        }

        if !saw_sramtext_section {
            return Err(ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason: "expected .sramtext section heading before disassembly records",
            });
        }

        if let Some((label_pc, label)) =
            parse_label(line).map_err(|reason| ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason,
            })?
        {
            if !is_offset_label(label) && !is_local_label(label) {
                known.clear();
            }

            // A named function label in the flash interval is itself a named
            // reference in the input, just as it is in the awk implementation.
            if named_references
                .iter()
                .any(|reference| flash.contains(reference.address))
            {
                offending.push(original.to_owned());
            }
            let _ = label_pc;
            continue;
        }

        if is_relocation_record(line).map_err(|reason| ResidencyError::MalformedLine {
            line_number,
            line: original.to_owned(),
            reason,
        })? {
            if named_references
                .iter()
                .any(|reference| flash.contains(reference.address))
            {
                offending.push(original.to_owned());
            }
            continue;
        }

        let instruction =
            parse_instruction(original).map_err(|reason| ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason,
            })?;
        instruction_count += 1;

        let operands_have_flash_reference = named_references_in(instruction.operands)
            .map_err(|reason| ResidencyError::MalformedLine {
                line_number,
                line: original.to_owned(),
                reason,
            })?
            .iter()
            .any(|reference| flash.contains(reference.address));

        let mut candidate = named_references
            .iter()
            .any(|reference| flash.contains(reference.address));

        if candidate && has_libreq_annotation(instruction.comment) {
            // Ignore the stale absolute libreq annotation only when no genuine
            // flash named operand is present, then restore a candidate only
            // for a tracked ADDI source or tracked memory base in flash.
            candidate = if operands_have_flash_reference {
                true
            } else {
                (instruction.mnemonic == "addi"
                    && operand_register(instruction.operands, 1)
                        .and_then(|register| known.get(register))
                        .is_some_and(|value| flash.contains(value.address)))
                    || (is_memory_op(instruction.mnemonic)
                        && memory_register(instruction.operands)
                            .and_then(|register| known.get(register))
                            .is_some_and(|value| flash.contains(value.address)))
            };
        } else if candidate
            && has_named_comment_reference(instruction.comment)
            && !operands_have_flash_reference
            && is_memory_op(instruction.mnemonic)
        {
            // Ignore a stale XIP annotation only for a memory dereference whose
            // base and effective address are both proven SRAM through the
            // AUIPC/ADDI/MV provenance chain.
            if let Some(base) = memory_register(instruction.operands) {
                if let (Some(value), Some(offset)) = (
                    known.get(base),
                    memory_offset(instruction.operands)
                        .and_then(|text| parse_number(text).ok().flatten()),
                ) {
                    let effective = wrapping_add(value.address, offset);
                    if value.auipc_origin
                        && value.auipc_addi
                        && value.auipc_mv
                        && sram.contains(value.address)
                        && sram.contains(effective)
                    {
                        candidate = false;
                    }
                }
            }
        }

        if candidate {
            offending.push(original.to_owned());
        }

        if matches!(instruction.mnemonic, "call" | "jal" | "jalr" | "tail") {
            invalidate_call_clobbers(&mut known);
        }

        track_register_write(&instruction, &mut known, line_number, original)?;
    }

    if !saw_file_format {
        return Err(ResidencyError::MissingFileFormatBanner);
    }
    if !saw_sramtext_section {
        return Err(ResidencyError::MissingSramTextSection);
    }
    if instruction_count == 0 {
        return Err(ResidencyError::NoInstructions);
    }

    Ok(offending)
}

fn is_section_heading(line: &str) -> bool {
    line.starts_with("Disassembly of section ") && line.ends_with(':')
}

fn file_format_banner(line: &str) -> Option<&str> {
    let (path, remainder) = line.rsplit_once(':')?;
    if path.trim().is_empty() {
        return None;
    }
    remainder.trim().strip_prefix("file format ")
}

fn parse_label(line: &str) -> Result<Option<(u32, &str)>, &'static str> {
    let Some((address_text, rest)) = line.split_once(" <") else {
        return Ok(None);
    };
    let Some(label) = rest.strip_suffix(">:") else {
        return Ok(None);
    };
    let address = parse_address(address_text)?;
    Ok(Some((address, label)))
}

fn is_offset_label(label: &str) -> bool {
    // Keep this lexical rule aligned with `<name+0x[0-9a-f]+>:` in the shell
    // checker. The spelling and lowercase hexadecimal suffix are significant.
    let Some((symbol, offset)) = label.rsplit_once("+0x") else {
        return false;
    };
    !symbol.is_empty()
        && !offset.is_empty()
        && offset
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_local_label(label: &str) -> bool {
    label
        .strip_prefix(".L")
        .is_some_and(|tail| !tail.is_empty())
}

fn is_relocation_record(line: &str) -> Result<bool, &'static str> {
    let Some((address, remainder)) = line.split_once(':') else {
        return Ok(false);
    };
    let is_relocation = remainder
        .split_whitespace()
        .next()
        .is_some_and(|token| token.starts_with("R_RISCV_"));
    if is_relocation {
        parse_address(address)?;
    }
    Ok(is_relocation)
}

fn parse_instruction(line: &str) -> Result<Instruction<'_>, &'static str> {
    let Some((pc_text, remainder)) = line.split_once(':') else {
        return Err("expected an instruction address followed by ':'");
    };
    let pc = parse_address(pc_text)?;
    let remainder = remainder.trim_start();
    let (encoding, assembly) = split_token(remainder).ok_or("missing instruction encoding")?;
    if encoding.is_empty()
        || encoding.len() > 8
        || encoding.len() % 2 != 0
        || !encoding.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid or non-RV32 instruction encoding");
    }

    let (operands_and_comment, comment) = assembly.find('#').map_or_else(
        || (assembly.trim(), ""),
        |comment_index| {
            (
                assembly[..comment_index].trim(),
                &assembly[comment_index + 1..],
            )
        },
    );
    let (mnemonic, operands) =
        split_token(operands_and_comment).ok_or("missing instruction mnemonic")?;
    if !mnemonic
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
    {
        return Err("invalid instruction mnemonic");
    }

    Ok(Instruction {
        pc,
        mnemonic,
        operands,
        comment,
    })
}

fn split_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    if input.is_empty() {
        return None;
    }
    let token_end = input.find(char::is_whitespace).unwrap_or(input.len());
    Some((&input[..token_end], input[token_end..].trim_start()))
}

fn parse_address(text: &str) -> Result<u32, &'static str> {
    if text.is_empty() || text.len() > 8 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid or 64-bit address; expected at most eight hexadecimal digits");
    }
    u32::from_str_radix(text, 16).map_err(|_| "invalid instruction or symbol address")
}

fn named_references(line: &str) -> Result<Vec<NamedReference>, &'static str> {
    named_references_in(line)
}

fn named_references_in(text: &str) -> Result<Vec<NamedReference>, &'static str> {
    let bytes = text.as_bytes();
    let mut references = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let Some(close_relative) = bytes[index + 1..].iter().position(|byte| *byte == b'>') else {
            index += 1;
            continue;
        };
        let close = index + 1 + close_relative;
        let digits_end = if index > 0 && bytes[index - 1] == b' ' {
            index - 1
        } else {
            index
        };
        let mut start = digits_end;
        while start > 0 && bytes[start - 1].is_ascii_hexdigit() {
            start -= 1;
        }
        let digits = &text[start..digits_end];
        if digits.len() > 8 {
            return Err("64-bit named address is not valid in RV32 disassembly");
        }
        // The source check recognizes fixed-width RV32 named references.
        if digits.len() == 8 {
            let address =
                u32::from_str_radix(digits, 16).map_err(|_| "invalid named reference address")?;
            references.push(NamedReference { address });
        }
        index = close + 1;
    }
    Ok(references)
}

fn has_libreq_annotation(comment: &str) -> bool {
    comment.split('<').skip(1).any(|tail| {
        let Some((symbol, _)) = tail.split_once('>') else {
            return false;
        };
        let Some(rest) = symbol.strip_prefix("__aros_libreq_") else {
            return false;
        };
        let Some((name, offset)) = rest.rsplit_once("+0x") else {
            return false;
        };
        !name.is_empty()
            && !offset.is_empty()
            && offset
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && preceding_lower_hex_address(comment, tail)
    })
}

fn preceding_lower_hex_address(comment: &str, tail: &str) -> bool {
    let tail_offset = comment.len().saturating_sub(tail.len() + 1);
    let prefix = &comment[..tail_offset];
    let Some(before_space) = prefix.strip_suffix(' ') else {
        return false;
    };
    let digits = before_space
        .rsplit(|character: char| !character.is_ascii_hexdigit())
        .next()
        .unwrap_or("");
    !digits.is_empty()
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn has_named_comment_reference(comment: &str) -> bool {
    let bytes = comment.as_bytes();
    for open in 0..bytes.len() {
        if bytes[open] != b'<' {
            continue;
        }
        if open == 0 || bytes[open - 1] != b' ' {
            continue;
        }
        let mut start = open - 1;
        while start > 0
            && (bytes[start - 1].is_ascii_digit() || (b'a'..=b'f').contains(&bytes[start - 1]))
        {
            start -= 1;
        }
        if start == open - 1
            || bytes
                .get(open + 1..)
                .and_then(|rest| rest.iter().position(|byte| *byte == b'>'))
                .is_none()
        {
            continue;
        }
        let close = open
            + 1
            + bytes[open + 1..]
                .iter()
                .position(|byte| *byte == b'>')
                .unwrap_or(0);
        if close > open + 1 {
            return true;
        }
    }
    false
}

fn operand_register(operands: &str, index: usize) -> Option<&str> {
    operands
        .split(',')
        .nth(index)
        .map(str::trim)
        .filter(|register| is_register(register))
}

fn memory_register(operands: &str) -> Option<&str> {
    let source = operands.split(',').nth(1)?.trim();
    let open = source.rfind('(')?;
    let close = source[open + 1..].find(')')? + open + 1;
    let register = source[open + 1..close].trim();
    is_register(register).then_some(register)
}

fn memory_offset(operands: &str) -> Option<&str> {
    let source = operands.split(',').nth(1)?.trim();
    let open = source.rfind('(')?;
    Some(source[..open].trim())
}

fn is_register(register: &str) -> bool {
    matches!(
        register,
        "zero"
            | "ra"
            | "sp"
            | "gp"
            | "tp"
            | "s0"
            | "s1"
            | "s2"
            | "s3"
            | "s4"
            | "s5"
            | "s6"
            | "s7"
            | "s8"
            | "s9"
            | "s10"
            | "s11"
            | "a0"
            | "a1"
            | "a2"
            | "a3"
            | "a4"
            | "a5"
            | "a6"
            | "a7"
            | "t0"
            | "t1"
            | "t2"
            | "t3"
            | "t4"
            | "t5"
            | "t6"
    )
}

fn is_memory_op(mnemonic: &str) -> bool {
    matches!(
        mnemonic,
        "lb" | "lbu"
            | "lh"
            | "lhu"
            | "lw"
            | "lwu"
            | "ld"
            | "sb"
            | "sh"
            | "sw"
            | "sd"
            | "flw"
            | "fld"
            | "fsw"
            | "fsd"
    )
}

fn invalidate_call_clobbers(known: &mut HashMap<&str, RegisterValue>) {
    known.retain(|register, _| {
        !matches!(
            *register,
            "ra" | "t0"
                | "t1"
                | "t2"
                | "t3"
                | "t4"
                | "t5"
                | "t6"
                | "a0"
                | "a1"
                | "a2"
                | "a3"
                | "a4"
                | "a5"
                | "a6"
                | "a7"
        )
    });
}

fn track_register_write<'a>(
    instruction: &Instruction<'a>,
    known: &mut HashMap<&'a str, RegisterValue>,
    line_number: usize,
    original: &str,
) -> Result<(), ResidencyError> {
    let Some(rd) = operand_register(instruction.operands, 0) else {
        return Ok(());
    };
    if rd == "zero"
        || matches!(
            instruction.mnemonic,
            "sb" | "sh"
                | "sw"
                | "beq"
                | "bne"
                | "blt"
                | "bge"
                | "bltu"
                | "bgeu"
                | "beqz"
                | "bnez"
                | "bltz"
                | "bgez"
                | "blez"
                | "bgtz"
                | "jr"
        )
    {
        return Ok(());
    }

    let source =
        operand_register(instruction.operands, 1).and_then(|register| known.get(register).copied());
    let source_text = instruction.operands.split(',').nth(1).map_or("", str::trim);
    let immediate_text = instruction.operands.split(',').nth(2).map_or("", str::trim);

    let malformed_number = || ResidencyError::MalformedLine {
        line_number,
        line: original.to_owned(),
        reason: "numeric operand exceeds the RV32 bound",
    };
    let source_immediate = parse_number(source_text).map_err(|()| malformed_number())?;
    let instruction_immediate = parse_number(immediate_text).map_err(|()| malformed_number())?;

    // The source script clears every recognized destination before attempting
    // to rematerialize one of the four tracked forms.
    known.remove(rd);

    let value = match instruction.mnemonic {
        "lui" => source_immediate.map(|immediate| RegisterValue {
            address: wrap_32(sign_extend_20(immediate) * 4096),
            ..RegisterValue::default()
        }),
        "auipc" => source_immediate.map(|immediate| RegisterValue {
            address: wrapping_add(instruction.pc, sign_extend_20(immediate) * 4096),
            auipc_origin: true,
            ..RegisterValue::default()
        }),
        "addi" => source
            .zip(instruction_immediate)
            .map(|(source, immediate)| RegisterValue {
                address: wrapping_add(source.address, immediate),
                auipc_origin: source.auipc_origin,
                auipc_addi: source.auipc_addi || source.auipc_origin,
                auipc_mv: source.auipc_mv,
            }),
        "mv" => source.map(|source| RegisterValue {
            address: source.address,
            auipc_origin: source.auipc_origin,
            auipc_addi: source.auipc_addi,
            auipc_mv: source.auipc_mv || (source.auipc_origin && source.auipc_addi),
        }),
        _ => None,
    };

    if let Some(value) = value {
        known.insert(rd, value);
    }
    Ok(())
}

fn parse_number(text: &str) -> Result<Option<i128>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (negative, unsigned) = text
        .strip_prefix('-')
        .map_or((false, text), |unsigned| (true, unsigned));
    let (radix, digits) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
        .map_or((10, unsigned), |digits| (16, digits));
    if digits.is_empty()
        || !digits.bytes().all(|byte| match radix {
            16 => byte.is_ascii_hexdigit(),
            _ => byte.is_ascii_digit(),
        })
    {
        return Ok(None);
    }
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| ())?;
    if magnitude > i128::from(u32::MAX) {
        return Err(());
    }
    Ok(Some(if negative { -magnitude } else { magnitude }))
}

const fn sign_extend_20(value: i128) -> i128 {
    if value >= 524_288 {
        value - 1_048_576
    } else {
        value
    }
}

fn wrap_32(value: i128) -> u32 {
    value.rem_euclid(i128::from(RV32_LIMIT)) as u32
}

fn wrapping_add(address: u32, offset: i128) -> u32 {
    wrap_32(i128::from(address) + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLASH: AddressRange = AddressRange {
        start: 0x4000_0000,
        end: 0x4400_0000,
    };
    const SRAM: AddressRange = AddressRange {
        start: 0x4ff0_0000,
        end: 0x4ff8_0000,
    };

    fn check(text: &str) -> Result<Vec<String>, ResidencyError> {
        let disassembly =
            format!("/fixture/aros-p4.elf:     file format elf32-littleriscv\n\n{text}");
        check_sram_residency(&disassembly, FLASH, SRAM)
    }

    #[test]
    fn rejects_direct_xip_branch_and_call_operands() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 00000063 bne a0,a1,40001000 <flash_branch>\n\
            4ff00004: 000000ef jal ra,40002000 <flash_call>\n";
        let bad = check(disassembly).unwrap();
        assert_eq!(bad.len(), 2);
        assert!(bad[0].contains("40001000 <flash_branch>"));
        assert!(bad[1].contains("40002000 <flash_call>"));
    }

    #[test]
    fn rejects_lui_materialized_xip_load_even_with_libreq_annotation() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 400002b7 lui t0,0x40000\n\
            4ff00004: 0002a503 lw a0,0(t0) # 40000000 <__aros_libreq_xip+0x0>\n";
        let bad = check(disassembly).unwrap();
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("lw a0,0(t0)"));
    }

    #[test]
    fn ignores_stale_libreq_annotation_with_unknown_base() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 0002a503 lw a0,0(t0) # 40000000 <__aros_libreq_stale+0x8>\n";
        assert!(check(disassembly).unwrap().is_empty());
    }

    #[test]
    fn ignores_stale_xip_comment_only_for_proven_sram_auipc_addi_mv_memory_access() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 00000297 auipc t0,0x0\n\
            4ff00004: 00028293 addi t0,t0,0\n\
            4ff00008: 00028313 mv t1,t0\n\
            4ff0000c: 00032503 lw a0,0(t1) # 40001000 <flash_symbol>\n";
        assert!(check(disassembly).unwrap().is_empty());
    }

    #[test]
    fn unknown_base_with_real_symbol_comment_fails_closed() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 0002a503 lw a0,0(t1) # 40001000 <flash_symbol>\n";
        assert_eq!(check(disassembly).unwrap().len(), 1);
    }

    #[test]
    fn call_invalidates_caller_saved_tracked_registers() {
        let disassembly = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 400002b7 lui t0,0x40000\n\
            4ff00004: 000000ef call 4ff01000 <helper>\n\
            4ff00008: 0002a503 lw a0,0(t0) # 40000000 <__aros_libreq_xip+0x0>\n";
        assert!(check(disassembly).unwrap().is_empty());
    }

    #[test]
    fn distinct_functions_clear_state_but_local_and_offset_labels_do_not() {
        let distinct_functions = "Disassembly of section .sramtext:\n\
            4ff00000 <first>:\n\
            4ff00000: 400002b7 lui t0,0x40000\n\
            4ff00004 <second>:\n\
            4ff00004: 0002a503 lw a0,0(t0) # 40000000 <__aros_libreq_stale+0x0>\n";
        assert!(check(distinct_functions).unwrap().is_empty());

        let local_label = "Disassembly of section .sramtext:\n\
            4ff00000 <first>:\n\
            4ff00000: 00000297 auipc t0,0x0\n\
            4ff00004: 00028293 addi t0,t0,0\n\
            4ff00008: 00028313 mv t1,t0\n\
            4ff0000c <.Llocal>:\n\
            4ff0000c: 00032503 lw a0,0(t1) # 40001000 <flash_symbol>\n";
        assert!(check(local_label).unwrap().is_empty());

        let offset_label = "Disassembly of section .sramtext:\n\
            4ff00000 <first>:\n\
            4ff00000: 00000297 auipc t0,0x0\n\
            4ff00004: 00028293 addi t0,t0,0\n\
            4ff00008: 00028313 mv t1,t0\n\
            4ff0000c <first+0x0c>:\n\
            4ff0000c: 00032503 lw a0,0(t1) # 40001000 <flash_symbol>\n";
        assert!(check(offset_label).unwrap().is_empty());
    }

    #[test]
    fn signed_immediates_and_auipc_arithmetic_wrap_as_rv32() {
        // 0xf0000000 + 0x50000 * 4096 wraps from 0x1_4000_0000 to 0x40000000.
        let disassembly = "Disassembly of section .sramtext:\n\
            f0000000 <run>:\n\
            f0000000: 50000297 auipc t0,0x50000\n\
            f0000004: 80028293 addi t0,t0,-2048\n\
            f0000008: 7ff28293 addi t0,t0,2047\n\
            f000000c: 00128293 addi t0,t0,1\n\
            f0000010: 0002a503 lw a0,0(t0) # 40000000 <__aros_libreq_wrapped+0x4>\n";
        let bad = check(disassembly).unwrap();
        assert_eq!(bad.len(), 1);
        assert!(bad[0].contains("lw a0,0(t0)"));
    }

    #[test]
    fn custom_ranges_are_used_instead_of_board_constants() {
        let disassembly = "Disassembly of section .sramtext:\n\
            20000000 <run>:\n\
            20000000: 10000537 lui a0,0x10000\n\
            20000004: 00052503 lw a0,0(a0) # 10000000 <flash_symbol>\n";
        let complete_disassembly =
            format!("/fixture/aros-p4.elf: file format elf32-littleriscv\n\n{disassembly}");
        let flash = AddressRange::new(0x1000_0000, 0x1100_0000).unwrap();
        let sram = AddressRange::new(0x2000_0000, 0x2100_0000).unwrap();
        let bad = check_sram_residency(&complete_disassembly, flash, sram).unwrap();
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn rejects_empty_malformed_and_64_bit_address_data() {
        assert_eq!(
            check_sram_residency(" \n\t", FLASH, SRAM).unwrap_err(),
            ResidencyError::EmptyOutput
        );
        assert_eq!(
            check("Disassembly of section .sramtext:\n").unwrap_err(),
            ResidencyError::NoInstructions
        );
        assert!(matches!(
            check("this is not objdump output\n"),
            Err(ResidencyError::MalformedLine { .. })
        ));
        assert!(matches!(
            check("Disassembly of section .text:\n4ff00000: 00000013 nop\n"),
            Err(ResidencyError::MalformedLine { .. })
        ));
        assert!(matches!(
            check(
                "0000000140000000 <wide_symbol>:\n\
                   0000000140000000: 00000013 nop\n"
            ),
            Err(ResidencyError::MalformedLine { .. })
        ));
        assert!(matches!(
            check(
                "4ff00000 <run>:\n\
                   4ff00000: 00000013 jal ra,0000000140000000 <wide_symbol>\n"
            ),
            Err(ResidencyError::MalformedLine { .. })
        ));
    }

    #[test]
    fn half_open_ranges_and_invalid_ranges_are_respected() {
        let edge = "Disassembly of section .sramtext:\n\
            4ff00000 <run>:\n\
            4ff00000: 000000ef jal ra,44000000 <one_past_flash>\n";
        assert!(check(edge).unwrap().is_empty());
        assert!(matches!(
            check_sram_residency(
                "4ff00000: 00000013 nop\n",
                AddressRange {
                    start: 0x4400_0000,
                    end: 0x4000_0000
                },
                SRAM,
            ),
            Err(ResidencyError::InvalidRange {
                region: "flash",
                ..
            })
        ));
        assert!(matches!(
            AddressRange::new(0, RV32_LIMIT + 1),
            Err(AddressRangeError::OutsideRv32 { .. })
        ));
    }

    #[test]
    fn accepts_complete_gnu_objdump_banner_section_and_realistic_records() {
        let disassembly =
            "/opt/esp/riscv32-esp-elf/bin/objdump:     file format elf32-littleriscv\n\
\n\
Disassembly of section .sramtext:\n\
\n\
4ff00000 <probe>:\n\
4ff00000:\t00004797\tauipc\ta5,0x4\n\
4ff00004:\tffc78793\taddi\ta5,a5,-4\n\
4ff00008:\t8b3e\tmv\ts6,a5\n\
4ff0000a:\t038b2703\tlw\ta4,56(s6) # 40000038 <aros_app_desc+0x18>\n";
        assert!(check_sram_residency(disassembly, FLASH, SRAM)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_missing_or_unsupported_elf_format_and_section_markers() {
        let no_format = "Disassembly of section .sramtext:\n4ff00000: 00000013 nop\n";
        assert_eq!(
            check_sram_residency(no_format, FLASH, SRAM).unwrap_err(),
            ResidencyError::MissingFileFormatBanner
        );

        let wrong_format = "/fixture/image.elf: file format elf64-littleriscv\n\
            Disassembly of section .sramtext:\n4ff00000: 00000013 nop\n";
        assert!(matches!(
            check_sram_residency(wrong_format, FLASH, SRAM),
            Err(ResidencyError::UnsupportedFileFormat { .. })
        ));

        let no_section = "/fixture/image.elf: file format elf32-littleriscv\n";
        assert_eq!(
            check_sram_residency(no_section, FLASH, SRAM).unwrap_err(),
            ResidencyError::MissingSramTextSection
        );
    }

    #[test]
    fn rejects_oversized_input_and_oversized_lines() {
        let oversized = "x".repeat(MAX_DISASSEMBLY_BYTES + 1);
        assert!(matches!(
            check_sram_residency(&oversized, FLASH, SRAM),
            Err(ResidencyError::InputTooLarge { .. })
        ));

        let oversized_line = format!(
            "/fixture/image.elf: file format elf32-littleriscv\n\
             {}\n",
            "x".repeat(MAX_LINE_BYTES + 1)
        );
        assert!(matches!(
            check_sram_residency(&oversized_line, FLASH, SRAM),
            Err(ResidencyError::LineTooLong { .. })
        ));
    }
}
