cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_cmake NAMES cmake REQUIRED)
find_program(_python NAMES python3 REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/sdk-file-copies")
set(_destination_arguments "")
if(AROS_SDK_COPY_DESTINATION_KIND)
    list(APPEND _destination_arguments
        "-DAROS_SDK_COPY_DESTINATION_KIND=${AROS_SDK_COPY_DESTINATION_KIND}")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-sdk-file-copies-${_suffix} with spaces")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary SDK file-copy test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_build "${_root}/build")
execute_process(
    COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
        "-DAROS_SDK_COPY_FIXTURE_DIR=${_fixture_dir}"
        "-DAROS_SDK_COPY_PYTHON=${_python}"
        ${_destination_arguments}
    RESULT_VARIABLE _configure_result OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "SDK file-copy fixture configure failed\n${_configure_stdout}\n${_configure_stderr}")
endif()

function(_build target out_result out_log)
    execute_process(
        COMMAND "${_cmake}" --build "${_build}" --target "${target}" --parallel 2
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(${out_result} "${_result}" PARENT_SCOPE)
    set(${out_log} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

function(_compare actual expected label)
    execute_process(
        COMMAND "${_cmake}" -E compare_files "${actual}" "${expected}"
        RESULT_VARIABLE _result)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${label} differs byte-for-byte: ${actual}")
    endif()
endfunction()

_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR "SDK file-copy initial build failed\n${_build_log}")
endif()

set(_destination "${_build}/SYS/Developer/SDK/fd/serial")
if(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "bin")
    set(_destination "${_build}/SYS/Developer/bin")
elseif(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "man1")
    set(_destination "${_build}/SYS/Developer/man/man1")
endif()
set(_expected_root "${_root}/expected")
file(MAKE_DIRECTORY "${_expected_root}")
file(WRITE "${_expected_root}/local-a.fd" "local-a;dollar=$value\r\n")
file(WRITE "${_expected_root}/local-b.fd" "local-b bytes\n")
file(WRITE "${_expected_root}/fetched-b.fd" "fetched-b;literal=$bytes\r\n")
execute_process(
    COMMAND "${_python}" "${_fixture_dir}/WriteBytes.py"
        "${_expected_root}/fetched-a.fd" "410d0a00ff5a"
    RESULT_VARIABLE _expected_result)
if(NOT _expected_result EQUAL 0)
    message(FATAL_ERROR "could not create expected binary SDK file")
endif()
foreach(_name IN ITEMS local-a.fd local-b.fd fetched-a.fd fetched-b.fd)
    _compare("${_destination}/${_name}" "${_expected_root}/${_name}" "${_name}")
endforeach()
if(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "bin")
    find_program(_test NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
    execute_process(COMMAND "${_test}" -x "${_destination}/local-a.fd" RESULT_VARIABLE _executable)
    execute_process(COMMAND "${_test}" -x "${_destination}/local-b.fd" RESULT_VARIABLE _nonexecutable)
    if(NOT _executable EQUAL 0 OR _nonexecutable EQUAL 0)
        message(FATAL_ERROR "Developer bin copies did not preserve source executable/nonexecutable modes")
    endif()
endif()
file(GLOB _products LIST_DIRECTORIES FALSE "${_destination}/*")
list(LENGTH _products _product_count)
if(NOT _product_count EQUAL 4)
    message(FATAL_ERROR "SDK copy published unexpected products: ${_products}")
endif()

_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0 OR _build_log MATCHES "Staging SDK files")
    message(FATAL_ERROR "unchanged Ninja SDK copy was not a no-op\n${_build_log}")
endif()

# The input is absent during configure and produced only after the fetch stamp.
execute_process(
    COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
        "-DAROS_SDK_COPY_FIXTURE_DIR=${_fixture_dir}"
        "-DAROS_SDK_COPY_PYTHON=${_python}"
        ${_destination_arguments}
    RESULT_VARIABLE _reconfigure_result OUTPUT_VARIABLE _reconfigure_stdout
    ERROR_VARIABLE _reconfigure_stderr)
if(NOT _reconfigure_result EQUAL 0)
    message(FATAL_ERROR "unchanged SDK reconfigure failed\n${_reconfigure_stdout}\n${_reconfigure_stderr}")
endif()
_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0 OR _build_log MATCHES "Staging SDK files")
    message(FATAL_ERROR "unchanged CMake reconfigure forced SDK restaging\n${_build_log}")
endif()

# The input is absent during configure and produced only after the fetch stamp.
# The depfile must make a post-fetch edit visible on the following build.
execute_process(
    COMMAND "${_python}" "${_fixture_dir}/WriteBytes.py"
        "${_build}/Ports/serialized fd/includes/fetched-a.fd" "4200ff0d0a"
    RESULT_VARIABLE _mutation_result)
if(NOT _mutation_result EQUAL 0)
    message(FATAL_ERROR "could not mutate fetched SDK source")
endif()
execute_process(
    COMMAND "${_python}" "${_fixture_dir}/WriteBytes.py"
        "${_expected_root}/fetched-a-mutated.fd" "4200ff0d0a"
    RESULT_VARIABLE _expected_result)
if(NOT _expected_result EQUAL 0)
    message(FATAL_ERROR "could not create expected mutated binary SDK file")
endif()
_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR "post-fetch mutation build failed\n${_build_log}")
endif()
_compare("${_destination}/fetched-a.fd" "${_expected_root}/fetched-a-mutated.fd"
    "post-fetch source mutation")

file(REMOVE "${_destination}/local-b.fd")
_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0 OR NOT EXISTS "${_destination}/local-b.fd")
    message(FATAL_ERROR "missing SDK product did not regenerate\n${_build_log}")
endif()
_compare("${_destination}/local-b.fd" "${_expected_root}/local-b.fd"
    "missing-output recovery")

# A post-configure source symlink must fail before publishing any member of the
# local batch, preserving the two already-published local products.
file(COPY_FILE "${_destination}/local-a.fd" "${_expected_root}/local-a-before.fd")
file(COPY_FILE "${_destination}/local-b.fd" "${_expected_root}/local-b-before.fd")
file(REMOVE "${_build}/source tree/fd/local-a.fd")
file(WRITE "${_root}/outside-local.fd" "outside\n")
file(CREATE_LINK "${_root}/outside-local.fd" "${_build}/source tree/fd/local-a.fd"
    SYMBOLIC RESULT _link_result)
if(NOT _link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime source symlink: ${_link_result}")
endif()
_build(all-sdk-files _build_result _build_log)
if(_build_result EQUAL 0 OR NOT _build_log MATCHES "symlink")
    message(FATAL_ERROR "runtime source symlink was not refused\n${_build_log}")
endif()
_compare("${_destination}/local-a.fd" "${_expected_root}/local-a-before.fd"
    "local output after symlink refusal")
_compare("${_destination}/local-b.fd" "${_expected_root}/local-b-before.fd"
    "batch member after symlink refusal")
file(REMOVE "${_build}/source tree/fd/local-a.fd")
file(WRITE "${_build}/source tree/fd/local-a.fd" "local-a;dollar=$value\r\n")
_build(all-sdk-files _build_result _build_log)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR "SDK source restoration build failed\n${_build_log}")
endif()

# Altering the private recipe to add an otherwise valid basename is still a
# contract mutation and must be detected before staging or touching products.
set(_local_recipe "")
file(GLOB _recipe_files "${_build}/.aros-sdk-file-copies/*.json")
foreach(_recipe IN LISTS _recipe_files)
    file(READ "${_recipe}" _recipe_content)
    if(_recipe_content MATCHES "local-a[.]fd")
        set(_local_recipe "${_recipe}")
        break()
    endif()
endforeach()
if(NOT _local_recipe)
    message(FATAL_ERROR "could not locate the configured local SDK file-copy recipe")
endif()
file(COPY_FILE "${_destination}/local-a.fd" "${_expected_root}/local-a-contract.fd")
file(COPY_FILE "${_destination}/local-b.fd" "${_expected_root}/local-b-contract.fd")
file(WRITE "${_local_recipe}" "[\"local-a.fd\",\"local-b.fd\",\"extra.fd\"]")
_build(all-sdk-files _build_result _build_log)
if(_build_result EQUAL 0 OR NOT _build_log MATCHES "recipe differs from its configure-time contract")
    message(FATAL_ERROR "mutated private recipe was not refused\n${_build_log}")
endif()
_compare("${_destination}/local-a.fd" "${_expected_root}/local-a-contract.fd"
    "local output after recipe refusal")
_compare("${_destination}/local-b.fd" "${_expected_root}/local-b-contract.fd"
    "batch member after recipe refusal")

function(_expect_configure_failure probe expected_text)
    set(_probe_build "${_root}/probe-${probe}")
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_probe_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine_dir}"
            "-DAROS_SDK_COPY_FIXTURE_DIR=${_fixture_dir}"
            "-DAROS_SDK_COPY_PYTHON=${_python}"
            "-DAROS_SDK_COPY_PROBE=${probe}"
            ${_destination_arguments}
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    # CMake wraps diagnostics according to the expanded path length.
    string(REGEX REPLACE "[ \t\r\n]+" " " _normalized_log "${_log}")
    if(_result EQUAL 0 OR NOT _normalized_log MATCHES "${expected_text}")
        message(FATAL_ERROR
            "SDK file-copy probe ${probe} was not rejected as expected (${_result}), "
            "expected /${expected_text}/\n${_log}")
    endif()
endfunction()

_expect_configure_failure(duplicate-scalar "exactly one NAME")
_expect_configure_failure(duplicate-fetch "exactly one NAME")
_expect_configure_failure(fetch-stamp-outside "fetch stamp escapes")
_expect_configure_failure(traversal "unsafe SDK file basename")
_expect_configure_failure(wildcard "unsafe SDK file basename")
_expect_configure_failure(option-basename "unsafe SDK file basename")
_expect_configure_failure(control-basename "unsafe SDK file basename")
_expect_configure_failure(semicolon-basename "FILES arguments cannot contain semicolons")
_expect_configure_failure(duplicate-files "duplicate SDK file basename")
_expect_configure_failure(outside-source "local source escapes its declared root")
_expect_configure_failure(outside-destination "destination escapes its declared root")
_expect_configure_failure(source-destination-overlap "source and destination roots overlap")
_expect_configure_failure(symlink-source "crosses a symlink")
_expect_configure_failure(fifo-source "not a regular file")
_expect_configure_failure(symlink-output "crosses a symlink")
_expect_configure_failure(duplicate-output "already owned")
if(AROS_SDK_COPY_DESTINATION_KIND)
    _expect_configure_failure(destination-subdir "destination escapes its declared root")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS
    "SDK file copies: binary-exact local/fetched staging, stamp-only late fetch depfile, no-op/recovery, batch-preserving runtime symlink and recipe refusals, and 16 configure refusals passed")
