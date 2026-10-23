//! Explicit argv0 is data selected independently of the executable's path.

use super::*;
use aros_common::{
    make_initial_values::ValidatedMakeInitialValues, make_namespace::ValidatedMakeNamespace,
};

#[test]
fn selected_argument_zero_reaches_gnu_make_and_the_digest_bound_packet() {
    let program = super::super::find_make_program();
    let executable_sha256 = super::super::measure_executable(&program);
    for selected in [None, Some("selected-make-argv0-sentinel")] {
        let temporary = tempfile::tempdir().expect("argument zero fixture");
        let root = super::super::private_root(&temporary);
        let cwd = root.join("working");
        private_directory(&cwd);
        synthetic_makefile(&cwd);
        let output_root = collection_root(&root);
        let mut value = control_value(&output_root, &program, &executable_sha256, 1);
        if let Some(argument_zero) = selected {
            value["version"] = 4.into();
            value["argument_zero"] = argument_zero.into();
        }
        let (control, control_sha256) = write_control(&root, &value);
        let arguments = valid_arguments("CLI_VALUE=unchanged");
        let environment = make_environment(&root, "private-argument-zero-fixture");
        let mut expected = expected_invocation(&arguments, &cwd, &environment);
        if let Some(argument_zero) = selected {
            expected = expected.with_argument_zero(argument_zero.into()).unwrap();
        }
        let output = run_collection(&control, &control_sha256, &cwd, &arguments, &environment);
        assert!(output.status.success() && output.stdout.is_empty() && output.stderr.is_empty());
        let actual = verify_complete_call(&output_root, 0, &control_sha256, Some(&expected));
        assert_eq!(actual.argument_zero(), selected);
        let stdout = fs::read(output_root.join("call-0000/initial.stdout")).unwrap();
        let namespace = ValidatedMakeNamespace::from_make_stdout(&stdout).unwrap();
        let values = ValidatedMakeInitialValues::from_make_stdout(&stdout, &namespace).unwrap();
        let observed = values
            .entries()
            .find(|entry| entry.name == "MAKE_COMMAND")
            .expect("GNU Make retains its selected program name");
        assert_eq!(
            observed.value,
            selected.unwrap_or_else(|| program.to_str().unwrap())
        );
        assert_no_source_effects(&cwd);
    }
}

#[test]
fn versioned_argument_zero_controls_refuse_before_claiming_a_slot() {
    let program = super::super::find_make_program();
    let executable_sha256 = super::super::measure_executable(&program);
    for case in [
        "missing",
        "null",
        "empty",
        "nul",
        "legacy",
        "unknown-version",
        "duplicate",
    ] {
        let temporary = tempfile::tempdir().expect("invalid argument zero fixture");
        let root = super::super::private_root(&temporary);
        let cwd = root.join("working");
        private_directory(&cwd);
        let output_root = collection_root(&root);
        let mut value = control_value(&output_root, &program, &executable_sha256, 1);
        value["version"] = 4.into();
        value["argument_zero"] = "selected".into();
        match case {
            "missing" => {
                value.as_object_mut().unwrap().remove("argument_zero");
            }
            "null" => value["argument_zero"] = Value::Null,
            "empty" => value["argument_zero"] = "".into(),
            "nul" => value["argument_zero"] = "bad\0word".into(),
            "legacy" => value["version"] = 3.into(),
            "unknown-version" => value["version"] = 7.into(),
            "duplicate" => {}
            _ => unreachable!(),
        }
        let mut bytes = serde_json::to_vec(&value).unwrap();
        if case == "duplicate" {
            assert_eq!(bytes.pop(), Some(b'}'));
            bytes.extend_from_slice(b",\"argument_zero\":\"duplicate\"}");
        }
        PrivateNativeCaptureDirectory::open(&root)
            .unwrap()
            .write_new(OsStr::new(CONTROL_NAME), &bytes)
            .unwrap();
        let control = root.join(CONTROL_NAME);
        let control_sha256 = sha256_bytes(&bytes);
        let output = run_collection(
            &control,
            &control_sha256,
            &cwd,
            &valid_arguments("CLI_VALUE=ignored"),
            &make_environment(&root, "ignored"),
        );
        super::super::assert_refused(&output);
        assert_eq!(fs::read_dir(&output_root).unwrap().count(), 0);
    }
}

#[test]
fn extended_capacity_is_versioned_and_crosses_the_legacy_slot_boundary() {
    let program = super::super::find_make_program();
    let executable_sha256 = super::super::measure_executable(&program);
    for (version, capacity, accepted) in [
        (3, 257, false),
        (4, 257, false),
        (5, 256, false),
        (5, 4097, false),
        (5, 257, true),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = super::super::private_root(&temporary);
        let cwd = root.join("working");
        private_directory(&cwd);
        synthetic_makefile(&cwd);
        let output_root = collection_root(&root);
        let mut value = control_value(&output_root, &program, &executable_sha256, capacity);
        value["version"] = version.into();
        if version != 3 {
            value["argument_zero"] = "selected-gmake".into();
        }
        if accepted {
            // These are allocator claims, not completed observations. The test
            // proves the next slot, never completion of this synthetic batch.
            let directory = PrivateNativeCaptureDirectory::open(&output_root).unwrap();
            for slot in 0..256 {
                directory
                    .write_new(OsStr::new(&format!("call-{slot:04}.claim")), b"")
                    .unwrap();
            }
        }
        let (control, digest) = write_control(&root, &value);
        let arguments = valid_arguments("CLI_VALUE=unchanged");
        let environment = make_environment(&root, "bounded-versioned-capacity");
        let output = run_collection(&control, &digest, &cwd, &arguments, &environment);
        if accepted {
            assert!(
                output.status.success() && output.stdout.is_empty() && output.stderr.is_empty()
            );
            let expected = expected_invocation(&arguments, &cwd, &environment)
                .with_argument_zero("selected-gmake".into())
                .unwrap();
            verify_complete_call(&output_root, 256, &digest, Some(&expected));
            let exhausted = run_collection(&control, &digest, &cwd, &arguments, &environment);
            super::super::assert_refused(&exhausted);
            assert!(!output_root.join("call-0257.claim").exists());
        } else {
            super::super::assert_refused(&output);
            assert_eq!(fs::read_dir(&output_root).unwrap().count(), 0);
        }
        assert_no_source_effects(&cwd);
    }
}

#[test]
fn legacy_single_controls_do_not_admit_the_new_argument_zero_field() {
    let program = super::super::find_make_program();
    let executable_sha256 = super::super::measure_executable(&program);
    for version in [1, 2] {
        let temporary = tempfile::tempdir().unwrap();
        let root = super::super::private_root(&temporary);
        let output_root = collection_root(&root);
        let mut value = control_value(&output_root, &program, &executable_sha256, 1);
        value["version"] = version.into();
        value["argument_zero"] = "not-a-legacy-field".into();
        value.as_object_mut().unwrap().remove("maximum_calls");
        if version == 1 {
            value.as_object_mut().unwrap().remove("phase");
        }
        let (control, control_sha256) = write_control(&root, &value);
        let output = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
            .env_clear()
            .current_dir(&root)
            .arg("--observe-make-launch")
            .arg(control)
            .arg(control_sha256.as_str())
            .arg("--")
            .output()
            .unwrap();
        super::super::assert_refused(&output);
        assert_eq!(fs::read_dir(output_root).unwrap().count(), 0);
    }
}
