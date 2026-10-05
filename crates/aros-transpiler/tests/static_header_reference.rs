//! Opt-in parity check for the source-owned network SDK header copies.
//!
//! This checks one actual Make owner against its declared source headers and
//! byte copies. It is not a network-library build or a full SDK qualification.

use aros_transpiler::{
    dirs::DirVars, generate_cmake, parse_mmakefile_with_dirs_and_context, DependencyGraph,
    TargetContext,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const OWNER: &str = "network-includes-copy";
const SOURCE_RELATIVE: &str = "workbench/network/common/include/mmakefile.src";
const INCLUDE_RELATIVE: &str = "workbench/network/common/include";
const INCLUDE_GLOBS: &[&str] = &[
    "*.h",
    "arpa/*.h",
    "bsdsocket/*.h",
    "clib/*.h",
    "defines/*.h",
    "libraries/*.h",
    "net/*.h",
    "netinet/*.h",
    "proto/*.h",
    "sys/*.h",
];
const INCLUDE_LIST_ASSIGNMENT: &str = "INCLUDES      := $(call WILDCARD, *.h arpa/*.h bsdsocket/*.h clib/*.h defines/*.h libraries/*.h net/*.h netinet/*.h proto/*.h sys/*.h)";

#[test]
#[ignore = "requires explicit AROS_TEST_P4_SOURCE_ROOT"]
fn actual_p4_network_header_owner_matches_exact_source_tree() {
    use std::fmt::Write as _;

    let source_root = required_source_root();
    let makefile_path = source_root.join(SOURCE_RELATIVE);
    assert!(
        makefile_path.is_file(),
        "P4 source root is missing {SOURCE_RELATIVE}"
    );
    let source_text = fs::read_to_string(&makefile_path).expect("read network include makefile");
    assert!(
        source_text.lines().any(|line| line.trim() == INCLUDE_LIST_ASSIGNMENT),
        "network static-header glob list changed; review this reference test against the source recipe"
    );
    for exact_source_fact in [
        "#MM- includes-copy : network-includes-copy",
        "#MM network-includes-copy : network-includes-setup",
        "network-includes-copy : $(DEST_INCLUDES)",
        "network-includes-setup :",
        "\t%mkdirs_q $(DIRS)",
        "$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%",
        "\t@$(CP) $< $@",
    ] {
        assert!(
            source_text.contains(exact_source_fact),
            "network static-header source fact changed: {exact_source_fact:?}"
        );
    }

    let expected_headers = enumerate_header_patterns(&source_root.join(INCLUDE_RELATIVE));
    assert!(
        !expected_headers.is_empty(),
        "the reference header patterns selected no source files"
    );
    let source_inventory = expected_headers
        .iter()
        .map(|(path, bytes)| (path.clone(), aros_common::sha256_bytes(bytes)))
        .collect::<BTreeMap<_, _>>();
    let inventory_bytes = serde_json::to_vec(&source_inventory).unwrap();
    eprintln!(
        "network static-header reference: {} files; path/SHA256 inventory {}",
        source_inventory.len(),
        aros_common::sha256_bytes(&inventory_bytes)
    );

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
    let parsed =
        parse_mmakefile_with_dirs_and_context(&makefile_path, &source_root, &dirs, &target)
            .expect("parse unchanged P4 network include makefile");
    let declarations = parsed
        .header_transforms
        .iter()
        .filter(|declaration| declaration.name == OWNER)
        .collect::<Vec<_>>();
    assert_eq!(
        declarations.len(),
        expected_headers.len(),
        "{OWNER} declarations do not cover the independent source glob set; native errors={:#?}",
        parsed.native_graph_errors
    );

    let mut declared_headers = BTreeMap::new();
    for declaration in &declarations {
        assert_eq!(declaration.file, SOURCE_RELATIVE);
        assert!(declaration.line > 0);
        assert!(declaration.copy_only, "{declaration:#?}");
        assert!(declaration.match_text.is_empty(), "{declaration:#?}");
        assert!(declaration.replacement.is_empty(), "{declaration:#?}");
        assert!(declaration.substitutions.is_empty(), "{declaration:#?}");

        let relative = declaration
            .output
            .strip_prefix("${AROS_SDK_INCLUDE_DIR}/")
            .expect("static header output must use the SDK include root");
        assert!(
            !relative.is_empty() && !relative.starts_with('/'),
            "unsafe output relative path: {relative:?}"
        );
        assert_eq!(
            declaration.input,
            format!("${{AROS_SOURCE_DIR}}/{INCLUDE_RELATIVE}/{relative}")
        );
        assert!(
            declared_headers
                .insert(relative.to_owned(), declaration)
                .is_none(),
            "duplicate declared header {relative}"
        );
    }
    assert_eq!(
        declared_headers.keys().collect::<Vec<_>>(),
        expected_headers.keys().collect::<Vec<_>>(),
        "static owner output paths differ from the independent fixed-glob expansion"
    );

    let mut graph = DependencyGraph::default();
    graph.header_transforms.extend(
        declarations
            .iter()
            .map(|declaration| (**declaration).clone()),
    );
    let generated = generate_cmake(&graph);
    let mut invocations = String::new();
    for block in generated.split("aros_transform_header(").skip(1) {
        let (arguments, _) = block
            .split_once(")\n\n")
            .expect("complete generated aros_transform_header invocation");
        writeln!(invocations, "aros_transform_header({arguments})").unwrap();
    }
    assert_eq!(
        invocations.matches("aros_transform_header(").count(),
        expected_headers.len(),
        "only the exact {OWNER} declarations should be emitted"
    );

    let temporary = tempfile::tempdir().expect("private static-header fixture root");
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical fixture root");
    let project = root.join("project");
    let build = root.join("build");
    fs::create_dir_all(&project).expect("create private CMake project");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .expect("CMake engine directory");
    let cmake_lists = format!(
        r#"cmake_minimum_required(VERSION 3.22)
project(actual_network_static_headers C)
set(_bootstrap "${{CMAKE_BINARY_DIR}}/bootstrap-stub")
file(MAKE_DIRECTORY "${{_bootstrap}}")
file(WRITE "${{_bootstrap}}/BootstrapSDK.cmake" "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(PREPEND CMAKE_MODULE_PATH "${{_bootstrap}}")
set(AROS_SOURCE_DIR "{}")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM esp32p4)
include("{}/AROS.cmake")
{invocations}
"#,
        source_root.display(),
        engine.display(),
    );
    fs::write(project.join("CMakeLists.txt"), cmake_lists).expect("write bounded CMake project");

    let configure = Command::new("cmake")
        .arg("-S")
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .expect("configure isolated static-header fixture");
    require_success(&configure, "configure actual network header fixture");
    build_owner(&build);
    assert_staged_bytes(&build, &expected_headers);

    let no_op = build_owner(&build);
    let no_op_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&no_op.stdout),
        String::from_utf8_lossy(&no_op.stderr)
    );
    assert!(
        no_op_output.contains("no work to do"),
        "static-header second build was not a no-op:\n{no_op_output}"
    );

    let first_header = expected_headers.keys().next().expect("nonempty headers");
    let staged_output = build.join("SDK/include").join(first_header);
    fs::remove_file(&staged_output).expect("remove only a fixture-generated header");
    build_owner(&build);
    assert_staged_bytes(&build, &expected_headers);
}

fn required_source_root() -> PathBuf {
    let value = std::env::var_os("AROS_TEST_P4_SOURCE_ROOT")
        .unwrap_or_else(|| panic!("AROS_TEST_P4_SOURCE_ROOT must be set explicitly"));
    PathBuf::from(value)
        .canonicalize()
        .unwrap_or_else(|error| panic!("AROS_TEST_P4_SOURCE_ROOT must name a directory: {error}"))
}

fn enumerate_header_patterns(include_root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut headers = BTreeMap::new();
    for pattern in INCLUDE_GLOBS {
        let prefix = pattern
            .strip_suffix("*.h")
            .expect("test glob must end in *.h");
        let directory_relative = prefix.strip_suffix('/').unwrap_or(prefix);
        let directory = include_root.join(directory_relative);
        if !directory.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&directory).unwrap_or_else(|error| {
            panic!("inspect header directory {}: {error}", directory.display())
        });
        assert!(
            metadata.file_type().is_dir(),
            "recipe glob directory must be a real directory: {}",
            directory.display()
        );
        for entry in fs::read_dir(&directory).unwrap_or_else(|error| {
            panic!("read header directory {}: {error}", directory.display())
        }) {
            let entry = entry.expect("read header directory entry");
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("h") {
                continue;
            }
            assert!(
                entry.file_type().expect("inspect header entry").is_file(),
                "recipe glob selected a non-regular header: {}",
                path.display()
            );
            let relative = if directory_relative.is_empty() {
                entry.file_name().to_string_lossy().into_owned()
            } else {
                format!(
                    "{directory_relative}/{}",
                    entry.file_name().to_string_lossy()
                )
            };
            assert!(
                headers
                    .insert(
                        relative.clone(),
                        fs::read(&path).expect("read source header")
                    )
                    .is_none(),
                "reference globs selected {relative} more than once"
            );
        }
    }
    headers
}

fn assert_staged_bytes(build: &Path, expected_headers: &BTreeMap<String, Vec<u8>>) {
    let sdk_include = build.join("SDK/include");
    let generated_include = build.join("GENINCDIR");
    let developer_include = build.join("SYS/Developer/include");
    let actual_headers = enumerate_header_patterns(&sdk_include);
    assert_eq!(
        actual_headers.keys().collect::<Vec<_>>(),
        expected_headers.keys().collect::<Vec<_>>(),
        "SDK output set differs from the independently expanded source glob set"
    );
    for (relative, expected) in expected_headers {
        assert_eq!(
            actual_headers.get(relative),
            Some(expected),
            "SDK header bytes changed for {relative}"
        );
        assert!(
            !generated_include.join(relative).exists(),
            "static header owner unexpectedly copied {relative} into GENINCDIR"
        );
        assert!(
            !developer_include.join(relative).exists(),
            "static header owner unexpectedly copied {relative} into Developer include"
        );
    }
    assert!(
        enumerate_header_patterns(&generated_include).is_empty(),
        "static header owner emitted unexpected GENINCDIR headers"
    );
}

fn build_owner(build: &Path) -> Output {
    let output = Command::new("cmake")
        .arg("--build")
        .arg(build)
        .args(["--target", OWNER])
        .output()
        .unwrap_or_else(|error| panic!("build {OWNER}: {error}"));
    require_success(&output, "build actual network static-header owner");
    output
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
