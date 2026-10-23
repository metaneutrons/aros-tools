//! Bounded repeated native observations under one externally selected control.
//!
//! Slot allocation retains failures and supplies no native ordering, complete
//! owner selection or execution authority. The live parent must observe the
//! entire launch result, reject incomplete/foreign slots, and bind all expected
//! owners and prepared inputs before sealing its own producer result.

use super::{observe_with_cancellation, with_observation_cancellation, LaunchControl};
use aros_common::{
    make_native_collection::{
        native_make_call_completion, LEGACY_NATIVE_MAKE_CALL_LIMIT, MAX_NATIVE_MAKE_CALLS,
    },
    make_native_invocation::PrivateNativeCaptureDirectory,
    Sha256Digest,
};
use std::ffi::{OsStr, OsString};

pub(super) fn launch(
    arguments: &[OsString],
    control: &LaunchControl,
    control_sha256: &Sha256Digest,
    parent: &PrivateNativeCaptureDirectory,
    control_name: &OsStr,
) -> Result<(), ()> {
    let (phase, maximum_calls, argument_zero, limit, recipe_controls) = match (
        control.version,
        control.phase,
        control.maximum_calls,
        control.argument_zero.as_deref(),
    ) {
        (3, Some(phase), Some(maximum), None) => {
            (phase, maximum, None, LEGACY_NATIVE_MAKE_CALL_LIMIT, false)
        }
        (4, Some(phase), Some(maximum), Some(value)) => {
            (phase, maximum, Some(value), LEGACY_NATIVE_MAKE_CALL_LIMIT, false)
        }
        (5, Some(phase), Some(maximum), Some(value)) if maximum > LEGACY_NATIVE_MAKE_CALL_LIMIT => {
            (phase, maximum, Some(value), MAX_NATIVE_MAKE_CALLS, false)
        }
        (6, Some(phase @ aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude), Some(maximum), Some(value)) => {
            (phase, maximum, Some(value), MAX_NATIVE_MAKE_CALLS, true)
        }
        _ => return Err(()),
    };
    if !(1..=limit).contains(&maximum_calls) {
        return Err(());
    }
    let collection = PrivateNativeCaptureDirectory::open(&control.output_root).map_err(|_| ())?;
    with_observation_cancellation(|cancellation| {
        if cancellation.is_cancelled() {
            return Err(());
        }
        parent.revalidate().map_err(|_| ())?;
        let (slot, directory) = collection
            .claim_observation_slot_bounded(maximum_calls)
            .map_err(|_| ())?;
        let mut forwarded = vec![
            arguments.first().ok_or(())?.clone(),
            "--observe-make-invocation".into(),
            directory.path().as_os_str().into(),
            control.program.as_os_str().into(),
            control.executable_sha256.to_string().into(),
            control.context_sha256.to_string().into(),
            "--".into(),
        ];
        forwarded.extend_from_slice(arguments.get(5..).ok_or(())?);
        observe_with_cancellation(
            &forwarded,
            phase,
            cancellation,
            argument_zero,
            recipe_controls,
        )?;
        directory.revalidate().map_err(|_| ())?;
        collection.revalidate().map_err(|_| ())?;
        parent
            .read_selected(control_name, control_sha256, 64 * 1024)
            .map_err(|_| ())?;
        if cancellation.is_cancelled() {
            return Err(());
        }
        let status =
            native_make_call_completion(slot, control_sha256, phase, &control.context_sha256)
                .map_err(|_| ())?;
        directory
            .write_new(OsStr::new("call-completed.json"), &status)
            .map_err(|_| ())?;
        directory.revalidate().map_err(|_| ())?;
        collection.revalidate().map_err(|_| ())?;
        parent.revalidate().map_err(|_| ())
    })
}
