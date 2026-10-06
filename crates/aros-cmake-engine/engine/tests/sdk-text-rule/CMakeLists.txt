cmake_minimum_required(VERSION 3.22)
project(SdkTextRuleFixture NONE)

if(NOT DEFINED AROS_ENGINE_DIR OR NOT DEFINED AROS_SDK_TEXT_FIXTURE_DIR)
    message(FATAL_ERROR "SDK text fixture needs engine and fixture directories")
endif()
include("${AROS_ENGINE_DIR}/SdkTextRules.cmake")

set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/lib")
file(MAKE_DIRECTORY
    "${CMAKE_BINARY_DIR}/Ports/zlib"
    "${CMAKE_BINARY_DIR}/Ports/zstd"
    "${AROS_DEVELOPER_LIB_DIR}")
file(COPY_FILE "${AROS_SDK_TEXT_FIXTURE_DIR}/zlib.pc.cmakein"
    "${CMAKE_BINARY_DIR}/Ports/zlib/zlib.pc.cmakein")
file(COPY_FILE "${AROS_SDK_TEXT_FIXTURE_DIR}/libzstd.pc.in"
    "${CMAKE_BINARY_DIR}/Ports/zstd/libzstd.pc.in")

foreach(_port IN ITEMS zlib zstd)
    set(_fetch_destination "${CMAKE_BINARY_DIR}/Ports/${_port}")
    set(_stamp "${CMAKE_BINARY_DIR}/Ports/${_port}/.fetch-complete")
    if(_port STREQUAL "zlib")
        set(_fetch_input "${_fetch_destination}/zlib.pc.cmakein")
    else()
        set(_fetch_input "${_fetch_destination}/libzstd.pc.in")
    endif()
    add_custom_command(
        OUTPUT "${_stamp}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
        DEPENDS "${_fetch_input}"
        VERBATIM)
    add_custom_target("sdk-text-${_port}-fetch" DEPENDS "${_stamp}")
    set_property(TARGET "sdk-text-${_port}-fetch" PROPERTY
        AROS_FETCH_DESTINATION "${_fetch_destination}")
    set_property(TARGET "sdk-text-${_port}-fetch" PROPERTY
        AROS_FETCH_COMPLETION_STAMP "${_stamp}")
endforeach()

set(_sdk_prefix "\${prefix}")
aros_transform_sdk_text(
    NAME sdk-text-zlib-pkgc
    FETCH sdk-text-zlib-fetch
    INPUT "${CMAKE_BINARY_DIR}/Ports/zlib/zlib.pc.cmakein"
    OUTPUT "${AROS_DEVELOPER_LIB_DIR}/pkgconfig/zlib.pc"
    OPERATIONS
        "REPLACE_ALL|@CMAKE_INSTALL_PREFIX@|/Developer"
        "REPLACE_ALL|@INSTALL_LIB_DIR@|${_sdk_prefix}/lib"
        "REPLACE_ALL|@INSTALL_INC_DIR@|${_sdk_prefix}/include"
        "REPLACE_ALL|@VERSION@|1.3.1"
        "REPLACE_LINE|exec_prefix=|exec_prefix=${_sdk_prefix}")
aros_transform_sdk_text(
    NAME sdk-text-zstd-pkgc
    FETCH sdk-text-zstd-fetch
    INPUT "${CMAKE_BINARY_DIR}/Ports/zstd/libzstd.pc.in"
    OUTPUT "${AROS_DEVELOPER_LIB_DIR}/pkgconfig/libzstd.pc"
    OPERATIONS
        "REPLACE_ALL|@PREFIX@|/Developer"
        "REPLACE_ALL|@LIBDIR@|${_sdk_prefix}/lib"
        "REPLACE_ALL|@INCLUDEDIR@|${_sdk_prefix}/include"
        "REPLACE_ALL|@VERSION@|1.5.7"
        "REPLACE_ALL|@LIBS_MT@|"
        "DELETE_LINE_PREFIX|Libs.private"
        "REPLACE_LINE|exec_prefix=|exec_prefix=${_sdk_prefix}")

add_custom_target(all-sdk-text)
add_dependencies(all-sdk-text sdk-text-zlib-pkgc sdk-text-zstd-pkgc)
