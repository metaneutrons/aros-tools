//! Complete family-v2 byte joins and durable recording, never authentication.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};

use aros_common::publication::{
    publish_atomic_file, validate_existing_directory_prefix_nofollow, AtomicFilePolicy,
    PortableOutputName,
};
use aros_common::sdk_manifest::SdkIdentity;
use aros_common::{
    measure_regular_file_bounded, open_regular_file_nofollow, sha256_bytes, Sha256Digest,
};
use aros_toolchain::compatibility::{
    NativeCompatibilityExpectedPackage, NativeCompatibilityInputClaims,
    PortableNativeCompatibilityRequest,
};
use aros_toolchain::native_candidate::{
    readback_release_compatibility_v2, ReleaseBuildLaneRequestV2, ReleaseBuildReadbackRequestV2,
    ReleaseBuildSideRequestV2, ReleaseCompatibilityReadbackRequestV2,
};
use aros_toolchain::plan::{Executor, Identity};
use aros_toolchain::qualification_evidence::EvidencePolicy;
use aros_toolchain::qualification_evidence_v2::SourceRunIdentityV2;
use aros_toolchain::qualification_readback_v2::{
    readback_qualification_bytes_v2, QualificationByteReadbackRequestV2,
};
use aros_toolchain::qualification_recording_v2::{
    record_qualification_bytes_v2, QualificationRecordingRequestV2,
};
use aros_toolchain::recovery_v2::{
    readback_recovery_bytes_v2, RecoveryByteReadbackRequestV2, RecoveryRequestV2,
};
use aros_toolchain::release_index_v2::{NativeReleaseIndexV2, INDEX_NAME};
use aros_toolchain::release_index_v2_readback::{
    readback_indexed_packages, IndexedPackageReadbackRequestV2, IndexedPackageReadbackV2,
};
use aros_toolchain::release_inputs::{BoundSdk, ReleaseInputs, ACTIVE_HOSTS};
use aros_toolchain::repackage_v2::{
    repackage_verified_package_v2, VerifiedPackageRepackageRequestV2,
};
use aros_toolchain::sdk_generation::SdkGenerationReadbackRequest;
use aros_toolchain::sdk_generation_lanes::ExpectedSdkGenerationLane;
use aros_toolchain::sdk_generation_matrix::{
    readback_sdk_generation_matrix, SdkGenerationMatrixReadbackRequest,
};
use aros_toolchain::sdk_package::SdkVerificationRequest;
use aros_toolchain::source_lock::CompilerFamily;

use super::qualification_bytes::QualificationArgs;
use super::recovery_family::RecoveryArgs;
use super::release_evidence::EvidenceArgs;
use super::release_evidence_selection::{self, SdkGenerationSelection, Selection};
use super::{native_error, print_json, RecordQualificationArgs, RepackageArgs, ResultFormat};

const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const INPUTS_NAME: &str = "toolchain-release-inputs-v2.json";

pub(super) fn run(args: &EvidenceArgs) -> miette::Result<()> {
    run_selected(args, Operation::Inspect)
}

pub(super) fn run_qualification(args: &QualificationArgs) -> miette::Result<()> {
    run_selected(&args.evidence, Operation::Verify(args))
}

pub(super) fn run_recovery(args: &RecoveryArgs) -> miette::Result<()> {
    run_selected(&args.qualification.evidence, Operation::Recovery(args))
}

pub(super) fn run_repackage(recovery: &RecoveryArgs, args: &RepackageArgs) -> miette::Result<()> {
    run_selected(
        &recovery.qualification.evidence,
        Operation::Repackage(recovery, args),
    )
}

#[derive(Clone, Copy)]
enum Operation<'a> {
    Inspect,
    Verify(&'a QualificationArgs),
    Record(&'a RecordQualificationArgs),
    Recovery(&'a RecoveryArgs),
    Repackage(&'a RecoveryArgs, &'a RepackageArgs),
}

impl<'a> Operation<'a> {
    const fn recovery(self) -> Option<&'a RecoveryArgs> {
        match self {
            Self::Recovery(selected) | Self::Repackage(selected, _) => Some(selected),
            _ => None,
        }
    }
}

/// V2 identity is selected by the independently retained exact index digest.
/// Legacy report layouts and source-lock names never enter this collector.
pub(super) fn run_record(args: &RecordQualificationArgs) -> miette::Result<()> {
    let index_sha256 = required(args.index_sha256.as_ref(), "--index-sha256")?;
    let index_bytes = read_selected(&args.release_dir.join(INDEX_NAME), index_sha256)?;
    let inputs = ReleaseInputs::load(&args.release_dir).map_err(|error| native_error(&error))?;
    let index =
        NativeReleaseIndexV2::parse(&index_bytes, &inputs).map_err(|error| native_error(&error))?;
    let evidence = EvidenceArgs {
        directory: args.release_dir.clone(),
        release_id: index.release_id().to_owned(),
        base_url: index.base_url().to_owned(),
        inputs_sha256: required(args.inputs_sha256.as_ref(), "--inputs-sha256")?.clone(),
        index_sha256: index_sha256.clone(),
        selection: required(args.selection.as_ref(), "--selection")?.clone(),
        selection_sha256: required(args.selection_sha256.as_ref(), "--selection-sha256")?.clone(),
        subject_manifest: required(args.subject_manifest.as_ref(), "--subject-manifest")?.clone(),
        subject_manifest_sha256: required(
            args.subject_manifest_sha256.as_ref(),
            "--subject-manifest-sha256",
        )?
        .clone(),
        forbidden_prefixes: args.forbidden_prefixes.clone(),
        format: args.format,
    };
    run_selected(&evidence, Operation::Record(args))
}

fn required<'a, T>(value: Option<&'a T>, name: &str) -> miette::Result<&'a T> {
    value.ok_or_else(|| miette::miette!("family-v2 recording requires {name}"))
}

fn select_sdk_generations<'a>(
    selection: &'a Selection,
    expected: &[ExpectedSdkGenerationLane],
) -> miette::Result<BTreeMap<ExpectedSdkGenerationLane, &'a SdkGenerationSelection>> {
    expected
        .iter()
        .map(|lane| {
            let selected = selection
                .sdk_generations
                .iter()
                .find(|selected| {
                    selected.sdk_id == lane.sdk_id()
                        && selected.group_id == lane.group_id()
                        && selected.target_profile == lane.profile()
                        && selected.host == lane.host()
                })
                .ok_or_else(|| {
                    miette::miette!("SDK generation has no exact independent selection")
                })?;
            Ok((lane.clone(), selected))
        })
        .collect()
}

fn read_sdk_generation_records(
    selections: &BTreeMap<ExpectedSdkGenerationLane, &SdkGenerationSelection>,
) -> miette::Result<BTreeMap<ExpectedSdkGenerationLane, Vec<u8>>> {
    selections
        .iter()
        .map(|(lane, selected)| {
            Ok((
                lane.clone(),
                read_selected(&selected.record.path, &selected.record.sha256)?,
            ))
        })
        .collect()
}

fn sdk_generation_packages(
    packages: &IndexedPackageReadbackRequestV2,
    selections: &BTreeMap<ExpectedSdkGenerationLane, &SdkGenerationSelection>,
) -> miette::Result<BTreeMap<ExpectedSdkGenerationLane, SdkVerificationRequest>> {
    selections
        .iter()
        .map(|(lane, selected)| {
            let mut package = packages
                .sdk_packages
                .get(lane.sdk_id())
                .ok_or_else(|| {
                    miette::miette!("SDK generation lacks a selected canonical package")
                })?
                .clone();
            package.package_dir.clone_from(&selected.package_dir);
            Ok((lane.clone(), package))
        })
        .collect()
}

fn sdk_compatibility_requests<'a>(
    packages: &'a IndexedPackageReadbackRequestV2,
    measured: &'a IndexedPackageReadbackV2,
    selections: &BTreeMap<ExpectedSdkGenerationLane, &SdkGenerationSelection>,
    claims: &'a BTreeMap<ExpectedSdkGenerationLane, NativeCompatibilityInputClaims>,
) -> miette::Result<BTreeMap<ExpectedSdkGenerationLane, PortableNativeCompatibilityRequest<'a>>> {
    selections.iter().map(|(lane, selected)| {
        let target_profile = lane.profile();
        let artifact = packages.index.artifacts().iter().find(|artifact| {
            artifact.group_id() == lane.group_id() && artifact.target_profile() == target_profile
                && artifact.host() == lane.host()
        }).ok_or_else(|| miette::miette!("SDK generation lacks an exact indexed compiler lane"))?;
        let package = measured.packages().iter().find(|package| package.asset() == artifact.asset())
            .ok_or_else(|| miette::miette!("SDK generation lacks a measured indexed compiler"))?.package();
        let group = packages.inputs.groups().iter().find(|group| group.id() == lane.group_id())
            .ok_or_else(|| miette::miette!("SDK generation lacks an input-derived compiler group"))?;
        let profile = group.profiles().select(target_profile).map_err(|error| native_error(&error))?;
        let sdk = packages.inputs.sdks().iter().find(|sdk| sdk.id() == lane.sdk_id())
            .ok_or_else(|| miette::miette!("SDK generation lacks an input-derived product identity"))?;
        if selected.compatibility.native_sdk_required != (group.source_lock().family() == CompilerFamily::Gnu) {
            return Err(miette::miette!("SDK generation native policy differs from its independently selected compiler family"));
        }
        let expected_package = NativeCompatibilityExpectedPackage {
            manifest: &package.manifest, archive_sha256: &package.archive_sha256,
            archive_size: package.archive_size, compiler: artifact.compiler(),
            source_commit: artifact.source_commit(), host: artifact.host(),
        };
        let expected = match group.source_lock().family() {
            CompilerFamily::Gnu => claims[lane].expectations(expected_package, group.profiles(), profile, true),
            CompilerFamily::Llvm => claims[lane].expectations_with_llvm_sdk(expected_package, group.profiles(), profile, sdk.identity()),
        }.map_err(|error| native_error(&error))?;
        Ok((lane.clone(), PortableNativeCompatibilityRequest {
            directory: selected.compatibility.directory.clone(),
            manifest_sha256: selected.compatibility.manifest_sha256.clone(), expected,
        }))
    }).collect()
}

fn selected_bound_llvm_sdk<'a>(
    sdks: &'a [BoundSdk],
    sdk_id: &str,
    family: CompilerFamily,
    group_id: &str,
    profile: &str,
    native_sdk_required: bool,
) -> miette::Result<&'a SdkIdentity> {
    if family != CompilerFamily::Llvm || native_sdk_required {
        return Err(miette::miette!(
            "explicit LLVM SDK selection requires an LLVM compiler lane without GNU SDK policy"
        ));
    }
    let sdk = sdks.iter().find(|sdk| sdk.id() == sdk_id).ok_or_else(|| {
        miette::miette!("selected LLVM SDK product is absent from release inputs")
    })?;
    if !sdk
        .compiler_profiles()
        .iter()
        .any(|reference| reference.group_id() == group_id && reference.profile() == profile)
    {
        return Err(miette::miette!(
            "selected LLVM SDK does not reference this exact compiler group and profile"
        ));
    }
    Ok(sdk.identity())
}

fn lane_llvm_sdk_identity<'a>(
    inputs: &'a ReleaseInputs,
    sdk_id: Option<&str>,
    native_sdk_required: bool,
    group_id: &str,
    target_profile: &str,
) -> miette::Result<Option<&'a SdkIdentity>> {
    let Some(sdk_id) = sdk_id else {
        return Ok(None);
    };
    let group = inputs
        .groups()
        .iter()
        .find(|group| group.id() == group_id)
        .ok_or_else(|| miette::miette!("selected compiler lane has no bound input group"))?;
    let profile = group
        .profiles()
        .select(target_profile)
        .map_err(|error| native_error(&error))?;
    selected_bound_llvm_sdk(
        inputs.sdks(),
        sdk_id,
        group.source_lock().family(),
        group.id(),
        profile.name(),
        native_sdk_required,
    )
    .map(Some)
}

fn select_lane_llvm_sdks<'a>(
    inputs: &'a ReleaseInputs,
    index: &NativeReleaseIndexV2,
    selection: &Selection,
) -> miette::Result<BTreeMap<String, Option<&'a SdkIdentity>>> {
    index
        .artifacts()
        .iter()
        .map(|artifact| {
            let lane = selection.lanes.get(artifact.asset()).ok_or_else(|| {
                miette::miette!("indexed compiler lane has no evidence selection")
            })?;
            Ok((
                artifact.asset().to_owned(),
                lane_llvm_sdk_identity(
                    inputs,
                    lane.llvm_sdk_id.as_deref(),
                    lane.compatibility.native_sdk_required,
                    artifact.group_id(),
                    artifact.target_profile(),
                )?,
            ))
        })
        .collect()
}

fn validate_lane_llvm_sdks(
    inputs: &ReleaseInputs,
    index: &NativeReleaseIndexV2,
    selection: &Selection,
) -> miette::Result<()> {
    for artifact in index.artifacts() {
        let lane = selection
            .lanes
            .get(artifact.asset())
            .ok_or_else(|| miette::miette!("indexed compiler lane has no evidence selection"))?;
        let _ = lane_llvm_sdk_identity(
            inputs,
            lane.llvm_sdk_id.as_deref(),
            lane.compatibility.native_sdk_required,
            artifact.group_id(),
            artifact.target_profile(),
        )?;
    }
    Ok(())
}

fn selected_qualification_readback(
    selected: &QualificationArgs,
) -> miette::Result<(Vec<u8>, EvidencePolicy)> {
    let bytes = read_selected(
        &selected.qualification_evidence,
        &selected.qualification_sha256,
    )?;
    let policy_bytes = read_selected(&selected.policy, &selected.policy_sha256)?;
    let policy = serde_json::from_slice(&policy_bytes).map_err(|_| {
        miette::miette!("qualification policy must match the closed verifier schema")
    })?;
    Ok((bytes, policy))
}

fn run_selected(args: &EvidenceArgs, operation: Operation<'_>) -> miette::Result<()> {
    let qualification = match operation {
        Operation::Verify(selected) => Some(selected),
        Operation::Recovery(selected) | Operation::Repackage(selected, _) => {
            Some(&selected.qualification)
        }
        _ => None,
    };
    let selection_bytes = read_selected(&args.selection, &args.selection_sha256)?;
    let selection = release_evidence_selection::parse(&selection_bytes)?;
    let inputs = ReleaseInputs::load(&args.directory).map_err(|error| native_error(&error))?;
    if inputs.collection_sha256() != &args.inputs_sha256 {
        return Err(miette::miette!(
            "release inputs differ from the independently selected digest"
        ));
    }
    let index_path = args.directory.join(INDEX_NAME);
    let index_bytes = read_selected(&index_path, &args.index_sha256)?;
    let index =
        NativeReleaseIndexV2::parse(&index_bytes, &inputs).map_err(|error| native_error(&error))?;
    validate_selection(args, &index, &selection, qualification)?;
    validate_lane_llvm_sdks(&inputs, &index, &selection)?;
    release_evidence_selection::require_sdk_package_ids(
        &selection,
        inputs
            .sdks()
            .iter()
            .map(aros_toolchain::release_inputs::BoundSdk::id),
    )?;
    // Every authority path must reacquire the same complete SDK byte matrix.
    // These byte joins still grant no original-job or signature authentication.
    let expected_sdk_generations = inputs.expected_sdk_generation_lanes();
    release_evidence_selection::require_sdk_generation_lanes(
        &selection,
        &expected_sdk_generations,
    )?;
    if let Some(selected) = operation.recovery() {
        require_independent_document(args, &selection, &selected.recovery_request)?;
    }
    if let Operation::Repackage(recovery, execution) = operation {
        repackage_preflight(execution, args, &selection, recovery)?;
    }
    if let Operation::Record(selected) = operation {
        recording_preflight(selected, args, &selection)?;
    }
    let selected_qualification = qualification
        .map(selected_qualification_readback)
        .transpose()?;
    let selected_recovery = if let Some(selected) = operation.recovery() {
        let bytes = read_selected(
            &selected.recovery_request,
            &selected.recovery_request_sha256,
        )?;
        Some(RecoveryRequestV2::parse(&bytes).map_err(|error| native_error(&error))?)
    } else {
        None
    };
    // Identity comes only from the independently digest-bound input collection;
    // the external selection contributes byte expectations, never source/ABI
    // identity, product membership or execution/authenticity claims.
    let sdk_packages = inputs
        .sdks()
        .iter()
        .map(|sdk| {
            let selected = &selection.sdk_packages[sdk.id()];
            (
                sdk.id().to_owned(),
                SdkVerificationRequest {
                    package_dir: args.directory.clone(),
                    identity: sdk.identity().clone(),
                    expected_tree_sha256: selected.tree_sha256.clone(),
                    expected_archive_sha256: selected.archive_sha256.clone(),
                    expected_archive_size: selected.archive_size,
                    forbidden_prefixes: args.forbidden_prefixes.clone(),
                },
            )
        })
        .collect();
    let packages = IndexedPackageReadbackRequestV2 {
        sdk_packages,
        directory: args.directory.clone(),
        inputs,
        index,
        build_environments: selection
            .lanes
            .iter()
            .map(|(asset, lane)| (asset.clone(), lane.build_environment.clone()))
            .collect(),
        forbidden_prefixes: args.forbidden_prefixes.clone(),
    };
    let lane_llvm_sdks = select_lane_llvm_sdks(&packages.inputs, &packages.index, &selection)?;
    let builds = build_request(args, &packages, &selection)?;
    // These actual archives, rather than an input observation's package
    // claims, supply the independently checked compatibility package bindings.
    let measured = readback_indexed_packages(&packages).map_err(|error| native_error(&error))?;
    let claims = selection
        .lanes
        .iter()
        .map(|(asset, lane)| {
            let selected = &lane.compatibility.inputs;
            let bytes = read_selected(&selected.path, &selected.sha256)?;
            let claims = NativeCompatibilityInputClaims::parse(&bytes, &selected.sha256)
                .map_err(|error| native_error(&error))?;
            Ok((asset.clone(), claims))
        })
        .collect::<miette::Result<BTreeMap<_, _>>>()?;
    let profiles = packages
        .index
        .artifacts()
        .iter()
        .map(|artifact| {
            let group = packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .ok_or_else(|| {
                    miette::miette!("indexed evidence lane has no selected input group")
                })?;
            let profile = group
                .profiles()
                .select(artifact.target_profile())
                .map_err(|error| native_error(&error))?;
            Ok((artifact.asset().to_owned(), profile))
        })
        .collect::<miette::Result<BTreeMap<_, _>>>()?;
    let lanes = packages
        .index
        .artifacts()
        .iter()
        .zip(measured.packages())
        .map(|(artifact, package)| {
            if artifact.asset() != package.asset() {
                return Err(miette::miette!(
                    "measured package order differs from the selected index"
                ));
            }
            let group = packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .ok_or_else(|| {
                    miette::miette!("indexed evidence lane has no selected input group")
                })?;
            let package = package.package();
            let expected_package = NativeCompatibilityExpectedPackage {
                manifest: &package.manifest,
                archive_sha256: &package.archive_sha256,
                archive_size: package.archive_size,
                compiler: artifact.compiler(),
                source_commit: artifact.source_commit(),
                host: artifact.host(),
            };
            let selected = &selection.lanes[artifact.asset()].compatibility;
            let expected = if let Some(identity) = lane_llvm_sdks[artifact.asset()] {
                claims[artifact.asset()].expectations_with_llvm_sdk(
                    expected_package,
                    group.profiles(),
                    profiles[artifact.asset()],
                    identity,
                )
            } else {
                claims[artifact.asset()].expectations(
                    expected_package,
                    group.profiles(),
                    profiles[artifact.asset()],
                    selected.native_sdk_required,
                )
            }
            .map_err(|error| native_error(&error))?;
            Ok((
                artifact.asset().to_owned(),
                PortableNativeCompatibilityRequest {
                    directory: selected.directory.clone(),
                    manifest_sha256: selected.manifest_sha256.clone(),
                    expected,
                },
            ))
        })
        .collect::<miette::Result<BTreeMap<_, _>>>()?;
    // This factory acquires all A/B bytes itself, then joins all compatibility
    // closures. No partial lane result or caller-created proof is accepted.
    let complete_request = ReleaseCompatibilityReadbackRequestV2 {
        builds: &builds,
        lanes,
    };
    // Independent selection chooses byte expectations only. Product membership,
    // compiler references and host coverage come exclusively from release inputs.
    let sdk_selections = select_sdk_generations(&selection, &expected_sdk_generations)?;
    let sdk_records = read_sdk_generation_records(&sdk_selections)?;
    let sdk_claims = sdk_selections
        .iter()
        .map(|(lane, selected)| {
            let bytes = read_selected(
                &selected.compatibility.inputs.path,
                &selected.compatibility.inputs.sha256,
            )?;
            let claims = NativeCompatibilityInputClaims::parse(
                &bytes,
                &selected.compatibility.inputs.sha256,
            )
            .map_err(|error| native_error(&error))?;
            Ok((lane.clone(), claims))
        })
        .collect::<miette::Result<BTreeMap<_, _>>>()?;
    let sdk_packages = sdk_generation_packages(&packages, &sdk_selections)?;
    let sdk_compatibility =
        sdk_compatibility_requests(&packages, &measured, &sdk_selections, &sdk_claims)?;
    let sdk_requests = expected_sdk_generations
        .iter()
        .map(|lane| {
            (
                lane.clone(),
                SdkGenerationReadbackRequest {
                    inputs: &packages.inputs,
                    sdk_id: lane.sdk_id(),
                    compiler_group_id: lane.group_id(),
                    record_bytes: &sdk_records[lane],
                    record_sha256: &sdk_selections[lane].record.sha256,
                    package: &sdk_packages[lane],
                    compatibility: &sdk_compatibility[lane],
                },
            )
        })
        .collect();
    let sdk_request =
        (!expected_sdk_generations.is_empty()).then_some(SdkGenerationMatrixReadbackRequest {
            complete: &complete_request,
            generations: sdk_requests,
        });
    let mut recorded = None;
    let mut recovery_decision = None;
    let mut recovered_packages = None;
    let mut sdk_generations = Vec::new();
    let readback = if let Operation::Record(selected) = operation {
        let producer_commit =
            super::parse_git_object(&selected.source_tag_commit, "source tag commit")?;
        let source_run = SourceRunIdentityV2 {
            repository: selected.source_repository.clone(),
            workflow: selected.source_workflow.clone(),
            run_id: selected.source_run_id,
            run_attempt: *required(selected.source_run_attempt.as_ref(), "--source-run-attempt")?,
            producer_commit,
            source_tag: selected.source_tag.clone(),
            tag_object: super::parse_git_object(&selected.source_tag_object, "source tag object")?,
        };
        let policy = EvidencePolicy {
            source_repository: selected.source_repository.clone(),
            source_workflow: selected.source_workflow.clone(),
            signer_repository: selected.attestation_repository.clone(),
            signer_workflow: selected.attestation_workflow.clone(),
            signer: selected.attestation_signer.clone(),
            now: selected.created_at,
        };
        let (evidence, complete, generations) =
            record_qualification_bytes_v2(&QualificationRecordingRequestV2 {
                index_bytes: &index_bytes,
                complete: &complete_request,
                sdk: sdk_request.as_ref(),
                source_run,
                policy,
                created_at: selected.created_at,
                expires_at: selected.expires_at,
            })
            .map_err(|error| native_error(&error))?
            .into_parts();
        recorded = Some(evidence);
        sdk_generations = generations;
        complete
    } else if let Some((bytes, policy)) = &selected_qualification {
        let request = QualificationByteReadbackRequestV2 {
            evidence_bytes: bytes,
            index_bytes: &index_bytes,
            complete: &complete_request,
            sdk: sdk_request.as_ref(),
            policy,
        };
        if let Some(recovery) = &selected_recovery {
            let request = RecoveryByteReadbackRequestV2 {
                recovery,
                qualification: &request,
            };
            let (decision, joined) = readback_recovery_bytes_v2(&request)
                .map_err(|error| native_error(&error))?
                .into_parts();
            recovery_decision = Some(decision);
            if let Operation::Repackage(_, execution) = operation {
                let asset =
                    super::repackage_family::selected(execution.family.asset.as_ref(), "--asset")?;
                recovered_packages = Some(
                    repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
                        recovery: &request,
                        asset: &asset,
                        extraction_roots: [
                            execution.first_extraction_dir.clone(),
                            execution.second_extraction_dir.clone(),
                        ],
                        output_dirs: [
                            execution.first_output_dir.clone(),
                            execution.second_output_dir.clone(),
                        ],
                    })
                    .map_err(|error| native_error(&error))?,
                );
            }
            let (complete, generations) = joined.into_parts();
            sdk_generations = generations;
            complete
        } else {
            let (complete, generations) = readback_qualification_bytes_v2(&request)
                .map_err(|error| native_error(&error))?
                .into_parts();
            sdk_generations = generations;
            complete
        }
    } else if let Some(sdk_request) = &sdk_request {
        let (complete, generations) = readback_sdk_generation_matrix(sdk_request)
            .map_err(|error| native_error(&error))?
            .into_parts();
        sdk_generations = generations;
        complete
    } else {
        readback_release_compatibility_v2(&complete_request)
            .map_err(|error| native_error(&error))?
    };
    read_selected(&args.selection, &args.selection_sha256)?;
    read_selected(&index_path, &args.index_sha256)?;
    read_selected(&args.directory.join(INPUTS_NAME), &args.inputs_sha256)?;
    for lane in selection.lanes.values() {
        read_selected(
            &lane.compatibility.inputs.path,
            &lane.compatibility.inputs.sha256,
        )?;
    }
    for selected in &selection.sdk_generations {
        read_selected(&selected.record.path, &selected.record.sha256)?;
        read_selected(
            &selected.compatibility.inputs.path,
            &selected.compatibility.inputs.sha256,
        )?;
    }
    if let Some(selected) = qualification {
        read_selected(
            &selected.qualification_evidence,
            &selected.qualification_sha256,
        )?;
        read_selected(&selected.policy, &selected.policy_sha256)?;
    }
    if let Some(selected) = operation.recovery() {
        read_selected(
            &selected.recovery_request,
            &selected.recovery_request_sha256,
        )?;
        require_independent_document(args, &selection, &selected.recovery_request)?;
    }
    let lanes = readback.builds().lanes().iter().zip(readback.lanes()).map(|(build, compatibility)| {
        serde_json::json!({
            "asset": build.asset(),
            "measurement_sha256": build.measurement_sha256(),
            "build_result_sha256": build.build_result_sha256(),
            "finished_receipt_sha256": build.finished_receipt_sha256(),
            "comparison_sha256": build.comparison_sha256(),
            "compatibility_inputs_sha256": selection.lanes[build.asset()].compatibility.inputs.sha256,
            "compatibility_manifest_sha256": compatibility.compatibility().manifest_sha256(),
            "compatibility_receipt_sha256": compatibility.compatibility().receipt().receipt_sha256,
            "standalone": standalone_document(compatibility.compatibility().standalone()),
        })
    }).collect::<Vec<_>>();
    let operation_name = match operation {
        Operation::Inspect => "verify-release-evidence",
        Operation::Verify(_) => "verify-qualification",
        Operation::Record(_) => "record-qualification",
        Operation::Recovery(_) => "verify-recovery",
        Operation::Repackage(_, _) => "repackage",
    };
    let mut result = serde_json::json!({
        "schema": "aros-toolchain-producer-stage-v2",
        "operation": operation_name,
        "release_format": "family-v2",
        "assurance": "byte-consistency-only",
        "release_id": args.release_id,
        "inputs_sha256": args.inputs_sha256,
        "index_sha256": args.index_sha256,
        "selection_sha256": args.selection_sha256,
        "subject_manifest_sha256": readback.builds().subject_manifest_sha256(),
        "checksums_sha256": readback.builds().checksums_sha256(),
        "provenance_sha256": readback.builds().provenance_sha256(),
        "lane_count": lanes.len(),
        "build_count": lanes.len() * 2,
        "lanes": lanes,
    });
    let sdk_packages = readback.builds().sdk_packages();
    if !sdk_generations.is_empty() {
        result["sdk_generation_count"] = serde_json::json!(sdk_generations.len());
        result["sdk_generations"] = serde_json::json!(sdk_generations
            .iter()
            .map(|joined| {
                let lane = joined.lane();
                let generation = joined.generation();
                serde_json::json!({
                    "sdk_id":lane.sdk_id(), "group_id":lane.group_id(),
                    "target_profile":lane.profile(), "host":lane.host(),
                    "generation_record_sha256":generation.record_sha256(),
                    "compatibility_manifest_sha256":generation.compatibility_manifest_sha256(),
                    "compatibility_receipt_sha256":generation.compatibility_receipt_sha256(),
                    "sdk_link_receipt_sha256":generation.sdk_link_receipt_sha256(),
                })
            })
            .collect::<Vec<_>>());
    }
    if !sdk_packages.is_empty() {
        // These are measured shared packages, not SDK-generation receipts or
        // authenticated owning-job evidence. Compiler-only result bytes stay
        // unchanged when no SDK products were selected.
        result["sdk_count"] = serde_json::json!(sdk_packages.len());
        result["sdks"] = serde_json::json!(sdk_packages
            .iter()
            .map(|sdk| {
                let lock = sdk.lock();
                serde_json::json!({
                    "sdk_id":lock.identity.sdk_id,
                    "identity":lock.identity,
                    "tree_sha256":lock.tree_sha256,
                    "archive_sha256":sdk.package().archive_sha256,
                    "archive_size":sdk.package().archive_size,
                    "manifest_sha256":lock.manifest.sha256,
                    "manifest_size":lock.manifest.size,
                })
            })
            .collect::<Vec<_>>());
    }
    if let Some(selected) = qualification {
        result["qualification_sha256"] = serde_json::json!(selected.qualification_sha256);
        result["policy_sha256"] = serde_json::json!(selected.policy_sha256);
    }
    if let Some(selected) = operation.recovery() {
        result["recovery_request_sha256"] = serde_json::json!(selected.recovery_request_sha256);
        result["decision"] = serde_json::to_value(recovery_decision.as_ref().ok_or_else(|| {
            miette::miette!("family-v2 recovery did not reacquire complete evidence")
        })?)
        .map_err(|_| miette::miette!("cannot serialize recovery decision"))?;
    }
    if let Operation::Repackage(_, execution) = operation {
        let output = recovered_packages.as_ref().ok_or_else(|| {
            miette::miette!("family-v2 recovery did not execute complete packaging")
        })?;
        let receipt = aros_toolchain::release_index::write_package_comparison_report(
            &execution.comparison_output,
            &output.comparison,
        )
        .map_err(|error| native_error(&error))?;
        result["source_release_id"] = serde_json::json!(args.release_id);
        result["release_id"] = serde_json::json!(
            output
                .packages
                .repackaged
                .first_verified
                .manifest
                .release_id
        );
        result["asset"] = serde_json::json!(execution.family.asset);
        result["source_archive_sha256"] = serde_json::json!(output.packages.source.archive_sha256);
        result["source_archive_size"] = serde_json::json!(output.packages.source.archive_size);
        result["first_package_dir"] =
            serde_json::json!(output.packages.repackaged.first.output_dir);
        result["second_package_dir"] =
            serde_json::json!(output.packages.repackaged.second.output_dir);
        result["comparison_receipt"] = serde_json::json!(receipt.path);
        result["comparison_receipt_sha256"] = serde_json::json!(receipt.sha256);
        result["package_set_sha256"] = serde_json::json!(output.comparison.package_set_sha256);
    }
    if let Operation::Record(selected) = operation {
        let evidence = recorded.as_ref().ok_or_else(|| {
            miette::miette!("family-v2 recording did not acquire complete evidence")
        })?;
        let mut bytes = serde_json::to_vec_pretty(evidence)
            .map_err(|_| miette::miette!("cannot serialize complete qualification evidence"))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_METADATA_BYTES {
            return Err(miette::miette!(
                "qualification evidence exceeds its metadata limit"
            ));
        }
        // Recheck the independent destination after all byte reads, then expose
        // exactly one complete durable claim file, never a partially written one.
        recording_preflight(selected, args, &selection)?;
        publish_atomic_file(&selected.output, &bytes, AtomicFilePolicy::NoClobber).map_err(
            |error| {
                miette::miette!("cannot durably record qualification without clobbering: {error}")
            },
        )?;
        result["qualification_sha256"] = serde_json::json!(sha256_bytes(&bytes));
        result["qualification_evidence"] = serde_json::json!(selected.output);
    }
    match args.format {
        ResultFormat::Json => print_json(&result),
        ResultFormat::Human => {
            aros_common::outputln!("Complete family-v2 byte read-back: {} lanes, {} builds\nChecksums SHA-256: {}\nExecution, signatures and publication are not authenticated by this operation.", readback.lanes().len(), readback.builds().lanes().len() * 2, readback.builds().checksums_sha256());
            if let Operation::Record(selected) = operation {
                aros_common::outputln!(
                    "Qualification claims recorded: {}",
                    selected.output.display()
                );
            }
            if let Operation::Repackage(_, execution) = operation {
                aros_common::outputln!("Local family-v2 repackage: {}\nFirst package: {}\nSecond package: {}\nComparison receipt: {}\nThese are packaging operations, not new compiler A/B builds.", result["release_id"].as_str().unwrap_or_default(), execution.first_output_dir.display(), execution.second_output_dir.display(), execution.comparison_output.display());
            }
            Ok(())
        }
    }
}

fn recording_preflight(
    selected: &RecordQualificationArgs,
    args: &EvidenceArgs,
    selection: &Selection,
) -> miette::Result<()> {
    let output = &selected.output;
    fresh_output_preflight(output, "qualification output")?;
    let protected = selected_evidence_paths(args, selection);
    for path in protected {
        if super::compatibility_export::filesystem_overlap(output, path)? {
            return Err(miette::miette!("qualification output must remain outside all selected release, package, compatibility and input paths"));
        }
    }
    Ok(())
}

fn repackage_preflight(
    execution: &RepackageArgs,
    args: &EvidenceArgs,
    selection: &Selection,
    recovery: &RecoveryArgs,
) -> miette::Result<()> {
    let destinations = [
        &execution.first_extraction_dir,
        &execution.second_extraction_dir,
        &execution.first_output_dir,
        &execution.second_output_dir,
        &execution.comparison_output,
    ];
    let mut protected = selected_evidence_paths(args, selection);
    protected.extend([
        recovery.qualification.qualification_evidence.as_path(),
        recovery.qualification.policy.as_path(),
        recovery.recovery_request.as_path(),
    ]);
    for (index, output) in destinations.iter().enumerate() {
        fresh_output_preflight(output, "repackage destination")?;
        for path in destinations[..index]
            .iter()
            .map(|path| path.as_path())
            .chain(protected.iter().copied())
        {
            if super::compatibility_export::filesystem_overlap(output, path)? {
                return Err(miette::miette!("repackage destinations must remain outside each other and all selected evidence paths"));
            }
        }
    }
    Ok(())
}

fn fresh_output_preflight(output: &Path, label: &str) -> miette::Result<()> {
    if !output.is_absolute()
        || output
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(miette::miette!(
            "{label} must have an absolute normalized path"
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| miette::miette!("{label} has no parent"))?;
    let leaf = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| miette::miette!("{label} has no portable leaf"))?;
    PortableOutputName::new(leaf).map_err(|_| miette::miette!("{label} has no portable leaf"))?;
    validate_existing_directory_prefix_nofollow(parent)
        .map_err(|_| miette::miette!("{label} parent must have no symlink components"))?;
    if !parent.is_dir() {
        return Err(miette::miette!("{label} parent must already exist"));
    }
    match std::fs::symlink_metadata(output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(miette::miette!("{label} already exists or is unavailable")),
    }
    Ok(())
}

fn selected_evidence_paths<'a>(args: &'a EvidenceArgs, selection: &'a Selection) -> Vec<&'a Path> {
    let protected = std::iter::once(args.directory.as_path())
        .chain(args.forbidden_prefixes.iter().map(PathBuf::as_path))
        .chain([args.selection.as_path(), args.subject_manifest.as_path()])
        .chain(selection.lanes.values().flat_map(|lane| {
            [
                lane.compatibility.directory.as_path(),
                lane.compatibility.inputs.path.as_path(),
                lane.comparison.path.as_path(),
            ]
            .into_iter()
            .chain(lane.builds.iter().flat_map(|build| {
                [
                    build.package_dir.as_path(),
                    build.measurement.path.as_path(),
                ]
            }))
        }))
        .chain(selection.sdk_generations.iter().flat_map(|sdk| {
            [
                sdk.package_dir.as_path(),
                sdk.record.path.as_path(),
                sdk.compatibility.directory.as_path(),
                sdk.compatibility.inputs.path.as_path(),
            ]
        }));
    protected.collect()
}

fn build_request(
    args: &EvidenceArgs,
    packages: &IndexedPackageReadbackRequestV2,
    selection: &Selection,
) -> miette::Result<ReleaseBuildReadbackRequestV2> {
    let lanes = packages
        .index
        .artifacts()
        .iter()
        .map(|artifact| {
            let selected = &selection.lanes[artifact.asset()];
            let group = packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .ok_or_else(|| {
                    miette::miette!("indexed evidence lane has no selected input group")
                })?;
            let recipe = group.recipe();
            let host = ACTIVE_HOSTS
                .iter()
                .copied()
                .find(|host| *host == artifact.host())
                .ok_or_else(|| {
                    miette::miette!("indexed evidence host is outside the selected active contract")
                })?;
            let builds = selected
                .builds
                .each_ref()
                .map(|side| ReleaseBuildSideRequestV2 {
                    package_dir: side.package_dir.clone(),
                    measurement: side.measurement.path.clone(),
                    measurement_sha256: side.measurement.sha256.clone(),
                    identity: Identity {
                        recipe_sha256: recipe.sha256().clone(),
                        source_commit: recipe.source().0.clone(),
                        producer_commit: recipe.producer().0.clone(),
                        tools_commit: recipe.tools().0.clone(),
                        host,
                        target_profile: artifact.target_profile().to_owned(),
                        executor: Executor {
                            contract_id: Some("aros-toolchain-producer-v1"),
                            contract_sha256: Some(side.executor.contract_sha256.clone()),
                            tools_commit: Some(recipe.tools().0.clone()),
                            binary_sha256: side.executor.binary_sha256.clone(),
                            origin_evidence_sha256: None,
                        },
                    },
                });
            Ok((
                artifact.asset().to_owned(),
                ReleaseBuildLaneRequestV2 {
                    builds,
                    comparison: selected.comparison.path.clone(),
                    comparison_sha256: selected.comparison.sha256.clone(),
                },
            ))
        })
        .collect::<miette::Result<_>>()?;
    Ok(ReleaseBuildReadbackRequestV2 {
        packages: packages.clone(),
        subject_manifest: args.subject_manifest.clone(),
        subject_manifest_sha256: args.subject_manifest_sha256.clone(),
        lanes,
    })
}

fn validate_selection(
    args: &EvidenceArgs,
    index: &NativeReleaseIndexV2,
    selection: &Selection,
    qualification: Option<&QualificationArgs>,
) -> miette::Result<()> {
    if index.release_id() != args.release_id || index.base_url() != args.base_url {
        return Err(miette::miette!(
            "release index differs from independently selected identity"
        ));
    }
    if selection.lanes.len() != index.artifacts().len()
        || index.artifacts().iter().any(|artifact| {
            selection
                .lanes
                .get(artifact.asset())
                .is_none_or(|lane| lane.required_paths != artifact.required_paths())
        })
    {
        return Err(miette::miette!(
            "evidence selection must exactly cover the input-derived index and required paths"
        ));
    }
    let documents = std::iter::once(args.selection.as_path())
        .chain(
            selection
                .lanes
                .values()
                .map(|lane| lane.compatibility.inputs.path.as_path()),
        )
        .chain(qualification.into_iter().flat_map(|selected| {
            [
                selected.qualification_evidence.as_path(),
                selected.policy.as_path(),
            ]
        }))
        .chain(selection.sdk_generations.iter().flat_map(|sdk| {
            [
                sdk.record.path.as_path(),
                sdk.compatibility.inputs.path.as_path(),
            ]
        }));
    for document in documents {
        require_independent_document(args, selection, document)?;
    }
    Ok(())
}

fn require_independent_document(
    args: &EvidenceArgs,
    selection: &Selection,
    document: &Path,
) -> miette::Result<()> {
    // Transport expectations must not hide among the inventory they select.
    let roots = std::iter::once(args.directory.as_path())
        .chain(selection.lanes.values().flat_map(|lane| {
            lane.builds
                .iter()
                .map(|build| build.package_dir.as_path())
                .chain(std::iter::once(lane.compatibility.directory.as_path()))
        }))
        .chain(selection.sdk_generations.iter().flat_map(|sdk| {
            [
                sdk.package_dir.as_path(),
                sdk.compatibility.directory.as_path(),
            ]
        }));
    for root in roots {
        if super::compatibility_export::filesystem_overlap(document, root)? {
            return Err(miette::miette!("independent evidence inputs must remain outside all selected release, package and compatibility roots"));
        }
    }
    Ok(())
}

fn read_selected(path: &Path, expected: &Sha256Digest) -> miette::Result<Vec<u8>> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(miette::miette!(
            "selected evidence metadata must have an absolute normalized path"
        ));
    }
    let file = open_regular_file_nofollow(path).map_err(|_| {
        miette::miette!("selected evidence metadata must be a no-follow regular file")
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| miette::miette!("cannot inspect selected evidence metadata"))?;
    if metadata.nlink() != 1 {
        return Err(miette::miette!(
            "selected evidence metadata must be singly linked"
        ));
    }
    let (identity, bytes) = measure_regular_file_bounded(path, MAX_METADATA_BYTES)
        .map_err(|_| {
            miette::miette!("selected evidence metadata is unavailable or exceeds its bound")
        })?
        .ok_or_else(|| miette::miette!("selected evidence metadata changed during read-back"))?;
    if (identity.device(), identity.inode()) != (metadata.dev(), metadata.ino())
        || sha256_bytes(&bytes) != *expected
    {
        return Err(miette::miette!("selected evidence metadata differs from its independently retained raw digest or identity"));
    }
    Ok(bytes)
}

fn standalone_document(
    report: &aros_toolchain::compatibility::StandaloneOutputReport,
) -> serde_json::Value {
    let member = |identity: &aros_toolchain::compatibility::StandaloneArtifactIdentity| {
        serde_json::json!({
            "sha256": identity.sha256,
            "size": identity.size,
            "class": match identity.class { aros_common::elf::Class::Elf32 => "elf32", aros_common::elf::Class::Elf64 => "elf64" },
        })
    };
    serde_json::Value::Object(
        report
            .targets
            .iter()
            .map(|(triple, target)| {
                (
                    triple.clone(),
                    serde_json::json!({"c": member(&target.c), "cxx": member(&target.cxx)}),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::lane_llvm_sdk_identity;
    use aros_common::sha256_bytes;
    use aros_toolchain::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};

    fn llvm_inputs() -> ReleaseInputs {
        const PRODUCER_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const TOOLS_COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
        const GNU_SOURCE_COMMIT: &str = "2222222222222222222222222222222222222222";
        let gnu_source_lock = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-source-lock-v3",
            "family":"gnu",
            "version":"16.2.0",
            "sources":[
                {
                    "component":"gcc","version":"16.2.0","purpose":"toolchain-component",
                    "filename":"gcc.tar.xz","url":"https://example.invalid/gcc.tar.xz",
                    "sha256":"3".repeat(64),"size":1
                },
                {
                    "component":"binutils","version":"2.47","purpose":"toolchain-component",
                    "filename":"binutils.tar.xz","url":"https://example.invalid/binutils.tar.xz",
                    "sha256":"4".repeat(64),"size":1
                }
            ],
            "host_python_packages":[{
                "name":"mako","version":"1.3.10","filename":"mako.tar.gz",
                "url":"https://example.invalid/mako.tar.gz","sha256":"5".repeat(64),
                "size":1,"source_root":"mako","python_path":"."
            }]
        }))
        .unwrap();
        let gnu_profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2",
            "family":"gnu",
            "upstream_commit":GNU_SOURCE_COMMIT,
            "profiles":[{
                "name":"rv32-aros","configure_target":"riscv-aros",
                "upstream_output_target":"riscv-aros","target_triple":"riscv-aros",
                "cpu":"riscv","platform":"fixture","float_abi":"ilp32f",
                "capabilities":["c","libgcc","standalone-collector"],
                "target":{
                    "schema":"aros-riscv-target-v1","isa":"rv32imafc","abi":"ilp32f",
                    "code_model":"medany","architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0",
                    "unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0
                }
            }]
        }))
        .unwrap();
        let mut gnu_recipe = json!({
            "schema":"aros-toolchain-recipe-v2",
            "source_commit":GNU_SOURCE_COMMIT,
            "source_tree":"6".repeat(40),
            "producer_commit":PRODUCER_COMMIT,
            "producer_tree":"e".repeat(40),
            "tools_commit":TOOLS_COMMIT,
            "tools_tree":"f".repeat(40),
            "source_date_epoch":0,
            "source_lock_sha256":sha256_bytes(&gnu_source_lock),
            "profiles_sha256":sha256_bytes(&gnu_profiles),
            "patches":[]
        });
        gnu_recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&gnu_recipe).unwrap()
        ));
        let gnu_recipe = serde_json::to_vec(&gnu_recipe).unwrap();
        let source_lock = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-source-lock-v2",
            "family":"llvm",
            "version":"17.0.6",
            "sources":[{
                "component":"llvm","version":"17.0.6","purpose":"toolchain-component",
                "filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz",
                "sha256":"1".repeat(64),"size":1
            }],
            "host_python_packages":[{
                "name":"wheel","version":"0.43.0","filename":"wheel.tar.gz",
                "url":"https://example.invalid/wheel.tar.gz","sha256":"2".repeat(64),
                "size":1,"source_root":"wheel","python_path":"."
            }]
        }))
        .unwrap();
        let profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2",
            "family":"llvm",
            "upstream_commit":SOURCE_COMMIT,
            "profiles":[
                {
                    "name":"pc-x86_64","configure_target":"pc-x86_64",
                    "upstream_output_target":"pc-x86_64","target_triple":"x86_64-unknown-aros",
                    "cpu":"x86_64","platform":"pc","float_abi":"",
                    "capabilities":["c","cxx","standalone-collector"]
                },
                {
                    "name":"pc-aarch64","configure_target":"pc-aarch64",
                    "upstream_output_target":"pc-aarch64","target_triple":"aarch64-unknown-aros",
                    "cpu":"aarch64","platform":"pc","float_abi":"",
                    "capabilities":["c","cxx","standalone-collector"]
                }
            ]
        }))
        .unwrap();
        let mut recipe = json!({
            "schema":"aros-toolchain-recipe-v2",
            "source_commit":SOURCE_COMMIT,
            "source_tree":"7".repeat(40),
            "producer_commit":PRODUCER_COMMIT,
            "producer_tree":"e".repeat(40),
            "tools_commit":TOOLS_COMMIT,
            "tools_tree":"f".repeat(40),
            "source_date_epoch":0,
            "source_lock_sha256":sha256_bytes(&source_lock),
            "profiles_sha256":sha256_bytes(&profiles),
            "patches":[]
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&recipe).unwrap()
        ));
        let recipe = serde_json::to_vec(&recipe).unwrap();
        let alt_profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2",
            "family":"llvm",
            "upstream_commit":SOURCE_COMMIT,
            "profiles":[
                {
                    "name":"pc-alt-x86_64","configure_target":"pc-alt-x86_64",
                    "upstream_output_target":"pc-alt-x86_64","target_triple":"x86_64-alt-unknown-aros",
                    "cpu":"x86_64","platform":"pc","float_abi":"",
                    "capabilities":["c","cxx","standalone-collector"]
                },
                {
                    "name":"pc-alt-aarch64","configure_target":"pc-alt-aarch64",
                    "upstream_output_target":"pc-alt-aarch64","target_triple":"aarch64-alt-unknown-aros",
                    "cpu":"aarch64","platform":"pc","float_abi":"",
                    "capabilities":["c","cxx","standalone-collector"]
                }
            ]
        }))
        .unwrap();
        let mut alt_recipe = json!({
            "schema":"aros-toolchain-recipe-v2",
            "source_commit":SOURCE_COMMIT,
            "source_tree":"7".repeat(40),
            "producer_commit":PRODUCER_COMMIT,
            "producer_tree":"e".repeat(40),
            "tools_commit":TOOLS_COMMIT,
            "tools_tree":"f".repeat(40),
            "source_date_epoch":0,
            "source_lock_sha256":sha256_bytes(&source_lock),
            "profiles_sha256":sha256_bytes(&alt_profiles),
            "patches":[]
        });
        alt_recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&alt_recipe).unwrap()
        ));
        let alt_recipe = serde_json::to_vec(&alt_recipe).unwrap();
        let identity = serde_json::to_vec(&json!({
            "sdk_id":"llvm-sdk",
            "version":"1.2.3",
            "target":{
                "target_triple":"i686-unknown-aros",
                "abi":{"target_triple":"i686-unknown-aros","data_model":"ilp32"}
            },
            "source":{
                "commit":"0000000000000000000000000000000000000003",
                "git_tree":"000000000000000000000000000000000000000d",
                "content_sha256":"0000000000000000000000000000000000000000000000000000000000000017",
                "contract_sha256":"0000000000000000000000000000000000000000000000000000000000000021"
            },
            "source_date_epoch":1234
        }))
        .unwrap();
        let reference =
            |file: &str, bytes: &[u8]| json!({"file":file,"sha256":sha256_bytes(bytes)});
        let documents = BTreeMap::from([
            ("gnu-rv32-recipe.json".to_owned(), gnu_recipe.clone()),
            (
                "gnu-rv32-source-lock.json".to_owned(),
                gnu_source_lock.clone(),
            ),
            ("gnu-rv32-profiles.json".to_owned(), gnu_profiles.clone()),
            ("llvm-pc-recipe.json".to_owned(), recipe.clone()),
            ("llvm-alt-recipe.json".to_owned(), alt_recipe.clone()),
            ("llvm-pc-source-lock.json".to_owned(), source_lock.clone()),
            ("llvm-alt-source-lock.json".to_owned(), source_lock.clone()),
            ("llvm-pc-profiles.json".to_owned(), profiles.clone()),
            ("llvm-alt-profiles.json".to_owned(), alt_profiles.clone()),
            ("llvm-sdk-identity.json".to_owned(), identity.clone()),
        ]);
        let collection = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-release-inputs-v2",
            "producer_commit":PRODUCER_COMMIT,
            "tools_commit":TOOLS_COMMIT,
            "hosts":ACTIVE_HOSTS,
            "groups":[
                {
                    "id":"gnu-rv32",
                    "recipe":reference("gnu-rv32-recipe.json", &gnu_recipe),
                    "source_lock":reference("gnu-rv32-source-lock.json", &gnu_source_lock),
                    "profiles":reference("gnu-rv32-profiles.json", &gnu_profiles)
                },
                {
                    "id":"llvm-alt",
                    "recipe":reference("llvm-alt-recipe.json", &alt_recipe),
                    "source_lock":reference("llvm-alt-source-lock.json", &source_lock),
                    "profiles":reference("llvm-alt-profiles.json", &alt_profiles)
                },
                {
                    "id":"llvm-pc",
                    "recipe":reference("llvm-pc-recipe.json", &recipe),
                    "source_lock":reference("llvm-pc-source-lock.json", &source_lock),
                    "profiles":reference("llvm-pc-profiles.json", &profiles)
                }
            ],
            "sdks":[{
                "id":"llvm-sdk",
                "identity":reference("llvm-sdk-identity.json", &identity),
                "source_preset":"aros-sdk-source-v1",
                "compiler_profiles":[{"group_id":"llvm-pc","profile":"pc-x86_64"}]
            }]
        }))
        .unwrap();
        ReleaseInputs::parse(&collection, &documents).unwrap()
    }

    #[test]
    fn selected_llvm_sdk_requires_exact_bound_product_context() {
        let inputs = llvm_inputs();
        let identity =
            lane_llvm_sdk_identity(&inputs, Some("llvm-sdk"), false, "llvm-pc", "pc-x86_64")
                .unwrap()
                .unwrap();
        assert_eq!(identity.sdk_id, "llvm-sdk");
        assert!(lane_llvm_sdk_identity(
            &inputs,
            Some("llvm-sdk"),
            false,
            "llvm-alt",
            "pc-alt-x86_64",
        )
        .is_err());
        assert!(
            lane_llvm_sdk_identity(&inputs, Some("llvm-sdk"), false, "llvm-pc", "pc-aarch64",)
                .is_err()
        );
        assert!(
            lane_llvm_sdk_identity(&inputs, Some("other-sdk"), false, "llvm-pc", "pc-x86_64",)
                .is_err()
        );
        assert!(
            lane_llvm_sdk_identity(&inputs, Some("llvm-sdk"), false, "gnu-rv32", "rv32-aros",)
                .is_err()
        );
        assert!(
            lane_llvm_sdk_identity(&inputs, Some("llvm-sdk"), true, "llvm-pc", "pc-x86_64",)
                .is_err()
        );
    }

    #[test]
    fn absent_lane_product_does_not_infer_an_sdk_from_bound_inputs() {
        let inputs = llvm_inputs();
        assert!(
            lane_llvm_sdk_identity(&inputs, None, false, "llvm-pc", "pc-x86_64")
                .unwrap()
                .is_none()
        );
    }
}
