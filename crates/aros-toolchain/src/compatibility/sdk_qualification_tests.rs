//! Qualification and recovery round-trips for complete synthetic SDK matrices.
//!
//! These assertions join claims to fixture bytes only. They do not prove
//! signatures, provider observations, or three-host execution.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;

use aros_common::{sha256_bytes, Sha256Digest};

use crate::native_candidate::ReleaseCompatibilityReadbackRequestV2;
use crate::qualification_evidence::EvidencePolicy;
use crate::qualification_evidence_v2::{QualificationEvidenceV2, SourceRunIdentityV2};
use crate::qualification_readback_v2::{
    readback_qualification_bytes_v2, QualificationByteReadbackRequestV2,
};
use crate::qualification_recording_v2::{
    record_qualification_bytes_v2, QualificationRecordingRequestV2,
};
use crate::recipe::GitObjectId;
use crate::recovery::{FailedStage, ObservedTag, RecoveryOperation};
use crate::recovery_v2::{
    readback_recovery_bytes_v2, RecoveryByteReadbackRequestV2, RecoveryRequestV2,
    RECOVERY_V2_SCHEMA,
};
use crate::release_index_v2::INDEX_NAME;
use crate::sdk_generation::SdkGenerationReadbackRequest;
use crate::sdk_generation_lanes::ExpectedSdkGenerationLane;
use crate::sdk_generation_matrix::{
    readback_sdk_generation_matrix, SdkGenerationMatrixReadbackRequest,
};

/// Exercise qualification and recovery against an already-owned complete
/// three-host fixture matrix. No opaque read-back is constructed by the test.
pub(super) fn probe(matrix: &SdkGenerationMatrixReadbackRequest<'_, '_>) {
    assert_eq!(
        matrix.generations.len(),
        3,
        "fixture must contain three SDK hosts"
    );
    let complete = matrix.complete;
    let packages = &complete.builds.packages;
    let index_bytes = fs::read(packages.directory.join(INDEX_NAME))
        .expect("selected release index should remain readable");
    let policy = policy();
    let selection = Selection {
        complete,
        matrix,
        index_bytes: &index_bytes,
        policy: &policy,
    };
    let selected_run = source_run(complete, &policy);
    let evidence = record_and_roundtrip(&selection, &selected_run);
    let evidence_bytes = serde_json::to_vec(&evidence).unwrap();

    let mutated = reject_missing_and_malformed_claims(&selection, &evidence, &selected_run);
    reject_wrong_matrix_selections(&selection, &evidence);
    reject_changed_generation_record(&selection, &evidence_bytes);
    recovery_roundtrip_and_rejections(&selection, &evidence, &evidence_bytes, &mutated);
}

struct Selection<'a, 'e> {
    complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    matrix: &'a SdkGenerationMatrixReadbackRequest<'a, 'e>,
    index_bytes: &'a [u8],
    policy: &'a EvidencePolicy,
}

fn record_and_roundtrip(
    selection: &Selection<'_, '_>,
    source_run: &SourceRunIdentityV2,
) -> QualificationEvidenceV2 {
    let recording = recording_request(selection, Some(selection.matrix), source_run.clone());
    let recorded = record_qualification_bytes_v2(&recording)
        .expect("complete SDK matrix should produce SDK qualification claims");
    assert_eq!(recorded.sdk_generations().len(), 3);
    let (evidence, _, retained_sdk) = recorded.into_parts();
    assert_eq!(
        retained_sdk.len(),
        3,
        "recording should retain all SDK tuples"
    );
    assert_eq!(evidence.sdk_generations.len(), 3);
    let evidence_bytes = serde_json::to_vec(&evidence).unwrap();
    let joined = readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
        evidence_bytes: &evidence_bytes,
        index_bytes: selection.index_bytes,
        complete: selection.complete,
        sdk: Some(selection.matrix),
        policy: selection.policy,
    })
    .expect("serialized SDK claims should join to the complete selected matrix");
    assert_eq!(joined.evidence(), &evidence);
    assert_eq!(joined.sdk_generations().len(), 3);
    assert_eq!(joined.into_parts().1.len(), 3);
    evidence
}

fn reject_missing_and_malformed_claims(
    selection: &Selection<'_, '_>,
    evidence: &QualificationEvidenceV2,
    source_run: &SourceRunIdentityV2,
) -> QualificationEvidenceV2 {
    let missing_recording = recording_request(selection, None, source_run.clone());
    assert!(
        record_qualification_bytes_v2(&missing_recording).is_err(),
        "SDK-bearing inputs must reject recording without the selected matrix"
    );
    let evidence_bytes = serde_json::to_vec(evidence).unwrap();
    reject_qualification(
        &evidence_bytes,
        selection,
        None,
        "SDK-bearing inputs must reject read-back without the selected matrix",
    );

    let mut missing = evidence.clone();
    missing.sdk_generations.pop();
    reject_claims(&missing, selection);
    let mut extra = evidence.clone();
    extra.sdk_generations.push(extra.sdk_generations[0].clone());
    reject_claims(&extra, selection);
    let mut reordered = evidence.clone();
    reordered.sdk_generations.reverse();
    reject_claims(&reordered, selection);
    let mut mutated = evidence.clone();
    mutated.sdk_generations[0].generation_record_sha256 =
        digest_other_than(&mutated.sdk_generations[0].generation_record_sha256);
    reject_claims(&mutated, selection);
    mutated
}

fn reject_wrong_matrix_selections(
    selection: &Selection<'_, '_>,
    evidence: &QualificationEvidenceV2,
) {
    let alternate_complete = ReleaseCompatibilityReadbackRequestV2 {
        builds: selection.complete.builds,
        lanes: selection.complete.lanes.clone(),
    };
    let alternate_matrix = SdkGenerationMatrixReadbackRequest {
        complete: &alternate_complete,
        generations: copy_generations(selection.matrix),
    };
    let recording = recording_request(
        selection,
        Some(&alternate_matrix),
        evidence.source_run.clone(),
    );
    assert!(
        record_qualification_bytes_v2(&recording).is_err(),
        "SDK selection must refer to the exact selected compiler request"
    );

    let mut wrong_tuple = copy_generations(selection.matrix);
    let tuple_keys = wrong_tuple.keys().cloned().collect::<Vec<_>>();
    let first = wrong_tuple.remove(&tuple_keys[0]).unwrap();
    let second = wrong_tuple.remove(&tuple_keys[1]).unwrap();
    wrong_tuple.insert(tuple_keys[0].clone(), second);
    wrong_tuple.insert(tuple_keys[1].clone(), first);
    assert!(
        readback_sdk_generation_matrix(&SdkGenerationMatrixReadbackRequest {
            complete: selection.complete,
            generations: wrong_tuple,
        })
        .is_err(),
        "matrix read-back must reject selections under another input tuple"
    );

    let mut wrong_hash = copy_generations(selection.matrix);
    let hash_keys = selection
        .matrix
        .generations
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let selected = selection.matrix.generations.get(&hash_keys[0]).unwrap();
    let other_hash = selection
        .matrix
        .generations
        .get(&hash_keys[1])
        .unwrap()
        .record_sha256;
    wrong_hash.insert(
        hash_keys[0].clone(),
        SdkGenerationReadbackRequest {
            inputs: selected.inputs,
            sdk_id: selected.sdk_id,
            compiler_group_id: selected.compiler_group_id,
            record_bytes: selected.record_bytes,
            record_sha256: other_hash,
            package: selected.package,
            compatibility: selected.compatibility,
        },
    );
    assert!(
        readback_sdk_generation_matrix(&SdkGenerationMatrixReadbackRequest {
            complete: selection.complete,
            generations: wrong_hash,
        })
        .is_err(),
        "matrix read-back must reject a substituted generation-record hash"
    );
}

fn reject_changed_generation_record(selection: &Selection<'_, '_>, evidence_bytes: &[u8]) {
    let selected_lane = selection.matrix.generations.keys().next().unwrap();
    let selected_generation = selection.matrix.generations.get(selected_lane).unwrap();
    let mut changed_record_bytes = selected_generation.record_bytes.to_vec();
    changed_record_bytes.push(b' ');
    let changed_record_sha256 = sha256_bytes(&changed_record_bytes);
    let changed_complete = copy_complete(selection.complete);
    let changed_generations = generations_with_changed_record(
        selection.matrix,
        selected_lane,
        &changed_record_bytes,
        &changed_record_sha256,
    );
    let changed_matrix = SdkGenerationMatrixReadbackRequest {
        complete: &changed_complete,
        generations: changed_generations,
    };
    let qualification = QualificationByteReadbackRequestV2 {
        evidence_bytes,
        index_bytes: selection.index_bytes,
        complete: &changed_complete,
        sdk: Some(&changed_matrix),
        policy: selection.policy,
    };
    assert!(
        readback_qualification_bytes_v2(&qualification).is_err(),
        "qualification must reject changed generation-record bytes in an isolated copy"
    );
    let recovery = recovery_request(evidence_bytes);
    assert!(
        readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
            recovery: &recovery,
            qualification: &qualification,
        })
        .is_err(),
        "recovery must reacquire and reject changed generation-record bytes"
    );
}

fn recovery_roundtrip_and_rejections(
    selection: &Selection<'_, '_>,
    evidence: &QualificationEvidenceV2,
    evidence_bytes: &[u8],
    mutated: &QualificationEvidenceV2,
) {
    let recovery = recovery_request(evidence_bytes);
    let qualification = qualification_request(selection, evidence_bytes, Some(selection.matrix));
    let recovered = readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
        recovery: &recovery,
        qualification: &qualification,
    })
    .expect("recovery should reacquire the original complete SDK matrix");
    assert_eq!(recovered.qualification().sdk_generations().len(), 3);

    let missing_sdk = qualification_request(selection, evidence_bytes, None);
    assert!(
        readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
            recovery: &recovery,
            qualification: &missing_sdk,
        })
        .is_err(),
        "recovery must reject qualification without reacquiring its SDK matrix"
    );

    // Recovery hashes this isolated receipt copy afresh, then checks its SDK
    // claim against measurements reacquired from the selected generation bytes.
    assert_ne!(mutated.sdk_generations, evidence.sdk_generations);
    let mutated_bytes = serde_json::to_vec(mutated).unwrap();
    let mutated_recovery = recovery_request(&mutated_bytes);
    let mutated_qualification =
        qualification_request(selection, &mutated_bytes, Some(selection.matrix));
    assert!(
        readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
            recovery: &mutated_recovery,
            qualification: &mutated_qualification,
        })
        .is_err(),
        "recovery must reject a mutated SDK qualification receipt"
    );
}

fn recording_request<'a, 'e>(
    selection: &'a Selection<'a, 'e>,
    matrix: Option<&'a SdkGenerationMatrixReadbackRequest<'a, 'e>>,
    source_run: SourceRunIdentityV2,
) -> QualificationRecordingRequestV2<'a, 'e> {
    QualificationRecordingRequestV2 {
        index_bytes: selection.index_bytes,
        complete: selection.complete,
        sdk: matrix,
        source_run,
        policy: selection.policy.clone(),
        created_at: 100,
        expires_at: 300,
    }
}

const fn qualification_request<'a, 'e>(
    selection: &'a Selection<'a, 'e>,
    evidence_bytes: &'a [u8],
    matrix: Option<&'a SdkGenerationMatrixReadbackRequest<'a, 'e>>,
) -> QualificationByteReadbackRequestV2<'a, 'e> {
    QualificationByteReadbackRequestV2 {
        evidence_bytes,
        index_bytes: selection.index_bytes,
        complete: selection.complete,
        sdk: matrix,
        policy: selection.policy,
    }
}

fn policy() -> EvidencePolicy {
    EvidencePolicy {
        source_repository: "https://github.com/example/aros-toolchains".into(),
        source_workflow: ".github/workflows/qualification.yml".into(),
        signer_repository: "https://github.com/example/aros-toolchains".into(),
        signer_workflow: ".github/workflows/qualification.yml".into(),
        signer: "github-actions".into(),
        now: 150,
    }
}

fn source_run(
    complete: &ReleaseCompatibilityReadbackRequestV2<'_>,
    policy: &EvidencePolicy,
) -> SourceRunIdentityV2 {
    SourceRunIdentityV2 {
        repository: policy.source_repository.clone(),
        workflow: policy.source_workflow.clone(),
        run_id: 42,
        run_attempt: 1,
        producer_commit: complete.builds.packages.index.producer_commit().clone(),
        source_tag: "release-test".into(),
        tag_object: GitObjectId::try_from("9a".repeat(20)).unwrap(),
    }
}

fn reject_claims(evidence: &QualificationEvidenceV2, selection: &Selection<'_, '_>) {
    let evidence_bytes = serde_json::to_vec(evidence).unwrap();
    reject_qualification(
        &evidence_bytes,
        selection,
        Some(selection.matrix),
        "SDK qualification claims must match complete measured tuples and bytes",
    );
}

fn reject_qualification(
    evidence_bytes: &[u8],
    selection: &Selection<'_, '_>,
    matrix: Option<&SdkGenerationMatrixReadbackRequest<'_, '_>>,
    message: &str,
) {
    assert!(
        readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
            evidence_bytes,
            index_bytes: selection.index_bytes,
            complete: selection.complete,
            sdk: matrix,
            policy: selection.policy,
        })
        .is_err(),
        "{message}"
    );
}

fn recovery_request(evidence_bytes: &[u8]) -> RecoveryRequestV2 {
    let evidence = QualificationEvidenceV2::parse(evidence_bytes).unwrap();
    RecoveryRequestV2 {
        schema: RECOVERY_V2_SCHEMA.to_owned(),
        qualification_sha256: sha256_bytes(evidence_bytes),
        operation: RecoveryOperation::CompatibilityReplay,
        failed_stage: FailedStage::CompatibilityHarness,
        observed_run: evidence.source_run.clone(),
        source_tag: ObservedTag {
            name: evidence.source_run.source_tag.clone(),
            tag_object: evidence.source_run.tag_object.clone(),
            peeled_commit: evidence.source_run.producer_commit.clone(),
        },
        observed_attestation: Some(evidence.attestation),
        handoff: None,
    }
}

fn digest_other_than(digest: &Sha256Digest) -> Sha256Digest {
    let candidate = Sha256Digest::parse(&"0".repeat(64)).unwrap();
    if &candidate == digest {
        Sha256Digest::parse(&"1".repeat(64)).unwrap()
    } else {
        candidate
    }
}

const fn copy_generation<'e>(
    selected: &SdkGenerationReadbackRequest<'e>,
) -> SdkGenerationReadbackRequest<'e> {
    generation_with_record(selected, selected.record_bytes, selected.record_sha256)
}

const fn generation_with_record<'e>(
    selected: &SdkGenerationReadbackRequest<'e>,
    record_bytes: &'e [u8],
    record_sha256: &'e Sha256Digest,
) -> SdkGenerationReadbackRequest<'e> {
    SdkGenerationReadbackRequest {
        inputs: selected.inputs,
        sdk_id: selected.sdk_id,
        compiler_group_id: selected.compiler_group_id,
        record_bytes,
        record_sha256,
        package: selected.package,
        compatibility: selected.compatibility,
    }
}

fn copy_complete<'a, 'e>(
    complete: &ReleaseCompatibilityReadbackRequestV2<'e>,
) -> ReleaseCompatibilityReadbackRequestV2<'a>
where
    'e: 'a,
{
    ReleaseCompatibilityReadbackRequestV2 {
        builds: complete.builds,
        lanes: complete.lanes.clone(),
    }
}

fn generations_with_changed_record<'a, 'e>(
    matrix: &SdkGenerationMatrixReadbackRequest<'_, 'e>,
    selected_lane: &ExpectedSdkGenerationLane,
    changed_record_bytes: &'a [u8],
    changed_record_sha256: &'a Sha256Digest,
) -> BTreeMap<ExpectedSdkGenerationLane, SdkGenerationReadbackRequest<'a>>
where
    'e: 'a,
{
    matrix
        .generations
        .iter()
        .map(|(lane, selected)| {
            let selected: &SdkGenerationReadbackRequest<'a> = selected;
            let (record_bytes, record_sha256) = if lane == selected_lane {
                (changed_record_bytes, changed_record_sha256)
            } else {
                (selected.record_bytes, selected.record_sha256)
            };
            (
                lane.clone(),
                generation_with_record(selected, record_bytes, record_sha256),
            )
        })
        .collect()
}

fn copy_generations<'e>(
    matrix: &SdkGenerationMatrixReadbackRequest<'_, 'e>,
) -> BTreeMap<ExpectedSdkGenerationLane, SdkGenerationReadbackRequest<'e>> {
    matrix
        .generations
        .iter()
        .map(|(lane, selected)| (lane.clone(), copy_generation(selected)))
        .collect()
}
