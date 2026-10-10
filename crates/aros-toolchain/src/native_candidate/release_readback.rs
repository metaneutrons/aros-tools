//! Complete input-derived A/B byte read-back, not execution authentication.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use aros_common::{
    measure_regular_file_bounded, open_regular_file_nofollow, sha256_bytes, Sha256Digest,
};

use super::{readback_finished_package_measurement, PortableFinishedPackageRequest};
use crate::package_verify::PackageVerificationRequest;
use crate::package_verify::VerifiedPackage;
use crate::plan::Identity;
use crate::release_attestation_manifest_v2::{
    verify_attestation_manifest_v2, AttestationManifestStageV2,
};
use crate::release_checksums_v2::{verify_final_checksums_v2, FinalChecksumsReadbackV2};
use crate::release_index::{compare_package_sets, PackageComparisonReport};
use crate::release_index_v2::NativeReleaseArtifactV2;
use crate::release_index_v2_readback::{
    validate_directory_path, IndexedPackageReadbackRequestV2, MAX_METADATA_BYTES,
};
use crate::ContractError;

/// Independently selected bytes and executor for one owning-host build export.
///
/// Selection must come from separately verified job/artifact evidence. These
/// fields are not an authentication API; do not derive them from the export.
#[derive(Debug, Clone)]
pub struct ReleaseBuildSideRequestV2 {
    /// Downloaded closed four-member package directory, distinct for A and B.
    pub package_dir: PathBuf,
    /// Downloaded owning-host export; original host roots need not exist here.
    pub measurement: PathBuf,
    /// SHA-256 of exact externally selected export bytes.
    pub measurement_sha256: Sha256Digest,
    /// Independently selected lane and native executor observation.
    pub identity: Identity,
}

/// Exactly two selected builds and one retained four-member comparison.
#[derive(Debug, Clone)]
pub struct ReleaseBuildLaneRequestV2 {
    /// Explicit A/B order; directory separation alone proves no execution.
    pub builds: [ReleaseBuildSideRequestV2; 2],
    /// Downloaded bounded, closed comparison report.
    pub comparison: PathBuf,
    /// Independently selected raw report digest, not its package-set digest.
    pub comparison_sha256: Sha256Digest,
}

/// Complete V2 release inputs, final files and independently selected build bytes.
#[derive(Debug, Clone)]
pub struct ReleaseBuildReadbackRequestV2 {
    /// Selected inputs/index and independent per-lane environment expectations.
    pub packages: IndexedPackageReadbackRequestV2,
    /// Retained pre-attestation subject list, outside the flat final inventory.
    pub subject_manifest: PathBuf,
    /// Independently retained digest of that exact pre-attestation list.
    pub subject_manifest_sha256: Sha256Digest,
    /// Exactly one entry per input-derived archive; no diagnostic subset.
    pub lanes: BTreeMap<String, ReleaseBuildLaneRequestV2>,
}

/// Complete byte-consistent build evidence, never release qualification.
///
/// Construction verifies every selected lane and the final package/input/subject
/// closure. It does not authenticate jobs/artifacts, prove two executions,
/// remeasure original raw trees, verify signatures or check compatibility. The
/// caller owns quiescent files; no lock or concurrent-writer snapshot is taken.
#[derive(Debug)]
pub struct ReleaseBuildReadbackV2 {
    lanes: Vec<ReleaseBuildLaneReadbackV2>,
    checksums: FinalChecksumsReadbackV2,
    subject_manifest_sha256: Sha256Digest,
}

impl ReleaseBuildReadbackV2 {
    /// Every indexed lane in canonical archive-name order.
    #[must_use]
    pub fn lanes(&self) -> &[ReleaseBuildLaneReadbackV2] {
        &self.lanes
    }

    /// Exact final checksum document measured during this read-back.
    #[must_use]
    pub const fn checksums_sha256(&self) -> &Sha256Digest {
        self.checksums.checksums_sha256()
    }

    /// Retained subject list joined unchanged to final package/input bytes.
    #[must_use]
    pub const fn subject_manifest_sha256(&self) -> &Sha256Digest {
        &self.subject_manifest_sha256
    }
}

/// Immutable measurements for one complete selected A/B lane.
#[derive(Debug)]
pub struct ReleaseBuildLaneReadbackV2 {
    asset: String,
    measurements: [Sha256Digest; 2],
    build_results: [Sha256Digest; 2],
    finished_receipts: [Sha256Digest; 2],
    comparison_sha256: Sha256Digest,
    report: PackageComparisonReport,
    package: VerifiedPackage,
}

impl ReleaseBuildLaneReadbackV2 {
    /// Actual verified A package, byte-joined to B and the final indexed set.
    #[must_use]
    pub const fn verified_package(&self) -> &VerifiedPackage {
        &self.package
    }

    /// Canonical index-derived archive basename.
    #[must_use]
    pub fn asset(&self) -> &str {
        &self.asset
    }

    /// Raw export digests in selected A/B order, not authentication.
    #[must_use]
    pub const fn measurement_sha256(&self) -> [&Sha256Digest; 2] {
        self.measurements.each_ref()
    }

    /// Raw build-result digests in selected A/B order.
    #[must_use]
    pub const fn build_result_sha256(&self) -> [&Sha256Digest; 2] {
        self.build_results.each_ref()
    }

    /// Finished record self-digests; neither raw file hashes nor execution proof.
    #[must_use]
    pub const fn finished_receipt_sha256(&self) -> [&Sha256Digest; 2] {
        self.finished_receipts.each_ref()
    }

    /// Digest of exact acquired comparison-report bytes.
    #[must_use]
    pub const fn comparison_sha256(&self) -> &Sha256Digest {
        &self.comparison_sha256
    }
}

/// Acquire every A/B export and join it to the complete selected final release.
///
/// Every package is verified against input-derived recipe/lock/profile/host and
/// independently supplied environment/executor values. Both four-member sets
/// are freshly byte-compared, joined to a closed externally selected report,
/// then to final checksums and the unchanged pre-attestation subjects. Bounded
/// no-follow reads and repeated byte observations reject observed mutations.
/// The operation writes nothing and returns no partial result.
///
/// # Errors
/// Rejects incomplete/extra lanes, unsafe or reused collector paths, wrong
/// selections, malformed exports/reports, differing packages or final files.
pub fn readback_release_builds_v2(
    request: &ReleaseBuildReadbackRequestV2,
) -> Result<ReleaseBuildReadbackV2, ContractError> {
    require_complete_selection(request)?;
    require_distinct_paths(request)?;
    let initial_subjects = verify_attestation_manifest_v2(
        &request.packages,
        &request.subject_manifest,
        AttestationManifestStageV2::Final,
    )?;
    if initial_subjects.sha256() != &request.subject_manifest_sha256 {
        return Err(error(
            "release build subjects differ from the retained pre-attestation bytes",
        ));
    }
    let checksums =
        verify_final_checksums_v2(&request.packages.directory, &request.packages.index)?;
    let lanes = request
        .packages
        .index
        .artifacts()
        .iter()
        .map(|artifact| read_lane(request, artifact, &checksums))
        .collect::<Result<Vec<_>, _>>()?;

    // Reobserve the original subject list and final inventory after all lane
    // reads, without pretending to hold an atomic filesystem snapshot.
    let final_subjects = verify_attestation_manifest_v2(
        &request.packages,
        &request.subject_manifest,
        AttestationManifestStageV2::Final,
    )?;
    let final_checksums =
        verify_final_checksums_v2(&request.packages.directory, &request.packages.index)?;
    if final_subjects.sha256() != initial_subjects.sha256()
        || final_subjects.members() != initial_subjects.members()
        || final_checksums.checksums_sha256() != checksums.checksums_sha256()
        || final_checksums.members() != checksums.members()
    {
        return Err(error("release build final bytes changed during read-back"));
    }
    for lane in &lanes {
        let selected = &request.lanes[lane.asset()];
        read_selected(&selected.comparison, &selected.comparison_sha256)?;
        for side in &selected.builds {
            read_selected(&side.measurement, &side.measurement_sha256)?;
        }
        let measured = compare_package_sets(
            &selected.builds[0].package_dir,
            &selected.builds[1].package_dir,
        )?;
        if measured.package_set_sha256 != lane.report.package_set_sha256
            || measured.members != lane.report.members
        {
            return Err(error(
                "release build package bytes changed during read-back",
            ));
        }
    }
    require_distinct_paths(request)?;
    Ok(ReleaseBuildReadbackV2 {
        lanes,
        checksums,
        subject_manifest_sha256: final_subjects.sha256().clone(),
    })
}

// Reobserve exact bytes already parsed by the complete factory. This internal
// join does not accept a caller-created proof or reuse a claim-only package.
// Unchanged archive/member hashes preserve the earlier full archive validation;
// no repeated decompression is needed merely to check for changed bytes.
pub(super) fn revalidate_release_builds_v2(
    request: &ReleaseBuildReadbackRequestV2,
    readback: &ReleaseBuildReadbackV2,
) -> Result<(), ContractError> {
    read_selected(&request.subject_manifest, &readback.subject_manifest_sha256)?;
    let checksums =
        verify_final_checksums_v2(&request.packages.directory, &request.packages.index)?;
    if checksums.checksums_sha256() != readback.checksums.checksums_sha256()
        || checksums.members() != readback.checksums.members()
    {
        return Err(error("release build final bytes changed during read-back"));
    }
    for lane in &readback.lanes {
        let selected = &request.lanes[lane.asset()];
        read_selected(&selected.comparison, &lane.comparison_sha256)?;
        for (side, digest) in selected.builds.iter().zip(&lane.measurements) {
            read_selected(&side.measurement, digest)?;
        }
        let measured = compare_package_sets(
            &selected.builds[0].package_dir,
            &selected.builds[1].package_dir,
        )?;
        if measured.package_set_sha256 != lane.report.package_set_sha256
            || measured.members != lane.report.members
        {
            return Err(error(
                "release build package bytes changed during read-back",
            ));
        }
    }
    require_distinct_paths(request)
}

fn read_lane(
    request: &ReleaseBuildReadbackRequestV2,
    artifact: &NativeReleaseArtifactV2,
    checksums: &FinalChecksumsReadbackV2,
) -> Result<ReleaseBuildLaneReadbackV2, ContractError> {
    let selected = &request.lanes[artifact.asset()];
    let group = request
        .packages
        .inputs
        .groups()
        .iter()
        .find(|group| group.id() == artifact.group_id())
        .ok_or_else(|| error("release build lane has no selected input group"))?;
    let profile = group.profiles().select(artifact.target_profile())?;
    let environment = request
        .packages
        .build_environments
        .get(artifact.asset())
        .ok_or_else(|| error("release build lane lacks an independent environment"))?;
    let mut sides = Vec::with_capacity(2);
    for side in &selected.builds {
        let package = PackageVerificationRequest {
            package_dir: side.package_dir.clone(),
            release_id: request.packages.index.release_id().into(),
            host: artifact.host().into(),
            recipe: group.recipe().clone(),
            source_lock: group.source_lock().clone(),
            profile: profile.clone(),
            build_environment: environment.clone(),
            forbidden_prefixes: request.packages.forbidden_prefixes.clone(),
        };
        sides.push(readback_finished_package_measurement(
            &PortableFinishedPackageRequest {
                measurement: &side.measurement,
                measurement_sha256: &side.measurement_sha256,
                identity: &side.identity,
                package: &package,
            },
        )?);
    }
    let report_bytes = read_selected(&selected.comparison, &selected.comparison_sha256)?;
    let report = PackageComparisonReport::parse(&report_bytes)?;
    let measured = compare_package_sets(
        &selected.builds[0].package_dir,
        &selected.builds[1].package_dir,
    )?;
    if measured.package_set_sha256 != report.package_set_sha256
        || measured.members != report.members
        || sides.iter().any(|side| side.package_members() != &measured)
    {
        return Err(error(
            "release build comparison differs from measured A/B package bytes",
        ));
    }
    report.validate_against_checksums_v2(artifact, checksums)?;
    Ok(ReleaseBuildLaneReadbackV2 {
        asset: artifact.asset().into(),
        measurements: std::array::from_fn(|i| sides[i].measurement_sha256().clone()),
        build_results: std::array::from_fn(|i| sides[i].build_result_sha256().clone()),
        finished_receipts: std::array::from_fn(|i| sides[i].finished_receipt_sha256().clone()),
        comparison_sha256: sha256_bytes(&report_bytes),
        report,
        package: sides[0].verified_package().clone(),
    })
}

fn require_complete_selection(
    request: &ReleaseBuildReadbackRequestV2,
) -> Result<(), ContractError> {
    let expected = request
        .packages
        .index
        .artifacts()
        .iter()
        .map(NativeReleaseArtifactV2::asset)
        .collect::<BTreeSet<_>>();
    let selected = request
        .lanes
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if expected != selected {
        return Err(error(
            "release build selections must exactly cover the input-derived index",
        ));
    }
    Ok(())
}

fn require_distinct_paths(request: &ReleaseBuildReadbackRequestV2) -> Result<(), ContractError> {
    let mut directories = vec![request.packages.directory.clone()];
    let mut documents = BTreeSet::from([request.subject_manifest.clone()]);
    for lane in request.lanes.values() {
        if !documents.insert(lane.comparison.clone()) {
            return Err(error(
                "release build evidence file is reused across selections",
            ));
        }
        for side in &lane.builds {
            if !documents.insert(side.measurement.clone()) {
                return Err(error(
                    "release build evidence file is reused across selections",
                ));
            }
            directories.push(side.package_dir.clone());
        }
    }
    directories.sort();
    let mut directory_identities = Vec::with_capacity(directories.len());
    for directory in &directories {
        validate_directory_path(directory)?;
        let canonical = directory
            .canonicalize()
            .map_err(|_| error("cannot resolve release build directory"))?;
        if &canonical != directory {
            return Err(error(
                "release build package directories must be canonical and nonoverlapping",
            ));
        }
        let metadata = crate::filesystem::open_directory(directory)
            .and_then(|file| file.metadata())
            .map_err(|_| error("cannot inspect no-follow release build directory identity"))?;
        directory_identities.push((metadata.dev(), metadata.ino()));
    }
    if directories
        .windows(2)
        .any(|pair| pair[1].starts_with(&pair[0]))
    {
        return Err(error(
            "release build package directories must be canonical and nonoverlapping",
        ));
    }
    require_unique_directory_identities(&directory_identities)?;
    let directory_set = directories
        .iter()
        .map(PathBuf::as_path)
        .collect::<BTreeSet<_>>();
    let mut file_identities = BTreeSet::new();
    for document in &documents {
        let parent = document
            .parent()
            .ok_or_else(|| error("release build evidence file has no parent"))?;
        validate_directory_path(parent)?;
        if document.file_name().is_none()
            || document
                .ancestors()
                .any(|ancestor| directory_set.contains(ancestor))
        {
            return Err(error(
                "release build evidence must remain outside package and final inventories",
            ));
        }
        let file = open_regular_file_nofollow(document)
            .map_err(|_| error("release build evidence must be a no-follow regular file"))?;
        let metadata = file
            .metadata()
            .map_err(|_| error("cannot inspect release build evidence identity"))?;
        if metadata.nlink() != 1 {
            return Err(error(
                "release build evidence must be singly linked regular files",
            ));
        }
        if !file_identities.insert((metadata.dev(), metadata.ino())) {
            return Err(error(
                "release build evidence file identity is reused across selections",
            ));
        }
    }
    Ok(())
}

fn require_unique_directory_identities(identities: &[(u64, u64)]) -> Result<(), ContractError> {
    let selected = identities.iter().collect::<BTreeSet<_>>();
    if selected.len() != identities.len() {
        return Err(error(
            "release build package directory identity is reused across selections",
        ));
    }
    Ok(())
}

fn read_selected(path: &Path, expected: &Sha256Digest) -> Result<Vec<u8>, ContractError> {
    let (_, bytes) = measure_regular_file_bounded(path, MAX_METADATA_BYTES)
        .map_err(|_| error("release build evidence is unsafe or exceeds its byte bound"))?
        .ok_or_else(|| error("release build evidence changed while read"))?;
    if sha256_bytes(&bytes) != *expected {
        return Err(error(
            "release build evidence differs from independently selected raw bytes",
        ));
    }
    Ok(bytes)
}

fn error(message: &str) -> ContractError {
    ContractError::comparison(message)
}

#[cfg(test)]
mod tests {
    use super::require_unique_directory_identities;

    #[test]
    fn directory_aliases_are_rejected_even_with_distinct_path_selections() {
        let error = require_unique_directory_identities(&[(1, 7), (1, 8), (1, 7)]).unwrap_err();
        assert!(error.to_string().contains("directory identity is reused"));
    }

    #[test]
    fn distinct_directory_devices_and_inodes_are_admitted() {
        assert!(require_unique_directory_identities(&[(1, 7), (1, 8), (2, 7)]).is_ok());
    }
}
