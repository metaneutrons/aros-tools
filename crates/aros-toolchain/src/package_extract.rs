//! Fresh, retained extraction for a native package already verified in place.
//!
//! This is deliberately not the consumer installation path. Compatibility
//! qualification needs two independently revalidated roots that remain
//! inspectable after a failed probe. It therefore accepts a complete native
//! package-set verification request, creates one absent owned root and streams
//! the exact verified archive through the producer's strict package parser.

use std::fs;
use std::path::{Component, Path, PathBuf};

use aros_common::{toolchain_tree_inventory, ArosToolchainManifest};

use crate::filesystem::open_directory;
use crate::package::canonical_asset_name;
use crate::package_verify::{
    extract_verified_archive, measure_archive, verify, PackageVerificationRequest, VerifiedPackage,
};
use crate::ContractError;

/// Inputs for one fresh extraction of a complete native package set.
#[derive(Debug, Clone)]
pub struct PackageExtractionRequest {
    /// Complete read-back contract for the local package directory.
    pub verification: PackageVerificationRequest,
    /// Absent destination for one extracted `toolchain/` payload root.
    pub output_root: PathBuf,
}

/// One extracted, reverified package payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPackage {
    /// Fresh extracted `toolchain/` root, retained after this operation returns.
    pub root: PathBuf,
    /// Read-back package identity that authorized this extraction.
    pub verified: VerifiedPackage,
}

/// Verify a complete package set, then extract its exact archive to one root.
///
/// The destination must be an absent direct child of an existing real parent.
/// A successful call leaves a fully re-inventoried payload at `output_root`.
/// On every post-creation failure it deliberately retains the fresh root and
/// any partial material for inspection; it neither retries nor removes it.
///
/// # Errors
///
/// Returns AX0602 for unsafe package inputs, changed archives, unsafe output
/// roots, malformed archive contents, or a post-extraction inventory mismatch.
/// This operation has no network, cache, source, tag or publication authority.
pub fn verify_and_extract(
    request: &PackageExtractionRequest,
) -> Result<ExtractedPackage, ContractError> {
    let verified = verify(&request.verification)?;
    let root = create_fresh_root(&request.output_root)?;
    let asset = canonical_asset_name(
        request.verification.source_lock.version(),
        &request.verification.host,
        request.verification.profile.name(),
    )?;
    let archive = request.verification.package_dir.join(asset);
    let (file, size, sha256) = measure_archive(&archive)?;
    if size != verified.archive_size || sha256 != verified.archive_sha256 {
        return Err(ContractError::verification(
            "package archive changed after complete package-set verification",
        ));
    }
    extract_verified_archive(
        file,
        &verified.manifest,
        &request.verification.forbidden_prefixes,
        &root,
    )?;
    verify_extracted_tree(&root, &verified.manifest)?;
    Ok(ExtractedPackage { root, verified })
}

/// Resolve one absent output root without creating it.
///
/// The returned path has a canonical real parent and one normal final segment.
/// Callers use this to reject overlapping destinations before the first
/// retained extraction begins; [`verify_and_extract`] repeats the same check
/// immediately before creation.
pub(crate) fn checked_absent_root(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::verification(
            "package extraction root must be an absolute path",
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        ContractError::verification("package extraction root must have one safe final path segment")
    })?;
    if !matches!(
        Path::new(name).components().next(),
        Some(Component::Normal(_))
    ) {
        return Err(ContractError::verification(
            "package extraction root must have one safe final path segment",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        ContractError::verification("package extraction root has no parent directory")
    })?;
    let parent = parent.canonicalize().map_err(|_| {
        ContractError::verification("package extraction parent cannot be canonicalized")
    })?;
    open_directory(&parent).map_err(|_| {
        ContractError::verification("package extraction parent is not a safe real directory")
    })?;
    let root = parent.join(name);
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(root),
        Ok(_) => Err(ContractError::verification(
            "package extraction root already exists and cannot be adopted",
        )),
        Err(_) => Err(ContractError::verification(
            "cannot safely inspect the package extraction root",
        )),
    }
}

fn create_fresh_root(path: &Path) -> Result<PathBuf, ContractError> {
    let root = checked_absent_root(path)?;
    fs::create_dir(&root).map_err(|_| {
        ContractError::verification("cannot create the fresh package extraction root")
    })?;
    open_directory(&root).map_err(|_| {
        ContractError::verification("fresh package extraction root is not a real directory")
    })?;
    Ok(root)
}

fn verify_extracted_tree(
    root: &Path,
    manifest: &ArosToolchainManifest,
) -> Result<(), ContractError> {
    let extracted_manifest = ArosToolchainManifest::load(root).map_err(|_| {
        ContractError::verification("extracted package manifest is invalid or unreadable")
    })?;
    if &extracted_manifest != manifest {
        return Err(ContractError::verification(
            "extracted package manifest differs from the verified package identity",
        ));
    }
    let (tree_sha256, entries) = toolchain_tree_inventory(root).map_err(|_| {
        ContractError::verification("cannot inventory the extracted package payload")
    })?;
    if tree_sha256 != manifest.tree_sha256 || entries != manifest.files {
        return Err(ContractError::verification(
            "extracted package payload differs from the verified package inventory",
        ));
    }
    Ok(())
}
