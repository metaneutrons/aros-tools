//! Complete family-v2 byte joins and durable recording, never authentication.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path};

use aros_common::publication::{
    publish_atomic_file, validate_existing_directory_prefix_nofollow, AtomicFilePolicy,
    PortableOutputName,
};
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
    readback_indexed_packages, IndexedPackageReadbackRequestV2,
};
use aros_toolchain::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};

use super::qualification_bytes::QualificationArgs;
use super::recovery_family::RecoveryArgs;
use super::release_evidence::EvidenceArgs;
use super::release_evidence_selection::{self, Selection};
use super::{native_error, print_json, RecordQualificationArgs, ResultFormat};

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

#[derive(Clone, Copy)]
enum Operation<'a> {
    Inspect,
    Verify(&'a QualificationArgs),
    Record(&'a RecordQualificationArgs),
    Recovery(&'a RecoveryArgs),
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

fn run_selected(args: &EvidenceArgs, operation: Operation<'_>) -> miette::Result<()> {
    let qualification = match operation {
        Operation::Verify(selected) => Some(selected),
        Operation::Recovery(selected) => Some(&selected.qualification),
        _ => None,
    };
    let selected_qualification = qualification
        .map(|selected| {
            let bytes = read_selected(
                &selected.qualification_evidence,
                &selected.qualification_sha256,
            )?;
            let policy_bytes = read_selected(&selected.policy, &selected.policy_sha256)?;
            let policy: EvidencePolicy = serde_json::from_slice(&policy_bytes).map_err(|_| {
                miette::miette!("qualification policy must match the closed verifier schema")
            })?;
            Ok::<_, miette::Report>((bytes, policy))
        })
        .transpose()?;
    let selected_recovery = if let Operation::Recovery(selected) = operation {
        let bytes = read_selected(
            &selected.recovery_request,
            &selected.recovery_request_sha256,
        )?;
        Some(RecoveryRequestV2::parse(&bytes).map_err(|error| native_error(&error))?)
    } else {
        None
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
    if let Operation::Recovery(selected) = operation {
        require_independent_document(args, &selection, &selected.recovery_request)?;
    }
    if let Operation::Record(selected) = operation {
        recording_preflight(selected, args, &selection)?;
    }
    let packages = IndexedPackageReadbackRequestV2 {
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
            let expected = claims[artifact.asset()]
                .expectations(
                    NativeCompatibilityExpectedPackage {
                        manifest: &package.manifest,
                        archive_sha256: &package.archive_sha256,
                        archive_size: package.archive_size,
                        compiler: artifact.compiler(),
                        source_commit: artifact.source_commit(),
                        host: artifact.host(),
                    },
                    group.profiles(),
                    profiles[artifact.asset()],
                )
                .map_err(|error| native_error(&error))?;
            let selected = &selection.lanes[artifact.asset()].compatibility;
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
    let mut recorded = None;
    let mut recovery_decision = None;
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
        let (evidence, complete) =
            record_qualification_bytes_v2(&QualificationRecordingRequestV2 {
                index_bytes: &index_bytes,
                complete: &complete_request,
                source_run,
                policy,
                created_at: selected.created_at,
                expires_at: selected.expires_at,
            })
            .map_err(|error| native_error(&error))?
            .into_parts();
        recorded = Some(evidence);
        complete
    } else if let Some((bytes, policy)) = &selected_qualification {
        let request = QualificationByteReadbackRequestV2 {
            evidence_bytes: bytes,
            index_bytes: &index_bytes,
            complete: &complete_request,
            policy,
        };
        if let Some(recovery) = &selected_recovery {
            let (decision, joined) = readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
                recovery,
                qualification: &request,
            })
            .map_err(|error| native_error(&error))?
            .into_parts();
            recovery_decision = Some(decision);
            joined.into_complete()
        } else {
            readback_qualification_bytes_v2(&request)
                .map_err(|error| native_error(&error))?
                .into_complete()
        }
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
    if let Some(selected) = qualification {
        read_selected(
            &selected.qualification_evidence,
            &selected.qualification_sha256,
        )?;
        read_selected(&selected.policy, &selected.policy_sha256)?;
    }
    if let Operation::Recovery(selected) = operation {
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
    if let Some(selected) = qualification {
        result["qualification_sha256"] = serde_json::json!(selected.qualification_sha256);
        result["policy_sha256"] = serde_json::json!(selected.policy_sha256);
    }
    if let Operation::Recovery(selected) = operation {
        result["recovery_request_sha256"] = serde_json::json!(selected.recovery_request_sha256);
        result["decision"] = serde_json::to_value(recovery_decision.as_ref().ok_or_else(|| {
            miette::miette!("family-v2 recovery did not reacquire complete evidence")
        })?)
        .map_err(|_| miette::miette!("cannot serialize recovery decision"))?;
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
    if !output.is_absolute()
        || output
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(miette::miette!(
            "qualification output must have an absolute normalized path"
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| miette::miette!("qualification output has no parent"))?;
    let leaf = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| miette::miette!("qualification output has no portable leaf"))?;
    PortableOutputName::new(leaf)
        .map_err(|_| miette::miette!("qualification output has no portable leaf"))?;
    validate_existing_directory_prefix_nofollow(parent).map_err(|_| {
        miette::miette!("qualification output parent must have no symlink components")
    })?;
    if !parent.is_dir() {
        return Err(miette::miette!(
            "qualification output parent must already exist"
        ));
    }
    match std::fs::symlink_metadata(output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(miette::miette!(
                "qualification output already exists or is unavailable"
            ))
        }
    }
    let protected = std::iter::once(args.directory.as_path())
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
        }));
    for path in protected {
        if super::compatibility_export::filesystem_overlap(output, path)? {
            return Err(miette::miette!("qualification output must remain outside all selected release, package, compatibility and input paths"));
        }
    }
    Ok(())
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
    let roots = std::iter::once(args.directory.as_path()).chain(selection.lanes.values().flat_map(
        |lane| {
            lane.builds
                .iter()
                .map(|build| build.package_dir.as_path())
                .chain(std::iter::once(lane.compatibility.directory.as_path()))
        },
    ));
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
