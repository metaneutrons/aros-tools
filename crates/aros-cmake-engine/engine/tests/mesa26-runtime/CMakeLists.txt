cmake_minimum_required(VERSION 3.22)
project(mesa26-runtime-fixture NONE)

foreach(_required IN ITEMS
        MESA26_RUNTIME_MODULE MESA26_RUNTIME_CASE AROS_MESA_VERSION AROS_SYS_DIR
        MESA26_LOADER_NAME MESA26_PROVIDER_NAME)
    if(NOT DEFINED ${_required})
        message(FATAL_ERROR "fixture requires ${_required}")
    endif()
endforeach()

set(_engine_cmake_list "${CMAKE_CURRENT_LIST_DIR}/../../CMakeLists.txt")
if(NOT EXISTS "${_engine_cmake_list}")
    message(FATAL_ERROR "fixture cannot find production engine CMakeLists.txt")
endif()
file(READ "${_engine_cmake_list}" _engine_cmake_source)
string(FIND "${_engine_cmake_source}"
    "include(\"\${GENERATED_TARGETS_CMAKE}\")" _generated_include_position)
string(FIND "${_engine_cmake_source}"
    "include(\"\${AROS_CMAKE_ENGINE_DIR}/Mesa26Runtime.cmake\")"
    _mesa26_include_position)
string(FIND "${_engine_cmake_source}"
    "aros_configure_mesa26_runtime()" _mesa26_call_position)
string(FIND "${_engine_cmake_source}"
    "include(\"\${AROS_CMAKE_ENGINE_DIR}/RuntimeProbes.cmake\")"
    _runtime_probes_include_position)
if(_generated_include_position LESS 0 OR _mesa26_include_position LESS 0 OR
   _mesa26_call_position LESS 0 OR _runtime_probes_include_position LESS 0 OR
   NOT _generated_include_position LESS _mesa26_include_position OR
   NOT _mesa26_include_position LESS _mesa26_call_position OR
   NOT _mesa26_call_position LESS _runtime_probes_include_position)
    message(FATAL_ERROR
        "production engine must include generated targets, then Mesa26Runtime, "
        "call aros_configure_mesa26_runtime(), and only then include RuntimeProbes")
endif()

add_custom_target(workbench-libs-gl)
set_target_properties(workbench-libs-gl PROPERTIES OUTPUT_NAME "${MESA26_LOADER_NAME}")
if(NOT MESA26_RUNTIME_CASE STREQUAL "missing-provider")
    add_custom_target(mesa3dgl-library)
    set_target_properties(mesa3dgl-library PROPERTIES OUTPUT_NAME "${MESA26_PROVIDER_NAME}")
endif()

include("${MESA26_RUNTIME_MODULE}")
aros_configure_mesa26_runtime()

if(MESA26_RUNTIME_CASE STREQUAL "positive")
    if(NOT TARGET mesa26-runtime-selection)
        message(FATAL_ERROR "Mesa26 runtime selection target was not created")
    endif()
    get_target_property(_loader_dependencies
        workbench-libs-gl MANUALLY_ADDED_DEPENDENCIES)
    foreach(_expected IN ITEMS mesa3dgl-library mesa26-runtime-selection)
        if(NOT _expected IN_LIST _loader_dependencies)
            message(FATAL_ERROR
                "workbench-libs-gl is missing runtime dependency ${_expected}")
        endif()
    endforeach()
elseif(MESA26_RUNTIME_CASE STREQUAL "non26")
    if(TARGET mesa26-runtime-selection)
        message(FATAL_ERROR "non26 Mesa configuration created a Mesa26 target")
    endif()
    get_target_property(_loader_dependencies
        workbench-libs-gl MANUALLY_ADDED_DEPENDENCIES)
    if(_loader_dependencies AND NOT _loader_dependencies STREQUAL "_loader_dependencies-NOTFOUND")
        message(FATAL_ERROR
            "non26 Mesa configuration added loader dependencies: ${_loader_dependencies}")
    endif()
endif()
