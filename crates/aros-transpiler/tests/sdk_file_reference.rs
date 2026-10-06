//! Opt-in parity check for the source-owned Codesets SDK fd declaration.
//!
//! This exercises only the exact fetched file-copy declaration and its CMake
//! rule. It is not a Codesets build or a full SDK qualification.

use aros_transpiler::{
    dirs::DirVars, generate_cmake, parse_mmakefile_with_dirs_and_context, DependencyGraph,
    TargetContext,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime},
};

const OWNER: &str = "workbench-libs-codesets-fd";
const FETCH: &str = "workbench-libs-codesets-fetch";
const SOURCE_RELATIVE: &str = "workbench/libs/codesets/mmakefile.src";
const FILE_NAME: &str = "codesets_lib.fd";

#[test]
#[ignore = "requires explicit isolated P4 source and native contract digest"]
fn actual_p4_autoinit_specs_copy_matches_source_bytes_and_repairs_missing_output() {
    use aros_common::{native_build_contract::load_bound_native_build_contract, TargetProfile};

    let root = required_directory("AROS_TEST_P4_SOURCE_ROOT");
    let profiles = TargetProfile::load_from_file(&root.join("aros-targets.toml")).unwrap();
    let profile = profiles
        .iter()
        .find(|profile| profile.name == "esp32p4-d1001")
        .unwrap();
    let loaded = load_bound_native_build_contract(
        &root,
        Path::new(profile.native_build_contract.as_deref().unwrap()),
        profile,
    )
    .unwrap();
    assert_eq!(
        loaded.sha256.as_str(),
        std::env::var("AROS_TEST_NATIVE_CONTRACT_SHA256").unwrap()
    );
    for path in ["compiler/autoinit/auto", "compiler/autoinit/mmakefile.src"] {
        assert!(loaded
            .contract
            .inputs
            .iter()
            .any(|input| input.path == path));
    }
    let selectors = profile.transpiler.as_ref().unwrap();
    let make_variables = loaded
        .contract
        .make_variables_for_host(aros_common::target::native_host_key().unwrap_or(""))
        .unwrap();
    let target = TargetContext {
        cpu: Some(loaded.contract.abi.source_cpu),
        platform: Some(profile.platform.clone()),
        family: Some(selectors.family.clone()),
        variant: Some(selectors.variant.clone()),
        toolchain: Some(selectors.toolchain.clone()),
        cpu32: Some(selectors.cpu32.clone()),
        use_mmu: Some(if selectors.use_mmu { "1" } else { "0" }.into()),
        float_abi: profile.float_abi.clone(),
        make_variables,
        make_include_bindings: loaded.contract.make_include_bindings,
        ..TargetContext::default()
    };
    let recipe = root.join("compiler/autoinit/mmakefile.src");
    let parsed =
        parse_mmakefile_with_dirs_and_context(&recipe, &root, &DirVars::load(&root), &target)
            .unwrap();
    let selected = parsed
        .sdk_file_copies
        .iter()
        .filter(|copy| copy.owner == "linklibs-autoinit-autofile")
        .collect::<Vec<_>>();
    let [copy] = selected.as_slice() else {
        panic!(
            "copies={:#?}; errors={:#?}",
            parsed.sdk_file_copies, parsed.native_graph_errors
        )
    };
    assert_eq!(copy.source_dir, "${AROS_SOURCE_DIR}/compiler/autoinit");
    assert_eq!(copy.destination, "${AROS_DEVELOPER_LIB_DIR}");
    assert_eq!(copy.files, ["auto"]);
    assert_eq!(copy.fetch_owner, None);
    let original = fs::read(root.join("compiler/autoinit/auto")).unwrap();
    let digest = aros_common::sha256_bytes(&original);
    let mut graph = DependencyGraph::default();
    graph.sdk_file_copies.push((*copy).clone());
    let invocation = only_sdk_file_copy_invocation(&generate_cmake(&graph));
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let build = temp.path().join("build");
    fs::create_dir(&project).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/SdkFileCopies.cmake");
    fs::write(
        project.join("CMakeLists.txt"),
        format!(
            r#"
cmake_minimum_required(VERSION 3.22)
project(actual_autoinit_copy NONE)
set(AROS_SOURCE_DIR "{}")
set(AROS_DEVELOPER_LIB_DIR "${{CMAKE_BINARY_DIR}}/SYS/Developer/lib")
include("{}")
{invocation}
"#,
            root.display(),
            helper.display()
        ),
    )
    .unwrap();
    let result = Command::new("cmake")
        .arg("-S")
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .unwrap();
    require_success(&result, "configure actual source autoinit copy");
    let output = build.join("SYS/Developer/lib/auto");
    build_target(&build, &copy.owner);
    assert_eq!(fs::read(&output).unwrap(), original);
    let before = fs::metadata(&output).unwrap().modified().unwrap();
    build_target(&build, &copy.owner);
    assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), before);
    fs::remove_file(&output).unwrap();
    build_target(&build, &copy.owner);
    assert_eq!(fs::read(output).unwrap(), original);
    assert_eq!(
        aros_common::sha256_bytes(&fs::read(root.join("compiler/autoinit/auto")).unwrap()),
        digest
    );
    println!("actual source autoinit specs: {} bytes, SHA256 {digest}; byte-exact staging/no-op/repair pass", original.len());
}

#[test]
#[ignore = "requires explicit P4 source and cached Codesets input roots"]
fn actual_p4_codesets_fd_copy_stages_exact_fetched_file_bytes() {
    let source_root = required_directory("AROS_TEST_P4_SOURCE_ROOT");
    let cached_input = required_codesets_input();
    let source_path = source_root.join(SOURCE_RELATIVE);
    assert!(
        source_path.is_file(),
        "P4 source root is missing {SOURCE_RELATIVE}"
    );

    let input_bytes = fs::read(&cached_input).expect("read cached Codesets fd input");
    let input_digest = aros_common::sha256_bytes(&input_bytes);

    let dirs = DirVars::load(&source_root);
    let target = TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("gnu".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("0".into()),
        float_abi: Some("ilp32f".into()),
        ..TargetContext::default()
    };
    let parsed = parse_mmakefile_with_dirs_and_context(&source_path, &source_root, &dirs, &target)
        .expect("parse unchanged P4 Codesets mmakefile");
    let declarations = parsed
        .sdk_file_copies
        .iter()
        .filter(|declaration| declaration.owner == OWNER)
        .collect::<Vec<_>>();
    let [declaration] = declarations.as_slice() else {
        panic!(
            "expected one {OWNER} declaration; copies={:#?}; native errors={:#?}",
            parsed.sdk_file_copies, parsed.native_graph_errors
        );
    };
    assert_eq!(declaration.file, SOURCE_RELATIVE);
    let joined_source = aros_transpiler::parser::join_continuations(
        &fs::read_to_string(&source_path).expect("read actual Codesets declaration"),
    );
    let declaration_line = joined_source
        .lines()
        .position(|line| line.starts_with("%copy_files_q mmake=workbench-libs-codesets-fd "))
        .expect("locate exact source copy in continuation-joined coordinates")
        + 1;
    assert_eq!(declaration.line, declaration_line);
    assert_eq!(declaration.fetch_owner.as_deref(), Some(FETCH));
    assert_eq!(
        declaration.source_dir,
        "${AROS_PORTS_DIR}/codesets/libcodesets-6.22/developer/fd"
    );
    assert_eq!(declaration.destination, "${AROS_DEVELOPER_FD_DIR}");
    assert_eq!(declaration.files, vec![FILE_NAME.to_owned()]);

    // Keep generation limited to this source declaration. In particular, do
    // not add whole-graph roots or unrelated SDK finalizers to the fixture.
    let mut graph = DependencyGraph::default();
    graph.sdk_file_copies.push((*declaration).clone());
    let generated = generate_cmake(&graph);
    let invocation = only_sdk_file_copy_invocation(&generated);
    for expected in [
        "NAME \"workbench-libs-codesets-fd\"",
        "SOURCE \"${AROS_PORTS_DIR}/codesets/libcodesets-6.22/developer/fd\"",
        "DESTINATION \"${AROS_DEVELOPER_FD_DIR}\"",
        "FETCH \"workbench-libs-codesets-fetch\"",
        "codesets_lib.fd",
    ] {
        assert!(
            invocation.contains(expected),
            "missing {expected:?}:\n{invocation}"
        );
    }

    let temporary = tempfile::tempdir().expect("private SDK-copy fixture root");
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical fixture root");
    let project = root.join("project");
    let build = root.join("build");
    let fetch_destination = build.join("Ports/codesets");
    let fetched_file = fetch_destination
        .join("libcodesets-6.22/developer/fd")
        .join(FILE_NAME);
    fs::create_dir_all(&project).expect("create CMake project");
    fs::create_dir_all(fetched_file.parent().expect("fetched file parent"))
        .expect("create private fetched-source directory");
    fs::copy(&cached_input, &fetched_file).expect("stage cached file as fetched input");
    fs::write(
        fetch_destination.join(".complete"),
        b"fixture fetch receipt\n",
    )
    .expect("write fetch completion stamp");

    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine/SdkFileCopies.cmake");
    let cmake_lists = format!(
        r#"
cmake_minimum_required(VERSION 3.22)
project(actual_codesets_fd_copy NONE)
include("{}")
set(AROS_SOURCE_DIR "{}")
set(AROS_BUILD_DIR "${{CMAKE_BINARY_DIR}}")
set(AROS_PORTS_DIR "${{CMAKE_BINARY_DIR}}/Ports")
set(AROS_DEVELOPER_FD_DIR "${{CMAKE_BINARY_DIR}}/SYS/Developer/SDK/fd")
add_custom_target({FETCH})
set_property(GLOBAL PROPERTY AROS_FETCH_TARGETS {FETCH})
set_property(TARGET {FETCH} PROPERTY AROS_FETCH_DESTINATION "${{AROS_PORTS_DIR}}/codesets")
set_property(TARGET {FETCH} PROPERTY AROS_FETCH_COMPLETION_STAMP "${{AROS_PORTS_DIR}}/codesets/.complete")
{invocation}
"#,
        helper.display(),
        source_root.display(),
        FETCH = FETCH,
        invocation = invocation,
    );
    fs::write(project.join("CMakeLists.txt"), cmake_lists).expect("write bounded CMake fixture");

    let configure = Command::new("cmake")
        .arg("-S")
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .expect("run CMake configure");
    require_success(&configure, "configure bounded SDK-file fixture");

    let output = build.join("SYS/Developer/SDK/fd").join(FILE_NAME);
    build_target(&build, OWNER);
    assert_eq!(fs::read(&output).expect("read staged fd file"), input_bytes);

    // The producer must repair a missing declared output and rebuild from an
    // updated fetched input without touching the cached original.
    fs::remove_file(&output).expect("remove only the fixture's staged output");
    build_target(&build, OWNER);
    assert_eq!(
        fs::read(&output).expect("read repaired fd file"),
        input_bytes
    );

    let updated_input = b"fixture-only updated Codesets fd bytes\n\0";
    fs::write(&fetched_file, updated_input).expect("mutate only the fixture's fetched copy");
    let staged_file = fs::OpenOptions::new()
        .write(true)
        .open(&fetched_file)
        .expect("open fixture input to set its timestamp");
    staged_file
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(2)))
        .expect("make mutated fixture input newer than its output");
    build_target(&build, OWNER);
    assert_eq!(
        fs::read(&output).expect("read rebuilt fd file"),
        updated_input
    );

    assert_eq!(
        aros_common::sha256_bytes(&fs::read(&cached_input).expect("reread cached input")),
        input_digest,
        "cached source input changed during the fixture"
    );
}

fn required_directory(name: &str) -> PathBuf {
    let value = std::env::var_os(name).unwrap_or_else(|| panic!("{name} must be set explicitly"));
    PathBuf::from(value)
        .canonicalize()
        .unwrap_or_else(|error| panic!("{name} must name a readable directory: {error}"))
}

fn required_codesets_input() -> PathBuf {
    let value = std::env::var_os("AROS_TEST_CODESETS_INPUT")
        .unwrap_or_else(|| panic!("AROS_TEST_CODESETS_INPUT must be set explicitly"));
    let path = PathBuf::from(value);
    let metadata = fs::symlink_metadata(&path)
        .unwrap_or_else(|error| panic!("cannot inspect cached Codesets input: {error}"));
    assert!(
        metadata.file_type().is_file(),
        "AROS_TEST_CODESETS_INPUT must be a regular non-symlink file"
    );
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|error| panic!("cannot canonicalize cached Codesets input: {error}"));
    assert_eq!(
        canonical.file_name().and_then(|name| name.to_str()),
        Some(FILE_NAME),
        "AROS_TEST_CODESETS_INPUT must name the exact {FILE_NAME} input"
    );
    let normalized = canonical.to_string_lossy().replace('\\', "/");
    assert!(
        normalized.ends_with("Ports/codesets/libcodesets-6.22/developer/fd/codesets_lib.fd"),
        "AROS_TEST_CODESETS_INPUT is not under the expected cached Codesets tree: {normalized}"
    );
    canonical
}

fn only_sdk_file_copy_invocation(generated: &str) -> String {
    let parts = generated.split("aros_stage_sdk_files(").collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        2,
        "expected only one generated SDK-file invocation:\n{generated}"
    );
    let (arguments, _) = parts[1]
        .split_once(")\n\n")
        .expect("complete generated SDK-file invocation");
    format!("aros_stage_sdk_files({arguments})")
}

fn build_target(build: &Path, target: &str) {
    let output = Command::new("cmake")
        .arg("--build")
        .arg(build)
        .args(["--target", target])
        .output()
        .unwrap_or_else(|error| panic!("build {target}: {error}"));
    require_success(&output, &format!("build SDK-file target {target}"));
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
