//! Opt-in parity check against the exact cached FreeType inputs used by the
//! P4 source checkout. This verifies the transpiler's source-derived CMake
//! products against independent GNU sed invocations; it is not a board build.

use aros_transpiler::{
    dirs::DirVars,
    generate_cmake, parse_mmakefile_with_dirs_and_context,
    source_text_rules::{SourceTextOperation, SourceTextOutput, SourceTextRuleDecl},
    DependencyGraph, TargetContext,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const OWNER: &str = "workbench-libs-freetype-genincludes";
const FETCH: &str = "freetype2-fetch";
const VERSION: &str = "2.14.3";

#[test]
#[ignore = "requires explicit P4 source and cached FreeType input roots"]
fn actual_p4_freetype_source_text_matches_independent_gnu_sed() {
    use std::fmt::Write as _;
    let source_root = required_directory("AROS_TEST_P4_SOURCE_ROOT");
    let input_root = required_directory("AROS_TEST_FREETYPE_INPUT_ROOT");
    assert!(
        source_root
            .join("workbench/libs/freetype2/mmakefile.src")
            .is_file(),
        "P4 source root is missing the FreeType mmakefile"
    );
    assert!(
        input_root
            .join("include/freetype/config/ftoption.h")
            .is_file(),
        "cached FreeType input root is missing ftoption.h"
    );
    assert!(
        input_root.join("builds/unix/freetype-config.in").is_file(),
        "cached FreeType input root is missing freetype-config.in"
    );

    let source_path = source_root.join("workbench/libs/freetype2/mmakefile.src");
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
        .expect("parse unchanged P4 FreeType mmakefile");
    let declarations = parsed
        .source_text_rules
        .iter()
        .filter(|declaration| declaration.owner == OWNER)
        .collect::<Vec<_>>();
    let [declaration] = declarations.as_slice() else {
        panic!(
            "expected one {OWNER} declaration; declarations={:#?}; native errors={:#?}",
            parsed.source_text_rules, parsed.native_graph_errors
        );
    };
    assert_eq!(declaration.fetch_owner, FETCH);
    assert_eq!(declaration.outputs.len(), 2, "{declaration:#?}");

    let option_header = output_named(declaration, "ftoption.h");
    let config_script = output_named(declaration, "freetype-config");
    assert_eq!(
        option_header.output,
        "${AROS_SDK_INCLUDE_DIR}/freetype/config/ftoption.h"
    );
    assert_eq!(
        option_header.input,
        format!(
            "${{AROS_PORTS_DIR}}/freetype2/freetype-{VERSION}/include/freetype/config/ftoption.h"
        )
    );
    assert_eq!(option_header.operations.len(), 4);
    assert_eq!(option_header.mode, None);
    assert_eq!(
        config_script.output,
        "${AROS_BUILD_DIR}/hosttools/${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}/freetype-config"
    );
    assert_eq!(
        config_script.input,
        format!("${{AROS_PORTS_DIR}}/freetype2/freetype-{VERSION}/builds/unix/freetype-config.in")
    );
    assert_eq!(config_script.operations.len(), 10);
    assert_eq!(config_script.mode.as_deref(), Some("744"));
    assert_eq!(
        operation_tokens(&option_header.operations),
        [
            "FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES",
            "FT_CONFIG_OPTION_SUBPIXEL_RENDERING",
            "FT_CONFIG_OPTION_SYSTEM_ZLIB",
            "FT_CONFIG_OPTION_USE_PNG",
        ]
    );
    assert_eq!(
        operation_tokens(&config_script.operations),
        [
            "%PKG_CONFIG%",
            "dynamic_libs=\"-lfreetype\"",
            "%LIBSSTATIC_CONFIG%",
            "%prefix%",
            "%exec_prefix%",
            "%includedir%",
            "%libdir%",
            "includedir/freetype2",
            "includedir/freetype/freetype/freetype",
            "%ft_version%",
        ]
    );

    let build_root = tempfile::tempdir().expect("private CMake build root");
    let project = build_root.path().join("project");
    let build = build_root.path().join("build");
    let port_destination = build.join("Ports/freetype2");
    copy_exact_template_tree(&input_root, &port_destination);
    fs::create_dir_all(&port_destination).expect("fetch destination");
    fs::write(
        port_destination.join(".complete"),
        b"fixture fetch receipt\n",
    )
    .expect("fixture completion stamp");
    fs::create_dir_all(&project).expect("CMake project directory");

    let mut graph = DependencyGraph::default();
    graph.source_text_rules.push((*declaration).clone());
    let generated = generate_cmake(&graph);
    assert_eq!(
        generated.matches("aros_transform_source_text(").count(),
        2,
        "{generated}"
    );
    let generated_cmake = project.join("source-text-products.cmake");
    // Execute the two generated declarations without unrelated whole-graph
    // finalizers: this fixture qualifies this capability, not the full SDK.
    let transforms = generated.split("aros_transform_source_text(").skip(1).fold(
        String::new(),
        |mut result, block| {
            let (arguments, _) = block
                .split_once(")\n\n")
                .expect("complete generated source-text invocation");
            writeln!(result, "aros_transform_source_text({arguments})").unwrap();
            result
        },
    );
    assert_eq!(transforms.matches("aros_transform_source_text(").count(), 2);
    fs::write(&generated_cmake, transforms).expect("write generated source-text declarations");

    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .expect("CMake engine directory");
    let cmake_lists = format!(
        r#"cmake_minimum_required(VERSION 3.22)
project(actual_freetype_source_text NONE)
include("{source_text_helper}")
set(AROS_BUILD_DIR "${{CMAKE_BINARY_DIR}}")
set(AROS_PORTS_DIR "${{CMAKE_BINARY_DIR}}/Ports")
set(AROS_DEVELOPER_INCLUDE_DIR "${{CMAKE_BINARY_DIR}}/SYS/Developer/include")
set(AROS_SDK_INCLUDE_DIR "${{CMAKE_BINARY_DIR}}/SDK/include")
set(AROS_GENINC_DIR "${{CMAKE_BINARY_DIR}}/GENINCDIR")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM esp32p4)
add_custom_target({FETCH})
set_property(TARGET {FETCH} PROPERTY AROS_FETCH_DESTINATION "${{AROS_PORTS_DIR}}/freetype2")
set_property(TARGET {FETCH} PROPERTY AROS_FETCH_COMPLETION_STAMP "${{AROS_PORTS_DIR}}/freetype2/.complete")
include("{generated_cmake}")
"#,
        source_text_helper = engine.join("SourceTextRules.cmake").display(),
        generated_cmake = generated_cmake.display(),
    );
    fs::write(project.join("CMakeLists.txt"), cmake_lists).expect("write isolated CMake fixture");

    run_success(
        Command::new("cmake")
            .arg("-S")
            .arg(&project)
            .arg("-B")
            .arg(&build)
            .args(["-G", "Ninja"]),
        "configure actual FreeType declarations",
    );
    run_success(
        Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", OWNER]),
        "build actual FreeType source-text products",
    );

    let gnu_sed = gnu_sed_program();
    let expected_header = run_sed(
        &gnu_sed,
        &[
            r"s|.*FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES.*|/*define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES*/\n|g",
            r"s|.*FT_CONFIG_OPTION_SUBPIXEL_RENDERING.*|#define FT_CONFIG_OPTION_SUBPIXEL_RENDERING\n|g",
            r"s|.*FT_CONFIG_OPTION_SYSTEM_ZLIB.*|#define FT_CONFIG_OPTION_SYSTEM_ZLIB\n|g",
            r"s|.*FT_CONFIG_OPTION_USE_PNG.*|#define FT_CONFIG_OPTION_USE_PNG\n|g",
        ],
        &input_root.join("include/freetype/config/ftoption.h"),
    );
    let developer_prefix = build.join("SYS/Developer");
    let include_root = build.join("SDK/include");
    let library_root = developer_prefix.join("lib");
    let script_expressions = [
        "s|%PKG_CONFIG%|false|g".to_owned(),
        r#"s|dynamic_libs="-lfreetype"|dynamic_libs="-lfreetype2"|g"#.to_owned(),
        "s|%LIBSSTATIC_CONFIG%|-lfreetype2.static -lpng -lz|g".to_owned(),
        format!("s|%prefix%|{}|g", developer_prefix.display()),
        format!("s|%exec_prefix%|{}|g", developer_prefix.display()),
        format!("s|%includedir%|{}|g", include_root.display()),
        format!("s|%libdir%|{}|g", library_root.display()),
        "s|includedir/freetype2|includedir/freetype|g".to_owned(),
        "s|includedir/freetype/freetype/freetype|includedir/freetype/freetype|g".to_owned(),
        format!("s|%ft_version%|{VERSION}|g"),
    ];
    let expected_script = run_sed(
        &gnu_sed,
        &script_expressions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        &input_root.join("builds/unix/freetype-config.in"),
    );

    let actual_header = build.join("SDK/include/freetype/config/ftoption.h");
    let actual_script = build.join("hosttools/riscv-esp32p4/freetype-config");
    assert_eq!(
        fs::read(&actual_header).expect("generated FreeType option header"),
        expected_header,
        "generated ftoption.h differs from independent GNU sed"
    );
    assert_eq!(
        fs::read(&actual_script).expect("generated freetype-config script"),
        expected_script,
        "generated freetype-config differs from independent GNU sed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(actual_script)
                .expect("script metadata")
                .permissions()
                .mode()
                & 0o777,
            0o744
        );
    }
}

fn required_directory(name: &str) -> PathBuf {
    let value = std::env::var_os(name).unwrap_or_else(|| panic!("{name} must be set explicitly"));
    PathBuf::from(value)
        .canonicalize()
        .unwrap_or_else(|error| panic!("{name} must name a readable directory: {error}"))
}

fn output_named<'a>(declaration: &'a SourceTextRuleDecl, name: &str) -> &'a SourceTextOutput {
    let matching = declaration
        .outputs
        .iter()
        .filter(|output| output.output.ends_with(name))
        .collect::<Vec<_>>();
    let [output] = matching.as_slice() else {
        panic!("expected one {name} output, got {matching:#?}");
    };
    output
}

fn operation_tokens(operations: &[SourceTextOperation]) -> Vec<&str> {
    operations
        .iter()
        .map(|operation| match operation {
            SourceTextOperation::ReplaceAll { token, .. }
            | SourceTextOperation::ReplaceWholeLineContaining { token, .. } => token.as_str(),
        })
        .collect()
}

fn copy_exact_template_tree(source: &Path, destination: &Path) {
    for relative in [
        Path::new("include/freetype/config/ftoption.h"),
        Path::new("builds/unix/freetype-config.in"),
    ] {
        let input = source.join(relative);
        let output = destination
            .join(format!("freetype-{VERSION}"))
            .join(relative);
        fs::create_dir_all(output.parent().expect("template parent"))
            .expect("create private fetched-template directory");
        fs::copy(input, output).expect("copy exact cached template bytes");
    }
}

fn gnu_sed_program() -> PathBuf {
    let program =
        std::env::var_os("AROS_TEST_GNU_SED").map_or_else(|| PathBuf::from("gsed"), PathBuf::from);
    let version = Command::new(&program)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "GNU sed executable {} is unavailable: {error}",
                program.display()
            )
        });
    assert!(
        version.status.success() && String::from_utf8_lossy(&version.stdout).contains("GNU sed"),
        "reference sed must be GNU sed: {:?}",
        String::from_utf8_lossy(&version.stdout)
    );
    program
}

fn run_sed(program: &Path, expressions: &[&str], input: &Path) -> Vec<u8> {
    let mut command = Command::new(program);
    for expression in expressions {
        command.arg("-e").arg(expression);
    }
    let output = command
        .arg(input)
        .output()
        .unwrap_or_else(|error| panic!("run GNU sed reference: {error}"));
    assert!(
        output.status.success(),
        "GNU sed reference failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn run_success(command: &mut Command, what: &str) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        output.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
