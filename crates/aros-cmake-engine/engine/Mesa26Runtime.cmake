include_guard(GLOBAL)

# The source-owned Mesa26 recipe publishes a versioned implementation and
# selects it through SYS/GL.default. This closes that runtime edge; linking
# the GL client archive alone cannot establish a usable guest GL library.
function(aros_configure_mesa26_runtime)
    if(NOT AROS_MESA_VERSION STREQUAL "26.0.0" OR
       NOT TARGET workbench-libs-gl)
        return()
    endif()
    if(NOT TARGET mesa3dgl-library)
        if(AROS_NATIVE_BUILD_CONTRACT)
            # A native contract selects the product; this one has the GL
            # loader but not the implementation. Say so, without inventing one.
            message(WARNING
                "Mesa26 GL loader is selected without its implementation; no "
                "GL.default is published for this native build")
            return()
        endif()
        message(FATAL_ERROR "Mesa26 GL loader requires its source-selected implementation")
    endif()
    get_target_property(_loader_name workbench-libs-gl OUTPUT_NAME)
    get_target_property(_implementation_name mesa3dgl-library OUTPUT_NAME)
    if(NOT _loader_name STREQUAL "gl.library" OR
       NOT _implementation_name STREQUAL "mesa3dgl26-0.library")
        message(FATAL_ERROR
            "Mesa26 runtime identity mismatch: ${_loader_name}, ${_implementation_name}")
    endif()
    set(_selection "${CMAKE_BINARY_DIR}/gen/mesa26-runtime/GL.default")
    file(GENERATE OUTPUT "${_selection}" CONTENT "mesa3dgl26-0\n")
    set(_env_dir "${AROS_SYS_DIR}/Prefs/Env-Archive/SYS")
    set(_env_output "${_env_dir}/GL.default")
    add_custom_command(OUTPUT "${_env_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_env_dir}"
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_selection}" "${_env_output}"
        DEPENDS "${_selection}"
        COMMENT "Publishing source-selected Mesa26 GL implementation"
        VERBATIM)
    add_custom_target(mesa26-runtime-selection DEPENDS "${_env_output}")
    add_dependencies(workbench-libs-gl mesa3dgl-library mesa26-runtime-selection)
endfunction()
