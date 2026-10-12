cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/LiteralCompilerRole.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
if(NOT DEFINED TEST_BINARY_DIR OR "${TEST_BINARY_DIR}" STREQUAL "")
    message(FATAL_ERROR "TEST_BINARY_DIR must name a fresh test root")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
find_program(_cmake NAMES cmake REQUIRED)

include("${_engine_dir}/NativeConsumerContract.cmake")
include("${_engine_dir}/LiteralCompilerRole.cmake")

function(_literal_role_set_synthetic_sdk_facts)
    # Unit facts exercise the role adapter only. They do not simulate or
    # attest a source-contract producer or release qualification.
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_ARCH "x86-64")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_CPU "")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_FPU "")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_CODE_MODEL "small")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_RED_ZONE "-mno-red-zone")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_PIC "-fno-pic")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_PIE "-fPIE")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_FLOAT_ABI "")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
        "aros-sdk-consumer-validation-v1")
endfunction()

if(DEFINED ROLE_PROBE)
    if(ROLE_PROBE STREQUAL "sdk-abi-conflict")
        _literal_role_set_synthetic_sdk_facts()
        set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_ARCH "armv7-a")
        set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_FLOAT_ABI "soft")
        _aros_literal_sdk_role_options(_options C "-mfloat-abi=hard")
    elseif(ROLE_PROBE STREQUAL "sdk-arch-conflict")
        _literal_role_set_synthetic_sdk_facts()
        _aros_literal_sdk_role_options(_options C "-march=wrong-arch")
    elseif(ROLE_PROBE STREQUAL "sdk-target-override")
        _literal_role_set_synthetic_sdk_facts()
        _aros_literal_sdk_role_options(_options C "--target=wrong-triple")
    elseif(ROLE_PROBE STREQUAL "sdk-width-override")
        _literal_role_set_synthetic_sdk_facts()
        _aros_literal_sdk_role_options(_options C "-m32")
    elseif(ROLE_PROBE STREQUAL "sdk-triple-mismatch")
        set(CMAKE_C_COMPILER_ID Clang)
        set(CMAKE_C_COMPILER_TARGET "i386-unknown-linux-gnu")
        _aros_literal_compiler_target_arguments(_options C "arm-unknown-aros")
    elseif(ROLE_PROBE STREQUAL "external-toolchain")
        set(CMAKE_C_COMPILER_ID Clang)
        set(CMAKE_C_COMPILER_EXTERNAL_TOOLCHAIN "/unvalidated/toolchain")
        _aros_literal_compiler_target_arguments(_options C "")
    elseif(ROLE_PROBE STREQUAL "selected-target-width-override")
        set(CMAKE_C_COMPILER_ID Clang)
        set(CMAKE_C_COMPILER_TARGET "i386-unknown-linux-gnu")
        _aros_literal_compiler_role_arguments(_options C "-m32" FALSE)
    elseif(ROLE_PROBE STREQUAL "gnu-abi-conflict")
        set(CMAKE_C_COMPILER_ID GNU)
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "-march=rv64imafdc;-mabi=lp64d;-mcmodel=medany;-mno-strict-align")
        _aros_literal_gnu_role_options(_options C "-mabi=ilp32")
    elseif(ROLE_PROBE STREQUAL "gnu-external-toolchain")
        set(CMAKE_C_COMPILER_ID GNU)
        set(CMAKE_C_COMPILER_EXTERNAL_TOOLCHAIN "/unvalidated/toolchain")
        _aros_literal_gnu_role_options(_options C "")
    elseif(ROLE_PROBE STREQUAL "gnu-source-duplicate")
        set(CMAKE_C_COMPILER_ID GNU)
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "-march=rv32imafc;-mabi=ilp32f;-mcmodel=medany;-mno-strict-align")
        _aros_literal_gnu_role_options(_options C "-mabi=ilp32f;-mabi=ilp32f")
    elseif(ROLE_PROBE STREQUAL "gnu-selected-duplicate")
        set(CMAKE_C_COMPILER_ID GNU)
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "-march=rv32imafc;-mabi=ilp32f;-mabi=ilp32f;-mcmodel=medany")
        _aros_literal_gnu_role_options(_options C "")
    elseif(ROLE_PROBE STREQUAL "external-sysroot")
        set(CMAKE_C_COMPILER_ID Clang)
        set(CMAKE_SYSROOT_COMPILE "/unvalidated/sysroot")
        _aros_literal_compiler_target_arguments(_options C "")
    elseif(ROLE_PROBE STREQUAL "native-endian-override")
        _aros_literal_sdk_role_options(_options C "-mbig-endian")
    elseif(ROLE_PROBE STREQUAL "native-sysroot-override" OR
           ROLE_PROBE STREQUAL "native-sysroot-duplicate")
        set(AROS_BUILD_DIR "${_test_binary_dir}")
        set(AROS_TARGET_SYSROOT "${AROS_BUILD_DIR}/SYS/Developer")
        if(ROLE_PROBE STREQUAL "native-sysroot-override")
            set(_source_arguments "--sysroot=${AROS_BUILD_DIR}/alternate")
        else()
            set(_source_arguments "--sysroot=${AROS_TARGET_SYSROOT};--sysroot;${AROS_TARGET_SYSROOT}")
        endif()
        _aros_literal_native_sysroot_arguments(_options "${_source_arguments}")
    elseif(ROLE_PROBE STREQUAL "selected-without-validation")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA "")
        _aros_literal_compiler_role_arguments(_options C "" TRUE)
    elseif(ROLE_PROBE STREQUAL "unknown-schema")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
            "aros-unknown-consumer-validation-v1")
        _aros_literal_native_consumer_compiler_role(_role)
    else()
        message(FATAL_ERROR "Unknown literal compiler-role probe")
    endif()
    message(FATAL_ERROR "Literal compiler-role probe unexpectedly succeeded")
endif()

_literal_role_set_synthetic_sdk_facts()
_aros_literal_native_consumer_compiler_role(_sdk_schema_role)
if(NOT _sdk_schema_role STREQUAL "sdk")
    message(FATAL_ERROR "Generic SDK schema did not select the SDK compiler role")
endif()
set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
    "aros-native-consumer-validation-v1")
_aros_literal_native_consumer_compiler_role(_gnu_schema_role)
if(NOT _gnu_schema_role STREQUAL "gnu")
    message(FATAL_ERROR "Legacy GNU consumer schema did not select the GNU compiler role")
endif()
set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
    "aros-sdk-consumer-validation-v1")
set(_sdk_source_arguments
    "-DREPEATED=first" "-march=x86-64" "-DVALUE=hello world" "-DREPEATED=second")
_aros_literal_sdk_role_options(_sdk_role_options C "${_sdk_source_arguments}")
foreach(_expected IN ITEMS -mno-red-zone -fno-pic -fPIE -mcmodel=small)
    if(NOT _expected IN_LIST _sdk_role_options)
        message(FATAL_ERROR "SDK role adapter omitted ${_expected}: ${_sdk_role_options}")
    endif()
endforeach()
if("-march=x86-64" IN_LIST _sdk_role_options)
    message(FATAL_ERROR "SDK role adapter duplicated an explicitly supplied ABI option")
endif()
if(NOT "-nostdlibinc" IN_LIST _sdk_role_options)
    message(FATAL_ERROR "SDK literal role must retain only explicit C-library header roots")
endif()

set(AROS_BUILD_DIR "${_test_binary_dir}")
set(AROS_TARGET_SYSROOT "${AROS_BUILD_DIR}/SYS/Developer")
_aros_literal_native_sysroot_arguments(_sysroot_options "-DKEEP=1")
if(NOT _sysroot_options STREQUAL "--sysroot=${AROS_TARGET_SYSROOT}")
    message(FATAL_ERROR "Native direct driver omitted its selected Developer sysroot")
endif()
foreach(_explicit IN ITEMS "--sysroot=${AROS_TARGET_SYSROOT}" "--sysroot;${AROS_TARGET_SYSROOT}")
    _aros_literal_native_sysroot_arguments(_sysroot_options "${_explicit}")
    if(NOT _sysroot_options STREQUAL "")
        message(FATAL_ERROR "Native direct driver duplicated a selected source sysroot")
    endif()
endforeach()

set(CMAKE_C_COMPILER_ID Clang)
set(CMAKE_C_COMPILER_TARGET "i386-unknown-linux-gnu")
_aros_literal_compiler_target_arguments(_target_arguments C "i386-unknown-linux-gnu")
if(NOT _target_arguments STREQUAL "--target=i386-unknown-linux-gnu")
    message(FATAL_ERROR "CMake compiler target was not materialized: ${_target_arguments}")
endif()

set(CMAKE_C_COMPILER_ID GNU)
unset(CMAKE_C_COMPILER_TARGET)
set(AROS_GNU_TARGET_COMPILE_OPTIONS
    "-march=rv64imafdc;-mabi=lp64d;-mcmodel=medany;-mno-strict-align")
_aros_literal_gnu_role_options(_gnu_role_options C "-DKEEP=1")
foreach(_expected IN ITEMS
        -march=rv64imafdc -mabi=lp64d -mcmodel=medany -mno-strict-align)
    if(NOT _expected IN_LIST _gnu_role_options)
        message(FATAL_ERROR "GNU role adapter omitted ${_expected}: ${_gnu_role_options}")
    endif()
endforeach()

function(_literal_role_expect_failure probe expected_text)
    execute_process(
        COMMAND "${_cmake}" "-DENGINE_DIR=${_engine_dir}"
            "-DTEST_BINARY_DIR=${_test_binary_dir}" "-DROLE_PROBE=${probe}"
            -P "${CMAKE_CURRENT_LIST_FILE}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 30)
    if(_result STREQUAL "0")
        message(FATAL_ERROR "${probe}: expected a rejected role conflict")
    endif()
    set(_log "${_stdout}\n${_stderr}")
    string(REGEX REPLACE "[ \t\r\n]+" " " _log "${_log}")
    string(FIND "${_log}" "${expected_text}" _found)
    if(_found LESS 0)
        message(FATAL_ERROR "${probe}: expected '${expected_text}'\n${_log}")
    endif()
endfunction()

_literal_role_expect_failure(sdk-abi-conflict "exactly one selected -mfloat-abi=soft")
_literal_role_expect_failure(sdk-arch-conflict "exactly one selected -march=x86-64")
_literal_role_expect_failure(sdk-target-override "cannot override compiler target")
_literal_role_expect_failure(sdk-width-override "cannot override compiler target")
_literal_role_expect_failure(sdk-triple-mismatch "differs from the selected source triple")
_literal_role_expect_failure(external-toolchain "external compiler toolchains are not supported")
_literal_role_expect_failure(selected-target-width-override "cannot override compiler target")
_literal_role_expect_failure(gnu-abi-conflict "conflicts with the validated target")
_literal_role_expect_failure(gnu-external-toolchain "external compiler toolchains are not supported")
_literal_role_expect_failure(gnu-source-duplicate "duplicate source GNU ABI option")
_literal_role_expect_failure(gnu-selected-duplicate "duplicate validated GNU ABI option")
_literal_role_expect_failure(external-sysroot "external compiler sysroot metadata is not supported")
_literal_role_expect_failure(native-endian-override "undeclared endian or instruction-mode override")
_literal_role_expect_failure(native-sysroot-override "differs from the selected Developer tree")
_literal_role_expect_failure(native-sysroot-duplicate "at most one complete selected native sysroot argument")
_literal_role_expect_failure(selected-without-validation "selected native source has no validated compiler role")
_literal_role_expect_failure(unknown-schema "unsupported validated native consumer schema")

# Native SDK contracts require upstream Clang, not AppleClang. CI installs
# LLVM; locate the same prerequisite locally without impersonating its ID.
if(APPLE)
    find_program(_brew NAMES brew REQUIRED)
    execute_process(COMMAND "${_brew}" --prefix llvm
        RESULT_VARIABLE _llvm_result OUTPUT_VARIABLE _llvm_prefix ERROR_VARIABLE _llvm_error
        OUTPUT_STRIP_TRAILING_WHITESPACE TIMEOUT 30)
    if(NOT _llvm_result STREQUAL "0")
        message(FATAL_ERROR "Native SDK compiler-role probes require installed LLVM: ${_llvm_error}")
    endif()
    find_program(_clang NAMES clang HINTS "${_llvm_prefix}/bin" NO_DEFAULT_PATH REQUIRED)
    find_program(_clangxx NAMES clang++ HINTS "${_llvm_prefix}/bin" NO_DEFAULT_PATH REQUIRED)
else()
    find_program(_clang NAMES clang REQUIRED)
    find_program(_clangxx NAMES clang++ REQUIRED)
endif()
find_program(_file NAMES file REQUIRED)
file(REAL_PATH "${_clang}" _clang)
file(REAL_PATH "${_clangxx}" _clangxx)
file(MAKE_DIRECTORY "${_test_binary_dir}")
file(REAL_PATH "${_test_binary_dir}" _test_binary_dir)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_test_binary_dir}/literal-compiler-role-${_suffix}")
while(EXISTS "${_test_root}" OR IS_SYMLINK "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_test_binary_dir}/literal-compiler-role-${_suffix}")
endwhile()
set(_fixture "${_test_root}/fixture")
file(MAKE_DIRECTORY "${_fixture}")
file(WRITE "${_fixture}/Toolchain.cmake" [=[
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
set(CMAKE_C_COMPILER "${LITERAL_ROLE_CLANG}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER "${LITERAL_ROLE_CLANGXX}" CACHE FILEPATH "" FORCE)
set(CMAKE_ASM_COMPILER "${LITERAL_ROLE_CLANG}" CACHE FILEPATH "" FORCE)
set(CMAKE_C_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
set(CMAKE_CXX_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
set(CMAKE_ASM_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
]=])
file(WRITE "${_fixture}/CMakeLists.txt" [==[
cmake_minimum_required(VERSION 3.22)
project(LiteralCompilerRoleFixture LANGUAGES C CXX ASM)
if(LITERAL_NATIVE_SDK)
    include("${ENGINE_DIR}/NativeConsumerContract.cmake")
endif()
include("${ENGINE_DIR}/LiteralObjects.cmake")
set(CMAKE_C_FLAGS "-DLITERAL_CMAKE_FLAGS_MUST_NOT_LEAK=1")
set(CMAKE_CXX_FLAGS "-DLITERAL_CXX_CMAKE_FLAGS_MUST_NOT_LEAK=1")
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/declared-source")
set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}/declared-build")
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/src" "${AROS_BUILD_DIR}")
if(LITERAL_NATIVE_SDK)
    # These isolated source-current facts exercise the SDK compiler adapter,
    # not Rust source sealing, producer identity, or release qualification.
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(AROS_TOOLCHAIN llvm)
    set(AROS_TARGET_CPU i386)
    set(AROS_TARGET_TRIPLE i386-unknown-linux-gnu)
    set(AROS_TARGET_SYSROOT "${AROS_BUILD_DIR}/SYS/Developer")
    set(AROS_GENINC_DIR "${AROS_BUILD_DIR}/GENINCDIR")
    set(AROS_SDK_INCLUDE_DIR "${AROS_TARGET_SYSROOT}/include")
    file(MAKE_DIRECTORY "${AROS_GENINC_DIR}" "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA
        "aros-sdk-consumer-validation-v1")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_POINTER_WIDTH 32)
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_ARCH i386)
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_CPU "")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_FPU "")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_CODE_MODEL "")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_RED_ZONE "-mno-red-zone")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_PIC "-fno-pic")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_COMPILER_PIE "-fno-pie")
    set_property(GLOBAL PROPERTY AROS_SDK_CONSUMER_VALIDATED_FLOAT_ABI "")
    set(_abi_flags "-march=i386 -mno-red-zone -fno-pic -fno-pie")
    foreach(_language IN ITEMS C CXX ASM)
        string(APPEND CMAKE_${_language}_FLAGS " ${_abi_flags}")
    endforeach()
    function(_aros_native_consumer_current out_source out_path out_digest)
        set(${out_source} "${AROS_SOURCE_DIR}" PARENT_SCOPE)
        set(${out_path} "component-fixture" PARENT_SCOPE)
        set(${out_digest} "component-fixture" PARENT_SCOPE)
    endfunction()
endif()
file(WRITE "${AROS_SOURCE_DIR}/src/main.c" [=[
#ifdef LITERAL_CMAKE_FLAGS_MUST_NOT_LEAK
#error CMAKE_C_FLAGS leaked into the literal compiler role
#endif
#ifndef __AVX__
#error source-owned ISA option was omitted
#endif
_Static_assert(sizeof(void *) == 4, "selected C compiler target must be 32-bit");
int literal_c_role(void) { return 0; }
]=])
file(WRITE "${AROS_SOURCE_DIR}/src/helper.cpp" [=[
#ifdef LITERAL_CXX_CMAKE_FLAGS_MUST_NOT_LEAK
#error CMAKE_CXX_FLAGS leaked into the literal compiler role
#endif
#ifndef __AVX__
#error source-owned ISA option was omitted
#endif
static_assert(sizeof(void *) == 4, "selected CXX compiler target must be 32-bit");
extern "C" int literal_cxx_role() { return 0; }
]=])
set(_c_object "${AROS_BUILD_DIR}/gen/literal/c-role.o")
set(_cxx_object "${AROS_BUILD_DIR}/gen/literal/cxx-role.o")
set(_source_extra_arguments)
if(LITERAL_NATIVE_COUNTER STREQUAL "sysroot")
    list(APPEND _source_extra_arguments "--sysroot=${AROS_BUILD_DIR}/alternate")
elseif(LITERAL_NATIVE_COUNTER STREQUAL "endian")
    list(APPEND _source_extra_arguments -mbig-endian)
elseif(LITERAL_NATIVE_COUNTER STREQUAL "arch")
    list(APPEND _source_extra_arguments -march=x86-64)
endif()
aros_compile_literal_object(NAME role-c SOURCE "${AROS_SOURCE_DIR}/src/main.c"
    OUTPUT "${_c_object}" LANGUAGE C
    ARGUMENTS "-DREPEATED=first" "-DVALUE=hello world" "-DREPEATED=second" -mavx ${_source_extra_arguments})
aros_compile_literal_object(NAME role-cxx SOURCE "${AROS_SOURCE_DIR}/src/helper.cpp"
    OUTPUT "${_cxx_object}" LANGUAGE CXX
    ARGUMENTS "-DREPEATED=first" "-DVALUE=hello world" "-DREPEATED=second" -mavx ${_source_extra_arguments})
aros_literal_object_group(NAME role-objects OBJECTS "${_c_object}" "${_cxx_object}")
]==])

function(_literal_role_compile_fixture mode native_sdk)
set(_build "${_test_root}/${mode}")
execute_process(
    COMMAND "${_cmake}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DENGINE_DIR=${_engine_dir}"
        "-DLITERAL_ROLE_CLANG=${_clang}"
        "-DLITERAL_ROLE_CLANGXX=${_clangxx}"
        "-DCMAKE_TOOLCHAIN_FILE=${_fixture}/Toolchain.cmake"
        "-DLITERAL_NATIVE_SDK=${native_sdk}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT 90)
if(NOT _configure_result STREQUAL "0")
    message(FATAL_ERROR
        "Clang compiler-role fixture configure failed\n${_configure_stdout}${_configure_stderr}")
endif()
execute_process(
    COMMAND "${_cmake}" --build "${_build}" --target role-objects --parallel 2
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr
    TIMEOUT 90)
if(NOT _build_result STREQUAL "0")
    message(FATAL_ERROR
        "Clang compiler-role fixture build failed\n${_build_stdout}${_build_stderr}")
endif()

foreach(_object IN ITEMS
        "${_build}/declared-build/gen/literal/c-role.o"
        "${_build}/declared-build/gen/literal/cxx-role.o")
    if(NOT EXISTS "${_object}" OR IS_SYMLINK "${_object}")
        message(FATAL_ERROR "Clang compiler-role fixture omitted ${_object}")
    endif()
    execute_process(
        COMMAND "${_file}" "${_object}"
        RESULT_VARIABLE _file_result
        OUTPUT_VARIABLE _description
        ERROR_VARIABLE _file_error
        TIMEOUT 30)
    if(NOT _file_result STREQUAL "0" OR
       NOT _description MATCHES "ELF" OR
       NOT _description MATCHES "32-bit" OR
       NOT _description MATCHES "80386" OR
       _description MATCHES "Mach-O")
        message(FATAL_ERROR
            "Clang compiler-role object has the wrong format, width, or architecture: ${_description}${_file_error}")
    endif()
endforeach()

file(GLOB _commands "${_build}/CMakeFiles/aros-literal-object-*.cmake")
list(LENGTH _commands _command_count)
if(NOT _command_count EQUAL 2)
    message(FATAL_ERROR "Clang compiler-role fixture must emit exactly two literal command files")
endif()
foreach(_command IN LISTS _commands)
    file(READ "${_command}" _contents)
    string(FIND "${_contents}" "--target=i386-unknown-linux-gnu]==]" _target_position)
    string(FIND "${_contents}" "-DREPEATED=first]==]" _first_position)
    string(FIND "${_contents}" "-DREPEATED=second]==]" _second_position)
    string(FIND "${_contents}" "-DVALUE=hello world]==]" _string_macro_position)
    string(FIND "${_contents}" "-mavx]==]" _avx_position)
    if(_target_position LESS 0 OR _first_position LESS 0 OR
       _second_position LESS 0 OR _string_macro_position LESS 0 OR _avx_position LESS 0 OR
       _first_position GREATER_EQUAL _string_macro_position OR
       _string_macro_position GREATER_EQUAL _second_position OR
       _second_position GREATER_EQUAL _avx_position OR
       _avx_position GREATER_EQUAL _target_position OR
       _contents MATCHES "LITERAL_CMAKE_FLAGS_MUST_NOT_LEAK|LITERAL_CXX_CMAKE_FLAGS_MUST_NOT_LEAK")
        message(FATAL_ERROR
            "Literal compiler command lost the CMake target or changed source argv: ${_command}")
    endif()
endforeach()

if(native_sdk)
    foreach(_command IN LISTS _commands)
        file(READ "${_command}" _contents)
        foreach(_abi IN ITEMS -march=i386 -mno-red-zone -fno-pic -fno-pie
                -nostdlibinc
                "--sysroot=${_build}/declared-build/SYS/Developer")
            string(FIND "${_contents}" "${_abi}]==]" _found)
            if(_found LESS 0)
                message(FATAL_ERROR "Native SDK literal command omitted ${_abi}")
            endif()
        endforeach()
    endforeach()
endif()
endfunction()

_literal_role_compile_fixture(generic FALSE)
_literal_role_compile_fixture(native-sdk TRUE)

foreach(_counter IN ITEMS sysroot endian arch)
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture}" -B "${_test_root}/native-reject-${_counter}" -G Ninja
            "-DENGINE_DIR=${_engine_dir}"
            "-DLITERAL_ROLE_CLANG=${_clang}" "-DLITERAL_ROLE_CLANGXX=${_clangxx}"
            "-DCMAKE_TOOLCHAIN_FILE=${_fixture}/Toolchain.cmake"
            -DLITERAL_NATIVE_SDK=ON "-DLITERAL_NATIVE_COUNTER=${_counter}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 90)
    if(_counter STREQUAL "sysroot")
        set(_expected "differs from the selected Developer tree")
    elseif(_counter STREQUAL "endian")
        set(_expected "undeclared endian or instruction-mode override")
    else()
        set(_expected "exactly one selected -march=i386")
    endif()
    string(REGEX REPLACE "[ \t\r\n]+" " " _log "${_stdout}\n${_stderr}")
    string(FIND "${_log}" "${_expected}" _found)
    if(_result STREQUAL "0" OR _found LESS 0)
        message(FATAL_ERROR "Native SDK ${_counter} counter-probe failed\n${_log}")
    endif()
endforeach()

# The object-format measurement must also distinguish a deliberately different
# target, rather than merely accepting whatever the host compiler emits.
file(WRITE "${_fixture}/counter.c" "int different_target(void) { return 0; }\n")
execute_process(COMMAND "${_clang}" --target=x86_64-unknown-linux-gnu
    -c "${_fixture}/counter.c" -o "${_test_root}/counter.o"
    RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 30)
if(NOT _result STREQUAL "0")
    message(FATAL_ERROR "Different-target measurement probe failed\n${_stdout}${_stderr}")
endif()
execute_process(COMMAND "${_file}" "${_test_root}/counter.o"
    RESULT_VARIABLE _result OUTPUT_VARIABLE _description ERROR_VARIABLE _stderr TIMEOUT 30)
if(NOT _result STREQUAL "0" OR NOT _description MATCHES "ELF" OR
   NOT _description MATCHES "64-bit" OR _description MATCHES "32-bit")
    message(FATAL_ERROR "Object-format measurement failed its different-target counter-probe: ${_description}${_stderr}")
endif()

message(STATUS "Literal compiler-role engine: schema probes and generic/native-SDK C/CXX Clang target checks passed")
