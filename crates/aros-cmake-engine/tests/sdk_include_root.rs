//! CMake root selection probes; these do not qualify a compiler or an SDK.
use std::{fs, path::Path, process::Command};

struct ProbeOutcome {
    output: std::process::Output,
    passed: bool,
    binding: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

fn probe(
    consumer: bool,
    mutation: &str,
    invalid_response: bool,
    generator_expression_source_path: bool,
) -> ProbeOutcome {
    let parent = tempfile::tempdir().unwrap();
    let prefix = if generator_expression_source_path {
        "$<1:changed>-"
    } else {
        "probe-"
    };
    let temporary = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent.path())
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let build = root.join("build");
    fs::create_dir_all(&build).unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("engine")
        .canonicalize()
        .unwrap();
    fs::write(root.join("one.c"), "fixture source input\n").unwrap();
    let binding = if consumer {
        let invalidation = if invalid_response {
            "file(READ \"${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json\" _response)\n\
             string(JSON _response SET \"${_response}\" contract_sha256 \"\\\"invalid-digest\\\"\")\n\
             file(WRITE \"${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json\" \"${_response}\\n\")\n"
        } else {
            ""
        };
        format!(
            "include(\"${{ENGINE}}/tests/NativeSourceSelectionFixture.cmake\")\n\
             aros_test_prepare_native_consumer_selection(\"${{ROOT}}\" \"${{ROOT}}/consumer.json\")\n\
             {invalidation}\
             aros_validate_native_consumer_contract()\n"
        )
    } else {
        String::new()
    };
    let expected = if consumer {
        "${CMAKE_BINARY_DIR}/SYS/Developer/include"
    } else {
        "${CMAKE_BINARY_DIR}/SDK/include"
    };
    let script = format!(
        "cmake_minimum_required(VERSION 3.22)\n\
         project(SdkIncludeRootProbe NONE)\n\
         include(\"${{ENGINE}}/NativeConsumerContract.cmake\")\n\
         {binding}\
         set(AROS_DEVELOPER_INCLUDE_DIR \"${{CMAKE_BINARY_DIR}}/SYS/Developer/include\")\n\
         {mutation}\n\
         aros_resolve_sdk_include_root(selected)\n\
         if(NOT selected STREQUAL \"{expected}\")\n\
             message(FATAL_ERROR \"incorrect SDK root: ${{selected}}\")\n\
         endif()\n\
         file(WRITE \"${{ROOT}}/passed\" \"selected SDK root\")\n"
    );
    fs::write(root.join("CMakeLists.txt"), script).unwrap();
    if invalid_response {
        // Ensure a failed current validation cannot leave a prior binding for
        // a later SDK gate to mistake for this configure's result.
        fs::write(build.join("aros-native-consumer-binding.json"), b"stale\n").unwrap();
    }
    let output = Command::new("cmake")
        .arg("-S")
        .arg(&root)
        .arg("-B")
        .arg(&build)
        .arg(format!("-DENGINE={}", engine.display()))
        .arg(format!("-DROOT={}", root.display()))
        .output()
        .unwrap();
    ProbeOutcome {
        output,
        passed: root.join("passed").exists(),
        binding: fs::read(build.join("aros-native-consumer-binding.json")).ok(),
        response: fs::read(build.join("native-consumer-validation-response.json")).ok(),
    }
}

#[test]
fn selected_consumer_uses_developer_root_and_legacy_retains_internal_root() {
    for consumer in [false, true] {
        let result = probe(consumer, "", false, false);
        assert!(
            result.output.status.success() && result.passed,
            "{}",
            String::from_utf8_lossy(&result.output.stderr)
        );
        if consumer {
            let response = result.response.expect("consumer response fixture");
            let binding = result.binding.expect("persisted consumer binding");
            assert_eq!(
                binding, response,
                "binding must preserve exact response bytes"
            );
            let binding_json: serde_json::Value = serde_json::from_slice(&binding).unwrap();
            assert!(
                binding_json["sdk_include_relative"] == "SYS/Developer/include",
                "binding must preserve the source-selected SDK include root"
            );
        } else {
            assert!(
                result.binding.is_none(),
                "legacy configure has no consumer binding"
            );
        }
    }
}

#[test]
fn persisted_binding_preserves_generator_expression_text_in_source_path() {
    let result = probe(true, "", false, true);
    assert!(
        result.output.status.success() && result.passed,
        "{}",
        String::from_utf8_lossy(&result.output.stderr)
    );
    let response = result.response.expect("consumer response fixture");
    let binding = result.binding.expect("persisted consumer binding");
    assert_eq!(
        binding, response,
        "binding must preserve exact response bytes"
    );
    let binding_json: serde_json::Value = serde_json::from_slice(&binding).unwrap();
    assert!(
        binding_json["source_dir"]
            .as_str()
            .unwrap()
            .contains("$<1:changed>"),
        "source path must retain literal generator-expression text"
    );
}

#[test]
fn changed_consumer_root_and_incompatible_sysroot_fail_before_publication() {
    for (mutation, expected) in [
        (
            "set(AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE SDK/include)",
            "ABI or consumer selectors changed after validation",
        ),
        (
            "set(AROS_DEVELOPER_INCLUDE_DIR \"${CMAKE_BINARY_DIR}/foreign/include\")",
            "source SDK include root differs from the Developer sysroot layout",
        ),
        (
            "set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT FALSE)",
            "requires fresh validation in this configure process",
        ),
    ] {
        let result = probe(true, mutation, false, false);
        assert!(
            !result.output.status.success() && !result.passed,
            "accepted {mutation}"
        );
        assert!(
            result.binding.is_none(),
            "published binding after {mutation}"
        );
        let error = String::from_utf8_lossy(&result.output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn invalid_consumer_response_leaves_no_binding_for_the_current_configure() {
    let result = probe(true, "", true, false);
    assert!(!result.output.status.success() && !result.passed);
    assert!(
        result.binding.is_none(),
        "stale binding survived failed validation"
    );
    let error = String::from_utf8_lossy(&result.output.stderr);
    assert!(error.contains("validation response differs"), "{error}");
}
