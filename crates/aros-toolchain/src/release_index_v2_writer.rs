//! Exclusive local output of a measured compiler-family index.
//!
//! This writes one index into a caller-owned pre-index stage. It is not final
//! checksum generation, evidence authentication or publication admission.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use rustix::fs::{self, Mode, OFlags};

use crate::filesystem::open_directory;
use crate::release_index_v2::{NativeReleaseIndexV2, CHECKSUMS_NAME, INDEX_NAME, PROVENANCE_NAME};
use crate::release_index_v2_builder::{build_measured_index_v2, MeasuredReleaseIndexRequestV2};
use crate::release_index_v2_readback::{
    require_exact_inventory, validate_directory_path, verify_bound_documents,
    verify_input_collection, verify_static_support, MAX_METADATA_BYTES,
};
use crate::ContractError;

/// Persisted canonical index bytes and their independently reread identity.
///
/// Construction is private. This result does not establish a stable snapshot of
/// package files or authenticate release evidence, signatures or provenance.
#[derive(Debug, Clone)]
pub struct WrittenReleaseIndexV2 {
    index: NativeReleaseIndexV2,
    path: PathBuf,
    sha256: Sha256Digest,
    size: u64,
}

impl WrittenReleaseIndexV2 {
    /// Complete index measured from the pre-index package stage.
    #[must_use]
    pub const fn index(&self) -> &NativeReleaseIndexV2 {
        &self.index
    }

    /// Absolute output path; exactly the canonical index basename.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the exact canonical bytes reread from the new file.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Size of those exact persisted bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

/// Verify the complete pre-index stage and exclusively write its canonical index.
///
/// All builder verification runs before file creation. The output is opened
/// relative to a held no-follow directory descriptor, with exclusive creation:
/// an existing regular file, directory, symlink or raced destination is never
/// replaced. File and directory synchronization precede descriptor-relative
/// read-back. Directory/file identities, exact output bytes and the expected
/// indexed-but-not-final inventory are checked before returning a result.
///
/// The caller must own a quiescent stage. No advisory ownership lock or stable
/// snapshot of concurrent package writers is provided. Checksums and provenance
/// remain absent. A failure after output reservation retains that file for
/// diagnosis, possibly incomplete; no successful result or automatic rollback
/// is returned. Retry requires a fresh stage, never replacement of the file.
/// Final package read-back and release/evidence gates remain mandatory.
///
/// # Errors
///
/// Returns a sanitized index-contract error for any builder violation, unsafe
/// or changing directory, existing output, persistence or read-back failure.
pub fn write_measured_index_v2(
    request: &MeasuredReleaseIndexRequestV2,
) -> Result<WrittenReleaseIndexV2, ContractError> {
    validate_directory_path(&request.directory)?;
    let directory = open_directory(&request.directory)
        .map_err(|_| error("cannot open the release stage directory without following links"))?;
    let directory_identity = identity(&directory)?;
    let index = build_measured_index_v2(request)?;
    let bytes = index.to_json_bytes()?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(error("canonical measured index exceeds the metadata bound"));
    }
    require_same_directory(&request.directory, directory_identity)?;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut output = File::from(
        fs::openat(&directory, INDEX_NAME, flags, Mode::from_raw_mode(0o644))
            .map_err(|_| error("cannot exclusively reserve the canonical release index"))?,
    );
    let output_identity = identity(&output)?;
    output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .and_then(|()| directory.sync_all())
        .map_err(|_| error("cannot persist and synchronize the canonical release index"))?;

    let mut readback = File::from(
        fs::openat(
            &directory,
            INDEX_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("cannot reopen the persisted index without following links"))?,
    );
    let metadata = readback
        .metadata()
        .map_err(|_| error("cannot inspect the persisted index"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() != bytes.len() as u64
        || identity(&readback)? != output_identity
    {
        return Err(error("persisted index identity or size changed"));
    }
    let mut actual = Vec::with_capacity(bytes.len());
    std::io::Read::by_ref(&mut readback)
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut actual)
        .map_err(|_| error("cannot read back the persisted canonical index"))?;
    if actual != bytes || NativeReleaseIndexV2::parse(&actual, &request.inputs)? != index {
        return Err(error(
            "persisted index differs from its measured canonical bytes",
        ));
    }
    require_same_directory(&request.directory, directory_identity)?;
    let mut inventory = index.expected_inventory().clone();
    inventory.remove(CHECKSUMS_NAME);
    inventory.remove(PROVENANCE_NAME);
    require_exact_inventory(&request.directory, &inventory)?;
    verify_input_collection(&request.directory, &request.inputs)?;
    verify_bound_documents(&request.directory, &request.inputs)?;
    verify_static_support(&request.directory)?;
    let final_file = File::from(
        fs::openat(
            &directory,
            INDEX_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("persisted index became inaccessible"))?,
    );
    let final_metadata = final_file
        .metadata()
        .map_err(|_| error("persisted index metadata became inaccessible"))?;
    if !final_metadata.is_file()
        || final_metadata.nlink() != 1
        || final_metadata.len() != bytes.len() as u64
        || identity(&final_file)? != output_identity
    {
        return Err(error("persisted index was replaced during read-back"));
    }
    let mut final_bytes = Vec::with_capacity(bytes.len());
    final_file
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut final_bytes)
        .map_err(|_| error("persisted index could not be revalidated after stage checks"))?;
    if final_bytes != bytes {
        return Err(error("persisted index bytes changed during stage checks"));
    }
    require_same_directory(&request.directory, directory_identity)?;
    Ok(WrittenReleaseIndexV2 {
        index,
        path: request.directory.join(INDEX_NAME),
        sha256: sha256_bytes(&actual),
        size: actual.len() as u64,
    })
}

fn identity(file: &File) -> Result<(u64, u64), ContractError> {
    let metadata = file
        .metadata()
        .map_err(|_| error("cannot inspect release output descriptor identity"))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn require_same_directory(path: &Path, expected: (u64, u64)) -> Result<(), ContractError> {
    validate_directory_path(path)?;
    let directory = open_directory(path)
        .map_err(|_| error("release stage directory changed or became inaccessible"))?;
    if identity(&directory)? != expected {
        return Err(error("release stage directory identity changed"));
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::index(message)
}
