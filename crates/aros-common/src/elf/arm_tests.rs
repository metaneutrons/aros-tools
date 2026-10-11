//! Synthetic ELF32 fixtures and counter-probes for the bounded ARM reader.

use super::*;

const SECTION_NAMES: &[u8] = b"\0.shstrtab\0.ARM.attributes\0";

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn object_with_sections(
    attribute_sections: &[&[u8]],
    class: u8,
    machine: u16,
    section_kind: u32,
) -> Vec<u8> {
    let (header_size, stride) = match class {
        1 => (52_usize, 40_usize),
        2 => (64_usize, 64_usize),
        _ => panic!("fixture class must be ELF32 or ELF64"),
    };
    let section_count = 2 + attribute_sections.len();
    let section_table = header_size;
    let payload = section_table + section_count * stride;
    let attributes_size: usize = attribute_sections.iter().map(|data| data.len()).sum();
    let mut bytes = vec![0_u8; payload + SECTION_NAMES.len() + attributes_size];

    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = class;
    bytes[5] = 1;
    bytes[6] = 1;
    put_u16(&mut bytes, 0x10, 1);
    put_u16(&mut bytes, 0x12, machine);
    put_u32(&mut bytes, 0x14, 1);
    if class == 1 {
        put_u32(&mut bytes, 0x20, section_table as u32);
        put_u16(&mut bytes, 0x28, header_size as u16);
        put_u16(&mut bytes, 0x2e, stride as u16);
        put_u16(&mut bytes, 0x30, section_count as u16);
        put_u16(&mut bytes, 0x32, 1);
    } else {
        put_u64(&mut bytes, 0x28, section_table as u64);
        put_u16(&mut bytes, 0x34, header_size as u16);
        put_u16(&mut bytes, 0x3a, stride as u16);
        put_u16(&mut bytes, 0x3c, section_count as u16);
        put_u16(&mut bytes, 0x3e, 1);
    }

    let names_offset = payload;
    let name_header = section_table + stride;
    put_u32(&mut bytes, name_header, 1);
    put_u32(&mut bytes, name_header + 4, super::super::SHT_STRTAB);
    write_section_range(
        &mut bytes,
        class,
        name_header,
        names_offset,
        SECTION_NAMES.len(),
    );

    let mut next_offset = names_offset + SECTION_NAMES.len();
    for (index, data) in attribute_sections.iter().enumerate() {
        let header = section_table + (index + 2) * stride;
        put_u32(&mut bytes, header, 11);
        put_u32(&mut bytes, header + 4, section_kind);
        write_section_range(&mut bytes, class, header, next_offset, data.len());
        next_offset += data.len();
    }

    bytes[names_offset..names_offset + SECTION_NAMES.len()].copy_from_slice(SECTION_NAMES);
    next_offset = names_offset + SECTION_NAMES.len();
    for data in attribute_sections {
        bytes[next_offset..next_offset + data.len()].copy_from_slice(data);
        next_offset += data.len();
    }
    bytes
}

fn write_section_range(bytes: &mut [u8], class: u8, header: usize, offset: usize, size: usize) {
    if class == 1 {
        put_u32(bytes, header + 16, offset as u32);
        put_u32(bytes, header + 20, size as u32);
    } else {
        put_u64(bytes, header + 24, offset as u64);
        put_u64(bytes, header + 32, size as u64);
    }
}

fn arm_object(attribute_data: &[u8]) -> Vec<u8> {
    object_with_sections(&[attribute_data], 1, MACHINE, SHT_ATTRIBUTES)
}

fn encode_uleb(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        bytes.push(byte);
        if value == 0 {
            return bytes;
        }
    }
}

fn integer_attribute(tag: u64, value: u64) -> Vec<u8> {
    let mut bytes = encode_uleb(tag);
    bytes.extend(encode_uleb(value));
    bytes
}

fn string_attribute(tag: u64, value: &[u8]) -> Vec<u8> {
    let mut bytes = encode_uleb(tag);
    bytes.extend_from_slice(value);
    bytes.push(0);
    bytes
}

fn compatibility_attribute(flag: u64, vendor: &[u8]) -> Vec<u8> {
    let mut bytes = encode_uleb(32);
    bytes.extend(encode_uleb(flag));
    bytes.extend_from_slice(vendor);
    bytes.push(0);
    bytes
}

fn file_scope(attributes: &[u8]) -> Vec<u8> {
    let tag = encode_uleb(1);
    let length = tag.len() + 4 + attributes.len();
    let mut bytes = tag;
    bytes.extend(u32::try_from(length).unwrap().to_le_bytes());
    bytes.extend_from_slice(attributes);
    bytes
}

fn vendor_block(name: &[u8], body: &[u8]) -> Vec<u8> {
    let length = 4 + name.len() + 1 + body.len();
    let mut bytes = u32::try_from(length).unwrap().to_le_bytes().to_vec();
    bytes.extend_from_slice(name);
    bytes.push(0);
    bytes.extend_from_slice(body);
    bytes
}

fn section_with_vendors(vendors: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![b'A'];
    for vendor in vendors {
        bytes.extend_from_slice(vendor);
    }
    bytes
}

fn aeabi_section(attributes: &[u8]) -> Vec<u8> {
    section_with_vendors(&[vendor_block(b"aeabi", &file_scope(attributes))])
}

fn cortex_a7_attributes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(string_attribute(4, b"7-A"));
    bytes.extend(string_attribute(5, b"Cortex-A7"));
    bytes.extend(integer_attribute(6, 10));
    bytes.extend(integer_attribute(7, 65));
    bytes.extend(integer_attribute(9, 2));
    bytes.extend(integer_attribute(10, 5));
    bytes.extend(integer_attribute(12, 2));
    bytes.extend(integer_attribute(27, 3));
    bytes.extend(integer_attribute(28, 1));
    bytes.extend(compatibility_attribute(1, b"gnu"));
    bytes
}

fn compile_arm_attribute_probe(
    driver: &std::path::Path,
    source: &std::path::Path,
    output_path: &std::path::Path,
    cpu_options: &[&str],
) {
    let mut command = std::process::Command::new(driver);
    command
        .args(["--target=arm-unknown-aros", "-mfloat-abi=hard", "-O0", "-c"])
        .args(cpu_options)
        .arg(source)
        .arg("-o")
        .arg(output_path);
    let output = crate::run_output_with_timeout(
        &mut command,
        crate::DEFAULT_CAPTURE_LIMIT,
        std::time::Duration::from_secs(120),
    )
    .expect("run explicitly selected ARM Clang");
    assert!(
        !output.timed_out && output.status.success(),
        "ARM Clang failed or timed out: {}",
        output.stderr.rendered_lossy()
    );
    assert!(output_path.is_file(), "ARM Clang produced no object");
}

#[test]
#[ignore = "requires an explicitly selected installed driver at AROS_TEST_ARM_CLANG"]
fn installed_clang_arm_attributes_distinguish_cortex_a7_with_equal_eflags() {
    use std::fs;

    let driver = std::path::PathBuf::from(
        std::env::var_os("AROS_TEST_ARM_CLANG")
            .expect("set AROS_TEST_ARM_CLANG to an absolute existing Clang driver"),
    );
    assert!(
        driver.is_absolute(),
        "AROS_TEST_ARM_CLANG must be an absolute path"
    );
    assert!(
        driver.is_file(),
        "AROS_TEST_ARM_CLANG must name an existing file"
    );

    let temporary = tempfile::tempdir().expect("create owned ARM compiler probe directory");
    let source = temporary.path().join("arm-attribute-probe.c");
    let default_object = temporary.path().join("default-cpu.o");
    let cortex_a7_object = temporary.path().join("cortex-a7.o");
    let architecture_object = temporary.path().join("armv7-a.o");
    fs::write(
        &source,
        b"int arm_attribute_probe(int value) { return value + 7; }\n",
    )
    .expect("write tiny ARM C fixture");

    compile_arm_attribute_probe(&driver, &source, &default_object, &[]);
    compile_arm_attribute_probe(
        &driver,
        &source,
        &cortex_a7_object,
        &["-mcpu=cortex-a7", "-mfpu=neon-vfpv4"],
    );
    // The AROS LLVM configure projection selects an architecture, not a
    // Cortex model. Measure that spelling separately; do not derive its
    // attributes from the board name or substitute a tools-owned CPU tune.
    compile_arm_attribute_probe(
        &driver,
        &source,
        &architecture_object,
        &["-march=armv7-a+fp", "-mfpu=neon-vfpv4"],
    );

    let default_bytes = fs::read(default_object).expect("read default-CPU object");
    let cortex_a7_bytes = fs::read(cortex_a7_object).expect("read Cortex-A7 object");
    let architecture_bytes = fs::read(architecture_object).expect("read ARMv7-A object");
    let default_elf = crate::elf::read(&default_bytes).expect("decode default-CPU ELF");
    let cortex_a7_elf = crate::elf::read(&cortex_a7_bytes).expect("decode Cortex-A7 ELF");
    let architecture_elf = crate::elf::read(&architecture_bytes).expect("decode ARMv7-A ELF");
    assert_eq!(default_elf.class, crate::elf::Class::Elf32);
    assert_eq!(cortex_a7_elf.class, crate::elf::Class::Elf32);
    assert_eq!(default_elf.machine, MACHINE);
    assert_eq!(cortex_a7_elf.machine, MACHINE);
    assert_eq!(default_elf.flags, cortex_a7_elf.flags);
    assert_eq!(architecture_elf.class, crate::elf::Class::Elf32);
    assert_eq!(architecture_elf.machine, MACHINE);
    assert_eq!(default_elf.flags, architecture_elf.flags);

    let default_attributes = read(&default_bytes).expect("decode default-CPU ARM attributes");
    let cortex_a7_attributes = read(&cortex_a7_bytes).expect("decode Cortex-A7 ARM attributes");
    let architecture_attributes = read(&architecture_bytes).expect("decode ARMv7-A attributes");
    assert_ne!(
        default_attributes.integer(6).unwrap(),
        cortex_a7_attributes.integer(6).unwrap(),
        "CPU architecture attribute must distinguish the selected Cortex-A7"
    );
    assert_ne!(
        default_attributes.integer(10).unwrap(),
        cortex_a7_attributes.integer(10).unwrap(),
        "floating-point architecture attribute must change"
    );
    assert_ne!(
        default_attributes.integer(12).unwrap(),
        cortex_a7_attributes.integer(12).unwrap(),
        "Advanced SIMD architecture attribute must change"
    );

    for (tag, expected) in [(6, 10), (7, 65), (10, 5), (12, 2), (28, 1)] {
        assert_eq!(
            cortex_a7_attributes.integer(tag).unwrap(),
            expected,
            "Cortex-A7 ARM attribute tag {tag}"
        );
    }
    // These are measured from the installed LLVM 11 driver, not copied from
    // the Cortex-A7 case: its architecture+fp selection records VFPv3/NEONv1
    // even with the same explicit FPU spelling. Equal e_flags prove neither.
    for (tag, expected) in [(6, 10), (7, 65), (10, 3), (12, 1), (28, 1)] {
        assert_eq!(
            architecture_attributes.integer(tag).unwrap(),
            expected,
            "ARMv7-A/NEON-VFPv4 attribute tag {tag}; measured {:?}",
            architecture_attributes.values
        );
    }
    assert_ne!(
        cortex_a7_attributes.integer(10).unwrap(),
        architecture_attributes.integer(10).unwrap(),
        "architecture and CPU selections cannot share assumed FPU attributes"
    );
    assert_ne!(
        cortex_a7_attributes.integer(12).unwrap(),
        architecture_attributes.integer(12).unwrap(),
        "architecture and CPU selections cannot share assumed SIMD attributes"
    );
}

#[test]
fn reads_cortex_a7_values_strings_odd_numeric_tags_and_compatibility() {
    let attributes = read(&arm_object(&aeabi_section(&cortex_a7_attributes()))).unwrap();
    assert_eq!(attributes.integer(6).unwrap(), 10);
    assert_eq!(attributes.integer(7).unwrap(), 65);
    assert_eq!(attributes.integer(9).unwrap(), 2);
    assert_eq!(attributes.integer(10).unwrap(), 5);
    assert_eq!(attributes.integer(12).unwrap(), 2);
    assert_eq!(attributes.integer(27).unwrap(), 3);
    assert_eq!(attributes.integer(28).unwrap(), 1);
    assert_eq!(attributes.integer(8).unwrap(), 0);
    assert_eq!(attributes.values[&4], Value::String(b"7-A".to_vec()));
    assert_eq!(attributes.values[&5], Value::String(b"Cortex-A7".to_vec()));
    assert_eq!(
        attributes.values[&32],
        Value::Compatibility {
            flag: 1,
            vendor: b"gnu".to_vec()
        }
    );
    assert!(attributes.integer(4).is_err());
    assert!(attributes.integer(32).is_err());
    for tag in [4_u64, 5, 32, 33, 64, 65, 128] {
        let empty = decode(&aeabi_section(&[])).unwrap();
        assert!(
            empty.integer(tag).is_err(),
            "an omitted non-integer or mandatory unknown tag {tag} cannot default to zero"
        );
    }
}

#[test]
fn accepts_optional_future_tags_and_skips_a_framed_private_vendor() {
    let mut attrs = cortex_a7_attributes();
    attrs.extend(string_attribute(65, b"future-name"));
    attrs.extend(integer_attribute(66, 999));
    attrs.extend(string_attribute(193, b"later-name"));
    let private = vendor_block(b"private", b"opaque bytes, not aeabi scopes");
    let section = section_with_vendors(&[private, vendor_block(b"aeabi", &file_scope(&attrs))]);
    let parsed = read(&arm_object(&section)).unwrap();
    assert_eq!(parsed.values[&65], Value::String(b"future-name".to_vec()));
    assert_eq!(parsed.integer(66).unwrap(), 999);
    assert_eq!(parsed.values[&193], Value::String(b"later-name".to_vec()));
}

#[test]
fn binds_to_one_elf32_arm_attribute_section() {
    let section = aeabi_section(&cortex_a7_attributes());
    assert!(read(&object_with_sections(
        &[&section],
        2,
        MACHINE,
        SHT_ATTRIBUTES
    ))
    .unwrap_err()
    .to_string()
    .contains("ELF32"));
    assert!(
        read(&object_with_sections(&[&section], 1, 62, SHT_ATTRIBUTES))
            .unwrap_err()
            .to_string()
            .contains("ARM ELF")
    );
    assert!(read(&object_with_sections(&[], 1, MACHINE, SHT_ATTRIBUTES))
        .unwrap_err()
        .to_string()
        .contains("missing"));
    assert!(read(&object_with_sections(
        &[&section, &section],
        1,
        MACHINE,
        SHT_ATTRIBUTES
    ))
    .unwrap_err()
    .to_string()
    .contains("duplicate"));
    assert!(read(&object_with_sections(&[&section], 1, MACHINE, 1))
        .unwrap_err()
        .to_string()
        .contains("section type"));
}

#[test]
fn rejects_attribute_section_ranges_and_sizes_before_decoding() {
    let section = aeabi_section(&cortex_a7_attributes());
    let mut out_of_range = arm_object(&section);
    let section_header = 52 + 2 * 40;
    put_u32(&mut out_of_range, section_header + 16, u32::MAX);
    assert!(read(&out_of_range).is_err());

    let mut oversized = arm_object(&section);
    put_u32(&mut oversized, section_header + 20, 65_537);
    assert!(read(&oversized).unwrap_err().to_string().contains("64 KiB"));

    let complete = arm_object(&section);
    for size in 0..complete.len() {
        assert!(read(&complete[..size]).is_err(), "prefix {size}");
    }
}

#[test]
fn rejects_unknown_formats_vendors_and_malformed_vendor_framing() {
    assert!(decode(b"").is_err());
    assert!(decode(b"X").is_err());
    assert!(decode(b"A").unwrap_err().to_string().contains("missing"));

    let private_only = section_with_vendors(&[vendor_block(b"private", b"opaque")]);
    assert!(decode(&private_only)
        .unwrap_err()
        .to_string()
        .contains("missing aeabi"));

    for size in [0_u32, 3, u32::MAX] {
        let mut malformed = aeabi_section(&[]);
        malformed[1..5].copy_from_slice(&size.to_le_bytes());
        assert!(decode(&malformed).is_err(), "vendor length {size}");
    }

    let unterminated_name = section_with_vendors(&[{
        let mut bytes = 10_u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"aeabix");
        bytes
    }]);
    assert!(decode(&unterminated_name).is_err());

    let oversized_name = section_with_vendors(&[vendor_block(&vec![b'x'; 4097], b"")]);
    assert!(decode(&oversized_name)
        .unwrap_err()
        .to_string()
        .contains("4096"));
}

#[test]
fn rejects_duplicate_vendors_scopes_and_attribute_tags() {
    let one_file = file_scope(&integer_attribute(6, 1));
    let duplicate_file = [one_file.as_slice(), one_file.as_slice()].concat();
    let duplicate_file = section_with_vendors(&[vendor_block(b"aeabi", &duplicate_file)]);
    assert!(decode(&duplicate_file).is_err());

    let duplicate_aeabi = section_with_vendors(&[
        vendor_block(b"aeabi", &file_scope(&integer_attribute(6, 1))),
        vendor_block(b"aeabi", &file_scope(&integer_attribute(7, 1))),
    ]);
    assert!(decode(&duplicate_aeabi).is_err());

    let duplicate_private = section_with_vendors(&[
        vendor_block(b"private", b"one"),
        vendor_block(b"private", b"two"),
        vendor_block(b"aeabi", &one_file),
    ]);
    assert!(decode(&duplicate_private).is_err());

    for scope in [0_u64, 2, 3, 4] {
        let body = encode_uleb(scope);
        let section = section_with_vendors(&[vendor_block(b"aeabi", &body)]);
        assert!(decode(&section).is_err(), "scope {scope}");
    }

    let duplicate_tag = [integer_attribute(6, 1), integer_attribute(6, 2)].concat();
    assert!(decode(&aeabi_section(&duplicate_tag)).is_err());
}

#[test]
fn rejects_unknown_mandatory_tags_and_nodefaults() {
    for tag in [0_u64, 1, 2, 3, 33, 35, 40, 53, 63, 64, 128] {
        let bytes = integer_attribute(tag, 1);
        assert!(decode(&aeabi_section(&bytes)).is_err(), "tag {tag}");
    }
    for tag in [34_u64, 36, 38, 42, 44, 46, 48, 50, 52] {
        let bytes = integer_attribute(tag, 7);
        assert_eq!(
            decode(&aeabi_section(&bytes)).unwrap().values[&tag],
            Value::Integer(7),
            "tag {tag}"
        );
    }
}

#[test]
fn rejects_bad_integer_and_string_encodings_and_value_counts() {
    for encoding in [
        vec![0x80],
        vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
        vec![0x80, 0x00],
        vec![0x81, 0x00],
    ] {
        let mut remaining = encoding.as_slice();
        assert!(uleb(&mut remaining).is_err());
    }

    let mut truncated = encode_uleb(6);
    truncated.push(0x80);
    assert!(decode(&aeabi_section(&truncated)).is_err());

    let unterminated = [encode_uleb(4), b"raw-name".to_vec()].concat();
    assert!(decode(&aeabi_section(&unterminated)).is_err());

    let oversized_string = [encode_uleb(4), vec![b'x'; 4097], vec![0]].concat();
    assert!(decode(&aeabi_section(&oversized_string))
        .unwrap_err()
        .to_string()
        .contains("4096"));

    let mut too_many = Vec::new();
    let mut added = 0;
    for tag in 65..20_000 {
        if tag % 128 < 64 {
            continue;
        }
        if tag % 2 == 1 {
            too_many.extend(string_attribute(tag, b"x"));
        } else {
            too_many.extend(integer_attribute(tag, 1));
        }
        added += 1;
        if added == 257 {
            break;
        }
    }
    assert_eq!(added, 257);
    assert!(decode(&aeabi_section(&too_many))
        .unwrap_err()
        .to_string()
        .contains("too many"));
}
