cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}/aros-full-genmodule-abi-${_suffix}")
if(NOT "$ENV{TMPDIR}")
    set(_root "/tmp/aros-full-genmodule-abi-${_suffix}")
endif()
set(_source "${CMAKE_CURRENT_LIST_DIR}/full-genmodule-abi")
set(_build "${_root}/build")
set(_host_genmodule "$ENV{AROS_HOST_GENMODULE}")
if(NOT _host_genmodule)
    set(_host_genmodule "${AROS_TEST_TREE}/build/pc-x86_64/hosttools/genmodule")
endif()
if(NOT EXISTS "${_host_genmodule}" OR IS_DIRECTORY "${_host_genmodule}")
    message(FATAL_ERROR
        "Full genmodule noincludes regression requires the legacy reference tool at "
        "${_host_genmodule}; set AROS_HOST_GENMODULE if it is elsewhere")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        "-DAROS_HOST_GENMODULE=${_host_genmodule}"
        ${AROS_TEST_TOOL_ARGS}
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "full genmodule ABI fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-includes
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "full genmodule ABI fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

foreach(_target includes-MUI probe-mui-includes)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _noincludes_result
        OUTPUT_VARIABLE _noincludes_stdout
        ERROR_VARIABLE _noincludes_stderr)
    if(NOT _noincludes_result EQUAL 0)
        message(FATAL_ERROR
            "MUI.MiamiPanel noincludes alias ${_target} failed "
            "(${_noincludes_result})\n${_noincludes_stdout}\n${_noincludes_stderr}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-mui-genmodfiles
    RESULT_VARIABLE _noincludes_files_result
    OUTPUT_VARIABLE _noincludes_files_stdout
    ERROR_VARIABLE _noincludes_files_stderr)
if(NOT _noincludes_files_result EQUAL 0)
    message(FATAL_ERROR
        "MUI.MiamiPanel noincludes genmodule producer failed "
        "(${_noincludes_files_result})\n"
        "${_noincludes_files_stdout}\n${_noincludes_files_stderr}")
endif()
if(NOT EXISTS "${_build}/mui-getlibbase-source.txt")
    message(FATAL_ERROR "MUI.MiamiPanel getlibbase archive source was not retained")
endif()
file(READ "${_build}/mui-getlibbase-source.txt" _getlibbase_source)
if(NOT EXISTS "${_getlibbase_source}")
    message(FATAL_ERROR
        "MUI.MiamiPanel getlibbase producer did not materialize ${_getlibbase_source}")
endif()
get_filename_component(_getlibbase_dir "${_getlibbase_source}" DIRECTORY)
get_filename_component(_mui_genmodule_root "${_getlibbase_dir}" DIRECTORY)
get_filename_component(_mui_genmodule_root "${_mui_genmodule_root}" DIRECTORY)
foreach(_absent IN ITEMS
        "${_mui_genmodule_root}/include/clib/MUI_protos.h"
        "${_mui_genmodule_root}/include/inline/MUI.h"
        "${_mui_genmodule_root}/include/defines/MUI.h"
        "${_mui_genmodule_root}/include/defines/MUI_LVO.h"
        "${_mui_genmodule_root}/include/proto/MUI.h"
        "${_mui_genmodule_root}/fd/MUI_lib.fd"
        "${_build}/SDK/include/clib/MUI_protos.h"
        "${_build}/SDK/include/inline/MUI.h"
        "${_build}/SDK/include/defines/MUI.h"
        "${_build}/SDK/include/defines/MUI_LVO.h"
        "${_build}/SDK/include/proto/MUI.h"
        "${_build}/GENINCDIR/clib/MUI_protos.h"
        "${_build}/GENINCDIR/inline/MUI.h"
        "${_build}/GENINCDIR/defines/MUI.h"
        "${_build}/GENINCDIR/defines/MUI_LVO.h"
        "${_build}/GENINCDIR/proto/MUI.h"
        "${_build}/SYS/Developer/include/clib/MUI_protos.h"
        "${_build}/SYS/Developer/include/inline/MUI.h"
        "${_build}/SYS/Developer/include/defines/MUI.h"
        "${_build}/SYS/Developer/include/defines/MUI_LVO.h"
        "${_build}/SYS/Developer/include/proto/MUI.h"
        "${_build}/SYS/Developer/SDK/fd/MUI_lib.fd")
    if(EXISTS "${_absent}")
        message(FATAL_ERROR "MUI.MiamiPanel noincludes unexpectedly created ${_absent}")
    endif()
endforeach()
file(READ "${_build}/build.ninja" _ninja_graph)
if(_ninja_graph MATCHES "MUI_protos\\.h|MUI_lib\\.fd")
    message(FATAL_ERROR "MUI.MiamiPanel noincludes declared header or FD Ninja outputs")
endif()
foreach(_stamp includes fd)
    if(NOT EXISTS "${_build}/${_stamp}.stamp")
        message(FATAL_ERROR
            "building probe-includes did not materialise ${_stamp}.stamp")
    endif()
endforeach()

# The only source-free full module has no function list.  The reference
# genmodule intentionally emits no FD for that shape, so its branch must not
# opt into the FD binder and declare an output that can never exist.
file(READ "${CMAKE_CURRENT_LIST_DIR}/../AROS.cmake" _aros_cmake)
string(FIND "${_aros_cmake}" "    if(ARG_GENMODULE_ONLY)" _source_free_start)
if(_source_free_start LESS 0)
    message(FATAL_ERROR "could not locate the source-free full-module branch")
endif()
string(SUBSTRING "${_aros_cmake}" ${_source_free_start} -1 _source_free_tail)
string(FIND "${_source_free_tail}" "        return()\n    endif()"
    _source_free_length)
if(_source_free_length LESS 0)
    message(FATAL_ERROR "could not locate the end of the source-free branch")
endif()
string(SUBSTRING "${_aros_cmake}" ${_source_free_start}
    ${_source_free_length} _source_free_branch)
if(_source_free_branch MATCHES
   "_aros_generate_module_support\\(_gm ABI|_aros_bind_genmodule_abi_targets|ARG_MMAKE_ID[}]-fd")
    message(FATAL_ERROR
        "source-free full modules must not declare a functionless FD output")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-includes
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0 OR
   NOT _noop_stdout MATCHES "no work to do")
    message(FATAL_ERROR
        "full genmodule ABI fixture was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "full genmodule ABI/FD target contract test passed")
