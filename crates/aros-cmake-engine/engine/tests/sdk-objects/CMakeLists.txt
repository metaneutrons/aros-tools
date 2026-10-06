cmake_minimum_required(VERSION 3.22)
project(SdkObjectsFixture LANGUAGES C CXX)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/SdkObjects.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
if(NOT DEFINED SDK_OBJECT_CASE OR "${SDK_OBJECT_CASE}" STREQUAL "")
    set(SDK_OBJECT_CASE success)
endif()

include("${ENGINE_DIR}/SdkObjects.cmake")

if(SDK_OBJECT_CASE STREQUAL "symlink-build-root")
    set(_build_root_alias "${CMAKE_BINARY_DIR}/build-root-alias")
    set(_build_root_target "${CMAKE_BINARY_DIR}/build-root-target")
    file(MAKE_DIRECTORY "${_build_root_target}")
    file(CREATE_LINK "${_build_root_target}" "${_build_root_alias}" SYMBOLIC)
    set(AROS_BUILD_DIR "${_build_root_alias}")
else()
    set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
endif()
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source-root")
set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/lib")
file(COPY "${CMAKE_CURRENT_LIST_DIR}/source/" DESTINATION "${AROS_SOURCE_DIR}")
file(MAKE_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")

set(_plain_source "${AROS_SOURCE_DIR}/plain/choice.c")
set(_xopen_source "${AROS_SOURCE_DIR}/xopen/choice.c")
set(_cpp_source "${AROS_SOURCE_DIR}/cpp/choice.cpp")
set(_plain_intermediate "${AROS_BUILD_DIR}/gen/sdk-objects/plain-choice.o")
set(_xopen_intermediate "${AROS_BUILD_DIR}/gen/sdk-objects/xopen-choice.o")
set(_cpp_intermediate "${AROS_BUILD_DIR}/gen/sdk-objects/cpp-choice.o")
set(_plain_output "${AROS_DEVELOPER_LIB_DIR}/plain-choice.o")
set(_xopen_output "${AROS_DEVELOPER_LIB_DIR}/xopen-choice.o")
set(_cpp_output "${AROS_DEVELOPER_LIB_DIR}/cpp-choice.o")

function(_add_plain)
    aros_compile_sdk_object(
        NAME sdk-plain-choice
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        DEFINES SDK_PLAIN_PROJECTION=17
        OPTIONS -fno-omit-frame-pointer -fno-common)
endfunction()

function(_add_xopen)
    aros_compile_sdk_object(
        NAME sdk-xopen-choice
        SOURCE "${_xopen_source}"
        INTERMEDIATE "${_xopen_intermediate}"
        OUTPUT "${_xopen_output}"
        LANGUAGE C
        DEFINES _XOPEN_SOURCE=700 SDK_XOPEN_PROJECTION=29
        OPTIONS -fno-omit-frame-pointer -fno-common)
endfunction()

function(_add_cpp)
    aros_compile_sdk_object(
        NAME sdk-cpp-choice
        SOURCE "${_cpp_source}"
        INTERMEDIATE "${_cpp_intermediate}"
        OUTPUT "${_cpp_output}"
        LANGUAGE CXX
        DEFINES SDK_CPP_PROJECTION=43
        OPTIONS -fno-omit-frame-pointer -fno-exceptions)
endfunction()

function(_add_program_object owner stem)
    aros_compile_sdk_object(
        NAME "${owner}"
        SOURCE "${AROS_SOURCE_DIR}/program/${stem}.c"
        INTERMEDIATE "${AROS_BUILD_DIR}/gen/sdk-objects/${stem}.o"
        OUTPUT "${AROS_DEVELOPER_LIB_DIR}/${stem}.o"
        LANGUAGE C
        OPTIONS -fno-omit-frame-pointer)
endfunction()

if(SDK_OBJECT_CASE STREQUAL "success" OR
   SDK_OBJECT_CASE MATCHES "^runtime-(output|intermediate|parent|root|source-file|source-parent|compiled-object)-symlink$")
    _add_plain()
    _add_xopen()
    _add_cpp()
    aros_sdk_object_group(
        NAME sdk-objects
        OBJECTS "${_plain_output}" "${_xopen_output}" "${_cpp_output}")
    get_target_property(_sdk_object_compile_targets sdk-objects
        AROS_SDK_OBJECT_COMPILE_TARGETS)
    if(NOT "${_sdk_object_compile_targets}" STREQUAL
       "sdk-plain-choice-compile;sdk-xopen-choice-compile;sdk-cpp-choice-compile")
        message(FATAL_ERROR
            "SDK object group compile target property is not its exact private owner list: ${_sdk_object_compile_targets}")
    endif()

    # Recreate the stale Make-style product left by another source with the
    # same basename. The target's private CMake object must replace it.
    file(MAKE_DIRECTORY "${AROS_BUILD_DIR}/gen/sdk-objects")
    execute_process(
        COMMAND "${CMAKE_C_COMPILER}" -DSDK_STALE_DECOY=1 -c
            "${AROS_SOURCE_DIR}/decoy/choice.c" -o
            "${AROS_BUILD_DIR}/gen/sdk-objects/plain-choice.o"
        RESULT_VARIABLE _stale_result
        OUTPUT_VARIABLE _stale_stdout
        ERROR_VARIABLE _stale_stderr)
    if(NOT _stale_result EQUAL 0)
        message(FATAL_ERROR "could not create stale Make-style object\n${_stale_stdout}${_stale_stderr}")
    endif()
elseif(SDK_OBJECT_CASE STREQUAL "duplicate-output")
    _add_plain()
    aros_compile_sdk_object(
        NAME sdk-duplicate-output
        SOURCE "${_xopen_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        DEFINES SDK_XOPEN_PROJECTION=29)
elseif(SDK_OBJECT_CASE STREQUAL "duplicate-owner")
    _add_plain()
    aros_compile_sdk_object(
        NAME sdk-plain-choice
        SOURCE "${_xopen_source}"
        INTERMEDIATE "${_xopen_intermediate}"
        OUTPUT "${_xopen_output}"
        LANGUAGE C
        DEFINES SDK_XOPEN_PROJECTION=29)
elseif(SDK_OBJECT_CASE STREQUAL "duplicate-group-owner")
    _add_plain()
    aros_sdk_object_group(NAME repeated-group OBJECTS "${_plain_output}")
    aros_sdk_object_group(NAME repeated-group OBJECTS "${_plain_output}")
elseif(SDK_OBJECT_CASE STREQUAL "outside-lib")
    aros_compile_sdk_object(
        NAME sdk-outside-lib
        SOURCE "${_plain_source}"
        INTERMEDIATE "${AROS_BUILD_DIR}/gen/sdk-objects/outside.o"
        OUTPUT "${AROS_BUILD_DIR}/outside.o"
        LANGUAGE C)
elseif(SDK_OBJECT_CASE STREQUAL "symlink-build-root")
    aros_compile_sdk_object(
        NAME sdk-symlink-build-root
        SOURCE "${_plain_source}"
        INTERMEDIATE "${AROS_BUILD_DIR}/gen/sdk-objects/plain-choice.o"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        DEFINES SDK_PLAIN_PROJECTION=17)
elseif(SDK_OBJECT_CASE STREQUAL "invalid-intermediate")
    aros_compile_sdk_object(
        NAME sdk-invalid-intermediate
        SOURCE "${_plain_source}"
        INTERMEDIATE "${AROS_BUILD_DIR}/other/plain-choice.o"
        OUTPUT "${_plain_output}"
        LANGUAGE C)
elseif(SDK_OBJECT_CASE STREQUAL "invalid-language")
    aros_compile_sdk_object(
        NAME sdk-invalid-language
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE OBJC)
elseif(SDK_OBJECT_CASE STREQUAL "language-mismatch")
    aros_compile_sdk_object(
        NAME sdk-language-mismatch
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE CXX)
elseif(SDK_OBJECT_CASE STREQUAL "source-symlink")
    file(CREATE_LINK "${_plain_source}" "${AROS_SOURCE_DIR}/plain/alias.c" SYMBOLIC)
    aros_compile_sdk_object(
        NAME sdk-source-symlink
        SOURCE "${AROS_SOURCE_DIR}/plain/alias.c"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C)
elseif(SDK_OBJECT_CASE STREQUAL "missing-group-owner")
    aros_sdk_object_group(
        NAME sdk-missing-group-owner
        OBJECTS "${AROS_DEVELOPER_LIB_DIR}/missing.o")
elseif(SDK_OBJECT_CASE STREQUAL "missing-program-role")
    _add_program_object(source-startup-owner startup)
    _add_program_object(source-detach-owner detach)
    aros_bind_source_sdk_program_inputs(REQUIRE CXX_STARTUP)
elseif(SDK_OBJECT_CASE STREQUAL "optional-program-roles")
    _add_program_object(source-startup-owner startup)
    set(AROS_CXX_STARTUP_TARGET stale-legacy-cxx)
    set(AROS_C_DETACH_TARGET stale-legacy-detach)
    aros_bind_source_sdk_program_inputs()
    if(NOT AROS_C_STARTUP_TARGET STREQUAL "source-startup-owner" OR
       AROS_CXX_STARTUP_TARGET OR AROS_C_DETACH_TARGET OR
       _aros_cxx_startup_output OR _aros_c_detach_output)
        message(FATAL_ERROR "optional source roles reused missing or stale owners")
    endif()
elseif(SDK_OBJECT_CASE STREQUAL "program-role-bindings")
    _add_program_object(source-startup-owner startup)
    _add_program_object(source-detach-owner detach)
    _add_program_object(source-cxx-startup-owner cxx-startup)
    set(_program_startup_output "${AROS_DEVELOPER_LIB_DIR}/startup.o")
    set(_program_detach_output "${AROS_DEVELOPER_LIB_DIR}/detach.o")
    set(_program_cxx_startup_output "${AROS_DEVELOPER_LIB_DIR}/cxx-startup.o")
    aros_sdk_object_group(
        NAME native-program-objects
        OBJECTS "${_program_startup_output}"
            "${_program_detach_output}" "${_program_cxx_startup_output}")
    get_target_property(_program_compile_targets native-program-objects
        AROS_SDK_OBJECT_COMPILE_TARGETS)
    if(NOT "${_program_compile_targets}" STREQUAL
       "source-startup-owner-compile;source-detach-owner-compile;source-cxx-startup-owner-compile")
        message(FATAL_ERROR
            "native program group compile targets are not its exact private owners: ${_program_compile_targets}")
    endif()
    aros_bind_source_sdk_program_inputs()
    if(NOT AROS_C_STARTUP_TARGET STREQUAL "source-startup-owner" OR
       NOT AROS_C_DETACH_TARGET STREQUAL "source-detach-owner" OR
       NOT AROS_CXX_STARTUP_TARGET STREQUAL "source-cxx-startup-owner")
        message(FATAL_ERROR
            "native program link roles did not resolve their registered owners: "
            "C=${AROS_C_STARTUP_TARGET}; detach=${AROS_C_DETACH_TARGET}; "
            "CXX=${AROS_CXX_STARTUP_TARGET}")
    endif()
    if(NOT _aros_c_startup_output STREQUAL "${_program_startup_output}" OR
       NOT _aros_c_detach_output STREQUAL "${_program_detach_output}" OR
       NOT _aros_cxx_startup_output STREQUAL "${_program_cxx_startup_output}")
        message(FATAL_ERROR
            "native program link roles did not resolve expected object paths: "
            "C=${_aros_c_startup_output}; detach=${_aros_c_detach_output}; "
            "CXX=${_aros_cxx_startup_output}")
    endif()
elseif(SDK_OBJECT_CASE STREQUAL "invalid-option")
    aros_compile_sdk_object(
        NAME sdk-invalid-option
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        OPTIONS "-fno-omit-frame-pointer --invalid")
elseif(SDK_OBJECT_CASE STREQUAL "plugin-option")
    aros_compile_sdk_object(
        NAME sdk-plugin-option
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        OPTIONS "-fplugin=/tmp/foo")
elseif(SDK_OBJECT_CASE STREQUAL "specs-option")
    aros_compile_sdk_object(
        NAME sdk-specs-option
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        OPTIONS "-specs=/tmp/foo")
elseif(SDK_OBJECT_CASE STREQUAL "output-option")
    aros_compile_sdk_object(
        NAME sdk-output-option
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        OPTIONS "-o/tmp/foo")
elseif(SDK_OBJECT_CASE STREQUAL "invalid-define")
    aros_compile_sdk_object(
        NAME sdk-invalid-define
        SOURCE "${_plain_source}"
        INTERMEDIATE "${_plain_intermediate}"
        OUTPUT "${_plain_output}"
        LANGUAGE C
        DEFINES "SDK_BAD=not valid")
else()
    message(FATAL_ERROR "unknown SDK_OBJECT_CASE '${SDK_OBJECT_CASE}'")
endif()
