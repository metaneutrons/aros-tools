cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_cmake NAMES cmake REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/source-text-rule")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-source-text-rule-${_suffix} with spaces")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary source text test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

function(_configure_fixture build generated_alias include_alias outside_root)
    set(_args
        "-DAROS_ENGINE_DIR=${_engine_dir}"
        "-DAROS_SOURCE_TEXT_FIXTURE_DIR=${_fixture_dir}")
    if(generated_alias)
        list(APPEND _args "-DAROS_SOURCE_TEXT_GEN_ROOT_ALIAS=ON")
    endif()
    if(include_alias)
        list(APPEND _args "-DAROS_SOURCE_TEXT_INCLUDE_ROOT_ALIAS=ON")
    endif()
    if(outside_root)
        list(APPEND _args "-DAROS_SOURCE_TEXT_TEST_OUTSIDE_ROOT=${outside_root}")
    endif()
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${build}" -G Ninja ${_args}
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(_configure_log "${_stdout}\n${_stderr}" PARENT_SCOPE)
    set(_configure_result "${_result}" PARENT_SCOPE)
endfunction()

function(_build_fixture build target out_log)
    execute_process(
        COMMAND "${_cmake}" --build "${build}" --target "${target}" --parallel 2
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(${out_log} "${_stdout}\n${_stderr}" PARENT_SCOPE)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "source text fixture build failed (${_result})\n${_stdout}\n${_stderr}")
    endif()
endfunction()

function(_compare_files actual expected label)
    execute_process(
        COMMAND "${_cmake}" -E compare_files "${actual}" "${expected}"
        RESULT_VARIABLE _result)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${label} differs byte-for-byte: ${actual}")
    endif()
endfunction()

set(_build "${_root}/build")
_configure_fixture("${_build}" FALSE FALSE "")
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR "source text fixture configure failed\n${_configure_log}")
endif()
_build_fixture("${_build}" all-source-text _first_build_log)

set(_expected_root "${_root}/expected")
file(MAKE_DIRECTORY "${_expected_root}")
set(_expected_header "#define SECOND final\n#define FT_CONFIG_OPTION_USE_PNG\n#define FT_CONFIG_OPTION_USE_BZIP2\nkeep=CRLF\r\nliteral=semi;$value")
set(_expected_header_path "${_expected_root}/ftoption.h")
file(WRITE "${_expected_header_path}" "${_expected_header}")
set(_literal_sysroot [=[${AROS_SYSROOT}]=])
set(_expected_helper "cross=/Developer/bin/cross-tool\nsysroot=${_literal_sysroot}\nsemi=one;two\n")
file(WRITE "${_expected_root}/cross-helper" "${_expected_helper}")
file(WRITE "${_expected_root}/generated.h" "configured=source-local-config\n")
file(WRITE "${_expected_root}/source-generated.h" "source=source-local-config")
file(WRITE "${_expected_root}/sdk-option.h" "sdk=source-local-config\n")
string(ASCII 52 20 32 _expected_aligned)
file(WRITE "${_expected_root}/aligned.bin" "${_expected_aligned}")
string(ASCII 48 160 10 68 79 78 69 10 84 65 73 76 _expected_line_search)
file(WRITE "${_expected_root}/line-search.bin" "${_expected_line_search}")

set(_header_output "${_build}/SYS/Developer/include/freetype/config/ftoption.h")
set(_helper_output "${_build}/hosttools/cross-helper")
set(_configured_gen_output "${_build}/GENINCDIR/config/generated.h")
set(_source_gen_output "${_build}/gen/include/config/source-generated.h")
set(_sdk_output "${_build}/SDK/include/freetype/config/sdk-option.h")
set(_aligned_output "${_build}/GENINCDIR/config/aligned.bin")
set(_line_search_output "${_build}/GENINCDIR/config/line-search.bin")
_compare_files("${_header_output}" "${_expected_header_path}" "FreeType header")
_compare_files("${_helper_output}" "${_expected_root}/cross-helper" "cross helper")
_compare_files("${_configured_gen_output}" "${_expected_root}/generated.h" "configured gen include")
_compare_files("${_source_gen_output}" "${_expected_root}/source-generated.h" "source gen include")
_compare_files("${_sdk_output}" "${_expected_root}/sdk-option.h" "SDK include")
_compare_files("${_aligned_output}" "${_expected_root}/aligned.bin" "byte-aligned replacement")
_compare_files("${_line_search_output}" "${_expected_root}/line-search.bin" "whole-line byte-aligned search")
file(READ "${_build}/Ports/freetype/include/config/line-search.in" _line_search_input_hex HEX)
string(TOLOWER "${_line_search_input_hex}" _line_search_input_hex)
if(NOT _line_search_input_hex STREQUAL "30a00a34142041420a5441494c")
    message(FATAL_ERROR
        "whole-line counterprobe bytes changed unexpectedly: ${_line_search_input_hex}")
endif()

file(GLOB_RECURSE _published_products LIST_DIRECTORIES FALSE
    "${_build}/SYS/Developer/include/*"
    "${_build}/GENINCDIR/*"
    "${_build}/gen/include/*"
    "${_build}/SDK/include/*"
    "${_build}/hosttools/*")
list(LENGTH _published_products _published_count)
if(NOT _published_count EQUAL 7)
    message(FATAL_ERROR
        "source text fixture published unexpected products (${_published_count}): ${_published_products}")
endif()

find_program(_stat NAMES stat PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
execute_process(COMMAND "${_stat}" -c %a "${_helper_output}"
    RESULT_VARIABLE _mode_result OUTPUT_VARIABLE _helper_mode ERROR_QUIET)
if(NOT _mode_result EQUAL 0)
    execute_process(COMMAND "${_stat}" -f %Lp "${_helper_output}"
        RESULT_VARIABLE _mode_result OUTPUT_VARIABLE _helper_mode ERROR_QUIET)
endif()
string(STRIP "${_helper_mode}" _helper_mode)
if(NOT _mode_result EQUAL 0 OR NOT _helper_mode STREQUAL "744")
    message(FATAL_ERROR "cross helper mode is not exactly 744: ${_helper_mode}")
endif()

find_program(_gsed NAMES gsed)
set(_sed "${_gsed}")
if(NOT _sed)
    find_program(_sed_candidate NAMES sed)
    if(_sed_candidate)
        execute_process(COMMAND "${_sed_candidate}" --version
            RESULT_VARIABLE _sed_version_result OUTPUT_VARIABLE _sed_version
            ERROR_QUIET)
        if(_sed_version_result EQUAL 0 AND _sed_version MATCHES "GNU sed")
            set(_sed "${_sed_candidate}")
        endif()
    endif()
endif()
if(_sed)
    set(_sed_expected "${_root}/gnu-sed-helper")
    execute_process(
        COMMAND "${_sed}"
            -e "s|@CROSS@|/Developer/bin/cross-tool|g"
            -e "s|@SYSROOT@|${_literal_sysroot}|g"
            -e "s|@SEMI@|one;two|g"
        INPUT_FILE "${_build}/Ports/freetype/builds/unix/cross-helper.in"
        OUTPUT_FILE "${_sed_expected}"
        RESULT_VARIABLE _sed_result ERROR_VARIABLE _sed_error)
    if(NOT _sed_result EQUAL 0)
        message(FATAL_ERROR "GNU sed source-text reference failed: ${_sed_error}")
    endif()
    _compare_files("${_helper_output}" "${_sed_expected}" "GNU sed cross helper")
    set(_sed_header_expected "${_root}/gnu-sed-header")
    execute_process(
        COMMAND "${_sed}"
            -e "s|.*FIRST.*|#define SECOND intermediate|"
            -e "s|.*SECOND.*|#define SECOND final|"
            -e "s|.*FT_CONFIG_OPTION_USE_PNG.*|#define FT_CONFIG_OPTION_USE_PNG|"
            -e "s|.*FT_CONFIG_OPTION_USE_BZIP2.*|#define FT_CONFIG_OPTION_USE_BZIP2|"
        INPUT_FILE "${_build}/Ports/freetype/include/config/ftoption.h.in"
        OUTPUT_FILE "${_sed_header_expected}"
        RESULT_VARIABLE _sed_header_result ERROR_VARIABLE _sed_header_error)
    if(NOT _sed_header_result EQUAL 0)
        message(FATAL_ERROR "GNU sed line reference failed: ${_sed_header_error}")
    endif()
    _compare_files("${_sed_header_expected}" "${_expected_header_path}" "GNU sed CRLF line contract")
    _compare_files("${_header_output}" "${_sed_header_expected}" "FreeType header GNU sed parity")
    set(_sed_result_text
        "GNU sed helper/line parity checked; matched CRLF loses CR while unmatched CRLF remains")
else()
    set(_sed_result_text "GNU sed unavailable; reference comparison skipped")
endif()

_build_fixture("${_build}" all-source-text _noop_log)
if(_noop_log MATCHES "Transforming source text")
    message(FATAL_ERROR "unchanged Ninja source text build was not a no-op\n${_noop_log}")
endif()

# The input is intentionally absent at configure time and fetched by a rule
# that exposes it only while building. Check whether a later edit is visible
# to the transform dependency graph instead of assuming it is.
set(_sdk_input "${_build}/Ports/freetype/include/config/sdk.in")
file(WRITE "${_sdk_input}" "sdk=mutated@CONFIG@\n")
_build_fixture("${_build}" all-source-text _input_mutation_log)
file(READ "${_sdk_output}" _sdk_after_mutation)
if(NOT _sdk_after_mutation STREQUAL "sdk=mutatedsource-local-config\n")
    message(FATAL_ERROR
        "post-fetch input mutation did not rebuild the product after configure-time absence\n"
        "${_input_mutation_log}\nactual=${_sdk_after_mutation}")
endif()
set(_input_mutation_result "post-fetch input mutation rebuilt the product")
file(WRITE "${_sdk_input}" "sdk=@CONFIG@\n")

file(REMOVE "${_header_output}")
_build_fixture("${_build}" all-source-text _repair_log)
if(NOT EXISTS "${_header_output}")
    message(FATAL_ERROR "missing source text product did not regenerate")
endif()
_compare_files("${_header_output}" "${_expected_header_path}" "regenerated FreeType header")

function(_expect_configure_failure probe expected_text preserved_path outside_root)
    set(_probe_build "${_root}/probe-${probe}")
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_probe_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine_dir}"
            "-DAROS_SOURCE_TEXT_FIXTURE_DIR=${_fixture_dir}"
            "-DAROS_SOURCE_TEXT_PROBE=${probe}"
            "-DAROS_SOURCE_TEXT_TEST_OUTSIDE_ROOT=${outside_root}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _log MATCHES "${expected_text}")
        message(FATAL_ERROR
            "source text probe ${probe} was not rejected as expected (${_result}), "
            "expected /${expected_text}/\n${_log}")
    endif()
    if(preserved_path)
        if(NOT EXISTS "${preserved_path}")
            message(FATAL_ERROR "source text probe ${probe} removed its existing product")
        endif()
        file(READ "${preserved_path}" _preserved)
        if(NOT _preserved STREQUAL "old product")
            message(FATAL_ERROR
                "source text probe ${probe} changed its existing product: ${_preserved}")
        endif()
    endif()
endfunction()

set(_wrong_mode_outside "${_root}/outside-wrong-mode")
_expect_configure_failure("wrong-mode" "MODE must be exactly 744"
    "${_root}/probe-wrong-mode/hosttools/cross-helper" "${_wrong_mode_outside}")
set(_malformed_outside "${_root}/outside-malformed")
_expect_configure_failure("malformed-operations" "exactly kind, token, and replacement"
    "${_root}/probe-malformed-operations/GENINCDIR/old.h" "${_malformed_outside}")
set(_symlink_outside "${_root}/outside-output-symlink")
_expect_configure_failure("output-symlink" "crosses a symlink"
    "${_symlink_outside}/preserved.h" "${_symlink_outside}")
set(_root_symlink_outside "${_root}/outside-configured-root")
_expect_configure_failure("configured-root-symlink" "crosses a symlink"
    "${_root_symlink_outside}/gen/include/preserved.h" "${_root_symlink_outside}")
set(_duplicate_outside "${_root}/outside-duplicate")
_expect_configure_failure("duplicate-output" "already owned"
    "${_root}/probe-duplicate-output/GENINCDIR/duplicate.h" "${_duplicate_outside}")
set(_outside_output "${_root}/outside-output/outside.h")
_expect_configure_failure("outside-output" "output must be below"
    "${_outside_output}" "${_root}/outside-output")
_expect_configure_failure("overlapping-generated-roots"
    "ambiguously inside multiple configured roots"
    "${_root}/probe-overlapping-generated-roots/gen/include/preserved.h"
    "${_root}/outside-overlapping-roots")
set(_fifo_output "${_root}/probe-fifo-input/GENINCDIR/old.h")
_expect_configure_failure("fifo-input" "input is not a regular file"
    "${_fifo_output}" "${_root}/outside-fifo")

# Runtime repeats the checks in case a path is replaced after configuration.
set(_runtime_recipe "${_build}/.aros-source-text-rules/runtime.json")
file(WRITE "${_runtime_recipe}"
    "[ {\"kind\":\"replace_all\",\"token\":\"@CONFIG@\",\"replacement\":\"changed\"} ]")
set(_runner "${_engine_dir}/RunSourceTextRule.cmake")
function(_expect_runtime_failure label input output recipe expected_text preserved_path)
    execute_process(
        COMMAND "${_cmake}"
            "-DINPUT=${input}"
            "-DOUTPUT=${output}"
            "-DINPUT_ROOT=${_build}/Ports/freetype"
            "-DOUTPUT_ROOT=${_build}/GENINCDIR"
            "-DDEVELOPER_INCLUDE_ROOT=${_build}/SYS/Developer/include"
            "-DSDK_INCLUDE_ROOT=${_build}/SDK/include"
            "-DGEN_INCLUDE_ROOT=${_build}/GENINCDIR"
            "-DSOURCE_GEN_INCLUDE_ROOT=${_build}/gen/include"
            "-DHOSTTOOLS_ROOT=${_build}/hosttools"
            "-DBINARY_ROOT=${_build}"
            "-DRECIPE=${recipe}"
            "-DDEPFILE=${recipe}.d"
            -P "${_runner}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr
        TIMEOUT 15)
    set(_log "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _log MATCHES "${expected_text}")
        message(FATAL_ERROR
            "runtime ${label} was not rejected as expected (${_result})\n${_log}")
    endif()
    file(READ "${preserved_path}" _preserved)
    if(NOT _preserved STREQUAL "old product")
        message(FATAL_ERROR "runtime ${label} modified pre-existing output: ${_preserved}")
    endif()
endfunction()

find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
set(_runtime_fifo "${_build}/Ports/freetype/runtime-fifo.in")
execute_process(COMMAND "${_mkfifo}" "${_runtime_fifo}" RESULT_VARIABLE _fifo_result)
if(NOT _fifo_result EQUAL 0)
    message(FATAL_ERROR "could not create runtime FIFO input")
endif()
set(_runtime_fifo_product "${_build}/GENINCDIR/config/runtime-fifo.h")
file(WRITE "${_runtime_fifo_product}" "old product")
_expect_runtime_failure("FIFO input" "${_runtime_fifo}" "${_runtime_fifo_product}"
    "${_runtime_recipe}" "input is not a regular file" "${_runtime_fifo_product}")

set(_runtime_outside "${_root}/outside-runtime-symlink")
file(MAKE_DIRECTORY "${_runtime_outside}")
file(WRITE "${_runtime_outside}/preserved.h" "old product")
set(_runtime_symlink "${_build}/GENINCDIR/runtime-link")
file(CREATE_LINK "${_runtime_outside}" "${_runtime_symlink}"
    SYMBOLIC RESULT _runtime_link_result)
if(NOT _runtime_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime output symlink: ${_runtime_link_result}")
endif()
_expect_runtime_failure("output symlink" "${_build}/Ports/freetype/include/config/generated.in"
    "${_runtime_symlink}/preserved.h" "${_runtime_recipe}" "crosses a symlink"
    "${_runtime_outside}/preserved.h")

# Exact Developer/SDK and configured/fixed source generated-root identities
# are aliases of the same root kind, not ambiguous containment.
set(_alias_build "${_root}/alias-build")
_configure_fixture("${_alias_build}" TRUE FALSE "")
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR "identical generated roots were treated as ambiguous\n${_configure_log}")
endif()
_build_fixture("${_alias_build}" all-source-text _alias_log)

set(_include_alias_build "${_root}/include-alias-build")
_configure_fixture("${_include_alias_build}" FALSE TRUE "")
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR "identical Developer/SDK include roots were treated as ambiguous\n${_configure_log}")
endif()
_build_fixture("${_include_alias_build}" all-source-text _include_alias_log)

message(STATUS
    "Source text test passed: seven byte-exact products, 744 mode, ${_sed_result_text}, "
    "Ninja no-op/recovery, ${_input_mutation_result}, root aliases and configure/runtime refusal preservation")
file(REMOVE_RECURSE "${_root}")
