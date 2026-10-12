use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

const FUNCTION_START: &str = "function(_aros_grub_iso_assets_fallback_required out_required)";

fn production_function() -> String {
    let cmake_lists = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine/CMakeLists.txt");
    let source = fs::read_to_string(cmake_lists).expect("read engine CMakeLists.txt");
    let start = source
        .find(FUNCTION_START)
        .expect("find production GRUB authority function");
    let function_end = source[start..]
        .find("endfunction()")
        .expect("find end of production GRUB authority function")
        + "endfunction()".len();
    source[start..start + function_end].to_owned()
}

fn run_fixture(setup: &str) -> (bool, String) {
    let temporary = TempDir::new().expect("create GRUB authority fixture");
    let script_path = temporary.path().join("grub-authority.cmake");
    let mut script = String::from("cmake_minimum_required(VERSION 3.22)\n");
    script.push_str(&production_function());
    script.push_str(
        "\nfunction(_aros_native_source_selection_validated out_validated)\n\
         set(${out_validated} \"${TEST_SOURCE_SELECTED}\" PARENT_SCOPE)\n\
         endfunction()\n",
    );
    script.push_str(setup);
    script.push_str(
        "\nset(AROS_TOOLCHAIN llvm)\n\
         set(AROS_TARGET_CPU x86_64)\n\
         set(AROS_TARGET_PLATFORM pc)\n\
         set(AROS_GRUB2_HOST_LANES_AVAILABLE TRUE)\n\
         _aros_grub_iso_assets_fallback_required(_required)\n\
         if(_required)\n\
           message(STATUS \"GRUB_FALLBACK_REQUIRED=TRUE\")\n\
         else()\n\
           message(STATUS \"GRUB_FALLBACK_REQUIRED=FALSE\")\n\
         endif()\n",
    );
    fs::write(&script_path, script).expect("write GRUB authority fixture");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(script_path)
        .output()
        .expect("run CMake GRUB authority fixture");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

#[test]
fn sdk_consumer_skips_legacy_grub_media_lanes_but_full_build_fails_closed() {
    let (sdk_succeeded, sdk_output) = run_fixture(
        "set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)\n\
         set(TEST_SOURCE_SELECTED TRUE)\n\
         set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA\n\
           aros-sdk-consumer-validation-v1)\n",
    );
    assert!(sdk_succeeded, "SDK consumer fixture failed:\n{sdk_output}");
    assert!(
        sdk_output.contains("GRUB_FALLBACK_REQUIRED=FALSE"),
        "SDK consumer entered the legacy GRUB media lane:\n{sdk_output}"
    );

    let (full_build_succeeded, full_build_output) = run_fixture("");
    assert!(
        !full_build_succeeded,
        "legacy full build unexpectedly skipped GRUB lanes"
    );
    assert!(
        full_build_output.contains("GRUB2 ISO asset staging requires grub2-host"),
        "legacy full build did not fail closed on its missing lane:\n{full_build_output}"
    );

    let (stale_selection_succeeded, stale_selection_output) = run_fixture(
        "set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)\n\
         set(TEST_SOURCE_SELECTED FALSE)\n\
         set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA\n\
           aros-sdk-consumer-validation-v1)\n",
    );
    assert!(
        !stale_selection_succeeded,
        "an unvalidated consumer selection incorrectly skipped GRUB lanes"
    );
    assert!(
        stale_selection_output.contains("GRUB2 ISO asset staging requires grub2-host"),
        "unvalidated consumer selection bypassed the legacy lane guard:\n{stale_selection_output}"
    );
}
