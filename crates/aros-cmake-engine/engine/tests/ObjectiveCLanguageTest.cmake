cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-objective-c-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/objective-c-language")

find_program(_clang NAMES clang
    HINTS "$ENV{HOME}/.aros/toolchain/bin"
          "/opt/homebrew/opt/llvm/bin"
          "/usr/local/opt/llvm/bin"
    REQUIRED)

set(_profiles
    "x86_64|x86_64-unknown-elf|3e00"
    "arm|arm-none-eabi|2800"
    "aarch64|aarch64-unknown-elf|b700")

foreach(_profile IN LISTS _profiles)
    string(REPLACE "|" ";" _fields "${_profile}")
    list(GET _fields 0 _processor)
    list(GET _fields 1 _triple)
    list(GET _fields 2 _machine)
    set(_build "${_root}/${_processor}")

    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DCMAKE_SYSTEM_NAME=Generic"
            "-DCMAKE_SYSTEM_PROCESSOR=${_processor}"
            "-DCMAKE_C_COMPILER=${_clang}"
            "-DOBJC_TEST_TRIPLE=${_triple}"
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)
    if(NOT _configure_result EQUAL 0)
        message(FATAL_ERROR
            "Objective-C ${_processor} configure failed (${_configure_result})\n"
            "${_configure_stdout}\n${_configure_stderr}")
    endif()

    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target objc-probe
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR
            "Objective-C ${_processor} build failed (${_build_result})\n"
            "${_build_stdout}\n${_build_stderr}")
    endif()

    file(GLOB_RECURSE _objects "${_build}/CMakeFiles/objc-probe.dir/*.o")
    list(LENGTH _objects _object_count)
    if(NOT _object_count EQUAL 1)
        message(FATAL_ERROR
            "Objective-C ${_processor} produced ${_object_count} objects: ${_objects}")
    endif()
    list(GET _objects 0 _object)
    file(READ "${_object}" _elf_header OFFSET 0 LIMIT 20 HEX)
    string(SUBSTRING "${_elf_header}" 0 8 _elf_magic)
    string(SUBSTRING "${_elf_header}" 36 4 _elf_machine)
    if(NOT _elf_magic STREQUAL "7f454c46" OR
       NOT _elf_machine STREQUAL "${_machine}")
        message(FATAL_ERROR
            "Objective-C ${_processor} object has ELF magic/machine "
            "${_elf_magic}/${_elf_machine}, expected 7f454c46/${_machine}")
    endif()
endforeach()

# A pre-populated cache must never be able to redirect Objective-C compilation
# to a host compiler while C continues to use the validated target compiler.
set(_split_build "${_root}/split-compiler")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_split_build}" -G Ninja
        "-DCMAKE_SYSTEM_NAME=Generic"
        "-DCMAKE_SYSTEM_PROCESSOR=x86_64"
        "-DCMAKE_C_COMPILER=${_clang}"
        "-DCMAKE_OBJC_COMPILER=${CMAKE_COMMAND}"
        "-DOBJC_TEST_TRIPLE=x86_64-unknown-elf"
    RESULT_VARIABLE _split_result
    OUTPUT_VARIABLE _split_stdout
    ERROR_VARIABLE _split_stderr)
if(_split_result EQUAL 0)
    message(FATAL_ERROR
        "Objective-C accepted a compiler different from C\n${_split_stdout}\n${_split_stderr}")
endif()
if(NOT "${_split_stdout}\n${_split_stderr}" MATCHES
       "Objective-C compiler must match the validated C compiler")
    message(FATAL_ERROR
        "Objective-C rejected the split compiler for the wrong reason\n"
        "${_split_stdout}\n${_split_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "LLVM Objective-C cross-language test passed")
