cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/LiteralObjects.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
if(NOT DEFINED TEST_BINARY_DIR OR "${TEST_BINARY_DIR}" STREQUAL "")
    message(FATAL_ERROR "TEST_BINARY_DIR must name a fresh test root")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
find_program(_cmake NAMES cmake REQUIRED)
if(APPLE)
    find_program(_brew NAMES brew REQUIRED)
    execute_process(COMMAND "${_brew}" --prefix llvm
        RESULT_VARIABLE _llvm_result
        OUTPUT_VARIABLE _llvm_prefix
        ERROR_VARIABLE _llvm_error
        OUTPUT_STRIP_TRAILING_WHITESPACE
        TIMEOUT 30)
    if(NOT _llvm_result STREQUAL "0")
        message(FATAL_ERROR "Literal dependency tests require installed LLVM: ${_llvm_error}")
    endif()
    find_program(_clang NAMES clang HINTS "${_llvm_prefix}/bin"
        NO_DEFAULT_PATH REQUIRED)
    find_program(_clangxx NAMES clang++ HINTS "${_llvm_prefix}/bin"
        NO_DEFAULT_PATH REQUIRED)
else()
    find_program(_clang NAMES clang REQUIRED)
    find_program(_clangxx NAMES clang++ REQUIRED)
endif()
file(REAL_PATH "${_clang}" _clang)
file(REAL_PATH "${_clangxx}" _clangxx)

file(MAKE_DIRECTORY "${_test_binary_dir}")
file(REAL_PATH "${_test_binary_dir}" _test_binary_dir)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_test_binary_dir}/literal-dependencies-${_suffix}")
while(EXISTS "${_test_root}" OR IS_SYMLINK "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_test_binary_dir}/literal-dependencies-${_suffix}")
endwhile()
file(MAKE_DIRECTORY "${_test_root}/fixture")
file(REAL_PATH "${_test_root}" _test_root)
file(WRITE "${_test_root}/fixture/NativeToolchain.cmake" [=[
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
set(CMAKE_C_COMPILER "${DEPENDENCY_CLANG}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER "${DEPENDENCY_CLANGXX}" CACHE FILEPATH "" FORCE)
set(CMAKE_ASM_COMPILER "${DEPENDENCY_CLANG}" CACHE FILEPATH "" FORCE)
set(CMAKE_C_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
set(CMAKE_CXX_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
set(CMAKE_ASM_COMPILER_TARGET "i386-unknown-linux-gnu" CACHE STRING "" FORCE)
]=])

file(WRITE "${_test_root}/fixture/CMakeLists.txt" [=[
cmake_minimum_required(VERSION 3.22)
project(LiteralDependenciesFixture LANGUAGES C CXX)
file(REAL_PATH "${CMAKE_C_COMPILER}" _real_c_compiler)
file(REAL_PATH "${CMAKE_CXX_COMPILER}" _real_cxx_compiler)
set(CMAKE_C_COMPILER "${_real_c_compiler}")
set(CMAKE_CXX_COMPILER "${_real_cxx_compiler}")
include("${ENGINE_DIR}/LiteralObjects.cmake")

set(AROS_SOURCE_DIR "${CMAKE_CURRENT_BINARY_DIR}/source")
set(AROS_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}/declared-build")
file(MAKE_DIRECTORY
    "${AROS_SOURCE_DIR}/c" "${AROS_SOURCE_DIR}/cxx"
    "${AROS_BUILD_DIR}/gen/c" "${AROS_BUILD_DIR}/gen/cxx")
file(WRITE "${AROS_SOURCE_DIR}/c/main.c"
    "#if DEPENDENCY_VARIANT\n#include \"dependency-only.h\"\n#else\n#include \"object-only.h\"\n#endif\nint literal_c(void) { return VALUE; }\n")
file(WRITE "${AROS_SOURCE_DIR}/c/dependency-only.h" "#define VALUE 7\n")
file(WRITE "${AROS_SOURCE_DIR}/c/object-only.h" "#define VALUE 11\n")
file(WRITE "${AROS_SOURCE_DIR}/cxx/main.cpp"
    "#if DEPENDENCY_VARIANT\n#include \"dependency-only.h\"\n#else\n#include \"object-only.h\"\n#endif\nint literal_cxx() { return VALUE; }\n")
file(WRITE "${AROS_SOURCE_DIR}/cxx/dependency-only.h" "#define VALUE 13\n")
file(WRITE "${AROS_SOURCE_DIR}/cxx/object-only.h" "#define VALUE 17\n")

set(_c_object "${AROS_BUILD_DIR}/gen/c/main.o")
set(_c_dependencies "${AROS_BUILD_DIR}/gen/c/main.d")
set(_cxx_object "${AROS_BUILD_DIR}/gen/cxx/main.o")
set(_cxx_dependencies "${AROS_BUILD_DIR}/gen/cxx/main.d")

if(DEPENDENCY_CASE STREQUAL "bad-pair")
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        OBJECT_OUTPUT "${AROS_BUILD_DIR}/gen/c/other.o"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1)
elseif(DEPENDENCY_CASE STREQUAL "outside-output")
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${AROS_BUILD_DIR}/elsewhere/main.d"
        OBJECT_OUTPUT "${_c_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1)
elseif(DEPENDENCY_CASE STREQUAL "outside-target")
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        OBJECT_OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/main.o"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1)
elseif(DEPENDENCY_CASE STREQUAL "output-symlink")
    file(WRITE "${AROS_BUILD_DIR}/gen/c/decoy.d" "decoy\n")
    file(CREATE_LINK "${AROS_BUILD_DIR}/gen/c/decoy.d" "${_c_dependencies}" SYMBOLIC)
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        OBJECT_OUTPUT "${_c_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1)
elseif(DEPENDENCY_CASE STREQUAL "missing-target")
    aros_compile_literal_object(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        LANGUAGE C OUTPUT_MODE DEPENDENCIES ARGUMENTS -DDEPENDENCY_VARIANT=1)
elseif(DEPENDENCY_CASE STREQUAL "private-collision-object-first")
    aros_compile_literal_object(NAME literal-object-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_object}"
        LANGUAGE C)
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_object}.d"
        OBJECT_OUTPUT "${_c_object}.o"
        LANGUAGE C)
elseif(DEPENDENCY_CASE STREQUAL "private-collision-dependency-first")
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_object}.d"
        OBJECT_OUTPUT "${_c_object}.o"
        LANGUAGE C)
    aros_compile_literal_object(NAME literal-object-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_object}"
        LANGUAGE C)
elseif(DEPENDENCY_CASE STREQUAL "dot-basename")
    aros_generate_literal_dependencies(NAME literal-dot-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${AROS_BUILD_DIR}/gen/c/unique.name.d"
        OBJECT_OUTPUT "${AROS_BUILD_DIR}/gen/c/unique.name.o"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(DEPENDENCY_CASE STREQUAL "native-includes")
    set(AROS_NATIVE_BUILD_CONTRACT ON)
    set(AROS_GENINC_DIR "${AROS_BUILD_DIR}/GENINCDIR")
    set(AROS_SDK_INCLUDE_DIR "${AROS_BUILD_DIR}/SDK/include")
    file(MAKE_DIRECTORY "${AROS_GENINC_DIR}" "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    function(_aros_native_source_selection_validated out_validated)
        set(${out_validated} TRUE PARENT_SCOPE)
    endfunction()
    function(_aros_literal_compiler_role_arguments out_arguments language
            source_arguments native_source_selected)
        # This fixture isolates the native header-root append. ABI and sysroot
        # validation is covered by LiteralCompilerRoleTest.
        set(${out_arguments} "" PARENT_SCOPE)
    endfunction()
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        OBJECT_OUTPUT "${_c_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(DEPENDENCY_CASE STREQUAL "native-role" OR
       DEPENDENCY_CASE STREQUAL "native-role-conflict")
    include("${ENGINE_DIR}/NativeConsumerContract.cmake")
    enable_language(ASM)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(AROS_TOOLCHAIN llvm)
    set(AROS_TARGET_CPU i386)
    set(AROS_TARGET_TRIPLE i386-unknown-linux-gnu)
    set(AROS_TARGET_SYSROOT "${AROS_BUILD_DIR}/SYS/Developer")
    set(AROS_GENINC_DIR "${AROS_BUILD_DIR}/GENINCDIR")
    set(AROS_SDK_INCLUDE_DIR "${AROS_TARGET_SYSROOT}/include")
    file(MAKE_DIRECTORY
        "${AROS_TARGET_SYSROOT}"
        "${AROS_GENINC_DIR}"
        "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    file(WRITE "${AROS_GENINC_DIR}/arch-selection.h" "#define ARCH_SELECTION 32\n")
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
    foreach(_language IN ITEMS C CXX ASM)
        string(APPEND CMAKE_${_language}_FLAGS
            " -march=i386 -mno-red-zone -fno-pic -fno-pie")
    endforeach()
    function(_aros_native_consumer_current out_source out_path out_digest)
        set(${out_source} "${AROS_SOURCE_DIR}" PARENT_SCOPE)
        set(${out_path} "component-fixture" PARENT_SCOPE)
        set(${out_digest} "component-fixture" PARENT_SCOPE)
    endfunction()
    file(WRITE "${AROS_SOURCE_DIR}/c/native.c"
        "#if defined(__i386__)\n#include <arch-selection.h>\n#else\n#include <wrong-arch.h>\n#endif\nint native_arch(void) { return ARCH_SELECTION; }\n")
    if(DEPENDENCY_CASE STREQUAL "native-role-conflict")
        set(_role_arguments -m64)
    else()
        set(_role_arguments)
    endif()
    if(_role_arguments)
        aros_generate_literal_dependencies(NAME literal-native-dependencies
            SOURCE "${AROS_SOURCE_DIR}/c/native.c"
            OUTPUT "${_c_dependencies}"
            OBJECT_OUTPUT "${_c_object}"
            LANGUAGE C ARGUMENTS ${_role_arguments})
    else()
        aros_generate_literal_dependencies(NAME literal-native-dependencies
            SOURCE "${AROS_SOURCE_DIR}/c/native.c"
            OUTPUT "${_c_dependencies}"
            OBJECT_OUTPUT "${_c_object}"
            LANGUAGE C)
    endif()
else()
    aros_compile_literal_object(NAME literal-object-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=0 -I "${AROS_SOURCE_DIR}/c")
    aros_generate_literal_dependencies(NAME literal-dependencies-c
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_c_dependencies}"
        OBJECT_OUTPUT "${_c_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_VARIANT=1 -I "${AROS_SOURCE_DIR}/c")

    aros_compile_literal_object(NAME literal-object-cxx
        SOURCE "${AROS_SOURCE_DIR}/cxx/main.cpp"
        OUTPUT "${_cxx_object}"
        LANGUAGE CXX ARGUMENTS -DDEPENDENCY_VARIANT=0 -I "${AROS_SOURCE_DIR}/cxx")
    aros_generate_literal_dependencies(NAME literal-dependencies-cxx
        SOURCE "${AROS_SOURCE_DIR}/cxx/main.cpp"
        OUTPUT "${_cxx_dependencies}"
        OBJECT_OUTPUT "${_cxx_object}"
        LANGUAGE CXX ARGUMENTS -DDEPENDENCY_VARIANT=1 -I "${AROS_SOURCE_DIR}/cxx")

    foreach(_dependency_output IN ITEMS "${_c_dependencies}" "${_cxx_dependencies}")
        string(TOLOWER "${_dependency_output}" _folded_dependency_output)
        string(SHA256 _dependency_key "${_folded_dependency_output}")
        get_property(_dependency_owner GLOBAL PROPERTY
            "AROS_LITERAL_DEPENDENCY_FILE_${_dependency_key}")
        get_property(_object_owner GLOBAL PROPERTY
            "AROS_LITERAL_OBJECT_${_dependency_key}")
        if(NOT _dependency_owner OR _object_owner)
            message(FATAL_ERROR
                "dependency outputs must have a dependency owner and no object provider")
        endif()
    endforeach()
endif()
]=])

function(_literal_configure case_name should_succeed expected_message)
    string(RANDOM LENGTH 8 ALPHABET 0123456789abcdef _case_suffix)
    if(case_name STREQUAL "special-path")
        set(_build "${_test_root}/special space # path-${_case_suffix}")
    else()
        set(_build "${_test_root}/${case_name}-${_case_suffix}")
    endif()
    set(_configure_command
        "${_cmake}" -S "${_test_root}/fixture" -B "${_build}" -G Ninja
        "-DENGINE_DIR=${_engine_dir}"
        "-DDEPENDENCY_CASE=${case_name}"
        "-DCMAKE_C_COMPILER=${_clang}"
        "-DCMAKE_CXX_COMPILER=${_clangxx}")
    if(case_name STREQUAL "native-role" OR
       case_name STREQUAL "native-role-conflict")
        list(APPEND _configure_command
            "-DDEPENDENCY_CLANG=${_clang}"
            "-DDEPENDENCY_CLANGXX=${_clangxx}"
            "-DCMAKE_TOOLCHAIN_FILE=${_test_root}/fixture/NativeToolchain.cmake")
    endif()
    execute_process(
        COMMAND ${_configure_command}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(should_succeed)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${case_name}: configure failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0")
            message(FATAL_ERROR "${case_name}: configure unexpectedly succeeded")
        endif()
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "${case_name}: expected '${expected_message}'\n${_log}")
        endif()
        message(STATUS "${case_name}: rejected (${expected_message})")
    endif()
    set(_LITERAL_TEST_BUILD "${_build}" PARENT_SCOPE)
endfunction()

function(_literal_build build_dir target label should_succeed expected_message)
    execute_process(
        COMMAND "${_cmake}" --build "${build_dir}" --target "${target}" --parallel 2
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(should_succeed)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${label}: build failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0")
            message(FATAL_ERROR "${label}: build unexpectedly succeeded\n${_log}")
        endif()
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "${label}: expected '${expected_message}'\n${_log}")
        endif()
    endif()
    set(_LITERAL_BUILD_LOG "${_log}" PARENT_SCOPE)
endfunction()

function(_literal_assert_no_temporary path label)
    file(GLOB _temporaries "${path}.literal-*.tmp")
    if(_temporaries)
        message(FATAL_ERROR "${label}: temporary dependency outputs remain: ${_temporaries}")
    endif()
endfunction()

_literal_configure(bad-pair FALSE "dependency output must be a .d sibling")
_literal_configure(outside-output FALSE "output must be one .d below AROS_BUILD_DIR/gen")
_literal_configure(outside-target FALSE "dependency output must be a .d sibling")
_literal_configure(output-symlink FALSE "symlink output path refused")
_literal_configure(missing-target FALSE "DEPENDENCIES requires DEPENDENCY_TARGET_OUTPUT")
_literal_configure(private-collision-object-first FALSE
    "output already owned by literal-object-c")
_literal_configure(private-collision-dependency-first FALSE
    "private dependency output already owned")
_literal_configure(dot-basename TRUE "")
_literal_build("${_LITERAL_TEST_BUILD}" literal-dot-dependencies
    "dotted dependency basename" TRUE "")
if(NOT EXISTS "${_LITERAL_TEST_BUILD}/declared-build/gen/c/unique.name.d")
    message(FATAL_ERROR "dotted dependency basename was rejected or not generated")
endif()

_literal_configure(native-role-conflict FALSE
    "native source arguments cannot override")
_literal_configure(native-role TRUE "")
set(_native_role_build "${_LITERAL_TEST_BUILD}")
_literal_build("${_native_role_build}" literal-native-dependencies
    "selected Clang dependency role" TRUE "")
set(_native_role_depfile
    "${_native_role_build}/declared-build/gen/c/main.d")
if(EXISTS "${_native_role_build}/declared-build/gen/c/main.o")
    message(FATAL_ERROR "native dependency mode emitted its .o target")
endif()
file(READ "${_native_role_depfile}" _native_role_dependency_contents)
if(NOT _native_role_dependency_contents MATCHES "main[.]o:" OR
   NOT _native_role_dependency_contents MATCHES "arch-selection[.]h" OR
   _native_role_dependency_contents MATCHES "wrong-arch[.]h")
    message(FATAL_ERROR
        "selected Clang i386 dependency command chose the wrong architecture header\n${_native_role_dependency_contents}")
endif()
string(SHA256 _native_role_key "${_native_role_depfile}")
set(_native_role_command
    "${_native_role_build}/CMakeFiles/aros-literal-object-${_native_role_key}.cmake")
file(READ "${_native_role_command}" _native_role_command_contents)
foreach(_role_argument IN ITEMS
        "--target=i386-unknown-linux-gnu"
        "-march=i386"
        "--sysroot=${_native_role_build}/declared-build/SYS/Developer"
        "-I${_native_role_build}/declared-build/GENINCDIR")
    string(FIND "${_native_role_command_contents}" "${_role_argument}]==]" _role_argument_found)
    if(_role_argument_found LESS 0)
        message(FATAL_ERROR "selected Clang dependency command omitted ${_role_argument}")
    endif()
endforeach()

_literal_configure(success TRUE "")
set(_build "${_LITERAL_TEST_BUILD}")
set(_build_root "${_build}/declared-build")
set(_c_source "${_build}/source/c/main.c")
set(_c_object "${_build_root}/gen/c/main.o")
set(_c_depfile "${_build_root}/gen/c/main.d")
set(_c_object_depfile "${_c_object}.d")
set(_cxx_object "${_build_root}/gen/cxx/main.o")
set(_cxx_depfile "${_build_root}/gen/cxx/main.d")

_literal_build("${_build}" literal-dependencies-c "C dependency mode" TRUE "")
if(EXISTS "${_c_object}" OR EXISTS "${_c_object_depfile}")
    message(FATAL_ERROR "C dependency mode emitted an object or private object depfile")
endif()
if(NOT EXISTS "${_c_depfile}" OR IS_SYMLINK "${_c_depfile}")
    message(FATAL_ERROR "C dependency mode omitted a regular .d output")
endif()
file(READ "${_c_depfile}" _c_dependency_contents)
if(NOT _c_dependency_contents MATCHES "main[.]o:" OR
   NOT _c_dependency_contents MATCHES "dependency-only[.]h" OR
   _c_dependency_contents MATCHES "object-only[.]h")
    message(FATAL_ERROR "C dependency command did not preserve its separate argv or -MQ target\n${_c_dependency_contents}")
endif()
_literal_assert_no_temporary("${_c_depfile}" "C dependency success")

_literal_build("${_build}" literal-dependencies-cxx "CXX dependency mode" TRUE "")
if(EXISTS "${_cxx_object}" OR NOT EXISTS "${_cxx_depfile}")
    message(FATAL_ERROR "CXX dependency mode emitted an object or omitted its .d")
endif()
file(READ "${_cxx_depfile}" _cxx_dependency_contents)
if(NOT _cxx_dependency_contents MATCHES "main[.]o:" OR
   NOT _cxx_dependency_contents MATCHES "dependency-only[.]h" OR
   _cxx_dependency_contents MATCHES "object-only[.]h")
    message(FATAL_ERROR "CXX dependency command did not preserve its separate argv or -MQ target\n${_cxx_dependency_contents}")
endif()

_literal_configure(special-path TRUE "")
set(_special_build "${_LITERAL_TEST_BUILD}")
set(_special_build_root "${_special_build}/declared-build")
set(_special_object "${_special_build_root}/gen/c/main.o")
set(_special_depfile "${_special_build_root}/gen/c/main.d")
_literal_build("${_special_build}" literal-dependencies-c
    "dependency target escaping with special parent path" TRUE "")
file(READ "${_special_depfile}" _special_dependency_contents)
set(_escaped_space [=[\ ]=])
set(_escaped_hash [=[\#]=])
foreach(_escaped IN ITEMS "${_escaped_space}" "${_escaped_hash}")
    string(FIND "${_special_dependency_contents}" "${_escaped}" _escaped_found)
    if(_escaped_found LESS 0)
        message(FATAL_ERROR
            "-MQ did not escape the special path character '${_escaped}' in the source-declared .d\n${_special_dependency_contents}")
    endif()
endforeach()
_literal_build("${_special_build}" literal-object-c
    "object private depfile with special parent path" TRUE "")
set(_special_object_depfile "${_special_object}.d")
file(READ "${_special_object_depfile}" _special_object_dependency_contents)
foreach(_escaped IN ITEMS "${_escaped_space}" "${_escaped_hash}")
    string(FIND "${_special_object_dependency_contents}" "${_escaped}" _escaped_found)
    if(_escaped_found LESS 0)
        message(FATAL_ERROR
            "-MQ did not escape the special path character '${_escaped}' in the object private depfile\n${_special_object_dependency_contents}")
    endif()
endforeach()
file(SHA256 "${_special_object}" _special_object_before_header_change)
file(WRITE "${_special_build}/source/c/object-only.h" "#define VALUE 29\n")
_literal_build("${_special_build}" literal-object-c
    "Ninja rebuild from escaped private depfile" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object")
    message(FATAL_ERROR
        "Ninja did not rebuild the object after reading the escaped private depfile\n${_LITERAL_BUILD_LOG}")
endif()
file(SHA256 "${_special_object}" _special_object_after_header_change)
if(_special_object_before_header_change STREQUAL _special_object_after_header_change)
    message(FATAL_ERROR "special-path header edit did not change the rebuilt object")
endif()

_literal_build("${_build}" literal-object-c "C object mode" TRUE "")
if(NOT EXISTS "${_c_object}" OR NOT EXISTS "${_c_object_depfile}")
    message(FATAL_ERROR "ordinary C object mode lost its object or private -MD depfile")
endif()
file(READ "${_c_object_depfile}" _c_object_dependency_contents)
if(NOT _c_object_dependency_contents MATCHES "object-only[.]h" OR
   _c_object_dependency_contents MATCHES "dependency-only[.]h")
    message(FATAL_ERROR "ordinary object private -MD semantics changed\n${_c_object_dependency_contents}")
endif()
file(SHA256 "${_c_object}" _object_before_dependency_header_change)
file(WRITE "${_build}/source/c/dependency-only.h" "#define VALUE 23\n")
_literal_build("${_build}" literal-object-c "object isolation after dependency header edit" TRUE "")
if("${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object")
    message(FATAL_ERROR "dependency-only header edit rebuilt the ordinary object\n${_LITERAL_BUILD_LOG}")
endif()
file(SHA256 "${_c_object}" _object_after_dependency_header_change)
if(NOT _object_after_dependency_header_change STREQUAL _object_before_dependency_header_change)
    message(FATAL_ERROR "dependency-only header edit changed the ordinary object")
endif()

file(SHA256 "${_c_depfile}" _c_depfile_before_rejected_runs)
string(SHA256 _c_dependency_key "${_c_depfile}")
set(_c_dependency_command "${_build}/CMakeFiles/aros-literal-object-${_c_dependency_key}.cmake")
if(NOT EXISTS "${_c_dependency_command}")
    message(FATAL_ERROR "C dependency command file is missing")
endif()
file(APPEND "${_c_dependency_command}" "# changed command file\n")
_literal_build("${_build}" literal-dependencies-c-check "changed dependency command" FALSE
    "changed command file")
file(SHA256 "${_c_depfile}" _c_depfile_after_changed_command)
if(NOT _c_depfile_after_changed_command STREQUAL _c_depfile_before_rejected_runs)
    message(FATAL_ERROR "changed command file altered the published dependency output")
endif()
_literal_assert_no_temporary("${_c_depfile}" "changed command rejection")

# A failed compiler run must not truncate or replace the last complete .d.
_literal_configure(success TRUE "")
set(_failure_build "${_LITERAL_TEST_BUILD}")
set(_failure_source "${_failure_build}/source/c/main.c")
set(_failure_depfile "${_failure_build}/declared-build/gen/c/main.d")
_literal_build("${_failure_build}" literal-dependencies-c "dependency failure setup" TRUE "")
file(SHA256 "${_failure_depfile}" _failure_depfile_before)
file(APPEND "${_failure_source}" "#error dependency compiler failure\n")
_literal_build("${_failure_build}" literal-dependencies-c "compiler failure" FALSE
    "Literal dependency compiler failed")
file(SHA256 "${_failure_depfile}" _failure_depfile_after)
if(NOT _failure_depfile_after STREQUAL _failure_depfile_before)
    message(FATAL_ERROR "failed compiler run replaced the existing dependency file")
endif()
_literal_assert_no_temporary("${_failure_depfile}" "compiler failure")

# The preflight refuses output symlinks and leaves the moved regular file intact.
_literal_configure(success TRUE "")
set(_symlink_build "${_LITERAL_TEST_BUILD}")
set(_symlink_depfile "${_symlink_build}/declared-build/gen/c/main.d")
set(_saved_depfile "${_symlink_build}/declared-build/gen/c/main.saved.d")
_literal_build("${_symlink_build}" literal-dependencies-c "symlink setup" TRUE "")
file(SHA256 "${_symlink_depfile}" _saved_depfile_hash)
file(RENAME "${_symlink_depfile}" "${_saved_depfile}")
file(CREATE_LINK "${_saved_depfile}" "${_symlink_depfile}" SYMBOLIC)
_literal_build("${_symlink_build}" literal-dependencies-c-check "symlink output rejection" FALSE
    "symlink dependency output")
file(SHA256 "${_saved_depfile}" _saved_depfile_after)
if(NOT _saved_depfile_after STREQUAL _saved_depfile_hash)
    message(FATAL_ERROR "symlink rejection changed the saved regular dependency file")
endif()
file(REMOVE "${_symlink_depfile}")
file(RENAME "${_saved_depfile}" "${_symlink_depfile}")
_literal_assert_no_temporary("${_symlink_depfile}" "symlink rejection")

_literal_configure(native-includes TRUE "")
file(GLOB _native_commands "${_LITERAL_TEST_BUILD}/CMakeFiles/aros-literal-object-*.cmake")
if(NOT _native_commands)
    message(FATAL_ERROR "native dependency mode did not write a command file")
endif()
list(GET _native_commands 0 _native_command)
file(READ "${_native_command}" _native_command_contents)
foreach(_native_include IN ITEMS
        "${_LITERAL_TEST_BUILD}/declared-build/GENINCDIR"
        "${_LITERAL_TEST_BUILD}/declared-build/SDK/include/aros/stdc"
        "${_LITERAL_TEST_BUILD}/declared-build/SDK/include")
    string(FIND "${_native_command_contents}" "-I${_native_include}]==]" _native_found)
    if(_native_found LESS 0)
        message(FATAL_ERROR "native dependency command omitted header root ${_native_include}")
    endif()
endforeach()

message(STATUS
    "Literal dependency files: C/CXX -M, selected i386 Clang role, -MQ special paths, private-depfile collision, and failure publication checks passed")
