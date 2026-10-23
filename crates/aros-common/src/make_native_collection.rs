//! Closed, independently selected collection readback; never execution authority.
//!
//! Call this only after the supervising parent has reaped the complete launch
//! tree. Slot numbers describe allocation, not native execution order. A caller
//! still binds the parent, executable/argv0, expected owners and prepared input
//! closure before sealing its live result. Files cannot supply those decisions.

use crate::{
    directory_entry_names_nofollow_bounded,
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    sha256_bytes, CancellationToken, Sha256Digest,
};
use std::{collections::BTreeSet, ffi::OsStr, time::Instant};

/// Call slots supported by the legacy observation allocation API.
pub const LEGACY_NATIVE_MAKE_CALL_LIMIT: usize = 256;
/// Maximum number of native Make calls accepted by bounded collection APIs.
pub const MAX_NATIVE_MAKE_CALLS: usize = 4096;
/// Maximum aggregate bytes retained from observation packets in one collection.
pub const MAX_NATIVE_MAKE_COLLECTION_BYTES: usize = 1024 * 1024 * 1024;

const MAX_NATIVE_MAKE_PACKET_BYTES: usize = 4 * 1024 * 1024;

/// One independently selected packet. Do not obtain the expected binding merely
/// by trusting `observation-selection.json` in the collection being admitted.
pub struct NativeMakeCallSelection {
    pub document_sha256: Sha256Digest,
    pub binding: MakeObservationBinding,
}

/// Encode the producer's exact success-only record from the selected context.
/// The record is inert data, not proof of successful ordinary Make execution.
///
/// # Errors
/// Refuses an out-of-bounds slot or an encoding failure.
pub fn native_make_call_completion(
    slot: usize,
    control_sha256: &Sha256Digest,
    phase: MakeObservationPhase,
    context_sha256: &Sha256Digest,
) -> Result<Vec<u8>, String> {
    if slot >= MAX_NATIVE_MAKE_CALLS {
        return Err("native observation slot exceeds bound".into());
    }
    serde_json::to_vec(&serde_json::json!({
        "schema":"aros-native-make-call-v1",
        "qualification":"instrumented-observation-data-only",
        "slot":slot,"phase":phase,"control_sha256":control_sha256,
        "context_sha256":context_sha256,
    }))
    .map_err(|_| "cannot encode native observation completion".into())
}

/// Read exactly the selected nonempty set of successful slots.
///
/// Refuses failed, missing, extra and foreign slots rather than filtering them out. All file
/// reads retain the existing no-follow/owned/private/single-link checks.
///
/// This proves selected data consistency only. It does not stop concurrent
/// writers, authenticate a same-uid process, or infer the expected calls. The
/// supervisor must stop/reap writers and supply the complete independent set.
///
/// # Errors
/// Refuses incomplete/mismatched data, unsafe paths, changed namespace, exhausted
/// explicit deadline, cancellation, or a selection outside 1..=4096 calls or
/// an aggregate packet byte count above 1 GiB.
pub fn read_native_make_collection(
    root: &PrivateNativeCaptureDirectory,
    control_sha256: &Sha256Digest,
    expected: &[NativeMakeCallSelection],
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Vec<RetainedMakeObservation>, String> {
    if !(1..=MAX_NATIVE_MAKE_CALLS).contains(&expected.len()) {
        return Err("native observation collection selection exceeds bound".into());
    }
    let names: BTreeSet<_> = (0..expected.len())
        .flat_map(|slot| [format!("call-{slot:04}"), format!("call-{slot:04}.claim")])
        .map(Into::into)
        .collect();
    check_namespace(root, &names, deadline, cancellation)?;
    let mut observations = Vec::with_capacity(expected.len());
    let mut aggregate_packet_bytes = 0usize;
    for (slot, selected) in expected.iter().enumerate() {
        check_control(deadline, cancellation)?;
        root.read_selected(
            OsStr::new(&format!("call-{slot:04}.claim")),
            &sha256_bytes(b""),
            1,
        )
        .map_err(|_| "native observation collection claim differs")?;
        let call =
            PrivateNativeCaptureDirectory::open(&root.path().join(format!("call-{slot:04}")))
                .map_err(|_| "native observation collection child is unsafe")?;
        let completion = native_make_call_completion(
            slot,
            control_sha256,
            selected.binding.phase,
            &selected.binding.context_sha256,
        )?;
        call.read_selected(
            OsStr::new("call-completed.json"),
            &sha256_bytes(&completion),
            4096,
        )
        .map_err(|_| "native observation collection completion differs")?;
        let remaining_bytes = MAX_NATIVE_MAKE_COLLECTION_BYTES
            .checked_sub(aggregate_packet_bytes)
            .ok_or("native observation collection byte limit exceeded")?;
        if remaining_bytes == 0 {
            return Err("native observation collection byte limit exceeded".into());
        }
        let bytes = call
            .read_selected(
                OsStr::new("observation.json"),
                &selected.document_sha256,
                MAX_NATIVE_MAKE_PACKET_BYTES.min(remaining_bytes),
            )
            .map_err(|_| "native observation collection packet differs")?;
        aggregate_packet_bytes =
            checked_native_make_collection_packet_bytes(aggregate_packet_bytes, bytes.len())?;
        let observation =
            RetainedMakeObservation::load(&bytes, &selected.document_sha256, &selected.binding)?;
        call.revalidate()
            .map_err(|_| "native observation child changed")?;
        observations.push(observation);
    }
    check_namespace(root, &names, deadline, cancellation)?;
    Ok(observations)
}

fn checked_native_make_collection_packet_bytes(
    aggregate_bytes: usize,
    packet_bytes: usize,
) -> Result<usize, String> {
    aggregate_bytes
        .checked_add(packet_bytes)
        .filter(|total| *total <= MAX_NATIVE_MAKE_COLLECTION_BYTES)
        .ok_or_else(|| "native observation collection byte limit exceeded".into())
}

fn check_control(deadline: Instant, cancellation: &CancellationToken) -> Result<(), String> {
    if cancellation.is_cancelled() || Instant::now() >= deadline {
        return Err("native observation collection readback interrupted".into());
    }
    Ok(())
}

fn check_namespace(
    root: &PrivateNativeCaptureDirectory,
    expected: &BTreeSet<std::ffi::OsString>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    check_control(deadline, cancellation)?;
    root.revalidate()
        .map_err(|_| "native observation root changed")?;
    let names = directory_entry_names_nofollow_bounded(root.path(), expected.len())
        .map_err(|_| "native observation collection namespace exceeds selection")?;
    root.revalidate()
        .map_err(|_| "native observation root changed")?;
    if names.into_iter().collect::<BTreeSet<_>>() != *expected {
        return Err("native observation collection namespace differs from selection".into());
    }
    check_control(deadline, cancellation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_packet_budget_checks_boundary_and_overflow_without_large_allocations() {
        let packet_bytes = 4 * 1024 * 1024;
        let exactly_at_limit = checked_native_make_collection_packet_bytes(
            MAX_NATIVE_MAKE_COLLECTION_BYTES - packet_bytes,
            packet_bytes,
        )
        .expect("exact aggregate byte limit is accepted");
        assert_eq!(exactly_at_limit, MAX_NATIVE_MAKE_COLLECTION_BYTES);
        assert!(checked_native_make_collection_packet_bytes(exactly_at_limit, 1).is_err());
        assert!(checked_native_make_collection_packet_bytes(usize::MAX, 1).is_err());
    }

    #[test]
    fn completion_accepts_last_bounded_slot_and_rejects_the_next() {
        let digest = sha256_bytes(b"selected context");
        let completion = native_make_call_completion(
            MAX_NATIVE_MAKE_CALLS - 1,
            &digest,
            MakeObservationPhase::PreFirstFile,
            &digest,
        )
        .expect("last bounded call slot is valid");
        let document: serde_json::Value =
            serde_json::from_slice(&completion).expect("completion is JSON");
        assert_eq!(document["slot"], MAX_NATIVE_MAKE_CALLS - 1);
        assert!(native_make_call_completion(
            MAX_NATIVE_MAKE_CALLS,
            &digest,
            MakeObservationPhase::PreFirstFile,
            &digest,
        )
        .is_err());
    }
}
