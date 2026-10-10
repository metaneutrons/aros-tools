cmake_minimum_required(VERSION 3.22)
project(AlwaysCxxLinkFixture LANGUAGES C CXX)
# This fixture stubs BootstrapSDK; its directory has to come first.
list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
if(TEST_NATIVE_SDK_OBJECTS)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED ON)
endif()
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
    list(GET _expected_cxx_runtime_libraries 3
        AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE)
    if(TEST_GNU_TOOLCHAIN)
        # This is a link-template fixture, not GNU native execution evidence.
        # Use declared names which intentionally have no LLVM aliases.
        set(AROS_TOOLCHAIN gnu)
        set(AROS_LINKER_BIN "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/declared-gnu-ld")
        file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}/bin")
        file(WRITE "${AROS_LINKER_BIN}" "link-template fixture\n")
        set(_expected_cxx_runtime_libraries
            "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libstdc++.a"
            "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libsupc++.a"
            "${AROS_CROSS_TOOLCHAIN_ROOT}/lib/libgcc.a")
        set(AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
            ${_expected_cxx_runtime_libraries})
        list(GET _expected_cxx_runtime_libraries 2
            AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE)
        file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}/lib")
        foreach(_runtime IN LISTS _expected_cxx_runtime_libraries)
            file(WRITE "${_runtime}" "GNU runtime template fixture\n")
        endforeach()
        if(TEST_ESCAPED_GNU_LINKER)
            set(AROS_LINKER_BIN "${CMAKE_BINARY_DIR}/outside-gnu-ld")
            file(WRITE "${AROS_LINKER_BIN}" "escaped fixture\n")
        endif()
    endif()
    get_filename_component(_builtins_dir
        "${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}" DIRECTORY)
    file(MAKE_DIRECTORY "${_builtins_dir}")
    file(WRITE "${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}" "fixture\n")
    if(NOT TEST_NATIVE_SDK_OBJECTS)
        add_custom_target(linklibs-startup)
    endif()
elseif(TEST_DEVELOPMENT_LLD)
    # Select a known executable without depending on which LLVM packages the
    # host image happens to install. The fixture inspects the generated link
    # template and never invokes this spelling as a linker.
    set(AROS_LLD_BIN "${CMAKE_COMMAND}")
else()
    # A defined CMake false constant suppresses find_program while remaining
    # false in the engine's feature branch. This makes the no-LLD fallback a
    # deterministic contract on hosts that do happen to provide ld.lld.
    set(AROS_LLD_BIN FALSE)
endif()
if(TEST_NATIVE_CONSUMER)
    include("${CMAKE_CURRENT_LIST_DIR}/../NativeSourceSelectionFixture.cmake")
    include("${CMAKE_CURRENT_LIST_DIR}/../../NativeConsumerContract.cmake")
    aros_test_prepare_native_consumer_selection("${AROS_SOURCE_DIR}"
        "${CMAKE_CURRENT_BINARY_DIR}/consumer.json")
    aros_validate_native_consumer_contract()
    if(TEST_CONSUMER_STALE_BINDING)
        file(APPEND "${AROS_NATIVE_CONSUMER_CONTRACT}" " ")
    endif()
endif()
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")
if(TEST_NATIVE_CONSUMER AND AROS_NATIVE_BUILD_CONTRACT_VALIDATED)
    message(FATAL_ERROR "SDK consumer acquired native build authority")
endif()

if(TEST_NATIVE_SDK_OBJECTS)
    set(_native_role_stems startup detach cxx-startup)
    if(TEST_NATIVE_MISSING_ROLE)
        if(TEST_NATIVE_MISSING_ROLE STREQUAL "C_STARTUP")
            set(_missing_native_role_stem startup)
        elseif(TEST_NATIVE_MISSING_ROLE STREQUAL "C_DETACH")
            set(_missing_native_role_stem detach)
        elseif(TEST_NATIVE_MISSING_ROLE STREQUAL "CXX_STARTUP")
            set(_missing_native_role_stem cxx-startup)
        else()
            message(FATAL_ERROR
                "unknown TEST_NATIVE_MISSING_ROLE '${TEST_NATIVE_MISSING_ROLE}'")
        endif()
        list(REMOVE_ITEM _native_role_stems "${_missing_native_role_stem}")
    endif()
    set(_native_role_outputs)
    foreach(stem IN LISTS _native_role_stems)
        if(TARGET "aros-c-${stem}-objects" OR TARGET aros-cxx-startup-objects)
            message(FATAL_ERROR "native SDK inherited a legacy bootstrap owner")
        endif()
        aros_compile_sdk_object(
            NAME "source-${stem}"
            SOURCE "${AROS_SOURCE_DIR}/compiler/startup/${stem}.c"
            INTERMEDIATE "${AROS_BUILD_DIR}/gen/compiler/startup/${stem}.o"
            OUTPUT "${AROS_DEVELOPER_LIB_DIR}/${stem}.o"
            LANGUAGE C)
        list(APPEND _native_role_outputs
            "${AROS_DEVELOPER_LIB_DIR}/${stem}.o")
    endforeach()
    aros_sdk_object_group(NAME linklibs-startup
        OBJECTS ${_native_role_outputs})
    aros_bind_source_sdk_program_inputs()
    if(TEST_NATIVE_MISSING_ROLE STREQUAL "C_STARTUP")
        aros_add_program(
            TARGET missing-c-startup-consumer
            MMAKE_ID missing-c-startup-consumer
            DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
            SOURCES module)
        message(FATAL_ERROR
            "native C startup consumer accepted a missing source-owned startup.o")
    elseif(TEST_NATIVE_MISSING_ROLE STREQUAL "C_DETACH")
        aros_add_program(
            TARGET missing-detach-consumer
            MMAKE_ID missing-detach-consumer
            DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
            DETACH
            SOURCES module)
        message(FATAL_ERROR
            "native detached program accepted a missing source-owned detach.o")
    elseif(TEST_NATIVE_MISSING_ROLE STREQUAL "CXX_STARTUP")
        aros_add_module_simple(
            TARGET missing-cxx-module
            MMAKE_ID missing-cxx-module
            DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
            MODTYPE library
            CXX_SOURCES module)
        message(FATAL_ERROR
            "native C++ module accepted a missing source-owned cxx-startup.o")
    endif()
    add_custom_target(source-declared-includes)
    set_property(TARGET source-declared-includes PROPERTY
        AROS_GENERATED_INCLUDE_DIRECTORY "${AROS_BUILD_DIR}/generated-sdk-quote")
    aros_add_target_dependency(linklibs-startup source-declared-includes)
    foreach(stem startup detach cxx-startup)
        get_target_property(dependencies "source-${stem}-compile" MANUALLY_ADDED_DEPENDENCIES)
        get_target_property(options "source-${stem}-compile" COMPILE_OPTIONS)
        if(NOT "source-declared-includes" IN_LIST dependencies OR
           NOT options MATCHES "generated-sdk-quote")
            message(FATAL_ERROR "source SDK object lost its declared header dependency/quote path")
        endif()
    endforeach()
    aros_bind_c_startup_target(linklibs-startup)
endif()

# Header selection comes from compile flags, not a declaration's link policy.
foreach(_header_target IN ITEMS default-headers standard-headers explicit-headers)
    add_library("${_header_target}" OBJECT module.c)
endforeach()
aros_apply_flags(standard-headers COMPILE_OPTIONS -noposixc)
aros_apply_flags(explicit-headers COMPILE_OPTIONS -noposixc)
aros_apply_includes(explicit-headers
    INCLUDES "${AROS_SDK_INCLUDE_DIR}/aros/posixc")
# Relative-runtime providers must not reintroduce an implicit POSIX namespace
# on a consumer whose compile declaration opted out.
add_library(header-provider INTERFACE)
set_property(TARGET header-provider PROPERTY AROS_CLIENT_NAMESPACE_INCLUDES
    "${AROS_SDK_INCLUDE_DIR}/aros/posixc;${AROS_SDK_INCLUDE_DIR}/aros/stdc")
_aros_bind_link_libraries(default-headers header-provider)
_aros_bind_link_libraries(standard-headers header-provider)
foreach(_header_target IN ITEMS default-headers standard-headers explicit-headers)
    file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/${_header_target}.includes"
        CONTENT "$<TARGET_GENEX_EVAL:${_header_target},$<TARGET_PROPERTY:${_header_target},INCLUDE_DIRECTORIES>>")
    file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/${_header_target}.options"
        CONTENT "$<TARGET_GENEX_EVAL:${_header_target},$<TARGET_PROPERTY:${_header_target},COMPILE_OPTIONS>>")
endforeach()

if(TEST_UNSUPPORTED_DETACH)
    aros_add_program(
        TARGET unsupported-detach
        MMAKE_ID unsupported-detach
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        DETACH SOURCES module
        DRIVER_LINK_OPTIONS "-Wl,-T,${CMAKE_CURRENT_LIST_DIR}/unused.ld")
    message(FATAL_ERROR "standalone detached declaration was silently accepted")
endif()

if(TEST_LOCKED_TOOLCHAIN)
    # The fixture uses the host compiler just to prove the custom output rule.
    # It separately checks the release-only direct-link template below.
    if(NOT TEST_NATIVE_SDK_OBJECTS)
        set_property(TARGET aros-cxx-startup-objects PROPERTY COMPILE_OPTIONS "")
    endif()
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
    if(TEST_GNU_TOOLCHAIN)
        # The inventory-bound collector is the toolchain's own front end: it
        # reads the linker from its descriptor and adds the sysroot-owned
        # extras, so it takes the linker arguments directly. aros-collect's
        # `--ld` form produces a module without its resident tag.
        set(_expected_link_prefix
            "^\"[^\"]+\" -r")
        if("${CMAKE_C_LINK_EXECUTABLE}${CMAKE_CXX_LINK_EXECUTABLE}" MATCHES "--ld ")
            message(FATAL_ERROR "GNU native links must not use the direct --ld form")
        endif()
        get_directory_property(_gnu_compile_options COMPILE_OPTIONS)
        if("${_gnu_compile_options}" MATCHES "--target=" OR
           "${CMAKE_CXX_LINK_EXECUTABLE}" MATCHES "libc[+][+]|libunwind|compiler-rt")
            message(FATAL_ERROR "GNU native consumer retained an LLVM-only compile/link contract")
        endif()
    endif()
    if(NOT CMAKE_C_LINK_EXECUTABLE MATCHES "${_expected_link_prefix}" OR
       NOT CMAKE_C_LINK_EXECUTABLE MATCHES
            "--sysroot=.*/SYS/Developer" OR
       NOT CMAKE_C_LINK_EXECUTABLE MATCHES
            "<LINK_LIBRARIES> \"${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}\"" OR
       NOT CMAKE_C_CREATE_SHARED_MODULE MATCHES
            "<LINK_LIBRARIES> \"${AROS_CROSS_TOOLCHAIN_BUILTINS_ARCHIVE}\"" OR
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
elseif(TEST_DEVELOPMENT_LLD)
    if(CMAKE_CXX_LINK_EXECUTABLE MATCHES "<CMAKE_CXX_COMPILER>")
        message(FATAL_ERROR
            "development LLD path unexpectedly retained the C++ driver")
    endif()
    set(_expected_development_link_prefix
        "\"${AROS_COLLECT_BIN}\" --ld \"${CMAKE_COMMAND}\" -- -r")
    string(FIND "${CMAKE_CXX_LINK_EXECUTABLE}"
        "${_expected_development_link_prefix}"
        _development_link_prefix_index)
    if(NOT _development_link_prefix_index EQUAL 0)
        message(FATAL_ERROR
            "development LLD path did not select the collector contract: "
            "${CMAKE_CXX_LINK_EXECUTABLE}")
    endif()
else()
    if(NOT CMAKE_CXX_LINK_EXECUTABLE MATCHES "<CMAKE_CXX_COMPILER>")
        message(FATAL_ERROR
            "development no-LLD fallback did not retain the host C++ driver")
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
aros_add_program(
    TARGET c-program
    MMAKE_ID c-program
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    SOURCES module)
aros_add_program(
    TARGET no-startup-program
    MMAKE_ID no-startup-program
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    NO_STARTUP
    SOURCES module)
aros_add_programs(
    TARGET c-programs
    MMAKE_ID c-programs
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    INSTALL_DIR "${CMAKE_BINARY_DIR}/c-programs"
    SOURCES module)
aros_add_programs(
    TARGET no-startup-programs
    MMAKE_ID no-startup-programs
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    INSTALL_DIR "${CMAKE_BINARY_DIR}/no-startup-programs"
    NO_STARTUP
    SOURCES module)

aros_add_program(
    TARGET detached-program
    MMAKE_ID detached-program
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    DETACH
    SOURCES module)
aros_add_programs(
    TARGET detached-programs
    MMAKE_ID detached-programs
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    INSTALL_DIR "${CMAKE_BINARY_DIR}/detached-programs"
    DETACH
    SOURCES module second)
aros_add_program(
    TARGET custom-detach-program
    MMAKE_ID custom-detach-program
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    NO_STARTUP DETACH
    SOURCES module)
foreach(_program IN ITEMS detached-program detached-programs-module detached-programs-second)
    get_target_property(_libraries "${_program}" LINK_LIBRARIES)
    if(NOT "${AROS_DEVELOPER_LIB_DIR}/detach.o" IN_LIST _libraries)
        message(FATAL_ERROR "${_program} lost declared detach.o: ${_libraries}")
    endif()
    get_target_property(_dependencies "${_program}" MANUALLY_ADDED_DEPENDENCIES)
    if(NOT "${AROS_C_DETACH_TARGET}" IN_LIST _dependencies)
        message(FATAL_ERROR "${_program} lacks the detach.o producer dependency")
    endif()
endforeach()
foreach(_program IN ITEMS c-program custom-detach-program)
    get_target_property(_libraries "${_program}" LINK_LIBRARIES)
    if("${AROS_DEVELOPER_LIB_DIR}/detach.o" IN_LIST _libraries)
        message(FATAL_ERROR "detach.o leaked to ${_program}")
    endif()
endforeach()

# Driver switches belong to the declaration, including every %build_progs
# member. Use a dotted identity to also reject regex/prefix-based matching.
aros_add_programs(
    TARGET static.commands
    MMAKE_ID static.commands
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    INSTALL_DIR "${CMAKE_BINARY_DIR}/static-commands"
    NO_STARTUP
    SOURCES module second)
aros_add_programs(
    TARGET staticXcommands
    MMAKE_ID staticXcommands
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    INSTALL_DIR "${CMAKE_BINARY_DIR}/dynamic-commands"
    NO_STARTUP
    SOURCES module second)
include("${CMAKE_CURRENT_LIST_DIR}/../../DefaultLinkSet.cmake")
foreach(_runtime IN ITEMS posixc stdcio stdc stdc.static exec)
    add_library("fixture-${_runtime}" STATIC module.c)
endforeach()
aros_set_default_link_set(
    "posixc|fixture-posixc|nostdc,noposixc|"
    "stdcio|fixture-stdcio|nostdc|"
    "stdc|fixture-stdc|nostdc|"
    "stdc.static|fixture-stdc.static||nostdc"
    "exec|fixture-exec|nosysbase|")
file(WRITE "${CMAKE_BINARY_DIR}/generated_targets.spec-switches.txt"
    "static.commands\tstatic\tnosysbase\nno-startup-program\tstatic\n")
aros_apply_default_link_set()
foreach(_program IN ITEMS static.commands-module static.commands-second
        no-startup-program)
    get_target_property(_libraries "${_program}" LINK_LIBRARIES)
    if(NOT "fixture-stdc.static" IN_LIST _libraries)
        message(FATAL_ERROR "${_program} lost its static runtime: ${_libraries}")
    endif()
    foreach(_dynamic IN ITEMS fixture-posixc fixture-stdcio fixture-stdc)
        if(_dynamic IN_LIST _libraries)
            message(FATAL_ERROR "${_program} retained ${_dynamic}")
        endif()
    endforeach()
endforeach()
get_target_property(_static_libraries static.commands-module LINK_LIBRARIES)
if("fixture-exec" IN_LIST _static_libraries)
    message(FATAL_ERROR "program group lost nosysbase")
endif()
foreach(_program IN ITEMS staticXcommands-module staticXcommands-second
        c-programs-module)
    get_target_property(_libraries "${_program}" LINK_LIBRARIES)
    if(NOT "fixture-stdc" IN_LIST _libraries OR
       "fixture-stdc.static" IN_LIST _libraries)
        message(FATAL_ERROR "static flag leaked to ${_program}: ${_libraries}")
    endif()
endforeach()

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

# Build-rule identity must not leak into loader-visible module filenames.
# The Wanderer parent-class lookup is a real consumer of this contract.
get_target_property(_hidd_runtime_name hidd-softpipe OUTPUT_NAME)
if(NOT _hidd_runtime_name STREQUAL "softpipe.hidd")
    message(FATAL_ERROR "HIDD runtime name must be softpipe.hidd, got ${_hidd_runtime_name}")
endif()
aros_add_hidd(TARGET softpipe MMAKE_ID foreign-softpipe
    DIRECTORY "${AROS_SOURCE_DIR}/arch/m68k-amiga/hidd/parallel"
    SOURCES "${CMAKE_CURRENT_LIST_DIR}/module.c")
get_target_property(_foreign_hidd_dir foreign-softpipe RUNTIME_OUTPUT_DIRECTORY)
get_target_property(_foreign_hidd_excluded foreign-softpipe AROS_FOREIGN_ARCH)
if(NOT _foreign_hidd_excluded OR NOT _foreign_hidd_dir MATCHES "/gen/foreign-modules/")
    message(FATAL_ERROR "foreign HIDD was not isolated from the native SYS namespace")
endif()
if(TEST_DUPLICATE_HIDD)
    aros_add_hidd(TARGET SoftPipe MMAKE_ID conflicting-hidd
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}" SOURCES module)
endif()
foreach(_case IN ITEMS
        "IconDrawerList|mui|IconDrawerList.mui"
        "IconImage|mcc|IconImage.mcc"
        "IconSettings|mcp|IconSettings.mcp"
        "usb-fixture|usbclass|usb-fixture.class"
        "bluetooth-fixture|btclass|bluetooth-fixture.class"
        "console-fixture|handler|console-fixture-handler"
        "printer-fixture|printer|printer-fixture")
    string(REPLACE "|" ";" _fields "${_case}")
    list(GET _fields 0 _public_name)
    list(GET _fields 1 _modtype)
    list(GET _fields 2 _expected_name)
    set(_build_id "build-id-${_public_name}")
    aros_add_custom_target(TARGET "${_public_name}"
        MMAKE_ID "${_build_id}" MODTYPE "${_modtype}"
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}" SOURCES module)
    get_target_property(_actual_name "${_build_id}" OUTPUT_NAME)
    if(NOT _actual_name STREQUAL _expected_name)
        message(FATAL_ERROR
            "${_build_id}: expected runtime name ${_expected_name}, got ${_actual_name}")
    endif()
endforeach()
file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/mui-runtime-path.txt"
    CONTENT "$<TARGET_FILE:build-id-IconDrawerList>")

# An explicit suffix/directory is a supported namespace distinction, not a
# reason to substitute an arbitrary build id for the public name.
aros_add_custom_target(TARGET IconDrawerList
    MMAKE_ID separate-mui-class MODTYPE mui MODSUFFIX mcc
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}" SOURCES module)
get_target_property(_separate_name separate-mui-class OUTPUT_NAME)
if(NOT _separate_name STREQUAL "IconDrawerList.mcc")
    message(FATAL_ERROR "explicit module suffix was not preserved")
endif()
aros_add_custom_target(TARGET IconDrawerList
    MMAKE_ID separate-mui-directory MODTYPE mui
    INSTALL_DIR "Classes/Other"
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}" SOURCES module)
get_target_property(_separate_dir separate-mui-directory RUNTIME_OUTPUT_DIRECTORY)
if(NOT _separate_dir STREQUAL "${AROS_SYS_DIR}/Classes/Other")
    message(FATAL_ERROR "explicit module runtime namespace was not preserved")
endif()
if(TEST_DUPLICATE_RUNTIME_MODULE)
    aros_add_custom_target(TARGET icondrawerlist
        MMAKE_ID conflicting-class MODTYPE mui
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}" SOURCES module)
endif()
aros_add_module_simple(
    TARGET simple
    MMAKE_ID simple-module
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    MODTYPE library
    ALWAYS_CXX_LINK
    SOURCES module)
get_target_property(_simple_name simple-module OUTPUT_NAME)
if(NOT _simple_name STREQUAL "simple.library")
    message(FATAL_ERROR "simple runtime module lost its loader-visible name: ${_simple_name}")
endif()

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

if(TEST_NATIVE_SDK_OBJECTS)
    set(_c_startup_compile "${AROS_C_STARTUP_TARGET}-compile")
else()
    set(_c_startup_compile aros-c-startup-objects)
endif()
if(NOT TARGET "${AROS_C_STARTUP_TARGET}" OR NOT TARGET "${_c_startup_compile}")
    message(FATAL_ERROR "AROS program startup producer targets are missing")
endif()
get_target_property(_c_startup_sources "${_c_startup_compile}" SOURCES)
if(NOT "${_c_startup_sources}" MATCHES "compiler/startup/startup[.]c")
    message(FATAL_ERROR "AROS program startup uses the wrong source: ${_c_startup_sources}")
endif()
foreach(_program IN ITEMS c-program cxx-program c-programs-module)
    _assert_manual_dependency("${_program}" "${AROS_C_STARTUP_TARGET}" TRUE)
    get_target_property(_program_libraries "${_program}" LINK_LIBRARIES)
    if(NOT "${_aros_c_startup_output}" IN_LIST _program_libraries)
        message(FATAL_ERROR "${_program} did not link explicit startup.o")
    endif()
endforeach()
foreach(_no_startup IN ITEMS hidd-c-default no-startup-program
        no-startup-programs-module)
    _assert_manual_dependency("${_no_startup}" "${AROS_C_STARTUP_TARGET}" FALSE)
    get_target_property(_libraries "${_no_startup}" LINK_LIBRARIES)
    if("${_aros_c_startup_output}" IN_LIST _libraries)
        message(FATAL_ERROR "${_no_startup} unexpectedly links startup.o")
    endif()
endforeach()

if(TEST_LOCKED_TOOLCHAIN)
    if(TEST_NATIVE_SDK_OBJECTS)
        set(_cxx_startup_compile "${AROS_CXX_STARTUP_TARGET}-compile")
    else()
        set(_cxx_startup_compile aros-cxx-startup-objects)
    endif()
    if(NOT TARGET "${AROS_CXX_STARTUP_TARGET}" OR
       NOT TARGET "${_cxx_startup_compile}")
        message(FATAL_ERROR
            "locked release did not create the cxx-startup producer targets")
    endif()
    get_target_property(_cxx_startup_sources "${_cxx_startup_compile}" SOURCES)
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
        _assert_manual_dependency("${_cxx_target}" "${AROS_CXX_STARTUP_TARGET}" TRUE)
    endforeach()
    _assert_manual_dependency(hidd-c-default "${AROS_CXX_STARTUP_TARGET}" FALSE)
endif()
