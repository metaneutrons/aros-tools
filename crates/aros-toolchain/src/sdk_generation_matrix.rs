//! Complete indexed SDK generation byte joins, not authenticated admission.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::native_candidate::{
    readback_release_compatibility_v2, ReleaseCompatibilityReadbackRequestV2,
    ReleaseCompatibilityReadbackV2,
};
use crate::sdk_generation::{
    readback_generation, SdkGenerationReadback, SdkGenerationReadbackRequest,
};
use crate::sdk_generation_lanes::ExpectedSdkGenerationLane;
use crate::ContractError;

/// Complete compiler/release selections and every independently selected SDK
/// generation. These declarations do not authenticate their owning jobs.
#[derive(Debug)]
pub struct SdkGenerationMatrixReadbackRequest<'a, 'e> {
    /// Complete indexed compiler A/B, comparison and compatibility closure.
    pub complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    /// Exact input-derived SDK/compiler-profile/host keys; no subset is valid.
    pub generations: BTreeMap<ExpectedSdkGenerationLane, SdkGenerationReadbackRequest<'e>>,
}

/// Opaque complete compiler and SDK byte join. No public constructor or
/// deserializer exists. This is not execution, origin or release authority.
#[derive(Debug)]
pub struct SdkGenerationMatrixReadback {
    complete: ReleaseCompatibilityReadbackV2,
    generations: Vec<IndexedSdkGenerationReadback>,
}

impl SdkGenerationMatrixReadback {
    /// Reacquired complete indexed compiler and package bytes.
    #[must_use]
    pub const fn complete(&self) -> &ReleaseCompatibilityReadbackV2 {
        &self.complete
    }

    /// SDK generation joins in canonical input-derived tuple order.
    #[must_use]
    pub fn generations(&self) -> &[IndexedSdkGenerationReadback] {
        &self.generations
    }
}

/// One complete generation whose compiler and SDK equal indexed measurements.
#[derive(Debug)]
pub struct IndexedSdkGenerationReadback {
    lane: ExpectedSdkGenerationLane,
    generation: SdkGenerationReadback,
}

impl IndexedSdkGenerationReadback {
    /// Independently input-derived product/compiler-profile/host tuple.
    #[must_use]
    pub const fn lane(&self) -> &ExpectedSdkGenerationLane {
        &self.lane
    }

    /// Reacquired SDK package, record and portable compatibility measurements.
    #[must_use]
    pub const fn generation(&self) -> &SdkGenerationReadback {
        &self.generation
    }
}

/// Read every selected SDK generation and join it to a complete indexed release.
///
/// Coverage is derived solely from validated release inputs. Each generation's
/// compiler expectations must match the actual indexed A/B compiler package;
/// every host's measured SDK must equal the same canonical indexed SDK package.
/// Generation records and host-bound compatibility receipts need not be equal.
/// All SDK roots are canonical and physically nonoverlapping with other selected
/// roots/documents. The complete release and all SDK closures are observed again
/// before success. Callers own quiescent paths; this is not an atomic snapshot.
/// No compiler executes, and no caller-created opaque proof is accepted.
///
/// # Errors
/// Rejects incomplete tuple coverage, substituted input/compiler/SDK selections,
/// aliased roots and changed generation/package/compatibility bytes. Complete
/// cryptographic owning-job authentication remains an external mandatory gate.
/// Existing SDK qualification/admission rejection is not changed by this API.
pub fn readback_sdk_generation_matrix(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
) -> Result<SdkGenerationMatrixReadback, ContractError> {
    require_complete_selection(request)?;
    require_distinct_roots(request)?;
    let complete = readback_release_compatibility_v2(request.complete)?;
    let generations = read_generations(request, &complete)?;
    let repeated_complete = readback_release_compatibility_v2(request.complete)?;
    let repeated_generations = read_generations(request, &repeated_complete)?;
    if !same_complete_bytes(&complete, &repeated_complete)
        || generations.len() != repeated_generations.len()
        || !generations
            .iter()
            .zip(&repeated_generations)
            .all(|(left, right)| {
                left.lane == right.lane
                    && left.generation.record_sha256() == right.generation.record_sha256()
                    && left.generation.package() == right.generation.package()
                    && left.generation.compatibility_manifest_sha256()
                        == right.generation.compatibility_manifest_sha256()
                    && left.generation.compatibility_receipt_sha256()
                        == right.generation.compatibility_receipt_sha256()
                    && left.generation.sdk_link_receipt_sha256()
                        == right.generation.sdk_link_receipt_sha256()
            })
    {
        return Err(error("SDK generation matrix bytes changed during readback"));
    }
    require_complete_selection(request)?;
    require_distinct_roots(request)?;
    Ok(SdkGenerationMatrixReadback {
        complete: repeated_complete,
        generations: repeated_generations,
    })
}

fn same_complete_bytes(
    left: &ReleaseCompatibilityReadbackV2,
    right: &ReleaseCompatibilityReadbackV2,
) -> bool {
    let a = left.builds();
    let b = right.builds();
    a.checksums_sha256() == b.checksums_sha256()
        && a.subject_manifest_sha256() == b.subject_manifest_sha256()
        && a.provenance_sha256() == b.provenance_sha256()
        && a.lanes().len() == b.lanes().len()
        && a.lanes().iter().zip(b.lanes()).all(|(a, b)| {
            a.asset() == b.asset()
                && a.verified_package() == b.verified_package()
                && a.measurement_sha256() == b.measurement_sha256()
                && a.build_result_sha256() == b.build_result_sha256()
                && a.finished_receipt_sha256() == b.finished_receipt_sha256()
                && a.comparison_sha256() == b.comparison_sha256()
        })
        && a.sdk_packages().len() == b.sdk_packages().len()
        && a.sdk_packages()
            .iter()
            .zip(b.sdk_packages())
            .all(|(a, b)| a.lock() == b.lock() && a.package() == b.package())
        && left.lanes().len() == right.lanes().len()
        && left.lanes().iter().zip(right.lanes()).all(|(a, b)| {
            a.asset() == b.asset()
                && a.compatibility().manifest_sha256() == b.compatibility().manifest_sha256()
                && a.compatibility().receipt() == b.compatibility().receipt()
                && a.compatibility().standalone() == b.compatibility().standalone()
        })
}

fn require_complete_selection(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
) -> Result<(), ContractError> {
    let inputs = &request.complete.builds.packages.inputs;
    let expected = inputs.expected_sdk_generation_lanes();
    if request.generations.len() != expected.len()
        || !request.generations.keys().eq(expected.iter())
    {
        return Err(error(
            "SDK generation matrix lacks exact input-derived host/profile coverage",
        ));
    }
    for (lane, selected) in &request.generations {
        if selected.inputs.collection_sha256() != inputs.collection_sha256()
            || selected.sdk_id != lane.sdk_id()
            || selected.compiler_group_id != lane.group_id()
            || selected.compatibility.expected.profile.name() != lane.profile()
            || selected.compatibility.expected.package.host != lane.host()
        {
            return Err(error(
                "SDK generation matrix selectors differ from their input-derived tuple",
            ));
        }
    }
    Ok(())
}

fn read_generations(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
    complete: &ReleaseCompatibilityReadbackV2,
) -> Result<Vec<IndexedSdkGenerationReadback>, ContractError> {
    let index = &request.complete.builds.packages.index;
    request
        .generations
        .iter()
        .map(|(lane, selected)| {
            let artifact = index
                .artifacts()
                .iter()
                .find(|artifact| {
                    (
                        artifact.group_id(),
                        artifact.target_profile(),
                        artifact.host(),
                    ) == (lane.group_id(), lane.profile(), lane.host())
                })
                .ok_or_else(|| error("SDK generation matrix has no indexed compiler lane"))?;
            let compiler = complete
                .builds()
                .lanes()
                .iter()
                .find(|build| build.asset() == artifact.asset())
                .ok_or_else(|| error("SDK generation matrix lacks its measured compiler package"))?
                .verified_package();
            let expected = &selected.compatibility.expected.package;
            if expected.manifest != &compiler.manifest
                || expected.archive_sha256 != &compiler.archive_sha256
                || expected.archive_size != compiler.archive_size
                || expected.compiler != artifact.compiler()
                || expected.source_commit != artifact.source_commit()
            {
                return Err(error(
                    "SDK generation compiler differs from the measured indexed package",
                ));
            }
            let indexed_sdk = complete
                .builds()
                .sdk_packages()
                .iter()
                .find(|sdk| sdk.lock().identity.sdk_id == lane.sdk_id())
                .ok_or_else(|| {
                    error("SDK generation matrix lacks its measured indexed SDK package")
                })?;
            let generation = readback_generation(selected)?;
            require_same_sdk_package(generation.package(), indexed_sdk.package())?;
            Ok(IndexedSdkGenerationReadback {
                lane: lane.clone(),
                generation,
            })
        })
        .collect()
}

fn require_same_sdk_package(
    generated: &crate::sdk_package::VerifiedSdkPackage,
    indexed: &crate::sdk_package::VerifiedSdkPackage,
) -> Result<(), ContractError> {
    if generated != indexed {
        return Err(error(
            "SDK generation host package differs from the canonical indexed SDK bytes",
        ));
    }
    Ok(())
}

fn require_distinct_roots(
    request: &SdkGenerationMatrixReadbackRequest<'_, '_>,
) -> Result<(), ContractError> {
    let mut protected = vec![
        request.complete.builds.packages.directory.clone(),
        request.complete.builds.subject_manifest.clone(),
    ];
    for build in request.complete.builds.lanes.values() {
        protected.push(build.comparison.clone());
        for side in &build.builds {
            protected.push(side.package_dir.clone());
            protected.push(side.measurement.clone());
        }
    }
    protected.extend(
        request
            .complete
            .lanes
            .values()
            .map(|lane| lane.directory.clone()),
    );
    for selected in request.generations.values() {
        for root in [
            &selected.package.package_dir,
            &selected.compatibility.directory,
        ] {
            require_root(root, &protected)?;
            protected.push(root.clone());
        }
    }
    Ok(())
}

fn require_root(root: &Path, protected: &[PathBuf]) -> Result<(), ContractError> {
    crate::release_index_v2_readback::validate_directory_path(root)?;
    if root
        .canonicalize()
        .map_err(|_| error("cannot resolve SDK generation matrix root"))?
        != *root
    {
        return Err(error("SDK generation matrix roots must be canonical"));
    }
    for other in protected {
        if aros_common::publication::filesystem_paths_overlap(root, other)
            .map_err(|_| error("cannot inspect SDK generation matrix root ancestry"))?
        {
            return Err(error(
                "SDK generation matrix roots must be physically nonoverlapping",
            ));
        }
    }
    Ok(())
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
mod root_tests {
    use super::{require_root, require_same_sdk_package};
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn canonical_sdk_join_rejects_valid_same_identity_with_different_payload_bytes() {
        use crate::sdk_package::{package, verify, SdkPackageRequest, SdkVerificationRequest};
        use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
        use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest};
        use std::collections::BTreeMap;

        let temporary = tempfile::tempdir().unwrap();
        let identity = SdkIdentity {
            sdk_id: "core".into(),
            version: "0.1.0".into(),
            target: SdkTarget {
                target_triple: "i686-aros".into(),
                abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
            },
            source: SdkSourceIdentity {
                commit: "1".repeat(40),
                git_tree: "2".repeat(40),
                content_sha256: sha256_bytes(b"synthetic SDK source"),
                contract_sha256: sha256_bytes(b"synthetic SDK contract"),
            },
            source_date_epoch: 946_684_800,
        };
        let verified = [b"#define SDK_VALUE 1\n", b"#define SDK_VALUE 2\n"]
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                let candidate = temporary.path().join(format!("candidate-{index}"));
                fs::create_dir(&candidate).unwrap();
                fs::write(candidate.join("public.h"), bytes).unwrap();
                let (tree, _) =
                    toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"]).unwrap();
                let tree = Sha256Digest::parse(&tree).unwrap();
                let output = package(&SdkPackageRequest {
                    candidate_root: candidate,
                    output_dir: temporary.path().join(format!("package-{index}")),
                    identity: identity.clone(),
                    expected_tree_sha256: tree.clone(),
                    forbidden_prefixes: vec![],
                })
                .unwrap();
                verify(&SdkVerificationRequest {
                    package_dir: output.output_dir,
                    identity: identity.clone(),
                    expected_tree_sha256: tree,
                    expected_archive_sha256: output.archive_sha256,
                    expected_archive_size: output.archive_size,
                    forbidden_prefixes: vec![],
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(verified[0].manifest.identity, verified[1].manifest.identity);
        require_same_sdk_package(&verified[0], &verified[0]).unwrap();
        let error = require_same_sdk_package(&verified[1], &verified[0]).unwrap_err();
        assert!(error.to_string().contains("canonical indexed SDK bytes"));
    }

    #[test]
    fn accepts_a_canonical_sdk_root_separate_from_packages_and_documents() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let sdk = parent.join("sdk-package");
        let release = parent.join("release");
        let report = parent.join("comparison.json");
        fs::create_dir(&sdk).unwrap();
        fs::create_dir(&release).unwrap();
        fs::write(&report, b"selected report").unwrap();
        require_root(&sdk, &[release, report]).unwrap();
    }

    #[test]
    fn rejects_reuse_and_ancestor_overlap_with_packages_or_documents() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let release = parent.join("release");
        let nested = release.join("nested");
        let report = release.join("comparison.json");
        fs::create_dir(&release).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(&report, b"selected report").unwrap();
        for (root, protected) in [
            (&release, &release),
            (&nested, &release),
            (&release, &nested),
            (&release, &report),
        ] {
            let failure = require_root(root, std::slice::from_ref(protected)).unwrap_err();
            assert!(failure.to_string().contains("physically nonoverlapping"));
        }
    }

    #[test]
    fn rejects_linked_or_absent_sdk_roots_without_modifying_selected_files() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let release = parent.join("release");
        let alias = parent.join("alias");
        let absent = parent.join("absent");
        fs::create_dir(&release).unwrap();
        let sentinel = release.join("sentinel");
        fs::write(&sentinel, b"keep selected bytes").unwrap();
        symlink(&release, &alias).unwrap();
        assert!(require_root(&alias, std::slice::from_ref(&release)).is_err());
        assert!(require_root(&absent, std::slice::from_ref(&release)).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep selected bytes");
        assert!(!absent.exists());
    }
}
