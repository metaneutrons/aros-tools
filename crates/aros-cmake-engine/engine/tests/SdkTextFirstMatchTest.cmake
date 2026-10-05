cmake_minimum_required(VERSION 3.22)
get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
include("${_engine}/SdkTextRules.cmake")
find_program(_sed NAMES gsed sed REQUIRED)
execute_process(COMMAND "${_sed}" --version OUTPUT_VARIABLE _sed_version
    RESULT_VARIABLE _sed_result)
if(NOT _sed_result EQUAL 0 OR NOT _sed_version MATCHES "GNU sed")
    message(FATAL_ERROR "SDK text first-match differential test requires GNU sed")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}/aros-sdk-text-first-${_suffix}")
if("$ENV{TMPDIR}" STREQUAL "")
    set(_root "/tmp/aros-sdk-text-first-${_suffix}")
endif()
file(MAKE_DIRECTORY "${_root}/Ports/fixture" "${_root}/SYS/Developer/lib" "${_root}/source")
set(_input "${_root}/Ports/fixture/template.pc.in")
set(_output "${_root}/SYS/Developer/lib/pkgconfig/fixture.pc")
set(_declaration "${_root}/source/declaration.mk")
file(WRITE "${_declaration}" "# exact source-owned recipe fixture\n")
file(SHA256 "${_declaration}" _declaration_sha)
set(_operations "REPLACE_FIRST_PER_LINE|@VERSION@|9.1;REPLACE_FIRST_PER_LINE| -I\${includedir}|;REPLACE_ALL|@ALL@|all;REPLACE_ALL| -L\${libdir}|")
foreach(_operation IN LISTS _operations)
    _aros_validate_sdk_text_operation("fixture" "${_operation}")
endforeach()

function(_run output source digest result)
    execute_process(COMMAND "${CMAKE_COMMAND}"
        "-DINPUT=${_input}" "-DOUTPUT=${output}"
        "-DINPUT_ROOT=${_root}/Ports/fixture"
        "-DOUTPUT_ROOT=${_root}/SYS/Developer/lib" "-DBINARY_ROOT=${_root}"
        "-DOPERATIONS=${_operations}"
        "-DDECLARATION_SOURCE=${source}"
        "-DDECLARATION_SOURCE_ROOT=${_root}/source"
        "-DDECLARATION_SHA256=${digest}"
        -P "${_engine}/RunSdkTextRule.cmake"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 15)
    set(${result} "${_result}" PARENT_SCOPE)
    set(_last_log "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

foreach(_case IN ITEMS repeated unterminated empty crlf mixed)
    if(_case STREQUAL repeated)
        set(_bytes [=[@VERSION@ @VERSION@ @ALL@ @ALL@
line -I${includedir} -I${includedir} -L${libdir} -L${libdir}

@VERSION@ @VERSION@
]=])
    elseif(_case STREQUAL unterminated)
        set(_bytes "@VERSION@ @VERSION@\nlast -I\${includedir} -I\${includedir}")
    elseif(_case STREQUAL crlf)
        set(_bytes "@VERSION@ @VERSION@\r\n -I\${includedir} -I\${includedir}\r\n")
    elseif(_case STREQUAL mixed)
        set(_bytes "ä;ö @VERSION@ @VERSION@\r\n\n -I\${includedir}\n@ALL@ @ALL@ -L\${libdir} -L\${libdir}\rEND @VERSION@")
    else()
        set(_bytes "")
    endif()
    file(WRITE "${_input}" "${_bytes}")
    _run("${_output}" "${_declaration}" "${_declaration_sha}" _result)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${_case} native SDK text failed: ${_last_log}")
    endif()
    execute_process(COMMAND "${_sed}" -e "s|@VERSION@|9.1|"
        -e "s| -I\${includedir}||" -e "s|@ALL@|all|g" -e "s| -L\${libdir}||g"
        INPUT_FILE "${_input}" OUTPUT_FILE "${_root}/${_case}.expected"
        RESULT_VARIABLE _reference_result)
    execute_process(COMMAND "${CMAKE_COMMAND}" -E compare_files
        "${_output}" "${_root}/${_case}.expected" RESULT_VARIABLE _compare)
    if(NOT _reference_result EQUAL 0 OR NOT _compare EQUAL 0)
        message(FATAL_ERROR "${_case} SDK text differs from independent GNU sed")
    endif()
endforeach()

set(_project [=[cmake_minimum_required(VERSION 3.22)
project(SdkTextBinding NONE)
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/lib")
include("@_engine@/SdkTextRules.cmake")
add_custom_target(fixture-fetch DEPENDS "${CMAKE_BINARY_DIR}/Ports/fixture/stamp")
set_property(TARGET fixture-fetch PROPERTY AROS_FETCH_DESTINATION "${CMAKE_BINARY_DIR}/Ports/fixture")
set_property(TARGET fixture-fetch PROPERTY AROS_FETCH_COMPLETION_STAMP "${CMAKE_BINARY_DIR}/Ports/fixture/stamp")
aros_transform_sdk_text(NAME fixture-text FETCH fixture-fetch
    FILE declaration.mk FILE_SHA256 "@_declaration_sha@"
    INPUT "${CMAKE_BINARY_DIR}/Ports/fixture/template.pc.in"
    OUTPUT "${AROS_DEVELOPER_LIB_DIR}/pkgconfig/fixture.pc"
    OPERATIONS "REPLACE_FIRST_PER_LINE|@VERSION@|9.1"
        "REPLACE_FIRST_PER_LINE| -I\${includedir}|" "REPLACE_ALL|@ALL@|all"
        "REPLACE_ALL| -L\${libdir}|")
]=])
string(REPLACE "@_engine@" "${_engine}" _project "${_project}")
string(REPLACE "@_declaration_sha@" "${_declaration_sha}" _project "${_project}")
file(WRITE "${_root}/source/CMakeLists.txt" "${_project}")
file(WRITE "${_root}/Ports/fixture/stamp" "locked fixture fetch\n")
execute_process(COMMAND "${CMAKE_COMMAND}" -S "${_root}/source" -B "${_root}" -G Ninja
    RESULT_VARIABLE _configure OUTPUT_VARIABLE _configure_out ERROR_VARIABLE _configure_err)
if(NOT _configure EQUAL 0)
    message(FATAL_ERROR "Source-bound SDK text configure failed: ${_configure_out}\n${_configure_err}")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_root}" --target fixture-text
    RESULT_VARIABLE _build OUTPUT_VARIABLE _build_out ERROR_VARIABLE _build_err)
if(NOT _build EQUAL 0)
    message(FATAL_ERROR "Source-bound SDK text build failed: ${_build_out}\n${_build_err}")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" -E compare_files
    "${_output}" "${_root}/mixed.expected" RESULT_VARIABLE _bound_compare)
if(NOT _bound_compare EQUAL 0)
    message(FATAL_ERROR "Source-bound SDK text CMake/Ninja output differs from sed")
endif()

file(WRITE "${_output}" "previous valid product\n")
# CMake cannot construct NUL strings. The fixed fixture byte is decoded by
# the system printf only in this test, never by the production runner.
find_program(_printf NAMES printf PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
execute_process(COMMAND "${_printf}" "%b" "\\000"
    OUTPUT_FILE "${_input}" RESULT_VARIABLE _nul_fixture)
if(NOT _nul_fixture EQUAL 0)
    message(FATAL_ERROR "Cannot create SDK text NUL counterprobe")
endif()
_run("${_output}" "${_declaration}" "${_declaration_sha}" _nul)
if(_nul EQUAL 0 OR NOT _last_log MATCHES "contains a NUL byte")
    message(FATAL_ERROR "NUL input was not refused: ${_last_log}")
endif()
string(REPEAT "x" 1048577 _too_large)
file(WRITE "${_input}" "${_too_large}")
_run("${_output}" "${_declaration}" "${_declaration_sha}" _oversized)
if(_oversized EQUAL 0 OR NOT _last_log MATCHES "exceeds the 1 MiB")
    message(FATAL_ERROR "Oversized SDK text was not refused: ${_last_log}")
endif()
file(WRITE "${_input}" "@VERSION@\n")
file(WRITE "${_declaration}" "changed source recipe\n")
_run("${_output}" "${_declaration}" "${_declaration_sha}" _changed)
if(_changed EQUAL 0 OR NOT _last_log MATCHES "source changed after translation")
    message(FATAL_ERROR "Changed declaration was not refused: ${_last_log}")
endif()
file(READ "${_output}" _preserved)
if(NOT _preserved STREQUAL "previous valid product\n")
    message(FATAL_ERROR "Failure changed the last valid SDK text product")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_root}" --target fixture-text
    RESULT_VARIABLE _stale_build OUTPUT_VARIABLE _stale_out ERROR_VARIABLE _stale_err)
if(_stale_build EQUAL 0 OR NOT "${_stale_out}\n${_stale_err}" MATCHES "source changed after translation")
    message(FATAL_ERROR "Changed source-bound SDK text Ninja build was not refused: ${_stale_out}\n${_stale_err}")
endif()
file(READ "${_output}" _preserved)
if(NOT _preserved STREQUAL "previous valid product\n")
    message(FATAL_ERROR "Failed source-bound Ninja build changed previous output")
endif()
file(CREATE_LINK "${_declaration}" "${_root}/source/linked.mk" SYMBOLIC)
_run("${_output}" "${_root}/source/linked.mk" "${_declaration_sha}" _linked)
if(_linked EQUAL 0 OR NOT _last_log MATCHES "crosses a symlink")
    message(FATAL_ERROR "Linked declaration was not refused: ${_last_log}")
endif()
file(WRITE "${_root}/outside.mk" "outside source\n")
_run("${_output}" "${_root}/outside.mk" "${_declaration_sha}" _outside)
if(_outside EQUAL 0 OR NOT _last_log MATCHES "escapes its declared root")
    message(FATAL_ERROR "Outside declaration was not refused: ${_last_log}")
endif()
message(STATUS "SDK first-match GNU sed parity and declaration refusals passed: ${_root}")
