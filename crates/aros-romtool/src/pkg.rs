//! AROS kickstart package (`PKG`) container format.
//!
//! The format is consumed by the 32-bit bootstrap in
//! `arch/all-pc/bootstrap/bootstrap.c` (`AddModule`) and is specified in
//! `tools/package/FORMAT`. This implementation is byte-compatible with the
//! historic Python packer `tools/package/pkg`.
//!
//! ```text
//! package     = header, file*
//! header      = 'P', 'K', 'G', version:u8, packageSize:u32be
//! file        = pathLength:u32be, path, dataLength:u32be, data
//! ```
//!
//! `path` occupies `pathLength + 1` bytes and is NUL-padded to a multiple of
//! four, so that the following `dataLength` field starts on a 4-byte boundary.
//! `packageSize` is the total size of the container in bytes; the reference
//! packer seeks back and patches it once all members are written. The bootstrap
//! itself ignores the field and uses the Multiboot module end address, but we
//! emit the true value to stay bit-identical to `tools/package/pkg`.
//!
//! Absolute member paths are reduced to their basename, matching the reference
//! packer: an absolute path is a property of the build machine, not of the
//! package. Relative paths are recorded verbatim.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Magic bytes at the start of every package.
pub const MAGIC: [u8; 3] = *b"PKG";

/// The only container version the bootstrap understands.
pub const VERSION: u8 = 1;

/// Length of the container header in bytes.
pub const HEADER_LEN: usize = 8;

/// A single member of a package.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path as recorded in the container. The bootstrap strips everything up
    /// to the last separator, so only the basename is semantically relevant.
    pub path: String,
    /// Raw file contents.
    pub data: Vec<u8>,
}

impl Entry {
    /// Reports whether the payload starts with an ELF magic number.
    ///
    /// The bootstrap silently ignores non-ELF members, so this is the check
    /// that distinguishes a usable module from dead weight in the container.
    #[must_use]
    pub fn is_elf(&self) -> bool {
        self.data.starts_with(b"\x7fELF")
    }

    /// The basename the bootstrap will match against when overriding modules.
    #[must_use]
    pub fn module_name(&self) -> &str {
        self.path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(self.path.as_str())
    }
}

/// Number of bytes the path field occupies, including at least one NUL
/// terminator, rounded up to a multiple of four.
const fn padded_path_len(path: &str) -> usize {
    (path.len() + 4) & !3
}

/// Serialises `entries` into the `PKG` container format.
///
/// # Errors
///
/// Returns an error if a path is not representable in ISO-8859-1, which the
/// historic packer requires, or if a length exceeds `u32`.
pub fn serialize(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(
        HEADER_LEN
            + entries
                .iter()
                .map(|e| padded_path_len(&e.path) + 8 + e.data.len())
                .sum::<usize>(),
    );

    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    // Patched with the true container size once all members are appended.
    out.extend_from_slice(&0u32.to_be_bytes());

    for entry in entries {
        let padded = padded_path_len(&entry.path);
        let path_field =
            u32::try_from(padded - 1).with_context(|| format!("path too long: {}", entry.path))?;
        let data_len = u32::try_from(entry.data.len())
            .with_context(|| format!("member too large: {}", entry.path))?;

        // The historic packer encodes paths as ISO-8859-1 (latin-1).
        if !entry.path.chars().all(|c| (c as u32) < 0x100) {
            bail!("path '{}' is not representable in ISO-8859-1", entry.path);
        }
        let path_bytes: Vec<u8> = entry.path.chars().map(|c| c as u8).collect();

        out.extend_from_slice(&path_field.to_be_bytes());
        out.extend_from_slice(&path_bytes);
        out.resize(out.len() + (padded - path_bytes.len()), 0u8);
        out.extend_from_slice(&data_len.to_be_bytes());
        out.extend_from_slice(&entry.data);
    }

    // The reference packer seeks back to offset 4 and stores the total size.
    let total = u32::try_from(out.len()).context("package exceeds 4 GiB")?;
    out[4..HEADER_LEN].copy_from_slice(&total.to_be_bytes());

    Ok(out)
}

/// Parses a `PKG` container.
///
/// # Errors
///
/// Returns an error on a bad magic number, an unsupported version, or a
/// truncated container.
pub fn parse(bytes: &[u8]) -> Result<Vec<Entry>> {
    if bytes.len() < HEADER_LEN {
        bail!("package is shorter than its {HEADER_LEN}-byte header");
    }
    if bytes[0..3] != MAGIC {
        bail!("bad magic {:02x?}, expected {:02x?}", &bytes[0..3], &MAGIC);
    }
    if bytes[3] != VERSION {
        bail!(
            "unsupported package version {}, expected {VERSION}",
            bytes[3]
        );
    }

    let mut entries = Vec::new();
    let mut pos = HEADER_LEN;

    while pos < bytes.len() {
        let path_len = read_u32be(bytes, pos, "path length")? as usize;
        pos += 4;

        // The field counts the padded field width minus the final NUL.
        let field = path_len + 1;
        if pos + field > bytes.len() {
            bail!("truncated path field at offset {pos}");
        }
        let raw = &bytes[pos..pos + field];
        let name_end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        // Decode ISO-8859-1, mirroring the historic packer.
        let path: String = raw[..name_end].iter().map(|&b| b as char).collect();
        pos += field;

        let data_len = read_u32be(bytes, pos, "data length")? as usize;
        pos += 4;
        if pos + data_len > bytes.len() {
            bail!("truncated payload for '{path}' at offset {pos}");
        }
        let data = bytes[pos..pos + data_len].to_vec();
        pos += data_len;

        entries.push(Entry { path, data });
    }

    Ok(entries)
}

fn read_u32be(bytes: &[u8], pos: usize, what: &str) -> Result<u32> {
    let slice = bytes
        .get(pos..pos + 4)
        .with_context(|| format!("truncated {what} field at offset {pos}"))?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Builds a package on disk from `files`.
///
/// `path_mode` decides what is recorded in the container's path field.
///
/// # Errors
///
/// Returns an error if a member cannot be read or the output cannot be written.
pub fn create(output: &Path, files: &[PathBuf], path_mode: PathMode) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(files.len());

    for file in files {
        let data = fs::read(file)
            .with_context(|| format!("cannot read package member '{}'", file.display()))?;
        let basename = || {
            file.file_name().map_or_else(
                || file.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let path = match path_mode {
            // Mirror the reference packer: absolute paths collapse to their
            // basename, relative paths are recorded verbatim.
            PathMode::Reference => {
                if file.is_absolute() {
                    basename()
                } else {
                    file.to_string_lossy().into_owned()
                }
            }
            PathMode::Basename => basename(),
        };
        entries.push(Entry { path, data });
    }

    let bytes = serialize(&entries)?;

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("cannot create output directory '{}'", parent.display())
            })?;
        }
    }
    let mut out = fs::File::create(output)
        .with_context(|| format!("cannot create package '{}'", output.display()))?;
    out.write_all(&bytes)
        .with_context(|| format!("cannot write package '{}'", output.display()))?;

    Ok(entries)
}

/// How member paths are recorded in the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// Match `tools/package/pkg`: absolute paths are reduced to their
    /// basename, relative paths are recorded verbatim. Output is bit-identical
    /// to the reference packer.
    Reference,
    /// Always record only the basename. The bootstrap strips the directory part
    /// anyway, so this keeps packages reproducible regardless of whether the
    /// build system passes absolute or relative paths.
    Basename,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_doc_example_roundtrips() {
        // tools/package/FORMAT: "PKG", 1, 28L, 3L, "foo", 0, 8L, "barbarba"
        let entries = vec![Entry {
            path: "foo".to_owned(),
            data: b"barbarba".to_vec(),
        }];
        let bytes = serialize(&entries).unwrap();

        assert_eq!(&bytes[0..3], b"PKG");
        assert_eq!(bytes[3], 1);
        // packageSize is the total container size: 28, exactly as documented.
        assert_eq!(&bytes[4..8], &28u32.to_be_bytes());
        // pathLength = 3, path field = "foo\0"
        assert_eq!(&bytes[8..12], &3u32.to_be_bytes());
        assert_eq!(&bytes[12..16], b"foo\0");
        assert_eq!(&bytes[16..20], &8u32.to_be_bytes());
        assert_eq!(&bytes[20..28], b"barbarba");
        assert_eq!(bytes.len(), 28);

        let back = parse(&bytes).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].path, "foo");
        assert_eq!(back[0].data, b"barbarba");
    }

    #[test]
    fn path_field_pads_to_four_bytes() {
        // A 4-character name needs 4 NULs of padding to keep dataLength aligned.
        assert_eq!(padded_path_len("foo"), 4);
        assert_eq!(padded_path_len("abcd"), 8);
        assert_eq!(padded_path_len("abcde"), 8);
        assert_eq!(padded_path_len("abcdefg"), 8);
        assert_eq!(padded_path_len("abcdefgh"), 12);
    }

    #[test]
    fn data_length_field_stays_aligned() {
        let entries = vec![
            Entry {
                path: "abcd".to_owned(),
                data: b"x".to_vec(),
            },
            Entry {
                path: "exec.library".to_owned(),
                data: b"yy".to_vec(),
            },
        ];
        let bytes = serialize(&entries).unwrap();
        let back = parse(&bytes).unwrap();
        assert_eq!(back[0].path, "abcd");
        assert_eq!(back[0].data, b"x");
        assert_eq!(back[1].path, "exec.library");
        assert_eq!(back[1].data, b"yy");
    }

    #[test]
    fn module_name_strips_directories() {
        let e = Entry {
            path: "/boot/pc/Devs/ata.device".to_owned(),
            data: Vec::new(),
        };
        assert_eq!(e.module_name(), "ata.device");
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = serialize(&[]).unwrap();
        bytes[1] = b'X';
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_payload() {
        let entries = vec![Entry {
            path: "a".to_owned(),
            data: vec![1, 2, 3, 4],
        }];
        let mut bytes = serialize(&entries).unwrap();
        bytes.truncate(bytes.len() - 2);
        assert!(parse(&bytes).is_err());
    }
}
