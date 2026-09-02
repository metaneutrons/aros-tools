cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-fetch-patch-${_suffix}")
set(_source "${_root}/source")
set(_build "${_root}/build")
set(_archive_stage "${_root}/archive-stage")
set(_archive_origin "${_root}/archive-origin")
set(_patch "${_source}/patches/value.patch")
set(_product "${_build}/product.txt")
set(_fetched_source "${_build}/ports/fixture-src/value.txt")

file(MAKE_DIRECTORY
    "${_source}/cmake"
    "${_source}/patches"
    "${_archive_stage}/fixture-src"
    "${_archive_origin}")
file(WRITE "${_archive_stage}/fixture-src/value.txt" "original\n")
file(WRITE "${_source}/cmake/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E tar czf
        "${_archive_origin}/fixture.tar.gz"
        --format=gnutar fixture-src
    WORKING_DIRECTORY "${_archive_stage}"
    RESULT_VARIABLE _archive_result
    OUTPUT_VARIABLE _archive_stdout
    ERROR_VARIABLE _archive_stderr)
if(NOT _archive_result EQUAL 0)
    message(FATAL_ERROR
        "could not create local fetch archive (${_archive_result})\n"
        "${_archive_stdout}\n${_archive_stderr}")
endif()
file(SHA256 "${_archive_origin}/fixture.tar.gz" ARCHIVE_SHA256)

set(_first_patch [=[--- a/value.txt
+++ b/value.txt
@@ -1 +1 @@
-original
+first
]=])
set(_second_patch [=[--- a/value.txt
+++ b/value.txt
@@ -1 +1 @@
-original
+second
]=])
file(WRITE "${_patch}" "${_first_patch}")

get_filename_component(_cmake_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
get_filename_component(_repo_root "${_cmake_dir}/.." ABSOLUTE)
set(_fixture_cmake [=[
cmake_minimum_required(VERSION 3.22)
project(FetchArchivePatchFixture NONE)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_FETCH_BIN "@FETCH_BIN@")
include("@AROS_CMAKE@")

set(_patch "${CMAKE_CURRENT_SOURCE_DIR}/patches/value.patch")
set(_ports "${CMAKE_BINARY_DIR}/ports")
set(_source "${_ports}/fixture-src")

aros_fetch_archive(
    NAME fixture-fetch
    ARCHIVE fixture
    SUFFIXES tar.gz
    ORIGINS "@ARCHIVE_ORIGIN@"
    CHECKSUMS "fixture.tar.gz=sha256:@ARCHIVE_SHA256@"
    LOCATION "${CMAKE_BINARY_DIR}/archives"
    DESTINATION "${_ports}"
    BASE "${_ports}"
    PATCH_ORIGINS "${CMAKE_CURRENT_SOURCE_DIR}/patches"
    PATCHES "value.patch:fixture-src:-f,-p1"
    SOURCE_DIR "${_source}"
    LOCAL_PATCH_FILES "${_patch}")

get_target_property(_fetch_stamp fixture-fetch AROS_FETCH_COMPLETION_STAMP)
add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/product.txt"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${_source}/value.txt" "${CMAKE_BINARY_DIR}/product.txt"
    DEPENDS "${_fetch_stamp}"
    VERBATIM)
add_custom_target(fixture-product DEPENDS "${CMAKE_BINARY_DIR}/product.txt")
]=])
if(DEFINED ENV{AROS_FETCH_BIN} AND NOT "$ENV{AROS_FETCH_BIN}" STREQUAL "")
    set(FETCH_BIN "$ENV{AROS_FETCH_BIN}")
else()
    find_program(FETCH_BIN NAMES aros-fetch)
endif()
if(NOT EXISTS "${FETCH_BIN}" OR IS_DIRECTORY "${FETCH_BIN}")
    message(FATAL_ERROR
        "required installed aros-fetch test executable is missing: ${FETCH_BIN}")
endif()
set(ARCHIVE_ORIGIN "${_archive_origin}")
set(AROS_CMAKE "${_cmake_dir}/AROS.cmake")
string(CONFIGURE "${_fixture_cmake}" _fixture_cmake @ONLY)
file(WRITE "${_source}/CMakeLists.txt" "${_fixture_cmake}")

function(_configure_fixture label)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "${label} configure failed (${_result})\n${_stdout}\n${_stderr}")
    endif()
endfunction()

function(_build_fixture label expect_success)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}"
            --target fixture-product
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set("${label}_RESULT" "${_result}" PARENT_SCOPE)
    set("${label}_LOG" "${_stdout}\n${_stderr}" PARENT_SCOPE)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "${label} build failed (${_result})\n${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "${label} build unexpectedly succeeded")
    endif()
endfunction()

function(_assert_contents path expected label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}")
        message(FATAL_ERROR "${label} is missing: ${path}")
    endif()
    file(READ "${path}" _actual)
    if(NOT _actual STREQUAL "${expected}")
        message(FATAL_ERROR
            "${label} contains '${_actual}', expected '${expected}'")
    endif()
endfunction()

_configure_fixture(initial)
_build_fixture(initial TRUE)
_assert_contents("${_fetched_source}" "first\n" "initial patched source")
_assert_contents("${_product}" "first\n" "initial product")

# Ensure Ninja observes the patch as newer than the successful fetch stamp even
# on filesystems with coarse timestamp resolution.
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1)
file(WRITE "${_patch}" "${_second_patch}")

_build_fixture(updated TRUE)
_assert_contents("${_fetched_source}" "second\n" "refreshed patched source")
_assert_contents("${_product}" "second\n" "refreshed product")

_build_fixture(noop TRUE)
string(FIND "${noop_LOG}" "ninja: no work to do." _noop_found)
if(_noop_found LESS 0)
    message(FATAL_ERROR
        "final fetch/product build was not a Ninja no-op:\n${noop_LOG}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "direct fetch patch refresh test passed")
