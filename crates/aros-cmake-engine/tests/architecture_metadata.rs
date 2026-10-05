use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    build: PathBuf,
    generated: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let temporary = tempfile::tempdir().expect("create CMake fixture");
        let source = temporary.path().join("source");
        let build = temporary.path().join("build");
        let generated = build.join("gen");
        fs::create_dir_all(&source).expect("create source root");
        fs::create_dir_all(&generated).expect("create generated root");
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("engine")
            .join("ArchitectureMetadata.cmake");
        let project = format!(
            "cmake_minimum_required(VERSION 3.21)\nproject(ArchitectureMetadataFixture NONE)\ninclude({})\nset(AROS_SOURCE_DIR {})\nset(AROS_GEN_DIR {})\n{}\n",
            cmake_quote(&helper),
            cmake_quote(&source),
            cmake_quote(&generated),
            body
        );
        fs::write(source.join("CMakeLists.txt"), project).expect("write CMake project");
        Self {
            _temporary: temporary,
            source,
            build,
            generated,
        }
    }

    fn configure(&self) -> Output {
        Command::new("cmake")
            .arg("-S")
            .arg(&self.source)
            .arg("-B")
            .arg(&self.build)
            .arg("-G")
            .arg("Ninja")
            .output()
            .expect("run CMake configure")
    }

    fn build_target(&self, target: &str) -> Output {
        Command::new("cmake")
            .arg("--build")
            .arg(&self.build)
            .arg("--target")
            .arg(target)
            .output()
            .expect("build CMake target")
    }

    fn with_aros_arch_include_tags() -> Self {
        let aros_cmake = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("engine")
            .join("AROS.cmake");
        let source = fs::read_to_string(aros_cmake).expect("read AROS.cmake");
        let start = source
            .find("if(NOT DEFINED AROS_TARGET_FAMILY)")
            .expect("find architecture family setup");
        let end = source[start..]
            .find("# aros_gate_arch(")
            .map(|offset| start + offset)
            .expect("find end of architecture tag setup");
        let selector = &source[start..end];
        let body = format!(
            r#"
set(AROS_TARGET_PLATFORM pc)
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_FAMILY unix)
{selector}
file(WRITE "${{CMAKE_BINARY_DIR}}/explicit-family-tags.txt" "${{AROS_ARCH_INCLUDE_TAGS}}")

unset(AROS_TARGET_FAMILY)
{selector}
file(WRITE "${{CMAKE_BINARY_DIR}}/missing-family-tags.txt" "${{AROS_ARCH_INCLUDE_TAGS}}")
"#
        );
        Self::new(&body)
    }
}

fn cmake_quote(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('"', "\\\"")
    )
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_configure_rejected(body: &str, expected: &str) {
    let fixture = Fixture::new(body);
    let output = fixture.configure();
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "unexpected configure success:\n{text}"
    );
    let normalized_text = normalize_whitespace(&text);
    let normalized_expected = normalize_whitespace(expected);
    assert!(
        normalized_text.contains(&normalized_expected),
        "expected diagnostic {expected:?}, got:\n{text}"
    );
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn empty_arch_linklib_is_a_noop_aggregate_with_its_include_prerequisite() {
    let body = r#"
add_custom_target(arch-includes
    COMMAND "${CMAKE_COMMAND}" -E touch "${CMAKE_BINARY_DIR}/includes.ready")
aros_empty_arch_linklib(NAME arch-riscv-linklib INCLUDE_TARGET arch-includes)
get_target_property(_empty_type arch-riscv-linklib TYPE)
file(WRITE "${CMAKE_BINARY_DIR}/empty-type.txt" "${_empty_type}")
"#;
    let fixture = Fixture::new(body);
    let configure = fixture.configure();
    let configure_text = output_text(&configure);
    assert!(
        configure.status.success(),
        "CMake configure failed:\n{configure_text}"
    );
    assert_eq!(
        fs::read_to_string(fixture.build.join("empty-type.txt")).expect("read target type"),
        "UTILITY"
    );

    let build = fixture.build_target("arch-riscv-linklib");
    let build_text = output_text(&build);
    assert!(build.status.success(), "CMake build failed:\n{build_text}");
    assert!(fixture.build.join("includes.ready").is_file());
}

#[test]
fn set_archincludes_writes_exact_make_template_content_as_a_build_output() {
    let fixture = Fixture::new(
        r#"
aros_set_archincludes_endpoint(
    NAME kernel-riscv-set-archincludes
    MODNAME kernel
    MAINDIR rom/kernel
    PRIORITY 05
    TAG riscv_esp32p4
    INCLUDE_DIRS arch/riscv-all/kernel "${AROS_GEN_DIR}/future/include"
)
aros_set_archincludes_endpoint(
    NAME empty-riscv-set-archincludes
    MODNAME exec
    MAINDIR rom/exec
    PRIORITY 0
    TAG riscv
)
aros_set_archincludes_endpoint(
    NAME 0
    MODNAME 0
    MAINDIR 0
    PRIORITY 0
    TAG 0
)
"#,
    );
    fs::create_dir_all(fixture.source.join("arch/riscv-all/kernel"))
        .expect("create source include directory");
    let configure = fixture.configure();
    let configure_text = output_text(&configure);
    assert!(
        configure.status.success(),
        "CMake configure failed:\n{configure_text}"
    );

    for target in [
        "kernel-riscv-set-archincludes",
        "empty-riscv-set-archincludes",
        "0",
    ] {
        let build = fixture.build_target(target);
        let build_text = output_text(&build);
        assert!(
            build.status.success(),
            "{target} build failed:\n{build_text}"
        );
    }

    let source = fs::canonicalize(&fixture.source).expect("canonicalize source root");
    let output = fixture
        .generated
        .join("rom/kernel/kernel/include/.kernel.includeflag.05.riscv_esp32p4");
    let expected = format!(
        "-I{}/arch/riscv-all/kernel -I{}/future/include \n",
        source.display(),
        fixture.generated.display()
    );
    assert_eq!(
        fs::read_to_string(output).expect("read generated include flag"),
        expected
    );

    let empty_output = fixture
        .generated
        .join("rom/exec/exec/include/.exec.includeflag.0.riscv");
    assert_eq!(
        fs::read_to_string(empty_output).expect("read empty include flag"),
        " \n"
    );
    assert_eq!(
        fs::read_to_string(fixture.generated.join("0/0/include/.0.includeflag.0.0"))
            .expect("read zero-valued selector output"),
        " \n"
    );
}

#[test]
fn architecture_include_tags_add_only_an_explicit_nonempty_family() {
    let fixture = Fixture::with_aros_arch_include_tags();
    let configure = fixture.configure();
    assert!(configure.status.success(), "{}", output_text(&configure));

    assert_eq!(
        fs::read_to_string(fixture.build.join("explicit-family-tags.txt"))
            .expect("read explicitly configured family tags"),
        "pc-x86_64;pc;x86_64;unix;native"
    );
    assert_eq!(
        fs::read_to_string(fixture.build.join("missing-family-tags.txt"))
            .expect("read tags without a family"),
        "pc-x86_64;pc;x86_64;native"
    );
}

#[test]
fn architecture_metadata_preserves_literal_definitions_and_their_order() {
    let fixture = Fixture::new(
        "aros_set_archincludes_endpoint(NAME flags MODNAME kernel MAINDIR rom/kernel PRIORITY 2 TAG generic DEFINITIONS BOARD=1 FEATURE FEATURE=2 EMPTY=)",
    );
    let configure = fixture.configure();
    assert!(configure.status.success(), "{}", output_text(&configure));
    let build = fixture.build_target("flags");
    assert!(build.status.success(), "{}", output_text(&build));
    assert_eq!(
        fs::read_to_string(
            fixture
                .build
                .join("gen/rom/kernel/kernel/include/.kernel.includeflag.2.generic")
        )
        .unwrap(),
        "-DBOARD=1 -DFEATURE -DFEATURE=2 -DEMPTY= \n",
    );
}

#[test]
fn architecture_metadata_rejects_unsafe_definition_tokens() {
    for definition in [
        "9INVALID=1",
        "NAME=$<CONFIG>",
        "NAME=hello world",
        "NAME=1;injected",
    ] {
        let body = format!("aros_set_archincludes_endpoint(NAME flags MODNAME kernel MAINDIR rom/kernel PRIORITY 2 TAG generic DEFINITIONS [==[{definition}]==])");
        assert_configure_rejected(&body, "invalid preprocessor definition");
    }
}

#[test]
fn empty_arch_linklib_rejects_missing_arguments_targets_and_duplicates() {
    assert_configure_rejected(
        "aros_empty_arch_linklib(NAME incomplete)",
        "Incomplete empty architecture linklib declaration",
    );
    assert_configure_rejected(
        "aros_empty_arch_linklib(NAME missing INCLUDE_TARGET not-created)",
        "include target not-created does not exist",
    );

    assert_configure_rejected(
        "add_custom_target(includes)\naros_empty_arch_linklib(NAME duplicate INCLUDE_TARGET includes)\naros_empty_arch_linklib(NAME duplicate INCLUDE_TARGET includes)",
        "target already exists",
    );
}

#[test]
fn archincludes_rejects_incomplete_duplicate_and_escaping_declarations() {
    assert_configure_rejected(
        "aros_set_archincludes_endpoint(NAME incomplete)",
        "Incomplete architecture include-flag declaration",
    );

    let duplicate_output = r"
aros_set_archincludes_endpoint(NAME first MODNAME kernel MAINDIR rom/kernel PRIORITY 5 TAG riscv INCLUDE_DIRS include)
aros_set_archincludes_endpoint(NAME second MODNAME kernel MAINDIR rom/kernel PRIORITY 5 TAG riscv INCLUDE_DIRS include)
";
    assert_configure_rejected(
        duplicate_output,
        "architecture include-flag output collision",
    );

    for (body, expected) in [
        (
            "aros_set_archincludes_endpoint(NAME escape MODNAME kernel MAINDIR ../outside PRIORITY 5 TAG riscv)",
            "MAINDIR escapes its generated root",
        ),
        (
            "aros_set_archincludes_endpoint(NAME escape MODNAME kernel MAINDIR rom/kernel PRIORITY 5 TAG riscv INCLUDE_DIRS ../outside)",
            "INCLUDE_DIRS entry escapes its allowed root",
        ),
        (
            "aros_set_archincludes_endpoint(NAME outside MODNAME kernel MAINDIR rom/kernel PRIORITY 5 TAG riscv INCLUDE_DIRS /tmp/outside/include)",
            "outside AROS_SOURCE_DIR and AROS_GEN_DIR",
        ),
        (
            "aros_set_archincludes_endpoint(NAME unresolved MODNAME kernel MAINDIR rom/kernel PRIORITY 5 TAG riscv INCLUDE_DIRS [==[${UNDECLARED_ROOT}/include]==])",
            "invalid or unresolved INCLUDE_DIRS entry",
        ),
    ] {
        assert_configure_rejected(body, expected);
    }
}
