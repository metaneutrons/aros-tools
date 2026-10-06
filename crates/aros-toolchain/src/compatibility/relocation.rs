//! Independent package extraction roots used to verify relocation behavior.

use std::path::{Path, PathBuf};

use crate::filesystem::open_directory;
use crate::package_extract::{
    checked_absent_root, verify_and_extract, ExtractedPackage, PackageExtractionRequest,
};
use crate::package_verify::PackageVerificationRequest;
use crate::ContractError;

/// Inputs for the two independent roots required by a relocation probe.
///
/// Both roots read the same complete package contract independently. The
/// operation creates no work, source, engine, helper, tag or release material.
#[derive(Debug, Clone)]
pub struct TwoRootRelocationRequest {
    /// Exact complete package set that every root must reverify.
    pub verification: PackageVerificationRequest,
    /// First absent destination root.
    pub first_root: PathBuf,
    /// Second absent destination root.
    pub second_root: PathBuf,
}

/// Two independently reverified extracted payload roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoRootRelocation {
    /// First retained payload root.
    pub first: ExtractedPackage,
    /// Second retained payload root.
    pub second: ExtractedPackage,
}

/// Materialize two independent roots from one complete native package set.
///
/// The two destination identities are checked before either one is created.
/// Each extraction independently revalidates the complete four-member package
/// set, remeasures the selected archive and re-inventories its payload. The
/// operation refuses an overlap with each other or the package directory, an
/// existing destination, or a changed package. If either extraction begins and
/// later fails, all already-created roots remain available for diagnosis; it
/// never removes or adopts material.
///
/// # Errors
///
/// Returns AX0602 for an unsafe package or extraction root. It has no process,
/// network, cache, source-tree, tag or publication authority.
pub fn extract_two_roots(
    request: &TwoRootRelocationRequest,
) -> Result<TwoRootRelocation, ContractError> {
    let first_root = checked_absent_root(&request.first_root)?;
    let second_root = checked_absent_root(&request.second_root)?;
    if first_root == second_root {
        return Err(ContractError::verification(
            "compatibility relocation roots must have distinct output identities",
        ));
    }
    let package_directory = checked_verified_package_directory(&request.verification.package_dir)?;
    if first_root.starts_with(&package_directory) || second_root.starts_with(&package_directory) {
        return Err(ContractError::verification(
            "compatibility relocation root cannot be created inside the verified package directory",
        ));
    }
    let first = verify_and_extract(&PackageExtractionRequest {
        verification: request.verification.clone(),
        output_root: first_root,
    })?;
    let second = verify_and_extract(&PackageExtractionRequest {
        verification: request.verification.clone(),
        output_root: second_root,
    })?;
    if first.verified != second.verified {
        return Err(ContractError::verification(
            "compatibility relocation roots were not extracted from one measured package identity",
        ));
    }
    Ok(TwoRootRelocation { first, second })
}

fn checked_verified_package_directory(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::verification(
            "verified package directory must be an absolute directory",
        ));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ContractError::verification("verified package directory cannot be canonicalized")
    })?;
    open_directory(&canonical).map_err(|_| {
        ContractError::verification(
            "verified package directory does not resolve to a real directory without symlink ancestors",
        )
    })?;
    Ok(canonical)
}
