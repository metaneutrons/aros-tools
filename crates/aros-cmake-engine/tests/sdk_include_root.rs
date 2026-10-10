//! CMake root selection probes; these do not qualify a compiler or an SDK.
use std::{fs, path::Path, process::Command};

fn probe(consumer: bool, mutation: &str) -> (std::process::Output, bool) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("engine")
        .canonicalize()
        .unwrap();
    fs::write(root.join("one.c"), "fixture source input\n").unwrap();
    let binding = if consumer {
        "include(\"${ENGINE}/tests/NativeSourceSelectionFixture.cmake\")\n\
         aros_test_prepare_native_consumer_selection(\"${ROOT}\" \"${ROOT}/consumer.json\")\n\
         aros_validate_native_consumer_contract()\n"
    } else {
        ""
    };
    let expected = if consumer {
        "${CMAKE_BINARY_DIR}/SYS/Developer/include"
    } else {
        "${CMAKE_BINARY_DIR}/SDK/include"
    };
    let script = format!(
        "cmake_minimum_required(VERSION 3.22)\n\
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
    let script_path = root.join("probe.cmake");
    fs::write(&script_path, script).unwrap();
    let output = Command::new("cmake")
        .current_dir(&root)
        .arg(format!("-DENGINE={}", engine.display()))
        .arg(format!("-DROOT={}", root.display()))
        .arg("-P")
        .arg(script_path)
        .output()
        .unwrap();
    (output, root.join("passed").exists())
}

#[test]
fn selected_consumer_uses_developer_root_and_legacy_retains_internal_root() {
    for consumer in [false, true] {
        let (output, passed) = probe(consumer, "");
        assert!(
            output.status.success() && passed,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
        let (output, passed) = probe(true, mutation);
        assert!(!output.status.success() && !passed, "accepted {mutation}");
        let error = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(error.contains(expected), "{error}");
    }
}
