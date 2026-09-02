cmake_minimum_required(VERSION 3.22)

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}/aros-full-genmodule-abi-${_suffix}")
if(NOT "$ENV{TMPDIR}")
    set(_root "/tmp/aros-full-genmodule-abi-${_suffix}")
endif()
set(_source "${CMAKE_CURRENT_LIST_DIR}/full-genmodule-abi")
set(_build "${_root}/build")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
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
