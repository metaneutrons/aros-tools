//! Same-host join of two complete candidates and their guarded packages.
//!
//! Disjoint local roots and byte equality do not prove independent execution.
//! This API has no serialization, authentication or publication authority.

use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};

use super::{FinishedCandidatePackage, FinishedCandidateReadback};
use crate::package::PackageFormat;
use crate::package_verify::{verify_with_format, PackageVerificationRequest};
use crate::release_checksums_v2::FinalChecksumsReadbackV2;
use crate::release_index::{compare_package_sets, PackageComparisonReport};
use crate::release_index_v2::{NativeReleaseArtifactV2, NativeReleaseIndexV2, INDEX_NAME};
use crate::release_inputs::ReleaseInputs;
use crate::ContractError;

/// Opaque local A/B join, retaining the live candidates and guarded package proofs.
///
/// The report's four members were measured against both complete packages.
/// Roots are disjoint and lane inputs agree, but neither property authenticates
/// execution independence. Callers retain exclusive ownership of every root.
#[derive(Debug)]
pub struct FinishedCandidateComparison<'a> {
    left_candidate: &'a FinishedCandidateReadback,
    left_package: &'a FinishedCandidatePackage,
    right_candidate: &'a FinishedCandidateReadback,
    right_package: &'a FinishedCandidatePackage,
    report: PackageComparisonReport,
}

impl FinishedCandidateComparison<'_> {
    /// Complete measured byte comparison; the unchanged schema-1 envelope.
    #[must_use]
    pub const fn report(&self) -> &PackageComparisonReport {
        &self.report
    }

    /// Finished candidate receipt digests in the explicitly selected A/B order.
    #[must_use]
    pub const fn candidate_receipt_digests(&self) -> [&Sha256Digest; 2] {
        [
            self.left_candidate.receipt_sha256(),
            self.right_candidate.receipt_sha256(),
        ]
    }

    /// Recheck candidates, receipt bytes and package members before using the join.
    ///
    /// # Errors
    /// Rejects changed, substituted, overlapping or inconsistent local evidence.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        require_disjoint_roots(
            self.left_candidate,
            self.left_package,
            self.right_candidate,
            self.right_package,
        )?;
        verify_side(self.left_candidate, self.left_package)?;
        verify_side(self.right_candidate, self.right_package)?;
        let measured = compare_package_sets(
            &self.left_package.output.output_dir,
            &self.right_package.output.output_dir,
        )?;
        if measured.package_set_sha256 != self.report.package_set_sha256
            || measured.members != self.report.members
        {
            return Err(error("finished A/B package bytes changed after comparison"));
        }
        self.left_candidate.revalidate()?;
        self.right_candidate.revalidate()
    }

    /// Join fresh local evidence to a selected V2 artifact and measured final files.
    ///
    /// The exact collection, selected group and index membership are checked here.
    /// This method does not refresh the final-checksum snapshot or verify a signer.
    ///
    /// # Errors
    /// Rejects changed local evidence, a different indexed lane or mismatched files.
    pub fn validate_against_checksums_v2(
        &self,
        inputs: &ReleaseInputs,
        index: &NativeReleaseIndexV2,
        artifact: &NativeReleaseArtifactV2,
        readback: &FinalChecksumsReadbackV2,
    ) -> Result<(), ContractError> {
        self.revalidate()?;
        if index.inputs_sha256() != inputs.collection_sha256() {
            return Err(error(
                "finished A/B index or artifact differs from the selected release inputs",
            ));
        }
        if index.producer_commit() != inputs.producer_commit()
            || index.tools_commit() != inputs.tools_commit()
            || !index.artifacts().contains(artifact)
        {
            return Err(error(
                "finished A/B index or artifact differs from the selected release inputs",
            ));
        }
        let index_bytes = index.to_json_bytes()?;
        if !readback.members().iter().any(|member| {
            member.name() == INDEX_NAME
                && member.sha256() == &sha256_bytes(&index_bytes)
                && usize::try_from(member.size()) == Ok(index_bytes.len())
        }) {
            return Err(error(
                "finished A/B final checksums do not bind the selected index bytes",
            ));
        }
        require_final_input_members(inputs, readback)?;
        let group = inputs
            .groups()
            .iter()
            .find(|group| group.id() == artifact.group_id())
            .ok_or_else(|| error("finished A/B indexed group is missing from release inputs"))?;
        for (candidate, package) in [
            (self.left_candidate, self.left_package),
            (self.right_candidate, self.right_package),
        ] {
            let record = &candidate.record;
            let manifest = &package.verified.manifest;
            if &record.identity.recipe_sha256 != group.recipe().sha256()
                || &record.source_lock_sha256 != group.source_lock().sha256()
                || &record.profiles_sha256 != group.profiles_reference().sha256()
                || &record.identity.producer_commit != inputs.producer_commit()
                || &record.identity.tools_commit != inputs.tools_commit()
                || manifest.release_id != index.release_id()
                || manifest.recipe_sha256 != group.recipe().sha256().as_str()
                || manifest.source_lock_sha256 != group.source_lock().sha256().as_str()
                || manifest.profiles_sha256 != group.profiles_reference().sha256().as_str()
                || artifact.host() != manifest.host
                || artifact.target_profile() != manifest.target_profile
                || artifact.target_triple() != manifest.target_triple
                || artifact.source_commit().as_str() != manifest.source_commit
                || Some(artifact.compiler()) != manifest.compiler.as_ref()
                || artifact.tree_sha256().as_str() != manifest.tree_sha256
            {
                return Err(error(
                    "finished A/B comparison differs from the selected indexed group or lane",
                ));
            }
        }
        self.report
            .validate_against_checksums_v2(artifact, readback)
    }
}

fn require_final_input_members(
    inputs: &ReleaseInputs,
    readback: &FinalChecksumsReadbackV2,
) -> Result<(), ContractError> {
    if !readback.members().iter().any(|member| {
        member.name() == "toolchain-release-inputs-v2.json"
            && member.sha256() == inputs.collection_sha256()
    }) {
        return Err(error(
            "finished A/B final files differ from release input documents",
        ));
    }
    for group in inputs.groups() {
        for (reference, bytes) in [
            (group.recipe_reference(), group.recipe_bytes()),
            (group.source_lock_reference(), group.source_lock_bytes()),
            (group.profiles_reference(), group.profiles_bytes()),
        ] {
            let member = readback
                .members()
                .iter()
                .find(|member| member.name() == reference.file())
                .ok_or_else(|| {
                    error("finished A/B final files differ from release input documents")
                })?;
            if member.sha256() != reference.sha256()
                || usize::try_from(member.size()) != Ok(bytes.len())
            {
                return Err(error(
                    "finished A/B final files differ from release input documents",
                ));
            }
        }
    }
    Ok(())
}

/// Compare two guarded V2 package sets bound to complete finished candidates.
///
/// Both sides must select the same recipe, source, profile, host and executor
/// observations. Their original work/output/package roots must be pairwise
/// disjoint across A and B, including canonical aliases and nested roots.
/// All four package members are freshly verified and compared. No file is written.
///
/// # Errors
/// Rejects inconsistent lane identities, root reuse, stale proofs or unequal sets.
pub fn compare_finished_candidate_packages<'a>(
    left_candidate: &'a FinishedCandidateReadback,
    left_package: &'a FinishedCandidatePackage,
    right_candidate: &'a FinishedCandidateReadback,
    right_package: &'a FinishedCandidatePackage,
) -> Result<FinishedCandidateComparison<'a>, ContractError> {
    let left = &left_candidate.record;
    let right = &right_candidate.record;
    if left.identity != right.identity
        || left.compiler != right.compiler
        || left.target_triple != right.target_triple
        || left.source_lock_sha256 != right.source_lock_sha256
        || left.profiles_sha256 != right.profiles_sha256
    {
        return Err(error(
            "finished A/B candidates select different lane inputs or executors",
        ));
    }
    require_disjoint_roots(left_candidate, left_package, right_candidate, right_package)?;
    let compared = compare_package_sets(
        &left_package.output.output_dir,
        &right_package.output.output_dir,
    )?;
    let report = PackageComparisonReport {
        schema: 1,
        operation: "compare".into(),
        byte_identical: true,
        package_set_sha256: compared.package_set_sha256,
        members: compared.members,
    };
    let joined = FinishedCandidateComparison {
        left_candidate,
        left_package,
        right_candidate,
        right_package,
        report,
    };
    joined.revalidate()?;
    Ok(joined)
}

pub(super) fn verify_side(
    candidate: &FinishedCandidateReadback,
    package: &FinishedCandidatePackage,
) -> Result<(), ContractError> {
    candidate.require_package_request(&package.request)?;
    if candidate.receipt_sha256() != &package.candidate_receipt_sha256
        || candidate.payload_sha256() != &package.candidate_payload_sha256
    {
        return Err(error(
            "finished A/B package was not bound to the selected candidate",
        ));
    }
    let selected = &package.request;
    let measured = verify_with_format(
        &PackageVerificationRequest {
            package_dir: package.output.output_dir.clone(),
            release_id: selected.release_id.clone(),
            host: selected.host.clone(),
            recipe: selected.recipe.clone(),
            source_lock: selected.source_lock.clone(),
            profile: selected.profile.clone(),
            build_environment: selected.build_environment.clone(),
            forbidden_prefixes: selected.forbidden_prefixes.clone(),
        },
        PackageFormat::CompilerFamilyV2,
    )?;
    if measured != package.verified {
        return Err(error(
            "finished A/B package differs from its guarded package read-back",
        ));
    }
    Ok(())
}

fn require_disjoint_roots(
    left: &FinishedCandidateReadback,
    left_package: &FinishedCandidatePackage,
    right: &FinishedCandidateReadback,
    right_package: &FinishedCandidatePackage,
) -> Result<(), ContractError> {
    let left_roots = roots(left, left_package)?;
    let right_roots = roots(right, right_package)?;
    if left_roots.iter().any(|left| {
        right_roots
            .iter()
            .any(|right| left.starts_with(right) || right.starts_with(left))
    }) {
        return Err(error(
            "finished A/B work, output and package roots must be disjoint",
        ));
    }
    Ok(())
}

fn roots(
    candidate: &FinishedCandidateReadback,
    package: &FinishedCandidatePackage,
) -> Result<[PathBuf; 3], ContractError> {
    let output = candidate
        .record
        .candidate_root
        .parent()
        .ok_or_else(|| error("finished A/B candidate has no output root"))?;
    Ok([
        canonical_directory(&candidate.work_dir)?,
        canonical_directory(output)?,
        canonical_directory(&package.output.output_dir)?,
    ])
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ContractError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error("finished A/B root cannot be inspected"))?;
    if !metadata.file_type().is_dir() {
        return Err(error("finished A/B root must be a real directory"));
    }
    path.canonicalize()
        .map_err(|_| error("finished A/B root cannot be resolved"))
}

fn error(message: &str) -> ContractError {
    ContractError::comparison(message)
}
