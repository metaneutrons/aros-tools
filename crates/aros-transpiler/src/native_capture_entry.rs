//! Private launcher entries before normal diagnostics/logging.
//!
//! Capture never runs a child. Explicit observation runs only the pinned GNU
//! initial-state sampler, which must stop before reading selected Make inputs.
//! All files, including a selection written during a late failure, remain
//! uncompleted data unless the supervising parent observes frontend exit0 and
//! binds the retained bytes to that run. File existence is never completion.
//! SIGINT/SIGTERM cancel and reap the controlled child group. This is not a
//! portable SIGKILL, crash or machine-failure cleanup guarantee.

use std::{ffi::OsString, path::Path, process::ExitCode};

#[cfg(unix)]
#[path = "native_capture_collection.rs"]
mod collection;

pub fn try_capture(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.get(1).is_some_and(|argument| {
        argument == "--observe-make-invocation"
            || argument == "--observe-make-launch"
            || argument == "--observe-make-launch-collection"
    }) {
        return Some(observe_entry(arguments));
    }
    if arguments
        .get(1)
        .is_none_or(|argument| argument != "--capture-make-invocation")
    {
        return None;
    }
    let result = arguments
        .get(2)
        .filter(|_| arguments.get(3).is_some_and(|separator| separator == "--"))
        .ok_or(())
        .and_then(|output| {
            #[cfg(unix)]
            {
                aros_common::make_native_invocation::capture_current(
                    Path::new(output),
                    &arguments[4..],
                )
                .map_err(|_| ())
            }
            #[cfg(not(unix))]
            {
                let _ = output;
                Err(())
            }
        });
    Some(if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        // Do not echo paths, argv, environment, or a nested I/O error.
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            b"native Make invocation capture refused\n",
        );
        ExitCode::FAILURE
    })
}

fn observe_entry(arguments: &[OsString]) -> ExitCode {
    #[cfg(unix)]
    let result = if arguments.get(1).is_some_and(|value| {
        value == "--observe-make-launch" || value == "--observe-make-launch-collection"
    }) {
        observe_launch(arguments)
    } else {
        observe(
            arguments,
            aros_common::make_native_observation::MakeObservationPhase::PreFirstFile,
        )
    };
    #[cfg(not(unix))]
    let result: Result<(), ()> = {
        let _ = arguments;
        Err(())
    };
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            b"native Make initial observation refused\n",
        );
        ExitCode::FAILURE
    }
}

#[cfg(unix)]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchControl {
    version: u32,
    output_root: std::path::PathBuf,
    program: std::path::PathBuf,
    executable_sha256: aros_common::Sha256Digest,
    context_sha256: aros_common::Sha256Digest,
    #[serde(default, deserialize_with = "explicit_phase")]
    phase: Option<aros_common::make_native_observation::MakeObservationPhase>,
    #[serde(default, deserialize_with = "explicit_maximum")]
    maximum_calls: Option<usize>,
    #[serde(default, deserialize_with = "explicit_argument_zero")]
    argument_zero: Option<String>,
}

#[cfg(unix)]
fn explicit_argument_zero<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    // The enclosing selected control is already bounded to 64 KiB; the
    // invocation model separately enforces its combined argv byte budget.
    if value.is_empty() || value.contains('\0') {
        return Err(serde::de::Error::custom(
            "unsupported selected argument zero",
        ));
    }
    Ok(Some(value))
}

#[cfg(unix)]
fn explicit_phase<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<aros_common::make_native_observation::MakeObservationPhase>, D::Error> {
    <aros_common::make_native_observation::MakeObservationPhase as serde::Deserialize>::deserialize(
        deserializer,
    )
    .map(Some)
}

#[cfg(unix)]
fn explicit_maximum<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<usize>, D::Error> {
    <usize as serde::Deserialize>::deserialize(deserializer).map(Some)
}

/// Short, digest-bound command form for native launchers with line limits.
/// The caller supplies the exact manifest SHA, not just a self-describing path.
#[cfg(unix)]
fn observe_launch(arguments: &[OsString]) -> Result<(), ()> {
    use aros_common::make_native_observation::MakeObservationPhase;
    use aros_common::{make_native_invocation::PrivateNativeCaptureDirectory, Sha256Digest};
    if arguments.get(4).is_none_or(|value| value != "--") {
        return Err(());
    }
    let path = Path::new(arguments.get(2).ok_or(())?);
    let expected = Sha256Digest::parse(
        arguments
            .get(3)
            .and_then(|value| value.to_str())
            .ok_or(())?,
    )
    .map_err(|_| ())?;
    let parent = PrivateNativeCaptureDirectory::open(path.parent().ok_or(())?).map_err(|_| ())?;
    let bytes = parent
        .read_selected(path.file_name().ok_or(())?, &expected, 64 * 1024)
        .map_err(|_| ())?;
    let control: LaunchControl = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if arguments
        .get(1)
        .is_some_and(|value| value == "--observe-make-launch-collection")
    {
        return collection::launch(
            arguments,
            &control,
            &expected,
            &parent,
            path.file_name().ok_or(())?,
        );
    }
    let phase = match (
        control.version,
        control.phase,
        control.maximum_calls,
        control.argument_zero,
    ) {
        (1, None, None, None) => MakeObservationPhase::PreFirstFile,
        (2, Some(phase), None, None) => phase,
        _ => return Err(()),
    };
    let mut forwarded = vec![
        arguments[0].clone(),
        "--observe-make-invocation".into(),
        control.output_root.into_os_string(),
        control.program.into_os_string(),
        control.executable_sha256.to_string().into(),
        control.context_sha256.to_string().into(),
        "--".into(),
    ];
    forwarded.extend_from_slice(&arguments[5..]);
    parent.revalidate().map_err(|_| ())?;
    observe(&forwarded, phase)
}

#[cfg(unix)]
fn observe(
    arguments: &[OsString],
    phase: aros_common::make_native_observation::MakeObservationPhase,
) -> Result<(), ()> {
    with_observation_cancellation(|cancellation| {
        observe_with_cancellation(arguments, phase, cancellation, None, false)
    })
}

#[cfg(unix)]
fn observe_with_cancellation(
    arguments: &[OsString],
    phase: aros_common::make_native_observation::MakeObservationPhase,
    cancellation: &aros_common::CancellationToken,
    argument_zero: Option<&str>,
    recipe_controls: bool,
) -> Result<(), ()> {
    use aros_common::{
        make_native_invocation::{
            capture_current_with_selected_argument_zero, capture_current_with_snapshot,
            PrivateNativeCaptureDirectory,
        },
        make_native_observation::MakeObservationPhase,
        make_native_sampler::{
            observe_first_input_make, observe_first_input_make_with_recipe_controls,
            observe_initial_make, InitialMakeObservationRequest,
        },
        sha256_bytes, Sha256Digest,
    };
    use std::{ffi::OsStr, time::Duration};

    if cancellation.is_cancelled() || arguments.get(6).is_none_or(|value| value != "--") {
        return Err(());
    }
    let root = Path::new(arguments.get(2).ok_or(())?);
    let program = Path::new(arguments.get(3).ok_or(())?);
    let executable_sha256 = Sha256Digest::parse(
        arguments
            .get(4)
            .and_then(|value| value.to_str())
            .ok_or(())?,
    )
    .map_err(|_| ())?;
    let context_sha256 = Sha256Digest::parse(
        arguments
            .get(5)
            .and_then(|value| value.to_str())
            .ok_or(())?,
    )
    .map_err(|_| ())?;
    let directory = PrivateNativeCaptureDirectory::open(root).map_err(|_| ())?;
    let qualification = match phase {
        MakeObservationPhase::PreFirstFile => "instrumented-initial-observation-only",
        MakeObservationPhase::FirstInputPrelude => "instrumented-first-input-observation-only",
    };
    // An exclusive start record prevents reusing a partially failed observation.
    let mut start_document = serde_json::json!({
        "version":1,"qualification":qualification,"phase":phase,
        "program":program,"executable_sha256":executable_sha256,
        "context_sha256":context_sha256,"timeout_seconds":40
    });
    if let Some(value) = argument_zero {
        start_document["version"] = 2.into();
        start_document["argument_zero"] = value.into();
    }
    if recipe_controls {
        if phase != MakeObservationPhase::FirstInputPrelude {
            return Err(());
        }
        start_document["version"] = 3.into();
        start_document["recipe_controls"] = true.into();
    }
    let start = serde_json::to_vec(&start_document).map_err(|_| ())?;
    directory
        .write_new(OsStr::new("observation-started.json"), &start)
        .map_err(|_| ())?;
    let invocation_path = root.join("invocation.json");
    let invocation = argument_zero
        .map_or_else(
            || capture_current_with_snapshot(&invocation_path, &arguments[7..]),
            |value| {
                capture_current_with_selected_argument_zero(
                    &invocation_path,
                    &arguments[7..],
                    value,
                )
            },
        )
        .map_err(|_| ())?;
    directory.revalidate().map_err(|_| ())?;
    {
        let request = InitialMakeObservationRequest {
            program,
            executable_sha256: &executable_sha256,
            invocation: &invocation,
            context_sha256: &context_sha256,
            timeout: Duration::from_secs(40),
        };
        let retain =
            |stage, output: &aros_common::ProcessOutput| retain_stage(&directory, stage, output);
        let observation = match phase {
            MakeObservationPhase::PreFirstFile => {
                observe_initial_make(&request, cancellation, retain)
            }
            MakeObservationPhase::FirstInputPrelude => {
                if recipe_controls {
                    observe_first_input_make_with_recipe_controls(&request, cancellation, retain)
                } else {
                    observe_first_input_make(&request, cancellation, retain)
                }
            }
        }
        .map_err(|_| ())?;
        let bytes = observation.to_bytes().map_err(|_| ())?;
        if cancellation.is_cancelled() {
            return Err(());
        }
        directory
            .write_new(OsStr::new("observation.json"), &bytes)
            .map_err(|_| ())?;
        // These are producer output metadata, not an independently authenticated
        // consumer selection. The supervising parent still binds them to its run.
        let selection = serde_json::to_vec(&serde_json::json!({
            "version":1,"qualification":qualification,
            "document_sha256":sha256_bytes(&bytes),"binding":observation.binding(),
        }))
        .map_err(|_| ())?;
        if cancellation.is_cancelled() {
            return Err(());
        }
        directory
            .write_new(OsStr::new("observation-selection.json"), &selection)
            .map_err(|_| ())?;
        directory.revalidate().map_err(|_| ())
    }
}

#[cfg(unix)]
fn with_observation_cancellation<T>(
    operation: impl FnOnce(&aros_common::CancellationToken) -> Result<T, ()>,
) -> Result<T, ()> {
    use tokio::signal::unix::{signal, SignalKind};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .map_err(|_| ())?;
    let (mut interrupt, mut terminate) = runtime.block_on(async {
        Ok::<_, ()>((
            signal(SignalKind::interrupt()).map_err(|_| ())?,
            signal(SignalKind::terminate()).map_err(|_| ())?,
        ))
    })?;
    let cancellation = aros_common::CancellationToken::default();
    let watcher_token = cancellation.clone();
    let (finished, mut completion) = tokio::sync::oneshot::channel::<()>();
    std::thread::scope(|scope| {
        let watcher = scope.spawn(move || {
            runtime.block_on(async {
                tokio::select! {
                    biased;
                    _ = interrupt.recv() => watcher_token.cancel(),
                    _ = terminate.recv() => watcher_token.cancel(),
                    _ = &mut completion => {},
                }
            });
        });
        let result = operation(&cancellation);
        let _ = finished.send(());
        watcher.join().map_err(|_| ())?;
        if cancellation.is_cancelled() {
            Err(())
        } else {
            result
        }
    })
}

#[cfg(unix)]
fn retain_stage(
    directory: &aros_common::make_native_invocation::PrivateNativeCaptureDirectory,
    stage: aros_common::make_native_sampler::NativeMakeObservationStage,
    output: &aros_common::ProcessOutput,
) -> Result<(), String> {
    use aros_common::make_native_sampler::NativeMakeObservationStage;
    use std::ffi::OsStr;
    let name = match stage {
        NativeMakeObservationStage::Discovery => "discovery",
        NativeMakeObservationStage::Initial => "initial",
    };
    for (suffix, stream) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let mut bytes = Vec::new();
        stream
            .write_rendered(&mut bytes)
            .map_err(|_| "cannot retain native observation stream")?;
        directory
            .write_new(OsStr::new(&format!("{name}.{suffix}")), &bytes)
            .map_err(|_| "cannot retain native observation stream")?;
    }
    let status = serde_json::to_vec(&serde_json::json!({
        "status":output.status.code(),"timed_out":output.timed_out,
        "cancelled":output.cancelled,"stdout_exact":!output.stdout.is_truncated(),
        "stderr_exact":!output.stderr.is_truncated(),
    }))
    .map_err(|_| "cannot retain native observation status")?;
    directory
        .write_new(OsStr::new(&format!("{name}.status.json")), &status)
        .map_err(|_| "cannot retain native observation status".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::with_observation_cancellation;
    use aros_common::make_native_invocation::PrivateNativeCaptureDirectory;
    use std::ffi::OsStr;

    #[test]
    fn late_cancellation_does_not_make_retained_files_a_success_authority() {
        let temporary = tempfile::tempdir().expect("private late cancellation fixture");
        let root = temporary.path().canonicalize().expect("canonical fixture");
        let directory = PrivateNativeCaptureDirectory::open(&root).expect("bind private fixture");
        let result = with_observation_cancellation(|cancellation| {
            directory
                .write_new(OsStr::new("selection.json"), b"uncompleted selected bytes")
                .map_err(|_| ())?;
            cancellation.cancel();
            Ok(())
        });
        assert!(
            result.is_err(),
            "late cancellation became a successful owner result"
        );
        assert!(
            root.join("selection.json").is_file(),
            "failed evidence was removed"
        );
    }
}
