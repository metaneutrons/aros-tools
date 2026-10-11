//! Bounded ARM ELF file-attribute decoding.
//!
//! The syntax and attribute encodings follow the ARM ABI addenda, addenda32,
//! sections "Formal syntax and encoding" and "Attribute tags":
//! <https://github.com/ARM-software/abi-aa/blob/main/addenda32/addenda32.rst>
//!
//! This reader accepts only one aeabi File-scope attribute set. It rejects
//! section and symbol scopes rather than merging them or guessing precedence.
//! Other vendor blocks are skipped after opaque, length-checked framing.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, ensure, Context, Result};

/// ARM's ELF machine identifier.
pub const MACHINE: u16 = 40;
/// ARM processor-specific attribute section type.
pub const SHT_ATTRIBUTES: u32 = 0x7000_0003;

const MAX_SECTION_SIZE: usize = 65_536;
const MAX_VALUES: usize = 256;
const MAX_NTBS_SIZE: usize = 4096;

/// One decoded ARM file attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Integer(u64),
    String(Vec<u8>),
    Compatibility { flag: u64, vendor: Vec<u8> },
}

/// A unique ARM File-scope attribute set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributes {
    pub values: BTreeMap<u64, Value>,
}

impl Attributes {
    /// Return an explicit integer attribute or its standardized omitted value.
    ///
    /// The reader rejects the Tag_nodefaults marker, so omitted integer
    /// attributes use the ABI default of zero. A present string or compatibility
    /// attribute is not an integer and produces an error.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested tag has no supported integer
    /// encoding or has a present non-integer value.
    pub fn integer(&self, tag: u64) -> Result<u64> {
        ensure!(
            (6..=31).contains(&tag)
                || is_known_integer_tag(tag)
                || (tag >= 66 && tag % 128 >= 64 && tag.is_multiple_of(2)),
            "ARM attribute {tag} has no supported integer encoding"
        );
        match self.values.get(&tag) {
            Some(Value::Integer(value)) => Ok(*value),
            Some(_) => bail!("ARM attribute {tag} is not an integer"),
            None => Ok(0),
        }
    }
}

/// Read the unique .ARM.attributes section from the selected ELF32 ARM object.
///
/// # Errors
///
/// Rejects non-ARM or non-ELF32 objects, missing or duplicate sections,
/// malformed ranges, unsupported scopes, unknown mandatory tags, and
/// over-limit or malformed attribute data.
pub fn read(bytes: &[u8]) -> Result<Attributes> {
    let object = super::read(bytes)?;
    ensure!(
        object.class == super::Class::Elf32,
        "ARM attributes require ELF32"
    );
    ensure!(object.machine == MACHINE, "not an ARM ELF object");

    let mut sections = object
        .sections
        .iter()
        .filter(|section| section.name == ".ARM.attributes");
    let section = sections.next().context("missing ARM attributes")?;
    ensure!(
        sections.next().is_none(),
        "duplicate ARM attribute sections"
    );
    ensure!(
        section.kind == SHT_ATTRIBUTES,
        "wrong ARM attribute section type"
    );

    let size = usize::try_from(section.size).context("ARM attribute size overflow")?;
    ensure!(size <= MAX_SECTION_SIZE, "ARM attributes exceed 64 KiB");
    let start = usize::try_from(section.offset).context("ARM attribute offset overflow")?;
    let end = start
        .checked_add(size)
        .context("ARM attribute range overflow")?;
    decode(bytes.get(start..end).context("truncated ARM attributes")?)
}

fn decode(bytes: &[u8]) -> Result<Attributes> {
    ensure!(
        bytes.len() <= MAX_SECTION_SIZE,
        "ARM attributes exceed 64 KiB"
    );
    ensure!(bytes.first() == Some(&b'A'), "unsupported attribute format");

    let mut remaining = &bytes[1..];
    let mut vendor_names = BTreeSet::new();
    let mut aeabi = None;
    while !remaining.is_empty() {
        let vendor_block = block(&mut remaining, 0)?;
        let mut body = &vendor_block[4..];
        let vendor_name = ntbs(&mut body)?;
        ensure!(
            vendor_names.insert(vendor_name),
            "duplicate ARM attribute vendor"
        );
        if vendor_name == b"aeabi" {
            ensure!(aeabi.is_none(), "duplicate aeabi vendor");
            aeabi = Some(file_scope(body)?);
        }
    }

    Ok(Attributes {
        values: aeabi.context("missing aeabi file attributes")?,
    })
}

fn file_scope(mut body: &[u8]) -> Result<BTreeMap<u64, Value>> {
    let mut file_attributes = None;
    while !body.is_empty() {
        let tag_start = body;
        let scope = uleb(&mut body)?;
        let tag_bytes = tag_start.len() - body.len();
        match scope {
            1 => {
                ensure!(
                    file_attributes.is_none(),
                    "duplicate ARM file attribute scopes"
                );
                let subsection = block(&mut body, tag_bytes)?;
                file_attributes = Some(attributes(&subsection[4..])?);
            }
            2 => bail!("ARM section-scoped attributes are unsupported"),
            3 => bail!("ARM symbol-scoped attributes are unsupported"),
            _ => bail!("unsupported ARM attribute scope {scope}"),
        }
    }
    file_attributes.context("missing ARM file attribute scope")
}

fn attributes(mut bytes: &[u8]) -> Result<BTreeMap<u64, Value>> {
    let mut values = BTreeMap::new();
    while !bytes.is_empty() {
        ensure!(values.len() < MAX_VALUES, "too many ARM attributes");
        let tag = uleb(&mut bytes)?;
        ensure!(!values.contains_key(&tag), "duplicate ARM attribute {tag}");
        ensure!(tag != 64, "ARM Tag_nodefaults is unsupported");

        let value = if matches!(tag, 4 | 5) {
            Value::String(ntbs(&mut bytes)?.to_vec())
        } else if tag == 32 {
            Value::Compatibility {
                flag: uleb(&mut bytes)?,
                vendor: ntbs(&mut bytes)?.to_vec(),
            }
        } else if (6..=31).contains(&tag) || is_known_integer_tag(tag) {
            Value::Integer(uleb(&mut bytes)?)
        } else if tag >= 65 && tag % 128 >= 64 {
            if tag % 2 == 1 {
                Value::String(ntbs(&mut bytes)?.to_vec())
            } else {
                Value::Integer(uleb(&mut bytes)?)
            }
        } else {
            bail!("unknown mandatory ARM attribute {tag}");
        };
        values.insert(tag, value);
    }
    Ok(values)
}

const fn is_known_integer_tag(tag: u64) -> bool {
    matches!(tag, 34 | 36 | 38 | 42 | 44 | 46 | 48 | 50 | 52)
}

// The encoded length includes any tag bytes before its four-byte length field.
// The returned slice begins at that length field and includes the whole block.
fn block<'a>(bytes: &mut &'a [u8], tag_bytes: usize) -> Result<&'a [u8]> {
    let length = u32::from_le_bytes(
        bytes
            .get(..4)
            .context("truncated ARM attribute length")?
            .try_into()
            .context("truncated ARM attribute length")?,
    ) as usize;
    let block_size = length
        .checked_sub(tag_bytes)
        .context("invalid ARM attribute length")?;
    ensure!(
        block_size >= 4,
        "ARM attribute block shorter than its header"
    );
    let result = bytes
        .get(..block_size)
        .context("ARM attribute block exceeds enclosing range")?;
    *bytes = &bytes[block_size..];
    Ok(result)
}

fn ntbs<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8]> {
    let size = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated ARM attribute string")?;
    ensure!(
        size <= MAX_NTBS_SIZE,
        "ARM attribute string exceeds 4096 bytes"
    );
    let value = &bytes[..size];
    *bytes = &bytes[size + 1..];
    Ok(value)
}

fn uleb(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0_u64;
    for index in 0..10 {
        let byte = *bytes.first().context("truncated ARM attribute integer")?;
        *bytes = &bytes[1..];
        ensure!(index != 9 || byte <= 1, "ARM attribute integer overflow");
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            ensure!(
                index == 0 || byte != 0,
                "non-canonical ARM attribute integer"
            );
            return Ok(value);
        }
    }
    bail!("ARM attribute integer overflow")
}

#[cfg(test)]
#[path = "arm_tests.rs"]
mod tests;
