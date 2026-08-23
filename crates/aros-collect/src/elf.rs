//! Just enough ELF to read a relocatable object's section names and symbols.
//!
//! `collect-aros` asks `objdump` for the sections and `nm` for the symbols
//! (`tools/collect-aros/backend-generic.c:31` and `:64`). Reading the headers
//! directly keeps this to one file with no dependency and no second process per
//! link, and neither table is a hard part of the format.

use anyhow::{bail, Context, Result};

/// ELF class, which decides how wide a set entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Elf32,
    Elf64,
}

impl Class {
    /// Bytes per pointer, and so per symbol-set entry.
    pub const fn pointer_bytes(self) -> u64 {
        match self {
            Self::Elf32 => 4,
            Self::Elf64 => 8,
        }
    }

    /// The linker-script data command that emits one pointer-sized word.
    /// `tools/collect-aros/gensets.c:164` picks it the same way, from the
    /// output format rather than from the host.
    pub const fn pointer_directive(self) -> &'static str {
        match self {
            Self::Elf32 => "LONG",
            Self::Elf64 => "QUAD",
        }
    }
}

/// Where a symbol lives, to the extent the collectors care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// `SHN_ABS`, which is how an absolute assembler symbol lands.
    Absolute,
    /// `SHN_UNDEF`.
    Undefined,
    /// Defined in one of the object's sections.
    Section,
}

/// A symbol's binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Local,
    Global,
    Weak,
    Other(u8),
}

/// A symbol, reduced to what the collectors ask about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub home: Home,
    pub binding: Binding,
}

/// What this needs from an object file.
#[derive(Debug, Clone)]
pub struct Object {
    pub class: Class,
    /// Section names in section-header order, index 0 included.
    pub sections: Vec<String>,
    /// Symbols from `.symtab`, in symbol-table order.
    pub symbols: Vec<Symbol>,
}

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

const SHT_SYMTAB: u32 = 2;
const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;

/// One section header's name index, file offset and size.
struct Header {
    name: u32,
    kind: u32,
    offset: u64,
    size: u64,
    link: u32,
    entsize: u64,
}

fn header_at(bytes: &[u8], at: usize, class: Class) -> Result<Header> {
    match class {
        Class::Elf64 => Ok(Header {
            name: u32_at(bytes, at)?,
            kind: u32_at(bytes, at + 4)?,
            offset: u64_at(bytes, at + 0x18)?,
            size: u64_at(bytes, at + 0x20)?,
            link: u32_at(bytes, at + 0x28)?,
            entsize: u64_at(bytes, at + 0x38)?,
        }),
        Class::Elf32 => Ok(Header {
            name: u32_at(bytes, at)?,
            kind: u32_at(bytes, at + 4)?,
            offset: u64::from(u32_at(bytes, at + 0x10)?),
            size: u64::from(u32_at(bytes, at + 0x14)?),
            link: u32_at(bytes, at + 0x18)?,
            entsize: u64::from(u32_at(bytes, at + 0x24)?),
        }),
    }
}

/// A NUL-terminated name out of a string table.
fn string_at(table: &[u8], at: usize) -> String {
    table
        .get(at..)
        .and_then(|rest| rest.split(|byte| *byte == 0).next())
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

/// Reads the class and the section names of an ELF object.
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
            u16_at(bytes, 0x3e)? as usize,
        ),
        Class::Elf32 => (
            u64::from(u32_at(bytes, 0x20)?),
            u16_at(bytes, 0x2e)? as usize,
            u16_at(bytes, 0x30)? as usize,
            u16_at(bytes, 0x32)? as usize,
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
    // name-table index in section 0, whose own fields are otherwise unused. A
    // module with that many sections is unlikely but the fallback is two lines.
    let first = header_at(bytes, shoff, class)?;
    let shnum = if shnum_field == 0 {
        usize::try_from(first.size).context("section count beyond addressable range")?
    } else {
        shnum_field
    };
    let shstrndx = if shstrndx_field == 0xffff {
        first.link as usize
    } else {
        shstrndx_field
    };
    if shstrndx >= shnum {
        bail!("section name table index {shstrndx} is out of range");
    }

    let names = header_at(bytes, shoff + shstrndx * shentsize, class)?;
    let start = usize::try_from(names.offset).context("name table beyond addressable range")?;
    let end = start
        .checked_add(usize::try_from(names.size).context("name table size out of range")?)
        .context("name table beyond addressable range")?;
    let table = bytes.get(start..end).context("truncated name table")?;

    let mut sections = Vec::with_capacity(shnum);
    let mut symtab = None;
    for index in 0..shnum {
        let header = header_at(bytes, shoff + index * shentsize, class)?;
        sections.push(string_at(table, header.name as usize));
        if header.kind == SHT_SYMTAB && header.entsize != 0 {
            symtab = Some(header);
        }
    }

    let symbols = if let Some(header) = symtab {
        read_symbols(bytes, shoff, shentsize, class, &header)?
    } else {
        Vec::new()
    };

    Ok(Object {
        class,
        sections,
        symbols,
    })
}

fn read_symbols(
    bytes: &[u8],
    shoff: usize,
    shentsize: usize,
    class: Class,
    symtab: &Header,
) -> Result<Vec<Symbol>> {
    let strtab = header_at(bytes, shoff + symtab.link as usize * shentsize, class)?;
    let start = usize::try_from(strtab.offset).context("string table beyond range")?;
    let end = start
        .checked_add(usize::try_from(strtab.size).context("string table size out of range")?)
        .context("string table beyond range")?;
    let names = bytes.get(start..end).context("truncated string table")?;

    let base = usize::try_from(symtab.offset).context("symbol table beyond range")?;
    let entsize = usize::try_from(symtab.entsize).context("symbol entry size out of range")?;
    let count = usize::try_from(symtab.size).context("symbol table size out of range")? / entsize;

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let at = base + index * entsize;
        let (name, info, shndx) = match class {
            Class::Elf64 => (
                u32_at(bytes, at)?,
                *bytes.get(at + 4).context("truncated symbol")?,
                u16_at(bytes, at + 6)?,
            ),
            Class::Elf32 => (
                u32_at(bytes, at)?,
                *bytes.get(at + 0xc).context("truncated symbol")?,
                u16_at(bytes, at + 0xe)?,
            ),
        };
        let home = match shndx {
            SHN_ABS => Home::Absolute,
            SHN_UNDEF => Home::Undefined,
            _ => Home::Section,
        };
        let binding = match info >> 4 {
            STB_LOCAL => Binding::Local,
            STB_GLOBAL => Binding::Global,
            STB_WEAK => Binding::Weak,
            other => Binding::Other(other),
        };
        out.push(Symbol {
            name: string_at(names, name as usize),
            home,
            binding,
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
}
