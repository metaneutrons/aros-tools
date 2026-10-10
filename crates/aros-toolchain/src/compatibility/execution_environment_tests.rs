//! Bounded tests for independently derived native compatibility environments.

use std::fs;
use std::path::Path;

use aros_common::{sha256_bytes, CancellationToken, DiagnosticCode};

use crate::compatibility::{
    derive_native_compatibility_environment_identity, prepare_host_tool_closure,
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
