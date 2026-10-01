cmake_minimum_required(VERSION 3.22)

foreach(_name IN ITEMS PACKAGE_OUTPUT PACKAGE_ROMTOOL)
    if(NOT DEFINED ${_name} OR "${${_name}}" STREQUAL "")
        message(FATAL_ERROR "package publication requires ${_name}")
    endif()
endforeach()
if(NOT EXISTS "${PACKAGE_ROMTOOL}" OR IS_DIRECTORY "${PACKAGE_ROMTOOL}")
    message(FATAL_ERROR "package publication requires a regular romtool executable")
endif()

set(_members "")
set(_after_separator FALSE)
math(EXPR _last "${CMAKE_ARGC} - 1")
foreach(_index RANGE 0 ${_last})
    if(CMAKE_ARGV${_index} STREQUAL "--")
        set(_after_separator TRUE)
    elseif(_after_separator)
        list(APPEND _members "${CMAKE_ARGV${_index}}")
    endif()
endforeach()
if(NOT _members)
    message(FATAL_ERROR "package publication requires member inputs")
endif()
foreach(_member IN LISTS _members)
    if(NOT EXISTS "${_member}" OR IS_DIRECTORY "${_member}" OR
       IS_SYMLINK "${_member}")
        message(FATAL_ERROR "package member is missing or unsafe: ${_member}")
    endif()
endforeach()

get_filename_component(_output_dir "${PACKAGE_OUTPUT}" DIRECTORY)
if(NOT IS_DIRECTORY "${_output_dir}" OR IS_SYMLINK "${_output_dir}")
    message(FATAL_ERROR "package output directory is missing or unsafe")
endif()
set(_pending "${PACKAGE_OUTPUT}.pending")
foreach(_path IN ITEMS "${PACKAGE_OUTPUT}" "${_pending}")
    if(IS_SYMLINK "${_path}" OR (EXISTS "${_path}" AND IS_DIRECTORY "${_path}"))
        message(FATAL_ERROR "package output path is unsafe: ${_path}")
    endif()
endforeach()
# The temporary name belongs exclusively to this Ninja output rule. A failed
# invocation never removes or overwrites the last published package.
if(EXISTS "${_pending}")
    file(REMOVE "${_pending}")
endif()
execute_process(
    COMMAND "${PACKAGE_ROMTOOL}" pkg create --basename -o "${_pending}" ${_members}
    RESULT_VARIABLE _create_result
    OUTPUT_VARIABLE _create_stdout
    ERROR_VARIABLE _create_stderr)
if(NOT _create_result EQUAL 0)
    message(FATAL_ERROR "package creation failed\n${_create_stdout}${_create_stderr}")
endif()
if(NOT EXISTS "${_pending}" OR IS_DIRECTORY "${_pending}" OR
   IS_SYMLINK "${_pending}")
    message(FATAL_ERROR "romtool did not create a regular private package")
endif()
file(SIZE "${_pending}" _size)
if(_size EQUAL 0)
    message(FATAL_ERROR "romtool created an empty package")
endif()
execute_process(
    COMMAND "${PACKAGE_ROMTOOL}" pkg list "${_pending}"
    RESULT_VARIABLE _inspect_result
    OUTPUT_VARIABLE _inspect_stdout
    ERROR_VARIABLE _inspect_stderr)
if(NOT _inspect_result EQUAL 0)
    message(FATAL_ERROR "package inspection failed\n${_inspect_stdout}${_inspect_stderr}")
endif()
if(IS_SYMLINK "${PACKAGE_OUTPUT}" OR
   (EXISTS "${PACKAGE_OUTPUT}" AND IS_DIRECTORY "${PACKAGE_OUTPUT}"))
    message(FATAL_ERROR "package destination changed before publication")
endif()
file(RENAME "${_pending}" "${PACKAGE_OUTPUT}" RESULT _publish_result)
if(NOT _publish_result STREQUAL "0")
    message(FATAL_ERROR "cannot atomically publish package: ${_publish_result}")
endif()
