//! Compile the projected Mesa option vectors through the embedded CMake rule.
//!
//! This fixture compiles three tiny object files. It checks ordered argv and
//! object format only; it does not build Mesa or qualify an SDK.

use aros_common::{
    elf::{self, Class},
    run_output_with_control, sha256_bytes, CancellationToken,
};
use aros_transpiler::capability::mesa::ordered_contract::OrderedMesaCompilerOptions;
use serde_json::{json, Value};
use std::{
    error::Error,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const CAPTURE_LIMIT: usize = 256 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Compile and verify one source-projected option case into a fresh output root.
///
/// `compiler` is a compiler installation root containing `bin/clang` and
/// `bin/clang++`; all other executable paths and directories are explicit.
/// The retained output contains generated CMake files, bounded process logs,
/// and hashes plus ELF identity for the three resulting objects.
pub fn verify_projection(
    root: &Path,
    host: &Path,
    cmake: &Path,
    ninja: &Path,
    compiler: &Path,
    triple: &str,
    options: &OrderedMesaCompilerOptions,
) -> Result<(), Box<dyn Error>> {
    let root = canonical_directory(root, "output root")?;
    if fs::read_dir(&root)?.next().transpose()?.is_some() {
        return Err(invalid("output root must be fresh and empty").into());
    }
    safe_path_text(&root, "output root")?;
    let host = canonical_directory(host, "host tool closure")?;
    let host_path = std::env::join_paths([host.as_os_str()])?;
    let cmake = canonical_file(cmake, "CMake executable")?;
    let ninja = canonical_file(ninja, "Ninja executable")?;
    let compiler = canonical_directory(compiler, "compiler root")?;
    // Validate their resolved files without replacing the selected driver role:
    // clang++ may be a symlink whose argv[0] still selects C++ driver behavior.
    let clang = compiler.join("bin/clang");
    let clangxx = compiler.join("bin/clang++");
    canonical_file(&clang, "compiler clang")?;
    canonical_file(&clangxx, "compiler clang++")?;
    safe_path_text(&cmake, "CMake executable")?;
    let ninja_text = safe_path_text(&ninja, "Ninja executable")?;
    let clang_text = safe_path_text(&clang, "compiler clang")?;
    let clangxx_text = safe_path_text(&clangxx, "compiler clang++")?;
    safe_path_text(&host, "host tool closure")?;
    if !matches!(triple, "arm-unknown-aros" | "aarch64-unknown-aros") {
        return Err(
            invalid("projection supports only the closed ARM and AArch64 test triples").into(),
        );
    }
    validate_triple(triple)?;
    validate_options(&options.c, "C")?;
    validate_options(&options.cxx, "CXX")?;
    validate_options(&options.vc4_bridge, "VC4 bridge")?;

    let source = root.join("project");
    let build = root.join("build");
    fs::create_dir(&source)?;
    fs::create_dir(&build)?;

    let engine = aros_cmake_engine::file("OrderedCompileContract.cmake")
        .ok_or_else(|| invalid("embedded OrderedCompileContract.cmake is missing"))?;
    fs::write(source.join("OrderedCompileContract.cmake"), engine)?;
    fs::write(source.join("CMakeLists.txt"), cmake_project(options))?;
    fs::write(
        source.join("verify_compile_commands.cmake"),
        compile_commands_script(options),
    )?;
    fs::write(source.join("c_probe.c"), probe_source(&options.c)?)?;
    fs::write(source.join("cxx_probe.cpp"), probe_source(&options.cxx)?)?;
    fs::write(
        source.join("bridge_probe.c"),
        probe_source(&options.vc4_bridge)?,
    )?;

    let build_text = safe_path_text(&build, "build directory")?;
    let source_text = safe_path_text(&source, "project directory")?;
    let configure_args = vec![
        "-S".to_owned(),
        source_text,
        "-B".to_owned(),
        build_text.clone(),
        "-G".to_owned(),
        "Ninja".to_owned(),
        format!("-DCMAKE_MAKE_PROGRAM:FILEPATH={ninja_text}"),
        format!("-DCMAKE_C_COMPILER:FILEPATH={clang_text}"),
        format!("-DCMAKE_CXX_COMPILER:FILEPATH={clangxx_text}"),
        format!("-DCMAKE_C_COMPILER_TARGET:STRING={triple}"),
        format!("-DCMAKE_CXX_COMPILER_TARGET:STRING={triple}"),
        "-DCMAKE_SYSTEM_NAME:STRING=Generic".to_owned(),
        "-DCMAKE_TRY_COMPILE_TARGET_TYPE:STRING=STATIC_LIBRARY".to_owned(),
        "-DCMAKE_C_COMPILER_WORKS:BOOL=TRUE".to_owned(),
        "-DCMAKE_CXX_COMPILER_WORKS:BOOL=TRUE".to_owned(),
        "-DCMAKE_EXPORT_COMPILE_COMMANDS:BOOL=ON".to_owned(),
    ];
    run_logged(
        &cmake,
        &configure_args,
        &host_path,
        &root,
        &root,
        "cmake-configure",
    )?;

    let verify_args = vec![
        "-P".to_owned(),
        source
            .join("verify_compile_commands.cmake")
            .to_str()
            .ok_or_else(|| invalid("verification script path is not UTF-8"))?
            .to_owned(),
    ];
    run_logged(
        &cmake,
        &verify_args,
        &host_path,
        &root,
        &root,
        "cmake-verify-argv",
    )?;

    let build_args = vec![
        "--build".to_owned(),
        build_text,
        "--verbose".to_owned(),
        "--parallel".to_owned(),
        "1".to_owned(),
        "--target".to_owned(),
        "mesa_c".to_owned(),
        "mesa_cxx".to_owned(),
        "mesa_bridge".to_owned(),
    ];
    run_logged(&cmake, &build_args, &host_path, &root, &root, "cmake-build")?;

    let object_records = verify_objects(&build, triple)?;
    let manifest = json!({
        "scope": "three tiny Mesa option-projection object targets; no Mesa or SDK build",
        "target_triple": triple,
        "objects": object_records,
    });
    fs::write(
        root.join("objects.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid(format!("{label} must be absolute")));
    }
    let path = fs::canonicalize(path)?;
    if !path.is_dir() {
        return Err(invalid(format!("{label} must be a directory")));
    }
    Ok(path)
}

fn canonical_file(path: &Path, label: &str) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid(format!("{label} must be absolute")));
    }
    let path = fs::canonicalize(path)?;
    if !path.is_file() {
        return Err(invalid(format!("{label} must be a regular file")));
    }
    Ok(path)
}

fn safe_path_text(path: &Path, label: &str) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid(format!("{label} must be valid UTF-8")))?;
    validate_cmake_literal(text, label)?;
    Ok(text.to_owned())
}

fn validate_triple(triple: &str) -> io::Result<()> {
    if triple.is_empty()
        || !triple
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
    {
        return Err(invalid("target triple contains unsafe characters"));
    }
    Ok(())
}

fn validate_options(options: &[String], label: &str) -> io::Result<()> {
    if options.is_empty() {
        return Err(invalid(format!("{label} option vector must be nonempty")));
    }
    for option in options {
        validate_cmake_literal(option, label)?;
        if option.starts_with('@') {
            return Err(invalid(format!(
                "{label} option vector contains a response file"
            )));
        }
    }
    Ok(())
}

fn validate_cmake_literal(value: &str, label: &str) -> io::Result<()> {
    if value.is_empty()
        || value.contains("]=]")
        || value.contains("]==]")
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n' | ';' | '$' | '<' | '>'))
    {
        return Err(invalid(format!(
            "{label} contains unsafe CMake literal data"
        )));
    }
    Ok(())
}

fn cmake_project(options: &OrderedMesaCompilerOptions) -> String {
    let mut project = String::from(
        "cmake_minimum_required(VERSION 3.22)\n\
         project(ArosMesaCompileProjection LANGUAGES C CXX)\n\
         include(\"${CMAKE_CURRENT_LIST_DIR}/OrderedCompileContract.cmake\")\n\n",
    );
    append_target(&mut project, "mesa_c", "C", "c_probe.c", &options.c);
    append_target(
        &mut project,
        "mesa_cxx",
        "CXX",
        "cxx_probe.cpp",
        &options.cxx,
    );
    append_target(
        &mut project,
        "mesa_bridge",
        "C",
        "bridge_probe.c",
        &options.vc4_bridge,
    );
    project
}

fn append_target(
    project: &mut String,
    target: &str,
    language: &str,
    source: &str,
    options: &[String],
) {
    writeln!(project, "add_library({target} OBJECT {source})").unwrap();
    write!(
        project,
        "_aros_target_ordered_compile_contract({target} {language}"
    )
    .unwrap();
    for option in options {
        write!(project, "\n    [==[{option}]==]").unwrap();
    }
    project.push_str("\n)\n\n");
}

fn compile_commands_script(options: &OrderedMesaCompilerOptions) -> String {
    let mut script = String::from(
        "cmake_minimum_required(VERSION 3.22)\n\
         set(_compile_commands_file \"${CMAKE_CURRENT_LIST_DIR}/../build/compile_commands.json\")\n\
         file(READ \"${_compile_commands_file}\" _compile_commands)\n\
         string(JSON _command_count LENGTH \"${_compile_commands}\")\n\
         if(NOT _command_count EQUAL 3)\n\
             message(FATAL_ERROR \"expected exactly three compile commands, got ${_command_count}\")\n\
         endif()\n\n",
    );
    append_expected(&mut script, "_expected_c", &options.c);
    append_expected(&mut script, "_expected_cxx", &options.cxx);
    append_expected(&mut script, "_expected_bridge", &options.vc4_bridge);
    script.push_str(
        r#"
set(_seen_sources "")
foreach(_index RANGE 0 2)
    string(JSON _source_file GET "${_compile_commands}" ${_index} file)
    string(JSON _command GET "${_compile_commands}" ${_index} command)
    get_filename_component(_source_name "${_source_file}" NAME)
    if(_source_name STREQUAL "c_probe.c")
        set(_target "mesa_c")
        set(_expected ${_expected_c})
        set(_expected_object "CMakeFiles/mesa_c.dir/c_probe.c")
    elseif(_source_name STREQUAL "cxx_probe.cpp")
        set(_target "mesa_cxx")
        set(_expected ${_expected_cxx})
        set(_expected_object "CMakeFiles/mesa_cxx.dir/cxx_probe.cpp")
    elseif(_source_name STREQUAL "bridge_probe.c")
        set(_target "mesa_bridge")
        set(_expected ${_expected_bridge})
        set(_expected_object "CMakeFiles/mesa_bridge.dir/bridge_probe.c")
    else()
        message(FATAL_ERROR "unexpected compile source ${_source_file}")
    endif()
    if(_source_file STREQUAL "${CMAKE_CURRENT_LIST_DIR}/${_source_name}")
        # The source path is the expected generated fixture file.
    else()
        message(FATAL_ERROR "compile command used unexpected source ${_source_file}")
    endif()
    if(_source_name IN_LIST _seen_sources)
        message(FATAL_ERROR "duplicate compile command for ${_source_name}")
    endif()
    list(APPEND _seen_sources "${_source_name}")

    separate_arguments(_argv UNIX_COMMAND "${_command}")
    list(LENGTH _argv _argv_count)
    list(LENGTH _expected _expected_count)
    if(_expected_count LESS 1)
        message(FATAL_ERROR "${_target} has no expected ordered options")
    endif()
    math(EXPR _last_start "${_argv_count} - ${_expected_count}")
    if(_last_start LESS 0)
        message(FATAL_ERROR "${_target} compile argv is shorter than its option vector")
    endif()
    math(EXPR _expected_last "${_expected_count} - 1")
    set(_sequence_matches 0)
    foreach(_start RANGE 0 ${_last_start})
        set(_matches TRUE)
        foreach(_offset RANGE 0 ${_expected_last})
            math(EXPR _argument_index "${_start} + ${_offset}")
            list(GET _argv ${_argument_index} _argument)
            list(GET _expected ${_offset} _expected_argument)
            if(NOT _argument STREQUAL _expected_argument)
                set(_matches FALSE)
                break()
            endif()
        endforeach()
        if(_matches)
            math(EXPR _sequence_matches "${_sequence_matches} + 1")
        endif()
    endforeach()
    if(NOT _sequence_matches EQUAL 1)
        message(FATAL_ERROR "${_target} must contain its complete ordered option vector once; found ${_sequence_matches}")
    endif()

    set(_owns_object FALSE)
    math(EXPR _last_argument "${_argv_count} - 1")
    foreach(_argument_index RANGE 0 ${_last_argument})
        list(GET _argv ${_argument_index} _argument)
        if(_argument STREQUAL "-o")
            math(EXPR _output_index "${_argument_index} + 1")
            if(_output_index LESS _argv_count)
                list(GET _argv ${_output_index} _output_path)
                if(_output_path STREQUAL "${_expected_object}.o" OR
                   _output_path STREQUAL "${_expected_object}.obj")
                    set(_owns_object TRUE)
                endif()
            endif()
        endif()
    endforeach()
    if(NOT _owns_object)
        message(FATAL_ERROR "${_target} compile command is not its expected object target")
    endif()
endforeach()
list(LENGTH _seen_sources _seen_count)
if(NOT _seen_count EQUAL 3)
    message(FATAL_ERROR "expected one command for each of three targets")
endif()
message(STATUS "All three compile commands preserve their complete ordered vectors")
"#,
    );
    script
}

fn append_expected(script: &mut String, variable: &str, options: &[String]) {
    writeln!(script, "set({variable}").unwrap();
    for option in options {
        writeln!(script, "    [==[{option}]==]").unwrap();
    }
    script.push_str(")\n\n");
}

fn probe_source(options: &[String]) -> io::Result<String> {
    let mut source = String::new();
    for name in counter_macros(options)? {
        writeln!(source, "#ifndef {name}").unwrap();
        writeln!(source, "#error ordered counter definition was lost").unwrap();
        source.push_str("#endif\n");
        writeln!(source, "#if {name} != 1").unwrap();
        writeln!(
            source,
            "#error ordered counter definition has the wrong value"
        )
        .unwrap();
        source.push_str("#endif\n");
    }
    source.push_str("int aros_mesa_projection_probe(void) { return 0; }\n");
    Ok(source)
}

fn counter_macros(options: &[String]) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for triple in options.windows(3) {
        let Some(definition) = triple[0].strip_prefix("-D") else {
            continue;
        };
        let Some((name, value)) = definition.split_once('=') else {
            continue;
        };
        if value != "1" || triple[1] != format!("-U{name}") || triple[2] != triple[0] {
            continue;
        }
        if name.is_empty()
            || !name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
        {
            return Err(invalid(
                "duplicate counter option has an invalid macro name",
            ));
        }
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn run_logged(
    program: &Path,
    args: &[String],
    host_path: &std::ffi::OsStr,
    current_dir: &Path,
    output_dir: &Path,
    label: &str,
) -> io::Result<()> {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", host_path)
        .env("LC_ALL", "C")
        .current_dir(current_dir)
        .args(args);
    let output = run_output_with_control(
        &mut command,
        CAPTURE_LIMIT,
        COMMAND_TIMEOUT,
        &CancellationToken::default(),
    )?;
    write_capture(output_dir, label, "stdout", &output.stdout)?;
    write_capture(output_dir, label, "stderr", &output.stderr)?;
    if !output.status.success() || output.timed_out || output.cancelled {
        return Err(io::Error::other(format!(
            "{label} failed (status {:?}, timeout {}, cancelled {}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            output.timed_out,
            output.cancelled,
            output.stdout.rendered_lossy(),
            output.stderr.rendered_lossy()
        )));
    }
    Ok(())
}

fn write_capture(
    output_dir: &Path,
    label: &str,
    stream_name: &str,
    stream: &aros_common::CapturedStream,
) -> io::Result<()> {
    let mut file = fs::File::create(output_dir.join(format!("{label}.{stream_name}")))?;
    stream.write_rendered(&mut file)
}

fn verify_objects(build: &Path, triple: &str) -> Result<Vec<Value>, Box<dyn Error>> {
    let (expected_class, expected_machine) = match triple {
        "arm-unknown-aros" => (Class::Elf32, 40_u16),
        "aarch64-unknown-aros" => (Class::Elf64, 183_u16),
        _ => return Err(invalid("unsupported ELF proof triple").into()),
    };
    let targets = [
        ("mesa_c", "c_probe.c"),
        ("mesa_cxx", "cxx_probe.cpp"),
        ("mesa_bridge", "bridge_probe.c"),
    ];
    let mut records = Vec::with_capacity(targets.len());
    for (target, source) in targets {
        let target_dir = build.join("CMakeFiles").join(format!("{target}.dir"));
        let object_files = fs::read_dir(&target_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|path| {
                matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("o" | "obj")
                )
            })
            .collect::<Vec<_>>();
        if object_files.len() != 1
            || ![format!("{source}.o"), format!("{source}.obj")]
                .iter()
                .any(|name| object_files[0].file_name() == Some(std::ffi::OsStr::new(name)))
        {
            return Err(invalid(format!(
                "{target} must produce exactly its one expected object"
            ))
            .into());
        }
        let object_path = &object_files[0];
        let metadata = fs::symlink_metadata(object_path)?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(invalid(format!("{target} object must be a regular nonempty file")).into());
        }
        let bytes = fs::read(object_path)?;
        let object = elf::read(&bytes)?;
        if object.kind != 1 {
            return Err(invalid(format!("{target} output is not an ELF ET_REL object")).into());
        }
        if object.class != expected_class || object.machine != expected_machine {
            return Err(invalid(format!("{target} ELF identity does not match {triple}")).into());
        }
        let class = match object.class {
            Class::Elf32 => "ELF32",
            Class::Elf64 => "ELF64",
        };
        let relative_path = object_path.strip_prefix(build)?.to_string_lossy();
        records.push(json!({
            "target": target,
            "source": source,
            "object": relative_path,
            "size": metadata.len(),
            "sha256": sha256_bytes(&bytes).as_str(),
            "elf": {
                "class": class,
                "kind": object.kind,
                "machine": object.machine,
            },
        }));
    }
    if records.len() != 3 {
        return Err(invalid("expected exactly three target object records").into());
    }
    Ok(records)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
