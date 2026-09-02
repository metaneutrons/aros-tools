cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
include("${_repo}/cmake/Executable.cmake")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/ahi-build")
set(_perl "/usr/bin/perl")
if(NOT EXISTS "${_perl}")
    message(FATAL_ERROR "AHI runner test requires /usr/bin/perl")
endif()
if(DEFINED ENV{AROS_AHI_RUNNER_BIN} AND
   NOT "$ENV{AROS_AHI_RUNNER_BIN}" STREQUAL "")
    set(_ahi_runner "$ENV{AROS_AHI_RUNNER_BIN}")
else()
    find_program(_ahi_runner NAMES aros-ahi-runner)
endif()
aros_path_is_executable("${_ahi_runner}" _ahi_runner_executable)
if(NOT _ahi_runner_executable)
    message(FATAL_ERROR "AHI runner test requires executable ${_ahi_runner}")
endif()
string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-ahi-build-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")

function(_ahi_configure mode case expect_success expected)
    set(_build "${_root}/${mode}-${case}")
    if(case STREQUAL "whitespace-build")
        set(_build "${_root}/${mode} build")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_REPO_ROOT=${_repo}" "-DHOST_PERL=${_perl}"
            "-DAROS_AHI_RUNNER_BIN=${_ahi_runner}"
            "-DAHI_FIXTURE_MODE=${mode}" "-DAHI_FIXTURE_CASE=${case}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "AHI ${mode} fixture configure failed\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "AHI ${mode}/${case} unexpectedly configured")
    endif()
    if(NOT "${expected}" STREQUAL "")
        string(FIND "${_log}" "${expected}" _at)
        if(_at LESS 0)
            message(FATAL_ERROR "AHI ${mode}/${case} missed '${expected}'\n${_log}")
        endif()
    endif()
    set(AHI_BUILD "${_build}" PARENT_SCOPE)
endfunction()

file(SHA256 "${_repo}/workbench/devs/AHI/configure" _configure_before)
file(TIMESTAMP "${_repo}/workbench/devs/AHI/configure" _configure_time_before UTC)
file(SHA256 "${_repo}/workbench/devs/AHI/ahi-build.inputs" _manifest_before)
foreach(_mode IN ITEMS x86_64 arm aarch64)
    _ahi_configure("${_mode}" "" TRUE "")
    set(_build "${AHI_BUILD}")
    set(_contract "${_build}/.aros-workbench-devs-AHI-subsystem-ahi-contract.cmake")
    if(NOT EXISTS "${_contract}")
        message(FATAL_ERROR "AHI ${_mode} fixture did not generate its runner contract")
    endif()
    file(READ "${_contract}" _contract_content)
    if(NOT _contract_content MATCHES "set\\(AHI_COLLECT ")
        message(FATAL_ERROR "AHI ${_mode} contract omitted the collector")
    endif()
    file(READ "${_build}/gen/configure/workbench/devs/AHI/${_mode}/ahi-cc"
        _wrapper_content)
    if(NOT _wrapper_content MATCHES
            "\\\"\\$collector\\\" --ld \\\"\\$linker\\\" -- \\\"\\$@\\\"")
        message(FATAL_ERROR "AHI ${_mode} wrapper bypasses the collector")
    endif()
    string(REGEX MATCHALL "-mfloat-abi=hard" _hard_float_flags "${_contract_content}")
    list(LENGTH _hard_float_flags _hard_float_count)
    if(_mode STREQUAL "arm")
        if(NOT _hard_float_count EQUAL 4)
            message(FATAL_ERROR
                "AHI arm contract must carry hard-float in C/CPP/AS/LD flags")
        endif()
    elseif(NOT _hard_float_count EQUAL 0)
        message(FATAL_ERROR "AHI ${_mode} contract unexpectedly carries ARM hard-float")
    endif()
    set(_target workbench-devs-AHI-subsystem)
    set(_hostile "/tmp/aros-ahi-inherited-path-must-not-be-used")
    set(_closed_environment
        "CPATH=${_hostile}" "C_INCLUDE_PATH=${_hostile}"
        "CPLUS_INCLUDE_PATH=${_hostile}" "LIBRARY_PATH=${_hostile}"
        "SDKROOT=${_hostile}" "PKG_CONFIG_PATH=${_hostile}"
        "PKG_CONFIG_LIBDIR=${_hostile}" "PKG_CONFIG_SYSROOT_DIR=${_hostile}"
        "CDPATH=${_hostile}" "ENV=${_hostile}" "BASH_ENV=${_hostile}"
        "CPP=/usr/bin/false"
        "AHI_BUILDHANDLER=no" "CPU=host-injected"
        "ASCPPFLAGS=-DHOST_INJECTED" "ARFLAGS=host-injected"
        "CFLAG_RESIDENT=-host-injected" "LDFLAG_RESIDENT=-host-injected"
        "STRIPFLAGS=host-injected" "INSTALL_PROGRAM=/usr/bin/false"
        "INSTALL_DATA=/usr/bin/false" "INSTALL_SCRIPT=/usr/bin/false"
        "DISTDIR=${_hostile}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile}" ${_closed_environment}
            "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "AHI ${_mode} fixture build failed\n${_stdout}${_stderr}")
    endif()
    if(NOT EXISTS "${_build}/collector.log")
        message(FATAL_ERROR "AHI ${_mode} fixture did not execute the collector")
    endif()
    set(_repair "${_build}/SYS/Devs/AudioModes")
    if(_mode STREQUAL "x86_64")
        string(APPEND _repair "/ac97")
    else()
        string(APPEND _repair "/RPIPWM")
    endif()
    if(NOT EXISTS "${_repair}")
        message(FATAL_ERROR "AHI ${_mode} fixture did not install repair product")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile}"
            "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _noop_result OUTPUT_VARIABLE _noop_stdout ERROR_VARIABLE _noop_stderr)
    set(_noop_log "${_noop_stdout}${_noop_stderr}")
    if(NOT _noop_result EQUAL 0 OR NOT _noop_log MATCHES "no work to do")
        message(FATAL_ERROR "AHI ${_mode} fixture no-op failed\n${_noop_log}")
    endif()
    file(REMOVE "${_repair}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile}"
            "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _repair_result
        OUTPUT_VARIABLE _repair_stdout ERROR_VARIABLE _repair_stderr)
    if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_repair}")
        message(FATAL_ERROR "AHI ${_mode} fixture repair failed\n${_repair_stdout}${_repair_stderr}")
    endif()
    _ahi_configure("${_mode}" "" TRUE "")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile}"
            "${CMAKE_COMMAND}" --build "${AHI_BUILD}" --target "${_target}"
        RESULT_VARIABLE _reconfigure_result
        OUTPUT_VARIABLE _reconfigure_stdout ERROR_VARIABLE _reconfigure_stderr)
    set(_reconfigure_log "${_reconfigure_stdout}${_reconfigure_stderr}")
    if(NOT _reconfigure_result EQUAL 0 OR NOT _reconfigure_log MATCHES "no work to do")
        message(FATAL_ERROR "AHI ${_mode} fixture reconfigure no-op failed\n${_reconfigure_log}")
    endif()
    if(_mode STREQUAL "x86_64")
        set(_escaped "${_root}/escaped-install")
        file(REMOVE_RECURSE "${_build}/SYS/Devs/AHI")
        file(MAKE_DIRECTORY "${_escaped}")
        file(CREATE_LINK "${_escaped}" "${_build}/SYS/Devs/AHI" SYMBOLIC)
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile}"
                "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
            RESULT_VARIABLE _symlink_result
            OUTPUT_VARIABLE _symlink_stdout ERROR_VARIABLE _symlink_stderr)
        set(_symlink_log "${_symlink_stdout}${_symlink_stderr}")
        if(_symlink_result EQUAL 0 OR
           NOT _symlink_log MATCHES "install product escaped through a symlink")
            message(FATAL_ERROR "AHI fixture accepted an install-output symlink\n${_symlink_log}")
        endif()
        file(GLOB _escaped_products "${_escaped}/*")
        if(_escaped_products)
            message(FATAL_ERROR "AHI fixture wrote through an install-output symlink")
        endif()
    endif()
endforeach()

_ahi_configure("x86_64" "symlink-binary" FALSE "audited paths escape their owning tree")
_ahi_configure("x86_64" "relative-perl" FALSE "PERL must be an absolute path")
_ahi_configure("x86_64" "missing-collector" FALSE
    "AROS_COLLECT_BIN is not an executable regular file")
_ahi_configure("x86_64" "missing-runner" FALSE
    "AROS_AHI_RUNNER_BIN is not an executable regular file")
_ahi_configure("x86_64" "whitespace-build" FALSE
    "_build_root_raw cannot contain whitespace for configure/Make")
file(SHA256 "${_repo}/workbench/devs/AHI/configure" _configure_after)
file(TIMESTAMP "${_repo}/workbench/devs/AHI/configure" _configure_time_after UTC)
file(SHA256 "${_repo}/workbench/devs/AHI/ahi-build.inputs" _manifest_after)
if(NOT _configure_before STREQUAL _configure_after OR
   NOT _configure_time_before STREQUAL _configure_time_after OR
   NOT _manifest_before STREQUAL _manifest_after)
    message(FATAL_ERROR "AHI runner modified its checkout")
endif()
file(REMOVE_RECURSE "${_root}")
message(STATUS "AHI build test passed")
