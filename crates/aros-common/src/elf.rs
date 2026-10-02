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

use anyhow::{bail, ensure, Context, Result};

pub mod riscv;

/// ELF class, which fixes the width of every offset below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Elf32,
    Elf64,
}

/// ELF OS ABI value assigned to AROS.
pub const OS_ABI_AROS: u8 = 15;
/// AROS ABI revision emitted by the supported toolchains.
pub const AROS_ABI_VERSION: u8 = 1;

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
    /// ELF `e_type`; compiler probes must distinguish objects from final links.
    pub kind: u16,
    /// ELF `e_machine`, independently measured rather than inferred from a name.
    pub machine: u16,
    /// Architecture-specific ELF `e_flags` (including the RISC-V floating ABI).
    pub flags: u32,
    /// ELF `EI_OSABI` byte from the object identity.
    pub os_abi: u8,
    /// ELF `EI_ABIVERSION` byte from the object identity.
    pub abi_version: u8,
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
const SHN_LORESERVE: u16 = 0xff00;
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

/// Limits copied names across both tables, including shared string suffixes.
struct NameBudget(usize);

impl NameBudget {
    fn new(file_bytes: usize) -> Self {
        Self(file_bytes.saturating_mul(4).min(64 * 1024 * 1024))
    }

    fn read(&mut self, table: &[u8], at: usize) -> Result<String> {
        if table.is_empty() && at == 0 {
            return Ok(String::new());
        }
        let rest = table.get(at..).context("ELF name offset out of range")?;
        let size = rest
            .iter()
            .position(|byte| *byte == 0)
            .context("unterminated ELF name")?;
        ensure!(size <= self.0, "ELF name allocation budget exceeded");
        let value = String::from_utf8_lossy(&rest[..size]);
        self.0 = self
            .0
            .checked_sub(value.len())
            .context("ELF name allocation budget exceeded")?;
        Ok(value.into_owned())
    }
}

/// Reads the class, the section headers and the symbols of an ELF file.
///
/// # Errors
///
/// Returns an error for malformed, truncated, unsupported, or non-ELF input.
/// Copied names are limited to four times the file size, at most 64 MiB, to
/// prevent repeated string-table offsets from amplifying allocations.
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
    let (header_size, header_size_offset, expected_stride) = match class {
        Class::Elf32 => (52, 0x28, 40),
        Class::Elf64 => (64, 0x34, 64),
    };
    ensure!(bytes.len() >= header_size, "truncated ELF header");
    ensure!(
        bytes[6] == 1 && u32_at(bytes, 0x14)? == 1,
        "unsupported ELF header version"
    );
    ensure!(
        usize::from(u16_at(bytes, header_size_offset)?) == header_size,
        "unsupported ELF header size"
    );
    let kind = u16_at(bytes, 0x10)?;
    let machine = u16_at(bytes, 0x12)?;
    let flags = u32_at(
        bytes,
        match class {
            Class::Elf32 => 0x24,
            Class::Elf64 => 0x30,
        },
    )?;

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
    if shoff == 0 {
        ensure!(
            (shentsize == 0 || shentsize == expected_stride)
                && shnum_field == 0
                && shstrndx_field == 0,
            "inconsistent absent ELF section table"
        );
        return Ok(Object {
            class,
            kind,
            machine,
            flags,
            os_abi: bytes[7],
            abi_version: bytes[8],
            sections: Vec::new(),
            symbols: Vec::new(),
        });
    }
    if shentsize != expected_stride {
        bail!("unsupported ELF section-header stride {shentsize}");
    }
    ensure!(
        shnum_field < usize::from(SHN_LORESERVE)
            && (shstrndx_field < SHN_LORESERVE || shstrndx_field == SHN_XINDEX),
        "reserved ELF section indices require extended encoding"
    );
    let shoff = usize::try_from(shoff).context("section table beyond addressable range")?;

    // A file with more than 0xff00 sections keeps the real count and the real
    // name-table index in section 0, whose own fields are otherwise unused.
    let first = raw_section(bytes, shoff, 0, class)?;
    let shnum = if shnum_field == 0 {
        usize::try_from(first.size).context("section count beyond addressable range")?
    } else {
        shnum_field
    };
    // Public section indices are u16. Refuse unsupported extended counts
    // rather than silently saturating indices, and validate the complete span
    // before using an untrusted count for allocation or iteration.
    ensure!(
        shnum <= usize::from(u16::MAX) + 1,
        "unsupported ELF section count"
    );
    let table_end = shnum
        .checked_mul(shentsize)
        .and_then(|size| shoff.checked_add(size))
        .context("section table range overflow")?;
    ensure!(table_end <= bytes.len(), "truncated section table");
    let shstrndx = if shstrndx_field == SHN_XINDEX {
        first.link as usize
    } else {
        shstrndx_field as usize
    };
    if shstrndx >= shnum {
        bail!("section name table index {shstrndx} is out of range");
    }

    let names = if shstrndx == 0 {
        &[][..]
    } else {
        let names_header = raw_section(bytes, shoff, shstrndx, class)?;
        ensure!(
            names_header.kind == SHT_STRTAB,
            "invalid section name table type"
        );
        table_bytes(bytes, &names_header)?
    };

    let mut sections = Vec::with_capacity(shnum);
    let mut name_budget = NameBudget::new(bytes.len());
    let mut symtab: Option<RawSection> = None;
    for index in 0..shnum {
        let mut section = raw_section(bytes, shoff, index, class)?;
        section.name = name_budget.read(names, section.name_offset as usize)?;
        if section.kind == SHT_SYMTAB {
            symtab = Some(section.clone());
        }
        sections.push(section.into());
    }

    let symbols = if let Some(header) = symtab {
        read_symbols(bytes, shoff, shnum, class, &header, &mut name_budget)?
    } else {
        Vec::new()
    };

    Ok(Object {
        class,
        kind,
        machine,
        flags,
        os_abi: bytes[7],
        abi_version: bytes[8],
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
    let at = index
        .checked_mul(entsize)
        .and_then(|offset| shoff.checked_add(offset))
        .context("section header range overflow")?;
    let end = at
        .checked_add(entsize)
        .context("section header range overflow")?;
    // Read all fields relative to a checked slice. Offsets inside the entry
    // cannot overflow even when the file supplies an extreme table offset.
    let bytes = bytes.get(at..end).context("truncated section header")?;
    let at = 0;
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
        index: u16::try_from(index).context("section index out of range")?,
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
    shnum: usize,
    class: Class,
    symtab: &RawSection,
    name_budget: &mut NameBudget,
) -> Result<Vec<Symbol>> {
    ensure!(
        (symtab.link as usize) < shnum,
        "symbol string table index out of range"
    );
    let strtab = raw_section(bytes, shoff, symtab.link as usize, class)?;
    ensure!(
        strtab.kind == SHT_STRTAB,
        "invalid symbol string table type"
    );
    let names = table_bytes(bytes, &strtab)?;

    let entsize = usize::try_from(symtab.entsize).context("symbol entry size out of range")?;
    let expected = match class {
        Class::Elf32 => 16,
        Class::Elf64 => 24,
    };
    ensure!(entsize == expected, "unsupported ELF symbol entry size");
    let bytes = table_bytes(bytes, symtab)?;
    ensure!(bytes.len() % entsize == 0, "partial ELF symbol entry");
    let count = bytes.len() / entsize;

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let at = index * entsize;
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
            name: name_budget.read(names, name as usize)?,
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
#[path = "elf/tests.rs"]
mod tests;
