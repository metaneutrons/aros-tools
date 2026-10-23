//! Actual Root -> Source-built MetaMake -> native first-input observation.
//!
//! Test-only live data collection. This is not ordinary Root preparation or
//! independent reconstruction of the complete Source-defined child environment.

use super::{check_observer, invocation, read_bytes, LiteralMakeChildCommand, NativeObserver};
use aros_common::{
    make_initial_values::ValidatedMakeInitialValues,
    make_namespace::ValidatedMakeNamespace,
    make_native_collection::{read_native_make_collection, NativeMakeCallSelection},
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_native_observation::{MakeObservationBinding, MakeObservationPhase},
    sha256_bytes, CancellationToken, Sha256Digest,
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) struct RootObservation {
    root: PrivateNativeCaptureDirectory,
    observer: NativeObserver,
    alias: PathBuf,
    program: PathBuf,
    program_sha256: Sha256Digest,
    argument_zero: String,
    control_sha256: Sha256Digest,
    context_sha256: Sha256Digest,
}

fn source_program_word(value: &str) -> Result<String, &'static str> {
    let command = LiteralMakeChildCommand::parse(value)
        .map_err(|_| "original Source Make command is not literal")?;
    if !command.arguments().is_empty() || !command.environment_assignments().is_empty() {
        return Err("original Source Make command has unbound arguments or environment");
    }
    Ok(command.program().to_owned())
}

impl RootObservation {
    pub(super) fn prepare(
        output: &Path,
        observer: &NativeObserver,
        root_invocation: &invocation::InvocationContext,
        source_word: &str,
        source_context: &serde_json::Value,
    ) -> Self {
        let argument_zero = source_program_word(source_word).unwrap();
        let root = PrivateNativeCaptureDirectory::open(output).unwrap();
        let alias = output.join("o");
        std::os::unix::fs::symlink(&observer.program, &alias).unwrap();
        let program = fs::canonicalize(root_invocation.program()).unwrap();
        let program_sha256 = root_invocation.program_sha256().clone();
        let collection = output.join("d");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&collection)
            .unwrap();
        let context = serde_json::to_vec(&serde_json::json!({
            "version":1,"source_context":source_context,
            "phase":MakeObservationPhase::FirstInputPrelude,
            "maximum_calls":1,"program":program,"program_sha256":program_sha256,
            "argument_zero":argument_zero,"observer":observer.program,
            "observer_sha256":observer.sha256,
        }))
        .unwrap();
        let context_sha256 = sha256_bytes(&context);
        root.write_new(OsStr::new("native-context.json"), &context)
            .unwrap();
        let control = serde_json::to_vec(&serde_json::json!({
            "version":4,"phase":MakeObservationPhase::FirstInputPrelude,
            "maximum_calls":1,"output_root":collection,
            "program":program,"executable_sha256":program_sha256,
            "context_sha256":context_sha256,"argument_zero":argument_zero,
        }))
        .unwrap();
        let control_sha256 = sha256_bytes(&control);
        root.write_new(OsStr::new("c"), &control).unwrap();
        Self {
            root,
            observer: NativeObserver {
                program: observer.program.clone(),
                sha256: observer.sha256.clone(),
                collection: true,
            },
            alias,
            program,
            program_sha256,
            argument_zero,
            control_sha256,
            context_sha256,
        }
    }

    pub(super) fn launcher_program(&self) -> &Path {
        &self.alias
    }

    pub(super) fn launcher(&self) -> String {
        format!(
            "{} --observe-make-launch-collection {} {} --",
            self.alias.display(),
            self.root.path().join("c").display(),
            self.control_sha256
        )
    }

    fn revalidate(&self) {
        self.root.revalidate().unwrap();
        check_observer(&self.observer);
        assert_eq!(
            fs::canonicalize(&self.alias).unwrap(),
            self.observer.program
        );
        assert_eq!(
            sha256_bytes(&fs::read(&self.program).unwrap()),
            self.program_sha256
        );
        self.root
            .read_selected(OsStr::new("c"), &self.control_sha256, 64 * 1024)
            .unwrap();
        self.root
            .read_selected(
                OsStr::new("native-context.json"),
                &self.context_sha256,
                1024 * 1024,
            )
            .unwrap();
    }

    pub(super) fn read_after_reaped(
        &self,
        expected_arguments: &[String],
        expected_cwd: &Path,
        remaining: Duration,
        cancellation: &CancellationToken,
    ) -> Vec<u8> {
        let deadline = Instant::now().checked_add(remaining).unwrap();
        self.revalidate();
        let call = self.root.path().join("d/call-0000");
        let invocation_bytes = read_bytes(&call.join("invocation.json")).unwrap();
        let captured = NativeMakeInvocation::parse(&invocation_bytes).unwrap();
        assert_eq!(captured.argument_zero(), Some(self.argument_zero.as_str()));
        assert_eq!(captured.arguments(), expected_arguments);
        assert_eq!(captured.working_directory(), expected_cwd);
        // Source selects the original word; the observer cannot mask a missing
        // or different actual executable in the native child's retained PATH.
        let resolved = which::which_in(
            &self.argument_zero,
            captured.environment().get("PATH"),
            expected_cwd,
        )
        .unwrap();
        assert_eq!(fs::canonicalize(resolved).unwrap(), self.program);
        let initial = read_bytes(&call.join("initial.stdout")).unwrap();
        let packet = read_bytes(&call.join("observation.json")).unwrap();
        // Independently supplied phase/context and checked native argv/cwd;
        // stream/packet hashes are live-run data, NOT independent parent or
        // complete environment authentication. No receipt consumes this yet.
        let binding = MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256: self.context_sha256.clone(),
            invocation_sha256: sha256_bytes(&invocation_bytes),
            stdout_sha256: sha256_bytes(&initial),
        };
        let collection = PrivateNativeCaptureDirectory::open(&self.root.path().join("d")).unwrap();
        let retained = read_native_make_collection(
            &collection,
            &self.control_sha256,
            &[NativeMakeCallSelection {
                document_sha256: sha256_bytes(&packet),
                binding,
            }],
            deadline,
            cancellation,
        )
        .unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[0].invocation().to_bytes().unwrap(),
            invocation_bytes
        );
        let namespace = ValidatedMakeNamespace::from_make_stdout(&initial).unwrap();
        let values = ValidatedMakeInitialValues::from_make_stdout(&initial, &namespace).unwrap();
        assert_eq!(
            values
                .entries()
                .find(|entry| entry.name == "MAKE_COMMAND")
                .unwrap()
                .value,
            self.argument_zero
        );
        // Wrong source context must not be accepted merely because the packet
        // and completion record are mutually consistent.
        let wrong = MakeObservationBinding {
            context_sha256: sha256_bytes(b"foreign-root-context"),
            ..retained[0].binding().clone()
        };
        let error = read_native_make_collection(
            &collection,
            &self.control_sha256,
            &[NativeMakeCallSelection {
                document_sha256: sha256_bytes(&packet),
                binding: wrong,
            }],
            deadline,
            cancellation,
        )
        .unwrap_err();
        assert!(error.contains("collection completion differs"), "{error}");
        self.root
            .write_new(OsStr::new("capture.json"), &invocation_bytes)
            .unwrap();
        self.revalidate();
        invocation_bytes
    }
}

#[test]
fn source_make_program_word_is_literal_not_a_basename_or_ambient_guess() {
    for word in ["gmake", "make", "/selected/custom-make"] {
        assert_eq!(source_program_word(word).unwrap(), word);
    }
    for value in [
        "",
        "$(HOST_MAKE)",
        "env MODE=x make",
        "MODE=x make",
        "make -r",
        "make; bad",
    ] {
        assert!(source_program_word(value).is_err());
    }
}

#[test]
fn observation_selection_keeps_legacy_default_and_refuses_typos_null_or_duplicate_modes() {
    let base =
        serde_json::json!({"program":"/selected/observer", "sha256":sha256_bytes(b"selected")});
    let legacy: NativeObserver = serde_json::from_value(base.clone()).unwrap();
    assert!(!legacy.collection);
    let mut selected = base.clone();
    selected["collection"] = true.into();
    assert!(
        serde_json::from_value::<NativeObserver>(selected)
            .unwrap()
            .collection
    );
    let mut typo = base.clone();
    typo["collecton"] = true.into();
    assert!(serde_json::from_value::<NativeObserver>(typo).is_err());
    let mut null = base;
    null["collection"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<NativeObserver>(null).is_err());
    let duplicate = format!(
        "{{\"program\":\"/selected/observer\",\"sha256\":\"{}\",\"collection\":true,\"collection\":false}}",
        sha256_bytes(b"selected")
    );
    assert!(serde_json::from_str::<NativeObserver>(&duplicate).is_err());
}
