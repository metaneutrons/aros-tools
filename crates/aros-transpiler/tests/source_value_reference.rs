//! Read-only comparison of a selected source value recipe with GNU sed.
//! No P4 payload, SDK or hardware qualification is implied.

use aros_common::native_build_contract::load_bound_native_build_contract;
use aros_common::TargetProfile;
use aros_transpiler::{parse_mmakefile_with_context, DependencyGraph, TargetContext};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn checked(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "requires exact isolated P4 source, native contract SHA and GNU sed"]
fn actual_p4_abi_value_matches_independent_gnu_sed_without_a_constant() {
    let source = PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").unwrap())
        .canonicalize()
        .unwrap();
    let expected = std::env::var("AROS_TEST_NATIVE_CONTRACT_SHA256").unwrap();
    let profiles = TargetProfile::load_from_file(&source.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let binding = load_bound_native_build_contract(
        &source,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
    )
    .unwrap();
    assert_eq!(binding.sha256.as_str(), expected);
    let selectors = profile.transpiler.as_ref().unwrap();
    let make_variables = binding
        .contract
        .make_variables_for_host(aros_common::target::native_host_key().unwrap_or(""))
        .unwrap();
    let context = TargetContext {
        cpu: Some(binding.contract.abi.source_cpu),
        platform: Some(profile.platform.clone()),
        family: Some(selectors.family.clone()),
        variant: Some(selectors.variant.clone()),
        toolchain: Some(selectors.toolchain.clone()),
        cpu32: Some(selectors.cpu32.clone()),
        use_mmu: Some(if selectors.use_mmu { "1" } else { "0" }.into()),
        float_abi: profile.float_abi.clone(),
        make_variables,
        make_include_bindings: binding.contract.make_include_bindings,
        ..TargetContext::default()
    };
    let parsed =
        parse_mmakefile_with_context(&source.join("rom/aros/mmakefile.src"), &source, &context)
            .unwrap();
    let [declaration] = parsed.source_value_rules.as_slice() else {
        panic!(
            "expected one source value: {:#?}",
            parsed.native_graph_errors
        );
    };
    assert_eq!(declaration.owner, "kernel-aros-create-abi-file");
    assert_eq!(
        declaration.file_sha256,
        aros_common::sha256_bytes(&fs::read(source.join(&declaration.file)).unwrap()).as_str()
    );
    let input = PathBuf::from(
        declaration
            .input
            .replace("${AROS_SOURCE_DIR}", source.to_str().unwrap()),
    );
    let gnu_sed = PathBuf::from(std::env::var_os("AROS_TEST_GNU_SED").unwrap());
    assert!(
        String::from_utf8_lossy(&checked(Command::new(&gnu_sed).arg("--version")).stdout)
            .contains("GNU sed")
    );
    let first = checked(
        Command::new(&gnu_sed)
            .args(["-n", &format!("s/{}// p", declaration.marker)])
            .arg(&input),
    );
    let mut second = Command::new(&gnu_sed)
        .args(["-n", "s/^ *//;s/[ */*].*//p"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    second
        .stdin
        .take()
        .unwrap()
        .write_all(&first.stdout)
        .unwrap();
    let reference = second.wait_with_output().unwrap();
    assert!(reference.status.success());
    let temporary = tempfile::Builder::new()
        .prefix("source-value-reference.")
        .tempdir_in(
            std::env::var_os("AROS_RV3_COMPILER_EVIDENCE_PARENT")
                .map_or_else(std::env::temp_dir, PathBuf::from),
        )
        .unwrap();
    let root = temporary.keep().canonicalize().unwrap();
    fs::write(root.join("gnu-sed-reference"), &reference.stdout).unwrap();
    let mut graph = DependencyGraph::new();
    graph.source_value_rules = parsed.source_value_rules;
    let generated = aros_transpiler::generate_cmake(&graph);
    let block = generated
        .split("aros_extract_source_value(")
        .nth(1)
        .unwrap()
        .split(")\n\n")
        .next()
        .unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/SourceValueRules.cmake")
        .canonicalize()
        .unwrap();
    let project = root.join("project");
    let build = root.join("build");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("CMakeLists.txt"),
        format!(
            r#"
cmake_minimum_required(VERSION 3.22)
project(source_value_reference NONE)
set(AROS_SOURCE_DIR "{}")
set(AROS_BUILD_DIR "${{CMAKE_BINARY_DIR}}")
include("{}")
aros_extract_source_value({block})
"#,
            source.display(),
            helper.display()
        ),
    )
    .unwrap();
    let configure = checked(
        Command::new("cmake")
            .arg("-S")
            .arg(&project)
            .arg("-B")
            .arg(&build)
            .args(["-G", "Ninja"]),
    );
    fs::write(root.join("configure.stdout"), configure.stdout).unwrap();
    fs::write(root.join("configure.stderr"), configure.stderr).unwrap();
    let run = checked(
        Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", "kernel-aros-create-abi-file"]),
    );
    fs::write(root.join("build.stdout"), run.stdout).unwrap();
    fs::write(root.join("build.stderr"), run.stderr).unwrap();
    let output = build.join("SYS/Prefs/Env-Archive/ABI");
    assert_eq!(fs::read(&output).unwrap(), reference.stdout);
    fs::write(root.join("evidence.txt"), format!("qualification=source-value-only\ncontract_sha256={expected}\nrecipe_sha256={}\ninput_sha256={}\noutput_sha256={}\nno_p4_payload=true\n",
        graph.source_value_rules[0].file_sha256, aros_common::sha256_bytes(&fs::read(input).unwrap()),
        aros_common::sha256_bytes(&fs::read(output).unwrap()))).unwrap();
    println!(
        "Source-value reference evidence retained at {}",
        root.display()
    );
}
