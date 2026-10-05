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
            "cmake_minimum_required(VERSION 3.21)\nproject(ArchitectureEndpointFixture C)\ninclude({})\n{}\n",
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
        text.contains(expected),
        "expected diagnostic {expected:?}, got:\n{text}"
    );
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
