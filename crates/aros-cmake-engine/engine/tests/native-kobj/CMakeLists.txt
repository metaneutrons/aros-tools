cmake_minimum_required(VERSION 3.22)
project(NativeKobjFixture LANGUAGES C)

if(NOT NATIVE_KOBJ_MODULE_DIR OR NOT IS_DIRECTORY "${NATIVE_KOBJ_MODULE_DIR}")
    message(FATAL_ERROR "NATIVE_KOBJ_MODULE_DIR is required")
endif()
include("${NATIVE_KOBJ_MODULE_DIR}/ModuleMacroContract.cmake")
include("${NATIVE_KOBJ_MODULE_DIR}/NativeKobj.cmake")

set(AROS_DEVELOPER_LIB_DIR "${CMAKE_CURRENT_BINARY_DIR}/lib")
file(MAKE_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
foreach(_library IN ITEMS foo arossupport dos intuition layers graphics oop
        utility expansion keymap deflib)
    add_library("native_archive_${_library}" STATIC archive.c)
    set_target_properties("native_archive_${_library}" PROPERTIES
        OUTPUT_NAME "${_library}"
        ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
endforeach()

foreach(_lane IN ITEMS start asm c cxx user end simple_arch simple_c)
    add_library("obj_${_lane}" OBJECT "${_lane}.c")
endforeach()
add_library(obj_arch OBJECT arch.c arch2.c)

set(_generated_object "${CMAKE_CURRENT_BINARY_DIR}/generated_extra.o")
set_source_files_properties("${_generated_object}" PROPERTIES GENERATED TRUE)
add_custom_command(
    OUTPUT "${_generated_object}"
    COMMAND "${CMAKE_COMMAND}" -E copy
        "${CMAKE_CURRENT_SOURCE_DIR}/generated.bin" "${_generated_object}"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/generated.bin"
    VERBATIM)
set(_duplicate_arch_object "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/obj_arch.dir/arch.c.o")
set_source_files_properties("${_duplicate_arch_object}" PROPERTIES GENERATED TRUE)

set(_collector "${CMAKE_CURRENT_SOURCE_DIR}/fake_collector.py")
set(_linker "${CMAKE_COMMAND}")
set(_nm "${CMAKE_COMMAND}")
set(_objcopy "${CMAKE_COMMAND}")
set(_libdir "${AROS_DEVELOPER_LIB_DIR}")
set(_full_output "${CMAKE_CURRENT_BINARY_DIR}/out/full.kobj")
set(_simple_output "${CMAKE_CURRENT_BINARY_DIR}/out/simple.kobj")

if(NATIVE_KOBJ_CASE STREQUAL "positive")
    aros_record_module_macro(OWNER full_owner FORM full)
    aros_add_native_kobj(
        NAME full_kobj OWNER full_owner FORM full OUTPUT "${_full_output}"
        LINKER "${_linker}" COLLECTOR "${_collector}" NM "${_nm}"
        OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        START_OBJECTS "$<TARGET_OBJECTS:obj_start>"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" "$<TARGET_OBJECTS:obj_arch>"
        ASM_OBJECTS "$<TARGET_OBJECTS:obj_asm>"
        C_OBJECTS "$<TARGET_OBJECTS:obj_c>"
        CXX_OBJECTS "$<TARGET_OBJECTS:obj_cxx>"
        USER_OBJECTS "$<TARGET_OBJECTS:obj_user>" "${_duplicate_arch_object}"
            "${_generated_object}"
        END_OBJECTS "$<TARGET_OBJECTS:obj_end>"
        USELIBS foo arossupport foo
        DEFNAME_LIBS deflib
        KOBJ_FLAGS -m riscvelf_aros --emit-relocs
        LDSCRIPT -T "${CMAKE_CURRENT_SOURCE_DIR}/link.ld"
        USER_FLAGS --no-undefined
        DEPENDS "${_generated_object}" "${_duplicate_arch_object}")

    aros_record_module_macro(OWNER simple_owner FORM simple)
    aros_add_native_kobj(
        NAME simple_kobj OWNER simple_owner FORM simple OUTPUT "${_simple_output}"
        LINKER "${_linker}" COLLECTOR "${_collector}" NM "${_nm}"
        OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_simple_arch>"
        C_OBJECTS "$<TARGET_OBJECTS:obj_simple_c>")
elseif(NATIVE_KOBJ_CASE STREQUAL "missing-contract")
    aros_add_native_kobj(NAME bad OWNER no_contract FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
elseif(NATIVE_KOBJ_CASE STREQUAL "abi-contract")
    aros_record_module_macro(OWNER abi_owner FORM abi-only)
    aros_add_native_kobj(NAME bad OWNER abi_owner FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
elseif(NATIVE_KOBJ_CASE STREQUAL "missing-groups")
    aros_record_module_macro(OWNER empty_owner FORM full)
    aros_add_native_kobj(NAME bad OWNER empty_owner FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}")
elseif(NATIVE_KOBJ_CASE STREQUAL "simple-extra")
    aros_record_module_macro(OWNER simple_owner FORM simple)
    aros_add_native_kobj(NAME bad OWNER simple_owner FORM simple
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>"
        USER_OBJECTS "$<TARGET_OBJECTS:obj_user>")
elseif(NATIVE_KOBJ_CASE STREQUAL "outside-output")
    aros_record_module_macro(OWNER outside_owner FORM full)
    aros_add_native_kobj(NAME bad OWNER outside_owner FORM full
        OUTPUT "${CMAKE_BINARY_DIR}/../escape.kobj"
        LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
elseif(NATIVE_KOBJ_CASE STREQUAL "missing-input")
    aros_record_module_macro(OWNER missing_input FORM full)
    aros_add_native_kobj(NAME bad OWNER missing_input FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "${CMAKE_CURRENT_BINARY_DIR}/missing.o")
elseif(NATIVE_KOBJ_CASE STREQUAL "unsafe-flag")
    aros_record_module_macro(OWNER unsafe_flag FORM full)
    aros_add_native_kobj(NAME bad OWNER unsafe_flag FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" KOBJ_FLAGS -o/tmp/escape)
elseif(NATIVE_KOBJ_CASE STREQUAL "search-override")
    aros_record_module_macro(OWNER search_override FORM full)
    aros_add_native_kobj(NAME bad OWNER search_override FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" KOBJ_FLAGS -L/tmp/redirect)
elseif(NATIVE_KOBJ_CASE STREQUAL "mode-override")
    aros_record_module_macro(OWNER mode_override FORM full)
    aros_add_native_kobj(NAME bad OWNER mode_override FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" KOBJ_FLAGS -shared)
elseif(NATIVE_KOBJ_CASE STREQUAL "script-override")
    aros_record_module_macro(OWNER script_override FORM full)
    aros_add_native_kobj(NAME bad OWNER script_override FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" USER_FLAGS --script=/tmp/other.ld)
elseif(NATIVE_KOBJ_CASE STREQUAL "alternate-library")
    aros_record_module_macro(OWNER alternate_library FORM full)
    aros_add_native_kobj(NAME bad OWNER alternate_library FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" KOBJ_FLAGS --library=foo)
elseif(NATIVE_KOBJ_CASE STREQUAL "unresolved-library")
    aros_record_module_macro(OWNER bad_library FORM full)
    aros_add_native_kobj(NAME bad OWNER bad_library FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" USELIBS not_present)
elseif(NATIVE_KOBJ_CASE STREQUAL "duplicate-owner")
    aros_record_module_macro(OWNER repeated_owner FORM full)
    aros_add_native_kobj(NAME first_kobj OWNER repeated_owner FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
    aros_add_native_kobj(NAME second_kobj OWNER repeated_owner FORM full
        OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/out/second.kobj"
        LINKER "${_linker}" COLLECTOR "${_collector}" NM "${_nm}"
        OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
elseif(NATIVE_KOBJ_CASE STREQUAL "duplicate-output")
    aros_record_module_macro(OWNER first_owner FORM full)
    aros_add_native_kobj(NAME first_kobj OWNER first_owner FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
    aros_record_module_macro(OWNER second_owner FORM full)
    aros_add_native_kobj(NAME second_kobj OWNER second_owner FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>")
elseif(NATIVE_KOBJ_CASE STREQUAL "output-dependency")
    aros_record_module_macro(OWNER output_dependency FORM full)
    file(WRITE "${_full_output}" "dependency")
    aros_add_native_kobj(NAME bad OWNER output_dependency FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" DEPENDS "${_full_output}")
elseif(NATIVE_KOBJ_CASE STREQUAL "stage-dependency")
    aros_record_module_macro(OWNER stage_dependency FORM full)
    set(_stage_dependency "${_full_output}.native-kobj-stage")
    file(WRITE "${_stage_dependency}" "dependency")
    aros_add_native_kobj(NAME bad OWNER stage_dependency FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" DEPENDS "${_stage_dependency}")
elseif(NATIVE_KOBJ_CASE STREQUAL "config-input-collision")
    aros_record_module_macro(OWNER config_input FORM full)
    set(_config_input "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/bad-native-kobj.json")
    file(WRITE "${_config_input}" "input")
    aros_add_native_kobj(NAME bad OWNER config_input FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "$<TARGET_OBJECTS:obj_arch>" DEPENDS "${_config_input}")
elseif(NATIVE_KOBJ_CASE STREQUAL "output-alias")
    aros_record_module_macro(OWNER output_alias FORM full)
    file(WRITE "${_full_output}" "output")
    set(_output_alias "${CMAKE_CURRENT_BINARY_DIR}/out/object-alias.o")
    file(CREATE_LINK "${_full_output}" "${_output_alias}" SYMBOLIC)
    aros_add_native_kobj(NAME bad OWNER output_alias FORM full
        OUTPUT "${_full_output}" LINKER "${_linker}" COLLECTOR "${_collector}"
        NM "${_nm}" OBJCOPY "${_objcopy}" LIBDIR "${_libdir}"
        ARCH_OBJECTS "${_output_alias}")
else()
    message(FATAL_ERROR "Unknown NATIVE_KOBJ_CASE: ${NATIVE_KOBJ_CASE}")
endif()
