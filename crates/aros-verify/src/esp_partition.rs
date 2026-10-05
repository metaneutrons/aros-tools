//! Independent checks of unsigned ESP-IDF partition-table containers.
//!
//! Source-derived numeric entries and geometry are supplied by the caller.
//! This unsigned contract covers application/data/custom partitions after the
//! reserved table sector, not bootloader/table self-descriptions or secure boot.
//! MD5 is only the legacy format checksum, never authentication. The complete
//! artifact is independently measured with SHA-256.

use aros_common::{sha256_bytes, Sha256Digest};
use md5::{Digest, Md5};
use serde::Serialize;
use std::collections::HashSet;

pub use aros_common::esp_partition::{
    parse_esp_partition_source, EspPartition, EspPartitionSource,
};

const TABLE_BYTES: usize = 0xc00;
const SECTOR_BYTES: u64 = 0x1000;
const RECORD_BYTES: usize = 32;

/// Independently measured unsigned table and its complete validated entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspPartitionFacts {
    pub sha256: Sha256Digest,
    pub size_bytes: usize,
    pub partitions: Vec<EspPartition>,
}

/// Validate every record, mandatory MD5, trailing erased padding and exact
/// source entries. Signed sectors and alternate layouts are not accepted.
///
/// # Errors
/// Rejects malformed containers, unsafe or mismatched geometry, duplicate
/// labels, overlaps, unsupported flags and checksum mismatches.
pub fn verify_esp_partition_table(
    bytes: &[u8],
    table_offset: u32,
    flash_bytes: u64,
    expected: &[EspPartition],
) -> Result<EspPartitionFacts, String> {
    let table_start = u64::from(table_offset);
    let table_end = table_start + SECTOR_BYTES;
    if flash_bytes == 0
        || flash_bytes > 1_u64 << 32
        || table_start % SECTOR_BYTES != 0
        || table_end > flash_bytes
        || expected.is_empty()
    {
        return Err("invalid source partition geometry".into());
    }
    if bytes.len() != TABLE_BYTES {
        return Err("unsupported unsigned partition-table size".into());
    }
    let mut partitions = Vec::new();
    let mut checksum_seen = false;
    let mut end_seen = false;
    for (index, record) in bytes.as_chunks::<RECORD_BYTES>().0.iter().enumerate() {
        if *record == [0xff; RECORD_BYTES] {
            if !checksum_seen {
                return Err("partition table ends before its MD5 checksum".into());
            }
            if bytes[index * RECORD_BYTES..]
                .iter()
                .any(|byte| *byte != 0xff)
            {
                return Err("partition table contains non-erased trailing data".into());
            }
            end_seen = true;
            break;
        }
        if record[..2] == [0xeb, 0xeb] {
            if checksum_seen || partitions.is_empty() || record[2..16] != [0xff; 14] {
                return Err("invalid partition MD5 record".into());
            }
            let digest = Md5::digest(&bytes[..index * RECORD_BYTES]);
            if record[16..] != digest[..] {
                return Err("partition MD5 checksum differs".into());
            }
            checksum_seen = true;
            continue;
        }
        if checksum_seen || record[..2] != [0xaa, 0x50] {
            return Err("unsupported partition record or record after checksum".into());
        }
        let label = &record[12..28];
        let label_end = label.iter().position(|byte| *byte == 0).unwrap_or(16);
        if label_end == 0
            || label[..label_end]
                .iter()
                .any(|byte| !(0x21..=0x7e).contains(byte))
            || label[label_end..].iter().any(|byte| *byte != 0)
        {
            return Err("partition label is empty, nonportable or has invalid padding".into());
        }
        partitions.push(EspPartition {
            name: String::from_utf8(label[..label_end].to_vec())
                .map_err(|_| "partition label is not UTF-8")?,
            kind: record[2],
            subtype: record[3],
            offset: word(record, 4),
            size: word(record, 8),
            flags: word(record, 28),
        });
    }
    if !end_seen {
        return Err("partition table has no erased end marker".into());
    }
    validate_ranges(&partitions, table_start, table_end, flash_bytes)?;
    if partitions != expected {
        return Err("partition table differs from resolved source entries".into());
    }
    Ok(EspPartitionFacts {
        sha256: sha256_bytes(bytes),
        size_bytes: bytes.len(),
        partitions,
    })
}

fn word(record: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        record[offset..offset + 4]
            .try_into()
            .expect("bounded record"),
    )
}

fn validate_ranges(
    partitions: &[EspPartition],
    table_start: u64,
    table_end: u64,
    flash_bytes: u64,
) -> Result<(), String> {
    let mut names = HashSet::new();
    for (index, partition) in partitions.iter().enumerate() {
        let start = u64::from(partition.offset);
        let end = start + u64::from(partition.size);
        let alignment = if partition.kind == 0 {
            0x10000
        } else {
            SECTOR_BYTES
        };
        if !names.insert(&partition.name) {
            return Err("duplicate partition label".into());
        }
        if partition.flags & !3 != 0 {
            return Err("unsupported partition flags".into());
        }
        if partition.size == 0 || start % alignment != 0 || end > flash_bytes {
            return Err("partition range is empty, unaligned or exceeds flash capacity".into());
        }
        if partition.kind == 0 && !partition.size.is_multiple_of(SECTOR_BYTES as u32) {
            return Err("unsigned app partition size is not sector aligned".into());
        }
        if start < table_end && table_start < end {
            return Err("partition overlaps the reserved table sector".into());
        }
        if start < table_end {
            return Err("partition lies before the reserved table sector ends".into());
        }
        for previous in &partitions[..index] {
            let previous_start = u64::from(previous.offset);
            let previous_end = previous_start + u64::from(previous.size);
            if start < previous_end && previous_start < end {
                return Err("partition ranges overlap".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_csv_encoding_passes_independent_binary_decoder_and_mutations_fail() {
        let csv =
            b"nvs,data,nvs,0x9000,12K,\nabcdefghijklmnop,app,ota_0,0x10000,1M,encrypted:readonly\n";
        let source = parse_esp_partition_source(csv, 0x8000, 2 << 20).unwrap();
        let artifact = aros_common::esp_partition::encode_esp_partition_source(
            csv,
            &source.sha256,
            0x8000,
            2 << 20,
        )
        .unwrap();
        let facts =
            verify_esp_partition_table(&artifact.bytes, 0x8000, 2 << 20, &source.partitions)
                .unwrap();
        assert_eq!(facts.partitions, source.partitions);
        assert_eq!(facts.sha256, artifact.sha256);
        for offset in [0, 2, 4, 8, 12, 28, 32, 60, 64, 80, 96, 3071] {
            let mut mutated = artifact.bytes.clone();
            mutated[offset] ^= 1;
            assert!(
                verify_esp_partition_table(&mutated, 0x8000, 2 << 20, &source.partitions).is_err(),
                "accepted changed byte at {offset}"
            );
        }
        assert!(
            verify_esp_partition_table(&artifact.bytes, 0x8000, 1 << 20, &source.partitions)
                .is_err()
        );
        let mut changed = source.partitions;
        changed[1].flags = 0;
        assert!(verify_esp_partition_table(&artifact.bytes, 0x8000, 2 << 20, &changed).is_err());
    }

    fn entry(name: &str, offset: u32, size: u32) -> EspPartition {
        EspPartition {
            name: name.into(),
            kind: 1,
            subtype: 2,
            offset,
            size,
            flags: 0,
        }
    }

    fn fixture(entries: &[EspPartition]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in entries {
            let mut record = [0; 32];
            record[..2].copy_from_slice(&[0xaa, 0x50]);
            record[2] = entry.kind;
            record[3] = entry.subtype;
            record[4..8].copy_from_slice(&entry.offset.to_le_bytes());
            record[8..12].copy_from_slice(&entry.size.to_le_bytes());
            record[12..12 + entry.name.len()].copy_from_slice(entry.name.as_bytes());
            record[28..32].copy_from_slice(&entry.flags.to_le_bytes());
            bytes.extend(record);
        }
        let checksum = Md5::digest(&bytes);
        bytes.extend([0xeb, 0xeb]);
        bytes.extend([0xff; 14]);
        bytes.extend_from_slice(&checksum);
        bytes.resize(TABLE_BYTES, 0xff);
        bytes
    }

    #[test]
    fn complete_table_requires_checksum_padding_and_exact_source_identity() {
        let entries = [
            entry("first", 0x9000, 0x1000),
            entry("second", 0xa000, 0x2000),
        ];
        let bytes = fixture(&entries);
        assert_eq!(
            verify_esp_partition_table(&bytes, 0x8000, 0x0010_0000, &entries)
                .unwrap()
                .partitions,
            entries
        );
        for index in [0, 12, 64, 66, 80, 100, TABLE_BYTES - 1] {
            let mut bad = bytes.clone();
            bad[index] ^= 1;
            assert!(verify_esp_partition_table(&bad, 0x8000, 0x0010_0000, &entries).is_err());
        }
        assert!(verify_esp_partition_table(
            &bytes[..bytes.len() - 1],
            0x8000,
            0x0010_0000,
            &entries
        )
        .is_err());
        let mut wrong = entries;
        wrong[1].subtype = 3;
        assert!(
            verify_esp_partition_table(&bytes, 0x8000, 0x0010_0000, &wrong)
                .unwrap_err()
                .contains("source entries")
        );
    }

    #[test]
    fn authenticated_geometry_errors_are_not_hidden_by_valid_md5() {
        let cases = [
            (
                vec![
                    entry("first", 0x9000, 0x2000),
                    entry("second", 0xa000, 0x1000),
                ],
                "overlap",
            ),
            (vec![entry("first", 0x8000, 0x1000)], "table sector"),
            (vec![entry("first", 0x1000, 0x1000)], "before"),
            (vec![entry("first", 0x9000, 0x0010_0000)], "capacity"),
            (vec![entry("first", 0x9001, 0x1000)], "unaligned"),
            (vec![entry("first", 0x9000, 0)], "empty"),
            (
                vec![
                    entry("first", 0x9000, 0x1000),
                    entry("first", 0xa000, 0x1000),
                ],
                "duplicate",
            ),
        ];
        for (entries, diagnostic) in cases {
            assert!(
                verify_esp_partition_table(&fixture(&entries), 0x8000, 0x0010_0000, &entries)
                    .unwrap_err()
                    .contains(diagnostic)
            );
        }
        let mut app = entry("app", 0x10000, 0x1001);
        app.kind = 0;
        assert!(
            verify_esp_partition_table(&fixture(&[app.clone()]), 0x8000, 0x0010_0000, &[app])
                .unwrap_err()
                .contains("sector aligned")
        );
        let mut entry = entry("first", 0x9000, 0x1000);
        entry.flags = 4;
        assert!(verify_esp_partition_table(
            &fixture(&[entry.clone()]),
            0x8000,
            0x0010_0000,
            &[entry]
        )
        .unwrap_err()
        .contains("flags"));
    }

    #[test]
    fn checksum_record_is_required_unique_final_and_canonical() {
        let entries = [entry("first", 0x9000, 0x1000)];
        let original = fixture(&entries);
        let mut missing = original.clone();
        missing[32..64].fill(0xff);
        assert!(
            verify_esp_partition_table(&missing, 0x8000, 0x0010_0000, &entries)
                .unwrap_err()
                .contains("before its MD5")
        );
        let mut duplicate = original.clone();
        duplicate[64..96].copy_from_slice(&original[32..64]);
        assert!(
            verify_esp_partition_table(&duplicate, 0x8000, 0x0010_0000, &entries)
                .unwrap_err()
                .contains("MD5 record")
        );
        let mut after = original.clone();
        after[64..96].copy_from_slice(&original[..32]);
        assert!(
            verify_esp_partition_table(&after, 0x8000, 0x0010_0000, &entries)
                .unwrap_err()
                .contains("after checksum")
        );
        let mut label = original.clone();
        label[18] = b'x'; // data beyond the label's first NUL
        let digest = Md5::digest(&label[..32]);
        label[48..64].copy_from_slice(&digest);
        assert!(
            verify_esp_partition_table(&label, 0x8000, 0x0010_0000, &entries)
                .unwrap_err()
                .contains("label")
        );
        assert!(verify_esp_partition_table(&original, 0x8001, 0x0010_0000, &entries).is_err());
        assert!(verify_esp_partition_table(&original, 0x8000, 0x8fff, &entries).is_err());
    }

    #[test]
    #[ignore = "requires the explicitly selected fresh external producer output"]
    fn independently_verify_fresh_vendor_partition_table() {
        let path =
            std::env::var_os("AROS_TEST_ESP_PARTITION").expect("set exact fresh output path");
        let bytes = std::fs::read(path).unwrap();
        let source_path = std::env::var_os("AROS_TEST_ESP_PARTITION_SOURCE")
            .expect("set the exact bound source CSV path");
        let source_bytes = std::fs::read(source_path).unwrap();
        let resolved = parse_esp_partition_source(&source_bytes, 0x8000, 32 << 20).unwrap();
        assert_eq!(
            resolved.sha256.to_string(),
            "7b9493ffcaf0a9c3bce3a76725889ccc214a6ca45cd0f19d9823344c4856b953"
        );
        let facts =
            verify_esp_partition_table(&bytes, 0x8000, 32 << 20, &resolved.partitions).unwrap();
        assert_eq!(
            facts.sha256.to_string(),
            "1011273e9c756cbbee22c1ba09794acb3bdc04f088d3a3154e61df0330b91b96"
        );
        assert_eq!(facts.size_bytes, 3072);
        let generated = aros_common::esp_partition::encode_esp_partition_source(
            &source_bytes,
            &resolved.sha256,
            0x8000,
            32 << 20,
        )
        .unwrap();
        assert_eq!(
            generated.bytes, bytes,
            "native encoding differs from the fresh vendor artifact"
        );
        assert_eq!(generated.sha256, facts.sha256);
        let independently_decoded = verify_esp_partition_table(
            &generated.bytes,
            generated.table_offset,
            generated.flash_bytes,
            &resolved.partitions,
        )
        .unwrap();
        assert_eq!(independently_decoded, facts);
        if let Some(parent) = std::env::var_os("AROS_TEST_ESP_PARTITION_EVIDENCE_PARENT") {
            let root = tempfile::Builder::new()
                .prefix("rv3-native-partition.")
                .tempdir_in(parent)
                .unwrap()
                .keep();
            std::fs::write(root.join("partition-table.bin"), &generated.bytes).unwrap();
            let receipt = serde_json::json!({
                "schema": "aros-esp-partition-encoder-probe-v1",
                "source_sha256": resolved.sha256,
                "table_offset": generated.table_offset,
                "flash_bytes": generated.flash_bytes,
                "artifact_sha256": generated.sha256,
                "artifact_size": generated.bytes.len(),
                "vendor_byte_equal": true,
                "independently_decoded": independently_decoded,
                "scope": "host-only partition format proof; no native kernel/BSP/Developer, CLI integration, provenance or device permission",
            });
            std::fs::write(
                root.join("probe-receipt.json"),
                serde_json::to_vec_pretty(&receipt).unwrap(),
            )
            .unwrap();
            eprintln!("Native partition proof retained at {}", root.display());
        }
    }
}
