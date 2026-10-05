cmake_minimum_required(VERSION 3.22)

# Reuse every byte, dependency, incremental, mutation and refusal probe for
# both new closed output roots. This is not a shell-script execution test.
foreach(_kind IN ITEMS bin man1)
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            "-DAROS_SDK_COPY_DESTINATION_KIND=${_kind}"
            -P "${CMAKE_CURRENT_LIST_DIR}/SdkFileCopiesTest.cmake"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "Developer ${_kind} copy probes failed\n${_stdout}\n${_stderr}")
    endif()
endforeach()
message(STATUS "Developer bin/man1 exact-root local/fetched copy probes passed")
