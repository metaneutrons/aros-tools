//! Bounded RISC-V ELF attribute decoding, independent of compiler filenames.
//!
//! The decoded ISA is evidence, not a supported-target or runtime declaration.
//! Consumers must compare it with their source-bound compiler contract.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Result};

mod contract;
pub use contract::{ArtifactRole, TargetContract};

/// RISC-V's ELF machine identifier.
pub const MACHINE: u16 = 243;
/// RISC-V processor-specific attribute section type.
pub const SHT_ATTRIBUTES: u32 = 0x7000_0003;
/// Mask selecting the floating-point calling convention in `e_flags`.
pub const FLOAT_ABI_MASK: u32 = 0x6;

/// One decoded attribute value, retaining optional future tags as evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum Value {
    Integer(u64),
    String(String),
}

/// A unique RISC-V file-attribute set with a mandatory architecture string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributes {
    pub architecture: String,
    pub values: BTreeMap<u64, Value>,
}

/// Read the unique `.riscv.attributes` section from the selected object bytes.
///
/// # Errors
///
/// Rejects non-RISC-V objects, missing/duplicate attributes, truncated ranges,
/// unknown mandatory tags and ambiguous vendor/file declarations. This narrow
/// compiler-probe reader deliberately does not infer attributes from flags.
pub fn read(bytes: &[u8]) -> Result<Attributes> {
    let object = super::read(bytes)?;
    ensure!(object.machine == MACHINE, "not a RISC-V ELF object");
    let mut sections = object
        .sections
        .iter()
        .filter(|section| section.name == ".riscv.attributes");
    let section = sections.next().context("missing RISC-V attributes")?;
    ensure!(
        sections.next().is_none(),
        "duplicate RISC-V attribute sections"
    );
    ensure!(
        section.kind == SHT_ATTRIBUTES,
        "wrong RISC-V attribute section type"
    );
    let start = usize::try_from(section.offset).context("attribute offset overflow")?;
    let size = usize::try_from(section.size).context("attribute size overflow")?;
    let end = start
        .checked_add(size)
        .context("attribute range overflow")?;
    decode(
        bytes
            .get(start..end)
            .context("truncated RISC-V attributes")?,
    )
}

fn decode(bytes: &[u8]) -> Result<Attributes> {
    ensure!(bytes.len() <= 65_536, "RISC-V attributes exceed 64 KiB");
    ensure!(bytes.first() == Some(&b'A'), "unsupported attribute format");
    let mut remaining = &bytes[1..];
    let mut file = None;
    while !remaining.is_empty() {
        let subsection = block(&mut remaining, 0)?;
        let mut body = &subsection[4..];
        ensure!(
            string(&mut body)? == "riscv",
            "unsupported attribute vendor"
        );
        ensure!(file.is_none(), "duplicate RISC-V vendor declarations");
        let tag_start = body;
        let tag = uleb(&mut body)?;
        ensure!(tag == 1, "expected file-scoped RISC-V attributes");
        let tag_bytes = tag_start.len() - body.len();
        let contents = block(&mut body, tag_bytes)?;
        ensure!(body.is_empty(), "multiple RISC-V file declarations");
        let mut values = BTreeMap::new();
        let mut attributes = &contents[4..];
        while !attributes.is_empty() {
            let tag = uleb(&mut attributes)?;
            ensure!(
                matches!(tag, 4 | 5 | 6 | 8 | 10 | 12 | 14 | 16) || tag % 128 >= 64,
                "unknown mandatory RISC-V attribute"
            );
            let value = if tag % 2 == 1 {
                Value::String(string(&mut attributes)?.to_owned())
            } else {
                Value::Integer(uleb(&mut attributes)?)
            };
            ensure!(
                values.insert(tag, value).is_none(),
                "duplicate RISC-V attribute"
            );
        }
        let Some(Value::String(architecture)) = values.get(&5) else {
            bail!("missing RISC-V architecture attribute");
        };
        ensure!(
            !architecture.is_empty()
                && architecture.len() <= 4096
                && architecture
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "invalid RISC-V architecture attribute"
        );
        file = Some(Attributes {
            architecture: architecture.clone(),
            values,
        });
    }
    file.context("missing RISC-V vendor attributes")
}

// `already_consumed` counts the tag before a sub-sub-section's length field.
// Both lengths include their own headers, not just the payload.
fn block<'a>(bytes: &mut &'a [u8], already_consumed: usize) -> Result<&'a [u8]> {
    let size = u32::from_le_bytes(
        bytes
            .get(..4)
            .context("truncated attribute length")?
            .try_into()
            .context("truncated attribute length")?,
    ) as usize;
    let size = size
        .checked_sub(already_consumed)
        .context("invalid attribute length")?;
    ensure!(size >= 4, "attribute block shorter than its header");
    let result = bytes
        .get(..size)
        .context("attribute block exceeds enclosing range")?;
    *bytes = &bytes[size..];
    Ok(result)
}

fn string<'a>(bytes: &mut &'a [u8]) -> Result<&'a str> {
    let size = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated attribute string")?;
    let value = std::str::from_utf8(&bytes[..size]).context("non-UTF-8 attribute string")?;
    *bytes = &bytes[size + 1..];
    Ok(value)
}

fn uleb(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.first().context("truncated attribute integer")?;
        *bytes = &bytes[1..];
        ensure!(shift != 63 || byte <= 1, "attribute integer overflow");
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("attribute integer overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(tags: &[u8]) -> Vec<u8> {
        let file_size = u32::try_from(5 + tags.len()).unwrap();
        let vendor_size = 10 + file_size;
        let mut bytes = vec![b'A'];
        bytes.extend(vendor_size.to_le_bytes());
        bytes.extend(b"riscv\0");
        bytes.push(1);
        bytes.extend(file_size.to_le_bytes());
        bytes.extend(tags);
        bytes
    }

    pub(super) fn object(class: u8) -> Vec<u8> {
        let attributes = encoded(if class == 1 {
            b"\x04\x10\x05rv32i2p1\0"
        } else {
            b"\x04\x10\x05rv64i2p1\0"
        });
        let names = b"\0.shstrtab\0.riscv.attributes\0";
        let (header, stride) = if class == 1 { (52, 40) } else { (64, 64) };
        let payload = header + 3 * stride;
        let mut bytes = vec![0_u8; payload + names.len() + attributes.len()];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = class;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[0x12..0x14].copy_from_slice(&MACHINE.to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&1_u32.to_le_bytes());
        let (off, size, count, name_index, section_offset, section_size) = if class == 1 {
            bytes[0x20..0x24].copy_from_slice(&(header as u32).to_le_bytes());
            (0x28, 0x2e, 0x30, 0x32, 16, 20)
        } else {
            bytes[0x28..0x30].copy_from_slice(&(header as u64).to_le_bytes());
            (0x34, 0x3a, 0x3c, 0x3e, 24, 32)
        };
        bytes[off..off + 2].copy_from_slice(&(header as u16).to_le_bytes());
        bytes[size..size + 2].copy_from_slice(&(stride as u16).to_le_bytes());
        bytes[count..count + 2].copy_from_slice(&3_u16.to_le_bytes());
        bytes[name_index..name_index + 2].copy_from_slice(&1_u16.to_le_bytes());
        for (index, name, kind, start, len) in [
            (1, 1_u32, super::super::SHT_STRTAB, payload, names.len()),
            (
                2,
                11_u32,
                SHT_ATTRIBUTES,
                payload + names.len(),
                attributes.len(),
            ),
        ] {
            let base = header + index * stride;
            bytes[base..base + 4].copy_from_slice(&name.to_le_bytes());
            bytes[base + 4..base + 8].copy_from_slice(&kind.to_le_bytes());
            if class == 1 {
                bytes[base + section_offset..base + section_offset + 4]
                    .copy_from_slice(&(start as u32).to_le_bytes());
                bytes[base + section_size..base + section_size + 4]
                    .copy_from_slice(&(len as u32).to_le_bytes());
            } else {
                bytes[base + section_offset..base + section_offset + 8]
                    .copy_from_slice(&(start as u64).to_le_bytes());
                bytes[base + section_size..base + section_size + 8]
                    .copy_from_slice(&(len as u64).to_le_bytes());
            }
        }
        bytes[payload..payload + names.len()].copy_from_slice(names);
        bytes[payload + names.len()..].copy_from_slice(&attributes);
        bytes
    }

    #[test]
    fn public_reader_binds_attributes_to_the_selected_elf_bytes() {
        for class in [1, 2] {
            let bytes = object(class);
            assert_eq!(
                read(&bytes).unwrap().architecture,
                if class == 1 { "rv32i2p1" } else { "rv64i2p1" }
            );
            // The bytes determine the machine independently of valid attributes.
            let mut foreign = bytes.clone();
            foreign[0x12..0x14].copy_from_slice(&62_u16.to_le_bytes());
            assert!(read(&foreign)
                .unwrap_err()
                .to_string()
                .contains("not a RISC-V"));
            let mut wrong_type = bytes.clone();
            let attribute_header = if class == 1 { 52 + 2 * 40 } else { 64 + 2 * 64 };
            wrong_type[attribute_header + 4..attribute_header + 8]
                .copy_from_slice(&1_u32.to_le_bytes());
            assert!(read(&wrong_type)
                .unwrap_err()
                .to_string()
                .contains("section type"));
            for size in 0..bytes.len() {
                assert!(
                    read(&bytes[..size]).is_err(),
                    "class {class}, prefix {size}"
                );
            }
        }
    }

    #[test]
    fn reads_real_format_and_retains_abi_attributes() {
        let parsed = decode(&encoded(b"\x04\x10\x05rv32i2p1_m2p0_f2p2_c2p0\0\x0e\x01")).unwrap();
        assert_eq!(parsed.architecture, "rv32i2p1_m2p0_f2p2_c2p0");
        assert_eq!(parsed.values[&4], Value::Integer(16));
        assert_eq!(parsed.values[&14], Value::Integer(1));
        let different = decode(&encoded(b"\x05rv64i2p1_d2p2\0")).unwrap();
        assert_ne!(parsed.architecture, different.architecture);
    }

    #[test]
    fn rejects_every_truncated_prefix() {
        let bytes = encoded(b"\x04\x10\x05rv32i2p1\0");
        for size in 0..bytes.len() {
            assert!(decode(&bytes[..size]).is_err(), "prefix {size}");
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_unknown_mandatory_values() {
        for tags in [
            b"\x04\x10".as_slice(),
            b"\x05rv32i2p1\0\x05rv64i2p1\0".as_slice(),
            b"\x05rv32i2p1\0\x12\x01".as_slice(),
            b"\x05rv32i2p1\0\x04\x10\x04\x10".as_slice(),
        ] {
            assert!(decode(&encoded(tags)).is_err());
        }
        assert!(decode(&encoded(b"\x05rv32i2p1\0\x40\x01")).is_ok());
    }

    #[test]
    fn rejects_duplicate_vendor_and_invalid_lengths() {
        let mut bytes = encoded(b"\x05rv32i2p1\0");
        bytes.extend_from_slice(&bytes.clone()[1..]);
        assert!(decode(&bytes).is_err());
        for size in [0_u32, 3, u32::MAX] {
            let mut bytes = encoded(b"\x05rv32i2p1\0");
            bytes[1..5].copy_from_slice(&size.to_le_bytes());
            assert!(decode(&bytes).is_err());
        }
        assert!(uleb(&mut b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02".as_slice()).is_err());
    }
}
