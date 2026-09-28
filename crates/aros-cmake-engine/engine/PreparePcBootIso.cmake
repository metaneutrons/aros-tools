cmake_minimum_required(VERSION 3.22)

foreach(_input IN ITEMS SYS_DIR STAGE_DIR CONFIG_SOURCE STARTUP_SOURCE)
    if(NOT DEFINED ${_input} OR "${${_input}}" STREQUAL "")
        message(FATAL_ERROR "boot-iso requires ${_input}")
    endif()
endforeach()
if(NOT IS_DIRECTORY "${SYS_DIR}" OR IS_SYMLINK "${SYS_DIR}")
    message(FATAL_ERROR "boot-iso requires an existing regular SYS directory; build AROS first")
endif()
get_filename_component(_build_dir "${SYS_DIR}" DIRECTORY)
if(NOT STAGE_DIR STREQUAL "${_build_dir}/gen/boot-iso/stage" OR
   IS_SYMLINK "${STAGE_DIR}" OR
   IS_SYMLINK "${_build_dir}/gen" OR
   IS_SYMLINK "${_build_dir}/gen/boot-iso")
    message(FATAL_ERROR "boot-iso refuses an unsafe staging directory")
endif()
foreach(_directory IN ITEMS "${SYS_DIR}/boot" "${SYS_DIR}/boot/pc"
        "${SYS_DIR}/boot/grub" "${SYS_DIR}/boot/grub/i386-pc"
        "${SYS_DIR}/S")
    if(IS_SYMLINK "${_directory}" OR
       (EXISTS "${_directory}" AND NOT IS_DIRECTORY "${_directory}"))
        message(FATAL_ERROR "boot-iso refuses an unsafe SYS directory: ${_directory}")
    endif()
endforeach()
foreach(_output IN ITEMS "${SYS_DIR}/S/Startup-Sequence"
        "${SYS_DIR}/boot/grub/grub.cfg")
    if(IS_SYMLINK "${_output}" OR
       (EXISTS "${_output}" AND IS_DIRECTORY "${_output}"))
        message(FATAL_ERROR "boot-iso refuses an unsafe SYS output: ${_output}")
    endif()
endforeach()
foreach(_file IN ITEMS "${CONFIG_SOURCE}" "${STARTUP_SOURCE}"
        "${SYS_DIR}/boot/pc/bootstrap"
        "${SYS_DIR}/boot/grub/i386-pc/grub2_eltorito")
    if(NOT EXISTS "${_file}" OR IS_DIRECTORY "${_file}" OR IS_SYMLINK "${_file}")
        message(FATAL_ERROR "boot-iso is missing a regular input: ${_file}")
    endif()
endforeach()
file(STRINGS "${CONFIG_SOURCE}" _modules REGEX "^    module2 /")
if(NOT _modules)
    message(FATAL_ERROR "boot-iso has no AROS modules in its GRUB configuration")
endif()
foreach(_module IN LISTS _modules)
    string(REGEX REPLACE "^    module2 /" "" _relative "${_module}")
    if(NOT _relative MATCHES "^boot/[A-Za-z0-9_./-]+$" OR
       _relative MATCHES "\\.\\.")
        message(FATAL_ERROR "boot-iso has an unsafe module path")
    endif()
    set(_file "${SYS_DIR}/${_relative}")
    if(NOT EXISTS "${_file}" OR IS_DIRECTORY "${_file}" OR IS_SYMLINK "${_file}")
        message(FATAL_ERROR "boot-iso is missing a regular module: ${_file}")
    endif()
endforeach()
file(REMOVE_RECURSE "${STAGE_DIR}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${SYS_DIR}" "${STAGE_DIR}"
    RESULT_VARIABLE _stage_result)
if(NOT _stage_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not isolate the SYS tree")
endif()
file(MAKE_DIRECTORY "${STAGE_DIR}/S" "${STAGE_DIR}/boot/grub")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${STARTUP_SOURCE}" "${STAGE_DIR}/S/Startup-Sequence"
    RESULT_VARIABLE _startup_result)
if(NOT _startup_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not stage Startup-Sequence")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CONFIG_SOURCE}" "${STAGE_DIR}/boot/grub/grub.cfg"
    RESULT_VARIABLE _config_result)
if(NOT _config_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not stage grub.cfg")
endif()
