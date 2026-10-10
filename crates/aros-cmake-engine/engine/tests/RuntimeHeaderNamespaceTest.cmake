cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja REQUIRED)

if(NOT DEFINED AROS_TEST_TOOLCHAIN OR AROS_TEST_TOOLCHAIN STREQUAL "")
    set(AROS_TEST_TOOLCHAIN llvm)
endif()
if(NOT AROS_TEST_TOOLCHAIN MATCHES "^(gnu|llvm)$")
    message(FATAL_ERROR "AROS_TEST_TOOLCHAIN must be gnu or llvm")
endif()

if(AROS_TEST_TOOLCHAIN STREQUAL "gnu")
    if(NOT DEFINED AROS_TEST_GNU_COMPILER OR
       NOT IS_ABSOLUTE "${AROS_TEST_GNU_COMPILER}" OR
       NOT EXISTS "${AROS_TEST_GNU_COMPILER}")
        message(FATAL_ERROR
            "GNU namespace test requires -DAROS_TEST_GNU_COMPILER=<absolute gcc path>")
    endif()
    set(_cc "${AROS_TEST_GNU_COMPILER}")
    if(DEFINED AROS_TEST_GNU_CXX_COMPILER AND
       NOT AROS_TEST_GNU_CXX_COMPILER STREQUAL "")
        set(_cxx "${AROS_TEST_GNU_CXX_COMPILER}")
    else()
        get_filename_component(_compiler_dir "${_cc}" DIRECTORY)
        get_filename_component(_compiler_name "${_cc}" NAME)
        if(NOT _compiler_name MATCHES "gcc(-[0-9.]+)?$")
            message(FATAL_ERROR
                "GNU C++ compiler must be explicit unless GCC ends in gcc[-version]: ${_cc}")
        endif()
        string(REGEX REPLACE "gcc(-[0-9.]+)?$" "g++\\1"
            _cxx_name "${_compiler_name}")
        set(_cxx "${_compiler_dir}/${_cxx_name}")
    endif()
    foreach(_compiler IN ITEMS "${_cc}" "${_cxx}")
        if(NOT IS_ABSOLUTE "${_compiler}" OR NOT EXISTS "${_compiler}")
            message(FATAL_ERROR "GNU namespace compiler is missing: ${_compiler}")
        endif()
    endforeach()
else()
    find_program(_cc NAMES clang REQUIRED)
    find_program(_cxx NAMES clang++ REQUIRED)
endif()

if(DEFINED AROS_TEST_ENGINE_DIR AND NOT AROS_TEST_ENGINE_DIR STREQUAL "")
    get_filename_component(_engine "${AROS_TEST_ENGINE_DIR}" ABSOLUTE)
else()
    get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
endif()
if(NOT EXISTS "${_engine}/AROS.cmake")
    message(FATAL_ERROR "AROS test engine is missing: ${_engine}/AROS.cmake")
endif()

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-runtime-header-namespace-${_suffix}")
set(_build "${_root}/build")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/runtime-header-namespace")
file(MAKE_DIRECTORY "${_root}")

set(_fixture_engine "${_engine}")
if(DEFINED AROS_TEST_EXPECT_FAILURE AND
   (AROS_TEST_EXPECT_FAILURE STREQUAL "IMPLICIT_PRIVATE_ANGLE" OR
    AROS_TEST_EXPECT_FAILURE STREQUAL "IMPLICIT_MODULE_ANGLE_OLD_POLICY"))
    # Reintroduce only the historical implicit -I roots in an isolated engine
    # copy. QUOTE_DIRS and the rest of the implementation stay unchanged.
    set(_fixture_engine "${_root}/old-policy-engine")
    file(COPY "${_engine}/" DESTINATION "${_fixture_engine}")
    file(READ "${_fixture_engine}/AROS.cmake" _engine_source)
    string(REPLACE
        [=[set(DIRS ${ARCH_DIRS} ${GENERIC_DIRS})]=]
        [=[set(DIRS ${GEN_DIRS} ${ARCH_DIRS} ${GENERIC_DIRS} ${FALLBACK_DIRS})]=]
        _counterprobe_source "${_engine_source}")
    if(_counterprobe_source STREQUAL _engine_source)
        message(FATAL_ERROR
            "could not restore exactly the old implicit DIRS policy in copied engine")
    endif()
    file(WRITE "${_fixture_engine}/AROS.cmake" "${_counterprobe_source}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -G Ninja
        "-DCMAKE_MAKE_PROGRAM=${_ninja}"
        "-DCMAKE_C_COMPILER=${_cc}"
        "-DCMAKE_CXX_COMPILER=${_cxx}"
        "-DCMAKE_SYSROOT=${_build}/SDK"
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY
        "-DTEST_AROS_TOOLCHAIN=${AROS_TEST_TOOLCHAIN}"
        "-DTEST_ENGINE_DIR=${_fixture_engine}"
        -S "${_fixture}" -B "${_build}"
    TIMEOUT 180
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    file(WRITE "${_root}/configure.log"
        "${_configure_stdout}${_configure_stderr}")
    message(FATAL_ERROR
        "${AROS_TEST_TOOLCHAIN} namespace fixture configure failed\n"
        "${_configure_stdout}${_configure_stderr}\n"
        "Build tree and configure log retained at ${_root}")
endif()

if(DEFINED AROS_TEST_EXPECT_FAILURE AND
   AROS_TEST_EXPECT_FAILURE STREQUAL "GNU_DEFAULT_NAMESPACE")
    if(NOT AROS_TEST_TOOLCHAIN STREQUAL "gnu")
        message(FATAL_ERROR "GNU default-namespace counterprobe requires gnu mode")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target default-c
        TIMEOUT 180
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    set(_build_log "${_build_stdout}${_build_stderr}")
    if(_build_result EQUAL 0 OR
       NOT _build_log MATCHES "runtime namespace mismatch")
        file(WRITE "${_root}/counterprobe.log" "${_build_log}")
        message(FATAL_ERROR
            "prior GNU namespace policy did not fail at the default namespace assertion\n"
            "${_build_log}\nBuild tree and counterprobe log retained at ${_root}")
    endif()
    file(WRITE "${_root}/counterprobe.log" "${_build_log}")
    message(STATUS
        "prior GNU policy rejected: default POSIX namespace selected the SDK root; "
        "counterprobe log: ${_root}/counterprobe.log")
    return()
elseif(DEFINED AROS_TEST_EXPECT_FAILURE AND
       AROS_TEST_EXPECT_FAILURE STREQUAL "IMPLICIT_PRIVATE_ANGLE")
    if(NOT AROS_TEST_TOOLCHAIN STREQUAL "gnu")
        message(FATAL_ERROR "implicit private angle counterprobe requires gnu mode")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target default-c
        TIMEOUT 180
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    set(_build_log "${_build_stdout}${_build_stderr}")
    if(_build_result EQUAL 0 OR
       NOT _build_log MATCHES "implicit private angle namespace mismatch")
        file(WRITE "${_root}/counterprobe.log" "${_build_log}")
        message(FATAL_ERROR
            "restored implicit -I roots did not fail at the private angle assertion\n"
            "${_build_log}\nBuild tree and counterprobe log retained at ${_root}")
    endif()
    file(REMOVE_RECURSE "${_root}")
    message(STATUS
        "prior implicit -I policy rejected: generated/module private name shadowed "
        "the SDK angle namespace; build tree removed after verification")
    return()
elseif(DEFINED AROS_TEST_EXPECT_FAILURE AND
       AROS_TEST_EXPECT_FAILURE STREQUAL "IMPLICIT_MODULE_ANGLE_OLD_POLICY")
    if(NOT AROS_TEST_TOOLCHAIN STREQUAL "llvm")
        message(FATAL_ERROR "old implicit-DIRS module counterprobe requires llvm mode")
    endif()
elseif(DEFINED AROS_TEST_EXPECT_FAILURE AND
       NOT AROS_TEST_EXPECT_FAILURE STREQUAL "")
    message(FATAL_ERROR
        "unsupported namespace counterprobe: ${AROS_TEST_EXPECT_FAILURE}; "
        "build tree retained at ${_root}")
endif()

foreach(_target IN ITEMS implicit-module-angle-c implicit-module-angle-cpp
        default-c default-cxx noposixc-c noposixc-cxx explicit-posixc-c
        explicit-posixc-cxx explicit-module-angle-c explicit-module-angle-cpp)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        TIMEOUT 180
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(_target MATCHES "^implicit-module-angle-")
        set(_build_log "${_build_stdout}${_build_stderr}")
        if(AROS_TEST_EXPECT_FAILURE STREQUAL "IMPLICIT_MODULE_ANGLE_OLD_POLICY")
            file(REMOVE_RECURSE "${_root}")
            if(_build_result EQUAL 0)
                message(FATAL_ERROR
                    "restored implicit -I roots let the module-only angle header compile")
            endif()
            message(FATAL_ERROR
                "restored implicit -I counterprobe did not compile module_order.h; "
                "the old module-root leak was not reproduced")
        endif()
        if(_build_result EQUAL 0 OR NOT _build_log MATCHES
           "module_order[.]h.*(file not found|No such file or directory)")
            file(WRITE "${_root}/failure-${_target}.log" "${_build_log}")
            message(FATAL_ERROR
                "${AROS_TEST_TOOLCHAIN} implicit module target ${_target} did not "
                "fail specifically for missing module_order.h\n${_build_log}\n"
                "Build tree and failure log retained at ${_root}")
        endif()
    elseif(NOT _build_result EQUAL 0)
        file(WRITE "${_root}/failure-${_target}.log"
            "${_build_stdout}${_build_stderr}")
        message(FATAL_ERROR
            "${AROS_TEST_TOOLCHAIN} namespace fixture target ${_target} failed\n"
            "${_build_stdout}${_build_stderr}\n"
            "Build tree and failure log retained at ${_root}")
    endif()
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "${AROS_TEST_TOOLCHAIN} runtime header namespace compile test passed")
