//! Independent, bounded checks of ESP32 v1 images with an appended SHA-256.
//!
//! Chip/revision/capacity are caller-supplied source facts, not board defaults.
//! This verifies the image container, not application runtime or secure boot.
//! Signed images, RAM-only multi-header images and trailing data are refused.

use aros_common::{sha256_bytes, Sha256Digest};
use serde::Serialize;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Exact source-bound image requirements. No board identity is hard-coded.
#[derive(Debug, Clone, Copy)]
pub struct EspImagePolicy {
    pub chip_id: u16,
    pub revision_min: u16,
    pub revision_max: u16,
    pub flash_bytes: u64,
    pub maximum_image_bytes: u64,
}

/// One bounded load segment; zero-address records are image padding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspImageSegment {
    pub load_address: u32,
    pub file_offset: usize,
    pub size_bytes: usize,
}

/// Independently checked facts, suitable for an enclosing artifact receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspImageFacts {
    pub sha256: Sha256Digest,
    pub size_bytes: usize,
    pub chip_id: u16,
    pub revision_min: u16,
    pub revision_max: u16,
    pub flash_bytes: u64,
    pub entry_point: u32,
    pub segments: Vec<EspImageSegment>,
}

/// Verify the complete container before interpreting it as a flash input.
///
/// # Errors
/// Rejects malformed/truncated containers, altered checksums/digests, wrong
/// source geometry, address overflow/overlap, or an unmapped entry point.
pub fn verify_esp_image(bytes: &[u8], policy: EspImagePolicy) -> Result<EspImageFacts, String> {
    if policy.revision_min > policy.revision_max
        || policy.flash_bytes == 0
        || policy.maximum_image_bytes == 0
        || policy.maximum_image_bytes > policy.flash_bytes
    {
        return Err("invalid source image policy".into());
    }
    if bytes.len() < 24 || bytes.len() as u64 > policy.maximum_image_bytes {
        return Err("image is truncated or exceeds its source-owned range".into());
    }
    if bytes[0] != 0xe9 || !(1..=16).contains(&bytes[1]) || bytes[2] > 3 {
        return Err("unsupported ESP image header".into());
    }
    if bytes[23] != 1 || bytes[19..23] != [0; 4] {
        return Err("image requires an appended digest and supported reserved header".into());
    }
    let chip_id = u16::from_le_bytes([bytes[12], bytes[13]]);
    let revision_min = u16::from_le_bytes([bytes[15], bytes[16]]);
    let revision_max = u16::from_le_bytes([bytes[17], bytes[18]]);
    if chip_id != policy.chip_id {
        return Err("image chip differs from source contract".into());
    }
    if revision_min != policy.revision_min || revision_max != policy.revision_max {
        return Err("image silicon revision interval differs from source contract".into());
    }
    let size_code = bytes[3] >> 4;
    if size_code > 7 {
        return Err("unsupported image flash capacity".into());
    }
    let flash_bytes = (1_u64 << 20) << size_code;
    if flash_bytes != policy.flash_bytes {
        return Err("image flash capacity differs from source contract".into());
    }
    let entry_point = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let mut cursor = 24_usize;
    let mut checksum = 0xef_u8;
    let mut segments: Vec<EspImageSegment> = Vec::with_capacity(usize::from(bytes[1]));
    for _ in 0..bytes[1] {
        let header_end = cursor.checked_add(8).ok_or("segment header overflow")?;
        let header = bytes
            .get(cursor..header_end)
            .ok_or("truncated segment header")?;
        let address = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        // ESP-IDF's ROM/container loader consumes whole 32-bit words. A
        // correctly authenticated container is still invalid otherwise.
        if !length.is_multiple_of(4) {
            return Err("image segment length is not four-byte aligned".into());
        }
        let end = header_end
            .checked_add(length)
            .ok_or("segment payload overflow")?;
        let payload = bytes
            .get(header_end..end)
            .ok_or("truncated segment payload")?;
        let address_end = u64::from(address) + length as u64;
        if address_end > 1_u64 << 32 {
            return Err("segment exceeds the RV32 address space".into());
        }
        if address != 0 && length != 0 {
            for previous in &segments {
                let previous_start = u64::from(previous.load_address);
                let previous_end = previous_start + previous.size_bytes as u64;
                if previous_start != 0
                    && u64::from(address) < previous_end
                    && previous_start < address_end
                {
                    return Err("image load segments overlap".into());
                }
            }
        }
        checksum = payload.iter().fold(checksum, |value, byte| value ^ byte);
        segments.push(EspImageSegment {
            load_address: address,
            file_offset: header_end,
            size_bytes: length,
        });
        cursor = end;
    }
    // Espressif puts the checksum in the final byte of a 16-byte block.
    let checksum_offset = cursor | 15;
    let digest_offset = checksum_offset
        .checked_add(1)
        .ok_or("digest offset overflow")?;
    if bytes.get(checksum_offset) != Some(&checksum) {
        return Err("image XOR checksum differs".into());
    }
    if bytes
        .get(cursor..checksum_offset)
        .ok_or("truncated image padding")?
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err("nonzero image checksum padding".into());
    }
    if bytes.len()
        != digest_offset
            .checked_add(32)
            .ok_or("digest size overflow")?
    {
        return Err("missing digest or unsupported trailing image data".into());
    }
    let digest = sha256_bytes(&bytes[..digest_offset]);
    let matches = digest
        .as_str()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .zip(&bytes[digest_offset..])
        .all(|(hex, byte)| *hex == [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 15)]]);
    if !matches {
        return Err("image validation SHA-256 differs".into());
    }
    if !segments.iter().any(|segment| {
        segment.load_address != 0
            && u64::from(entry_point) >= u64::from(segment.load_address)
            && u64::from(entry_point) < u64::from(segment.load_address) + segment.size_bytes as u64
    }) {
        return Err("image entry point is outside its load segments".into());
    }
    Ok(EspImageFacts {
        sha256: sha256_bytes(bytes),
        size_bytes: bytes.len(),
        chip_id,
        revision_min,
        revision_max,
        flash_bytes,
        entry_point,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> EspImagePolicy {
        EspImagePolicy {
            chip_id: 18,
            revision_min: 100,
            revision_max: 199,
            flash_bytes: 32 << 20,
            maximum_image_bytes: 0x6000,
        }
    }

    fn fixture(length: usize) -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[0] = 0xe9;
        bytes[1] = 1;
        bytes[2] = 2;
        bytes[3] = 0x50;
        bytes[4..8].copy_from_slice(&0x4ff0_0000_u32.to_le_bytes());
        bytes[12..14].copy_from_slice(&18_u16.to_le_bytes());
        bytes[15..17].copy_from_slice(&100_u16.to_le_bytes());
        bytes[17..19].copy_from_slice(&199_u16.to_le_bytes());
        bytes[23] = 1;
        bytes.extend(0x4ff0_0000_u32.to_le_bytes());
        bytes.extend(u32::try_from(length).unwrap().to_le_bytes());
        bytes.extend(vec![0x5a; length]);
        let checksum = bytes[32..].iter().fold(0xef, |value, byte| value ^ byte);
        while bytes.len() % 16 != 15 {
            bytes.push(0);
        }
        bytes.push(checksum);
        let digest = sha256_bytes(&bytes);
        for offset in (0..64).step_by(2) {
            bytes.push(u8::from_str_radix(&digest.as_str()[offset..offset + 2], 16).unwrap());
        }
        bytes
    }

    #[test]
    fn container_and_all_valid_checksum_alignment_positions_pass() {
        for length in (4..=64).step_by(4) {
            let facts = verify_esp_image(&fixture(length), policy()).unwrap();
            assert_eq!(facts.segments[0].size_bytes, length);
        }
    }

    #[test]
    fn authenticated_but_unaligned_segment_lengths_fail() {
        for length in (1..=32).filter(|length| length % 4 != 0) {
            // fixture() calculates valid XOR and SHA-256 for this exact
            // container, so authentication cannot mask the format error.
            assert!(verify_esp_image(&fixture(length), policy())
                .unwrap_err()
                .contains("four-byte aligned"));
        }
    }

    #[test]
    fn altered_bytes_geometry_truncation_and_trailing_data_fail() {
        let bytes = fixture(16);
        for index in [0, 1, 2, 3, 4, 12, 15, 17, 19, 23, 28, 32, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[index] ^= 0x80;
            assert!(
                verify_esp_image(&bad, policy()).is_err(),
                "accepted altered byte {index}"
            );
        }
        for length in 0..bytes.len() {
            assert!(verify_esp_image(&bytes[..length], policy()).is_err());
        }
        let mut bad = bytes.clone();
        bad.push(0);
        assert!(verify_esp_image(&bad, policy()).is_err());
        let mut wrong = policy();
        wrong.maximum_image_bytes = 32;
        assert!(verify_esp_image(&bytes, wrong)
            .unwrap_err()
            .contains("range"));
        let mut wrong = policy();
        wrong.revision_max = 299;
        assert!(verify_esp_image(&bytes, wrong)
            .unwrap_err()
            .contains("revision"));
        let mut wrong = policy();
        wrong.chip_id = 19;
        assert!(verify_esp_image(&bytes, wrong)
            .unwrap_err()
            .contains("chip"));
    }

    fn refresh_digest(bytes: &mut Vec<u8>) {
        bytes.truncate(bytes.len() - 32);
        let digest = sha256_bytes(bytes);
        for offset in (0..64).step_by(2) {
            bytes.push(u8::from_str_radix(&digest.as_str()[offset..offset + 2], 16).unwrap());
        }
    }

    #[test]
    fn independently_authenticated_segment_headers_still_require_safe_ranges() {
        let mut bytes = fixture(16)[..48].to_vec();
        bytes[1] = 2;
        bytes.extend(0x4ff0_0100_u32.to_le_bytes());
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend([0x5a; 16]);
        while bytes.len() % 16 != 15 {
            bytes.push(0);
        }
        bytes.push(0xef); // two identical payloads cancel in the XOR checksum
        bytes.extend([0; 32]);
        refresh_digest(&mut bytes);
        assert_eq!(
            verify_esp_image(&bytes, policy()).unwrap().segments.len(),
            2
        );
        let mut overlap = bytes.clone();
        overlap[48..52].copy_from_slice(&0x4ff0_0000_u32.to_le_bytes());
        refresh_digest(&mut overlap);
        assert!(verify_esp_image(&overlap, policy())
            .unwrap_err()
            .contains("overlap"));
        let mut overflow = bytes.clone();
        overflow[48..52].copy_from_slice(&0xffff_fff8_u32.to_le_bytes());
        refresh_digest(&mut overflow);
        assert!(verify_esp_image(&overflow, policy())
            .unwrap_err()
            .contains("address space"));
        let mut unmapped = bytes;
        unmapped[4..8].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
        refresh_digest(&mut unmapped);
        assert!(verify_esp_image(&unmapped, policy())
            .unwrap_err()
            .contains("entry point"));
    }

    #[test]
    #[ignore = "requires the explicitly selected fresh external producer output"]
    fn independently_verify_fresh_vendor_bootloader() {
        let path = std::env::var_os("AROS_TEST_ESP_IMAGE").expect("set exact fresh output path");
        let bytes = std::fs::read(path).unwrap();
        let facts = verify_esp_image(&bytes, policy()).unwrap();
        assert_eq!(
            facts.sha256.to_string(),
            "06b2d6fbea7537fe974f7eebcc652dd5570886bcc7f87173abd44ae9c568dd7e"
        );
        assert_eq!(facts.size_bytes, 22544);
        assert_eq!(facts.segments.len(), 3);
        for (index, expected) in [(32, "checksum"), (bytes.len() - 1, "SHA-256")] {
            let mut bad = bytes.clone();
            bad[index] ^= 1;
            assert!(verify_esp_image(&bad, policy())
                .unwrap_err()
                .contains(expected));
        }
    }
}
