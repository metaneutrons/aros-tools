include_guard(GLOBAL)

set(_LITERAL_GNU_PROCESS_OUTPUT_LIMIT 1048576)
find_program(_LITERAL_GNU_REGULAR_TEST NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)

function(_literal_gnu_check_bounded_file path label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}" OR IS_SYMLINK "${path}")
        message(FATAL_ERROR "${label} is missing or not a regular file")
    endif()
    execute_process(COMMAND "${_LITERAL_GNU_REGULAR_TEST}" -f "${path}"
        RESULT_VARIABLE _regular_result OUTPUT_QUIET ERROR_QUIET TIMEOUT 5)
    if(NOT _regular_result STREQUAL "0")
        message(FATAL_ERROR "${label} is not a regular file")
    endif()
    file(SIZE "${path}" _size)
    if(_size GREATER _LITERAL_GNU_PROCESS_OUTPUT_LIMIT)
        message(FATAL_ERROR
            "${label} exceeds the ${_LITERAL_GNU_PROCESS_OUTPUT_LIMIT}-byte limit (${_size} bytes)")
    endif()
endfunction()

function(_literal_gnu_execute_bounded_process out_result out_stdout out_stderr label log_directory timeout)
    if(NOT ARGN)
        message(FATAL_ERROR "${label} command must not be empty")
    endif()
    file(MAKE_DIRECTORY "${log_directory}")
    string(RANDOM LENGTH 24 ALPHABET 0123456789abcdef _capture_id)
    set(_stdout_file "${log_directory}/.gnu-fixture-${_capture_id}.stdout")
    set(_stderr_file "${log_directory}/.gnu-fixture-${_capture_id}.stderr")
    while(EXISTS "${_stdout_file}" OR IS_SYMLINK "${_stdout_file}" OR
          EXISTS "${_stderr_file}" OR IS_SYMLINK "${_stderr_file}")
        string(RANDOM LENGTH 24 ALPHABET 0123456789abcdef _capture_id)
        set(_stdout_file "${log_directory}/.gnu-fixture-${_capture_id}.stdout")
        set(_stderr_file "${log_directory}/.gnu-fixture-${_capture_id}.stderr")
    endwhile()
    execute_process(
        COMMAND ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_FILE "${_stdout_file}"
        ERROR_FILE "${_stderr_file}"
        TIMEOUT "${timeout}")

    # Check both sizes before reading either stream into a CMake variable.
    _literal_gnu_check_bounded_file("${_stdout_file}" "${label} stdout capture")
    _literal_gnu_check_bounded_file("${_stderr_file}" "${label} stderr capture")
    file(READ "${_stdout_file}" _stdout)
    file(READ "${_stderr_file}" _stderr)
    set(${out_result} "${_result}" PARENT_SCOPE)
    set(${out_stdout} "${_stdout}" PARENT_SCOPE)
    set(${out_stderr} "${_stderr}" PARENT_SCOPE)
endfunction()
