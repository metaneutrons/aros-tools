cmake_minimum_required(VERSION 3.22)

find_program(_cmake NAMES cmake REQUIRED)
find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_sed NAMES gsed sed PATHS /opt/homebrew/bin /usr/local/bin /usr/bin /bin
    NO_DEFAULT_PATH REQUIRED)
execute_process(COMMAND "${_sed}" --version RESULT_VARIABLE _sed_version_result
    OUTPUT_VARIABLE _sed_version ERROR_VARIABLE _sed_version_error)
if(NOT _sed_version_result STREQUAL "0" OR NOT "${_sed_version}${_sed_version_error}" MATCHES "GNU sed")
    message(FATAL_ERROR "SourceValueRuleTest requires GNU sed for independent reference output")
endif()

get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
file(REAL_PATH "${_temp_base}" _temp_base)
string(RANDOM LENGTH 14 ALPHABET 0123456789abcdef _nonce)
set(_root "${_temp_base}/aros-source-value-rule-${_nonce} with spaces")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "source-value test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")
file(REAL_PATH "${_root}" _root)
set(_source_root "${_root}/source tree")
set(_fixture "${_root}/fixture")
file(MAKE_DIRECTORY "${_source_root}/inputs" "${_source_root}/rules" "${_fixture}")
file(WRITE "${_source_root}/rules/declaring-mmakefile.src" "# source-value test declaration\n")
set(_declaring_file "${_source_root}/rules/declaring-mmakefile.src")
file(SHA256 "${_declaring_file}" _declaring_sha)
string(HEX "MARK" _marker_hex)

file(WRITE "${_fixture}/CMakeLists.txt" [=[
cmake_minimum_required(VERSION 3.22)
project(source_value_rule_fixture NONE)
if(NOT DEFINED ENGINE_DIR OR NOT DEFINED SOURCE_ROOT OR NOT DEFINED INPUT_PATH OR
   NOT DEFINED OUTPUT_REL OR NOT DEFINED FILE_REL OR NOT DEFINED FILE_SHA256 OR
   NOT DEFINED MARKER_HEX OR NOT DEFINED RULE_NAME)
    message(FATAL_ERROR "source-value fixture arguments are incomplete")
endif()
set(AROS_SOURCE_DIR "${SOURCE_ROOT}")
include("${ENGINE_DIR}/SourceValueRules.cmake")
set(_output "${CMAKE_BINARY_DIR}/SYS/Prefs/${OUTPUT_REL}")
if(DEFINED OUTPUT_ABSOLUTE AND NOT "${OUTPUT_ABSOLUTE}" STREQUAL "")
    set(_output "${OUTPUT_ABSOLUTE}")
endif()
if(CASE_MODE STREQUAL "duplicate")
    aros_extract_source_value(NAME source-value-first
        INPUT "${INPUT_PATH}" OUTPUT "${_output}" MARKER_HEX "${MARKER_HEX}"
        FILE "${FILE_REL}" FILE_SHA256 "${FILE_SHA256}")
    aros_extract_source_value(NAME source-value-second
        INPUT "${INPUT_PATH}" OUTPUT "${_output}" MARKER_HEX "${MARKER_HEX}"
        FILE "${FILE_REL}" FILE_SHA256 "${FILE_SHA256}")
else()
    aros_extract_source_value(NAME "${RULE_NAME}"
        INPUT "${INPUT_PATH}" OUTPUT "${_output}" MARKER_HEX "${MARKER_HEX}"
        FILE "${FILE_REL}" FILE_SHA256 "${FILE_SHA256}")
endif()
]=])

set(_default_input "${_source_root}/inputs/value.conf")
set(_output_rel "extracted/value.txt")
set(_output "${_root}/main-build/SYS/Prefs/${_output_rel}")

function(_configure_case build case_mode success expected_message)
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture}" -B "${build}" -G Ninja
            "-DENGINE_DIR=${_engine_dir}"
            "-DSOURCE_ROOT=${_source_root}"
            "-DINPUT_PATH=${_default_input}"
            "-DOUTPUT_REL=${_output_rel}"
            "-DFILE_REL=rules/declaring-mmakefile.src"
            "-DFILE_SHA256=${_declaring_sha}"
            "-DMARKER_HEX=${_marker_hex}"
            -DRULE_NAME=source-value-product
            "-DCASE_MODE=${case_mode}"
            ${ARGN}
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(success)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${case_mode}: configure failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0" OR NOT _log MATCHES "${expected_message}")
            message(FATAL_ERROR
                "${case_mode}: expected configure rejection /${expected_message}/ (${_result})\n${_log}")
        endif()
    endif()
    set(_CONFIGURE_BUILD "${build}" PARENT_SCOPE)
endfunction()

function(_build_target build success expected_message out_log)
    execute_process(
        COMMAND "${_cmake}" --build "${build}" --target source-value-product --parallel 2
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(success)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "source-value build failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0" OR NOT _log MATCHES "${expected_message}")
            message(FATAL_ERROR "source-value build failure was not detected (${_result})\n${_log}")
        endif()
    endif()
    set(${out_log} "${_log}" PARENT_SCOPE)
endfunction()

function(_gnu_sed_reference input output)
    set(_first_stage "${_root}/reference-stage-${_nonce}-${ARGV2}.txt")
    execute_process(
        COMMAND "${_sed}" -n "s/MARK// p"
        INPUT_FILE "${input}" OUTPUT_FILE "${_first_stage}"
        RESULT_VARIABLE _first_result ERROR_VARIABLE _first_error TIMEOUT 20)
    if(NOT _first_result STREQUAL "0")
        message(FATAL_ERROR "GNU sed first-stage reference failed: ${_first_error}")
    endif()
    execute_process(
        COMMAND "${_sed}" -n "s/^ *//;s/[ */*].*//p"
        INPUT_FILE "${_first_stage}" OUTPUT_FILE "${output}"
        RESULT_VARIABLE _second_result ERROR_VARIABLE _second_error TIMEOUT 20)
    if(NOT _second_result STREQUAL "0")
        message(FATAL_ERROR "GNU sed second-stage reference failed: ${_second_error}")
    endif()
endfunction()

function(_compare_to_reference label input output)
    set(_expected "${_root}/expected-${_nonce}-${label}.txt")
    _gnu_sed_reference("${input}" "${_expected}" "${label}")
    execute_process(COMMAND "${_cmake}" -E compare_files "${output}" "${_expected}"
        RESULT_VARIABLE _compare_result)
    if(NOT _compare_result STREQUAL "0")
        file(READ "${_output}" _actual)
        file(READ "${_expected}" _reference)
        message(FATAL_ERROR "${label}: extraction differs from GNU sed\nactual=[${_actual}]\nexpected=[${_reference}]")
    endif()
endfunction()

function(_expect_preserved_output label path expected)
    if(NOT EXISTS "${path}" OR IS_SYMLINK "${path}" OR IS_DIRECTORY "${path}")
        message(FATAL_ERROR "${label}: previous output was removed or replaced")
    endif()
    file(READ "${path}" _actual)
    if(NOT _actual STREQUAL expected)
        message(FATAL_ERROR "${label}: previous output changed: [${_actual}]")
    endif()
endfunction()

# The first and second stages operate on separate GNU sed processes, preserving
# the host implementation as an independent oracle for line-ending behavior.
set(_main_build "${_root}/main-build")
file(WRITE "${_default_input}"
    "unselected line\n  cpuMARK= r1 / remainder\nleftMARKMARK prefix*tail\nMARKfirst/last")
_configure_case("${_main_build}" positive TRUE "")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("multiple-lines-no-final-newline" "${_default_input}" "${_output}")

set(_crlf "miss\r\n  cpuMARK= esp32p4 /ignored\r\nMARKsecond*ignored\r\n")
file(WRITE "${_default_input}" "${_crlf}")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("crlf" "${_default_input}" "${_output}")

file(WRITE "${_default_input}" "  MARKcpu= riscv64 /tail\nlastMARKname*tail")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("no-final-newline" "${_default_input}" "${_output}")

file(WRITE "${_default_input}" "no matching literal here\nsecond line\n")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("no-match" "${_default_input}" "${_output}")

file(WRITE "${_default_input}" "MARKmatched-but-no-delimiter\nMARKalso-without-delimiter")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("no-delimiter" "${_default_input}" "${_output}")

# A stable build does not rewrite identical bytes.  Waiting past the filesystem
# timestamp granularity makes this observable without relying on Ninja logs.
file(TIMESTAMP "${_output}" _stable_timestamp "%s")
execute_process(COMMAND "${_cmake}" -E sleep 1)
_build_target("${_main_build}" TRUE "" _build_log)
file(TIMESTAMP "${_output}" _after_noop_timestamp "%s")
if(NOT _stable_timestamp STREQUAL _after_noop_timestamp)
    message(FATAL_ERROR "no-op source-value build overwrote unchanged output")
endif()

file(WRITE "${_default_input}" "  MARKcpu= changed /tail\n")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("input-mutation" "${_default_input}" "${_output}")
file(WRITE "${_output}" "tampered product\n")
_build_target("${_main_build}" TRUE "" _build_log)
_compare_to_reference("output-repair" "${_default_input}" "${_output}")

# Configure-time rejection cases cover argument validation and path ownership.
file(WRITE "${_default_input}" "MARKcpu= valid /tail\n")
_configure_case("${_root}/bad-marker-odd" bad-marker FALSE "MARKER_HEX" "-DMARKER_HEX=4")
_configure_case("${_root}/bad-marker-bre" bad-marker FALSE "unsafe BRE/shell" "-DMARKER_HEX=2e")
_configure_case("${_root}/bad-marker-nonascii" bad-marker FALSE "unsafe BRE/shell" "-DMARKER_HEX=ff")
_configure_case("${_root}/bad-file-hash" bad-hash FALSE "does not match FILE_SHA256"
    "-DFILE_SHA256=0000000000000000000000000000000000000000000000000000000000000000")
_configure_case("${_root}/bad-output-escape" output-escape FALSE "dot path component"
    "-DOUTPUT_REL=../../outside/value.txt")
_configure_case("${_root}/bad-input-outside" input-outside FALSE "escapes its declared root"
    "-DINPUT_PATH=${_root}/outside/input.conf")
_configure_case("${_root}/bad-duplicate-output" duplicate FALSE "already")

set(_input_link "${_source_root}/inputs/input-link.conf")
set(_input_outside "${_root}/outside-input.conf")
file(WRITE "${_input_outside}" "MARKoutside /value\n")
file(CREATE_LINK "${_input_outside}" "${_input_link}" SYMBOLIC RESULT _input_link_result)
if(NOT _input_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create input symlink: ${_input_link_result}")
endif()
_configure_case("${_root}/bad-input-symlink" input-symlink FALSE "crosses a symlink"
    "-DINPUT_PATH=${_input_link}")
file(REMOVE "${_input_link}")

set(_recipe_link "${_source_root}/rules/linked-mmakefile.src")
file(CREATE_LINK "${_input_outside}" "${_recipe_link}" SYMBOLIC RESULT _recipe_link_result)
if(NOT _recipe_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create declaring FILE symlink: ${_recipe_link_result}")
endif()
_configure_case("${_root}/bad-file-symlink" file-symlink FALSE "crosses a symlink"
    "-DFILE_REL=rules/linked-mmakefile.src")
file(REMOVE "${_recipe_link}")

set(_outside_output "${_root}/outside-output.txt")
file(WRITE "${_outside_output}" "preserve outside target\n")
set(_config_symlink_build "${_root}/bad-output-symlink")
set(_config_symlink "${_config_symlink_build}/SYS/Prefs/${_output_rel}")
get_filename_component(_config_symlink_parent "${_config_symlink}" DIRECTORY)
file(MAKE_DIRECTORY "${_config_symlink_parent}")
file(CREATE_LINK "${_outside_output}" "${_config_symlink}" SYMBOLIC RESULT _config_symlink_result)
if(NOT _config_symlink_result STREQUAL "0")
    message(FATAL_ERROR "could not create output symlink: ${_config_symlink_result}")
endif()
_configure_case("${_config_symlink_build}" output-symlink FALSE "crosses a symlink")
file(READ "${_outside_output}" _outside_preserved)
if(NOT _outside_preserved STREQUAL "preserve outside target\n")
    message(FATAL_ERROR "configure-time output symlink probe modified the outside target")
endif()

set(_config_directory_build "${_root}/bad-output-directory")
set(_config_directory "${_config_directory_build}/SYS/Prefs/${_output_rel}")
file(MAKE_DIRECTORY "${_config_directory}")
_configure_case("${_config_directory_build}" output-directory FALSE "not a regular non-symlink file")

find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
set(_input_fifo "${_source_root}/inputs/input-fifo.conf")
execute_process(COMMAND "${_mkfifo}" "${_input_fifo}" RESULT_VARIABLE _mkfifo_input_result)
if(NOT _mkfifo_input_result STREQUAL "0")
    message(FATAL_ERROR "could not create input FIFO")
endif()
_configure_case("${_root}/bad-input-fifo" input-fifo FALSE "not a regular non-symlink file"
    "-DINPUT_PATH=${_input_fifo}")
file(REMOVE "${_input_fifo}")

# Runtime checks are repeated by the always-run target, including when Ninja
# would otherwise consider the byproduct current.
set(_runtime_recipe_build "${_root}/runtime-recipe")
_configure_case("${_runtime_recipe_build}" runtime-recipe TRUE "")
_build_target("${_runtime_recipe_build}" TRUE "" _build_log)
set(_runtime_output "${_runtime_recipe_build}/SYS/Prefs/${_output_rel}")
file(READ "${_runtime_output}" _runtime_original)
file(GLOB _private_recipes "${_runtime_recipe_build}/.aros-source-value-rules/*.json")
list(LENGTH _private_recipes _private_recipe_count)
if(NOT _private_recipe_count EQUAL 1)
    message(FATAL_ERROR "expected one sealed source-value recipe, found ${_private_recipe_count}")
endif()
list(GET _private_recipes 0 _private_recipe)
file(WRITE "${_private_recipe}" "tampered private recipe\n")
_build_target("${_runtime_recipe_build}" FALSE "private recipe changed after configuration" _build_log)
_expect_preserved_output("private recipe mutation" "${_runtime_output}" "${_runtime_original}")

set(_runtime_file_build "${_root}/runtime-file-sha")
_configure_case("${_runtime_file_build}" runtime-file TRUE "")
_build_target("${_runtime_file_build}" TRUE "" _build_log)
set(_runtime_file_output "${_runtime_file_build}/SYS/Prefs/${_output_rel}")
file(READ "${_runtime_file_output}" _runtime_file_original)
file(WRITE "${_declaring_file}" "# changed source declaration\n")
_build_target("${_runtime_file_build}" FALSE "SHA256 drifted" _build_log)
_expect_preserved_output("declaring FILE hash drift" "${_runtime_file_output}" "${_runtime_file_original}")
file(WRITE "${_declaring_file}" "# source-value test declaration\n")

set(_runtime_output_build "${_root}/runtime-output-path")
_configure_case("${_runtime_output_build}" runtime-output TRUE "")
_build_target("${_runtime_output_build}" TRUE "" _build_log)
set(_runtime_path_output "${_runtime_output_build}/SYS/Prefs/${_output_rel}")
file(WRITE "${_outside_output}" "preserve outside target\n")
file(REMOVE "${_runtime_path_output}")
file(CREATE_LINK "${_outside_output}" "${_runtime_path_output}" SYMBOLIC RESULT _runtime_link_result)
if(NOT _runtime_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime output symlink")
endif()
_build_target("${_runtime_output_build}" FALSE "crosses a symlink" _build_log)
file(READ "${_outside_output}" _outside_preserved)
if(NOT _outside_preserved STREQUAL "preserve outside target\n")
    message(FATAL_ERROR "runtime output symlink probe modified its target")
endif()
file(REMOVE "${_runtime_path_output}")
file(MAKE_DIRECTORY "${_runtime_path_output}")
_build_target("${_runtime_output_build}" FALSE "not a regular non-symlink file" _build_log)
file(REMOVE_RECURSE "${_runtime_path_output}")
execute_process(COMMAND "${_mkfifo}" "${_runtime_path_output}" RESULT_VARIABLE _mkfifo_output_result)
if(NOT _mkfifo_output_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime output FIFO")
endif()
_build_target("${_runtime_output_build}" FALSE "not a regular non-symlink file" _build_log)
file(REMOVE "${_runtime_path_output}")

set(_runtime_input_build "${_root}/runtime-input-path")
_configure_case("${_runtime_input_build}" runtime-input TRUE "")
_build_target("${_runtime_input_build}" TRUE "" _build_log)
set(_runtime_input_output "${_runtime_input_build}/SYS/Prefs/${_output_rel}")
file(READ "${_runtime_input_output}" _runtime_input_original)
file(REMOVE "${_default_input}")
file(CREATE_LINK "${_input_outside}" "${_default_input}" SYMBOLIC RESULT _runtime_input_link_result)
if(NOT _runtime_input_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime input symlink")
endif()
_build_target("${_runtime_input_build}" FALSE "crosses a symlink" _build_log)
_expect_preserved_output("runtime input symlink" "${_runtime_input_output}" "${_runtime_input_original}")

file(REMOVE_RECURSE "${_root}")
message(STATUS "SourceValueRuleTest passed: sed byte parity, source/path/hash validation, no-op, repair")
