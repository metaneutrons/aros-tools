include_guard(GLOBAL)

# Make hashes GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS) of the
# VC4 consumer into the table, so gca_bind refuses a module built with other
# layout-relevant flags. This build compiles the consumer with its own target,
# ISA flags and reviewed defines, which differ from Make's (#353). Hash exactly
# those, in their declared order, and leave paths such as the sysroot out.
function(_aros_mesa26_gca_abi_flags out_var compiler_target c_flags defines)
    if(defines STREQUAL "")
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI ABI flags need the consumer defines")
    endif()
    set(_flags "")
    if(NOT compiler_target STREQUAL "")
        list(APPEND _flags "--target=${compiler_target}")
    endif()
    separate_arguments(_isa_flags UNIX_COMMAND "${c_flags}")
    list(APPEND _flags ${_isa_flags})
    foreach(_define IN LISTS defines)
        list(APPEND _flags "-D${_define}")
    endforeach()
    list(JOIN _flags " " _flags)
    set(${out_var} "${_flags}" PARENT_SCOPE)
endfunction()

# Mesa 26's ARM pipe HIDDs live in separate modules from mesa3dgl.library.
# Their imports must use one generated, versioned provider table. This is a
# closed source capability, not a generic invocation of galliumglue.py.
function(aros_build_mesa26_gallium_core_api)
    if(NOT TARGET linklibs-gallium_vc4)
        return()
    endif()

    get_target_property(_vc4_defines linklibs-gallium_vc4 COMPILE_DEFINITIONS)
    if(NOT "AROS_MESA_MAJOR=26" IN_LIST _vc4_defines)
        return()
    endif()
    if(NOT AROS_TARGET_CPU STREQUAL "arm" AND
       NOT AROS_TARGET_CPU STREQUAL "aarch64")
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI has no ABI for ${AROS_TARGET_CPU}")
    endif()

    set(_required_targets
        linklibs-gallium_vc4
        mesa3d-linklib-compiler
        mesa3d-linklib-galliumauxiliary
        mesa3d-linklib-mesautil
        mesa3d-linklib-mesa
        mesa3d-linklib-glapi
        mesa3d-linklib-mesadevutil
        mesa3dgl-library
        hidd-vc4gallium)
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        list(APPEND _required_targets linklibs-gallium_v3d hidd-v3d)
    endif()
    foreach(_target IN LISTS _required_targets)
        if(NOT TARGET "${_target}")
            message(FATAL_ERROR "Mesa 26 GalliumCoreAPI requires ${_target}")
        endif()
    endforeach()

    set(_source_script
        "${AROS_SOURCE_DIR}/workbench/libs/mesa/galliumglue.py")
    set(_source_patch
        "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff")
    foreach(_source IN ITEMS "${_source_script}" "${_source_patch}")
        if(NOT EXISTS "${_source}" OR IS_DIRECTORY "${_source}")
            message(FATAL_ERROR "Mesa 26 GalliumCoreAPI input is missing: ${_source}")
        endif()
    endforeach()
    file(SHA256 "${_source_script}" _script_sha)
    if(NOT _script_sha STREQUAL
       "02ff4dcc8c3395fa3e7ab3a2f005ef0b2e6c3b777b316bb7d0a8bff5a0a26e19")
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI generator changed without review")
    endif()
    file(SHA256 "${_source_patch}" _patch_sha)
    if(NOT _patch_sha STREQUAL
       "463c7b7930e02b9b44ba0edf893357eec50184f6634cef0f389f503780b64ff2")
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI ABI patch changed without review")
    endif()

    _aros_mesa26_gca_abi_flags(_abi_flags "${CMAKE_C_COMPILER_TARGET}"
        "${CMAKE_C_FLAGS}" "${_vc4_defines}")

    find_package(Python3 COMPONENTS Interpreter QUIET)
    if(NOT Python3_Interpreter_FOUND OR NOT Python3_EXECUTABLE)
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI requires Python 3")
    endif()
    execute_process(COMMAND "${Python3_EXECUTABLE}" -c
        "import sys; raise SystemExit(0 if sys.version_info.major == 3 else 1)"
        RESULT_VARIABLE _python_probe OUTPUT_QUIET ERROR_QUIET TIMEOUT 10)
    if(NOT "${_python_probe}" STREQUAL "0")
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI Python 3 is unusable")
    endif()
    if(NOT AROS_CROSS_TOOLCHAIN_ROOT OR
       NOT AROS_CROSS_TOOLCHAIN_TREE_SHA256)
        message(FATAL_ERROR "Mesa 26 GalliumCoreAPI requires a verified toolchain")
    endif()
    get_filename_component(_tool_bin
        "${AROS_CROSS_TOOLCHAIN_ROOT}/bin" REALPATH)
    foreach(_tool IN ITEMS CMAKE_AR CMAKE_NM CMAKE_OBJCOPY)
        if(NOT EXISTS "${${_tool}}" OR IS_DIRECTORY "${${_tool}}")
            message(FATAL_ERROR "Mesa 26 GalliumCoreAPI lacks ${_tool}")
        endif()
        get_filename_component(_actual_bin "${${_tool}}" DIRECTORY)
        get_filename_component(_actual_bin "${_actual_bin}" REALPATH)
        if(NOT _actual_bin STREQUAL _tool_bin)
            message(FATAL_ERROR
                "Mesa 26 GalliumCoreAPI ${_tool} is outside the verified toolchain")
        endif()
        file(SHA256 "${${_tool}}" _tool_sha)
        set(_${_tool}_sha "${_tool_sha}")
    endforeach()

    set(_providers
        mesa3d-linklib-compiler
        mesa3d-linklib-galliumauxiliary
        mesa3d-linklib-mesautil
        mesa3d-linklib-mesa
        mesa3d-linklib-glapi
        mesa3d-linklib-mesadevutil)
    set(_consumers linklibs-gallium_vc4)
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        list(APPEND _consumers linklibs-gallium_v3d)
    endif()
    set(_provider_paths "")
    set(_consumer_paths "")
    foreach(_target IN LISTS _providers)
        list(APPEND _provider_paths "$<TARGET_FILE:${_target}>")
    endforeach()
    foreach(_target IN LISTS _consumers)
        list(APPEND _consumer_paths "$<TARGET_FILE:${_target}>")
    endforeach()

    set(_generated "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/galliumcoreapi")
    set(_generation_stamp "${_generated}/generation.stamp")
    set(_libdir "${AROS_BUILD_DIR}/gen/lib/mesa26.0.0")
    set(_outputs
        "${_generated}/gallium_core_api.h"
        "${_generated}/gca_table.c"
        "${_generated}/gca_bind.c"
        "${_generated}/gca_glue.S"
        "${_generated}/gca_redefs.txt"
        "${_generated}/gca_private.list")
    add_custom_command(
        OUTPUT "${_generation_stamp}"
        BYPRODUCTS ${_outputs}
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_generated}"
        COMMAND "${Python3_EXECUTABLE}" -s -B "${_source_script}"
            --nm "${CMAKE_NM}" --ar "${CMAKE_AR}"
            --consumer ${_consumer_paths}
            --providers ${_provider_paths}
            --private-data nir_intrinsic_infos nir_op_infos
                _util_cpu_caps_state glsl_type_builtin_int
                glsl_type_builtin_uint glsl_type_builtin_vec4
                util_dynarray_is_data_stack_allocated
            --mesa-version 26.0.0 --arch "${AROS_TARGET_CPU}"
            "--abi-flags=${_abi_flags}"
            --abi-files "${_source_patch}" --outdir "${_generated}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_generation_stamp}"
        DEPENDS ${_consumers} ${_providers}
            "${_source_script}" "${_source_patch}"
        VERBATIM)
    add_custom_target(mesa26-gca-generate DEPENDS "${_generation_stamp}")

    set_source_files_properties(
        "${_generated}/gca_table.c"
        "${_generated}/gca_bind.c"
        "${_generated}/gca_glue.S" PROPERTIES GENERATED TRUE)
    add_library(mesa26-gca-provider STATIC "${_generated}/gca_table.c")
    add_library(mesa26-gca-consumer-objects OBJECT
        "${_generated}/gca_bind.c" "${_generated}/gca_glue.S")
    foreach(_target IN ITEMS mesa26-gca-provider mesa26-gca-consumer-objects)
        add_dependencies(${_target} mesa26-gca-generate)
        target_include_directories(${_target} PRIVATE "${_generated}")
    endforeach()
    set_target_properties(mesa26-gca-provider PROPERTIES
        OUTPUT_NAME galliumcoreapi
        ARCHIVE_OUTPUT_DIRECTORY "${_libdir}")

    set(_vc4_drm_source
        "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/aros_drm_vc4.c")
    if(NOT EXISTS "${_vc4_drm_source}")
        message(FATAL_ERROR "Mesa 26 VC4 winsys source is missing")
    endif()
    add_library(mesa26-gca-vc4-drm-raw OBJECT "${_vc4_drm_source}")
    get_target_property(_vc4_includes linklibs-gallium_vc4 INCLUDE_DIRECTORIES)
    get_target_property(_vc4_options linklibs-gallium_vc4 COMPILE_OPTIONS)
    if(_vc4_includes)
        target_include_directories(mesa26-gca-vc4-drm-raw PRIVATE ${_vc4_includes})
    endif()
    if(_vc4_options)
        target_compile_options(mesa26-gca-vc4-drm-raw PRIVATE ${_vc4_options})
    endif()
    target_compile_definitions(mesa26-gca-vc4-drm-raw PRIVATE ${_vc4_defines})
    target_include_directories(mesa26-gca-vc4-drm-raw PRIVATE
        "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"
        "${_generated}")
    add_dependencies(mesa26-gca-vc4-drm-raw mesa26-gca-generate)

    set(_vc4_output "${_libdir}/libgallium_vc4_gca.a")
    set(_v3d_output "${_libdir}/libgallium_v3d_gca.a")
    set(_archive_outputs "${_vc4_output}")
    set(_v3d_archive "")
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        list(APPEND _archive_outputs "${_v3d_output}")
        set(_v3d_archive "$<TARGET_FILE:linklibs-gallium_v3d>")
        set(_expected_map_sha
            "67734c63c3f9ddcfade036eb14dbf981c0db0f158855f6f0b3b1ab3332c705c8")
    else()
        # The ARM32 VC4-only import set is a separate target-ABI contract.
        set(_expected_map_sha
            "c08571eca9781d256f496588aef15add77c6565e4591db2d20147408848277ed")
    endif()
    add_custom_command(
        OUTPUT ${_archive_outputs}
        COMMAND "${CMAKE_COMMAND}"
            "-DGCA_AR=${CMAKE_AR}"
            "-DGCA_OBJCOPY=${CMAKE_OBJCOPY}"
            "-DGCA_NM=${CMAKE_NM}"
            "-DGCA_AR_SHA256=${_CMAKE_AR_sha}"
            "-DGCA_OBJCOPY_SHA256=${_CMAKE_OBJCOPY_sha}"
            "-DGCA_NM_SHA256=${_CMAKE_NM_sha}"
            "-DGCA_EXPECTED_MAP_SHA256=${_expected_map_sha}"
            "-DGCA_GENERATED=${_generated}"
            "-DGCA_RAW_VC4=$<TARGET_FILE:linklibs-gallium_vc4>"
            "-DGCA_RAW_V3D=${_v3d_archive}"
            "-DGCA_VC4_OUTPUT=${_vc4_output}"
            "-DGCA_V3D_OUTPUT=${_v3d_output}"
            "-DGCA_BIND_OBJECTS=$<JOIN:$<TARGET_OBJECTS:mesa26-gca-consumer-objects>,|>"
            "-DGCA_VC4_DRM_OBJECTS=$<JOIN:$<TARGET_OBJECTS:mesa26-gca-vc4-drm-raw>,|>"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunMesa26GalliumCoreAPI.cmake"
        DEPENDS mesa26-gca-generate mesa26-gca-consumer-objects
            mesa26-gca-vc4-drm-raw ${_consumers}
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunMesa26GalliumCoreAPI.cmake"
        VERBATIM)
    add_custom_target(mesa26-gca-consumer-archives DEPENDS ${_archive_outputs})
    add_library(mesa26-gca-vc4 STATIC IMPORTED GLOBAL)
    set_target_properties(mesa26-gca-vc4 PROPERTIES
        IMPORTED_LOCATION "${_vc4_output}")
    add_dependencies(mesa26-gca-vc4 mesa26-gca-consumer-archives)
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        add_library(mesa26-gca-v3d STATIC IMPORTED GLOBAL)
        set_target_properties(mesa26-gca-v3d PROPERTIES
            IMPORTED_LOCATION "${_v3d_output}")
        add_dependencies(mesa26-gca-v3d mesa26-gca-consumer-archives)
    endif()

    add_dependencies(hidd-vc4gallium mesa26-gca-consumer-archives)
    target_link_libraries(hidd-vc4gallium PRIVATE mesa26-gca-vc4)
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        add_dependencies(hidd-v3d mesa26-gca-consumer-archives)
        target_link_libraries(hidd-v3d PRIVATE mesa26-gca-v3d)
    endif()
    # The table introduces imports from the six core archives. Keep it and
    # those providers in the same rescan group, not as a trailing static lib.
    if(AROS_LLD_BIN)
        set(_group_start --start-group)
        set(_group_end --end-group)
    else()
        set(_group_start -Wl,--start-group)
        set(_group_end -Wl,--end-group)
    endif()
    target_link_libraries(mesa3dgl-library PRIVATE
        ${_group_start} mesa26-gca-provider ${_providers} ${_group_end})

    # The successful archive rewrite is not proof that the final modules
    # contain the table, bind entry point and every trampoline. Check the
    # linked products with the same verified target nm and pinned import map.
    foreach(_product IN ITEMS hidd-vc4gallium mesa3dgl-library)
        if(_product STREQUAL "hidd-vc4gallium")
            set(_kind vc4)
        else()
            set(_kind provider)
        endif()
        add_custom_command(TARGET ${_product} POST_BUILD
            COMMAND "${CMAKE_COMMAND}"
                "-DGCA_PRODUCT=$<TARGET_FILE:${_product}>"
                "-DGCA_KIND=${_kind}"
                "-DGCA_NM=${CMAKE_NM}"
                "-DGCA_NM_SHA256=${_CMAKE_NM_sha}"
                "-DGCA_MAP=${_generated}/gca_redefs.txt"
                "-DGCA_MAP_SHA256=${_expected_map_sha}"
                -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyMesa26GalliumCoreAPI.cmake"
            VERBATIM)
    endforeach()
    if(AROS_TARGET_CPU STREQUAL "aarch64")
        add_custom_command(TARGET hidd-v3d POST_BUILD
            COMMAND "${CMAKE_COMMAND}"
                "-DGCA_PRODUCT=$<TARGET_FILE:hidd-v3d>"
                "-DGCA_KIND=v3d"
                "-DGCA_NM=${CMAKE_NM}"
                "-DGCA_NM_SHA256=${_CMAKE_NM_sha}"
                "-DGCA_MAP=${_generated}/gca_redefs.txt"
                "-DGCA_MAP_SHA256=${_expected_map_sha}"
                -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyMesa26GalliumCoreAPI.cmake"
            VERBATIM)
    endif()
endfunction()
