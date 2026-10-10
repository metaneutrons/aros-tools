//! Complete indexed build/compatibility byte joins, not execution admission.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt as _;

use super::release_readback::revalidate_release_builds_v2;
use super::{
    readback_release_builds_v2, ReleaseBuildLaneReadbackV2, ReleaseBuildReadbackRequestV2,
    ReleaseBuildReadbackV2,
};
use crate::compatibility::{
    readback_portable_native_compatibility, PortableNativeCompatibilityReadback,
    PortableNativeCompatibilityRequest,
};
use crate::release_index_v2::NativeReleaseArtifactV2;
use crate::release_index_v2_readback::validate_directory_path;
use crate::ContractError;

/// Complete selected build evidence and one independent compatibility selection
/// per indexed archive. No diagnostic subset is accepted.
///
/// Every expectation and raw export digest remains an independently selected
/// declaration. This request does not authenticate the owning jobs, original
/// environments, runtime, source or signatures.
#[derive(Debug)]
pub struct ReleaseCompatibilityReadbackRequestV2<'a> {
    /// Complete indexed A/B package/build/comparison byte selection.
    pub builds: &'a ReleaseBuildReadbackRequestV2,
    /// Exact index-derived archive keys and protected portable evidence roots.
    pub lanes: BTreeMap<String, PortableNativeCompatibilityRequest<'a>>,
}

/// Complete byte-consistent build and compatibility results.
///
/// This opaque result cannot authorize qualification, recovery or publication.
/// In particular, consistent receipts and ELF outputs do not prove two builds,
/// compatibility command execution, relocation execution or evidence origin.
#[derive(Debug)]
pub struct ReleaseCompatibilityReadbackV2 {
    builds: ReleaseBuildReadbackV2,
    lanes: Vec<ReleaseCompatibilityLaneReadbackV2>,
}

impl ReleaseCompatibilityReadbackV2 {
    /// Complete A/B byte read-back for the same selected indexed release.
    #[must_use]
    pub const fn builds(&self) -> &ReleaseBuildReadbackV2 {
        &self.builds
    }

    /// Complete compatibility closure in canonical indexed-asset order.
    #[must_use]
    pub fn lanes(&self) -> &[ReleaseCompatibilityLaneReadbackV2] {
        &self.lanes
    }
}

/// One compatibility closure bound to its actual measured indexed package.
#[derive(Debug)]
pub struct ReleaseCompatibilityLaneReadbackV2 {
    asset: String,
    compatibility: PortableNativeCompatibilityReadback,
}

impl ReleaseCompatibilityLaneReadbackV2 {
    /// Canonical index-derived archive basename, never receipt-selected.
    #[must_use]
    pub fn asset(&self) -> &str {
        &self.asset
    }

    /// Measured portable manifest, six-phase receipt and actual standalone ELFs.
    #[must_use]
    pub const fn compatibility(&self) -> &PortableNativeCompatibilityReadback {
        &self.compatibility
    }
}

/// Acquire every selected compatibility closure and bind it to complete builds.
///
/// First verifies all A/B exports, package members, retained comparisons, final
/// index/inputs/checksums and unchanged pre-attestation subjects. Each portable
/// expectation must equal its actual verified package and index-derived
/// source/compiler/host/profile selection, before any compatibility bytes are
/// accepted. Downloaded reports/logs/ELFs are then read through the existing
/// bounded portable validator. The whole build join and every compatibility
/// closure are observed again before returning; no partial result is returned.
///
/// All evidence roots must be canonical, physically distinct, nonoverlapping
/// and separate from package/final inventories and build evidence documents.
/// The operation writes nothing, executes no compiler and never opens original
/// runner roots. Callers exclusively own quiescent files: repeated observations
/// are not an atomic snapshot or a concurrent-writer lock.
///
/// # Errors
/// Returns AX0703 for incomplete selections, unsafe/reused roots, inconsistent
/// independent expectations or changed evidence, propagating package/build and
/// portable validation errors. External authentication and signatures remain
/// mandatory separate gates; success is not release admission.
pub fn readback_release_compatibility_v2(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
) -> Result<ReleaseCompatibilityReadbackV2, ContractError> {
    require_complete_selection(request)?;
    require_distinct_roots(request)?;
    let builds = readback_release_builds_v2(request.builds)?;
    let lanes = request
        .builds
        .packages
        .index
        .artifacts()
        .iter()
        .zip(builds.lanes())
        .map(|(artifact, build)| read_lane(request, artifact, build))
        .collect::<Result<Vec<_>, _>>()?;

    for ((artifact, build), lane) in request
        .builds
        .packages
        .index
        .artifacts()
        .iter()
        .zip(builds.lanes())
        .zip(&lanes)
    {
        let repeated = read_lane(request, artifact, build)?;
        if repeated.asset != lane.asset
            || repeated.compatibility.manifest_sha256() != lane.compatibility.manifest_sha256()
            || repeated.compatibility.receipt() != lane.compatibility.receipt()
            || repeated.compatibility.standalone() != lane.compatibility.standalone()
        {
            return Err(error(
                "release compatibility bytes changed during read-back",
            ));
        }
    }
    revalidate_release_builds_v2(request.builds, &builds)?;
    require_distinct_roots(request)?;
    Ok(ReleaseCompatibilityReadbackV2 { builds, lanes })
}

fn read_lane(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    artifact: &NativeReleaseArtifactV2,
    build: &ReleaseBuildLaneReadbackV2,
) -> Result<ReleaseCompatibilityLaneReadbackV2, ContractError> {
    let selected = &request.lanes[artifact.asset()];
    let expected = &selected.expected;
    let package = build.verified_package();
    let group = request
        .builds
        .packages
        .inputs
        .groups()
        .iter()
        .find(|group| group.id() == artifact.group_id())
        .ok_or_else(|| error("release compatibility lane has no selected input group"))?;
    let profile = group.profiles().select(artifact.target_profile())?;
    if build.asset() != artifact.asset()
        || expected.package.manifest != &package.manifest
        || expected.package.archive_sha256 != &package.archive_sha256
        || expected.package.archive_size != package.archive_size
        || expected.package.compiler != artifact.compiler()
        || expected.package.source_commit != artifact.source_commit()
        || expected.package.host != artifact.host()
    {
        return Err(error(
            "release compatibility expectation differs from its actual indexed package",
        ));
    }
    let expected_profile = expected.profiles.select(artifact.target_profile())?;
    if expected.profile.document_sha256() != profile.document_sha256()
        || expected_profile.document_sha256() != profile.document_sha256()
        || expected.profile.name() != profile.name()
        || expected.profile.target_triple() != profile.target_triple()
        || expected.profiles.upstream_commit() != group.profiles().upstream_commit()
        || expected.upstream_source_commit != group.profiles().upstream_commit()
    {
        return Err(error(
            "release compatibility profile or upstream source differs from its indexed input group",
        ));
    }
    Ok(ReleaseCompatibilityLaneReadbackV2 {
        asset: artifact.asset().into(),
        compatibility: readback_portable_native_compatibility(selected)?,
    })
}

fn require_complete_selection(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
) -> Result<(), ContractError> {
    let expected = request
        .builds
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
            "release compatibility selections must exactly cover the input-derived index",
        ));
    }
    Ok(())
}

fn require_distinct_roots(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
) -> Result<(), ContractError> {
    let mut roots = vec![request.builds.packages.directory.clone()];
    for build in request.builds.lanes.values() {
        roots.extend(build.builds.iter().map(|side| side.package_dir.clone()));
    }
    roots.extend(request.lanes.values().map(|lane| lane.directory.clone()));
    roots.sort();
    let mut physical = BTreeSet::new();
    for root in &roots {
        validate_directory_path(root)?;
        if root
            .canonicalize()
            .map_err(|_| error("cannot resolve release compatibility root"))?
            != *root
        {
            return Err(error(
                "release compatibility roots must be canonical and nonoverlapping",
            ));
        }
        let metadata = crate::filesystem::open_directory(root)
            .and_then(|file| file.metadata())
            .map_err(|_| error("cannot inspect no-follow release compatibility root identity"))?;
        if !physical.insert((metadata.dev(), metadata.ino())) {
            return Err(error(
                "release compatibility root identity is reused across selections",
            ));
        }
    }
    if roots.windows(2).any(|pair| pair[1].starts_with(&pair[0])) {
        return Err(error(
            "release compatibility roots must be canonical and nonoverlapping",
        ));
    }
    let documents = std::iter::once(&request.builds.subject_manifest).chain(
        request.builds.lanes.values().flat_map(|lane| {
            std::iter::once(&lane.comparison)
                .chain(lane.builds.iter().map(|side| &side.measurement))
        }),
    );
    for document in documents {
        if request
            .lanes
            .values()
            .any(|lane| document.starts_with(&lane.directory))
        {
            return Err(error(
                "release compatibility roots must exclude build evidence documents",
            ));
        }
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
