//! Local compiler-family recovery execution, never publication authority.
//!
//! The complete original qualification is reacquired before and after the
//! operation. A selected original package is extracted twice and packaged under
//! the admitted fresh identity. These are two packaging operations, not two new
//! compiler executions. External authentication remains a protected-caller duty.

use std::path::{Component, PathBuf};

use aros_common::publication::{
    filesystem_paths_overlap, validate_existing_directory_prefix_nofollow,
};

use crate::package::{package_with_format, PackageFormat, PackageRequest};
use crate::package_extract::{
    checked_absent_root, verify_and_extract_with_format, PackageExtractionRequest,
};
use crate::package_verify::{verify_with_format, PackageVerificationRequest};
use crate::recovery::RecoveryDecision;
use crate::recovery_v2::{readback_recovery_bytes_v2, RecoveryByteReadbackRequestV2};
use crate::release_index::{compare_package_sets, PackageSetComparison};
use crate::repackage::{RepackageOutput, VerifiedPackageRepackageOutput};
use crate::ContractError;

/// Explicit selected lane and four fresh, nonoverlapping local destinations.
///
/// No recipe, environment or compiler identity is supplied separately: they
/// are taken from the complete selected qualification input group. The caller
/// must exclusively own quiescent inputs and outputs and authenticate external
/// observations before relying on this local operation for any release action.
#[derive(Debug)]
pub struct VerifiedPackageRepackageRequestV2<'a, 'e> {
    /// Complete recovery request and original qualification byte closure.
    pub recovery: &'a RecoveryByteReadbackRequestV2<'a, 'e>,
    /// Exact index-derived archive basename; no first-match lane discovery.
    pub asset: &'a str,
    /// Two absent extraction roots with existing real, no-follow parents.
    pub extraction_roots: [PathBuf; 2],
    /// Two absent package directories, separate from all selected evidence.
    pub output_dirs: [PathBuf; 2],
}

/// Actual format-v2 package results and freshly measured four-member comparison.
#[derive(Debug)]
pub struct VerifiedPackageRepackageOutputV2 {
    /// Original verified package plus both independently verified new sets.
    pub packages: VerifiedPackageRepackageOutput,
    /// Exact byte-identical outer member comparison, not compiler A/B evidence.
    pub comparison: PackageSetComparison,
}

/// Repackage one qualified LLVM or GNU lane twice without modifying originals.
///
/// This does not execute a compiler, create an index, sign anything, contact a
/// forge or publish a release. All four destinations must be absent and outside
/// every original release/build/compatibility evidence path before any write.
/// Post-creation failures retain only the caller-selected new paths for diagnosis;
/// no existing path is removed, adopted, retried or overwritten.
///
/// # Errors
/// Rejects replay-only decisions, unselected lanes, unsafe/overlapping paths,
/// changed complete qualification or source package, and altered payloads.
/// Lower-layer package errors retain their original diagnostics.
pub fn repackage_verified_package_v2(
    request: &VerifiedPackageRepackageRequestV2<'_, '_>,
) -> Result<VerifiedPackageRepackageOutputV2, ContractError> {
    let admitted = readback_recovery_bytes_v2(request.recovery)?;
    let decision = admitted.decision().clone();
    let RecoveryDecision::Repackage { ref release_id, .. } = decision else {
        return Err(error(
            "compatibility replay cannot execute packaging recovery",
        ));
    };
    let complete = request.recovery.qualification.complete;
    let packages = &complete.builds.packages;
    let artifact = packages
        .index
        .artifacts()
        .iter()
        .find(|artifact| artifact.asset() == request.asset)
        .ok_or_else(|| error("recovery asset is not a selected indexed lane"))?;
    let group = packages
        .inputs
        .groups()
        .iter()
        .find(|group| group.id() == artifact.group_id())
        .ok_or_else(|| error("recovery lane has no bound input group"))?;
    let lane = complete
        .builds
        .lanes
        .get(request.asset)
        .ok_or_else(|| error("recovery lane has no complete build selection"))?;
    let qualified = admitted
        .qualification()
        .complete()
        .builds()
        .lanes()
        .iter()
        .find(|lane| lane.asset() == request.asset)
        .ok_or_else(|| error("recovery lane has no complete measured package"))?
        .verified_package()
        .clone();
    validate_destinations(request)?;
    let source_request = PackageVerificationRequest {
        package_dir: lane.builds[0].package_dir.clone(),
        release_id: packages.index.release_id().to_owned(),
        host: artifact.host().to_owned(),
        recipe: group.recipe().clone(),
        source_lock: group.source_lock().clone(),
        profile: group.profiles().select(artifact.target_profile())?.clone(),
        build_environment: packages.build_environments[request.asset].clone(),
        forbidden_prefixes: packages.forbidden_prefixes.clone(),
    };
    let format = PackageFormat::CompilerFamilyV2;
    let source = verify_with_format(&source_request, format)?;
    if source != qualified {
        return Err(error(
            "selected original package changed after qualification",
        ));
    }
    let extract = |root: &PathBuf| {
        verify_and_extract_with_format(
            &PackageExtractionRequest {
                verification: source_request.clone(),
                output_root: root.clone(),
            },
            format,
        )
    };
    let first_root = extract(&request.extraction_roots[0])?;
    let second_root = extract(&request.extraction_roots[1])?;
    if first_root.verified != source || second_root.verified != source {
        return Err(error("original package changed during recovery extraction"));
    }
    let mut expected_manifest = source.manifest.clone();
    expected_manifest.release_id.clone_from(release_id);
    let execute = |candidate_root, output_dir| {
        let package_request = PackageRequest {
            candidate_root,
            output_dir,
            release_id: release_id.clone(),
            host: source_request.host.clone(),
            recipe: source_request.recipe.clone(),
            source_lock: source_request.source_lock.clone(),
            profile: source_request.profile.clone(),
            build_environment: source_request.build_environment.clone(),
            forbidden_prefixes: source_request.forbidden_prefixes.clone(),
        };
        let output = package_with_format(&package_request, format)?;
        let mut verification = source_request.clone();
        verification.package_dir.clone_from(&output.output_dir);
        verification.release_id.clone_from(release_id);
        let verified = verify_with_format(&verification, format)?;
        if verified.manifest != expected_manifest {
            return Err(error(
                "recovery changed qualified payload or non-release metadata",
            ));
        }
        Ok((output, verified))
    };
    let (first, first_verified) = execute(first_root.root, request.output_dirs[0].clone())?;
    let (second, second_verified) = execute(second_root.root, request.output_dirs[1].clone())?;
    let comparison = compare_package_sets(&first.output_dir, &second.output_dir)?;
    let repeated = readback_recovery_bytes_v2(request.recovery)?;
    if repeated.decision() != &decision {
        return Err(error("recovery decision changed during packaging"));
    }
    let repeated_comparison = compare_package_sets(&first.output_dir, &second.output_dir)?;
    if repeated_comparison.members != comparison.members
        || repeated_comparison.package_set_sha256 != comparison.package_set_sha256
    {
        return Err(error(
            "recovered package bytes changed during qualification revalidation",
        ));
    }
    Ok(VerifiedPackageRepackageOutputV2 {
        packages: VerifiedPackageRepackageOutput {
            source,
            repackaged: RepackageOutput {
                decision,
                first,
                second,
                first_verified,
                second_verified,
            },
        },
        comparison,
    })
}

fn validate_destinations(
    request: &VerifiedPackageRepackageRequestV2<'_, '_>,
) -> Result<(), ContractError> {
    let destinations = request
        .extraction_roots
        .iter()
        .chain(&request.output_dirs)
        .map(|path| {
            if path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                return Err(error(
                    "recovery destinations must have normalized absolute paths",
                ));
            }
            let parent = path
                .parent()
                .ok_or_else(|| error("recovery destination has no parent"))?;
            validate_existing_directory_prefix_nofollow(parent)
                .map_err(|_| error("recovery destination parent has a symlink component"))?;
            checked_absent_root(path)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let complete = request.recovery.qualification.complete;
    let protected = std::iter::once(complete.builds.packages.directory.as_path())
        .chain(std::iter::once(complete.builds.subject_manifest.as_path()))
        .chain(complete.builds.lanes.values().flat_map(|lane| {
            std::iter::once(lane.comparison.as_path()).chain(
                lane.builds
                    .iter()
                    .flat_map(|side| [side.package_dir.as_path(), side.measurement.as_path()]),
            )
        }))
        .chain(complete.lanes.values().map(|lane| lane.directory.as_path()))
        .map(|path| {
            path.canonicalize()
                .map_err(|_| error("original recovery evidence path is inaccessible"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (position, destination) in destinations.iter().enumerate() {
        for path in destinations[..position].iter().chain(&protected) {
            if filesystem_paths_overlap(destination, path)
                .map_err(|_| error("cannot establish original recovery evidence ancestry"))?
            {
                return Err(error(
                    "recovery destinations overlap each other or original evidence",
                ));
            }
        }
        for prefix in &complete.builds.packages.forbidden_prefixes {
            if filesystem_paths_overlap(destination, prefix)
                .map_err(|_| error("cannot establish forbidden build-root ancestry"))?
            {
                return Err(error(
                    "recovery destinations overlap a forbidden build root",
                ));
            }
        }
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}
