cmake_minimum_required(VERSION 3.22)
project(AlwaysCxxLinkFixture LANGUAGES C CXX)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
if(TEST_LOCKED_TOOLCHAIN)
    set(AROS_CROSS_TOOLCHAIN_ROOT "${CMAKE_BINARY_DIR}/release-toolchain")
    set(AROS_TARGET_PROFILE pc-x86_64)
    set(AROS_TARGET_TRIPLE x86_64-unknown-aros)
    set(AROS_LLD_BIN "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/ld.lld")
    set(_expected_cxx_runtime_libraries
        "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libc++.a"
        "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libc++abi.a"
        "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libunwind.a"
        "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/clang/11.0.0/lib/aros/libclang_rt.builtins-x86_64.a")
    set(AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
        ${_expected_cxx_runtime_libraries})
    add_custom_target(linklibs-startup)
endif()
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

if(TEST_LOCKED_TOOLCHAIN)
    # The fixture uses the host compiler just to prove the custom output rule.
    # It separately checks the release-only direct-link template below.
    set_property(TARGET aros-cxx-startup-objects PROPERTY COMPILE_OPTIONS "")
    aros_bind_cxx_startup_target("linklibs-startup")
    # Every link goes through aros-collect, which runs the prefix-owned ld.lld
    # twice; the second pass is what turns the .aros.set.* sections into the
    # arrays the code reads. The rule used to start with ld.lld directly, and
    # this expectation still said so, which is why this fixture was red from
    # the day that rule changed (OPEN-POINTS 45). What matters here is
    # unchanged: the locked release links with its own linker, under its own
    # sysroot, partially.
    set(_expected_link_prefix
        "^\"[^\"]*/aros-collect\" --ld \"${AROS_CROSS_TOOLCHAIN_ROOT}/bin/ld[.]lld\" -- -r")
    if(NOT CMAKE_C_LINK_EXECUTABLE MATCHES "${_expected_link_prefix}" OR
       NOT CMAKE_C_LINK_EXECUTABLE MATCHES
            "--sysroot=.*/SYS/Developer" OR
       NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES "${_expected_link_prefix}" OR
       NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES
            "--sysroot=.*/SYS/Developer" OR
       NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES
            "\".*/SYS/Developer/lib/cxx-startup[.]o\" <OBJECTS>" OR
       NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES "--start-group" OR
       NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES "--end-group")
        message(FATAL_ERROR
            "locked release did not select exact C/C++ partial-link rules\n"
            "C: ${CMAKE_C_LINK_EXECUTABLE}\nCXX: ${CMAKE_CXX_LINK_EXECUTABLE}")
    endif()
    foreach(_forbidden_driver_fragment IN ITEMS
            "<CMAKE_CXX_COMPILER>" "collect-aros" "-nostdlib"
            "-nostartfiles" "-Wl,")
        string(FIND "${CMAKE_CXX_LINK_EXECUTABLE}" "${_forbidden_driver_fragment}"
            _forbidden_driver_fragment_index)
        if(NOT _forbidden_driver_fragment_index EQUAL -1)
            message(FATAL_ERROR
                "locked release C++ partial-link rule retained driver fragment "
                "${_forbidden_driver_fragment}")
        endif()
    endforeach()
    string(FIND "${CMAKE_CXX_LINK_EXECUTABLE}" "<LINK_LIBRARIES>"
        _project_libraries_index)
    if(_project_libraries_index LESS 0)
        message(FATAL_ERROR
            "locked release C++ partial-link rule lost <LINK_LIBRARIES>")
    endif()
    foreach(_archive IN LISTS _expected_cxx_runtime_libraries)
        string(FIND "${CMAKE_CXX_LINK_EXECUTABLE}" "\"${_archive}\""
            _archive_index)
        if(_archive_index LESS 0 OR
           _archive_index LESS _project_libraries_index)
            message(FATAL_ERROR
                "locked release C++ partial-link rule did not append ${_archive} "
                "after project link libraries")
        endif()
    endforeach()
    foreach(_forbidden_default IN ITEMS -lmui -lposixc -lamiga -lstdc)
        string(FIND "${CMAKE_CXX_LINK_EXECUTABLE}" "${_forbidden_default}"
            _forbidden_default_index)
        if(NOT _forbidden_default_index EQUAL -1)
            message(FATAL_ERROR
                "locked release C++ partial-link rule retained driver default "
                "${_forbidden_default}")
        endif()
    endforeach()
    get_directory_property(_locked_compile_options COMPILE_OPTIONS)
    if(NOT "--sysroot=${AROS_DEVELOPER_DIR}" IN_LIST _locked_compile_options)
        message(FATAL_ERROR
            "locked release did not pass its consumer-owned Developer sysroot")
    endif()
else()
    if(CMAKE_CXX_LINK_EXECUTABLE MATCHES "<CMAKE_CXX_COMPILER>")
        message(FATAL_ERROR
            "development fallback unexpectedly selected the AROS C++ driver")
    endif()
endif()

# These use plain C source on purpose.  The `alwayscxxlink` contract must set
# the linker language independently of CMake's source-language inference.
aros_add_hidd(
    TARGET softpipe
    MMAKE_ID hidd-softpipe
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_hidd(
    TARGET nouveau
    MMAKE_ID hidd-nouveau
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_hidd(
    TARGET c-default
    MMAKE_ID hidd-c-default
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    SOURCES module)

# A declared C++ lane selects the C++ linker even without alwayscxxlink=yes.
aros_add_hidd(
    TARGET cxx-default
    MMAKE_ID hidd-cxx-default
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    CXX_SOURCES module)
aros_add_program(
    TARGET cxx-program
    MMAKE_ID cxx-program
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    CXX_SOURCES module)
aros_add_programs(
    TARGET cxx-programs
    MMAKE_ID cxx-programs
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    CXX_SOURCES module)

# Cover the shared full-module, custom-module and simple-module paths too.
aros_add_library(
    TARGET mesa
    MMAKE_ID mesa-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_device(
    TARGET device
    MMAKE_ID device-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_resource(
    TARGET resource
    MMAKE_ID resource-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_custom_target(
    TARGET custom
    MMAKE_ID custom-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    MODTYPE hook
    ALWAYS_CXX_LINK
    SOURCES module)
aros_add_module_simple(
    TARGET simple
    MMAKE_ID simple-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    MODTYPE library
    ALWAYS_CXX_LINK
    SOURCES module)

function(_assert_linker_language target expected)
    get_target_property(_actual "${target}" LINKER_LANGUAGE)
    if(NOT "${_actual}" STREQUAL "${expected}")
        message(FATAL_ERROR
            "${target}: expected linker language ${expected}, got ${_actual}")
    endif()
endfunction()

function(_assert_manual_dependency target dependency expected)
    get_target_property(_dependencies "${target}" MANUALLY_ADDED_DEPENDENCIES)
    if(_dependencies STREQUAL "_dependencies-NOTFOUND")
        set(_dependencies "")
    endif()
    list(FIND _dependencies "${dependency}" _dependency_index)
    if(expected AND _dependency_index EQUAL -1)
        message(FATAL_ERROR
            "${target}: expected explicit dependency on ${dependency}, got "
            "${_dependencies}")
    elseif(NOT expected AND NOT _dependency_index EQUAL -1)
        message(FATAL_ERROR
            "${target}: unexpectedly depends on ${dependency}")
    endif()
endfunction()

foreach(_target IN ITEMS
        hidd-softpipe hidd-nouveau mesa-module device-module resource-module
        custom-module simple-module hidd-cxx-default cxx-program
        cxx-programs-module)
    _assert_linker_language("${_target}" CXX)
endforeach()
_assert_linker_language(hidd-c-default C)

if(TEST_LOCKED_TOOLCHAIN)
    if(NOT TARGET aros-cxx-startup OR
       NOT TARGET aros-cxx-startup-objects)
        message(FATAL_ERROR
            "locked release did not create the cxx-startup producer targets")
    endif()
    get_target_property(_cxx_startup_sources aros-cxx-startup-objects SOURCES)
    if(NOT "${_cxx_startup_sources}" MATCHES
            "compiler/startup/cxx-startup[.]c")
        message(FATAL_ERROR
            "locked release cxx-startup target does not use the real source: "
            "${_cxx_startup_sources}")
    endif()
    foreach(_cxx_target IN ITEMS
            hidd-softpipe hidd-nouveau mesa-module device-module resource-module
            custom-module simple-module hidd-cxx-default cxx-program
            cxx-programs-module linklibs-startup)
        _assert_manual_dependency("${_cxx_target}" aros-cxx-startup TRUE)
    endforeach()
    _assert_manual_dependency(hidd-c-default aros-cxx-startup FALSE)
endif()
