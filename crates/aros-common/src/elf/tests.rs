//! ELF identity, bounds and table counter-probes.

use super::*;

fn header(class: u8) -> Vec<u8> {
    let (size, size_offset) = if class == 1 { (52, 0x28) } else { (64, 0x34) };
    let mut bytes = vec![0; size];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = class;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[0x14..0x18].copy_from_slice(&1_u32.to_le_bytes());
    bytes[size_offset..size_offset + 2].copy_from_slice(&(size as u16).to_le_bytes());
    bytes
}

fn section_table() -> Vec<u8> {
    let mut bytes = header(2);
    bytes.resize(64 + 3 * 64 + 1 + 24, 0);
    bytes[0x28..0x30].copy_from_slice(&64_u64.to_le_bytes());
    bytes[0x3a..0x3c].copy_from_slice(&64_u16.to_le_bytes());
    bytes[0x3c..0x3e].copy_from_slice(&3_u16.to_le_bytes());
    // A string table and a symbol table containing the mandatory null symbol.
    bytes[132..136].copy_from_slice(&SHT_STRTAB.to_le_bytes());
    bytes[152..160].copy_from_slice(&256_u64.to_le_bytes());
    bytes[160..168].copy_from_slice(&1_u64.to_le_bytes());
    bytes[196..200].copy_from_slice(&SHT_SYMTAB.to_le_bytes());
    bytes[216..224].copy_from_slice(&257_u64.to_le_bytes());
    bytes[224..232].copy_from_slice(&24_u64.to_le_bytes());
    bytes[232..236].copy_from_slice(&1_u32.to_le_bytes());
    bytes[248..256].copy_from_slice(&24_u64.to_le_bytes());
    bytes
}

#[test]
fn a_non_elf_or_truncated_or_big_endian_input_is_refused() {
    for bytes in [
        b"not an object".as_slice(),
        b"\x7fELF\x02\x01",
        b"\x7fELF\x02\x02",
    ] {
        assert!(read(bytes).is_err());
    }
}

#[test]
fn retains_the_aros_elf_identity() {
    let mut object = header(2);
    object[7] = OS_ABI_AROS;
    object[8] = AROS_ABI_VERSION;
    let parsed = read(&object).unwrap();
    assert_eq!(parsed.class, Class::Elf64);
    assert_eq!(parsed.os_abi, OS_ABI_AROS);
    assert_eq!(parsed.abi_version, AROS_ABI_VERSION);
}

#[test]
fn measures_machine_type_and_flags_for_both_widths() {
    for (class, flag_offset) in [(1, 0x24), (2, 0x30)] {
        let mut bytes = header(class);
        bytes[0x10..0x12].copy_from_slice(&2_u16.to_le_bytes());
        bytes[0x12..0x14].copy_from_slice(&243_u16.to_le_bytes());
        bytes[flag_offset..flag_offset + 4].copy_from_slice(&5_u32.to_le_bytes());
        let object = read(&bytes).unwrap();
        assert_eq!((object.kind, object.machine, object.flags), (2, 243, 5));
        bytes[0x12..0x14].copy_from_slice(&62_u16.to_le_bytes());
        bytes[flag_offset..flag_offset + 4].copy_from_slice(&3_u32.to_le_bytes());
        let different = read(&bytes).unwrap();
        assert_eq!((different.machine, different.flags), (62, 3));
    }
}

#[test]
fn validates_header_version_size_and_absent_table_consistency() {
    for class in [1, 2] {
        let (size_offset, stride_offset, count_offset, stride) = if class == 1 {
            (0x28, 0x2e, 0x30, 40_u16)
        } else {
            (0x34, 0x3a, 0x3c, 64_u16)
        };
        let mut bytes = header(class);
        bytes[stride_offset..stride_offset + 2].copy_from_slice(&stride.to_le_bytes());
        assert!(read(&bytes).is_ok(), "absent table may retain entry size");
        for offset in [6, 0x14, size_offset] {
            let mut invalid = bytes.clone();
            invalid[offset] = 0;
            assert!(read(&invalid).is_err(), "header offset {offset}");
        }
        bytes[count_offset] = 1;
        assert!(read(&bytes).is_err());
    }
}

#[test]
fn rejects_section_table_offsets_strides_and_counts_before_allocation() {
    assert_eq!(read(&section_table()).unwrap().sections.len(), 3);
    for offset in [u64::MAX, 280, 64] {
        let mut bytes = section_table();
        bytes[0x28..0x30].copy_from_slice(&offset.to_le_bytes());
        if offset == 64 {
            bytes[0x3a..0x3c].copy_from_slice(&63_u16.to_le_bytes());
        }
        assert!(read(&bytes).is_err());
    }
    for count in [u64::MAX, 65_537, 5] {
        let mut bytes = section_table();
        bytes[0x3c..0x3e].fill(0);
        bytes[96..104].copy_from_slice(&count.to_le_bytes());
        assert!(read(&bytes).is_err(), "extended section count {count}");
    }
    let mut extended = section_table();
    extended[0x3c..0x3e].fill(0);
    extended[96..104].copy_from_slice(&3_u64.to_le_bytes());
    assert_eq!(read(&extended).unwrap().sections.len(), 3);
}

#[test]
fn rejects_invalid_symbol_geometry_and_links_before_allocation() {
    assert_eq!(read(&section_table()).unwrap().symbols.len(), 1);
    for (at, value) in [
        (216, u64::MAX),
        (224, u64::MAX),
        (224, 23),
        (248, 0),
        (248, 1),
        (248, 25),
    ] {
        let mut bytes = section_table();
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        assert!(read(&bytes).is_err(), "symbol geometry {at}:{value}");
    }
    let mut bytes = section_table();
    bytes[232..236].copy_from_slice(&3_u32.to_le_bytes());
    assert!(read(&bytes).is_err());
    bytes[232..236].copy_from_slice(&0_u32.to_le_bytes());
    assert!(read(&bytes).is_err(), "null section is not a string table");
}

#[test]
fn reserved_section_indices_require_extended_encoding() {
    for (offset, value) in [
        (0x3c, 0xff00_u16),
        (0x3c, 0xffff),
        (0x3e, 0xff00),
        (0x3e, 0xfffe),
    ] {
        let mut bytes = section_table();
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        assert!(read(&bytes)
            .unwrap_err()
            .to_string()
            .contains("extended encoding"));
    }
}

#[test]
fn repeated_string_offsets_cannot_amplify_name_allocations() {
    let mut bytes = header(2);
    let payload = 64 + 13 * 64;
    bytes.resize(payload + 1026, b'a');
    bytes[64..payload].fill(0);
    bytes[0x28..0x30].copy_from_slice(&64_u64.to_le_bytes());
    bytes[0x3a..0x3c].copy_from_slice(&64_u16.to_le_bytes());
    bytes[0x3c..0x3e].copy_from_slice(&13_u16.to_le_bytes());
    bytes[0x3e..0x40].copy_from_slice(&1_u16.to_le_bytes());
    bytes[132..136].copy_from_slice(&SHT_STRTAB.to_le_bytes());
    bytes[152..160].copy_from_slice(&(payload as u64).to_le_bytes());
    bytes[160..168].copy_from_slice(&1026_u64.to_le_bytes());
    bytes[payload] = 0;
    *bytes.last_mut().unwrap() = 0;
    assert!(read(&bytes).is_ok());
    for index in 1..13 {
        bytes[64 + index * 64..68 + index * 64].copy_from_slice(&1_u32.to_le_bytes());
    }
    assert!(read(&bytes)
        .unwrap_err()
        .to_string()
        .contains("name allocation budget"));
}

#[test]
fn invalid_or_unterminated_names_fail_instead_of_becoming_empty() {
    let mut budget = NameBudget::new(64);
    assert_eq!(budget.read(b"\0valid\0", 1).unwrap(), "valid");
    assert!(budget.read(b"\0valid\0", 99).is_err());
    assert!(budget.read(b"\0unterminated", 1).is_err());
}
