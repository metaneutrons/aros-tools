//! Bounded execution of an evidence-admitted packaging recovery.
//!
//! This module is deliberately local-only: it creates two fresh package sets
//! beneath caller-owned absent paths, verifies them, and compares every outer
//! member before returning. It cannot create tags, advance an index, contact a
//! forge, obtain credentials, or publish a release.

use std::fs;
use std::path::{Path, PathBuf};

use aros_common::Sha256Digest;

use crate::package::{package, PackageOutput, PackageRequest};
use crate::package_extract::{checked_absent_root, verify_and_extract, PackageExtractionRequest};
use crate::package_verify::{verify, PackageVerificationRequest, VerifiedPackage};
use crate::recovery::{evaluate_recovery, RecoveryDecision, RecoveryRequest, ReleaseAssetKind};
use crate::release_index::{compare_package_sets, NativeReleaseArtifact, NativeReleaseIndex};
use crate::ContractError;

/// Inputs to one two-output, packaging-only recovery execution.
///
/// The complete [`RecoveryRequest`] is retained here rather than accepting a
/// prior decision alone. This makes the execution boundary re-evaluate the
/// immutable evidence, remote handoff observation, and failure classification
/// immediately before it writes either local package directory.
#[derive(Debug, Clone)]
pub struct RepackageRequest {
    /// Complete isolated recovery evidence and observed handoff state.
    pub recovery: RecoveryRequest,
    /// First independent retained candidate and absent output directory.
    pub first: PackageRequest,
    /// Second independent retained candidate and absent output directory.
    pub second: PackageRequest,
}

/// Measured result of a two-output packaging recovery.
///
/// A caller cannot derive an index or publication operation from this value.
/// The protected workflow must separately construct and verify a fresh closed
/// release inventory after this local comparison has succeeded.
#[derive(Debug, Clone)]
pub struct RepackageOutput {
    /// The fresh identity admitted by the re-evaluated recovery policy.
    pub decision: RecoveryDecision,
    /// First atomically published local package set.
    pub first: PackageOutput,
    /// Second atomically published local package set.
    pub second: PackageOutput,
    /// Bounded read-back of the first package set.
    pub first_verified: VerifiedPackage,
    /// Bounded read-back of the second package set.
    pub second_verified: VerifiedPackage,
}

/// Inputs for recovery from one retained, fully verified package set.
///
/// A source producer run normally retains packages rather than the much larger
/// build candidates. This boundary verifies the retained package twice and
/// extracts it to two independently owned roots before handing the results to
/// the existing two-output recovery executor. It has no transport, tag,
/// index, credential, or publication capability.
#[derive(Debug, Clone)]
pub struct VerifiedPackageRepackageRequest {
    /// Complete recovery policy input, re-evaluated before any extraction.
    pub recovery: RecoveryRequest,
    /// Exact expected identity for the retained source package set.
    pub source: PackageVerificationRequest,
    /// Absent first owned extraction root.
    pub first_extraction_root: PathBuf,
    /// Absent second owned extraction root.
    pub second_extraction_root: PathBuf,
    /// Absent first output package-set directory.
    pub first_output_dir: PathBuf,
    /// Absent second output package-set directory.
    pub second_output_dir: PathBuf,
}

/// Result of re-packaging one retained source package into two new sets.
#[derive(Debug, Clone)]
pub struct VerifiedPackageRepackageOutput {
    /// The once-verified, evidence-bound retained source package identity.
    pub source: VerifiedPackage,
    /// The two independently re-packaged and compared output sets.
    pub repackaged: RepackageOutput,
}

/// Execute an admitted packaging recovery as two independent local packages.
///
/// The function first re-evaluates all recovery evidence, then proves that
/// both inputs refer to the same selected source-index lane and fresh release
/// identity. It refuses pre-existing, overlapping, or shared candidate/output
/// roots before any package output is created. Each output is independently
/// packaged and read back; both payload-tree digests must remain equal to the
/// original qualified lane and all four outer package members must be byte
/// identical before success is returned.
///
/// # Errors
///
/// Returns AX0901 unless recovery remains eligible or the retained candidate
/// identities differ from the original qualified release. Package, read-back,
/// comparison and filesystem failures retain their existing producer codes.
pub fn repackage(request: &RepackageRequest) -> Result<RepackageOutput, ContractError> {
    let decision = evaluate_recovery(&request.recovery)?;
    let release_id = match &decision {
        RecoveryDecision::Repackage { release_id, .. } => release_id,
        RecoveryDecision::ReplayCompatibility => {
            return Err(ContractError::recovery(
                "compatibility replay evidence has no packaging-recovery authority",
            ));
        }
    };

    validate_requests(request, release_id)?;
    let source_index =
        NativeReleaseIndex::parse(&request.recovery.release_index_bytes).map_err(|_| {
            ContractError::recovery(
                "recovery execution requires the measured qualified release index",
            )
        })?;
    let qualified = qualified_lane(&source_index, &request.first)?;

    let first = package(&request.first)?;
    let first_verified = verify(&verification_request(&request.first))?;
    validate_repackaged_lane(&first_verified, qualified)?;

    let second = package(&request.second)?;
    let second_verified = verify(&verification_request(&request.second))?;
    validate_repackaged_lane(&second_verified, qualified)?;

    compare_package_sets(&first.output_dir, &second.output_dir)?;
    Ok(RepackageOutput {
        decision,
        first,
        second,
        first_verified,
        second_verified,
    })
}

/// Re-package a retained source package twice under a fresh recovery identity.
///
/// The source package must be one exact member of the full isolated release
/// inventory named in the recovery request. It is verified in place and then
/// extracted twice to distinct, caller-owned absent roots. The existing
/// [`repackage`] boundary re-evaluates recovery eligibility, packages both
/// roots, verifies both outputs, and proves their outer members are
/// byte-identical.
///
/// # Errors
///
/// Returns AX0901 when recovery is no longer eligible or the retained package
/// is not the exact evidence-bound source asset. Verification and package
/// failures preserve their existing producer diagnostics.
pub fn repackage_verified_package(
    request: &VerifiedPackageRepackageRequest,
) -> Result<VerifiedPackageRepackageOutput, ContractError> {
    let decision = evaluate_recovery(&request.recovery)?;
    let release_id = match &decision {
        RecoveryDecision::Repackage { release_id, .. } => release_id.clone(),
        RecoveryDecision::ReplayCompatibility => {
            return Err(ContractError::recovery(
                "compatibility replay evidence has no packaging-recovery authority",
            ));
        }
    };
    if request.source.release_id != request.recovery.evidence.release.release_id {
        return Err(ContractError::recovery(
            "retained package release identity differs from recovery evidence",
        ));
    }
    validate_extraction_roots(request)?;

    let source = verify(&request.source)?;
    validate_source_archive(&request.recovery, &request.source, &source)?;
    let first = verify_and_extract(&PackageExtractionRequest {
        verification: request.source.clone(),
        output_root: request.first_extraction_root.clone(),
    })?;
    let second = verify_and_extract(&PackageExtractionRequest {
        verification: request.source.clone(),
        output_root: request.second_extraction_root.clone(),
    })?;
    if first.verified != source || second.verified != source {
        return Err(ContractError::recovery(
            "retained package changed between recovery verification and extraction",
        ));
    }

    let first_request = repackage_request_for(
        &request.source,
        first.root,
        request.first_output_dir.clone(),
        &release_id,
    );
    let second_request = repackage_request_for(
        &request.source,
        second.root,
        request.second_output_dir.clone(),
        &release_id,
    );
    let repackaged = repackage(&RepackageRequest {
        recovery: request.recovery.clone(),
        first: first_request,
        second: second_request,
    })?;
    if repackaged.decision != decision {
        return Err(ContractError::recovery(
            "recovery decision changed between retained-package extraction and repackage",
        ));
    }
    Ok(VerifiedPackageRepackageOutput { source, repackaged })
}

fn validate_extraction_roots(
    request: &VerifiedPackageRepackageRequest,
) -> Result<(), ContractError> {
    let first = checked_absent_root(&request.first_extraction_root)?;
    let second = checked_absent_root(&request.second_extraction_root)?;
    if first == second || first.starts_with(&second) || second.starts_with(&first) {
        return Err(ContractError::recovery(
            "recovery extraction roots must be distinct and non-overlapping",
        ));
    }
    Ok(())
}

fn validate_source_archive(
    recovery: &RecoveryRequest,
    source_request: &PackageVerificationRequest,
    source: &VerifiedPackage,
) -> Result<(), ContractError> {
    let asset = crate::package::canonical_asset_name(
        source_request.source_lock.version(),
        &source_request.host,
        source_request.profile.name(),
    )?;
    let expected = recovery
        .assets
        .iter()
        .find(|candidate| candidate.name == asset)
        .ok_or_else(|| {
            ContractError::recovery(
                "recovery evidence does not name the retained source package archive",
            )
        })?;
    if expected.kind != ReleaseAssetKind::Regular
        || expected.sha256 != source.archive_sha256
        || expected.size != source.archive_size
    {
        return Err(ContractError::recovery(
            "retained source package archive differs from the measured release inventory",
        ));
    }
    Ok(())
}

fn repackage_request_for(
    source: &PackageVerificationRequest,
    candidate_root: PathBuf,
    output_dir: PathBuf,
    release_id: &str,
) -> PackageRequest {
    PackageRequest {
        candidate_root,
        output_dir,
        release_id: release_id.into(),
        host: source.host.clone(),
        recipe: source.recipe.clone(),
        source_lock: source.source_lock.clone(),
        profile: source.profile.clone(),
        build_environment: source.build_environment.clone(),
        forbidden_prefixes: source.forbidden_prefixes.clone(),
    }
}

fn validate_requests(request: &RepackageRequest, release_id: &str) -> Result<(), ContractError> {
    let first = &request.first;
    let second = &request.second;
    if first.release_id != release_id || second.release_id != release_id {
        return Err(ContractError::recovery(
            "repackaged manifests must use exactly the fresh policy-admitted release identity",
        ));
    }
    if first.host != second.host
        || first.profile.name() != second.profile.name()
        || first.profile.target_triple() != second.profile.target_triple()
        || first.source_lock.version() != second.source_lock.version()
        || first.recipe.source().0 != second.recipe.source().0
        || first.recipe.producer().0 != second.recipe.producer().0
        || first.recipe.tools().0 != second.recipe.tools().0
        || first.build_environment != second.build_environment
        || first.forbidden_prefixes != second.forbidden_prefixes
    {
        return Err(ContractError::recovery(
            "independent repackage inputs do not describe one identical qualified lane",
        ));
    }
    let first_candidate = canonical_real_directory(&first.candidate_root, "first candidate")?;
    let second_candidate = canonical_real_directory(&second.candidate_root, "second candidate")?;
    if first_candidate == second_candidate
        || first_candidate.starts_with(&second_candidate)
        || second_candidate.starts_with(&first_candidate)
    {
        return Err(ContractError::recovery(
            "recovery requires two distinct, non-overlapping retained candidate roots",
        ));
    }
    let first_output = absent_output(&first.output_dir, "first package output")?;
    let second_output = absent_output(&second.output_dir, "second package output")?;
    if first_output == second_output
        || first_output.starts_with(&second_output)
        || second_output.starts_with(&first_output)
        || first_output.starts_with(&first_candidate)
        || first_output.starts_with(&second_candidate)
        || second_output.starts_with(&first_candidate)
        || second_output.starts_with(&second_candidate)
    {
        return Err(ContractError::recovery(
            "recovery package outputs must be distinct and outside retained candidate roots",
        ));
    }
    Ok(())
}

fn canonical_real_directory(path: &Path, role: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::recovery(format!(
            "{role} must be an absolute real directory"
        )));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::recovery(format!("{role} is inaccessible")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::recovery(format!(
            "{role} must be a real directory"
        )));
    }
    fs::canonicalize(path)
        .map_err(|_| ContractError::recovery(format!("{role} cannot be canonicalized")))
}

fn absent_output(path: &Path, role: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() || path.exists() {
        return Err(ContractError::recovery(format!(
            "{role} must be an absent absolute path"
        )));
    }
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ContractError::recovery(format!("{role} must have a basename")))?;
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::recovery(format!("{role} has no parent directory")))?;
    let parent = fs::canonicalize(parent)
        .map_err(|_| ContractError::recovery(format!("{role} parent cannot be canonicalized")))?;
    Ok(parent.join(name))
}

fn qualified_lane<'a>(
    index: &'a NativeReleaseIndex,
    request: &PackageRequest,
) -> Result<&'a NativeReleaseArtifact, ContractError> {
    if index.source_commit != request.recipe.source().0.as_str()
        || index.producer_commit != request.recipe.producer().0.as_str()
        || index.tools_commit != request.recipe.tools().0.as_str()
    {
        return Err(ContractError::recovery(
            "repackage recipe identities differ from the qualified release index",
        ));
    }
    index
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.enabled
                && artifact.host == request.host
                && artifact.target_profile == request.profile.name()
                && artifact.target_triple == request.profile.target_triple()
                && artifact.llvm_version == request.source_lock.version()
        })
        .ok_or_else(|| {
            ContractError::recovery(
                "repackage input does not select an enabled qualified release-index lane",
            )
        })
}

fn verification_request(package: &PackageRequest) -> PackageVerificationRequest {
    PackageVerificationRequest {
        package_dir: package.output_dir.clone(),
        release_id: package.release_id.clone(),
        host: package.host.clone(),
        recipe: package.recipe.clone(),
        source_lock: package.source_lock.clone(),
        profile: package.profile.clone(),
        build_environment: package.build_environment.clone(),
        forbidden_prefixes: package.forbidden_prefixes.clone(),
    }
}

fn validate_repackaged_lane(
    package: &VerifiedPackage,
    qualified: &NativeReleaseArtifact,
) -> Result<(), ContractError> {
    let expected = Sha256Digest::parse(&qualified.tree_sha256).map_err(|_| {
        ContractError::recovery("qualified release index contains an invalid payload tree digest")
    })?;
    if package.manifest.tree_sha256 != expected.to_string() {
        return Err(ContractError::recovery(
            "repackaged payload tree differs from the qualified release-index lane",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{absent_output, canonical_real_directory};

    #[test]
    fn roots_require_real_retained_candidates_and_absent_outputs() {
        let temporary = tempfile::tempdir().unwrap();
        let candidate = temporary.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        assert_eq!(
            canonical_real_directory(&candidate, "candidate").unwrap(),
            fs::canonicalize(&candidate).unwrap()
        );

        let output = temporary.path().join("fresh-output");
        assert_eq!(
            absent_output(&output, "output").unwrap(),
            fs::canonicalize(temporary.path())
                .unwrap()
                .join("fresh-output")
        );
        fs::create_dir(&output).unwrap();
        assert!(absent_output(&output, "output").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn retained_candidate_must_not_be_a_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        fs::create_dir(&target).unwrap();
        let linked = temporary.path().join("linked");
        symlink(&target, &linked).unwrap();
        assert!(canonical_real_directory(&linked, "candidate").is_err());
    }
}
