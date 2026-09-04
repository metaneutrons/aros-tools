cmake_minimum_required(VERSION 3.22)

get_filename_component(_cmake_root "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
file(GLOB _standalone_scripts
    "${_cmake_root}/Run*.cmake"
    "${_cmake_root}/Verify*.cmake"
    "${_cmake_root}/Write*.cmake"
    "${_cmake_root}/scripts/Verify*.cmake")
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
