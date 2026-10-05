cmake_minimum_required(VERSION 3.22)
project(NativeCoreFixture LANGUAGES C)

if(NOT DEFINED AROS_TEST_ENGINE_DIR OR NOT DEFINED NATIVE_CORE_LINKER)
    message(FATAL_ERROR "native-core fixture requires engine and linker paths")
endif()

list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_LIST_DIR}")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_LLD_BIN "${NATIVE_CORE_LINKER}")
set(AROS_CMAKE_ENGINE_DIR "${AROS_TEST_ENGINE_DIR}")
include("${AROS_TEST_ENGINE_DIR}/AROS.cmake")
include("${AROS_TEST_ENGINE_DIR}/NativeCore.cmake")
include("${AROS_TEST_ENGINE_DIR}/ModuleMacroContract.cmake")
include("${AROS_TEST_ENGINE_DIR}/NativeKobj.cmake")

set(_case "${NATIVE_CORE_CASE}")
if(_case MATCHES "^source-bridge-")
    include("${AROS_TEST_ENGINE_DIR}/SourceNativeKobj.cmake")
    set(AROS_NATIVE_BUILD_CONTRACT fixture-source-contract)
    aros_record_module_macro(OWNER module-kernel FORM full)
    if(NOT _case STREQUAL "source-bridge-missing-metadata")
        set(_source_kobj_json [=[{"defname":"module-kernel","declaration_line":0,"scoped_declaration_line":0,"included_make_opts":[],"user_objects":{"state":"known_empty","raw":null,"source":[]},"defname_libs":{"state":"known_empty","raw":null,"source":[]},"user_ldflags":{"state":"known_empty","raw":null,"source":[]},"use_libs":{"state":"known_empty","raw":null,"source":[]},"kobj_ldflags":{"state":"known_empty","raw":null,"source":[]},"kernel_kobj_ldscript":{"state":"known_empty","raw":null,"source":[]},"funcinstr_libs":{"state":"known_empty","raw":null,"source":[]},"function_instrumentation":{"state":"exact","raw":"no","words":["no"],"source":[]} }]=])
        if(_case STREQUAL "source-bridge-unresolved-kobj-flags")
            string(JSON _source_kobj_json SET "${_source_kobj_json}" kobj_ldflags
                [=[{"state":"unresolved","raw":null,"reason":"fixture dynamic KOBJ flags","source":[]}]=])
        elseif(_case STREQUAL "source-bridge-function-instrumentation")
            string(JSON _source_kobj_json SET "${_source_kobj_json}" function_instrumentation raw "\"yes\"")
            string(JSON _source_kobj_json SET "${_source_kobj_json}" function_instrumentation words "[\"yes\"]")
        endif()
        aros_record_module_kobj_inputs(OWNER module-kernel JSON "${_source_kobj_json}")
    endif()
    aros_add_source_native_kobj(
        NAME source-native-kobj OWNER module-kernel
        OUTPUT "${CMAKE_BINARY_DIR}/out/source.kobj"
        LINKER "${NATIVE_CORE_LINKER}"
        COLLECTOR "${CMAKE_CURRENT_LIST_DIR}/fake_collector.py"
        NM "${CMAKE_COMMAND}" OBJCOPY "${CMAKE_COMMAND}"
        LIBDIR "${CMAKE_BINARY_DIR}/lib")
    message(FATAL_ERROR "source-native KOBJ bridge case unexpectedly configured: ${_case}")
endif()

file(REAL_PATH "${NATIVE_CORE_LINKER}" _native_core_real_linker)
get_filename_component(_native_core_real_linker_name "${_native_core_real_linker}" NAME)
set(_native_core_linker "${CMAKE_BINARY_DIR}/fixture-ld.lld")
if(_native_core_real_linker_name STREQUAL "lld")
    # Homebrew resolves ld.lld to the generic lld executable. Restore the
    # GNU flavor explicitly because NativeKobj canonicalizes tool paths.
    set(_native_core_linker_prefix "-flavor gnu ")
else()
    set(_native_core_linker_prefix "")
endif()
file(WRITE "${_native_core_linker}"
    "#!/bin/sh\nexec \"${_native_core_real_linker}\" ${_native_core_linker_prefix}\"$@\"\n")
file(CHMOD "${_native_core_linker}" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
    WORLD_READ WORLD_EXECUTE)

set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/lib")
file(MAKE_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
foreach(_library IN ITEMS dos intuition layers graphics oop utility expansion keymap)
    add_library("native-core-${_library}" STATIC archive.c)
    set_target_properties("native-core-${_library}" PROPERTIES
        OUTPUT_NAME "${_library}"
        ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
endforeach()

add_executable(module-kernel EXCLUDE_FROM_ALL module.c)
add_executable(module-task EXCLUDE_FROM_ALL task.c)
set_target_properties(module-kernel module-task PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/runtime-modules")
add_library(module-kernel-kobj-c OBJECT EXCLUDE_FROM_ALL module.c)
add_library(module-kernel-kobj-user OBJECT EXCLUDE_FROM_ALL external.c)
add_library(module-task-kobj-c OBJECT EXCLUDE_FROM_ALL task.c)
foreach(_owner IN ITEMS module-kernel module-task)
    aros_record_module_macro(OWNER "${_owner}" FORM full)
endforeach()
set_property(TARGET module-kernel PROPERTY AROS_CANONICAL_RUNTIME_NAME kernel.resource)
set_property(TARGET module-task PROPERTY AROS_CANONICAL_RUNTIME_NAME task.resource)
add_library(linklibs-core STATIC linklib.c)
set_target_properties(linklibs-core PROPERTIES
    OUTPUT_NAME coredep
    ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")

if(_case STREQUAL "foreign-module" OR _case STREQUAL "resolve-foreign-module")
    set_property(TARGET module-kernel PROPERTY AROS_FOREIGN_ARCH TRUE)
endif()

if(_case STREQUAL "resolve-ambiguous-module")
    add_executable(module-kernel-copy EXCLUDE_FROM_ALL module.c)
    set_property(TARGET module-kernel-copy PROPERTY AROS_CANONICAL_RUNTIME_NAME kernel.resource)
endif()
if(_case STREQUAL "resolve-private-module")
    add_executable(module-kernel-copy EXCLUDE_FROM_ALL module.c)
    set_property(TARGET module-kernel-copy PROPERTY AROS_CANONICAL_RUNTIME_NAME kernel.resource)
    set_property(TARGET module-kernel-copy PROPERTY AROS_PRIVATE_RUNTIME_ALTERNATIVE TRUE)
elseif(_case STREQUAL "resolve-private-only-module")
    set_property(TARGET module-kernel PROPERTY AROS_PRIVATE_RUNTIME_ALTERNATIVE TRUE)
endif()
if(_case STREQUAL "resolve-ambiguous-archive")
    add_library(linklibs-core-copy STATIC linklib.c)
    set_target_properties(linklibs-core-copy PROPERTIES
        OUTPUT_NAME coredep
        ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
elseif(_case STREQUAL "resolve-private-archive")
    add_library(linklibs-core-private STATIC linklib.c)
    set_target_properties(linklibs-core-private PROPERTIES
        OUTPUT_NAME coredep
        ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}"
        AROS_PRIVATE_RUNTIME_ALTERNATIVE TRUE)
elseif(_case STREQUAL "resolve-private-only-archive")
    set_property(TARGET linklibs-core PROPERTY AROS_PRIVATE_RUNTIME_ALTERNATIVE TRUE)
elseif(_case STREQUAL "resolve-noncanonical-archive")
    add_library(linklibs-core-copy STATIC linklib.c)
    set_target_properties(linklibs-core-copy PROPERTIES
        OUTPUT_NAME coredep
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/other-libraries")
elseif(_case STREQUAL "resolve-noncanonical-only-archive")
    set_property(TARGET linklibs-core PROPERTY
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/other-libraries")
endif()
if(_case STREQUAL "resolve-foreign-archive")
    set_property(TARGET linklibs-core PROPERTY AROS_FOREIGN_ARCH TRUE)
endif()

if(_case STREQUAL "resolve-missing-module")
    aros_resolve_native_core_module(_resolved absent.resource)
elseif(_case STREQUAL "resolve-foreign-module")
    aros_resolve_native_core_module(_resolved kernel.resource)
elseif(_case STREQUAL "resolve-ambiguous-module")
    aros_resolve_native_core_module(_resolved kernel.resource)
elseif(_case STREQUAL "resolve-ambiguous-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
elseif(_case STREQUAL "resolve-foreign-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
elseif(_case STREQUAL "resolve-private-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
    message(STATUS "resolved=${_resolved}")
elseif(_case STREQUAL "resolve-private-only-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
elseif(_case STREQUAL "resolve-noncanonical-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
    message(STATUS "resolved=${_resolved}")
elseif(_case STREQUAL "resolve-noncanonical-only-archive")
    aros_resolve_native_core_linklib(_resolved coredep)
elseif(_case STREQUAL "resolve-private-module")
    aros_resolve_native_core_module(_resolved kernel.resource)
    message(STATUS "resolved=${_resolved}")
elseif(_case STREQUAL "resolve-private-only-module")
    aros_resolve_native_core_module(_resolved kernel.resource)
endif()

if(_case STREQUAL "no-sources")
    add_executable(module-empty EXCLUDE_FROM_ALL)
    set_property(TARGET module-empty PROPERTY AROS_CANONICAL_RUNTIME_NAME empty.resource)
endif()

separate_arguments(_fixture_compile_flags NATIVE_COMMAND "${CMAKE_C_FLAGS}")

if(_case STREQUAL "late-generated-dependency")
    set(_late_header "${CMAKE_BINARY_DIR}/native-core-generated.h")
    set(_late_dependency_marker "${CMAKE_BINARY_DIR}/late-generated-dependency.marker")
    add_custom_command(
        OUTPUT "${_late_header}" "${_late_dependency_marker}"
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                "${CMAKE_CURRENT_LIST_DIR}/late-generated.h" "${_late_header}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_late_dependency_marker}"
        COMMENT "Creating late generated-header dependency marker"
        DEPENDS "${CMAKE_CURRENT_LIST_DIR}/late-generated.h"
        VERBATIM)
    add_custom_target(native-core-late-generated-header
        DEPENDS "${_late_header}" "${_late_dependency_marker}")
    target_include_directories(module-kernel-kobj-c PRIVATE "${CMAKE_BINARY_DIR}")
    target_compile_definitions(module-kernel-kobj-c PRIVATE NATIVE_CORE_TEST_GENERATED_HEADER)
    # Attach generated-input ordering to the source OBJECT group before its
    # KOBJ target is declared. The registered KOBJ consumes this group directly;
    # there is no runtime-object mirror to refresh later.
    add_dependencies(module-kernel-kobj-c native-core-late-generated-header)
endif()

set(_builtins "${CMAKE_BINARY_DIR}/toolchain/libbuiltins.a")
if(_case STREQUAL "missing-builtins")
    set(_builtins "${CMAKE_BINARY_DIR}/toolchain/missing.a")
else()
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/toolchain")
    execute_process(
        COMMAND "${CMAKE_C_COMPILER}" ${_fixture_compile_flags}
                -c "${CMAKE_CURRENT_LIST_DIR}/builtins.c"
                -o "${CMAKE_BINARY_DIR}/fixture-builtins.o"
        RESULT_VARIABLE _builtins_compile_result
        OUTPUT_VARIABLE _builtins_compile_stdout
        ERROR_VARIABLE _builtins_compile_stderr)
    if(NOT _builtins_compile_result EQUAL 0)
        message(FATAL_ERROR "could not compile builtin fixture: ${_builtins_compile_stdout}${_builtins_compile_stderr}")
    endif()
    execute_process(
        COMMAND "${CMAKE_AR}" rcs "${_builtins}" "${CMAKE_BINARY_DIR}/fixture-builtins.o"
        RESULT_VARIABLE _builtins_ar_result
        OUTPUT_VARIABLE _builtins_ar_stdout
        ERROR_VARIABLE _builtins_ar_stderr)
    if(NOT _builtins_ar_result EQUAL 0)
        message(FATAL_ERROR "could not archive builtin fixture: ${_builtins_ar_stdout}${_builtins_ar_stderr}")
    endif()
    if(_case STREQUAL "invalid-builtins")
        file(WRITE "${_builtins}" "not an archive\n")
    elseif(_case STREQUAL "thin-builtins")
        file(WRITE "${_builtins}" "!<thin>\n")
    endif()
endif()

set(_script "${CMAKE_CURRENT_LIST_DIR}/kernel.ld")
if(_case STREQUAL "missing-script")
    set(_script "${CMAKE_CURRENT_LIST_DIR}/missing.ld")
endif()
set(_linker "${_native_core_linker}")
if(_case STREQUAL "missing-linker")
    set(_linker "${CMAKE_BINARY_DIR}/missing-ld.lld")
elseif(_case STREQUAL "nonexecutable-linker")
    set(_linker "${CMAKE_BINARY_DIR}/not-an-executable-linker")
    file(WRITE "${_linker}" "not executable\n")
elseif(_case STREQUAL "linker-timeout")
    set(_linker "${CMAKE_BINARY_DIR}/slow-linker")
    file(WRITE "${_linker}" "#!/bin/sh\nexec /bin/sleep 30\n")
    file(CHMOD "${_linker}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
endif()

set(_collector "${CMAKE_CURRENT_LIST_DIR}/fake_collector.py")
set(_kobj_dir "${CMAKE_BINARY_DIR}/gen/native-core")
aros_add_native_kobj(
    NAME native-kobj-module-kernel OWNER module-kernel FORM full
    OUTPUT "${_kobj_dir}/module-kernel.kobj"
    LINKER "${_native_core_linker}" COLLECTOR "${_collector}"
    NM "${CMAKE_COMMAND}" OBJCOPY "${CMAKE_COMMAND}"
    LIBDIR "${AROS_DEVELOPER_LIB_DIR}"
    C_OBJECTS "$<TARGET_OBJECTS:module-kernel-kobj-c>"
    USER_OBJECTS "$<TARGET_OBJECTS:module-kernel-kobj-user>")
if(NOT _case STREQUAL "missing-kobj")
    aros_add_native_kobj(
        NAME native-kobj-module-task OWNER module-task FORM full
        OUTPUT "${_kobj_dir}/module-task.kobj"
        LINKER "${_native_core_linker}" COLLECTOR "${_collector}"
        NM "${CMAKE_COMMAND}" OBJCOPY "${CMAKE_COMMAND}"
        LIBDIR "${AROS_DEVELOPER_LIB_DIR}"
        C_OBJECTS "$<TARGET_OBJECTS:module-task-kobj-c>")
endif()
if(_case STREQUAL "wrong-owner-kobj")
    set_property(GLOBAL PROPERTY AROS_NATIVE_KOBJ_TARGET_module-task
        native-kobj-module-kernel)
elseif(_case STREQUAL "unregistered-kobj")
    get_property(_registered_owners GLOBAL PROPERTY AROS_NATIVE_KOBJ_OWNERS)
    list(REMOVE_ITEM _registered_owners module-task)
    set_property(GLOBAL PROPERTY AROS_NATIVE_KOBJ_OWNERS "${_registered_owners}")
endif()

set(_module_ids module-kernel module-task)
if(_case STREQUAL "missing-module")
    set(_module_ids module-absent)
elseif(_case STREQUAL "duplicate-module")
    set(_module_ids module-kernel module-kernel)
elseif(_case STREQUAL "no-sources")
    set(_module_ids module-empty)
endif()
set(_linklib_ids linklibs-core)
if(_case STREQUAL "nonarchive-provider")
    set(_linklib_ids module-task)
elseif(_case STREQUAL "noncanonical-provider-direct")
    set_property(TARGET linklibs-core PROPERTY
        ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/other-libraries")
elseif(_case STREQUAL "private-archive-provider")
    set_property(TARGET linklibs-core PROPERTY AROS_PRIVATE_RUNTIME_ALTERNATIVE TRUE)
elseif(_case STREQUAL "foreign-archive-provider")
    set_property(TARGET linklibs-core PROPERTY AROS_FOREIGN_ARCH TRUE)
endif()

set(_output "${CMAKE_BINARY_DIR}/gen/native-core/core.elf")
if(_case STREQUAL "unsafe-output")
    set(_output "${CMAKE_BINARY_DIR}/../outside.elf")
elseif(_case STREQUAL "symlink-output")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/gen")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/../outside-native-core")
    file(CREATE_LINK "${CMAKE_BINARY_DIR}/../outside-native-core" "${CMAKE_BINARY_DIR}/gen/escape" SYMBOLIC)
    set(_output "${CMAKE_BINARY_DIR}/gen/escape/core.elf")
endif()

aros_add_native_core(
    NAME native-core-fixture
    OUTPUT "${_output}"
    LINKER_SCRIPT "${_script}"
    MODULES ${_module_ids}
    LINKLIBS ${_linklib_ids}
    BUILTINS "${_builtins}"
    LINKER "${_linker}")

if(_case STREQUAL "duplicate-target")
    aros_add_native_core(
        NAME native-core-fixture
        OUTPUT "${CMAKE_BINARY_DIR}/gen/native-core/second.elf"
        LINKER_SCRIPT "${_script}"
        MODULES module-kernel module-task
        LINKLIBS linklibs-core
        BUILTINS "${_builtins}"
        LINKER "${_linker}")
endif()

file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/native-core.properties"
    CONTENT "output=$<TARGET_PROPERTY:native-core-fixture,AROS_NATIVE_CORE_OUTPUT>\nmap=$<TARGET_PROPERTY:native-core-fixture,AROS_NATIVE_CORE_MAP>\nmembers=$<TARGET_PROPERTY:native-core-fixture,AROS_NATIVE_CORE_MEMBER_IDS>\nkobjtargets=$<TARGET_PROPERTY:native-core-fixture,AROS_NATIVE_CORE_KOBJ_TARGETS>\nkobjfiles=$<TARGET_PROPERTY:native-core-fixture,AROS_NATIVE_CORE_KOBJ_FILES>\n")
