//! Just enough ELF for the two things this workspace asks of it.
//!
//! `aros-collect` needs section names and the symbols that mark a symbol set or
//! a library-version requirement. The boot checker needs section geometry and
//! symbol addresses, so it can model how the bootstrap's loader placed a
//! relocatable kickstart in memory and turn a faulting instruction pointer back
//! into a symbol.
//!
//! Both used to be served by a reader of their own, which is one reader too
//! many for one format. Kept format-level on purpose: the AROS-specific parts,
//! what a symbol set means and how the loader packs sections, belong to the
//! callers.

use anyhow::{bail, Context, Result};

/// ELF class, which fixes the width of every offset below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Elf32,
    Elf64,
}

impl Class {
    /// Bytes per pointer.
    #[must_use]
    pub const fn pointer_bytes(self) -> u64 {
        match self {
            Self::Elf32 => 4,
            Self::Elf64 => 8,
        }
    }

    /// The linker-script data command that emits one pointer-sized word.
    #[must_use]
    pub const fn pointer_directive(self) -> &'static str {
        match self {
            Self::Elf32 => "LONG",
            Self::Elf64 => "QUAD",
        }
    }
}

/// Where a symbol lives, to the extent the callers care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// `SHN_ABS`, which is how an absolute assembler symbol lands.
    Absolute,
    /// `SHN_UNDEF`.
    Undefined,
    /// Defined in the section of the given index.
    Section(u16),
}

/// A symbol's binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Local,
    Global,
    Weak,
    Other(u8),
}

/// One section header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub index: u16,
    pub name: String,
    pub kind: u32,
    pub flags: u64,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
    pub link: u32,
    pub entsize: u64,
}

impl Section {
    #[must_use]
    pub const fn is_alloc(&self) -> bool {
        self.flags & SHF_ALLOC != 0
    }

    #[must_use]
    pub const fn is_write(&self) -> bool {
        self.flags & SHF_WRITE != 0
    }

    #[must_use]
    pub const fn is_nobits(&self) -> bool {
        self.kind == SHT_NOBITS
    }
}

/// One symbol-table entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub home: Home,
    pub binding: Binding,
}

/// What this reader returns.
#[derive(Debug, Clone)]
pub struct Object {
    pub class: Class,
    /// Section headers in index order, index 0 included.
    pub sections: Vec<Section>,
    /// Symbols from `.symtab`, in symbol-table order.
    pub symbols: Vec<Symbol>,
}

impl Object {
    /// Section names in index order, for a caller that wants only those.
    #[must_use]
    pub fn section_names(&self) -> Vec<String> {
        self.sections
            .iter()
            .map(|section| section.name.clone())
            .collect()
    }
}

pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_NOBITS: u32 = 8;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const SHN_XINDEX: u16 = 0xffff;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    let slice: [u8; 2] = bytes
        .get(at..at + 2)
        .context("truncated ELF")?
        .try_into()
        .context("truncated ELF")?;
    Ok(u16::from_le_bytes(slice))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let slice: [u8; 4] = bytes
        .get(at..at + 4)
        .context("truncated ELF")?
        .try_into()
        .context("truncated ELF")?;
    Ok(u32::from_le_bytes(slice))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64> {
    let slice: [u8; 8] = bytes
        .get(at..at + 8)
        .context("truncated ELF")?
        .try_into()
        .context("truncated ELF")?;
    Ok(u64::from_le_bytes(slice))
}

/// A NUL-terminated name out of a string table.
fn string_at(table: &[u8], at: usize) -> String {
    table
        .get(at..)
        .and_then(|rest| rest.split(|byte| *byte == 0).next())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

/// Reads the class, the section headers and the symbols of an ELF file.
pub fn read(bytes: &[u8]) -> Result<Object> {
    if bytes.get(..4) != Some(b"\x7fELF") {
        bail!("not an ELF file");
    }
    let class = match bytes.get(4) {
        Some(1) => Class::Elf32,
        Some(2) => Class::Elf64,
        other => bail!("unknown ELF class {other:?}"),
    };
    // Every AROS target this build supports is little-endian, and a big-endian
    // object would make every field below wrong rather than merely unhandled.
    if bytes.get(5) != Some(&1) {
        bail!("only little-endian ELF is handled");
    }

    let (shoff, shentsize, shnum_field, shstrndx_field) = match class {
        Class::Elf64 => (
            u64_at(bytes, 0x28)?,
            u16_at(bytes, 0x3a)? as usize,
            u16_at(bytes, 0x3c)? as usize,
            u16_at(bytes, 0x3e)?,
        ),
        Class::Elf32 => (
            u64::from(u32_at(bytes, 0x20)?),
            u16_at(bytes, 0x2e)? as usize,
            u16_at(bytes, 0x30)? as usize,
            u16_at(bytes, 0x32)?,
        ),
    };
    if shoff == 0 || shentsize == 0 {
        return Ok(Object {
            class,
            sections: Vec::new(),
            symbols: Vec::new(),
        });
    }
    let shoff = usize::try_from(shoff).context("section table beyond addressable range")?;

    // A file with more than 0xff00 sections keeps the real count and the real
    // name-table index in section 0, whose own fields are otherwise unused.
    let first = raw_section(bytes, shoff, 0, class)?;
    let shnum = if shnum_field == 0 {
        usize::try_from(first.size).context("section count beyond addressable range")?
    } else {
        shnum_field
    };
    let shstrndx = if shstrndx_field == SHN_XINDEX {
        first.link as usize
    } else {
        shstrndx_field as usize
    };
    if shstrndx >= shnum {
        bail!("section name table index {shstrndx} is out of range");
    }

    let names_header = raw_section(bytes, shoff, shstrndx, class)?;
    let names = table_bytes(bytes, &names_header)?;

    let _ = shentsize;
    let mut sections = Vec::with_capacity(shnum);
    let mut symtab: Option<RawSection> = None;
    for index in 0..shnum {
        let mut section = raw_section(bytes, shoff, index, class)?;
        section.name = string_at(names, section.name_offset as usize);
        if section.kind == SHT_SYMTAB && section.entsize != 0 {
            symtab = Some(section.clone());
        }
        sections.push(section.into());
    }

    let symbols = if let Some(header) = symtab {
        read_symbols(bytes, shoff, class, &header)?
    } else {
        Vec::new()
    };

    Ok(Object {
        class,
        sections,
        symbols,
    })
}

/// A section header before its name is resolved.
#[derive(Debug, Clone)]
struct RawSection {
    index: u16,
    name_offset: u32,
    name: String,
    kind: u32,
    flags: u64,
    offset: u64,
    size: u64,
    align: u64,
    link: u32,
    entsize: u64,
}

impl From<RawSection> for Section {
    fn from(raw: RawSection) -> Self {
        Self {
            index: raw.index,
            name: raw.name,
            kind: raw.kind,
            flags: raw.flags,
            offset: raw.offset,
            size: raw.size,
            align: raw.align,
            link: raw.link,
            entsize: raw.entsize,
        }
    }
}

fn raw_section(bytes: &[u8], shoff: usize, index: usize, class: Class) -> Result<RawSection> {
    let entsize = match class {
        Class::Elf64 => 0x40,
        Class::Elf32 => 0x28,
    };
    let at = shoff + index * entsize;
    let (name_offset, kind, flags, offset, size, link, table_entsize, align) = match class {
        Class::Elf64 => (
            u32_at(bytes, at)?,
            u32_at(bytes, at + 4)?,
            u64_at(bytes, at + 8)?,
            u64_at(bytes, at + 0x18)?,
            u64_at(bytes, at + 0x20)?,
            u32_at(bytes, at + 0x28)?,
            u64_at(bytes, at + 0x38)?,
            u64_at(bytes, at + 0x30)?,
        ),
        Class::Elf32 => (
            u32_at(bytes, at)?,
            u32_at(bytes, at + 4)?,
            u64::from(u32_at(bytes, at + 8)?),
            u64::from(u32_at(bytes, at + 0x10)?),
            u64::from(u32_at(bytes, at + 0x14)?),
            u32_at(bytes, at + 0x18)?,
            u64::from(u32_at(bytes, at + 0x24)?),
            u64::from(u32_at(bytes, at + 0x20)?),
        ),
    };
    Ok(RawSection {
        index: u16::try_from(index).unwrap_or(u16::MAX),
        name_offset,
        name: String::new(),
        kind,
        flags,
        offset,
        size,
        align,
        link,
        entsize: table_entsize,
    })
}

fn table_bytes<'a>(bytes: &'a [u8], header: &RawSection) -> Result<&'a [u8]> {
    let start = usize::try_from(header.offset).context("table beyond addressable range")?;
    let end = start
        .checked_add(usize::try_from(header.size).context("table size out of range")?)
        .context("table beyond addressable range")?;
    bytes.get(start..end).context("truncated table")
}

fn read_symbols(
    bytes: &[u8],
    shoff: usize,
    class: Class,
    symtab: &RawSection,
) -> Result<Vec<Symbol>> {
    let strtab = raw_section(bytes, shoff, symtab.link as usize, class)?;
    let names = table_bytes(bytes, &strtab)?;

    let base = usize::try_from(symtab.offset).context("symbol table beyond range")?;
    let entsize = usize::try_from(symtab.entsize).context("symbol entry size out of range")?;
    let count = usize::try_from(symtab.size).context("symbol table size out of range")? / entsize;

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let at = base + index * entsize;
        let (name, info, shndx, value, size) = match class {
            Class::Elf64 => (
                u32_at(bytes, at)?,
                *bytes.get(at + 4).context("truncated symbol")?,
                u16_at(bytes, at + 6)?,
                u64_at(bytes, at + 8)?,
                u64_at(bytes, at + 0x10)?,
            ),
            Class::Elf32 => (
                u32_at(bytes, at)?,
                *bytes.get(at + 0xc).context("truncated symbol")?,
                u16_at(bytes, at + 0xe)?,
                u64::from(u32_at(bytes, at + 4)?),
                u64::from(u32_at(bytes, at + 8)?),
            ),
        };
        out.push(Symbol {
            name: string_at(names, name as usize),
            value,
            size,
            home: match shndx {
                SHN_ABS => Home::Absolute,
                SHN_UNDEF => Home::Undefined,
                other => Home::Section(other),
            },
            binding: match info >> 4 {
                STB_LOCAL => Binding::Local,
                STB_GLOBAL => Binding::Global,
                STB_WEAK => Binding::Weak,
                other => Binding::Other(other),
            },
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_elf_input_is_refused() {
        assert!(read(b"not an object").is_err());
    }

    #[test]
    fn a_truncated_header_is_refused() {
        assert!(read(b"\x7fELF\x02\x01").is_err());
    }

    #[test]
    fn a_big_endian_object_is_refused_rather_than_misread() {
        assert!(read(b"\x7fELF\x02\x02").is_err());
    }
}
