cmake_minimum_required(VERSION 3.24)
get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_root "${CMAKE_CURRENT_BINARY_DIR}/mesa-configured-driver")
set(_source "${_root}/source")
file(MAKE_DIRECTORY "${_source}/configured")
file(WRITE "${_source}/cprobe.c" "int c_probe;\n")
file(WRITE "${_source}/cxxprobe.cpp" "int cxx_probe;\n")
file(WRITE "${_source}/CMakeLists.txt" [=[
cmake_minimum_required(VERSION 3.24)
project(ConfiguredMesaDriverFixture LANGUAGES C CXX)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Give the real host parent directory observable settings. The Mesa child
# receives only its own fake compiler target and must not clear these values.
set(CMAKE_C_FLAGS "-DPARENT_C_SENTINEL=1")
set(CMAKE_CXX_FLAGS "-DPARENT_CXX_SENTINEL=1")
set(CMAKE_C_COMPILER_TARGET "")
set(CMAKE_CXX_COMPILER_TARGET "")
set(_parent_c_flags "${CMAKE_C_FLAGS}")
set(_parent_cxx_flags "${CMAKE_CXX_FLAGS}")
set(_parent_c_target "${CMAKE_C_COMPILER_TARGET}")
set(_parent_cxx_target "${CMAKE_CXX_COMPILER_TARGET}")

add_subdirectory(configured)
add_library(ordinary OBJECT cprobe.c cxxprobe.cpp)

if(NOT "${CMAKE_C_FLAGS}" STREQUAL "${_parent_c_flags}" OR
   NOT "${CMAKE_CXX_FLAGS}" STREQUAL "${_parent_cxx_flags}" OR
   NOT "${CMAKE_C_COMPILER_TARGET}" STREQUAL "${_parent_c_target}" OR
   NOT "${CMAKE_CXX_COMPILER_TARGET}" STREQUAL "${_parent_cxx_target}")
    message(FATAL_ERROR "configured Mesa child changed parent compiler state")
endif()
]=])
file(WRITE "${_source}/configured/CMakeLists.txt" [=[
include("${ENGINE}/ConfiguredMesaDriver.cmake")

set(_triple "aarch64-aros-elf")
# Bind in a real child directory. The fake selected target must be cleared in
# this directory so it does not become an automatic compiler flag.
set(CMAKE_C_COMPILER_TARGET "${_triple}")
set(CMAKE_CXX_COMPILER_TARGET "${_triple}")
set(_expected_c "${CMAKE_C_COMPILER}")
set(_expected_cxx "${CMAKE_CXX_COMPILER}")
if(CASE STREQUAL "wrong-c-path")
    # cmake itself is a known existing executable with a distinct spelling;
    # the binder only compares this path and never invokes it as a compiler.
    set(_expected_c "${CMAKE_COMMAND}")
elseif(CASE STREQUAL "unmeasured-arg1")
    set(CMAKE_C_COMPILER_ARG1 "unmeasured-prefix")
elseif(CASE STREQUAL "wrong-target")
    set(CMAKE_C_COMPILER_TARGET "other-target-none")
elseif(CASE STREQUAL "automatic-sysroot")
    set(CMAKE_SYSROOT "${CMAKE_BINARY_DIR}/AutomaticSDK")
elseif(CASE STREQUAL "automatic-compile-sysroot")
    set(CMAKE_SYSROOT_COMPILE "${CMAKE_BINARY_DIR}/AutomaticSDK")
elseif(CASE STREQUAL "empty-link-sysroot")
    set(CMAKE_SYSROOT_LINK "${CMAKE_BINARY_DIR}/AutomaticSDK")
elseif(CASE STREQUAL "external-toolchain")
    set(CMAKE_C_COMPILER_EXTERNAL_TOOLCHAIN "${CMAKE_BINARY_DIR}/ExternalToolchain")
endif()

set(_nominal_build "/nominal/build")
set(_nominal_gen "/nominal/build/gen")
set(_nominal_ports "/nominal/build/Ports")
set(_nominal_sdk "/nominal/build/sdk/Developer")
if(CASE STREQUAL "escaped-sdk")
    set(_nominal_sdk "/nominal/build/sdk/../escaped/Developer")
elseif(CASE STREQUAL "generated-sdk")
    set(_nominal_sdk "/nominal/build/gen/sdk/Developer")
elseif(CASE STREQUAL "ports-sdk")
    set(_nominal_sdk "/nominal/build/Ports/sdk/Developer")
endif()

set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
set(AROS_DEVELOPER_DIR "${CMAKE_BINARY_DIR}/SDK/Developer")
set(AROS_TARGET_SYSROOT "${AROS_DEVELOPER_DIR}")
file(MAKE_DIRECTORY "${AROS_DEVELOPER_DIR}")
if(CASE STREQUAL "wrong-productive-role")
    set(AROS_TARGET_SYSROOT "${CMAKE_BINARY_DIR}/OtherSDK")
endif()

if(CASE STREQUAL "split-sysroot")
    _aros_bind_mesa_compiler_roles("${_expected_c}" "${_expected_cxx}"
        "${_triple}" "${_nominal_build}" "${_nominal_gen}" "${_nominal_ports}"
        --sysroot "${_nominal_sdk}")
elseif(CASE STREQUAL "equals-sysroot")
    _aros_bind_mesa_compiler_roles("${_expected_c}" "${_expected_cxx}"
        "${_triple}" "${_nominal_build}" "${_nominal_gen}" "${_nominal_ports}"
        "--sysroot=${_nominal_sdk}")
elseif(CASE STREQUAL "empty-sysroot" OR CASE STREQUAL "empty-link-sysroot")
    _aros_bind_mesa_compiler_roles("${_expected_c}" "${_expected_cxx}"
        "${_triple}" "${_nominal_build}" "${_nominal_gen}" "${_nominal_ports}")
elseif(CASE STREQUAL "malformed-option")
    _aros_bind_mesa_compiler_roles("${_expected_c}" "${_expected_cxx}"
        "${_triple}" "${_nominal_build}" "${_nominal_gen}" "${_nominal_ports}"
        --sysroot)
else()
    _aros_bind_mesa_compiler_roles("${_expected_c}" "${_expected_cxx}"
        "${_triple}" "${_nominal_build}" "${_nominal_gen}" "${_nominal_ports}"
        "--sysroot=${_nominal_sdk}")
endif()

if(NOT _aros_mesa_driver_roles_bound OR
   NOT "${CMAKE_C_COMPILER_TARGET}" STREQUAL "" OR
   NOT "${CMAKE_CXX_COMPILER_TARGET}" STREQUAL "")
    message(FATAL_ERROR "Mesa binder did not bind roles within its child directory")
endif()
add_library(configured OBJECT
    "${CMAKE_SOURCE_DIR}/cprobe.c" "${CMAKE_SOURCE_DIR}/cxxprobe.cpp")
target_compile_options(configured PRIVATE ${_aros_mesa_sysroot_options})
]=])

foreach(_case IN ITEMS split-sysroot equals-sysroot empty-sysroot wrong-c-path
        unmeasured-arg1 wrong-target escaped-sdk generated-sdk ports-sdk
        wrong-productive-role malformed-option automatic-sysroot
        automatic-compile-sysroot empty-link-sysroot external-toolchain)
    set(_build "${_root}/build-${_case}")
    execute_process(COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}"
        -G Ninja "-DENGINE=${_engine}" "-DCASE=${_case}"
        -DCMAKE_TOOLCHAIN_FILE= -DCMAKE_C_FLAGS= -DCMAKE_CXX_FLAGS=
        RESULT_VARIABLE _configure OUTPUT_VARIABLE _out ERROR_VARIABLE _err)

    set(_reason "")
    if(_case STREQUAL "wrong-c-path")
        set(_reason "Mesa compiler differs from the measured source driver")
    elseif(_case STREQUAL "unmeasured-arg1")
        set(_reason "Mesa compiler has an unmeasured driver prefix")
    elseif(_case STREQUAL "wrong-target")
        set(_reason "Mesa compiler target differs from selected identity")
    elseif(_case STREQUAL "escaped-sdk" OR _case STREQUAL "generated-sdk" OR
           _case STREQUAL "ports-sdk")
        set(_reason "Mesa compiler sysroot is not a distinct measured SDK role")
    elseif(_case STREQUAL "wrong-productive-role")
        set(_reason "Mesa productive sysroot differs from the SDK role")
    elseif(_case STREQUAL "malformed-option")
        set(_reason "Mesa compiler role has an unmeasured sysroot shape")
    elseif(_case STREQUAL "automatic-sysroot" OR
           _case STREQUAL "automatic-compile-sysroot" OR _case STREQUAL "empty-link-sysroot")
        set(_reason "Mesa compiler has an unmeasured automatic sysroot")
    elseif(_case STREQUAL "external-toolchain")
        set(_reason "Mesa compiler has an unmeasured external toolchain")
    endif()

    if(_reason)
        if(_configure EQUAL 0 OR NOT "${_out}${_err}" MATCHES "${_reason}")
            message(FATAL_ERROR "${_case} was refused for the wrong reason: ${_out}${_err}")
        endif()
        continue()
    endif()
    if(NOT _configure EQUAL 0)
        message(FATAL_ERROR "${_case} configure failed: ${_out}${_err}")
    endif()

    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}" --parallel 1
        RESULT_VARIABLE _build_result OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR "${_case} host C/CXX object compile failed: ${_out}${_err}")
    endif()

    file(READ "${_build}/compile_commands.json" _commands)
    string(JSON _count LENGTH "${_commands}")
    if(NOT _count EQUAL 4)
        message(FATAL_ERROR "${_case} should compile each language for both sibling targets")
    endif()

    set(_configured_count 0)
    set(_ordinary_count 0)
    math(EXPR _last "${_count} - 1")
    foreach(_i RANGE 0 ${_last})
        string(JSON _command GET "${_commands}" ${_i} command)
        string(JSON _file GET "${_commands}" ${_i} file)
        separate_arguments(_argv UNIX_COMMAND "${_command}")
        set(_observed_sysroot "")
        set(_expect_sysroot_value FALSE)
        foreach(_arg IN LISTS _argv)
            if(_arg STREQUAL "--sysroot")
                if(_expect_sysroot_value)
                    message(FATAL_ERROR "${_case} repeated a split sysroot flag: ${_command}")
                endif()
                set(_expect_sysroot_value TRUE)
                list(APPEND _observed_sysroot "--sysroot")
            elseif(_arg MATCHES "^--sysroot=")
                if(_expect_sysroot_value)
                    message(FATAL_ERROR "${_case} changed split sysroot shape: ${_command}")
                endif()
                list(APPEND _observed_sysroot "${_arg}")
            elseif(_expect_sysroot_value)
                list(APPEND _observed_sysroot "${_arg}")
                set(_expect_sysroot_value FALSE)
            endif()
            if(_arg STREQUAL "--target" OR _arg STREQUAL "-target" OR
               _arg MATCHES "^(--target=|-target=)")
                message(FATAL_ERROR "${_case} retained CMake's independent target flag: ${_command}")
            endif()
        endforeach()
        if(_expect_sysroot_value)
            message(FATAL_ERROR "${_case} omitted the split sysroot value: ${_command}")
        endif()
        if(_command MATCHES "/nominal/" OR _command MATCHES "aarch64-aros-elf")
            message(FATAL_ERROR "${_case} leaked a nominal path or fake target: ${_command}")
        endif()
        if(_file MATCHES "[.]c$")
            set(_parent_flag "-DPARENT_C_SENTINEL=1")
        elseif(_file MATCHES "[.]cpp$")
            set(_parent_flag "-DPARENT_CXX_SENTINEL=1")
        else()
            message(FATAL_ERROR "${_case} compiled an unexpected source: ${_file}")
        endif()
        string(FIND "${_command}" "${_parent_flag}" _parent_flag_index)
        if(_parent_flag_index LESS 0)
            message(FATAL_ERROR "${_case} lost a parent compiler flag: ${_command}")
        endif()

        if(_command MATCHES "CMakeFiles/configured[.]dir/")
            math(EXPR _configured_count "${_configured_count} + 1")
            if(_case STREQUAL "split-sysroot")
                set(_expected_sysroot "--sysroot;${_build}/SDK/Developer")
            elseif(_case STREQUAL "equals-sysroot")
                set(_expected_sysroot "--sysroot=${_build}/SDK/Developer")
            else()
                set(_expected_sysroot "")
            endif()
            list(JOIN _observed_sysroot ";" _observed_sysroot_value)
            if(NOT "${_observed_sysroot_value}" STREQUAL "${_expected_sysroot}")
                message(FATAL_ERROR "${_case} changed configured sysroot argv: ${_observed_sysroot}")
            endif()
        elseif(_command MATCHES "CMakeFiles/ordinary[.]dir/")
            math(EXPR _ordinary_count "${_ordinary_count} + 1")
            if(NOT "${_observed_sysroot}" STREQUAL "")
                message(FATAL_ERROR "${_case} changed the ordinary sibling: ${_command}")
            endif()
        else()
            message(FATAL_ERROR "${_case} compile command has no expected sibling owner: ${_command}")
        endif()
    endforeach()
    if(NOT _configured_count EQUAL 2 OR NOT _ordinary_count EQUAL 2)
        message(FATAL_ERROR "${_case} lost C/CXX ownership in compile_commands.json")
    endif()
endforeach()
