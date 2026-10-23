//! Repeated native Make observations use fresh, bounded private call slots.

use aros_common::{
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    sha256_bytes, Sha256Digest,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

const CONTROL_NAME: &str = "control.json";
const COLLECTION_FLAG: &str = "--observe-make-launch-collection";
const SELECTED_MAKEFILE: &str = "selected.mk";

#[path = "collection_readback.rs"]
mod readback;

#[path = "collection_argv_zero.rs"]
mod argv_zero;

fn private_directory(path: &Path) {
    fs::create_dir(path).expect("create private test directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("set private test directory mode");
}

fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write private test file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("set private test file mode");
}

fn collection_root(root: &Path) -> PathBuf {
    let output_root = root.join("collection");
    private_directory(&output_root);
    assert_private_directory(&output_root);
    output_root
}

fn synthetic_makefile(cwd: &Path) {
    private_file(
        &cwd.join(SELECTED_MAKEFILE),
        b"$(shell printf parsed > parse-marker)\n.PHONY: all\nall:\n\t@printf recipe > recipe-marker\n",
    );
}

fn control_value(
    output_root: &Path,
    program: &Path,
    executable_sha256: &Sha256Digest,
    maximum_calls: usize,
) -> Value {
    json!({
        "version": 3,
        "output_root": output_root,
        "program": program,
        "executable_sha256": executable_sha256.as_str(),
        "context_sha256": super::context_digest(),
        "phase": "first-input-prelude",
        "maximum_calls": maximum_calls,
    })
}

fn write_control(root: &Path, value: &Value) -> (PathBuf, Sha256Digest) {
    let bytes = serde_json::to_vec(value).expect("encode collection control");
    PrivateNativeCaptureDirectory::open(root)
        .expect("bind private control directory")
        .write_new(OsStr::new(CONTROL_NAME), &bytes)
        .expect("retain private collection control");
    super::assert_private_file(&root.join(CONTROL_NAME));
    (root.join(CONTROL_NAME), sha256_bytes(&bytes))
}

fn make_environment(root: &Path, private_value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".into(),
            std::env::var("PATH").expect("test runner provides PATH"),
        ),
        ("LC_ALL".into(), "C".into()),
        ("PRIVATE_COLLECTION_VALUE".into(), private_value.into()),
        ("AROS_TRANSPILER_LOG_LEVEL".into(), "trace".into()),
        (
            "AROS_TRANSPILER_LOG_FILE".into(),
            root.join("must-not-log").to_string_lossy().into_owned(),
        ),
    ])
}

fn valid_arguments(cli_value: &str) -> Vec<String> {
    ["-r", "-R", "-n", "-f", SELECTED_MAKEFILE, cli_value, "all"]
        .map(str::to_owned)
        .to_vec()
}

fn collection_command(
    control: &Path,
    control_sha256: &Sha256Digest,
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    command
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg(COLLECTION_FLAG)
        .arg(control)
        .arg(control_sha256.as_str())
        .arg("--")
        .args(arguments);
    command
}

fn run_collection(
    control: &Path,
    control_sha256: &Sha256Digest,
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> Output {
    collection_command(control, control_sha256, cwd, arguments, environment)
        .output()
        .expect("run collection observation entrypoint")
}

fn expected_invocation(
    arguments: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> NativeMakeInvocation {
    NativeMakeInvocation::new(arguments.to_vec(), cwd.to_path_buf(), environment.clone())
        .expect("construct exact expected Make invocation")
}

fn call_name(slot: usize) -> String {
    format!("call-{slot:04}")
}

fn assert_private_directory(path: &Path) {
    let metadata = fs::metadata(path).expect("private directory exists");
    assert!(metadata.is_dir(), "retained output is not a directory");
    assert!(
        metadata.permissions().mode() & 0o7777 == 0o700,
        "retained directory does not have mode 0700"
    );
}

fn assert_call_files(output_root: &Path, slot: usize, complete: bool) -> PathBuf {
    let name = call_name(slot);
    let call_directory = output_root.join(&name);
    assert_private_directory(&call_directory);
    super::assert_private_file(&output_root.join(format!("{name}.claim")));
    assert!(
        fs::metadata(output_root.join(format!("{name}.claim")))
            .expect("inspect retained slot claim")
            .len()
            == 0,
        "slot claim is not empty"
    );

    for file in ["observation-started.json", "invocation.json"] {
        super::assert_private_file(&call_directory.join(file));
    }
    if complete {
        for file in [
            "discovery.stdout",
            "discovery.stderr",
            "discovery.status.json",
            "initial.stdout",
            "initial.stderr",
            "initial.status.json",
            "observation.json",
            "observation-selection.json",
            "call-completed.json",
        ] {
            super::assert_private_file(&call_directory.join(file));
        }
        super::assert_status(&call_directory.join("discovery.status.json"), 2);
        super::assert_status(&call_directory.join("initial.status.json"), 2);
    } else {
        for file in [
            "discovery.stdout",
            "discovery.stderr",
            "discovery.status.json",
            "initial.stdout",
            "initial.stderr",
            "initial.status.json",
            "observation.json",
            "observation-selection.json",
            "call-completed.json",
        ] {
            assert!(
                !call_directory.join(file).exists(),
                "failed slot published a later-stage output"
            );
        }
    }
    call_directory
}

fn verify_complete_call(
    output_root: &Path,
    slot: usize,
    control_sha256: &Sha256Digest,
    expected: Option<&NativeMakeInvocation>,
) -> NativeMakeInvocation {
    let call_directory = assert_call_files(output_root, slot, true);
    let started: Value = serde_json::from_slice(
        &fs::read(call_directory.join("observation-started.json"))
            .expect("read private observation start"),
    )
    .expect("parse private observation start");
    let context_sha256 = super::context_digest();
    assert!(
        started["phase"].as_str() == Some("first-input-prelude")
            && started["context_sha256"].as_str() == Some(context_sha256.as_str()),
        "observation start lost its selected phase or context"
    );

    let invocation_bytes = fs::read(call_directory.join("invocation.json"))
        .expect("read exact retained invocation packet");
    let invocation = NativeMakeInvocation::parse(&invocation_bytes)
        .expect("parse exact retained invocation packet");
    if let Some(expected) = expected {
        let expected_bytes = expected
            .to_bytes()
            .expect("encode independently selected invocation packet");
        assert!(
            invocation_bytes == expected_bytes && &invocation == expected,
            "retained invocation packet differs from exact selected inputs"
        );
    }

    let document = fs::read(call_directory.join("observation.json"))
        .expect("read completed observation document");
    let selection: Value = serde_json::from_slice(
        &fs::read(call_directory.join("observation-selection.json"))
            .expect("read completed observation selection"),
    )
    .expect("parse completed observation selection");
    let document_sha256 = sha256_bytes(&document);
    assert!(
        selection["document_sha256"].as_str() == Some(document_sha256.as_str()),
        "selection digest differs from retained observation bytes"
    );
    let binding: MakeObservationBinding = serde_json::from_value(selection["binding"].clone())
        .expect("parse selected observation binding");
    assert!(
        binding.phase == MakeObservationPhase::FirstInputPrelude
            && binding.context_sha256 == context_sha256,
        "observation binding differs from selected phase or context"
    );
    let retained = RetainedMakeObservation::load(&document, &document_sha256, &binding)
        .expect("load completed observation data");
    assert!(
        retained.invocation() == &invocation,
        "observation document differs from exact invocation packet"
    );

    let completion: Value = serde_json::from_slice(
        &fs::read(call_directory.join("call-completed.json"))
            .expect("read successful call completion"),
    )
    .expect("parse successful call completion");
    assert!(
        completion["slot"].as_u64() == Some(slot as u64)
            && completion["phase"].as_str() == Some("first-input-prelude")
            && completion["control_sha256"].as_str() == Some(control_sha256.as_str())
            && completion["context_sha256"].as_str() == Some(context_sha256.as_str()),
        "completion record differs from selected slot or control bindings"
    );
    invocation
}

fn collection_snapshot(root: &Path) -> BTreeMap<String, (String, u32)> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).expect("list retained collection entries") {
        let entry = entry.expect("read retained collection entry");
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = fs::symlink_metadata(&path).expect("inspect retained collection entry");
        let mode = metadata.permissions().mode() & 0o7777;
        if metadata.is_dir() {
            assert!(
                result
                    .insert(format!("{name}/"), ("directory".into(), mode))
                    .is_none(),
                "duplicate retained collection entry"
            );
            for child in fs::read_dir(&path).expect("list retained call directory") {
                let child = child.expect("read retained call file");
                let child_path = child.path();
                let child_name = format!("{name}/{}", child.file_name().to_string_lossy());
                let child_metadata =
                    fs::symlink_metadata(&child_path).expect("inspect retained call file");
                assert!(
                    result
                        .insert(
                            child_name,
                            (
                                sha256_bytes(
                                    &fs::read(&child_path).expect("read retained call file")
                                )
                                .to_string(),
                                child_metadata.permissions().mode() & 0o7777,
                            ),
                        )
                        .is_none(),
                    "duplicate retained collection entry"
                );
            }
        } else {
            assert!(
                result
                    .insert(
                        name,
                        (
                            sha256_bytes(&fs::read(&path).expect("read retained slot claim"))
                                .to_string(),
                            mode,
                        ),
                    )
                    .is_none(),
                "duplicate retained collection entry"
            );
        }
    }
    result
}

fn assert_no_source_effects(cwd: &Path) {
    assert!(
        !cwd.join("parse-marker").exists(),
        "selected source input was parsed"
    );
    assert!(
        !cwd.join("recipe-marker").exists(),
        "selected source recipe was executed"
    );
}

#[test]
fn collection_preserves_two_exact_observations_and_exhausts_without_overwrite() {
    let temporary = tempfile::tempdir().expect("collection observation fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    private_directory(&cwd);
    synthetic_makefile(&cwd);
    let output_root = collection_root(&root);
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program);
    let (control, control_sha256) = write_control(
        &root,
        &control_value(&output_root, &program, &executable_sha256, 2),
    );

    let cases = [
        ("CLI_SECRET=cli-first", "environment-first"),
        ("CLI_SECRET=cli-second", "environment-second"),
    ];
    for (slot, (cli_value, environment_value)) in cases.into_iter().enumerate() {
        let arguments = valid_arguments(cli_value);
        let environment = make_environment(&root, environment_value);
        let expected = expected_invocation(&arguments, &cwd, &environment);
        let output = run_collection(&control, &control_sha256, &cwd, &arguments, &environment);
        assert!(
            output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
            "collection observation did not complete privately"
        );
        verify_complete_call(&output_root, slot, &control_sha256, Some(&expected));
    }
    assert_no_source_effects(&cwd);
    let before_refusal = collection_snapshot(&output_root);
    let third_arguments = valid_arguments("CLI_SECRET=cli-third");
    let third_environment = make_environment(&root, "environment-third");
    super::assert_refused(&run_collection(
        &control,
        &control_sha256,
        &cwd,
        &third_arguments,
        &third_environment,
    ));
    assert!(
        collection_snapshot(&output_root) == before_refusal
            && !output_root.join("call-0002.claim").exists(),
        "exhausted collection changed or added a retained slot"
    );
}

#[test]
fn unsupported_startup_consumes_a_failed_slot_and_later_success_uses_next_slot() {
    let temporary = tempfile::tempdir().expect("failed collection slot fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    private_directory(&cwd);
    synthetic_makefile(&cwd);
    let output_root = collection_root(&root);
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program);
    let (control, control_sha256) = write_control(
        &root,
        &control_value(&output_root, &program, &executable_sha256, 2),
    );

    let failed_arguments = [
        "-r".to_owned(),
        "-R".to_owned(),
        "-n".to_owned(),
        "-f".to_owned(),
        SELECTED_MAKEFILE.to_owned(),
        "--eval=$(file >startup-marker,launched)".to_owned(),
        "all".to_owned(),
    ];
    let failed_environment = make_environment(&root, "environment-failed");
    super::assert_refused(&run_collection(
        &control,
        &control_sha256,
        &cwd,
        &failed_arguments,
        &failed_environment,
    ));
    assert_no_source_effects(&cwd);
    assert!(
        !cwd.join("startup-marker").exists(),
        "unsupported startup effects reached GNU Make"
    );
    let failed_directory = assert_call_files(&output_root, 0, false);
    let failed_before_success = collection_snapshot(&output_root);

    let valid = valid_arguments("CLI_SECRET=after-failure");
    let environment = make_environment(&root, "environment-after-failure");
    let expected = expected_invocation(&valid, &cwd, &environment);
    let output = run_collection(&control, &control_sha256, &cwd, &valid, &environment);
    assert!(
        output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
        "valid call did not complete in the next collection slot"
    );
    verify_complete_call(&output_root, 1, &control_sha256, Some(&expected));
    assert!(
        collection_snapshot(&output_root)
            .into_iter()
            .filter(|(name, _)| name.starts_with("call-0000/"))
            .collect::<BTreeMap<_, _>>()
            == failed_before_success
                .into_iter()
                .filter(|(name, _)| name.starts_with("call-0000/"))
                .collect::<BTreeMap<_, _>>(),
        "later success changed retained failed-slot evidence"
    );
    super::assert_private_file(&failed_directory.join("invocation.json"));

    let before_exhaustion = collection_snapshot(&output_root);
    let third_arguments = valid_arguments("CLI_SECRET=third");
    let third_environment = make_environment(&root, "environment-third");
    super::assert_refused(&run_collection(
        &control,
        &control_sha256,
        &cwd,
        &third_arguments,
        &third_environment,
    ));
    assert!(
        collection_snapshot(&output_root) == before_exhaustion
            && !output_root.join("call-0002.claim").exists(),
        "exhausted collection changed or added a retained slot"
    );
}

#[test]
fn collection_control_schema_and_digest_refuse_before_any_slot_claim() {
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program);
    let malformed = [
        "missing maximum_calls",
        "null maximum_calls",
        "zero maximum_calls",
        "oversized maximum_calls",
        "wrong version",
        "missing phase",
        "null phase",
        "unknown field",
        "version two collection mode",
        "control digest mismatch",
    ];

    for case in malformed {
        let temporary = tempfile::tempdir().expect("invalid collection control fixture");
        let root = super::private_root(&temporary);
        let cwd = root.join("working directory");
        private_directory(&cwd);
        let output_root = collection_root(&root);
        let mut value = control_value(&output_root, &program, &executable_sha256, 3);
        match case {
            "missing maximum_calls" => {
                let _ = value
                    .as_object_mut()
                    .expect("control object")
                    .remove("maximum_calls");
            }
            "null maximum_calls" => value["maximum_calls"] = Value::Null,
            "zero maximum_calls" => value["maximum_calls"] = json!(0),
            "oversized maximum_calls" => value["maximum_calls"] = json!(257),
            "wrong version" => value["version"] = json!(4),
            "missing phase" => {
                let _ = value
                    .as_object_mut()
                    .expect("control object")
                    .remove("phase");
            }
            "null phase" => value["phase"] = Value::Null,
            "unknown field" => value["unexpected"] = json!(true),
            "version two collection mode" => value["version"] = json!(2),
            "control digest mismatch" => {}
            _ => unreachable!("closed malformed-case list"),
        }
        let (control, actual_digest) = write_control(&root, &value);
        let selected_digest = if case == "control digest mismatch" {
            sha256_bytes(b"different selected control bytes")
        } else {
            actual_digest
        };
        let environment = make_environment(&root, "schema-refusal-environment");
        let arguments = valid_arguments("CLI_SECRET=never-launched");
        super::assert_refused(&run_collection(
            &control,
            &selected_digest,
            &cwd,
            &arguments,
            &environment,
        ));
        assert!(
            fs::read_dir(&output_root)
                .expect("inspect untouched collection directory")
                .next()
                .is_none(),
            "invalid control created a collection slot"
        );
    }
}

#[test]
fn concurrent_collection_calls_claim_unique_slots_and_exhaust_cleanly() {
    let temporary = tempfile::tempdir().expect("concurrent collection fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    private_directory(&cwd);
    synthetic_makefile(&cwd);
    let output_root = collection_root(&root);
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program);
    let (control, control_sha256) = write_control(
        &root,
        &control_value(&output_root, &program, &executable_sha256, 3),
    );

    let cases = [
        ("CLI_SECRET=parallel-one", "parallel-environment-one"),
        ("CLI_SECRET=parallel-two", "parallel-environment-two"),
        ("CLI_SECRET=parallel-three", "parallel-environment-three"),
    ];
    let mut expected_packets = BTreeSet::new();
    let mut children: Vec<Child> = Vec::new();
    for (cli_value, environment_value) in cases {
        let arguments = valid_arguments(cli_value);
        let environment = make_environment(&root, environment_value);
        let invocation = expected_invocation(&arguments, &cwd, &environment);
        assert!(
            expected_packets.insert(
                sha256_bytes(&invocation.to_bytes().expect("encode expected invocation"))
                    .to_string(),
            ),
            "expected invocation packets are not distinct"
        );
        children.push(
            collection_command(&control, &control_sha256, &cwd, &arguments, &environment)
                .spawn()
                .expect("start concurrent collection call"),
        );
    }

    for child in children {
        let output = child
            .wait_with_output()
            .expect("collect concurrent collection call");
        assert!(
            output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
            "concurrent collection call did not complete privately"
        );
    }

    for slot in 0..3 {
        let invocation = verify_complete_call(&output_root, slot, &control_sha256, None);
        let retained_bytes = invocation
            .to_bytes()
            .expect("encode retained concurrent invocation");
        let digest = sha256_bytes(&retained_bytes).to_string();
        assert!(
            expected_packets.remove(&digest),
            "concurrent slot overwrote or duplicated an exact invocation packet"
        );
    }
    assert!(
        expected_packets.is_empty(),
        "a concurrent invocation packet was not retained"
    );
    assert_no_source_effects(&cwd);

    let before_exhaustion = collection_snapshot(&output_root);
    let arguments = valid_arguments("CLI_SECRET=after-concurrency");
    let environment = make_environment(&root, "parallel-environment-excess");
    super::assert_refused(&run_collection(
        &control,
        &control_sha256,
        &cwd,
        &arguments,
        &environment,
    ));
    assert!(
        collection_snapshot(&output_root) == before_exhaustion
            && !output_root.join("call-0003.claim").exists(),
        "concurrent limit exhaustion changed or added a retained slot"
    );
}
