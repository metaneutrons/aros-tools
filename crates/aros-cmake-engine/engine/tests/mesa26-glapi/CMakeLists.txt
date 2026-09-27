cmake_minimum_required(VERSION 3.22)
project(Mesa26Generators NONE)

foreach(_required IN ITEMS MESA26_TEST_SOURCE_ROOT MESA26_TEST_AROS_SOURCE
        MESA26_TEST_ENGINE)
    if(NOT DEFINED ${_required} OR NOT IS_DIRECTORY "${${_required}}")
        message(FATAL_ERROR "${_required} must name an existing directory")
    endif()
endforeach()
set(AROS_SOURCE_DIR "${MESA26_TEST_AROS_SOURCE}")
include("${MESA26_TEST_ENGINE}/PythonGenerators.cmake")

get_filename_component(_fetch_destination "${MESA26_TEST_SOURCE_ROOT}" DIRECTORY)
set(_fetch_stamp "${CMAKE_BINARY_DIR}/mesa26-source-ready.stamp")
add_custom_command(OUTPUT "${_fetch_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fetch_stamp}" VERBATIM)
add_custom_target(mesa3d-fetch DEPENDS "${_fetch_stamp}")
set_property(TARGET mesa3d-fetch PROPERTY AROS_FETCH_DESTINATION
    "${_fetch_destination}")
set_property(TARGET mesa3d-fetch PROPERTY AROS_FETCH_COMPLETION_STAMP
    "${_fetch_stamp}")
if(DEFINED MESA26_TEST_PYTHON_ROOT)
    get_filename_component(_python_fetch_destination
        "${MESA26_TEST_PYTHON_ROOT}" DIRECTORY)
    foreach(_package IN ITEMS mako markupsafe)
        set(_package_stamp "${CMAKE_BINARY_DIR}/${_package}-ready.stamp")
        add_custom_command(OUTPUT "${_package_stamp}"
            COMMAND "${CMAKE_COMMAND}" -E touch "${_package_stamp}" VERBATIM)
        add_custom_target("mesa3d-${_package}-fetch" DEPENDS "${_package_stamp}")
        set_property(TARGET "mesa3d-${_package}-fetch" PROPERTY
            AROS_FETCH_DESTINATION "${_python_fetch_destination}")
        set_property(TARGET "mesa3d-${_package}-fetch" PROPERTY
            AROS_FETCH_COMPLETION_STAMP "${_package_stamp}")
    endforeach()
endif()

set(_build_root "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/26.0.0")
set(_local_root "${AROS_SOURCE_DIR}/workbench/libs/mesa/libglapi")
aros_generate_python_outputs(
    OWNER mesa3d-linklib-glapi-generate
    SOURCE_ROOT "${MESA26_TEST_SOURCE_ROOT}"
    BUILD_ROOT "${_build_root}"
    FETCH_TARGET mesa3d-fetch
    DRIVER_SCRIPT "${_local_root}/mesa26_generate.py"
    SOURCE_INPUTS
        src/mesa/glapi/glapi/gen/gl_and_es_API.xml
        src/glx/libgl-symbols.txt
    LOCAL_INPUTS "${_local_root}/public_glapi_required_symbols.txt"
    JOB
        SCRIPT src/mesa/glapi/mapi_abi.py
        OUTPUT src/mesa/glapi/shared-glapi/shared_glapi_mapi_tmp.h
        ARGUMENTS header
    JOB
        LOCAL_SCRIPT
        SCRIPT "${_local_root}/gen_public_glapi_wrappers.sh"
        OUTPUT src/mesa/glapi/shared-glapi/public_glapi_wrappers.c
        DEPENDS_ON_OUTPUTS src/mesa/glapi/shared-glapi/shared_glapi_mapi_tmp.h
        ARGUMENTS wrappers)

set(_yaml "${MESA26_TEST_SOURCE_ROOT}/src/util/format/u_format.yaml")
aros_generate_python_outputs(
    OWNER mesa3d-linklib-mesautil-generated
    SOURCE_ROOT "${MESA26_TEST_SOURCE_ROOT}"
    BUILD_ROOT "${_build_root}"
    FETCH_TARGET mesa3d-fetch
    SOURCE_INPUTS
        src/util/format/u_format.yaml
        src/util/format/u_format_parse.py
        src/util/format/u_format_pack.py
    JOB
        SCRIPT src/util/format/u_format_table.py
        OUTPUT src/util/format/u_format_gen.h
        ARGUMENTS "${_yaml}" --enums
    JOB
        SCRIPT src/util/format/u_format_table.py
        OUTPUT src/util/format/u_format_pack.h
        ARGUMENTS "${_yaml}" --header
    JOB
        SCRIPT src/util/format/u_format_table.py
        OUTPUT src/util/format/u_format_table.c
        ARGUMENTS "${_yaml}"
    JOB
        SCRIPT src/util/format_srgb.py
        OUTPUT src/util/format_srgb.c)

set(_mesa_root "${AROS_SOURCE_DIR}/workbench/libs/mesa")
set(_aux "src/gallium/auxiliary")
set(_perf "${MESA26_TEST_SOURCE_ROOT}/src/util/perf")
set(_pipe "${MESA26_TEST_SOURCE_ROOT}/src/gallium/include/pipe")
set(_blend "${MESA26_TEST_SOURCE_ROOT}/src/util/blend.h")
aros_generate_python_outputs(
    OWNER mesa3d-linklib-galliumauxiliary-generated
    SOURCE_ROOT "${MESA26_TEST_SOURCE_ROOT}"
    BUILD_ROOT "${_build_root}"
    FETCH_TARGET mesa3d-fetch
    DRIVER_SCRIPT "${_mesa_root}/mesa26_generate.py"
    PACKAGE_FETCH_TARGETS mesa3d-mako-fetch mesa3d-markupsafe-fetch
    PACKAGE_SOURCE_ROOTS "${MESA26_TEST_PYTHON_ROOT}" "${MESA26_TEST_PYTHON_ROOT}"
    PACKAGE_PYTHON_PATHS . .
    SOURCE_INPUTS
        src/util/perf/u_trace.py
        src/gallium/include/pipe/p_defines.h
        src/gallium/include/pipe/p_video_enums.h
        src/util/blend.h
    LOCAL_INPUTS "${_mesa_root}/mesa20_generate.py"
    JOB
        SCRIPT ${_aux}/indices/u_indices_gen.py
        OUTPUT ${_aux}/indices/u_indices_gen.c
        ARGUMENTS python-output @OUTPUT@
    JOB
        SCRIPT ${_aux}/indices/u_unfilled_gen.py
        OUTPUT ${_aux}/indices/u_unfilled_gen.c
        ARGUMENTS python-output @OUTPUT@
    JOB
        SCRIPT ${_aux}/util/u_tracepoints.py
        OUTPUT ${_aux}/util/u_tracepoints.c
        ARGUMENTS python-dual-output u_tracepoints.c u_tracepoints.h
            -p "${_perf}" -C @OUT_C@ -H @OUT_H@
    JOB
        SCRIPT ${_aux}/util/u_tracepoints.py
        OUTPUT ${_aux}/util/u_tracepoints.h
        ARGUMENTS python-dual-output u_tracepoints.c u_tracepoints.h
            -p "${_perf}" -C @OUT_C@ -H @OUT_H@
    JOB
        SCRIPT ${_aux}/driver_trace/enums2names.py
        OUTPUT ${_aux}/driver_trace/tr_util.c
        ARGUMENTS python-dual-output tr_util.c tr_util.h
            "${_pipe}/p_defines.h" "${_pipe}/p_video_enums.h" "${_blend}"
            -C @OUT_C@ -H @OUT_H@
    JOB
        SCRIPT ${_aux}/driver_trace/enums2names.py
        OUTPUT ${_aux}/driver_trace/tr_util.h
        ARGUMENTS python-dual-output tr_util.c tr_util.h
            "${_pipe}/p_defines.h" "${_pipe}/p_video_enums.h" "${_blend}"
            -C @OUT_C@ -H @OUT_H@)

# Optional end-to-end probe of the exact declarations emitted by the Rust
# transpiler. The production graph owns them; this fixture supplies only
# verified local inputs and synthetic fetch-completion stamps.
if(DEFINED MESA26_TEST_GENERATED_TARGETS)
    if(NOT DEFINED MESA26_TEST_PORTS_ROOT OR
            NOT IS_DIRECTORY "${MESA26_TEST_PORTS_ROOT}")
        message(FATAL_ERROR
            "MESA26_TEST_PORTS_ROOT must name the prepared ports directory")
    endif()
    set(AROS_PORTS_DIR "${MESA26_TEST_PORTS_ROOT}")
    get_filename_component(_test_input_parent "${MESA26_TEST_PORTS_ROOT}" DIRECTORY)
    set_property(TARGET mesa3d-fetch PROPERTY AROS_FETCH_DESTINATION
        "${_test_input_parent}")
    foreach(_package IN ITEMS mako markupsafe)
        if(TARGET "mesa3d-${_package}-fetch")
            set_property(TARGET "mesa3d-${_package}-fetch" PROPERTY
                AROS_FETCH_DESTINATION "${_test_input_parent}")
        endif()
    endforeach()
    set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
    file(READ "${MESA26_TEST_GENERATED_TARGETS}" _generated_targets)
    string(REGEX MATCHALL "aros_generate_python_outputs\\([^)]*\\)"
        _generator_declarations "${_generated_targets}")
    foreach(_declaration IN LISTS _generator_declarations)
        if(_declaration MATCHES "OWNER (mesa3d-linklib-(compiler|mesa)-generated|linklibs-gallium_v3d-generated)")
            cmake_language(EVAL CODE "${_declaration}")
        endif()
    endforeach()
endif()
