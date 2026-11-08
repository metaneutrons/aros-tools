//! Closed reader for the two members of a start-artifact ZIP
//! (`compiler-sdk-start-artifact-v2.py`).
//!
//! A byte-transport reader only: it authenticates no signature and validates
//! no record. The archive must be digest- and size-pinned by the caller.

use std::io::{Cursor, Read};

use zip::{CompressionMethod, ZipArchive};

use super::claim::sha256_hex;
use super::json::is_hex;
use super::{fail, integrity, require, Result, MAX_BUNDLE, MAX_RECORD};

pub const RECORD_BASENAME: &str = "compiler-sdk-release-start-v2.json";
pub const BUNDLE_BASENAME: &str = "compiler-sdk-release-start-v2.sigstore.json";
pub const MAX_ARCHIVE: usize = MAX_RECORD + MAX_BUNDLE + 1024 * 1024;

const LOCAL_SIGNATURE: u32 = 0x0403_4B50;
const LOCAL_HEADER: usize = 30;

/// The two members, byte for byte.
#[derive(Debug, PartialEq, Eq)]
pub struct StartMembers {
    pub record: Vec<u8>,
    pub bundle: Vec<u8>,
}

const fn le16(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

const fn le32(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

/// Compare the local header with the central directory entry.
fn check_local_header(
    raw: &[u8],
    offset: u64,
    name: &str,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
) -> Result<()> {
    let header_end = usize::try_from(offset)
        .ok()
        .and_then(|start| start.checked_add(LOCAL_HEADER).map(|end| (start, end)));
    let Some((start, end)) = header_end.filter(|(_, end)| *end <= raw.len()) else {
        return fail("start ZIP local header is outside the captured archive");
    };
    require(
        le32(raw, start) == LOCAL_SIGNATURE,
        "start ZIP member does not point to a local file header",
    )?;
    let local_flags = le16(raw, start + 6);
    let local_method = le16(raw, start + 8);
    let local_crc = le32(raw, start + 14);
    let local_compressed = u64::from(le32(raw, start + 18));
    let local_size = u64::from(le32(raw, start + 22));
    let name_length = usize::from(le16(raw, start + 26));
    let extra_length = usize::from(le16(raw, start + 28));
    let name_end = end + name_length;
    let data_start = name_end + extra_length;
    require(
        name_length == name.len()
            && data_start <= raw.len()
            && &raw[end..name_end] == name.as_bytes(),
        "start ZIP local member name or span differs",
    )?;
    // Encryption, strong encryption and masked headers are not accepted; the
    // remaining bits are the data descriptor (0x0008) and the UTF-8 name flag
    // (0x0800), neither of which changes the pinned member bytes.
    require(
        local_flags & !0x0808 == 0 && local_method == method,
        "start ZIP local flags or compression method differ from the closed contract",
    )?;
    // The extra field must be a well-formed TLV sequence inside its span.
    let mut cursor = name_end;
    while cursor < data_start {
        require(
            data_start - cursor >= 4,
            "start ZIP local extra field is truncated",
        )?;
        let field_size = usize::from(le16(raw, cursor + 2));
        cursor += 4 + field_size;
        require(
            cursor <= data_start,
            "start ZIP local extra field exceeds its span",
        )?;
    }
    let data_end = data_start as u64 + compressed;
    require(
        data_end <= raw.len() as u64,
        "start ZIP compressed member span is outside the captured archive",
    )?;
    // Without a data descriptor the local numeric fields must agree. ZIP64
    // sentinel sizes are not accepted for these small, bounded members.
    if local_flags & 0x0008 == 0 {
        require(
            local_crc == crc && local_compressed == compressed && local_size == size,
            "start ZIP local CRC or sizes differ from central metadata",
        )?;
    }
    Ok(())
}

/// Read exactly the sealed record and bundle from a pinned ZIP capture.
///
/// # Errors
///
/// Fails closed for a digest or size that differs from the pin, any member
/// other than the two sealed files, encrypted, directory, oversized or
/// unsupported-compression members, local/central header disagreement, a CRC
/// or length mismatch, or trailing compressed data.
pub fn read_start_archive(
    raw: &[u8],
    expected_sha256: &str,
    expected_size: usize,
) -> Result<StartMembers> {
    require(
        is_hex(expected_sha256, 64),
        "expected archive digest must be lowercase SHA-256",
    )?;
    require(
        (1..=MAX_ARCHIVE).contains(&expected_size),
        "expected archive size must be a positive in-limit integer",
    )?;
    if raw.len() != expected_size || sha256_hex(raw) != expected_sha256 {
        return integrity(
            "start archive bytes differ from the independently pinned digest or size",
        );
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(raw)) else {
        return fail("start ZIP is malformed or member data failed integrity checks");
    };
    require(
        archive.len() == 2,
        "start ZIP must contain exactly the sealed record and bundle",
    )?;
    let limits = [(RECORD_BASENAME, MAX_RECORD), (BUNDLE_BASENAME, MAX_BUNDLE)];
    let mut contents: [Option<Vec<u8>>; 2] = [None, None];
    let mut declared_total = 0usize;
    for index in 0..archive.len() {
        let Ok(member) = archive.by_index_raw(index) else {
            return fail("start ZIP is malformed or member data failed integrity checks");
        };
        let Ok(name) = member.name().map(std::borrow::Cow::into_owned) else {
            return fail("start ZIP contains a malformed member name");
        };
        let Some(slot) = limits.iter().position(|(expected, _)| *expected == name) else {
            return fail("start ZIP contains a duplicate or unexpected member");
        };
        require(
            contents[slot].is_none(),
            "start ZIP contains a duplicate or unexpected member",
        )?;
        let mode = member.unix_mode().unwrap_or(0o100_644);
        require(
            mode & 0o170_000 == 0o100_000
                && !member.is_dir()
                && !member.encrypted()
                && !member.is_symlink(),
            "start ZIP member is not an ordinary unencrypted file",
        )?;
        let method = match member.compression() {
            CompressionMethod::Stored => 0,
            CompressionMethod::Deflated => 8,
            _ => return fail("start ZIP member uses an unsupported compression method"),
        };
        let limit = limits[slot].1;
        let (size, compressed) = (member.size(), member.compressed_size());
        require(
            size > 0 && size <= limit as u64,
            "start ZIP member has an empty or oversized declared length",
        )?;
        require(
            compressed > 0 && compressed <= MAX_ARCHIVE as u64,
            "start ZIP member has an invalid compressed length",
        )?;
        declared_total += size as usize;
        let offset = member.header_start();
        let crc = member.crc32();
        drop(member);
        check_local_header(raw, offset, &name, method, crc, compressed, size)?;
        contents[slot] = Some(Vec::new());
    }
    require(
        declared_total <= MAX_RECORD + MAX_BUNDLE,
        "start ZIP declared member lengths exceed their aggregate limit",
    )?;
    let mut out = [Vec::new(), Vec::new()];
    for (slot, (name, limit)) in limits.iter().enumerate() {
        let Ok(mut member) = archive.by_name(name) else {
            return fail("start ZIP must contain exactly the sealed record and bundle");
        };
        let declared = member.size();
        let mut bytes = Vec::new();
        // Bound before capture; one extra byte reveals an overlong stream.
        let read = member
            .by_ref()
            .take(*limit as u64 + 1)
            .read_to_end(&mut bytes);
        // The zip crate verifies the CRC when the stream ends cleanly.
        require(
            read.is_ok() && bytes.len() as u64 == declared && bytes.len() <= *limit,
            "start ZIP member full length or CRC differs from its metadata",
        )?;
        out[slot] = bytes;
    }
    let [record, bundle] = out;
    Ok(StartMembers { record, bundle })
}
