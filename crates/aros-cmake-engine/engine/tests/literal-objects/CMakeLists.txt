cmake_minimum_required(VERSION 3.22)
project(LiteralObjectsFixture LANGUAGES C CXX)
file(REAL_PATH "${CMAKE_C_COMPILER}" _fixture_real_c_compiler)
file(REAL_PATH "${CMAKE_CXX_COMPILER}" _fixture_real_cxx_compiler)
set(CMAKE_C_COMPILER "${_fixture_real_c_compiler}")
set(CMAKE_CXX_COMPILER "${_fixture_real_cxx_compiler}")

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/LiteralObjects.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
if(NOT DEFINED LITERAL_CASE)
    set(LITERAL_CASE success)
endif()
include("${ENGINE_DIR}/LiteralObjects.cmake")
set(CMAKE_C_FLAGS "-DLITERAL_CMAKE_GLOBAL_C=1")
set(CMAKE_CXX_FLAGS "-DLITERAL_CMAKE_GLOBAL_CXX=1")

set(_source_real "${CMAKE_BINARY_DIR}/source-real")
file(COPY "${CMAKE_CURRENT_LIST_DIR}/source/" DESTINATION "${_source_real}")
set(AROS_SOURCE_DIR "${_source_real}")
if(LITERAL_CASE STREQUAL "source-root-symlink")
    set(_source_alias "${CMAKE_BINARY_DIR}/source-alias")
    file(CREATE_LINK "${_source_real}" "${_source_alias}" SYMBOLIC)
    set(AROS_SOURCE_DIR "${_source_alias}")
endif()

set(_build_real "${CMAKE_BINARY_DIR}/declared-build")
file(MAKE_DIRECTORY "${_build_real}")
set(AROS_BUILD_DIR "${_build_real}")
if(LITERAL_CASE STREQUAL "build-root-symlink")
    set(_build_alias "${CMAKE_BINARY_DIR}/build-alias")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/build-real-target")
    file(CREATE_LINK "${CMAKE_BINARY_DIR}/build-real-target" "${_build_alias}" SYMBOLIC)
    set(AROS_BUILD_DIR "${_build_alias}")
endif()
if(LITERAL_CASE STREQUAL "output-parent-symlink")
    file(MAKE_DIRECTORY "${_build_real}/gen-target")
    file(CREATE_LINK "${_build_real}/gen-target" "${_build_real}/gen" SYMBOLIC)
endif()

if(LITERAL_CASE STREQUAL "native-includes")
    set(AROS_NATIVE_BUILD_CONTRACT ON)
endif()

set(_source "${AROS_SOURCE_DIR}/src/main.c")
set(_cpp_source "${AROS_SOURCE_DIR}/src/helper.cpp")
if(LITERAL_CASE STREQUAL "source-file-symlink")
    file(CREATE_LINK "${_source}" "${AROS_SOURCE_DIR}/src/alias.c" SYMBOLIC)
    set(_source "${AROS_SOURCE_DIR}/src/alias.c")
elseif(LITERAL_CASE STREQUAL "source-parent-symlink")
    file(CREATE_LINK "${AROS_SOURCE_DIR}/src" "${AROS_SOURCE_DIR}/src-alias" SYMBOLIC)
    set(_source "${AROS_SOURCE_DIR}/src-alias/main.c")
endif()

set(_output "${AROS_BUILD_DIR}/gen/literal/main.o")
set(_cpp_output "${AROS_BUILD_DIR}/gen/literal/helper.o")
if(LITERAL_CASE STREQUAL "output-file-symlink")
    file(MAKE_DIRECTORY "${AROS_BUILD_DIR}/gen/literal")
    file(WRITE "${AROS_BUILD_DIR}/gen/literal/decoy.o" "not an object\n")
    file(CREATE_LINK "${AROS_BUILD_DIR}/gen/literal/decoy.o" "${_output}" SYMBOLIC)
endif()
if(LITERAL_CASE STREQUAL "compiler-symlink")
    file(CREATE_LINK "${_fixture_real_c_compiler}" "${CMAKE_BINARY_DIR}/compiler-alias" SYMBOLIC)
    set(CMAKE_C_COMPILER "${CMAKE_BINARY_DIR}/compiler-alias")
elseif(LITERAL_CASE STREQUAL "runtime-compiler-symlink")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/selected-compiler-dir")
    file(COPY_FILE "${_fixture_real_c_compiler}"
        "${CMAKE_BINARY_DIR}/selected-compiler-dir/selected-compiler")
    file(CHMOD "${CMAKE_BINARY_DIR}/selected-compiler-dir/selected-compiler"
        PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
    set(CMAKE_C_COMPILER "${CMAKE_BINARY_DIR}/selected-compiler-dir/selected-compiler")
elseif(LITERAL_CASE STREQUAL "compiler-launcher")
    set(CMAKE_C_COMPILER_LAUNCHER "${CMAKE_COMMAND}")
endif()

function(_add_c_object)
    aros_compile_literal_object(
        NAME literal-c
        SOURCE "${_source}"
        OUTPUT "${_output}"
        LANGUAGE C
        ARGUMENTS -D X=1 -U X -D X=2 -Wall -Wno-unused-parameter
            -I "${AROS_SOURCE_DIR}"
            -isystem "${AROS_SOURCE_DIR}/sysinclude")
endfunction()

function(_add_cpp_object)
    aros_compile_literal_object(
        NAME literal-cxx
        SOURCE "${_cpp_source}"
        OUTPUT "${_cpp_output}"
        LANGUAGE CXX
        ARGUMENTS -D X=1 -U X -D X=2 -Wall -Wno-unused-parameter
            -I "${AROS_SOURCE_DIR}"
            -isystem "${AROS_SOURCE_DIR}/sysinclude")
endfunction()

if(LITERAL_CASE STREQUAL "outside-source")
    aros_compile_literal_object(NAME literal-outside SOURCE "${CMAKE_BINARY_DIR}/outside.c"
        OUTPUT "${_output}" LANGUAGE C)
elseif(LITERAL_CASE STREQUAL "output-outside-gen")
    aros_compile_literal_object(NAME literal-output-outside SOURCE "${_source}"
        OUTPUT "${AROS_BUILD_DIR}/other/main.o" LANGUAGE C)
elseif(LITERAL_CASE STREQUAL "source-file-symlink" OR
       LITERAL_CASE STREQUAL "source-parent-symlink" OR
       LITERAL_CASE STREQUAL "source-root-symlink" OR
       LITERAL_CASE STREQUAL "build-root-symlink" OR
       LITERAL_CASE STREQUAL "output-parent-symlink" OR
       LITERAL_CASE STREQUAL "compiler-symlink" OR
       LITERAL_CASE STREQUAL "runtime-compiler-symlink" OR
       LITERAL_CASE STREQUAL "compiler-launcher")
    _add_c_object()
    if(LITERAL_CASE STREQUAL "runtime-compiler-symlink")
        _add_cpp_object()
        aros_literal_object_group(NAME literal-objects OBJECTS "${_output}" "${_cpp_output}")
    endif()
elseif(LITERAL_CASE STREQUAL "bad-response")
    aros_compile_literal_object(NAME literal-bad-response SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS @response.rsp)
elseif(LITERAL_CASE STREQUAL "bad-plugin")
    aros_compile_literal_object(NAME literal-bad-plugin SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-fplugin=/tmp/plugin.so")
elseif(LITERAL_CASE STREQUAL "bad-specs")
    aros_compile_literal_object(NAME literal-bad-specs SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-specs=/tmp/unsafe.specs")
elseif(LITERAL_CASE STREQUAL "bad-output-role")
    aros_compile_literal_object(NAME literal-bad-output SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-o/tmp/elsewhere.o")
elseif(LITERAL_CASE STREQUAL "bad-semicolon")
    aros_compile_literal_object(NAME literal-bad-semicolon SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-DX=1;touch")
elseif(LITERAL_CASE STREQUAL "bad-newline")
    string(ASCII 10 _newline)
    aros_compile_literal_object(NAME literal-bad-newline SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-DX=1${_newline}touch")
elseif(LITERAL_CASE STREQUAL "bad-missing-define")
    aros_compile_literal_object(NAME literal-bad-missing-define SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -D)
elseif(LITERAL_CASE STREQUAL "bad-invalid-define")
    aros_compile_literal_object(NAME literal-bad-invalid-define SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -D 1BAD=x)
elseif(LITERAL_CASE STREQUAL "bad-shell")
    aros_compile_literal_object(NAME literal-bad-shell SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-DX=1|touch")
elseif(LITERAL_CASE STREQUAL "bad-dollar")
    aros_compile_literal_object(NAME literal-bad-dollar SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-DX=$value")
elseif(LITERAL_CASE STREQUAL "bad-serialization")
    aros_compile_literal_object(NAME literal-bad-serialization SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS "-DX=]==]payload")
elseif(LITERAL_CASE STREQUAL "bad-machine-option")
    aros_compile_literal_object(NAME literal-bad-machine-option SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -mllvm)
elseif(LITERAL_CASE STREQUAL "output-file-symlink")
    _add_c_object()
elseif(LITERAL_CASE STREQUAL "bad-cmodel")
    aros_compile_literal_object(NAME literal-bad-cmodel SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -mcmodel=invalid)
elseif(LITERAL_CASE STREQUAL "missing-build-sysroot")
    aros_compile_literal_object(NAME literal-missing-build-sysroot SOURCE "${_source}"
        OUTPUT "${_output}" LANGUAGE C
        ARGUMENTS --sysroot "${AROS_BUILD_DIR}/SYS/Developer")
elseif(LITERAL_CASE STREQUAL "runtime-sysroot-symlink")
    file(MAKE_DIRECTORY "${AROS_BUILD_DIR}/SYS/Developer")
    aros_compile_literal_object(
        NAME literal-c
        SOURCE "${_source}"
        OUTPUT "${_output}"
        LANGUAGE C
        ARGUMENTS -DX=1 -UX -DX=2 -I "${AROS_SOURCE_DIR}"
            -isystem "${AROS_SOURCE_DIR}/sysinclude"
            --sysroot "${AROS_BUILD_DIR}/SYS/Developer")
    aros_literal_object_group(NAME literal-objects OBJECTS "${_output}")
elseif(LITERAL_CASE STREQUAL "external-include")
    aros_compile_literal_object(NAME literal-external-include SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -I /tmp/outside)
elseif(LITERAL_CASE STREQUAL "sdk-output-conflict")
    string(TOLOWER "${_output}" _folded)
    string(SHA256 _key "${_folded}")
    set_property(GLOBAL PROPERTY "AROS_SDK_OBJECT_${_key}" "sdk-existing-owner")
    aros_compile_literal_object(NAME literal-sdk-conflict SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C)
elseif(LITERAL_CASE STREQUAL "duplicate-output")
    _add_c_object()
    aros_compile_literal_object(NAME literal-second-owner SOURCE "${_cpp_source}" OUTPUT "${_output}"
        LANGUAGE CXX)
elseif(LITERAL_CASE STREQUAL "missing-group-object")
    aros_literal_object_group(NAME literal-missing-group OBJECTS "${_output}")
elseif(LITERAL_CASE STREQUAL "group-case-alias")
    _add_c_object()
    aros_literal_object_group(NAME literal-case-alias
        OBJECTS "${AROS_BUILD_DIR}/gen/literal/MAIN.o")
elseif(LITERAL_CASE STREQUAL "valid-m-options")
    aros_compile_literal_object(NAME literal-valid-m SOURCE "${_source}" OUTPUT "${_output}"
        LANGUAGE C ARGUMENTS -mcmodel=medany -mstrict-align -mno-strict-align)
    aros_literal_object_group(NAME literal-objects OBJECTS "${_output}")
else()
    _add_c_object()
    _add_cpp_object()
    aros_literal_object_group(NAME literal-objects OBJECTS "${_output}" "${_cpp_output}")
    get_target_property(_compile_targets literal-objects AROS_LITERAL_OBJECT_COMPILE_TARGETS)
    if(NOT "${_compile_targets}" STREQUAL "literal-c-compile;literal-cxx-compile")
        message(FATAL_ERROR "group compile property is not the exact compile owner list: ${_compile_targets}")
    endif()
endif()
