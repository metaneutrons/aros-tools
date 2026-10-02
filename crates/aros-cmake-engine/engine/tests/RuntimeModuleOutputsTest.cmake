cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root /tmp)
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros runtime outputs ${_suffix}")
cmake_path(NORMAL_PATH _root)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/runtime-module-outputs")
foreach(_case IN ITEMS selected reverse case-insensitive none-selected
        separate-directory separate-suffix both-selected no-root late-conflict)
    set(_build "${_root}/${_case}")
    execute_process(COMMAND "${CMAKE_COMMAND}" -G Ninja -S "${_fixture}"
        -B "${_build}" "-DTEST_CASE=${_case}"
        TIMEOUT "${AROS_TEST_CHILD_TIMEOUT}"
        RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(_case MATCHES "^(both-selected|no-root|late-conflict)$")
        if(_status EQUAL 0 OR NOT "${_stdout}${_stderr}" MATCHES
                "Conflicting runtime module output.*example.resource")
            message(FATAL_ERROR "runtime outputs ${_case} missed conflict: ${_stdout}${_stderr}")
        endif()
        foreach(_owner IN ITEMS selected-provider manual-provider)
            if(NOT "${_stdout}${_stderr}" MATCHES "${_owner}")
                message(FATAL_ERROR "runtime outputs conflict omitted ${_owner}")
            endif()
        endforeach()
        continue()
    endif()
    if(NOT _status EQUAL 0)
        message(FATAL_ERROR "runtime outputs ${_case} configure failed (${_status}): ${_stdout}${_stderr}")
    endif()
    file(READ "${_build}/selected-provider.path" _selected)
    file(READ "${_build}/manual-provider.path" _manual)
    file(READ "${_build}/unique-provider.path" _unique)
    if(NOT _unique STREQUAL "${_build}/SYS/Devs/unique.resource")
        message(FATAL_ERROR "unique runtime output moved: ${_unique}")
    endif()
    if(_case STREQUAL none-selected)
        foreach(_path IN ITEMS "${_selected}" "${_manual}")
            if(NOT _path MATCHES "/gen/manual-modules/[0-9a-f]+/example.resource$")
                message(FATAL_ERROR "unselected runtime alternative escaped private output: ${_path}")
            endif()
        endforeach()
    elseif(_case STREQUAL separate-directory)
        if(NOT _selected STREQUAL "${_build}/SYS/Devs/example.resource" OR
           NOT _manual STREQUAL "${_build}/SYS/Libs/example.resource")
            message(FATAL_ERROR "explicit distinct runtime directories moved")
        endif()
    elseif(_case STREQUAL separate-suffix)
        if(NOT _selected STREQUAL "${_build}/SYS/Devs/example.resource" OR
           NOT _manual STREQUAL "${_build}/SYS/Devs/example.library")
            message(FATAL_ERROR "explicit distinct runtime suffixes moved")
        endif()
    elseif(_case MATCHES "^(selected|reverse|case-insensitive)$")
        if(NOT _selected STREQUAL "${_build}/SYS/Devs/example.resource" OR
           NOT _manual MATCHES "/gen/manual-modules/[0-9a-f]+/[Ee][Xx][Aa][Mm][Pp][Ll][Ee][.][Rr][Ee][Ss][Oo][Uu][Rr][Cc][Ee]$")
            message(FATAL_ERROR "wrong selected/private runtime output: ${_selected};${_manual}")
        endif()
    endif()
    if(_manual STREQUAL _selected)
        message(FATAL_ERROR "runtime alternatives share an output")
    endif()
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        TIMEOUT "${AROS_TEST_CHILD_TIMEOUT}"
        RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(NOT _status EQUAL 0 OR NOT EXISTS "${_unique}")
        message(FATAL_ERROR "runtime outputs ${_case} build failed (${_status}): ${_stdout}${_stderr}")
    endif()
    if(_case STREQUAL none-selected AND
       (EXISTS "${_selected}" OR EXISTS "${_manual}" OR
        EXISTS "${_build}/SYS/Devs/example.resource"))
        message(FATAL_ERROR "default build compiled unselected runtime alternatives")
    endif()
    if(_case STREQUAL none-selected)
        execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}"
            --target selected-provider manual-provider
            TIMEOUT "${AROS_TEST_CHILD_TIMEOUT}"
            RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
        if(NOT _status EQUAL 0 OR NOT EXISTS "${_selected}" OR
           NOT EXISTS "${_manual}" OR
           EXISTS "${_build}/SYS/Devs/example.resource")
            message(FATAL_ERROR
                "unselected runtime alternatives were not independently private: "
                "${_stdout}${_stderr}")
        endif()
    endif()
    if(_case MATCHES "^(selected|reverse|case-insensitive)$")
        if(NOT EXISTS "${_selected}" OR EXISTS "${_manual}")
            message(FATAL_ERROR "default build selected wrong runtime alternative")
        endif()
        file(SHA256 "${_selected}" _before)
        execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target manual-provider
            TIMEOUT "${AROS_TEST_CHILD_TIMEOUT}"
            RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
        if(NOT _status EQUAL 0 OR NOT EXISTS "${_manual}")
            message(FATAL_ERROR "manual runtime alternative not buildable (${_status}): ${_stdout}${_stderr}")
        endif()
        file(SHA256 "${_selected}" _after)
        if(NOT _before STREQUAL _after)
            message(FATAL_ERROR "manual runtime alternative overwrote selected output")
        endif()
        file(SHA256 "${_manual}" _manual_sha)
        if(_manual_sha STREQUAL _after)
            message(FATAL_ERROR "independent provider fixture failed to produce distinct binaries")
        endif()
    endif()
endforeach()

set(_builder_fixture
    "${CMAKE_CURRENT_LIST_DIR}/runtime-module-builders")
foreach(_builder IN ITEMS library genmodule-only device resource)
    if(_builder STREQUAL "device" OR _builder STREQUAL "resource")
        set(_runtime_owner version)
    else()
        set(_runtime_owner runtime-owner)
    endif()
    if(_builder STREQUAL "genmodule-only" OR _builder STREQUAL "resource")
        set(_manual_builder custom)
    else()
        set(_manual_builder simple)
    endif()
    foreach(_selection IN ITEMS runtime manual both)
        set(_case "${_builder}-${_manual_builder}-${_selection}")
        set(_build "${_root}/${_case}")
        execute_process(COMMAND "${CMAKE_COMMAND}" -G Ninja
            -S "${_builder_fixture}" -B "${_build}"
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_HOST_GENMODULE=${AROS_TEST_TOOLS_DIR}/aros-genmodule"
            "-DTEST_BUILDER=${_builder}"
            "-DTEST_MANUAL_BUILDER=${_manual_builder}"
            "-DTEST_SELECTION=${_selection}"
            ${AROS_TEST_TOOL_ARGS}
            TIMEOUT "${AROS_TEST_CHILD_TIMEOUT}"
            RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
        if(_selection STREQUAL "both")
            if(_status EQUAL 0 OR NOT "${_stdout}${_stderr}" MATCHES
                    "Conflicting runtime module output.*version.resource")
                message(FATAL_ERROR
                    "runtime builder ${_case} missed conflict: ${_stdout}${_stderr}")
            endif()
            foreach(_owner IN ITEMS "${_runtime_owner}" manual-owner)
                if(NOT "${_stdout}${_stderr}" MATCHES "${_owner}")
                    message(FATAL_ERROR
                        "runtime builder conflict omitted ${_owner}: ${_stdout}${_stderr}")
                endif()
            endforeach()
            continue()
        endif()
        if(NOT _status EQUAL 0)
            message(FATAL_ERROR
                "runtime builder ${_case} configure failed (${_status}): ${_stdout}${_stderr}")
        endif()
        file(READ "${_build}/runtime-owner.path" _runtime_path)
        file(READ "${_build}/manual-owner.path" _manual_path)
        set(_canonical_path "${_build}/SYS/Devs/version.resource")
        if(_selection STREQUAL "runtime")
            if(NOT _runtime_path STREQUAL _canonical_path OR
               NOT _manual_path MATCHES
                   "/gen/manual-modules/[0-9a-f]+/version[.]resource$")
                message(FATAL_ERROR
                    "runtime builder ${_case} selected wrong paths: "
                    "${_runtime_path};${_manual_path}")
            endif()
        else()
            if(NOT _manual_path STREQUAL _canonical_path OR
               NOT _runtime_path MATCHES
                   "/gen/manual-modules/[0-9a-f]+/version[.]resource$")
                message(FATAL_ERROR
                    "runtime builder ${_case} selected wrong paths: "
                    "${_runtime_path};${_manual_path}")
            endif()
        endif()
    endforeach()
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "runtime module output closure and builder tests passed")
