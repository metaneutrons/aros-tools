cmake_minimum_required(VERSION 3.22)

# The two fixture files are byte-for-byte snapshots of the cached locked
# zlib 1.3.1 and zstd 1.5.7 templates in AROS-NX's pc-x86_64 build tree.
find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_sed NAMES sed REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/sdk-text-rule")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-sdk-text-rule-${_suffix}")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary SDK text test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")
set(_build "${_root}/build")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture_dir}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
        "-DAROS_SDK_TEXT_FIXTURE_DIR=${_fixture_dir}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "SDK text fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target all-sdk-text --parallel 2
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "SDK text fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

set(_sdk_prefix "\${prefix}")
set(_zlib_input "${_build}/Ports/zlib/zlib.pc.cmakein")
set(_zstd_input "${_build}/Ports/zstd/libzstd.pc.in")
set(_zlib_expected "${_root}/zlib.expected")
set(_zstd_expected "${_root}/zstd.expected")
execute_process(
    COMMAND "${_sed}"
        -e "s|@CMAKE_INSTALL_PREFIX@|/Developer|g"
        -e "s|@INSTALL_LIB_DIR@|${_sdk_prefix}/lib|g"
        -e "s|@INSTALL_INC_DIR@|${_sdk_prefix}/include|g"
        -e "s|@VERSION@|1.3.1|g"
        -e "s|^exec_prefix=.*|exec_prefix=${_sdk_prefix}|"
    INPUT_FILE "${_zlib_input}" OUTPUT_FILE "${_zlib_expected}"
    RESULT_VARIABLE _zlib_sed_result ERROR_VARIABLE _zlib_sed_error)
if(NOT _zlib_sed_result EQUAL 0)
    message(FATAL_ERROR "reference zlib sed failed: ${_zlib_sed_error}")
endif()
execute_process(
    COMMAND "${_sed}"
        -e "s|@PREFIX@|/Developer|g"
        -e "s|@LIBDIR@|${_sdk_prefix}/lib|g"
        -e "s|@INCLUDEDIR@|${_sdk_prefix}/include|g"
        -e "s|@VERSION@|1.5.7|g"
        -e "s|@LIBS_MT@||g"
        -e "/^Libs\\.private/d"
        -e "s|^exec_prefix=.*|exec_prefix=${_sdk_prefix}|"
    INPUT_FILE "${_zstd_input}" OUTPUT_FILE "${_zstd_expected}"
    RESULT_VARIABLE _zstd_sed_result ERROR_VARIABLE _zstd_sed_error)
if(NOT _zstd_sed_result EQUAL 0)
    message(FATAL_ERROR "reference zstd sed failed: ${_zstd_sed_error}")
endif()

set(_zlib_output "${_build}/SYS/Developer/lib/pkgconfig/zlib.pc")
set(_zstd_output "${_build}/SYS/Developer/lib/pkgconfig/libzstd.pc")
foreach(_pair IN ITEMS "${_zlib_output}|${_zlib_expected}"
                       "${_zstd_output}|${_zstd_expected}")
    string(REPLACE "|" ";" _paths "${_pair}")
    list(GET _paths 0 _actual)
    list(GET _paths 1 _expected)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E compare_files "${_actual}" "${_expected}"
        RESULT_VARIABLE _compare_result)
    if(NOT _compare_result EQUAL 0)
        message(FATAL_ERROR "SDK text product differs byte-for-byte from sed: ${_actual}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target all-sdk-text --parallel 2
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0 OR
   "${_noop_stdout}\n${_noop_stderr}" MATCHES "Generating SDK text product")
    message(FATAL_ERROR
        "unchanged SDK text build was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(REMOVE "${_zstd_output}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target sdk-text-zstd-pkgc --parallel 2
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_zstd_output}")
    message(FATAL_ERROR
        "missing SDK text product did not regenerate (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E compare_files "${_zstd_output}" "${_zstd_expected}"
    RESULT_VARIABLE _repair_compare_result)
if(NOT _repair_compare_result EQUAL 0)
    message(FATAL_ERROR "regenerated zstd SDK text differs from reference sed")
endif()

function(_expect_runner_failure label build input output input_root output_root operation message_text)
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            "-DINPUT=${input}"
            "-DOUTPUT=${output}"
            "-DINPUT_ROOT=${input_root}"
            "-DOUTPUT_ROOT=${output_root}"
            "-DBINARY_ROOT=${build}"
            "-DOPERATIONS=${operation}"
            -P "${_engine_dir}/RunSdkTextRule.cmake"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 15)
    set(_log "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _log MATCHES "${message_text}")
        message(FATAL_ERROR
            "${label} was not rejected as expected (${_result}); expected /${message_text}/\n${_log}")
    endif()
endfunction()

set(_normal_root "${_build}/SYS/Developer/lib")
find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
set(_fifo "${_build}/Ports/zlib/nonregular.pc.in")
execute_process(COMMAND "${_mkfifo}" "${_fifo}"
    COMMAND_ERROR_IS_FATAL ANY TIMEOUT 10)
_expect_runner_failure("FIFO input" "${_build}" "${_fifo}"
    "${_normal_root}/pkgconfig/rejected.pc" "${_build}/Ports/zlib" "${_normal_root}"
    "REPLACE_ALL|@VERSION@|1.3.1" "SDK text input is not a regular file")
set(_fifo_output "${_normal_root}/pkgconfig/nonregular.pc")
execute_process(COMMAND "${_mkfifo}" "${_fifo_output}"
    COMMAND_ERROR_IS_FATAL ANY TIMEOUT 10)
_expect_runner_failure("FIFO output" "${_build}" "${_zlib_input}"
    "${_fifo_output}" "${_build}/Ports/zlib" "${_normal_root}"
    "REPLACE_ALL|@VERSION@|1.3.1" "SDK text output is not a replaceable regular file")
_expect_runner_failure("unsafe regular-expression token" "${_build}" "${_zlib_input}"
    "${_normal_root}/pkgconfig/rejected.pc" "${_build}/Ports/zlib" "${_normal_root}"
    "REPLACE_ALL|.*|x" "Unsafe SDK text literal token")
_expect_runner_failure("unrecognized operation/command" "${_build}" "${_zlib_input}"
    "${_normal_root}/pkgconfig/rejected.pc" "${_build}/Ports/zlib" "${_normal_root}"
    "RUN|touch|output" "Unsafe SDK text operation")
_expect_runner_failure("output path escape" "${_build}" "${_zlib_input}"
    "${_root}/outside.pc" "${_build}/Ports/zlib" "${_normal_root}"
    "REPLACE_ALL|@VERSION@|1.3.1" "SDK text output is outside Developer/lib/pkgconfig")

set(_symlink_build "${_root}/symlink-build")
set(_symlink_outside "${_root}/outside")
file(MAKE_DIRECTORY "${_symlink_build}/Ports/zlib" "${_symlink_outside}/SYS/Developer/lib")
file(COPY_FILE "${_fixture_dir}/zlib.pc.cmakein"
    "${_symlink_build}/Ports/zlib/zlib.pc.cmakein")
file(CREATE_LINK "${_symlink_outside}" "${_symlink_build}/redirect" SYMBOLIC
    RESULT _symlink_result)
if(NOT _symlink_result STREQUAL "0")
    message(FATAL_ERROR "could not create SDK output ancestor symlink: ${_symlink_result}")
endif()
_expect_runner_failure("output ancestor symlink" "${_symlink_build}"
    "${_symlink_build}/Ports/zlib/zlib.pc.cmakein"
    "${_symlink_build}/redirect/SYS/Developer/lib/pkgconfig/zlib.pc"
    "${_symlink_build}/Ports/zlib"
    "${_symlink_build}/redirect/SYS/Developer/lib"
    "REPLACE_ALL|@VERSION@|1.3.1"
    "SDK text output root escapes the physical build directory")

message(STATUS
    "SDK text test passed: cached zlib/zstd templates match sed byte-for-byte, "
    "no-op and missing-output recovery, regex/command/path/symlink/FIFO rejection")
file(REMOVE_RECURSE "${_root}")
