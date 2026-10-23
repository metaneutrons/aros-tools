//! Readback of a stopped collection never silently drops failed/foreign slots.

use super::*;
use aros_common::{
    make_native_collection::{read_native_make_collection, NativeMakeCallSelection},
    CancellationToken,
};
use std::time::{Duration, Instant};

fn selected_call(
    root: &Path,
    slot: usize,
    expected: &NativeMakeInvocation,
) -> NativeMakeCallSelection {
    let call = root.join(call_name(slot));
    let stdout = fs::read(call.join("initial.stdout")).expect("retained exact stdout");
    let invocation = expected.to_bytes().expect("independent invocation bytes");
    NativeMakeCallSelection {
        document_sha256: sha256_bytes(&fs::read(call.join("observation.json")).expect("packet")),
        binding: MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256: super::super::context_digest(),
            invocation_sha256: sha256_bytes(&invocation),
            stdout_sha256: sha256_bytes(&stdout),
        },
    }
}

fn read(
    output: &Path,
    control: &Sha256Digest,
    selected: &[NativeMakeCallSelection],
) -> Result<Vec<RetainedMakeObservation>, String> {
    read_native_make_collection(
        &PrivateNativeCaptureDirectory::open(output).expect("held private parent"),
        control,
        selected,
        Instant::now() + Duration::from_secs(5),
        &CancellationToken::default(),
    )
}

#[test]
fn closed_readback_binds_all_calls_and_refuses_missing_foreign_or_relabelled_data() {
    let temporary = tempfile::tempdir().expect("readback fixture");
    let root = super::super::private_root(&temporary);
    let output = collection_root(&root);
    let cwd = root.join("working");
    private_directory(&cwd);
    synthetic_makefile(&cwd);
    let program = super::super::find_make_program();
    let (control_path, control) = write_control(
        &root,
        &control_value(
            &output,
            &program,
            &super::super::measure_executable(&program),
            2,
        ),
    );
    let environment = make_environment(&root, "private-readback-value");
    let mut selected = Vec::new();
    for slot in 0..2 {
        let arguments = valid_arguments(&format!("CALL_VALUE={slot}"));
        let expected = expected_invocation(&arguments, &cwd, &environment);
        assert!(
            run_collection(&control_path, &control, &cwd, &arguments, &environment)
                .status
                .success()
        );
        selected.push(selected_call(&output, slot, &expected));
    }
    assert_eq!(
        read(&output, &control, &selected)
            .expect("whole selected set")
            .len(),
        2
    );
    assert!(read(&output, &control, &[]).is_err());
    assert!(read(&output, &control, &selected[..1]).is_err());
    assert!(read(&output, &sha256_bytes(b"foreign control"), &selected).is_err());
    selected[1].binding.context_sha256 = sha256_bytes(b"foreign context");
    assert!(read(&output, &control, &selected).is_err());
    selected[1].binding.context_sha256 = super::super::context_digest();
    selected[1].binding.invocation_sha256 = sha256_bytes(b"foreign invocation");
    assert!(read(&output, &control, &selected).is_err());
    let expected = expected_invocation(&valid_arguments("CALL_VALUE=1"), &cwd, &environment);
    selected[1] = selected_call(&output, 1, &expected);
    assert!(read(&output, &control, &selected).is_ok());
    let original_digest = selected[1].document_sha256.clone();
    selected[1].document_sha256 = sha256_bytes(b"foreign packet");
    assert!(read(&output, &control, &selected).is_err());
    selected[1].document_sha256 = original_digest;
    let packet = output.join("call-0001/observation.json");
    let original_packet = fs::read(&packet).expect("packet bytes");
    private_file(&packet, b"{}");
    assert!(read(&output, &control, &selected).is_err());
    private_file(&packet, &original_packet);
    assert!(read(&output, &control, &selected).is_ok());

    let completion = output.join("call-0001/call-completed.json");
    let original = fs::read(&completion).expect("completion bytes");
    private_file(&completion, b"{}");
    assert!(read(&output, &control, &selected).is_err());
    private_file(&completion, &original);
    assert!(read(&output, &control, &selected).is_ok());

    // A later failed slot remains evidence; it cannot be omitted by selecting
    // only the two successful packets from the same parent namespace.
    PrivateNativeCaptureDirectory::open(&output)
        .expect("parent")
        .claim_observation_slot(3)
        .expect("retained incomplete slot");
    assert!(read(&output, &control, &selected).is_err());
    assert_no_source_effects(&cwd);
}

#[test]
fn readback_refuses_expired_cancelled_and_linked_claims_without_modification() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().expect("readback controls fixture");
    let root = super::super::private_root(&temporary);
    let output = collection_root(&root);
    let parent = PrivateNativeCaptureDirectory::open(&output).expect("parent");
    let (_, child) = parent.claim_observation_slot(1).expect("claim");
    let selection = NativeMakeCallSelection {
        document_sha256: sha256_bytes(b"missing packet"),
        binding: MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256: super::super::context_digest(),
            invocation_sha256: sha256_bytes(b"invocation"),
            stdout_sha256: sha256_bytes(b"stdout"),
        },
    };
    let selected = [selection];
    let control = sha256_bytes(b"control");
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    let before = collection_snapshot(&output);
    let cancelled_result = read_native_make_collection(
        &parent,
        &control,
        &selected,
        Instant::now() + Duration::from_secs(5),
        &cancelled,
    );
    assert!(
        matches!(cancelled_result, Err(message) if message == "native observation collection readback interrupted")
    );
    let expired_result = read_native_make_collection(
        &parent,
        &control,
        &selected,
        Instant::now(),
        &CancellationToken::default(),
    );
    assert!(
        matches!(expired_result, Err(message) if message == "native observation collection readback interrupted")
    );
    assert!(read(&output, &control, &selected).is_err()); // No success record.
    assert_eq!(collection_snapshot(&output), before);
    drop(child);
    let claim = output.join("call-0000.claim");
    fs::rename(&claim, root.join("original-claim")).expect("retain claim outside collection");
    symlink(root.join("original-claim"), &claim).expect("linked claim");
    assert!(read(&output, &control, &selected).is_err());
}
