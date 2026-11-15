cmake_minimum_required(VERSION 3.22)
# Apple's host ar otherwise timestamps its index differently across two
# independent invocations. This test-only normalization applies to both;
# the separate admitted GNU fixture verifies native AR/RANLIB bytes directly.
set(ENV{ZERO_AR_DATE} 1)
set(fixture "${CMAKE_CURRENT_LIST_DIR}/source-archives")
cmake_path(GET CMAKE_CURRENT_LIST_DIR PARENT_PATH engine)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(base "$ENV{TMPDIR}")
else()
    set(base /tmp)
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef suffix)
set(root "${base}/aros-source-archives-${suffix} with spaces")
if(EXISTS "${root}" OR IS_SYMLINK "${root}")
    message(FATAL_ERROR "source archive test root already exists")
endif()
file(MAKE_DIRECTORY "${root}")
file(REAL_PATH "${root}" root)
# Record how the runner invokes ar. Debian, Ubuntu and Arch build binutils with
# deterministic archives by default, so the bytes alone cannot show whether the
# runner asked for it; the toolchain this producer builds does not.
if(DEFINED ARCHIVE_TEST_AR AND DEFINED ARCHIVE_TEST_RANLIB)
    set(real_ar "${ARCHIVE_TEST_AR}")
    set(real_ranlib "${ARCHIVE_TEST_RANLIB}")
else()
    find_program(real_ar NAMES gar ar REQUIRED)
    find_program(real_ranlib NAMES granlib ranlib REQUIRED)
endif()
set(ar_log "${root}/ar-calls.log")
file(MAKE_DIRECTORY "${root}/tools")
file(WRITE "${root}/tools/ar"
    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '${ar_log}'\nexec '${real_ar}' \"$@\"\n")
file(CHMOD "${root}/tools/ar" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE
    GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
set(ARCHIVE_TEST_AR "${root}/tools/ar")
set(ARCHIVE_TEST_RANLIB "${real_ranlib}")
function(configure build probe)
    set(tool_arguments)
    if(DEFINED ARCHIVE_TEST_AR AND DEFINED ARCHIVE_TEST_RANLIB)
        list(APPEND tool_arguments "-DFIXTURE_AR=${ARCHIVE_TEST_AR}"
            "-DFIXTURE_RANLIB=${ARCHIVE_TEST_RANLIB}")
    endif()
    execute_process(COMMAND "${CMAKE_COMMAND}" -S "${fixture}" -B "${build}" -G Ninja
        "-DENGINE_DIR=${engine}" "-DPROBE=${probe}" ${tool_arguments}
        RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
    set(result "${result}" PARENT_SCOPE)
    set(log "${stdout}\n${stderr}" PARENT_SCOPE)
endfunction()
function(build_archive build)
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${build}" --target source-archive
        RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
    if(NOT result STREQUAL "0")
        message(FATAL_ERROR "source archive build failed: ${stdout}\n${stderr}")
    endif()
endfunction()
set(build "${root}/build")
configure("${build}" "")
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "source archive configure failed: ${log}")
endif()
build_archive("${build}")
include("${build}/selected-tools.cmake")
set(archive "${build}/SYS/Developer/lib/libsource.a")
execute_process(COMMAND "${AR}" t "${archive}" RESULT_VARIABLE result OUTPUT_VARIABLE members)
string(REPLACE "__.SYMDEF SORTED\n" "" members "${members}")
if(NOT result STREQUAL "0" OR NOT members STREQUAL "two.o\none.o\n")
    message(FATAL_ERROR "source archive lost source member order: ${members}")
endif()
# The runner asks GNU and LLVM tools for deterministic members (D) and leaves
# Apple's ar alone; the independent reference makes the same choice.
set(reference_flags cr)
set(reference_ranlib_arguments)
set(deterministic FALSE)
execute_process(COMMAND "${AR}" --version RESULT_VARIABLE version_result
    OUTPUT_VARIABLE version_text ERROR_QUIET)
if(version_result EQUAL 0 AND version_text MATCHES "GNU|LLVM")
    set(deterministic TRUE)
    set(reference_flags crD)
    set(reference_ranlib_arguments -D)
endif()
if(deterministic)
    # The runner itself must ask for deterministic members, whatever the host
    # ar does by default. Read before the reference invocation below logs too.
    file(READ "${ar_log}" ar_calls)
    if(NOT ar_calls MATCHES "(^|\n)crD ")
        message(FATAL_ERROR
            "source archive runner did not request deterministic members; ar was called with:\n${ar_calls}")
    endif()
endif()
set(reference "${root}/reference.a")
execute_process(COMMAND "${AR}" "${reference_flags}" "${reference}" "${build}/gen/objects/two.o"
    "${build}/gen/objects/one.o" RESULT_VARIABLE result)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "reference AR failed")
endif()
execute_process(COMMAND "${RANLIB}" ${reference_ranlib_arguments} "${reference}" RESULT_VARIABLE result)
execute_process(COMMAND "${CMAKE_COMMAND}" -E compare_files "${archive}" "${reference}"
    RESULT_VARIABLE comparison)
if(NOT result STREQUAL "0" OR NOT comparison STREQUAL "0")
    message(FATAL_ERROR "source archive differs from independent AR/RANLIB invocation")
endif()
if(deterministic)
    # No member may record who ran ar (qualify-v2-full 43611118879 differed on
    # exactly this between the Linux runners and macOS).
    execute_process(COMMAND "${AR}" tv "${archive}" RESULT_VARIABLE result
        OUTPUT_VARIABLE listing)
    string(STRIP "${listing}" listing)
    string(REPLACE "\n" ";" listing_lines "${listing}")
    if(NOT result STREQUAL "0" OR NOT listing_lines)
        message(FATAL_ERROR "cannot list source archive members")
    endif()
    foreach(line IN LISTS listing_lines)
        if(NOT line MATCHES " 0/0 ")
            message(FATAL_ERROR "source archive member records its owner: ${line}")
        endif()
    endforeach()
    # Control: the same objects through the non-deterministic modifier carry the
    # real owner, so the check above could tell the modes apart. Skipped for
    # root, whose uid is already 0.
    execute_process(COMMAND id -u OUTPUT_VARIABLE uid OUTPUT_STRIP_TRAILING_WHITESPACE)
    if(NOT uid STREQUAL "0")
        set(control "${root}/control.a")
        execute_process(COMMAND "${AR}" crU "${control}" "${build}/gen/objects/two.o"
            RESULT_VARIABLE result)
        execute_process(COMMAND "${AR}" tv "${control}" OUTPUT_VARIABLE control_listing)
        if(NOT result STREQUAL "0" OR control_listing MATCHES " 0/0 ")
            message(FATAL_ERROR
                "control archive shows no owner, so this test cannot tell the modes apart: ${control_listing}")
        endif()
    endif()
endif()
file(SHA256 "${archive}" first_digest)
build_archive("${build}")
file(SHA256 "${archive}" second_digest)
if(NOT first_digest STREQUAL second_digest)
    message(FATAL_ERROR "unchanged archive invocation changed bytes")
endif()
configure("${build}" remove-member)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "reconfigure failed: ${log}")
endif()
build_archive("${build}")
execute_process(COMMAND "${AR}" t "${archive}" RESULT_VARIABLE result OUTPUT_VARIABLE members)
string(REPLACE "__.SYMDEF SORTED\n" "" members "${members}")
if(NOT result STREQUAL "0" OR NOT members STREQUAL "one.o\n")
    message(FATAL_ERROR "removed member survived fresh archive publication: ${members}")
endif()
foreach(probe unregistered duplicate flags escape collision file-collision no-contract interface-collision unknown-family)
    configure("${root}/${probe}" "${probe}")
    if(result STREQUAL "0" OR NOT log MATCHES "Source archive:")
        message(FATAL_ERROR "archive refusal probe ${probe} failed: ${log}")
    endif()
endforeach()
configure("${root}/llvm-archive" llvm-archive)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "LLVM source archive configure failed: ${log}")
endif()
build_archive("${root}/llvm-archive")
execute_process(COMMAND "${AR}" t "${root}/llvm-archive/SYS/Developer/lib/libsource.a"
    RESULT_VARIABLE result OUTPUT_VARIABLE members)
string(REPLACE "__.SYMDEF SORTED\n" "" members "${members}")
if(NOT result STREQUAL "0" OR NOT members STREQUAL "two.o\none.o\n")
    message(FATAL_ERROR "LLVM source archive lost declared member order: ${members}")
endif()
file(APPEND "${root}/llvm-archive/toolchain/bin/archive-tool" "# changed executable bytes\n")
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${root}/llvm-archive" --target source-archive-check
    RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
if(result STREQUAL "0" OR NOT "${stdout}\n${stderr}" MATCHES "final compiler bytes differ")
    message(FATAL_ERROR "changed LLVM archive tool was not refused: ${stdout}\n${stderr}")
endif()
foreach(_probe_and_error IN ITEMS
        "build-contract-changed|contract changed after validation"
        "build-bool-without-validation|source validation must run first"
        "consumer-bool-without-validation|requires fresh validation in this configure"
        "consumer-contract-changed|contract changed after validation"
        "consumer-selector-changed|configuration changed after validation"
        "consumer-build-coexistence|build and consumer selections")
    string(REPLACE "|" ";" _parts "${_probe_and_error}")
    list(GET _parts 0 _consumer_probe)
    list(GET _parts 1 _expected_error)
    configure("${root}/${_consumer_probe}" "${_consumer_probe}")
    if(result STREQUAL "0" OR NOT log MATCHES "${_expected_error}")
        message(FATAL_ERROR
            "consumer source-selection refusal probe ${_consumer_probe} failed: ${log}")
    endif()
endforeach()
configure("${root}/consumer-archive" consumer-archive)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "consumer source archive configure failed: ${log}")
endif()
build_archive("${root}/consumer-archive")
execute_process(
    COMMAND "${AR}" t "${root}/consumer-archive/SYS/Developer/lib/libsource.a"
    RESULT_VARIABLE result OUTPUT_VARIABLE members)
string(REPLACE "__.SYMDEF SORTED\n" "" members "${members}")
if(NOT result STREQUAL "0" OR NOT members STREQUAL "two.o\none.o\n")
    message(FATAL_ERROR
        "consumer-selected archive lost source member order: ${members}")
endif()
configure("${root}/linked" link-consumer)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "archive consumer configure failed: ${log}")
endif()
# This cold invocation names only the executable. Its imported archive must
# schedule every compile and archive producer without a prior utility build.
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${root}/linked" --target consumer
    RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "archive consumer cold link failed: ${stdout}\n${stderr}")
endif()
execute_process(COMMAND "${root}/linked/consumer" RESULT_VARIABLE result TIMEOUT 10)
if(NOT result STREQUAL "0")
    message(FATAL_ERROR "archive consumer result differs from its exact object producers")
endif()
# A future-dated symlink must not bypass the check target on a no-op build.
file(RENAME "${build}/gen/objects/one.o" "${build}/gen/objects/one.saved.o")
file(CREATE_LINK "${build}/gen/objects/one.saved.o" "${build}/gen/objects/one.o" SYMBOLIC)
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${build}" --target source-archive-check
    RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
if(result STREQUAL "0" OR NOT "${stdout}\n${stderr}" MATCHES "symlink")
    message(FATAL_ERROR "archive member symlink was not refused: ${stdout}\n${stderr}")
endif()
file(REMOVE "${build}/gen/objects/one.o")
file(RENAME "${build}/gen/objects/one.saved.o" "${build}/gen/objects/one.o")
file(GLOB command_files "${build}/CMakeFiles/aros-source-archive-*.cmake")
list(GET command_files 0 command_file)
file(APPEND "${command_file}" "# changed after configure\n")
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${build}" --target source-archive
    RESULT_VARIABLE result OUTPUT_VARIABLE stdout ERROR_VARIABLE stderr TIMEOUT 90)
if(result STREQUAL "0" OR NOT "${stdout}\n${stderr}" MATCHES "changed command file refused")
    message(FATAL_ERROR "changed archive command was not refused: ${stdout}\n${stderr}")
endif()
message(STATUS "Source archive differential, order, incremental and refusal probes passed")
