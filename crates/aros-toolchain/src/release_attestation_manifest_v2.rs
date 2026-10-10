//! Measured pre-attestation subjects, distinct from final release checksums.
//!
//! The manifest is a workflow input outside the release directory. Its digest
//! binds the list supplied to external attestation; it does not authenticate
//! that attestation, include itself as a subject, or admit publication.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use aros_common::{sha256_bytes, Sha256Digest};
use rustix::fs::{self, Mode, OFlags};

use crate::filesystem::open_directory;
use crate::release_checksums_v2::{
    measure_attestation_members, verify_final_checksums_v2, FinalChecksumMemberV2,
};
use crate::release_index_v2::{CHECKSUMS_NAME, PROVENANCE_NAME};
use crate::release_index_v2_readback::{
    readback_indexed_packages_with_inventory, validate_directory_path,
    IndexedPackageReadbackRequestV2, MAX_METADATA_BYTES,
};
use crate::ContractError;

/// Exact inventory boundary at which existing subjects are verified.
#[derive(Debug, Clone, Copy)]
pub enum AttestationManifestStageV2 {
    /// Canonical index present; provenance and final checksums absent.
    PreAttestation,
    /// Provenance present; final checksums absent.
    PreChecksums,
    /// Complete final inventory, including verified final checksums.
    Final,
}

/// Verified manifest and independently measured subject identities.
#[derive(Debug, Clone)]
pub struct AttestationManifestReadbackV2 {
    sha256: Sha256Digest,
    size: u64,
    members: Vec<FinalChecksumMemberV2>,
}

impl AttestationManifestReadbackV2 {
    /// SHA-256 of the exact canonical subject-list bytes, not final checksums.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Size of the exact persisted subject manifest.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// All subjects in filename order; excludes provenance and final checksums.
    #[must_use]
    pub fn members(&self) -> &[FinalChecksumMemberV2] {
        &self.members
    }
}

/// Write one exclusive subject manifest from the exact pre-attestation stage.
///
/// Before output reservation this verifies every compiler-family package,
/// independent build environment, input/support/index document, required path,
/// and forbidden prefix. The list covers every index-derived member except
/// `SHA256SUMS` and the later provenance bundle. Encoding matches final checksum
/// lines, but the manifest must live outside the release directory.
///
/// Existing files, links, directories or raced outputs are never replaced.
/// Descriptor-relative persistence and independent read-back follow creation.
/// A failure after reservation retains diagnostic output, never a successful
/// result. Retry needs a fresh output. The caller must own a quiescent stage;
/// this is not an ownership lock or a snapshot against concurrent writers.
///
/// # Errors
/// Returns a sanitized index error for invalid inputs, unsafe/existing output,
/// resource violations, persistence failure or observed changes.
pub fn write_attestation_manifest_v2(
    request: &IndexedPackageReadbackRequestV2,
    output: &Path,
) -> Result<AttestationManifestReadbackV2, ContractError> {
    let (parent, name) = manifest_destination(request, output)?;
    let output_directory = open_directory(parent)
        .map_err(|_| error("cannot open subject manifest parent without following links"))?;
    let output_directory_identity = identity(&output_directory)?;
    let release_directory = open_directory(&request.directory)
        .map_err(|_| error("cannot open subject release stage without following links"))?;
    let release_identity = identity(&release_directory)?;
    require_distinct_directories(output_directory_identity, release_identity)?;
    let stage = AttestationManifestStageV2::PreAttestation;
    verify_packages(request, stage)?;
    let (bytes, _) = measure_attestation_members(&request.directory, &request.index)?;
    require_same_directory(parent, output_directory_identity)?;
    require_same_directory(&request.directory, release_identity)?;

    let mut file = File::from(
        fs::openat(
            &output_directory,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .map_err(|_| error("cannot exclusively reserve subject manifest output"))?,
    );
    let output_identity = identity(&file)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| output_directory.sync_all())
        .map_err(|_| error("cannot persist and synchronize subject manifest"))?;
    let readback = verify_attestation_manifest_v2(request, output, stage)?;
    if readback.sha256() != &sha256_bytes(&bytes) || readback.size() != bytes.len() as u64 {
        return Err(error(
            "persisted subject manifest differs from measured bytes",
        ));
    }
    let persisted = open_manifest(&output_directory, name)?;
    if identity(&persisted)? != output_identity || read_manifest(persisted)? != bytes {
        return Err(error(
            "persisted subject manifest identity or bytes changed",
        ));
    }
    require_same_directory(parent, output_directory_identity)?;
    require_same_directory(&request.directory, release_identity)?;
    Ok(readback)
}

/// Re-measure every subject and verify the unchanged manifest at an exact stage.
///
/// The same pre-attestation list is checked before or after provenance appears;
/// the final stage additionally verifies complete final checksums. Packages and
/// bound input documents are read back at every stage. The output file is not
/// part of the release inventory. Success measures bytes only: provenance is
/// neither parsed nor authenticated and no compiler/compatibility result is
/// inferred. This function writes nothing and returns no partial result.
///
/// # Errors
/// Returns a sanitized index error for an inexact/invalid stage, unsafe manifest,
/// subject changes, malformed checksum lines or observed identity/byte changes.
pub fn verify_attestation_manifest_v2(
    request: &IndexedPackageReadbackRequestV2,
    manifest: &Path,
    stage: AttestationManifestStageV2,
) -> Result<AttestationManifestReadbackV2, ContractError> {
    let (parent, name) = manifest_destination(request, manifest)?;
    let directory = open_directory(parent)
        .map_err(|_| error("cannot open subject manifest parent without following links"))?;
    let parent_identity = identity(&directory)?;
    let file = open_manifest(&directory, name)?;
    let manifest_identity = identity(&file)?;
    let existing = read_manifest(file)?;
    let release = open_directory(&request.directory)
        .map_err(|_| error("cannot open subject release stage without following links"))?;
    let release_identity = identity(&release)?;
    require_distinct_directories(parent_identity, release_identity)?;
    verify_packages(request, stage)?;
    let (bytes, members) = measure_attestation_members(&request.directory, &request.index)?;
    if bytes != existing {
        return Err(error(
            "subject manifest does not cover the exact measured canonical subjects",
        ));
    }
    // Repeat package/input and inventory observation after streaming subjects.
    // This is still not a stable snapshot against concurrent mutation.
    verify_packages(request, stage)?;
    let final_file = open_manifest(&directory, name)?;
    if identity(&final_file)? != manifest_identity || read_manifest(final_file)? != existing {
        return Err(error(
            "subject manifest identity or bytes changed during verification",
        ));
    }
    require_same_directory(parent, parent_identity)?;
    require_same_directory(&request.directory, release_identity)?;
    Ok(AttestationManifestReadbackV2 {
        sha256: sha256_bytes(&existing),
        size: existing.len() as u64,
        members,
    })
}

fn verify_packages(
    request: &IndexedPackageReadbackRequestV2,
    stage: AttestationManifestStageV2,
) -> Result<(), ContractError> {
    let mut inventory = request.index.expected_inventory().clone();
    match stage {
        AttestationManifestStageV2::PreAttestation => {
            inventory.remove(CHECKSUMS_NAME);
            inventory.remove(PROVENANCE_NAME);
        }
        AttestationManifestStageV2::PreChecksums => {
            inventory.remove(CHECKSUMS_NAME);
        }
        AttestationManifestStageV2::Final => {}
    }
    readback_indexed_packages_with_inventory(request, &inventory)?;
    if matches!(stage, AttestationManifestStageV2::Final) {
        verify_final_checksums_v2(&request.directory, &request.index)?;
    }
    Ok(())
}

fn manifest_destination<'a>(
    request: &IndexedPackageReadbackRequestV2,
    output: &'a Path,
) -> Result<(&'a Path, &'a std::ffi::OsStr), ContractError> {
    validate_directory_path(&request.directory)?;
    if !output.is_absolute() || output.starts_with(&request.directory) {
        return Err(error(
            "subject manifest must be absolute and outside the release directory",
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| error("subject manifest lacks a parent"))?;
    let name = output
        .file_name()
        .ok_or_else(|| error("subject manifest lacks a filename"))?;
    validate_directory_path(parent)?;
    Ok((parent, name))
}

fn open_manifest(directory: &File, name: &std::ffi::OsStr) -> Result<File, ContractError> {
    let file = File::from(
        fs::openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| error("cannot safely open subject manifest"))?,
    );
    let metadata = file
        .metadata()
        .map_err(|_| error("cannot inspect subject manifest"))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_METADATA_BYTES {
        return Err(error(
            "subject manifest exceeds its exclusive regular-file bound",
        ));
    }
    Ok(file)
}

fn read_manifest(mut file: File) -> Result<Vec<u8>, ContractError> {
    let size = file
        .metadata()
        .map_err(|_| error("cannot inspect subject manifest"))?
        .len();
    let mut bytes = Vec::with_capacity(size as usize);
    std::io::Read::by_ref(&mut file)
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("cannot read subject manifest"))?;
    if bytes.len() as u64 != size || bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(error(
            "subject manifest changed or exceeded its bound while read",
        ));
    }
    Ok(bytes)
}

fn identity(file: &File) -> Result<(u64, u64), ContractError> {
    let metadata = file
        .metadata()
        .map_err(|_| error("cannot inspect subject descriptor identity"))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn require_distinct_directories(
    manifest_parent: (u64, u64),
    release: (u64, u64),
) -> Result<(), ContractError> {
    if manifest_parent == release {
        return Err(error(
            "subject manifest parent aliases the release directory",
        ));
    }
    Ok(())
}

fn require_same_directory(path: &Path, expected: (u64, u64)) -> Result<(), ContractError> {
    validate_directory_path(path)?;
    let directory =
        open_directory(path).map_err(|_| error("subject directory became inaccessible"))?;
    if identity(&directory)? != expected {
        return Err(error("subject directory identity changed"));
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::index(message)
}

#[cfg(test)]
mod tests {
    use super::require_distinct_directories;

    #[test]
    fn manifest_parent_cannot_alias_the_release_descriptor() {
        assert!(require_distinct_directories((1, 7), (1, 8)).is_ok());
        assert!(require_distinct_directories((2, 7), (1, 7)).is_ok());
        let error = require_distinct_directories((1, 7), (1, 7)).unwrap_err();
        assert!(error.to_string().contains("aliases the release directory"));
    }
}
