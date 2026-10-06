cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_cmake NAMES cmake REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/developer-lib-copy")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-developer-lib-copy-${_suffix} with spaces")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary Developer library copy test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_build "${_root}/build")
execute_process(
    COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
        "-DAROS_DEVELOPER_LIB_COPY_FIXTURE_DIR=${_fixture_dir}"
    RESULT_VARIABLE _configure_result OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "Developer library copy fixture configure failed\n${_configure_stdout}\n${_configure_stderr}")
endif()

function(_build_lib out_result out_log)
    execute_process(
        COMMAND "${_cmake}" --build "${_build}" --target stage-developer-libraries --parallel 2
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

function(_find_current_recipe out_recipe out_digest)
    file(SHA256 "${_source_root}/lib-a.bin" _a_hash)
    file(SHA256 "${_source_root}/lib-b.txt" _b_hash)
    file(GLOB _recipes LIST_DIRECTORIES FALSE "${_build}/.aros-sdk-file-copies/*.json")
    set(_found "")
    foreach(_candidate IN LISTS _recipes)
        file(READ "${_candidate}" _json)
        string(JSON _candidate_a ERROR_VARIABLE _json_error GET "${_json}" sha256 0)
        if(_json_error STREQUAL "NOTFOUND" AND _candidate_a STREQUAL _a_hash)
            string(JSON _candidate_b ERROR_VARIABLE _json_error GET "${_json}" sha256 1)
            if(_json_error STREQUAL "NOTFOUND" AND _candidate_b STREQUAL _b_hash)
                set(_found "${_candidate}")
                break()
            endif()
        endif()
    endforeach()
    if(NOT _found)
        message(FATAL_ERROR "could not locate the currently configured Developer library recipe")
    endif()
    file(SHA256 "${_found}" _digest)
    set(${out_recipe} "${_found}" PARENT_SCOPE)
    set(${out_digest} "${_digest}" PARENT_SCOPE)
endfunction()

function(_run_configured_recipe recipe digest out_result out_log)
    get_filename_component(_recipe_root "${recipe}" DIRECTORY)
    get_filename_component(_recipe_stem "${recipe}" NAME_WE)
    set(_depfile "${_recipe_root}/${_recipe_stem}.d")
    execute_process(
        COMMAND "${_cmake}"
            "-DSOURCE_ROOT=${_source_root}"
            "-DSOURCE_BOUNDARY=${_source_boundary}"
            -DSOURCE_KIND=local
            "-DDESTINATION=${_destination}"
            -DDESTINATION_KIND=lib
            "-DFD_ROOT=${_destination}"
            "-DBINARY_ROOT=${_build}"
            -DFETCH_DESTINATION=
            "-DRECIPE=${recipe}"
            "-DRECIPE_SHA256=${digest}"
            "-DDEPFILE=${_depfile}"
            -P "${_engine_dir}/RunSdkFileCopies.cmake"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(${out_result} "${_result}" PARENT_SCOPE)
    set(${out_log} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

set(_source_boundary "${_build}/source tree")
set(_source_root "${_source_boundary}/lib")
set(_destination "${_build}/SYS/Developer/lib")
set(_expected_root "${_root}/expected")
file(MAKE_DIRECTORY "${_expected_root}")
set(_initial_a "alpha;beta\r\nomega\r\n")
set(_initial_b "semicolon;payload;with;crlf\r\nsecond line\r\n")
file(WRITE "${_expected_root}/lib-a.bin" "${_initial_a}")
file(WRITE "${_expected_root}/lib-b.txt" "${_initial_b}")

_build_lib(_build_result _build_log)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR "initial Developer library build failed\n${_build_log}")
endif()
foreach(_name IN ITEMS lib-a.bin lib-b.txt)
    _compare("${_destination}/${_name}" "${_expected_root}/${_name}" "initial ${_name}")
endforeach()

_build_lib(_build_result _build_log)
if(NOT _build_result EQUAL 0 OR _build_log MATCHES "Staging SDK files")
    message(FATAL_ERROR "unchanged Ninja Developer library copy was not a no-op\n${_build_log}")
endif()

file(REMOVE "${_destination}/lib-b.txt")
_build_lib(_build_result _build_log)
if(NOT _build_result EQUAL 0 OR NOT EXISTS "${_destination}/lib-b.txt")
    message(FATAL_ERROR "missing Developer library output did not regenerate\n${_build_log}")
endif()
_compare("${_destination}/lib-b.txt" "${_expected_root}/lib-b.txt" "missing-output recovery")

# A source edit is both a CMake configure dependency and a new hash-bound recipe.
set(_changed_a "changed;alpha\r\nchanged;omega\r\n")
file(WRITE "${_source_root}/lib-a.bin" "${_changed_a}")
file(WRITE "${_expected_root}/lib-a-changed.bin" "${_changed_a}")
_build_lib(_build_result _build_log)
if(NOT _build_result EQUAL 0 OR
   NOT _build_log MATCHES "Developer library fixture configure marker")
    message(FATAL_ERROR
        "local input change did not trigger successful CMake reconfiguration\n${_build_log}")
endif()
_compare("${_destination}/lib-a.bin" "${_expected_root}/lib-a-changed.bin"
    "reconfigured source change")
_find_current_recipe(_recipe _recipe_digest)

# Calling the runner directly bypasses Ninja's auto-regeneration. Its retained
# configure-time hash must reject a source edit before either existing output changes.
file(COPY_FILE "${_destination}/lib-a.bin" "${_expected_root}/lib-a-before-stale-runner.bin")
file(COPY_FILE "${_destination}/lib-b.txt" "${_expected_root}/lib-b-before-stale-runner.txt")
file(WRITE "${_source_root}/lib-a.bin" "unconfigured;change\r\n")
_run_configured_recipe("${_recipe}" "${_recipe_digest}" _runner_result _runner_log)
if(_runner_result EQUAL 0 OR NOT _runner_log MATCHES "source differs from its configured hash")
    message(FATAL_ERROR "stale configured source was not refused\n${_runner_log}")
endif()
_compare("${_destination}/lib-a.bin" "${_expected_root}/lib-a-before-stale-runner.bin"
    "output after stale source refusal")
_compare("${_destination}/lib-b.txt" "${_expected_root}/lib-b-before-stale-runner.txt"
    "batch member after stale source refusal")

# Runtime source symlink refusal, again with direct runner invocation so no
# configure step can replace the pinned recipe first.
file(WRITE "${_source_root}/lib-a.bin" "${_changed_a}")
file(WRITE "${_root}/outside-source.bin" "outside source\r\n")
file(REMOVE "${_source_root}/lib-a.bin")
file(CREATE_LINK "${_root}/outside-source.bin" "${_source_root}/lib-a.bin"
    SYMBOLIC RESULT _source_link_result)
if(NOT _source_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime source symlink: ${_source_link_result}")
endif()
_run_configured_recipe("${_recipe}" "${_recipe_digest}" _runner_result _runner_log)
if(_runner_result EQUAL 0 OR NOT _runner_log MATCHES "symlink")
    message(FATAL_ERROR "runtime Developer library source symlink was not refused\n${_runner_log}")
endif()
_compare("${_destination}/lib-a.bin" "${_expected_root}/lib-a-before-stale-runner.bin"
    "output after source symlink refusal")
_compare("${_destination}/lib-b.txt" "${_expected_root}/lib-b-before-stale-runner.txt"
    "batch member after source symlink refusal")
file(REMOVE "${_source_root}/lib-a.bin")
file(WRITE "${_source_root}/lib-a.bin" "${_changed_a}")

# An output symlink is refused before touching its target or the other batch member.
file(WRITE "${_root}/outside-output.bin" "outside output sentinel\r\n")
file(COPY_FILE "${_root}/outside-output.bin" "${_expected_root}/outside-output-before.bin")
file(REMOVE "${_destination}/lib-a.bin")
file(CREATE_LINK "${_root}/outside-output.bin" "${_destination}/lib-a.bin"
    SYMBOLIC RESULT _output_link_result)
if(NOT _output_link_result STREQUAL "0")
    message(FATAL_ERROR "could not create runtime output symlink: ${_output_link_result}")
endif()
_run_configured_recipe("${_recipe}" "${_recipe_digest}" _runner_result _runner_log)
if(_runner_result EQUAL 0 OR NOT _runner_log MATCHES "symlink")
    message(FATAL_ERROR "runtime Developer library output symlink was not refused\n${_runner_log}")
endif()
_compare("${_root}/outside-output.bin" "${_expected_root}/outside-output-before.bin"
    "outside target after output symlink refusal")
_compare("${_destination}/lib-b.txt" "${_expected_root}/lib-b-before-stale-runner.txt"
    "batch member after output symlink refusal")
file(REMOVE "${_destination}/lib-a.bin")
file(COPY_FILE "${_expected_root}/lib-a-before-stale-runner.bin" "${_destination}/lib-a.bin")

# A private recipe is independently pinned by the configure-time digest.
file(WRITE "${_recipe}" "{\"files\":[\"lib-a.bin\"],\"sha256\":[]}")
_run_configured_recipe("${_recipe}" "${_recipe_digest}" _runner_result _runner_log)
if(_runner_result EQUAL 0 OR
   NOT _runner_log MATCHES "recipe differs from its configure-time contract")
    message(FATAL_ERROR "mutated Developer library recipe was not refused\n${_runner_log}")
endif()
_compare("${_destination}/lib-a.bin" "${_expected_root}/lib-a-before-stale-runner.bin"
    "output after recipe refusal")
_compare("${_destination}/lib-b.txt" "${_expected_root}/lib-b-before-stale-runner.txt"
    "batch member after recipe refusal")

function(_expect_configure_failure probe expected_text)
    set(_probe_build "${_root}/probe-${probe}")
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${_probe_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine_dir}"
            "-DAROS_DEVELOPER_LIB_COPY_FIXTURE_DIR=${_fixture_dir}"
            "-DAROS_DEVELOPER_LIB_COPY_PROBE=${probe}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _log MATCHES "${expected_text}")
        message(FATAL_ERROR
            "Developer library probe ${probe} was not rejected as expected (${_result}), "
            "expected /${expected_text}/\n${_log}")
    endif()
endfunction()

_expect_configure_failure(lib-subdir "destination escapes its declared root")
_expect_configure_failure(lib-fetch "require local source inputs")
_expect_configure_failure(duplicate-output "SDK output")

file(REMOVE_RECURSE "${_root}")
message(STATUS
    "Developer library copies: byte-exact CRLF/semicolon payloads, Ninja no-op and output repair, source-triggered reconfigure, hash-bound stale-source/recipe refusal, source/output symlink refusal, and LIB scope/fetch/collision probes passed")
