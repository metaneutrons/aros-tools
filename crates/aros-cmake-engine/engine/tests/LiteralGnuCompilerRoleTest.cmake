cmake_minimum_required(VERSION 3.22)

foreach(_required IN ITEMS ENGINE_DIR TEST_BINARY_DIR GNU_CC GNU_CXX GNU_READELF GNU_AR GNU_RANLIB)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "${_required} must be provided")
    endif()
endforeach()

if(NOT IS_DIRECTORY "${ENGINE_DIR}")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
file(REAL_PATH "${_engine_dir}" _engine_dir)
if(NOT EXISTS "${_engine_dir}/LiteralObjects.cmake" OR
   NOT EXISTS "${_engine_dir}/LiteralCompilerRole.cmake" OR
   NOT EXISTS "${_engine_dir}/NativeConsumerContract.cmake")
    message(FATAL_ERROR "ENGINE_DIR does not contain the literal GNU compiler-role adapter")
endif()

find_program(_test_program NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
foreach(_tool_var IN ITEMS GNU_CC GNU_CXX GNU_READELF GNU_AR GNU_RANLIB)
    if(NOT IS_ABSOLUTE "${${_tool_var}}")
        cmake_path(ABSOLUTE_PATH ${_tool_var} NORMALIZE OUTPUT_VARIABLE _tool_path)
    else()
        set(_tool_path "${${_tool_var}}")
        cmake_path(ABSOLUTE_PATH _tool_path NORMALIZE OUTPUT_VARIABLE _tool_path)
    endif()
    file(REAL_PATH "${_tool_path}" _tool_path)
    execute_process(COMMAND "${_test_program}" -f "${_tool_path}"
        RESULT_VARIABLE _is_regular)
    execute_process(COMMAND "${_test_program}" -x "${_tool_path}"
        RESULT_VARIABLE _is_executable)
    if(NOT EXISTS "${_tool_path}" OR IS_DIRECTORY "${_tool_path}" OR
       NOT _is_regular STREQUAL "0" OR NOT _is_executable STREQUAL "0")
        message(FATAL_ERROR "${_tool_var} must name an existing executable: ${_tool_path}")
    endif()
    set(_${_tool_var} "${_tool_path}")
endforeach()

# All five inputs must come from the same relocated GCC/Binutils package.
cmake_path(GET _GNU_CC PARENT_PATH _gnu_package_dir)
foreach(_tool_path IN ITEMS "${_GNU_CXX}" "${_GNU_READELF}" "${_GNU_AR}" "${_GNU_RANLIB}")
    cmake_path(GET _tool_path PARENT_PATH _tool_package_dir)
    if(NOT _tool_package_dir STREQUAL _gnu_package_dir)
        message(FATAL_ERROR "GNU compiler and Binutils inputs must share one package directory")
    endif()
endforeach()

if(NOT IS_ABSOLUTE "${TEST_BINARY_DIR}")
    cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
else()
    set(_test_binary_dir "${TEST_BINARY_DIR}")
    cmake_path(ABSOLUTE_PATH _test_binary_dir NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
endif()
file(MAKE_DIRECTORY "${_test_binary_dir}")
file(REAL_PATH "${_test_binary_dir}" _test_binary_dir)
find_program(_ninja NAMES ninja REQUIRED)

string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_test_binary_dir}/literal-gnu-compiler-role-${_suffix}")
while(EXISTS "${_test_root}" OR IS_SYMLINK "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_test_binary_dir}/literal-gnu-compiler-role-${_suffix}")
endwhile()
set(_fixture "${_test_root}/fixture")
file(MAKE_DIRECTORY "${_fixture}/declared-source/src")

file(WRITE "${_fixture}/Toolchain.cmake" [=[
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR riscv32)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
set(CMAKE_C_COMPILER "${PROBE_GNU_CC}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER "${PROBE_GNU_CXX}" CACHE FILEPATH "" FORCE)
set(CMAKE_AR "${PROBE_GNU_AR}" CACHE FILEPATH "" FORCE)
set(CMAKE_RANLIB "${PROBE_GNU_RANLIB}" CACHE FILEPATH "" FORCE)
set(CMAKE_C_COMPILER_AR "${PROBE_GNU_AR}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER_AR "${PROBE_GNU_AR}" CACHE FILEPATH "" FORCE)
set(CMAKE_C_COMPILER_RANLIB "${PROBE_GNU_RANLIB}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER_RANLIB "${PROBE_GNU_RANLIB}" CACHE FILEPATH "" FORCE)
]=])

file(WRITE "${_fixture}/CMakeLists.txt" [=[
cmake_minimum_required(VERSION 3.22)
project(LiteralGnuCompilerRoleFixture LANGUAGES C CXX)

if(NOT CMAKE_C_COMPILER_ID STREQUAL "GNU" OR
   NOT CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    message(FATAL_ERROR "The fixture requires the selected GNU C and C++ drivers")
endif()
if(NOT CMAKE_TRY_COMPILE_TARGET_TYPE STREQUAL "STATIC_LIBRARY")
    message(FATAL_ERROR "GNU compiler detection must use static-library try-compile")
endif()

include("${ENGINE_DIR}/NativeConsumerContract.cmake")
include("${ENGINE_DIR}/LiteralObjects.cmake")

# Compiler detection above runs without these poison values. Literal object
# commands must not import either language's global CMake flags.
set(CMAKE_C_FLAGS "-DLITERAL_CMAKE_C_FLAGS_POISON=1")
set(CMAKE_CXX_FLAGS "-DLITERAL_CMAKE_CXX_FLAGS_POISON=1")

set(AROS_SOURCE_DIR "${CMAKE_CURRENT_LIST_DIR}/declared-source")
set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}/declared-build")
file(REAL_PATH "${AROS_SOURCE_DIR}" AROS_SOURCE_DIR)
file(MAKE_DIRECTORY
    "${AROS_BUILD_DIR}/gen/literal"
    "${AROS_BUILD_DIR}/GENINCDIR"
    "${AROS_BUILD_DIR}/SYS/Developer/include/aros/stdc")
set(AROS_TARGET_SYSROOT "${AROS_BUILD_DIR}/SYS/Developer")
set(AROS_DEVELOPER_INCLUDE_DIR "${AROS_TARGET_SYSROOT}/include")
set(AROS_SDK_INCLUDE_DIR "${AROS_DEVELOPER_INCLUDE_DIR}")
set(AROS_GENINC_DIR "${AROS_BUILD_DIR}/GENINCDIR")

set(AROS_GNU_TARGET_COMPILE_OPTIONS
    -march=rv32imafc -mabi=ilp32f -mcmodel=medany -mno-strict-align)
if(NOT AROS_GNU_TARGET_COMPILE_OPTIONS STREQUAL
       "-march=rv32imafc;-mabi=ilp32f;-mcmodel=medany;-mno-strict-align")
    message(FATAL_ERROR "The GNU target options differ from the selected RV32 role")
endif()

set(_binding_fixture "${CMAKE_BINARY_DIR}/source-binding-test-double.json")
file(WRITE "${_binding_fixture}" "{\"fixture_only\":true}\n")
file(REAL_PATH "${_binding_fixture}" _binding_fixture)
file(SHA256 "${_binding_fixture}" _binding_digest)

if(LITERAL_ROLE_MODE STREQUAL "consumer")
    set(AROS_NATIVE_CONSUMER_CONTRACT "${_binding_fixture}")
    set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256 "${_binding_digest}")
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED FALSE)
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
        "aros-native-consumer-validation-v1")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ISA "rv32imafc")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ABI "ilp32f")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_CODE_MODEL "medany")

    # ISOLATED TEST DOUBLE: this binds only the fixture's source and dummy
    # contract for compiler-adapter coverage. It is not source-seal,
    # source-graph, or release-qualification evidence.
    function(_aros_native_consumer_current out_source out_path out_digest)
        file(REAL_PATH "${AROS_SOURCE_DIR}" _source)
        file(REAL_PATH "${AROS_NATIVE_CONSUMER_CONTRACT}" _path)
        file(SHA256 "${_path}" _digest)
        set(${out_source} "${_source}" PARENT_SCOPE)
        set(${out_path} "${_path}" PARENT_SCOPE)
        set(${out_digest} "${_digest}" PARENT_SCOPE)
    endfunction()

    if(NOT COMMAND aros_validate_native_consumer_compiler_target)
        message(FATAL_ERROR "The native consumer GNU target validator is unavailable")
    endif()
    aros_validate_native_consumer_compiler_target()
elseif(LITERAL_ROLE_MODE STREQUAL "native-build")
    set(AROS_NATIVE_BUILD_CONTRACT "${_binding_fixture}")
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${_binding_digest}")
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED FALSE)
    set(AROS_NATIVE_BUILD_ABI_ISA "rv32imafc")
    set(AROS_NATIVE_BUILD_ABI "ilp32f")
    set(AROS_NATIVE_BUILD_ABI_CODE_MODEL "medany")

    # ISOLATED TEST DOUBLE: this binds only the fixture's source and dummy
    # contract for compiler-adapter coverage. It is not source-seal,
    # source-graph, or release-qualification evidence.
    function(_aros_native_require_current_validation out_source out_path out_digest)
        file(REAL_PATH "${AROS_SOURCE_DIR}" _source)
        file(REAL_PATH "${AROS_NATIVE_BUILD_CONTRACT}" _path)
        file(SHA256 "${_path}" _digest)
        set(${out_source} "${_source}" PARENT_SCOPE)
        set(${out_path} "${_path}" PARENT_SCOPE)
        set(${out_digest} "${_digest}" PARENT_SCOPE)
    endfunction()

    if(NOT COMMAND aros_validate_native_build_compiler_target)
        message(FATAL_ERROR "The native build GNU target validator is unavailable")
    endif()
    aros_validate_native_build_compiler_target()
else()
    message(FATAL_ERROR "LITERAL_ROLE_MODE must be consumer or native-build")
endif()

set(_source_arguments
    "-DROLE_SEQUENCE=1"
    "-UROLE_SEQUENCE"
    "-DROLE_SEQUENCE=2"
    "-DROLE_TEXT=\"hello world\"")
if(LITERAL_GNU_ABI_CONFLICT)
    list(APPEND _source_arguments -mabi=ilp32d)
endif()

set(_c_object "${AROS_BUILD_DIR}/gen/literal/role-c.o")
set(_cxx_object "${AROS_BUILD_DIR}/gen/literal/role-cxx.o")
aros_compile_literal_object(NAME literal-role-c
    SOURCE "${AROS_SOURCE_DIR}/src/role.c"
    OUTPUT "${_c_object}"
    LANGUAGE C
    ARGUMENTS ${_source_arguments})
aros_compile_literal_object(NAME literal-role-cxx
    SOURCE "${AROS_SOURCE_DIR}/src/role.cpp"
    OUTPUT "${_cxx_object}"
    LANGUAGE CXX
    ARGUMENTS ${_source_arguments})
aros_literal_object_group(NAME literal-role-objects
    OBJECTS "${_c_object}" "${_cxx_object}")
]=])

file(WRITE "${_fixture}/declared-source/src/role.c" [=[
#ifdef LITERAL_CMAKE_C_FLAGS_POISON
#error CMAKE_C_FLAGS leaked into the literal compiler command
#endif
#if !defined(__riscv_xlen) || __riscv_xlen != 32
#error selected compiler role is not RV32
#endif
#ifndef __riscv_float_abi_single
#error selected compiler role does not use the single-float ABI
#endif
#if ROLE_SEQUENCE != 2
#error ordered -D/-U source arguments were not preserved
#endif
_Static_assert(sizeof(void *) == 4, "selected C compiler must have 32-bit pointers");
_Static_assert(sizeof(ROLE_TEXT) == 12, "string macro with spaces was changed");
int literal_gnu_role_c(void) { return ROLE_SEQUENCE; }
]=])

file(WRITE "${_fixture}/declared-source/src/role.cpp" [=[
#ifdef LITERAL_CMAKE_CXX_FLAGS_POISON
#error CMAKE_CXX_FLAGS leaked into the literal compiler command
#endif
#if !defined(__riscv_xlen) || __riscv_xlen != 32
#error selected compiler role is not RV32
#endif
#ifndef __riscv_float_abi_single
#error selected compiler role does not use the single-float ABI
#endif
#if ROLE_SEQUENCE != 2
#error ordered -D/-U source arguments were not preserved
#endif
static_assert(sizeof(void *) == 4, "selected CXX compiler must have 32-bit pointers");
static_assert(sizeof(ROLE_TEXT) == 12, "string macro with spaces was changed");
extern "C" int literal_gnu_role_cxx() { return ROLE_SEQUENCE; }
]=])

function(_literal_gnu_expect_elf object expected_class expected_float_abi label)
    execute_process(
        COMMAND "${_GNU_READELF}" -h "${object}"
        RESULT_VARIABLE _readelf_result
        OUTPUT_VARIABLE _readelf_header
        ERROR_VARIABLE _readelf_error
        TIMEOUT 30)
    if(NOT _readelf_result STREQUAL "0")
        message(FATAL_ERROR "${label}: GNU readelf failed\n${_readelf_error}")
    endif()

    set(_class_ok FALSE)
    set(_data_ok FALSE)
    set(_machine_ok FALSE)
    set(_abi_ok FALSE)
    string(REPLACE "\n" ";" _header_lines "${_readelf_header}")
    foreach(_line IN LISTS _header_lines)
        if(_line MATCHES "^[ \t]*Class:[ \t]+${expected_class}[ \t]*$")
            set(_class_ok TRUE)
        elseif(_line MATCHES "^[ \t]*Data:[ \t]+2's complement, little endian[ \t]*$")
            set(_data_ok TRUE)
        elseif(_line MATCHES "^[ \t]*Machine:[ \t]+RISC-V[ \t]*$")
            set(_machine_ok TRUE)
        elseif(expected_float_abi STREQUAL "single" AND
               _line MATCHES "^[ \t]*Flags:[ \t]+0x[0-9A-Fa-f]+,.*single-float ABI[ \t]*$")
            set(_abi_ok TRUE)
        elseif(expected_float_abi STREQUAL "double" AND
               _line MATCHES "^[ \t]*Flags:[ \t]+0x[0-9A-Fa-f]+,.*double-float ABI[ \t]*$")
            set(_abi_ok TRUE)
        endif()
    endforeach()
    if(NOT _class_ok OR NOT _data_ok OR NOT _machine_ok OR NOT _abi_ok)
        message(FATAL_ERROR
            "${label}: ELF header fields do not match ${expected_class}, little-endian RISC-V, ${expected_float_abi}-float ABI\n${_readelf_header}")
    endif()
endfunction()

function(_literal_gnu_verify_command_file command_file expected_sysroot)
    file(STRINGS "${command_file}" _command_lines)
    set(_ordered_arguments
        "-DROLE_SEQUENCE=1"
        "-UROLE_SEQUENCE"
        "-DROLE_SEQUENCE=2"
        "-DROLE_TEXT=\"hello world\"")
    set(_expected_arguments
        ${_ordered_arguments}
        -march=rv32imafc
        -mabi=ilp32f
        -mcmodel=medany
        -mno-strict-align
        "--sysroot=${expected_sysroot}")
    foreach(_expected IN LISTS _expected_arguments)
        set(_count 0)
        foreach(_line IN LISTS _command_lines)
            if(_line STREQUAL "    [==[${_expected}]==]")
                math(EXPR _count "${_count} + 1")
            endif()
        endforeach()
        if(NOT _count EQUAL 1)
            message(FATAL_ERROR
                "${command_file}: expected exactly one serialized argument '${_expected}', found ${_count}")
        endif()
    endforeach()

    list(FIND _command_lines "    [==[-DROLE_SEQUENCE=1]==]" _first_define)
    list(FIND _command_lines "    [==[-UROLE_SEQUENCE]==]" _undefine)
    list(FIND _command_lines "    [==[-DROLE_SEQUENCE=2]==]" _second_define)
    list(FIND _command_lines "    [==[-DROLE_TEXT=\"hello world\"]==]" _string_macro)
    if(_first_define LESS 0 OR _undefine LESS 0 OR _second_define LESS 0 OR
       _string_macro LESS 0 OR NOT _first_define LESS _undefine OR
       NOT _undefine LESS _second_define OR NOT _second_define LESS _string_macro)
        message(FATAL_ERROR "${command_file}: source-owned -D/-U/string macro order changed")
    endif()

    file(READ "${command_file}" _contents)
    if(_contents MATCHES "LITERAL_CMAKE_C_FLAGS_POISON|LITERAL_CMAKE_CXX_FLAGS_POISON")
        message(FATAL_ERROR "${command_file}: global CMake flags leaked into literal argv")
    endif()
endfunction()

function(_literal_gnu_configure mode build_dir conflict)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${build_dir}" -G Ninja
            "-DENGINE_DIR=${_engine_dir}"
            "-DLITERAL_ROLE_MODE=${mode}"
            "-DLITERAL_GNU_ABI_CONFLICT=${conflict}"
            "-DPROBE_GNU_CC=${_GNU_CC}"
            "-DPROBE_GNU_CXX=${_GNU_CXX}"
            "-DPROBE_GNU_AR=${_GNU_AR}"
            "-DPROBE_GNU_RANLIB=${_GNU_RANLIB}"
            "-DCMAKE_TOOLCHAIN_FILE=${_fixture}/Toolchain.cmake"
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr
        TIMEOUT 120)
    set(_configure_log "${_configure_stdout}\n${_configure_stderr}")
    set(_configure_result "${_configure_result}" PARENT_SCOPE)
    set(_configure_log "${_configure_log}" PARENT_SCOPE)
endfunction()

function(_literal_gnu_positive_mode mode)
    set(_build "${_test_root}/${mode}")
    _literal_gnu_configure("${mode}" "${_build}" FALSE)
    if(NOT _configure_result STREQUAL "0")
        message(FATAL_ERROR "GNU ${mode} compiler-role configure failed\n${_configure_log}")
    endif()

    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target literal-role-objects --parallel 2
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr
        TIMEOUT 120)
    if(NOT _build_result STREQUAL "0")
        message(FATAL_ERROR "GNU ${mode} literal object build failed\n${_build_stdout}${_build_stderr}")
    endif()

    set(_c_object "${_build}/declared-build/gen/literal/role-c.o")
    set(_cxx_object "${_build}/declared-build/gen/literal/role-cxx.o")
    foreach(_object IN ITEMS "${_c_object}" "${_cxx_object}")
        if(NOT EXISTS "${_object}" OR IS_SYMLINK "${_object}")
            message(FATAL_ERROR "GNU ${mode} fixture omitted literal object ${_object}")
        endif()
        _literal_gnu_expect_elf("${_object}" ELF32 single "GNU ${mode} object")
    endforeach()

    file(GLOB _commands "${_build}/CMakeFiles/aros-literal-object-*.cmake")
    list(LENGTH _commands _command_count)
    if(NOT _command_count EQUAL 2)
        message(FATAL_ERROR
            "GNU ${mode} fixture must emit exactly two serialized literal command files; got ${_command_count}")
    endif()
    foreach(_command IN LISTS _commands)
        _literal_gnu_verify_command_file(
            "${_command}" "${_build}/declared-build/SYS/Developer")
    endforeach()
endfunction()

file(MAKE_DIRECTORY "${_fixture}/counter-source" "${_test_root}/counter-output")
file(WRITE "${_fixture}/counter-source/counter.c" [=[
int literal_gnu_different_abi_counter(void) { return 0; }
]=])

_literal_gnu_positive_mode(consumer)
_literal_gnu_positive_mode(native-build)

# This adapter conflict must stop configuration before the runner can compile
# either source. The output path remains absent because no build is started.
set(_negative_build "${_test_root}/consumer-abi-conflict")
_literal_gnu_configure(consumer "${_negative_build}" TRUE)
string(REGEX REPLACE "[ \t\r\n]+" " " _negative_log "${_configure_log}")
string(FIND "${_negative_log}"
    "native GNU source ABI option conflicts with the validated target" _conflict_found)
if(_configure_result STREQUAL "0" OR _conflict_found LESS 0)
    message(FATAL_ERROR "GNU source -mabi=ilp32d conflict was not rejected by the adapter\n${_negative_log}")
endif()
if(EXISTS "${_negative_build}/declared-build/gen/literal/role-c.o" OR
   EXISTS "${_negative_build}/declared-build/gen/literal/role-cxx.o")
    message(FATAL_ERROR "GNU ABI conflict configure unexpectedly produced a literal object")
endif()

set(_counter_object "${_test_root}/counter-output/rv64-double.o")
execute_process(
    COMMAND "${_GNU_CC}"
        -march=rv64imafdc -mabi=lp64d -mcmodel=medany
        -c "${_fixture}/counter-source/counter.c"
        -o "${_counter_object}"
    RESULT_VARIABLE _counter_result
    OUTPUT_VARIABLE _counter_stdout
    ERROR_VARIABLE _counter_stderr
    TIMEOUT 60)
if(NOT _counter_result STREQUAL "0")
    message(FATAL_ERROR "Different-ABI GNU counter compile failed\n${_counter_stdout}${_counter_stderr}")
endif()
_literal_gnu_expect_elf("${_counter_object}" ELF64 double "GNU RV64 double-float counter object")

message(STATUS
    "Literal GNU compiler-role engine: consumer and native-build RV32 ELF/argv checks passed; RV64 counter discriminated")
