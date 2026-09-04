cmake_minimum_required(VERSION 3.22)

# Executes one capability-checked Python generator and publishes its stdout as
# a build-tree file. Keeping redirection in CMake avoids a host shell, makes a
# failed command unable to leave a successful-looking partial output, and
# produces the same operation on Ninja hosts for macOS, Linux, and Windows.
foreach(_required RUN_OWNER RUN_PYTHON RUN_SCRIPT RUN_OUTPUT RUN_BUILD_ROOT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "RunPythonStdout: ${_required} is required")
    endif()
endforeach()

cmake_path(IS_PREFIX RUN_BUILD_ROOT "${RUN_OUTPUT}" NORMALIZE _owned_output)
if(NOT _owned_output)
    message(FATAL_ERROR
        "${RUN_OWNER}: stdout output ${RUN_OUTPUT} is outside ${RUN_BUILD_ROOT}")
endif()
if(NOT EXISTS "${RUN_PYTHON}")
    message(FATAL_ERROR "${RUN_OWNER}: Python interpreter vanished: ${RUN_PYTHON}")
endif()
if(NOT EXISTS "${RUN_SCRIPT}")
    message(FATAL_ERROR "${RUN_OWNER}: generator script is missing: ${RUN_SCRIPT}")
endif()
if(DEFINED RUN_WORKING_DIRECTORY AND NOT RUN_WORKING_DIRECTORY STREQUAL "")
    set(_working_directory "${RUN_WORKING_DIRECTORY}")
else()
    get_filename_component(_working_directory "${RUN_SCRIPT}" DIRECTORY)
endif()
if(NOT IS_DIRECTORY "${_working_directory}")
    message(FATAL_ERROR
        "${RUN_OWNER}: generator working directory is missing: ${_working_directory}")
endif()

if(NOT DEFINED RUN_ARGUMENT_COUNT)
    set(RUN_ARGUMENT_COUNT 0)
endif()
if(NOT RUN_ARGUMENT_COUNT MATCHES "^[0-9]+$")
    message(FATAL_ERROR
        "${RUN_OWNER}: invalid Python argument count '${RUN_ARGUMENT_COUNT}'")
endif()

set(_command "${RUN_PYTHON}" "${RUN_SCRIPT}")
if(RUN_ARGUMENT_COUNT GREATER 0)
    math(EXPR _argument_last "${RUN_ARGUMENT_COUNT} - 1")
    foreach(_index RANGE 0 ${_argument_last})
        if(NOT DEFINED RUN_ARGUMENT_${_index})
            message(FATAL_ERROR
                "${RUN_OWNER}: missing Python argument ${_index}")
        endif()
        list(APPEND _command "${RUN_ARGUMENT_${_index}}")
    endforeach()
endif()

get_filename_component(_output_directory "${RUN_OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${_output_directory}")
set(_temporary "${RUN_OUTPUT}.aros-python-tmp")
file(REMOVE "${_temporary}")

execute_process(
    COMMAND ${_command}
    WORKING_DIRECTORY "${_working_directory}"
    OUTPUT_FILE "${_temporary}"
    ERROR_VARIABLE _stderr
    RESULT_VARIABLE _result)
if(NOT _result EQUAL 0)
    file(REMOVE "${_temporary}")
    string(STRIP "${_stderr}" _stderr)
    message(FATAL_ERROR
        "${RUN_OWNER}: Python generator failed with exit ${_result}\n"
        "script: ${RUN_SCRIPT}\n"
        "stderr: ${_stderr}")
endif()

file(RENAME "${_temporary}" "${RUN_OUTPUT}" RESULT _rename_error)
if(NOT _rename_error STREQUAL "0")
    file(REMOVE "${_temporary}")
    message(FATAL_ERROR
        "${RUN_OWNER}: cannot publish ${RUN_OUTPUT}: ${_rename_error}")
endif()
