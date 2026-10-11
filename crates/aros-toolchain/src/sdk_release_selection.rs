//! Measure a local SDK package into an explicit release byte-selection lock.
//!
//! This module records selected bytes only. It does not establish execution,
//! producer provenance, publication, publisher authenticity, or authorization.

use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::SDK_MAX_METADATA_BYTES;
use aros_common::sdk_release_lock::{SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_SCHEMA};
use aros_common::{measure_regular_file_digest_bounded, FileIdentity, Sha256Result};

use crate::sdk_package::{
    self, SdkLockedArchiveRequest, SdkVerificationRequest, VerifiedSdkPackage,
};
use crate::ContractError;

/// Inputs for measuring one already packaged local SDK release selection.
#[derive(Debug, Clone)]
pub struct MeasuredSdkReleaseLockRequest {
    /// Package inputs and independently selected SDK/archive identities.
    pub package: SdkVerificationRequest,
    /// Canonical `v`-prefixed release tag for the selected release directory.
    pub release_tag: String,
    /// Credential-free HTTPS base URL ending in `release_tag`.
    pub base_url: String,
}

/// Verify a local SDK package and measure its external manifest into a lock.
///
/// The package request supplies the expected SDK identity, inventory digest,
/// archive digest and archive size. The manifest digest and size are measured
/// from the bounded no-follow external manifest. The package is fully verified
/// before and after measurement, then its archive is independently checked
/// against the constructed lock.
///
/// This is a byte-consistency operation. The returned lock does not attest
/// that the SDK was executed, published, or produced by an authorized job.
///
/// # Errors
///
/// Returns an error if the release selection is invalid, any package member
/// fails full SDK package verification, the manifest cannot be measured as a
/// bounded no-follow regular file, inputs change during the operation, or the
/// constructed lock does not verify the selected archive.
pub fn build_measured_sdk_release_lock(
    request: &MeasuredSdkReleaseLockRequest,
) -> Result<SdkReleaseLock, ContractError> {
    build_measured_sdk_release_lock_with(request, sdk_package::verify).map(|(lock, _)| lock)
}

/// Measure a selected SDK package in a shared release directory.
///
/// The caller must establish and recheck the exact union inventory of the
/// shared release directory around this call. This function verifies only the
/// selected SDK members and retains the same manifest and archive measurement
/// checks as [`build_measured_sdk_release_lock`].
pub(crate) fn build_measured_sdk_release_lock_in_release_directory(
    request: &MeasuredSdkReleaseLockRequest,
) -> Result<(SdkReleaseLock, VerifiedSdkPackage), ContractError> {
    build_measured_sdk_release_lock_with(
        request,
        sdk_package::verify_selected_members_in_release_directory,
    )
}

fn build_measured_sdk_release_lock_with(
    request: &MeasuredSdkReleaseLockRequest,
    verify_package: impl Fn(&SdkVerificationRequest) -> Result<VerifiedSdkPackage, ContractError>,
) -> Result<(SdkReleaseLock, VerifiedSdkPackage), ContractError> {
    let archive_digest = request.package.expected_archive_sha256.clone();
    let archive_size = request.package.expected_archive_size;
    let preflight = SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: request.release_tag.clone(),
        base_url: request.base_url.clone(),
        identity: request.package.identity.clone(),
        tree_sha256: request.package.expected_tree_sha256.clone(),
        archive: SdkArtifactDigest {
            sha256: archive_digest.clone(),
            size: archive_size,
        },
        // A valid bounded placeholder lets the shared lock validator check
        // selection fields before reading the potentially large archive.
        manifest: SdkArtifactDigest {
            sha256: aros_common::sha256_bytes(b"SDK manifest measurement preflight"),
            size: 1,
        },
    };
    preflight.validate().map_err(|_| {
        ContractError::verification(
            "SDK release tag, base URL, identity or selected artifact values are invalid",
        )
    })?;

    let archive_name = request
        .package
        .identity
        .asset_name()
        .map_err(|_| ContractError::verification("invalid selected SDK archive identity"))?;
    let archive_path = request.package.package_dir.join(&archive_name);
    let manifest_path = manifest_path(&request.package.package_dir, &archive_name);

    let initial = verify_package(&request.package)?;
    ensure_selected_package(&request.package, &initial)?;
    let (manifest_identity_before, manifest_before) = measure_manifest(&manifest_path)?;

    let lock = SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: request.release_tag.clone(),
        base_url: request.base_url.clone(),
        // These values stay selected by the request; they are not inferred
        // from whatever package happened to be present on disk.
        identity: request.package.identity.clone(),
        tree_sha256: request.package.expected_tree_sha256.clone(),
        archive: SdkArtifactDigest {
            sha256: archive_digest,
            size: archive_size,
        },
        manifest: artifact_digest(&manifest_before),
    };
    let canonical = lock.canonical_bytes().map_err(|_| {
        ContractError::verification("measured SDK release lock is invalid or oversized")
    })?;
    let decoded = SdkReleaseLock::from_bytes(&canonical)
        .map_err(|_| ContractError::verification("cannot validate canonical SDK release lock"))?;
    if decoded != lock {
        return Err(ContractError::verification(
            "canonical SDK release lock did not preserve its selected fields",
        ));
    }
    let lock = decoded;

    let locked = sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
        archive: archive_path,
        lock: lock.clone(),
    })?;
    let (manifest_identity_after, manifest_after) = measure_manifest(&manifest_path)?;
    let final_package = verify_package(&request.package)?;
    ensure_selected_package(&request.package, &final_package)?;
    if initial != final_package
        || locked != final_package
        || manifest_identity_before != manifest_identity_after
        || manifest_before != manifest_after
    {
        return Err(ContractError::verification(
            "SDK package or external manifest changed while measuring its release lock",
        ));
    }
    Ok((lock, final_package))
}

fn manifest_path(package_dir: &Path, archive_name: &str) -> PathBuf {
    package_dir.join(format!("{archive_name}.manifest.json"))
}

fn measure_manifest(path: &Path) -> Result<(FileIdentity, Sha256Result), ContractError> {
    measure_regular_file_digest_bounded(path, SDK_MAX_METADATA_BYTES as u64).map_err(|_| {
        ContractError::verification(
            "external SDK manifest is missing, unsafe, changed or exceeds its metadata limit",
        )
    })
}

fn artifact_digest(measurement: &Sha256Result) -> SdkArtifactDigest {
    SdkArtifactDigest {
        sha256: measurement.digest.clone(),
        size: measurement.size,
    }
}

fn ensure_selected_package(
    request: &SdkVerificationRequest,
    verified: &VerifiedSdkPackage,
) -> Result<(), ContractError> {
    if verified.manifest.identity != request.identity
        || verified.manifest.tree_sha256 != request.expected_tree_sha256
        || verified.archive_sha256 != request.expected_archive_sha256
        || verified.archive_size != request.expected_archive_size
    {
        return Err(ContractError::verification(
            "verified SDK package differs from its independently selected identity or bytes",
        ));
    }
    Ok(())
}
