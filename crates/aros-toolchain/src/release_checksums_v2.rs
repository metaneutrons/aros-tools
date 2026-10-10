//! Read-only final checksum verification for an exact compiler-family inventory.
//!
//! File checksums establish byte identities, not package validity, compiler
//! functionality, signature authenticity, provenance or publication readiness.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use aros_common::{open_regular_file_nofollow, sha256_bytes, sha256_reader, Sha256Digest};

use crate::release_index_v2::{
    NativeReleaseIndexV2, CHECKSUMS_NAME, INDEX_NAME, MAX_ARCHIVE_BYTES, PROVENANCE_NAME,
};
use crate::release_index_v2_readback::{
    read_bounded_regular_file, require_exact_inventory, validate_directory_path, MAX_METADATA_BYTES,
};
use crate::ContractError;

/// Completed local checksum read-back for every other final release member.
#[derive(Debug, Clone)]
pub struct FinalChecksumsReadbackV2 {
    checksums_sha256: Sha256Digest,
    members: Vec<FinalChecksumMemberV2>,
}

impl FinalChecksumsReadbackV2 {
    /// SHA-256 of the exact canonical checksum document that passed verification.
    #[must_use]
    pub const fn checksums_sha256(&self) -> &Sha256Digest {
        &self.checksums_sha256
    }

    /// Independently measured identities in canonical filename order.
    #[must_use]
    pub fn members(&self) -> &[FinalChecksumMemberV2] {
        &self.members
    }
}

/// One exact regular release file measured during checksum verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalChecksumMemberV2 {
    name: String,
    sha256: Sha256Digest,
    size: u64,
}

impl FinalChecksumMemberV2 {
    /// Flat filename derived from the validated release index inventory.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// SHA-256 measured from the safely opened file stream.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Byte length measured from the same stream.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

/// Verify the final checksum document against every other exact inventory file.
///
/// The input index must already be bound to validated release inputs. The final
/// inventory includes the provenance bundle, whose bytes are hashed here but
/// whose signature is not authenticated. The checksum document covers all
/// members except itself: lowercase SHA-256, two spaces, flat filename and LF,
/// with complete lines in lexical order, matching the v1 checksum convention.
///
/// This operation writes nothing. It validates real directory ancestors and
/// exact regular-file inventory before and after streaming bounded files, and
/// binds measured archive hashes and sizes to the index. The on-disk index must
/// match its exact canonical serialized bytes, even if checksums are rehashed.
/// It checks that the
/// checksum document remained equal at both observations. It does not acquire
/// an ownership lock or guarantee a stable snapshot against concurrent writers.
///
/// # Errors
///
/// Returns a sanitized index-contract error for unsafe or incomplete material,
/// resource-limit violations, index-mismatched archives, or any noncanonical,
/// incomplete or incorrect checksum document. No partial result is returned.
pub fn verify_final_checksums_v2(
    directory: &Path,
    index: &NativeReleaseIndexV2,
) -> Result<FinalChecksumsReadbackV2, ContractError> {
    validate_directory_path(directory)?;
    require_exact_inventory(directory, index.expected_inventory())?;
    let checksum_path = directory.join(CHECKSUMS_NAME);
    let existing = read_bounded_regular_file(&checksum_path, "final release checksums")?;
    let (canonical, members) = measure_final_checksum_members(directory, index)?;
    if canonical != existing {
        return Err(ContractError::index(
            "final release checksums do not cover the exact measured canonical inventory",
        ));
    }
    validate_directory_path(directory)?;
    require_exact_inventory(directory, index.expected_inventory())?;
    if read_bounded_regular_file(&checksum_path, "final release checksums")? != existing {
        return Err(ContractError::index(
            "final release checksum document changed during verification",
        ));
    }
    Ok(FinalChecksumsReadbackV2 {
        checksums_sha256: sha256_bytes(&existing),
        members,
    })
}

// Shared measurement for the verifier and the exclusive local writer. The
// caller validates its exact stage inventory; this helper never writes files.
pub(crate) fn measure_final_checksum_members(
    directory: &Path,
    index: &NativeReleaseIndexV2,
) -> Result<(Vec<u8>, Vec<FinalChecksumMemberV2>), ContractError> {
    measure_checksum_members(directory, index, false)
}

// The pre-attestation manifest has the same canonical encoding but excludes
// both outputs that can only exist after external attestation. It is stored
// outside the release inventory and is never its own attestation subject.
pub(crate) fn measure_attestation_members(
    directory: &Path,
    index: &NativeReleaseIndexV2,
) -> Result<(Vec<u8>, Vec<FinalChecksumMemberV2>), ContractError> {
    measure_checksum_members(directory, index, true)
}

fn measure_checksum_members(
    directory: &Path,
    index: &NativeReleaseIndexV2,
    pre_attestation: bool,
) -> Result<(Vec<u8>, Vec<FinalChecksumMemberV2>), ContractError> {
    let index_bytes = index
        .to_json_bytes()
        .map_err(|_| ContractError::index("cannot serialize the bound checksum release index"))?;
    let index_digest = sha256_bytes(&index_bytes);
    let archives = index
        .artifacts()
        .iter()
        .map(|artifact| (artifact.asset(), artifact))
        .collect::<BTreeMap<_, _>>();
    let mut members = Vec::with_capacity(index.expected_inventory().len() - 1);
    let mut lines = Vec::with_capacity(members.capacity());
    for name in index.expected_inventory() {
        if name == CHECKSUMS_NAME || (pre_attestation && name == PROVENANCE_NAME) {
            continue;
        }
        let archive = archives.get(name.as_str());
        let limit = if archive.is_some() {
            MAX_ARCHIVE_BYTES
        } else {
            MAX_METADATA_BYTES
        };
        let member = measure_member(directory, name, limit)?;
        if name == INDEX_NAME
            && (member.sha256() != &index_digest || member.size() != index_bytes.len() as u64)
        {
            return Err(ContractError::index(
                "checksum release index differs from its bound canonical bytes",
            ));
        }
        if archive.is_some_and(|artifact| {
            member.sha256() != artifact.sha256() || member.size() != artifact.size()
        }) {
            return Err(ContractError::index(
                "measured checksum archive identity differs from the bound index",
            ));
        }
        lines.push(format!("{}  {}\n", member.sha256(), member.name()));
        members.push(member);
    }
    lines.sort_unstable();
    let canonical = lines.concat().into_bytes();
    if canonical.len() as u64 > MAX_METADATA_BYTES {
        return Err(ContractError::index(
            "measured final checksums exceed their metadata bound",
        ));
    }
    Ok((canonical, members))
}

fn measure_member(
    directory: &Path,
    name: &str,
    limit: u64,
) -> Result<FinalChecksumMemberV2, ContractError> {
    let mut file = open_regular_file_nofollow(&directory.join(name))
        .map_err(|_| ContractError::index("cannot safely open checksum inventory member"))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::index("cannot inspect checksum inventory member"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(ContractError::index(
            "checksum inventory member exceeds its regular-file resource bound",
        ));
    }
    let measured = sha256_reader(&mut file.by_ref().take(limit + 1))
        .map_err(|_| ContractError::index("cannot measure checksum inventory member"))?;
    if measured.size > limit || measured.size != metadata.len() {
        return Err(ContractError::index(
            "checksum inventory member changed or exceeded its bound while measured",
        ));
    }
    Ok(FinalChecksumMemberV2 {
        name: name.to_owned(),
        sha256: measured.digest,
        size: measured.size,
    })
}
