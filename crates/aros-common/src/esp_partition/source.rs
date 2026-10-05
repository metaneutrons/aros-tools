//! Bounded resolution of explicitly positioned source CSV partitions.
//! Format keywords are ESP-IDF vocabulary, not a board registry. Offsets,
//! sizes, names, type assignments and flags remain source-owned.

use super::{validate_ranges, EspPartition, SECTOR_BYTES};
use crate::{sha256_bytes, Sha256Digest};

/// Parsed source bytes and their exact resolved partition declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EspPartitionSource {
    pub sha256: Sha256Digest,
    pub partitions: Vec<EspPartition>,
}

/// Resolve a source table without environment expansion or auto geometry.
///
/// Supports app/data/custom numeric types, IDF subtype names, explicit unsigned
/// byte offsets/sizes and K/M units. Subtypes must be explicit even for data
/// entries. UTF-8 without BOM and portable ASCII labels are required.
/// Implicit offsets, negative sizes,
/// bootloader/table self-descriptions and custom subtype extensions fail closed.
///
/// # Errors
/// Rejects unsupported syntax, ambiguous/defaulted geometry, oversized inputs,
/// unsafe labels, malformed flags, overlaps and out-of-flash declarations.
pub fn parse_esp_partition_source(
    bytes: &[u8],
    table_offset: u32,
    flash_bytes: u64,
) -> Result<EspPartitionSource, String> {
    if bytes.len() > 64 * 1024 {
        return Err("source partition CSV exceeds its byte budget".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "source partition CSV is not UTF-8")?;
    let table_start = u64::from(table_offset);
    let table_end = table_start + SECTOR_BYTES;
    if flash_bytes == 0
        || flash_bytes > 1_u64 << 32
        || !table_start.is_multiple_of(SECTOR_BYTES)
        || table_end > flash_bytes
    {
        return Err("invalid source partition geometry".into());
    }
    let mut partitions = Vec::new();
    let mut previous_end = table_end;
    for (index, line) in text.lines().enumerate() {
        if line.len() > 1024 || index >= 1024 {
            return Err("source partition CSV exceeds its line budget".into());
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if partitions.len() >= 94 {
            return Err("source partition table exceeds the unsigned record budget".into());
        }
        let fail = |reason: &str| format!("source partition CSV line {}: {reason}", index + 1);
        if line.contains(['$', '\0', '\\', '"', '\'']) {
            return Err(fail(
                "environment expansion, escaping and quoted fields are unsupported",
            ));
        }
        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        if !(5..=6).contains(&fields.len()) {
            return Err(fail("expected five fields plus optional flags"));
        }
        let name = fields[0];
        if name.is_empty()
            || name.len() > 16
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            || matches!(name, "." | "..")
        {
            return Err(fail("label must be a portable token of at most 16 bytes"));
        }
        let kind = match fields[1].to_ascii_lowercase().as_str() {
            "app" => 0,
            "data" => 1,
            number => u8::try_from(parse_number(number).map_err(|reason| fail(&reason))?)
                .map_err(|_| fail("partition type is outside the byte range"))?,
        };
        if matches!(kind, 2 | 3) {
            return Err(fail(
                "bootloader/table self-descriptions require a different contract",
            ));
        }
        let subtype = parse_subtype(kind, fields[2]).map_err(|reason| fail(&reason))?;
        let offset = parse_number(fields[3]).map_err(|reason| fail(&reason))?;
        let size = parse_number(fields[4]).map_err(|reason| fail(&reason))?;
        let mut flags = 0;
        if let Some(value) = fields.get(5).filter(|value| !value.is_empty()) {
            for flag in value.split(':') {
                let bit = match flag {
                    "encrypted" => 1,
                    "readonly" => 2,
                    _ => return Err(fail("unknown partition flag")),
                };
                if flags & bit != 0 {
                    return Err(fail("duplicate partition flag"));
                }
                flags |= bit;
            }
        }
        if u64::from(offset) < previous_end {
            return Err(fail(
                "partitions must be explicitly positioned in nonoverlapping source order",
            ));
        }
        previous_end = u64::from(offset) + u64::from(size);
        partitions.push(EspPartition {
            name: name.into(),
            kind,
            subtype,
            offset,
            size,
            flags,
        });
    }
    if partitions.is_empty() {
        return Err("source partition CSV has no entries".into());
    }
    validate_ranges(&partitions, table_start, table_end, flash_bytes)?;
    validate_data_subtypes(&partitions)?;
    Ok(EspPartitionSource {
        sha256: sha256_bytes(bytes),
        partitions,
    })
}

fn validate_data_subtypes(partitions: &[EspPartition]) -> Result<(), String> {
    let mut ota_seen = false;
    let mut tee_ota_seen = false;
    for partition in partitions.iter().filter(|partition| partition.kind == 1) {
        let readonly = partition.flags & 2 != 0;
        if matches!(partition.subtype, 0 | 3) && readonly {
            return Err("OTA/coredump source partitions must remain read-write".into());
        }
        if partition.subtype == 2 && partition.size < 0x3000 && !readonly {
            return Err("small NVS source partitions require readonly".into());
        }
        let seen = match partition.subtype {
            0 => &mut ota_seen,
            0x90 => &mut tee_ota_seen,
            _ => continue,
        };
        if *seen || partition.size != 0x2000 {
            return Err(
                "OTA metadata source partition must be unique and exactly two sectors".into(),
            );
        }
        *seen = true;
    }
    Ok(())
}

fn parse_number(value: &str) -> Result<u32, String> {
    let (value, multiplier) = match value.as_bytes().last().map(u8::to_ascii_lowercase) {
        Some(b'k') => (&value[..value.len() - 1], 1024),
        Some(b'm') => (&value[..value.len() - 1], 1024 * 1024),
        _ => (value, 1),
    };
    let (digits, radix) = if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (digits, 16)
    } else if let Some(digits) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        (digits, 2)
    } else if let Some(digits) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        (digits, 8)
    } else {
        if value.len() > 1 && value.starts_with('0') {
            return Err("ambiguous leading-zero number".into());
        }
        (value, 10)
    };
    if digits.is_empty() || digits.starts_with(['+', '-']) {
        return Err("geometry must use explicit unsigned numbers".into());
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| "invalid or overflowing unsigned number".into())
}

fn parse_subtype(kind: u8, value: &str) -> Result<u8, String> {
    let value = value.to_ascii_lowercase();
    let named = match (kind, value.as_str()) {
        (0, "factory") | (1, "ota") => Some(0),
        (0, "test") => Some(0x20),
        (1, "phy") => Some(1),
        (1, "nvs") => Some(2),
        (1, "coredump") => Some(3),
        (1, "nvs_keys") => Some(4),
        (1, "efuse") => Some(5),
        (1, "undefined") => Some(6),
        (1, "esphttpd") => Some(0x80),
        (1, "fat") => Some(0x81),
        (1, "spiffs") => Some(0x82),
        (1, "littlefs") => Some(0x83),
        (1, "tee_ota") => Some(0x90),
        _ => None,
    };
    if let Some(named) = named {
        return Ok(named);
    }
    if kind == 0 {
        for (prefix, limit, base) in [("ota_", 16, 0x10), ("tee_", 2, 0x30)] {
            if let Some(index) = value.strip_prefix(prefix) {
                let index: u8 = index.parse().map_err(|_| "invalid app subtype slot")?;
                if index >= limit || format!("{prefix}{index}") != value {
                    return Err("unsupported app subtype slot".into());
                }
                return Ok(base + index);
            }
        }
    }
    u8::try_from(parse_number(&value)?)
        .map_err(|_| "partition subtype is outside the byte range".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_source_units_names_and_numeric_custom_types_resolve() {
        let source = b"# exact source geometry\r\nnvs,data,nvs,0x9000,24K,\r\nota_0,app,ota_0,0x20000,8M,\r\nbsp,0x40,0,0x820000,0x7e0000,readonly\r\n";
        let resolved = parse_esp_partition_source(source, 0x8000, 32 << 20).unwrap();
        assert_eq!(resolved.sha256, sha256_bytes(source));
        assert_eq!(resolved.partitions[1].kind, 0);
        assert_eq!(resolved.partitions[1].subtype, 0x10);
        assert_eq!(resolved.partitions[1].size, 8 << 20);
        assert_eq!(resolved.partitions[2].kind, 0x40);
        assert_eq!(resolved.partitions[2].flags, 2);
    }

    #[test]
    fn source_defaults_expansion_ambiguity_and_unsafe_geometry_are_refused() {
        for row in [
            "part,data,nvs,,0x3000,",
            "part,data,nvs,0x9000,-0x10000,",
            "part,data,nvs,$OFFSET,0x3000,",
            "part,data,nvs,0x9000,0x3000,unknown",
            "part,data,nvs,0x9000,0x3000,readonly:readonly",
            "part,data,nvs,0x9000,0x3000,,extra",
            "../part,data,nvs,0x9000,0x3000,",
            "part,data,nvs,0x8000,0x3000,",
            "part,data,nvs,0x9000,0xffffffff,",
            "part,0x100,nvs,0x9000,0x3000,",
            "part,bootloader,primary,0x1000,0x3000,",
            "part,data,,0x9000,0x3000,",
            "part,app,ota_16,0x10000,0x3000,",
            "part,data,nvs,09000,0x3000,",
            "part,data,nvs,0x9000,0x3000,\npart,data,nvs,0xc000,0x3000,",
            "part,data,nvs,0xc000,0x3000,\nsecond,data,nvs,0x9000,0x3000,",
            "part,data,nvs,0x9000,0x3000,\nsecond,data,nvs,0xa000,0x3000,",
        ] {
            assert!(
                parse_esp_partition_source(row.as_bytes(), 0x8000, 32 << 20).is_err(),
                "accepted {row}"
            );
        }
        for number in [
            "",
            "+1",
            "-1",
            "1_000",
            "010",
            "0x",
            "4294967296",
            "0xffffffffK",
        ] {
            assert!(parse_number(number).is_err(), "accepted {number}");
        }
    }

    #[test]
    fn source_data_subtype_constraints_match_the_vendor_generator() {
        for source in [
            "nvs,data,nvs,0x9000,0x1000,",
            "ota,data,ota,0x9000,0x1000,",
            "ota,data,ota,0x9000,0x2000,readonly",
            "dump,data,coredump,0x9000,0x2000,readonly",
            "ota,data,ota,0x9000,0x2000,\nother,data,ota,0xb000,0x2000,",
            "tee,data,tee_ota,0x9000,0x2000,\nother,data,tee_ota,0xb000,0x2000,",
        ] {
            assert!(
                parse_esp_partition_source(source.as_bytes(), 0x8000, 32 << 20).is_err(),
                "accepted {source}"
            );
        }
        for source in [
            "nvs,data,nvs,0x9000,0x1000,readonly",
            "ota,data,ota,0x9000,0x2000,",
            "tee,data,tee_ota,0x9000,0x2000,",
        ] {
            assert!(
                parse_esp_partition_source(source.as_bytes(), 0x8000, 32 << 20).is_ok(),
                "refused {source}"
            );
        }
    }
}
