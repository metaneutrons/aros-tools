//! Exclusive local checksum output after complete indexed package validation.
//!
//! This stage establishes measured bytes, not authenticated provenance,
//! compiler qualification or permission to publish a release.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use rustix::fs::{self, Mode, OFlags};

use crate::filesystem::open_directory;
use crate::release_checksums_v2::{
    measure_final_checksum_members, verify_final_checksums_v2, FinalChecksumsReadbackV2,
};
use crate::release_index_v2::CHECKSUMS_NAME;
use crate::release_index_v2_readback::{
    readback_indexed_packages, readback_indexed_packages_with_inventory, validate_directory_path,
    IndexedPackageReadbackRequestV2, MAX_METADATA_BYTES,
};
use crate::ContractError;

/// Newly persisted checksum bytes and their independently verified inventory.
///
/// This result has no public constructor and cannot admit publication. The
/// provenance bundle was measured as a file, not authenticated as a signature.
#[derive(Debug, Clone)]
pub struct WrittenFinalChecksumsV2 {
    path: PathBuf,
    sha256: Sha256Digest,
    size: u64,
    readback: FinalChecksumsReadbackV2,
}

impl WrittenFinalChecksumsV2 {
    /// Absolute output path; exactly the canonical checksum basename.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the exact canonical persisted checksum document.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Size of the independently reread checksum document.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Complete final checksum measurement; not signature verification.
    #[must_use]
    pub const fn readback(&self) -> &FinalChecksumsReadbackV2 {
        &self.readback
    }
}

/// Validate a complete pre-checksum stage and exclusively write `SHA256SUMS`.
///
/// The stage must contain exactly the index-derived final inventory minus
/// `SHA256SUMS`, including the already assembled provenance bundle. Before
/// reserving any output this verifies bound input/support/index documents,
/// every compiler-family package and its independent environment, required
/// paths and forbidden prefixes. All remaining regular files are measured;
/// archive identities and canonical index bytes must match the bound index.
///
/// Output creation is descriptor-relative, no-follow and exclusive. Existing
/// files, links or other destinations are never replaced. File/directory fsync
/// precedes independent full checksum verification and full package read-back.
/// The output descriptor identity, bytes and directory identity are checked
/// again after those final checks. Failures return no successful result; after
/// reservation the diagnostic output is retained and retry needs a fresh stage.
///
/// The caller must own a quiescent stage: this is not a lock or concurrent-writer
/// snapshot. It neither creates nor authenticates provenance, signatures, A/B,
/// compatibility, relocation or publication evidence. Those later gates remain
/// mandatory; canonical checksums alone cannot establish a qualified release.
///
/// # Errors
///
/// Returns a sanitized index-contract error for any invalid input, unsafe or
/// existing output, persistence, read-back or observed identity/byte change.
pub fn write_final_checksums_v2(
    request: &IndexedPackageReadbackRequestV2,
) -> Result<WrittenFinalChecksumsV2, ContractError> {
    validate_directory_path(&request.directory)?;
    let directory = open_directory(&request.directory)
        .map_err(|_| error("cannot open the checksum stage without following links"))?;
    let directory_identity = identity(&directory)?;
    let mut inventory = request.index.expected_inventory().clone();
    inventory.remove(CHECKSUMS_NAME);
    readback_indexed_packages_with_inventory(request, &inventory)?;
    let (bytes, _) = measure_final_checksum_members(&request.directory, &request.index)?;
    require_same_directory(&request.directory, directory_identity)?;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut output = File::from(
        fs::openat(
            &directory,
            CHECKSUMS_NAME,
            flags,
            Mode::from_raw_mode(0o644),
        )
        .map_err(|_| error("cannot exclusively reserve the canonical checksum document"))?,
    );
    let output_identity = identity(&output)?;
    output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .and_then(|()| directory.sync_all())
        .map_err(|_| error("cannot persist and synchronize the checksum document"))?;
    require_same_directory(&request.directory, directory_identity)?;
    let readback = verify_final_checksums_v2(&request.directory, &request.index)?;
    readback_indexed_packages(request)?;
    if readback.checksums_sha256() != &sha256_bytes(&bytes) {
        return Err(error(
            "persisted checksums differ from the measured canonical bytes",
        ));
    }
    let final_file = File::from(
        fs::openat(
            &directory,
            CHECKSUMS_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("persisted checksum document became inaccessible"))?,
    );
    let metadata = final_file
        .metadata()
        .map_err(|_| error("persisted checksum metadata became inaccessible"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() != bytes.len() as u64
        || identity(&final_file)? != output_identity
    {
        return Err(error("persisted checksum identity or size changed"));
    }
    let mut actual = Vec::with_capacity(bytes.len());
    final_file
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut actual)
        .map_err(|_| error("cannot read back the persisted checksum document"))?;
    if actual != bytes {
        return Err(error(
            "persisted checksum bytes changed during stage checks",
        ));
    }
    require_same_directory(&request.directory, directory_identity)?;
    Ok(WrittenFinalChecksumsV2 {
        path: request.directory.join(CHECKSUMS_NAME),
        sha256: sha256_bytes(&actual),
        size: actual.len() as u64,
        readback,
    })
}

fn identity(file: &File) -> Result<(u64, u64), ContractError> {
    let metadata = file
        .metadata()
        .map_err(|_| error("cannot inspect checksum descriptor identity"))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn require_same_directory(path: &Path, expected: (u64, u64)) -> Result<(), ContractError> {
    validate_directory_path(path)?;
    let directory = open_directory(path)
        .map_err(|_| error("checksum stage directory changed or became inaccessible"))?;
    if identity(&directory)? != expected {
        return Err(error("checksum stage directory identity changed"));
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::index(message)
}
