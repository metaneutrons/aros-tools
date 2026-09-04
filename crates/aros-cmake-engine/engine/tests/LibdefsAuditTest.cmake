cmake_minimum_required(VERSION 3.22)

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}/aros-libdefs-audit-${_suffix}")
if(NOT "$ENV{TMPDIR}")
    set(_root "/tmp/aros-libdefs-audit-${_suffix}")
endif()
file(MAKE_DIRECTORY "${_root}/rust/a" "${_root}/rust/b"
    "${_root}/reference/a" "${_root}/reference/b")
foreach(_path IN ITEMS
        "${_root}/rust/a/shared_libdefs.h"
        "${_root}/reference/a/shared_libdefs.h")
    file(WRITE "${_path}" "#define FUNCTIONS_COUNT  7\n")
endforeach()
foreach(_path IN ITEMS
        "${_root}/rust/b/shared_libdefs.h"
        "${_root}/reference/b/shared_libdefs.h")
    file(WRITE "${_path}" "#define FUNCTIONS_COUNT  11\n")
endforeach()
set(_manifest "${_root}/manifest.txt")
set(_report "${_root}/report.txt")
file(WRITE "${_manifest}"
    "module-a\t${_root}/rust/a/shared_libdefs.h\t${_root}/reference/a/shared_libdefs.h\n"
    "module-b\t${_root}/rust/b/shared_libdefs.h\t${_root}/reference/b/shared_libdefs.h\n")

execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -DAROS_LIBDEFS_AUDIT_MANIFEST=${_manifest}
        -DAROS_LIBDEFS_AUDIT_REPORT=${_report}
        -P "${CMAKE_CURRENT_LIST_DIR}/../RunLibdefsAudit.cmake"
    RESULT_VARIABLE _pass_result
    OUTPUT_VARIABLE _pass_stdout
    ERROR_VARIABLE _pass_stderr)
if(NOT _pass_result EQUAL 0)
    message(FATAL_ERROR
        "matching libdefs audit failed (${_pass_result})\n${_pass_stdout}\n${_pass_stderr}")
endif()
file(READ "${_report}" _pass_report)
if(NOT _pass_report MATCHES
   "compared=2\nmissing=0\nunder=0\nover=0\nmismatches=0")
    message(FATAL_ERROR "unexpected passing report:\n${_pass_report}")
endif()

file(WRITE "${_root}/reference/b/shared_libdefs.h"
    "#define FUNCTIONS_COUNT  12\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -DAROS_LIBDEFS_AUDIT_MANIFEST=${_manifest}
        -DAROS_LIBDEFS_AUDIT_REPORT=${_report}
        -P "${CMAKE_CURRENT_LIST_DIR}/../RunLibdefsAudit.cmake"
    RESULT_VARIABLE _fail_result
    OUTPUT_VARIABLE _fail_stdout
    ERROR_VARIABLE _fail_stderr)
if(_fail_result EQUAL 0)
    message(FATAL_ERROR "under-sized libdefs audit unexpectedly passed")
endif()
file(READ "${_report}" _fail_report)
if(NOT _fail_report MATCHES
   "under=1\nover=0\nmismatches=1\nunder\tmodule-b\trust=11\treference=12")
    message(FATAL_ERROR "unexpected failing report:\n${_fail_report}")
endif()

file(WRITE "${_root}/rust/b/shared_libdefs.h"
    "#define FUNCTIONS_COUNT  13\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -DAROS_LIBDEFS_AUDIT_MANIFEST=${_manifest}
        -DAROS_LIBDEFS_AUDIT_REPORT=${_report}
        -P "${CMAKE_CURRENT_LIST_DIR}/../RunLibdefsAudit.cmake"
    RESULT_VARIABLE _over_result
    OUTPUT_VARIABLE _over_stdout
    ERROR_VARIABLE _over_stderr)
if(_over_result EQUAL 0)
    message(FATAL_ERROR "over-sized libdefs audit unexpectedly passed")
endif()
file(READ "${_report}" _over_report)
if(NOT _over_report MATCHES
   "under=0\nover=1\nmismatches=1\nover\tmodule-b\trust=13\treference=12")
    message(FATAL_ERROR "unexpected over-sized report:\n${_over_report}")
endif()

file(REMOVE "${_root}/rust/b/shared_libdefs.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -DAROS_LIBDEFS_AUDIT_MANIFEST=${_manifest}
        -DAROS_LIBDEFS_AUDIT_REPORT=${_report}
        -P "${CMAKE_CURRENT_LIST_DIR}/../RunLibdefsAudit.cmake"
    RESULT_VARIABLE _missing_result
    OUTPUT_VARIABLE _missing_stdout
    ERROR_VARIABLE _missing_stderr)
if(_missing_result EQUAL 0)
    message(FATAL_ERROR "missing libdefs audit unexpectedly passed")
endif()
file(READ "${_report}" _missing_report)
if(NOT _missing_report MATCHES
   "compared=1\nmissing=1\nunder=0\nover=0\nmismatches=0\nmissing\tmodule-b\trust=\treference=12")
    message(FATAL_ERROR "unexpected missing report:\n${_missing_report}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "libdefs FUNCTIONS_COUNT audit test passed")
