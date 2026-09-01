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
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{
    measure_regular_file, publish_atomic_file, sha256_bytes, AtomicFilePolicy,
    PublicationFailureClass, PublicationReceipt, Sha256Digest,
};

/// Existing-output policy for package creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatePolicy {
    /// Safe default: never replace an existing path.
    NoClobber,
    /// Replace only a regular file whose measured SHA-256 matches this value.
    ReplaceIfSha256(Sha256Digest),
}

/// Magic bytes at the start of every package.
pub const MAGIC: [u8; 3] = *b"PKG";

/// The only container version the bootstrap understands.
pub const VERSION: u8 = 1;

/// Length of the container header in bytes.
pub const HEADER_LEN: usize = 8;

/// Stable failure class for package creation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFailureKind {
    /// A source member could not be read.
    Input,
    /// The requested package or its staged bytes failed validation.
    Validation,
    /// Staging or atomic destination publication failed.
    Publication,
    /// Publication failed and at least one rollback/recovery action failed.
    RollbackIncomplete,
}

/// A package-creation failure with its stable phase and most relevant path.
#[derive(Debug)]
pub struct CreateFailure {
    kind: CreateFailureKind,
    path: PathBuf,
    publication_class: Option<PublicationFailureClass>,
    source: anyhow::Error,
}

impl CreateFailure {
    fn new(kind: CreateFailureKind, path: impl Into<PathBuf>, source: anyhow::Error) -> Self {
        Self {
            kind,
            path: path.into(),
            publication_class: None,
            source,
        }
    }

    fn publication(
        kind: CreateFailureKind,
        path: impl Into<PathBuf>,
        class: PublicationFailureClass,
        source: anyhow::Error,
    ) -> Self {
        Self {
            kind,
            path: path.into(),
            publication_class: Some(class),
            source,
        }
    }

    /// Stable phase used by the command's diagnostic boundary.
    #[must_use]
    pub const fn kind(&self) -> CreateFailureKind {
        self.kind
    }

    /// Path most directly associated with the failure.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Typed remediation class from the shared publication primitive.
    #[must_use]
    pub const fn publication_class(&self) -> Option<PublicationFailureClass> {
        self.publication_class
    }

    /// Consume the classified wrapper and return its full causal error.
    #[must_use]
    pub fn into_source(self) -> anyhow::Error {
        self.source
    }
}

impl fmt::Display for CreateFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatter.alternate() {
            write!(formatter, "{:#}", self.source)
        } else {
            self.source.fmt(formatter)
        }
    }
}

impl StdError for CreateFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A single member of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path as recorded in the container. The bootstrap strips everything up
    /// to the last separator, so only the basename is semantically relevant.
    pub path: String,
    /// Raw file contents.
    pub data: Vec<u8>,
}

/// Successfully created package and its durable-publication receipt.
#[derive(Debug)]
pub struct CreateOutcome {
    /// Entries encoded into the published package.
    pub entries: Vec<Entry>,
    /// Recovery completed while acquiring the publication namespace.
    pub publication: PublicationReceipt,
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
        bail!("bad magic {:02x?}, expected {:02x?}", &bytes[0..3], MAGIC);
    }
    if bytes[3] != VERSION {
        bail!(
            "unsupported package version {}, expected {VERSION}",
            bytes[3]
        );
    }
    let declared_size = read_u32be(bytes, 4, "package size")? as usize;
    if declared_size != bytes.len() {
        bail!(
            "package size field declares {declared_size} bytes, actual container has {}",
            bytes.len()
        );
    }

    let mut entries = Vec::new();
    let mut pos = HEADER_LEN;

    while pos < bytes.len() {
        let path_len = read_u32be(bytes, pos, "path length")? as usize;
        pos = pos
            .checked_add(4)
            .context("path length offset exceeds addressable memory")?;

        // The field counts the padded field width minus the final NUL.
        let field = path_len
            .checked_add(1)
            .context("path field length exceeds addressable memory")?;
        let path_end = pos
            .checked_add(field)
            .context("path field offset exceeds addressable memory")?;
        let raw = bytes
            .get(pos..path_end)
            .with_context(|| format!("truncated path field at offset {pos}"))?;
        let name_end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        // Decode ISO-8859-1, mirroring the historic packer.
        let path: String = raw[..name_end].iter().map(|&b| b as char).collect();
        pos = path_end;

        let data_len = read_u32be(bytes, pos, "data length")? as usize;
        pos = pos
            .checked_add(4)
            .context("data length offset exceeds addressable memory")?;
        let data_end = pos
            .checked_add(data_len)
            .context("payload offset exceeds addressable memory")?;
        let data = bytes
            .get(pos..data_end)
            .with_context(|| format!("truncated payload for '{path}' at offset {pos}"))?
            .to_vec();
        pos = data_end;

        entries.push(Entry { path, data });
    }

    Ok(entries)
}

fn read_u32be(bytes: &[u8], pos: usize, what: &str) -> Result<u32> {
    let end = pos
        .checked_add(4)
        .with_context(|| format!("{what} field offset exceeds addressable memory"))?;
    let slice = bytes
        .get(pos..end)
        .with_context(|| format!("truncated {what} field at offset {pos}"))?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Builds a package on disk from `files`.
///
/// `path_mode` decides what is recorded in the container's path field.
///
/// # Errors
///
/// Returns an error if a member cannot be read or validated, or if the staged
/// package cannot be verified and atomically published. The destination is not
/// touched until every input and the complete staged package have passed
/// validation.
pub fn create(
    output: &Path,
    files: &[PathBuf],
    path_mode: PathMode,
    allow_non_elf: bool,
    policy: &CreatePolicy,
) -> std::result::Result<CreateOutcome, CreateFailure> {
    let mut entries = Vec::with_capacity(files.len());

    for file in files {
        let data = fs::read(file)
            .with_context(|| format!("cannot read package member '{}'", file.display()))
            .map_err(|error| CreateFailure::new(CreateFailureKind::Input, file, error))?;
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

    let non_elf: Vec<(usize, &str)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.is_elf())
        .map(|(index, entry)| (index, entry.module_name()))
        .collect();
    if !allow_non_elf && !non_elf.is_empty() {
        return Err(CreateFailure::new(
            CreateFailureKind::Validation,
            &files[non_elf[0].0],
            anyhow::anyhow!(
                "these members are not ELF objects and would be ignored by the bootstrap: {}\n\
                 pass --allow-non-elf to package them anyway",
                non_elf
                    .iter()
                    .map(|(_, name)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let bytes = serialize(&entries)
        .map_err(|error| CreateFailure::new(CreateFailureKind::Validation, output, error))?;

    let reparsed = parse(&bytes)
        .with_context(|| {
            format!(
                "staged package verification failed for '{}'",
                output.display()
            )
        })
        .map_err(|error| CreateFailure::new(CreateFailureKind::Validation, output, error))?;
    if reparsed != entries {
        return Err(CreateFailure::new(
            CreateFailureKind::Validation,
            output,
            anyhow::anyhow!(
                "staged package verification failed for '{}': parsed entries differ from inputs",
                output.display()
            ),
        ));
    }

    let atomic_policy = match policy {
        CreatePolicy::NoClobber => AtomicFilePolicy::NoClobber,
        CreatePolicy::ReplaceIfSha256(expected) => {
            let measured = measure_regular_file(output)
                .with_context(|| {
                    format!(
                        "cannot measure existing package for compare-and-swap '{}'",
                        output.display()
                    )
                })
                .map_err(|error| {
                    CreateFailure::new(CreateFailureKind::Publication, output, error)
                })?;
            let Some((identity, current)) = measured else {
                return Err(CreateFailure::publication(
                    CreateFailureKind::Publication,
                    output,
                    PublicationFailureClass::Conflict,
                    anyhow::anyhow!(
                        "compare-and-swap replacement requires an existing regular file '{}'; omit --replace-if-sha256 for a new output",
                        output.display()
                    ),
                ));
            };
            let actual = sha256_bytes(&current);
            if &actual != expected {
                return Err(CreateFailure::publication(
                    CreateFailureKind::Publication,
                    output,
                    PublicationFailureClass::Conflict,
                    anyhow::anyhow!(
                        "compare-and-swap digest mismatch for '{}': expected {}, found {}",
                        output.display(),
                        expected,
                        actual
                    ),
                ));
            }
            AtomicFilePolicy::ReplaceIf {
                identity,
                sha256: actual,
            }
        }
    };
    let publication = publish_atomic_file(output, &bytes, atomic_policy).map_err(|error| {
        let class = aros_common::publication_failure_class(&error);
        let kind = if aros_common::is_rollback_incomplete(&error) {
            CreateFailureKind::RollbackIncomplete
        } else {
            CreateFailureKind::Publication
        };
        CreateFailure::publication(
            kind,
            output,
            class,
            anyhow::Error::new(error).context(format!(
                "cannot durably publish package '{}'",
                output.display()
            )),
        )
    })?;

    Ok(CreateOutcome {
        entries,
        publication,
    })
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

    #[test]
    fn rejects_mismatched_package_size() {
        let mut bytes = serialize(&[]).unwrap();
        bytes[4..8].copy_from_slice(&9u32.to_be_bytes());
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn invalid_member_leaves_existing_destination_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let member = directory.path().join("broken.bin");
        let output = directory.path().join("kickstart.pkg");
        fs::write(&member, b"not an ELF object").unwrap();
        fs::write(&output, b"existing package sentinel").unwrap();

        let error = create(
            &output,
            &[member],
            PathMode::Basename,
            false,
            &CreatePolicy::NoClobber,
        )
        .unwrap_err();

        assert!(error.to_string().contains("not ELF objects"));
        assert_eq!(fs::read(output).unwrap(), b"existing package sentinel");
    }

    #[test]
    fn existing_package_requires_explicit_digest_cas() {
        let directory = tempfile::tempdir().unwrap();
        let member = directory.path().join("exec.library");
        let output = directory.path().join("kickstart.pkg");
        fs::write(&member, b"\x7fELFpayload").unwrap();
        fs::write(&output, b"existing package sentinel").unwrap();

        let expected = sha256_bytes(&fs::read(&output).unwrap());
        let outcome = create(
            &output,
            &[member],
            PathMode::Basename,
            false,
            &CreatePolicy::ReplaceIfSha256(expected),
        )
        .unwrap();
        let published = fs::read(output).unwrap();

        assert_eq!(parse(&published).unwrap(), outcome.entries);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(directory.path().join("kickstart.pkg"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
    }
}
