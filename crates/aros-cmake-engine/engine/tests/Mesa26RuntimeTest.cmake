cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros mesa26 runtime ${_suffix}")
cmake_path(NORMAL_PATH _root)
set(_source "${CMAKE_CURRENT_LIST_DIR}/mesa26-runtime")
set(_module "${CMAKE_CURRENT_LIST_DIR}/../Mesa26Runtime.cmake")

function(_source_inventory root output)
    file(GLOB_RECURSE _files LIST_DIRECTORIES FALSE RELATIVE "${root}"
        "${root}/*")
    list(SORT _files)
    set(_inventory "")
    foreach(_file IN LISTS _files)
        file(SHA256 "${root}/${_file}" _digest)
        string(APPEND _inventory "${_file}:${_digest}\n")
    endforeach()
    set(${output} "${_inventory}" PARENT_SCOPE)
endfunction()

function(_assert_source_unchanged expected)
    _source_inventory("${_source}" _actual)
    if(NOT _actual STREQUAL expected)
        message(FATAL_ERROR "Mesa26 runtime configure/build wrote into its source fixture")
    endif()
endfunction()

function(_configure case version loader provider expect_success expected_message)
    set(_build "${_root}/${case}")
    set(_sys_dir "${_build}/SYS")
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            -S "${_source}" -B "${_build}" -G Ninja
            "-DMESA26_RUNTIME_MODULE=${_module}"
            "-DMESA26_RUNTIME_CASE=${case}"
            "-DAROS_MESA_VERSION=${version}"
            "-DAROS_SYS_DIR=${_sys_dir}"
            "-DMESA26_LOADER_NAME=${loader}"
            "-DMESA26_PROVIDER_NAME=${provider}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "Mesa26 runtime ${case} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "Mesa26 runtime ${case} configure unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "Mesa26 runtime ${case} missed '${expected_message}':\n${_log}")
        endif()
    endif()
    set(_build "${_build}" PARENT_SCOPE)
    set(_sys_dir "${_sys_dir}" PARENT_SCOPE)
endfunction()

function(_assert_consumer_loader_build case)
    set(_build "${_root}/${case}")
    set(_sys_dir "${_build}/SYS")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target workbench-libs-gl
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "Mesa26 consumer loader ${case} build failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
    if(EXISTS "${_build}/gen/mesa26-runtime/GL.default" OR
       EXISTS "${_sys_dir}/Prefs/Env-Archive/SYS/GL.default")
        message(FATAL_ERROR
            "Mesa26 consumer loader ${case} published GL.default")
    endif()
endfunction()

_source_inventory("${_source}" _source_before)

_configure(positive 26.0.0 gl.library mesa3dgl26-0.library TRUE "")
set(_positive_build "${_build}")
set(_positive_sys_dir "${_sys_dir}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_positive_build}"
        --target mesa26-runtime-selection
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "Mesa26 runtime selection build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
set(_selection "${_positive_sys_dir}/Prefs/Env-Archive/SYS/GL.default")
if(NOT EXISTS "${_selection}")
    message(FATAL_ERROR "Mesa26 runtime selection was not materialized at ${_selection}")
endif()
file(READ "${_selection}" _selection_content)
if(NOT _selection_content STREQUAL "mesa3dgl26-0\n")
    message(FATAL_ERROR "Mesa26 runtime selection has wrong bytes: '${_selection_content}'")
endif()
file(REMOVE "${_selection}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_positive_build}"
        --target mesa26-runtime-selection
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0)
    message(FATAL_ERROR
        "Mesa26 runtime selection repair failed (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()
if(NOT EXISTS "${_selection}")
    message(FATAL_ERROR "Mesa26 runtime selection deletion was not repaired")
endif()
file(READ "${_selection}" _repaired_content)
if(NOT _repaired_content STREQUAL "mesa3dgl26-0\n")
    message(FATAL_ERROR "Repaired Mesa26 runtime selection has wrong bytes")
endif()

_configure(wrong-loader 26.0.0 not-gl.library mesa3dgl26-0.library FALSE
    "Mesa26 runtime identity mismatch: not-gl.library, mesa3dgl26-0.library")
_configure(wrong-provider 26.0.0 gl.library not-mesa.library FALSE
    "Mesa26 runtime identity mismatch: gl.library, not-mesa.library")
_configure(missing-provider 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "Mesa26 GL loader requires its source-selected implementation")

# A source-bound SDK consumer can expose the GL loader without selecting the
# source-owned Mesa runtime implementation. Exercise both shapes of the SDK
# graph, including a missing implementation target.
_configure(consumer-missing-provider 26.0.0 gl.library mesa3dgl26-0.library TRUE "")
_assert_consumer_loader_build(consumer-missing-provider)
_configure(consumer-present-provider 26.0.0 gl.library mesa3dgl26-0.library TRUE "")
_assert_consumer_loader_build(consumer-present-provider)

# A cache flag cannot manufacture configure-process validation. Current
# bindings also reject a changed contract digest or changed target selector.
_configure(consumer-cached-flag 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "requires fresh validation")
_configure(consumer-changed-digest 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "configuration changed after validation")
_configure(consumer-changed-selector 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "AROS_TARGET_PLATFORM changed after validation")
_configure(consumer-changed-contract-bytes 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "contract changed after validation")
_configure(consumer-and-build-contract 26.0.0 gl.library mesa3dgl26-0.library FALSE
    "build and consumer selections")

_configure(non26 25.3.0 gl.library mesa3dgl26-0.library TRUE "")
set(_non26_build "${_build}")
set(_non26_sys_dir "${_sys_dir}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_non26_build}"
    RESULT_VARIABLE _non26_result
    OUTPUT_VARIABLE _non26_stdout
    ERROR_VARIABLE _non26_stderr)
if(NOT _non26_result EQUAL 0)
    message(FATAL_ERROR
        "non26 Mesa configure build failed (${_non26_result})\n"
        "${_non26_stdout}\n${_non26_stderr}")
endif()
if(EXISTS "${_non26_build}/gen/mesa26-runtime/GL.default" OR
   EXISTS "${_non26_sys_dir}/Prefs/Env-Archive/SYS/GL.default")
    message(FATAL_ERROR "non26 Mesa configuration materialized Mesa26 runtime files")
endif()

_assert_source_unchanged("${_source_before}")
file(REMOVE_RECURSE "${_root}")
message(STATUS "Mesa26 runtime dependency and selection tests passed")
