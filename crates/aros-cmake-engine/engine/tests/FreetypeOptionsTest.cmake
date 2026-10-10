cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-freetype-options-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/freetype-options")

function(_run_freetype_case _mode _source_text_owner _expect_success)
    set(_build "${_root}/build-${_mode}-${_source_text_owner}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
            "-DTEST_SOURCE_TEXT_OWNER=${_source_text_owner}"
            "-DTEST_SELECTION_MODE=${_mode}"
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)

    if(NOT _expect_success)
        if(_configure_result EQUAL 0)
            message(FATAL_ERROR
                "FreeType options case ${_mode} unexpectedly configured successfully")
        endif()
        if(_mode STREQUAL "MISSING_HELPER")
            set(_expected_error "_aros_native_source_selection_validated is unavailable")
        else()
            set(_expected_error "FreeType option consumer does not exist")
        endif()
        string(FIND "${_configure_stdout}\n${_configure_stderr}"
            "${_expected_error}" _error_at)
        if(NOT _mode STREQUAL "MISSING_HELPER")
            string(FIND "${_configure_stdout}\n${_configure_stderr}"
                "freetype-demo-consumer" _missing_consumer_at)
            if(_missing_consumer_at EQUAL -1)
                set(_error_at -1)
            endif()
        endif()
        if(_error_at EQUAL -1)
            message(FATAL_ERROR
                "FreeType options case ${_mode} failed for the wrong reason\n"
                "${_configure_stdout}\n${_configure_stderr}")
        endif()
        return()
    endif()

    if(NOT _configure_result EQUAL 0)
        message(FATAL_ERROR
            "FreeType options fixture ${_mode} configure failed (${_configure_result})\n"
            "${_configure_stdout}\n${_configure_stderr}")
    endif()

    if(_mode STREQUAL "CLASSIC")
        set(_targets freetype-consumer freetype-demo-consumer)
    else()
        # Subset cases intentionally omit the unselected demo consumer.
        set(_targets freetype-consumer)
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target ${_targets}
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR
            "FreeType options consumer raced its generated header (${_build_result})\n"
            "${_build_stdout}\n${_build_stderr}")
    endif()

    set(_output "${_build}/SDK/include/freetype/config/ftoption.h")
    file(READ "${_output}" _content)
    foreach(_expected IN ITEMS
            "/*define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES*/"
            "#define FT_CONFIG_OPTION_SUBPIXEL_RENDERING"
            "#define FT_CONFIG_OPTION_SYSTEM_ZLIB"
            "#define FT_CONFIG_OPTION_USE_PNG")
        string(FIND "${_content}" "${_expected}" _expected_at)
        if(_expected_at EQUAL -1)
            message(FATAL_ERROR "FreeType options output omitted ${_expected}")
        endif()
    endforeach()
endfunction()

# The source-text owner and normal-writer paths both preserve full classic
# ordering, and both accept build- and consumer-selected source subsets.
foreach(_source_text_owner IN ITEMS OFF ON)
    _run_freetype_case(CLASSIC "${_source_text_owner}" TRUE)
    _run_freetype_case(BUILD_SUBSET "${_source_text_owner}" TRUE)
    _run_freetype_case(CONSUMER_SUBSET "${_source_text_owner}" TRUE)
    _run_freetype_case(UNVALIDATED_SUBSET "${_source_text_owner}" FALSE)
endforeach()

# A classic graph may not silently lose a declared consumer, and native state
# may not bypass source-selection validation when the shared helper is absent
# or reports that source selection was not validated.
_run_freetype_case(CLASSIC_MISSING OFF FALSE)
_run_freetype_case(CLASSIC_MISSING ON FALSE)
_run_freetype_case(MISSING_HELPER OFF FALSE)

file(REMOVE_RECURSE "${_root}")
message(STATUS "FreeType option consumer-ordering and validated-subset tests passed")
