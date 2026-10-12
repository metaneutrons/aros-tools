use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    build: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let temporary = tempfile::tempdir().expect("create CMake fixture root");
        let source = temporary.path().join("source");
        let build = temporary.path().join("build");
        fs::create_dir_all(&source).expect("create fixture source directory");
        fs::write(
            source.join("owner.c"),
            "int owner_anchor(void) { return 0; }\n",
        )
        .expect("write owner source");
        fs::write(source.join("selected.c"), selected_source()).expect("write selected source");
        fs::write(
            source.join("main.c"),
            "int arch_value(void);\nint main(void) { return arch_value() == 44 ? 0 : 1; }\n",
        )
        .expect("write main source");
        fs::write(
            source.join("other.c"),
            "int other_source(void) { return 0; }\n",
        )
        .expect("write unrelated source");
        fs::write(source.join("external.o"), "external object fixture\n")
            .expect("write external object fixture");

        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("engine")
            .join("ArchitectureEndpoints.cmake");
        let project = format!(
            "cmake_minimum_required(VERSION 3.22)\nproject(ArchitectureEndpointFixture C)\nset(ENGINE {})\ninclude({})\n{}\n",
            cmake_quote(helper.parent().expect("engine directory")),
            cmake_quote(&helper),
            body
        );
        fs::write(source.join("CMakeLists.txt"), project).expect("write CMake project");

        Self {
            _temporary: temporary,
            source,
            build,
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

    fn build(&self, target: &str) -> Output {
        Command::new("cmake")
            .arg("--build")
            .arg(&self.build)
            .arg("--target")
            .arg(target)
            .output()
            .expect("run Ninja build")
    }
}

const fn selected_source() -> &'static str {
    "#include \"generated.h\"\n\
     #if OWNER_DEFINITION != 2\n#error missing owner definition\n#endif\n\
     #if SOURCE_FLAG != 3\n#error missing per-source flag\n#endif\n\
     #if LATE_OWNER_FLAG != 7 || LATE_INTERFACE_FLAG != 11\n#error missing final owner state\n#endif\n\
     int arch_value(void) { return GENERATED_VALUE + OWNER_DEFINITION + SOURCE_FLAG; }\n"
}

fn cmake_quote(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    format!("\"{}\"", normalized.replace('"', "\\\""))
}

const fn valid_targets() -> &'static str {
    "add_library(owner STATIC \"${CMAKE_CURRENT_SOURCE_DIR}/owner.c\" \"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\")\nadd_custom_target(include-owner)\n"
}

fn binding(endpoint: &str, owner: &str, include_owner: &str, sources: &str) -> String {
    format!(
        "aros_bind_arch_object_endpoint(ENDPOINT {endpoint} OWNER {owner} INCLUDE_OWNER {include_owner} SOURCES {sources})"
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
    assert!(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains(expected),
        "expected diagnostic {expected:?}, got:\n{text}"
    );
}

fn files_under(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("read fixture directory entry").path())
        .flat_map(|path| {
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_configured(fixture: &Fixture) {
    let output = fixture.configure();
    let text = output_text(&output);
    assert!(output.status.success(), "CMake configure failed:\n{text}");
}

fn compile_command(entry: &Value) -> String {
    if let Some(command) = entry.get("command").and_then(Value::as_str) {
        return command.to_owned();
    }
    entry
        .get("arguments")
        .and_then(Value::as_array)
        .expect("compile database entry has command or arguments")
        .iter()
        .map(|argument| argument.as_str().expect("compile argument is a string"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
// `${...}` below is CMake syntax, not a missing Rust format interpolation.
#[allow(clippy::literal_string_with_formatting_args)]
fn architecture_endpoint_compiles_once_with_owner_state_and_include_prerequisite() {
    let body = r##"
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(generated_dir "${CMAKE_CURRENT_BINARY_DIR}/generated")
set(generated_header "${generated_dir}/generated.h")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/emit_header.cmake" [==[
file(MAKE_DIRECTORY "${OUT_DIR}")
file(WRITE "${OUT_FILE}" "#define GENERATED_VALUE 39\n")
]==])
add_custom_command(
    OUTPUT "${generated_header}"
    COMMAND "${CMAKE_COMMAND}" "-DOUT_DIR=${generated_dir}" "-DOUT_FILE=${generated_header}"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/emit_header.cmake"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/emit_header.cmake"
    VERBATIM
)
add_custom_target(include-owner DEPENDS "${generated_header}")
add_executable(owner "${CMAKE_CURRENT_SOURCE_DIR}/main.c" "${CMAKE_CURRENT_SOURCE_DIR}/selected.c")
target_include_directories(owner PRIVATE "${generated_dir}")
target_compile_definitions(owner PRIVATE OWNER_DEFINITION=2)
target_compile_options(owner PRIVATE -Wall)
set_property(TARGET owner PROPERTY COMPILE_FLAGS "-DLEGACY_TARGET_FLAG=1")
set_source_files_properties("${CMAKE_CURRENT_SOURCE_DIR}/selected.c" PROPERTIES COMPILE_OPTIONS "-DSOURCE_FLAG=3")
aros_bind_arch_source_endpoint(
    ENDPOINT arch-owner
    OWNER owner
    INCLUDE_OWNER include-owner
    DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    BASENAMES selected
)
# Simulate phase-two dependencies and the post-graph default-link pass.
target_compile_options(owner PRIVATE -DLATE_OWNER_FLAG=7)
add_library(late-interface INTERFACE)
target_compile_definitions(late-interface INTERFACE LATE_INTERFACE_FLAG=11)
target_link_libraries(owner PRIVATE late-interface)
"##
    .to_owned();
    let fixture = Fixture::new(&body);
    assert_configured(&fixture);

    let build = Command::new("cmake")
        .arg("--build")
        .arg(&fixture.build)
        .arg("--target")
        .arg("owner")
        .arg("--verbose")
        .output()
        .expect("build configured owner");
    let text = output_text(&build);
    assert!(build.status.success(), "CMake build failed:\n{text}");

    let executable = fixture
        .build
        .join(if cfg!(windows) { "owner.exe" } else { "owner" });
    let run = Command::new(&executable)
        .output()
        .expect("run linked architecture endpoint fixture");
    assert!(run.status.success(), "fixture executable failed: {run:?}");

    let selected =
        fs::canonicalize(fixture.source.join("selected.c")).expect("canonicalize selected source");
    let compile_db: Vec<Value> = serde_json::from_slice(
        &fs::read(fixture.build.join("compile_commands.json")).expect("read compile database"),
    )
    .expect("parse compile database");
    let selected_entries: Vec<_> = compile_db
        .iter()
        .filter(|entry| {
            entry
                .get("file")
                .and_then(Value::as_str)
                .and_then(|file| fs::canonicalize(file).ok())
                .as_deref()
                == Some(selected.as_path())
        })
        .collect();
    assert_eq!(
        selected_entries.len(),
        1,
        "selected source must compile once"
    );

    let command = compile_command(selected_entries[0]);
    let generated_dir = fixture.build.join("generated");
    assert!(
        command.contains(&generated_dir.to_string_lossy().to_string()),
        "owner include directory did not reach endpoint command: {command}"
    );
    for state in [
        "OWNER_DEFINITION=2",
        "SOURCE_FLAG=3",
        "LEGACY_TARGET_FLAG=1",
        "LATE_OWNER_FLAG=7",
        "LATE_INTERFACE_FLAG=11",
        "-Wall",
    ] {
        assert!(command.contains(state), "missing {state} in {command}");
    }
}

#[test]
fn architecture_client_linklib_endpoint_binds_both_archive_variants() {
    let body = r#"
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
# This fixture tests source resolution and compiler-object dependencies only.
# No bootstrap generation or complete SDK qualification is claimed.
find_program(AROS_GENMODULE_BIN NAMES true PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/compiler/include/asm" "${AROS_SOURCE_DIR}/compiler/startup"
    "${AROS_SOURCE_DIR}/compiler/arossupport/include")
file(WRITE "${AROS_SOURCE_DIR}/compiler/include/asm/cpu.h" "/* Fixture bootstrap header; not SDK evidence. */\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/startup.c" "int fixture_startup(void) { return 0; }\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c" "int fixture_detach(void) { return 0; }\n")
include("${ENGINE}/AROS.cmake")
set(AROS_ARCH_INCLUDE_TAGS x86_64)
file(MAKE_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/arch" "${CMAKE_CURRENT_SOURCE_DIR}/generic" "${CMAKE_CURRENT_SOURCE_DIR}/module")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/arch/mathinit.c" "int mathinit_arch(void) { return 1; }\n")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c" "int mathinit_generic(void) { return 0; }\n")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generic/other.c" "int generic_other(void) { return 0; }\n")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generated.c" "int generated_stub(void) { return 0; }\n")
aros_set_arch_source_options("x86_64|arch|mathinit|-DARCH_CLIENT_LINKLIB=1")
aros_resolve_arch_linklib_sources(_resolved _dropped "${CMAKE_CURRENT_SOURCE_DIR}/module"
    SOURCES "${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c" "${CMAKE_CURRENT_SOURCE_DIR}/generic/other.c"
    ARCH_SOURCES "x86_64|arch|mathinit")
if(NOT _dropped STREQUAL "mathinit")
    message(FATAL_ERROR "generic basename was not filtered: ${_dropped}")
endif()
set(_expected_sources
    "${CMAKE_CURRENT_SOURCE_DIR}/generic/other.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/arch/mathinit.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/generated.c")
list(APPEND _resolved "${CMAKE_CURRENT_SOURCE_DIR}/generated.c")
if(NOT "${_resolved}" STREQUAL "${_expected_sources}")
    message(FATAL_ERROR "client archive source order differs from make.tmpl: ${_resolved}")
endif()
add_custom_target(include-owner)
add_library(owner-linklib STATIC ${_resolved})
add_library(owner-linklib-rel STATIC ${_resolved})
set_target_properties(owner-linklib owner-linklib-rel PROPERTIES
    ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/archives")
set_property(TARGET owner-linklib PROPERTY AROS_GENERIC_LINKLIB_SOURCES
    "${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c;${CMAKE_CURRENT_SOURCE_DIR}/generic/other.c")
set_property(TARGET owner-linklib-rel PROPERTY AROS_GENERIC_LINKLIB_SOURCES
    "${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c;${CMAKE_CURRENT_SOURCE_DIR}/generic/other.c")
aros_bind_arch_linklib_source_endpoint(
    ENDPOINT arch-owner-linklib
    INCLUDE_OWNER include-owner
    DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/arch"
    ARCHIVE_TARGETS owner-linklib owner-linklib-rel
    BASENAMES mathinit)
foreach(_archive IN ITEMS owner-linklib owner-linklib-rel)
    get_target_property(_object_owner "${_archive}" AROS_ARCH_CLIENT_OBJECT_OWNER)
    get_target_property(_sources "${_object_owner}" SOURCES)
    if(NOT "${_sources}" STREQUAL "${_expected_sources}")
        message(FATAL_ERROR "client source order differs on ${_archive}: ${_sources}")
    endif()
    if(NOT "${CMAKE_CURRENT_SOURCE_DIR}/arch/mathinit.c" IN_LIST _sources)
        message(FATAL_ERROR "selected source missing from ${_archive}")
    endif()
    if("${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c" IN_LIST _sources)
        message(FATAL_ERROR "generic source retained by ${_archive}")
    endif()
endforeach()
get_source_file_property(_arch_options "${CMAKE_CURRENT_SOURCE_DIR}/arch/mathinit.c" COMPILE_OPTIONS)
if(NOT "-DARCH_CLIENT_LINKLIB=1" IN_LIST _arch_options)
    message(FATAL_ERROR "per-source architecture linklib option was lost: ${_arch_options}")
endif()
get_target_property(_dependencies arch-owner-linklib MANUALLY_ADDED_DEPENDENCIES)
foreach(_required IN ITEMS include-owner)
    if(NOT _required IN_LIST _dependencies)
        message(FATAL_ERROR "missing endpoint dependency ${_required}")
    endif()
endforeach()
foreach(_forbidden IN ITEMS owner-linklib owner-linklib-rel)
    if(_forbidden IN_LIST _dependencies)
        message(FATAL_ERROR "endpoint must not depend on whole archive ${_forbidden}")
    endif()
endforeach()
add_dependencies(owner-linklib arch-owner-linklib)
add_dependencies(owner-linklib-rel arch-owner-linklib)
"#;
    let fixture = Fixture::new(body);
    let configured = fixture.configure();
    assert!(
        configured.status.success(),
        "CMake configure failed:\n{}",
        output_text(&configured)
    );

    let endpoint_build = fixture.build("arch-owner-linklib");
    assert!(
        endpoint_build.status.success(),
        "architecture endpoint build failed:\n{}",
        output_text(&endpoint_build)
    );
    assert!(!fixture.build.join("archives/libowner-linklib.a").exists());
    assert!(!fixture
        .build
        .join("archives/libowner-linklib-rel.a")
        .exists());
    let target_objects = files_under(&fixture.build.join("CMakeFiles"))
        .into_iter()
        .filter(|path| {
            let path_text = path.to_string_lossy();
            path_text.contains("aros-arch-client-objects-")
                && path.file_name().is_some_and(|name| name == "mathinit.c.o")
        })
        .collect::<Vec<_>>();
    assert_eq!(target_objects.len(), 2, "{target_objects:?}");
    let other_objects = files_under(&fixture.build.join("CMakeFiles"))
        .into_iter()
        .filter(|path| {
            let path_text = path.to_string_lossy();
            path_text.contains("aros-arch-client-objects-")
                && path.extension().is_some_and(|extension| extension == "o")
                && path.file_name().is_some_and(|name| name != "mathinit.c.o")
        })
        .collect::<Vec<_>>();
    assert!(
        other_objects.is_empty(),
        "unexpected archive objects: {other_objects:?}"
    );

    for target in ["owner-linklib", "owner-linklib-rel"] {
        let archive_build = fixture.build(target);
        assert!(
            archive_build.status.success(),
            "main archive build failed for {target}:\n{}",
            output_text(&archive_build)
        );
        assert!(fixture
            .build
            .join(format!("archives/lib{target}.a"))
            .exists());
        let members = Command::new("ar")
            .arg("t")
            .arg(fixture.build.join(format!("archives/lib{target}.a")))
            .output()
            .expect("inspect actual archive member order");
        assert!(members.status.success(), "{}", output_text(&members));
        assert_eq!(
            String::from_utf8_lossy(&members.stdout)
                .lines()
                // BSD ar displays GNU symbol/name tables and trailing name
                // separators. Neither is a compiler-object member.
                .filter(|name| !matches!(
                    *name,
                    "/" | "//" | "/SYM64/" | "__.SYMDEF" | "__.SYMDEF SORTED"
                ))
                .map(|name| name.trim_end_matches('/'))
                .collect::<Vec<_>>(),
            ["other.c.o", "mathinit.c.o", "generated.c.o"]
        );
    }
}

#[test]
fn architecture_client_linklib_endpoint_rejects_missing_or_unreplaced_sources() {
    let prefix = r#"
file(MAKE_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/arch" "${CMAKE_CURRENT_SOURCE_DIR}/generic")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/arch/mathinit.c" "int mathinit_arch(void) { return 1; }\n")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/generic/mathinit.c" "int mathinit_generic(void) { return 0; }\n")
add_custom_target(include-owner)
"#;
    let missing = format!(
        "{prefix}\nadd_library(owner-linklib STATIC \"${{CMAKE_CURRENT_SOURCE_DIR}}/owner.c\")\nset_property(TARGET owner-linklib PROPERTY AROS_GENERIC_LINKLIB_SOURCES \"${{CMAKE_CURRENT_SOURCE_DIR}}/generic/mathinit.c\")\naros_bind_arch_linklib_source_endpoint(ENDPOINT arch-owner-linklib INCLUDE_OWNER include-owner DIRECTORY \"${{CMAKE_CURRENT_SOURCE_DIR}}/arch\" ARCHIVE_TARGETS owner-linklib BASENAMES mathinit)"
    );
    assert_configure_rejected(&missing, "has no unique selected source");

    let generic_retained = format!(
        "{prefix}\nadd_library(owner-linklib STATIC \"${{CMAKE_CURRENT_SOURCE_DIR}}/arch/mathinit.c\" \"${{CMAKE_CURRENT_SOURCE_DIR}}/generic/mathinit.c\")\nset_property(TARGET owner-linklib PROPERTY AROS_GENERIC_LINKLIB_SOURCES \"${{CMAKE_CURRENT_SOURCE_DIR}}/generic/mathinit.c\")\naros_bind_arch_linklib_source_endpoint(ENDPOINT arch-owner-linklib INCLUDE_OWNER include-owner DIRECTORY \"${{CMAKE_CURRENT_SOURCE_DIR}}/arch\" ARCHIVE_TARGETS owner-linklib BASENAMES mathinit)"
    );
    assert_configure_rejected(&generic_retained, "generic mathinit was not replaced");
}

#[test]
fn architecture_source_endpoint_preserves_nested_source_identity() {
    let body = r#"
file(MAKE_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/arch/math")
file(WRITE "${CMAKE_CURRENT_SOURCE_DIR}/arch/math/value.c" "int arch_value(void) { return 1; }\n")
add_library(owner STATIC "${CMAKE_CURRENT_SOURCE_DIR}/arch/math/value.c")
add_custom_target(include-owner)
aros_bind_arch_source_endpoint(ENDPOINT nested-arch OWNER owner INCLUDE_OWNER include-owner
    DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/arch" BASENAMES math/value)
"#;
    let fixture = Fixture::new(body);
    let configured = fixture.configure();
    assert!(configured.status.success(), "{}", output_text(&configured));
    let built = fixture.build("nested-arch");
    assert!(built.status.success(), "{}", output_text(&built));
    assert!(files_under(&fixture.build).iter().any(|path| path
        .to_string_lossy()
        .contains("nested-arch.dir/arch/math/value.c.o")));
    for name in ["math/../value", "math//value", "/math/value", "math/."] {
        assert_configure_rejected(
            &body.replace("BASENAMES math/value", &format!("BASENAMES {name}")),
            "Unsafe architecture source basename",
        );
    }
}

#[test]
fn architecture_source_endpoint_refuses_missing_shadowed_and_ambiguous_sources() {
    for basename in ["missing", "../selected", "selected;selected"] {
        let body = format!("{}\naros_bind_arch_source_endpoint(ENDPOINT arch OWNER owner INCLUDE_OWNER include-owner DIRECTORY \"${{CMAKE_CURRENT_SOURCE_DIR}}\" BASENAMES \"{basename}\")", valid_targets());
        let fixture = Fixture::new(&body);
        assert!(
            !fixture.configure().status.success(),
            "unsafe source request accepted: {basename}"
        );
    }
    let body = format!("{}\nfile(WRITE \"${{CMAKE_CURRENT_SOURCE_DIR}}/selected.cpp\" \"int alternative;\\n\")\ntarget_sources(owner PRIVATE \"${{CMAKE_CURRENT_SOURCE_DIR}}/selected.cpp\")\naros_bind_arch_source_endpoint(ENDPOINT arch OWNER owner INCLUDE_OWNER include-owner DIRECTORY \"${{CMAKE_CURRENT_SOURCE_DIR}}\" BASENAMES selected)", valid_targets());
    assert_configure_rejected(&body, "no unique selected source");
}

#[test]
fn architecture_endpoint_rejects_missing_and_utility_owners() {
    assert_configure_rejected(
        &format!(
            "{}\n{}",
            valid_targets(),
            binding(
                "endpoint",
                "missing-owner",
                "include-owner",
                "\"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\""
            )
        ),
        "conflicting or missing owner",
    );

    let utility_owner = format!(
        "add_custom_target(owner)\nadd_custom_target(include-owner)\n{}",
        binding(
            "endpoint",
            "owner",
            "include-owner",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\""
        )
    );
    assert_configure_rejected(&utility_owner, "owner is not a compilation target");
}

#[test]
fn architecture_endpoint_rejects_missing_include_owner() {
    let body = format!(
        "add_library(owner STATIC \"${{CMAKE_CURRENT_SOURCE_DIR}}/owner.c\" \"${{CMAKE_CURRENT_SOURCE_DIR}}/selected.c\")\n{}",
        binding(
            "endpoint",
            "owner",
            "missing-include-owner",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\""
        )
    );
    assert_configure_rejected(&body, "conflicting or missing owner");
}

#[test]
fn architecture_endpoint_rejects_duplicate_endpoint_name() {
    let body = format!(
        "{}\nadd_custom_target(endpoint)\n{}",
        valid_targets(),
        binding(
            "endpoint",
            "owner",
            "include-owner",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\""
        )
    );
    assert_configure_rejected(&body, "conflicting or missing owner");
}

#[test]
fn architecture_endpoint_rejects_invalid_source_bindings() {
    let cases = [
        (
            "absent source",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/missing.c\"",
            "unbound or duplicate source",
        ),
        (
            "non-owner source",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/other.c\"",
            "unbound or duplicate source",
        ),
        (
            "duplicate source",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\" \"${CMAKE_CURRENT_SOURCE_DIR}/selected.c\"",
            "unbound or duplicate source",
        ),
        (
            "empty source list",
            "",
            "Incomplete architecture object binding",
        ),
    ];
    for (name, sources, expected) in cases {
        let body = format!(
            "{}\n{}",
            valid_targets(),
            binding("endpoint", "owner", "include-owner", sources)
        );
        let fixture = Fixture::new(&body);
        let output = fixture.configure();
        let text = output_text(&output);
        assert!(
            !output.status.success(),
            "{name} unexpectedly configured:\n{text}"
        );
        assert!(
            text.contains(expected),
            "{name}: expected {expected:?}, got:\n{text}"
        );
    }

    let external = format!(
        "set_source_files_properties(\"${{CMAKE_CURRENT_SOURCE_DIR}}/external.o\" PROPERTIES EXTERNAL_OBJECT TRUE)\nadd_library(owner STATIC \"${{CMAKE_CURRENT_SOURCE_DIR}}/owner.c\" \"${{CMAKE_CURRENT_SOURCE_DIR}}/external.o\")\nadd_custom_target(include-owner)\n{}",
        binding(
            "endpoint",
            "owner",
            "include-owner",
            "\"${CMAKE_CURRENT_SOURCE_DIR}/external.o\""
        )
    );
    assert_configure_rejected(&external, "external object is not a source effect");
}
