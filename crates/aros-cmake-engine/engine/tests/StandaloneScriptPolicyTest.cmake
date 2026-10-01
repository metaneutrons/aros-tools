cmake_minimum_required(VERSION 3.22)

get_filename_component(_cmake_root "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
# Standalone runners use Run<Action> names. RuntimeProbes is an included
# target-definition module, not a cmake -P entry point.
file(GLOB _standalone_scripts
    "${_cmake_root}/Run*.cmake"
    "${_cmake_root}/Verify*.cmake"
    "${_cmake_root}/Write*.cmake"
    "${_cmake_root}/scripts/Verify*.cmake")
# CMake globbing is case-insensitive on macOS, even on a case-sensitive volume;
# apply the runner naming contract to the actual filename instead.
foreach(_script IN LISTS _standalone_scripts)
    get_filename_component(_name "${_script}" NAME)
    if(_name MATCHES "^Run" AND NOT _name MATCHES "^Run[A-Z]")
        list(REMOVE_ITEM _standalone_scripts "${_script}")
    endif()
endforeach()
list(FIND _standalone_scripts "${_cmake_root}/RunConfigureBuild.cmake" _runner_index)
list(FIND _standalone_scripts "${_cmake_root}/RuntimeProbes.cmake" _module_index)
if(_runner_index EQUAL -1 OR NOT _module_index EQUAL -1)
    message(FATAL_ERROR "standalone runner selection includes a module or excludes a real runner")
endif()
list(APPEND _standalone_scripts
    "${_cmake_root}/CopyDirRecursive.cmake"
    "${_cmake_root}/StageHeaderGlob.cmake"
    "${_cmake_root}/SubstituteHeader.cmake"
    "${_cmake_root}/TransformHeader.cmake")
list(REMOVE_DUPLICATES _standalone_scripts)
list(SORT _standalone_scripts)

foreach(_script IN LISTS _standalone_scripts)
    file(READ "${_script}" _content)
    if(NOT _content MATCHES
       "(^|\n)cmake_minimum_required\\(VERSION 3\\.22\\)($|\n)")
        file(RELATIVE_PATH _relative "${_cmake_root}" "${_script}")
        message(FATAL_ERROR
            "Standalone cmake -P entry point has no policy baseline: ${_relative}")
    endif()
endforeach()

list(LENGTH _standalone_scripts _script_count)
message(STATUS
    "standalone CMake policy-baseline test passed (${_script_count} scripts)")
