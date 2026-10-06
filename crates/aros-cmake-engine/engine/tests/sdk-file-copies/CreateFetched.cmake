cmake_minimum_required(VERSION 3.22)
foreach(_required IN ITEMS FETCH_ROOT PYTHON FIXTURE_DIR)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "CreateFetched requires ${_required}")
    endif()
endforeach()
file(MAKE_DIRECTORY "${FETCH_ROOT}/includes")
execute_process(
    COMMAND "${PYTHON}" "${FIXTURE_DIR}/WriteBytes.py"
        "${FETCH_ROOT}/includes/fetched-a.fd" "410d0a00ff5a"
    RESULT_VARIABLE _binary_result OUTPUT_QUIET ERROR_VARIABLE _binary_error)
if(NOT _binary_result EQUAL 0)
    message(FATAL_ERROR "could not create binary fetch fixture: ${_binary_error}")
endif()
file(WRITE "${FETCH_ROOT}/includes/fetched-b.fd" "fetched-b;literal=$bytes\r\n")
file(WRITE "${FETCH_ROOT}/.complete" "fetched inputs ready\n")
