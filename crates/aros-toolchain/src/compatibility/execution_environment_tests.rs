//! Bounded tests for independently derived native compatibility environments.

use std::fs;
use std::path::Path;

use aros_common::{sha256_bytes, CancellationToken, DiagnosticCode};

use crate::compatibility::{
    derive_native_compatibility_environment_identity,
    derive_native_compatibility_environment_identity_with_epoch, prepare_host_tool_closure,
    CompatibilityHostTool, CompatibilityPhase, HostToolClosure, HostToolClosureRequest,
};

use super::execute_native_compatibility;
use super::tests::request;

#[test]
fn independently_derived_environments_match_all_six_execution_reports() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());
    let output_roots = [
        &request.cmake_build_root,
        &request.upstream_build_root,
        &request.standalone_output_root,
        &request.reports_root,
    ];
    for output_root in output_roots {
        assert!(
            !output_root.exists(),
            "fixture unexpectedly created {output_root:?}"
        );
    }

    let derived = derive_native_compatibility_environment_identity(
        &request.host_python,
        &request.host_tools,
        &request.host,
    )
    .unwrap();
    assert_eq!(
        derived.standalone_environment_sha256,
        sha256_bytes(b"{\"environment\":{\"PATH\":\"/nonexistent\"},\"host_tools\":{}}\n")
    );

    for output_root in output_roots {
        assert!(!output_root.exists(), "derivation created {output_root:?}");
    }

    let report = execute_native_compatibility(&request, &CancellationToken::default()).unwrap();
    let sdk_phases = [
        CompatibilityPhase::CmakeConsumer,
        CompatibilityPhase::UpstreamConfigure,
        CompatibilityPhase::UpstreamIncludes,
        CompatibilityPhase::UpstreamLinklibs,
    ];
    for phase in sdk_phases {
        let phase_report = &report.probes.reports[&phase];
        assert_eq!(
            phase_report.environment_sha256, derived.sdk_environment_sha256,
            "SDK environment digest differs for {phase:?}"
        );
        assert_eq!(phase_report.host_tools, derived.host_tools);
    }
    for phase in [
        CompatibilityPhase::StandaloneC,
        CompatibilityPhase::StandaloneCxx,
    ] {
        let phase_report = &report.probes.reports[&phase];
        assert_eq!(
            phase_report.environment_sha256, derived.standalone_environment_sha256,
            "standalone environment digest differs for {phase:?}"
        );
        assert!(phase_report.host_tools.is_empty());
    }
}

#[test]
fn explicit_consumer_epoch_reaches_cmake_and_upstream_and_binds_all_sdk_phases() {
    const EPOCH: u64 = 946_684_800;

    let temporary = tempfile::tempdir().unwrap();
    let (mut request, _, _) = request(temporary.path());
    request.consumer_source_date_epoch = Some(EPOCH);

    let cmake_epoch_log = temporary.path().join("cmake-source-date-epoch.log");
    append_script_line(
        &request.cmake_program,
        &format!(
            "printf '%s\\n' \"$SOURCE_DATE_EPOCH\" > '{}'",
            cmake_epoch_log.display()
        ),
    );

    let make_program = request.host_tools.tools["make"].program.clone();
    let make_epoch_log = temporary.path().join("make-source-date-epoch.log");
    append_script_line(
        &make_program,
        &format!(
            "printf '%s\\n' \"$SOURCE_DATE_EPOCH\" >> '{}'",
            make_epoch_log.display()
        ),
    );
    request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: temporary.path().join("host-tools-with-epoch-logging"),
        tools: closure_tools(&request.host_tools),
    })
    .unwrap();

    let output_roots = [
        &request.cmake_build_root,
        &request.upstream_build_root,
        &request.standalone_output_root,
        &request.reports_root,
    ];
    for output_root in output_roots {
        assert!(
            !output_root.exists(),
            "fixture unexpectedly created {output_root:?}"
        );
    }

    let legacy = derive_native_compatibility_environment_identity(
        &request.host_python,
        &request.host_tools,
        &request.host,
    )
    .unwrap();
    let explicit_none = derive_native_compatibility_environment_identity_with_epoch(
        &request.host_python,
        &request.host_tools,
        &request.host,
        None,
    )
    .unwrap();
    assert_eq!(explicit_none, legacy);

    let mut selected_digests = std::collections::BTreeSet::new();
    for epoch in [
        0,
        EPOCH,
        EPOCH + 1,
        aros_common::sdk_manifest::SDK_MAX_SOURCE_DATE_EPOCH,
    ] {
        let identity = derive_native_compatibility_environment_identity_with_epoch(
            &request.host_python,
            &request.host_tools,
            &request.host,
            Some(epoch),
        )
        .unwrap();
        assert_eq!(
            identity.sdk_environment_sha256,
            expected_sdk_environment_digest(&request, epoch)
        );
        assert_ne!(
            identity.sdk_environment_sha256,
            legacy.sdk_environment_sha256
        );
        assert_eq!(
            identity.standalone_environment_sha256,
            legacy.standalone_environment_sha256
        );
        assert!(selected_digests.insert(identity.sdk_environment_sha256.to_string()));
    }

    let derived = derive_native_compatibility_environment_identity_with_epoch(
        &request.host_python,
        &request.host_tools,
        &request.host,
        Some(EPOCH),
    )
    .unwrap();
    let independently_expected = expected_sdk_environment_digest(&request, EPOCH);
    assert_eq!(derived.sdk_environment_sha256, independently_expected);
    assert_ne!(
        derived.sdk_environment_sha256,
        legacy.sdk_environment_sha256
    );
    assert_eq!(derived.host_tools, legacy.host_tools);
    assert_eq!(
        derived.standalone_environment_sha256,
        legacy.standalone_environment_sha256
    );
    assert_eq!(
        derived.standalone_environment_sha256,
        sha256_bytes(b"{\"environment\":{\"PATH\":\"/nonexistent\"},\"host_tools\":{}}\n")
    );

    for output_root in output_roots {
        assert!(!output_root.exists(), "derivation created {output_root:?}");
    }

    let report = execute_native_compatibility(&request, &CancellationToken::default()).unwrap();
    let sdk_phases = [
        CompatibilityPhase::CmakeConsumer,
        CompatibilityPhase::UpstreamConfigure,
        CompatibilityPhase::UpstreamIncludes,
        CompatibilityPhase::UpstreamLinklibs,
    ];
    for phase in sdk_phases {
        let phase_report = &report.probes.reports[&phase];
        assert_eq!(
            phase_report.environment_sha256, independently_expected,
            "{phase:?}"
        );
        assert_eq!(phase_report.host_tools, derived.host_tools);
    }
    for phase in [
        CompatibilityPhase::StandaloneC,
        CompatibilityPhase::StandaloneCxx,
    ] {
        let phase_report = &report.probes.reports[&phase];
        assert_eq!(
            phase_report.environment_sha256, derived.standalone_environment_sha256,
            "{phase:?}"
        );
        assert!(phase_report.host_tools.is_empty());
    }

    assert_eq!(
        fs::read_to_string(cmake_epoch_log).unwrap().trim(),
        EPOCH.to_string()
    );
    let make_epochs = fs::read_to_string(make_epoch_log).unwrap();
    assert_eq!(
        make_epochs.lines().map(str::to_owned).collect::<Vec<_>>(),
        vec![EPOCH.to_string(), EPOCH.to_string()]
    );
}

#[test]
fn rejects_out_of_range_consumer_epoch_before_creating_output_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let (mut request, _, _) = request(temporary.path());
    request.consumer_source_date_epoch =
        Some(aros_common::sdk_manifest::SDK_MAX_SOURCE_DATE_EPOCH + 1);
    let output_roots = [
        &request.cmake_build_root,
        &request.upstream_build_root,
        &request.standalone_output_root,
        &request.reports_root,
    ];
    for output_root in output_roots {
        assert!(
            !output_root.exists(),
            "fixture unexpectedly created {output_root:?}"
        );
    }

    assert_ax0703(
        &execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err(),
        "consumer source-date epoch exceeds the SDK portable tar timestamp range",
    );
    for output_root in output_roots {
        assert!(!output_root.exists(), "rejection created {output_root:?}");
    }
}

#[test]
fn rejects_a_mutated_measured_host_executable() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());
    let measured_cc = request.host_tools.tools["cc"].program.clone();
    fs::write(&measured_cc, "#!/bin/sh\nexit 1\n").unwrap();

    assert_ax0703(
        &request.host_tools.revalidate().unwrap_err(),
        "compatibility host-tool executable changed after preparation",
    );
    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &request.host_tools,
            &request.host,
        )
        .unwrap_err(),
        "compatibility host-tool executable changed after preparation",
    );
}

#[test]
fn rejects_missing_or_unexpected_host_roles() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());

    let mut missing_tools = closure_tools(&request.host_tools);
    missing_tools.retain(|tool| tool.name != "make");
    let missing = host_tool_closure(
        temporary.path().join("host-tools-missing-role"),
        missing_tools,
    );
    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &missing,
            &request.host,
        )
        .unwrap_err(),
        "native compatibility host-tool closure does not bind the exact measured command set: missing make",
    );

    let mut unexpected_tools = closure_tools(&request.host_tools);
    unexpected_tools.push(CompatibilityHostTool {
        name: "unexpected".into(),
        program: request.cmake_program.clone(),
    });
    let unexpected = host_tool_closure(
        temporary.path().join("host-tools-unexpected-role"),
        unexpected_tools,
    );
    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &unexpected,
            &request.host,
        )
        .unwrap_err(),
        "native compatibility host-tool closure does not bind the exact measured command set; unexpected unexpected",
    );
}

#[test]
fn rejects_a_host_selector_that_does_not_match_the_measured_roles() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());

    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &request.host_tools,
            "macos-aarch64",
        )
        .unwrap_err(),
        "native compatibility host-tool closure does not bind the exact measured command set: missing gsed, llvm-arcc, llvm-ranlibcc, xcode-select, xcrun",
    );
}

#[test]
fn rejects_an_unsupported_host_selector() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());

    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &request.host_tools,
            "freebsd-x86_64",
        )
        .unwrap_err(),
        "native compatibility host-tool closure does not support host 'freebsd-x86_64'",
    );
}

#[test]
fn rejects_a_python_interpreter_that_differs_from_the_python3_role() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());
    let mut tools = closure_tools(&request.host_tools);
    let cc = request.host_tools.tools["cc"].program.clone();
    let python = tools
        .iter_mut()
        .find(|tool| tool.name == "python3")
        .unwrap();
    python.program = cc;
    let mismatched = host_tool_closure(temporary.path().join("host-tools-python-mismatch"), tools);

    assert_ax0703(
        &derive_native_compatibility_environment_identity(
            &request.host_python,
            &mismatched,
            &request.host,
        )
        .unwrap_err(),
        "native compatibility host-tool closure does not bind the exact measured command set",
    );
}

fn assert_ax0703(error: &crate::ContractError, expected_message: &str) {
    let diagnostic = &error.diagnostics().diagnostics[0];
    assert_eq!(diagnostic.code, DiagnosticCode::ProducerCompatibility);
    assert_eq!(diagnostic.message, expected_message);
}

fn closure_tools(closure: &HostToolClosure) -> Vec<CompatibilityHostTool> {
    closure
        .tools
        .iter()
        .map(|(name, identity)| CompatibilityHostTool {
            name: name.clone(),
            program: identity.program.clone(),
        })
        .collect()
}

fn host_tool_closure(
    output_root: impl AsRef<Path>,
    tools: Vec<CompatibilityHostTool>,
) -> HostToolClosure {
    prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: output_root.as_ref().to_path_buf(),
        tools,
    })
    .unwrap()
}

fn append_script_line(path: &Path, line: &str) {
    let mut script = fs::read_to_string(path).unwrap();
    script.push('\n');
    script.push_str(line);
    script.push('\n');
    fs::write(path, script).unwrap();
}

fn expected_sdk_environment_digest(
    request: &crate::compatibility::NativeCompatibilityRequest,
    epoch: u64,
) -> aros_common::Sha256Digest {
    let mut variables = request.host_python.compatibility_environment().unwrap();
    variables.insert("ac_cv_prog_cc_c23".into(), String::new());
    variables.insert("AROS_FETCH_OFFLINE".into(), "1".into());
    variables.insert("SOURCE_DATE_EPOCH".into(), epoch.to_string());
    variables.insert("PATH".into(), "/__aros_compatibility_host_tools__".into());
    let host_tools = request
        .host_tools
        .tools
        .iter()
        .map(|(name, identity)| {
            (
                name.clone(),
                serde_json::json!({
                    "sha256": identity.sha256.clone(),
                    "size": identity.size,
                }),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let bytes = crate::canonical::bytes(&serde_json::json!({
        "environment": variables,
        "host_tools": host_tools,
    }))
    .unwrap();
    sha256_bytes(&bytes)
}
