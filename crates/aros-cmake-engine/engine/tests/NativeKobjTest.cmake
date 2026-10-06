cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/native-kobj")
set(_test_root "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/NativeKobjTest")
file(REMOVE_RECURSE "${_test_root}")
file(MAKE_DIRECTORY "${_test_root}")
file(CHMOD "${_fixture_dir}/fake_collector.py"
    PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)

function(_configure_case case_name build_dir expect_success expected_error)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -G Ninja
            "-DCMAKE_MAKE_PROGRAM=${_ninja}"
            "-DNATIVE_KOBJ_MODULE_DIR=${_engine_dir}"
            "-DNATIVE_KOBJ_CASE=${case_name}"
            -S "${_fixture_dir}" -B "${build_dir}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_output "${_stdout}${_stderr}")
    if(expect_success)
        if(NOT _result EQUAL 0)
            message(FATAL_ERROR
                "NativeKobj configure failed (${case_name}): ${_output}")
        endif()
    else()
        if(_result EQUAL 0)
            message(FATAL_ERROR
                "NativeKobj unexpectedly configured (${case_name})")
        endif()
        string(FIND "${_output}" "${expected_error}" _error_at)
        if(_error_at EQUAL -1)
            message(FATAL_ERROR
                "NativeKobj failed for the wrong reason (${case_name}): ${_output}")
        endif()
    endif()
endfunction()

function(_assert_json_arg json index expected description)
    string(JSON _actual ERROR_VARIABLE _error GET "${json}" ${index})
    if(_error OR NOT _actual STREQUAL expected)
        message(FATAL_ERROR
            "NativeKobj ${description}: argv[${index}] expected '${expected}', got '${_actual}'")
    endif()
endfunction()

function(_find_stage json out_var)
    string(JSON _length LENGTH "${json}")
    if(_length GREATER 0)
        math(EXPR _last "${_length} - 1")
        foreach(_index RANGE 0 ${_last})
            string(JSON _value GET "${json}" ${_index})
            if(_value STREQUAL "-o")
                math(EXPR _stage_index "${_index} + 1")
                string(JSON _stage GET "${json}" ${_stage_index})
                set(${out_var} "${_stage}" PARENT_SCOPE)
                return()
            endif()
        endforeach()
    endif()
    set(${out_var} "" PARENT_SCOPE)
endfunction()

set(_build "${_test_root}/positive")
_configure_case(positive "${_build}" TRUE "")
set(_log "${_test_root}/collector.log")
file(WRITE "${_log}" "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj simple_kobj
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "NativeKobj positive build failed: ${_build_stdout}${_build_stderr}")
endif()

file(STRINGS "${_log}" _commands)
set(_full_link "")
set(_simple_link "")
set(_localization_count 0)
foreach(_command IN LISTS _commands)
    string(JSON _operation GET "${_command}" 0)
    if(_operation STREQUAL "--ld")
        _find_stage("${_command}" _stage)
        if(_stage MATCHES "/full\\.kobj\\.native-kobj-stage$")
            set(_full_link "${_command}")
        elseif(_stage MATCHES "/simple\\.kobj\\.native-kobj-stage$")
            set(_simple_link "${_command}")
        endif()
    elseif(_operation STREQUAL "--localize-kobj")
        math(EXPR _localization_count "${_localization_count} + 1")
    endif()
endforeach()
if(NOT _full_link OR NOT _simple_link OR NOT _localization_count EQUAL 2)
    message(FATAL_ERROR "NativeKobj did not link/localize both source forms")
endif()

_assert_json_arg("${_full_link}" 0 "--ld" "full form")
_assert_json_arg("${_full_link}" 2 "--report" "report capture")
_assert_json_arg("${_full_link}" 3 "${_build}/out/full.kobj.native-kobj-stage.sets.report"
    "private report path")
_assert_json_arg("${_full_link}" 4 "--" "full form")
_assert_json_arg("${_full_link}" 5 "-r" "full form")
_assert_json_arg("${_full_link}" 6 "-m" "full form")
_assert_json_arg("${_full_link}" 7 "riscvelf_aros" "emulation order")
_assert_json_arg("${_full_link}" 8 "--emit-relocs" "full form")
_assert_json_arg("${_full_link}" 9 "-T" "full form")
_assert_json_arg("${_full_link}" 10 "${_fixture_dir}/link.ld" "owned linker script")
_assert_json_arg("${_full_link}" 11 "-o" "full form")
set(_full_suffixes
    "/start.c.o" "/arch.c.o" "/arch2.c.o" "/asm.c.o" "/c.c.o"
    "/cxx.c.o" "/user.c.o" "/generated_extra.o" "/end.c.o")
set(_index 13)
foreach(_suffix IN LISTS _full_suffixes)
    string(JSON _object GET "${_full_link}" ${_index})
    if(NOT _object MATCHES "${_suffix}$")
        message(FATAL_ERROR
            "NativeKobj full group order mismatch at argv[${_index}]: ${_object}")
    endif()
    math(EXPR _index "${_index} + 1")
endforeach()
_assert_json_arg("${_full_link}" 22 "--no-undefined" "full form")
_assert_json_arg("${_full_link}" 23 "-L" "canonical library search path")
_assert_json_arg("${_full_link}" 25 "-lfoo" "full library order")
_assert_json_arg("${_full_link}" 26 "-lfoo" "repeated USELIBS")
_assert_json_arg("${_full_link}" 27 "-ldos" "automatic library order")
_assert_json_arg("${_full_link}" 28 "-lintuition" "automatic library order")
_assert_json_arg("${_full_link}" 29 "-llayers" "automatic library order")
_assert_json_arg("${_full_link}" 30 "-lgraphics" "automatic library order")
_assert_json_arg("${_full_link}" 31 "-loop" "automatic library order")
_assert_json_arg("${_full_link}" 32 "-lutility" "automatic library order")
_assert_json_arg("${_full_link}" 33 "-lexpansion" "automatic library order")
_assert_json_arg("${_full_link}" 34 "-lkeymap" "automatic library order")
_assert_json_arg("${_full_link}" 35 "-ldeflib" "full DEFNAME_LIBS")
string(FIND "${_full_link}" "arossupport" _excluded_library_at)
if(NOT _excluded_library_at EQUAL -1)
    message(FATAL_ERROR "NativeKobj did not filter arossupport from USELIBS")
endif()

_assert_json_arg("${_simple_link}" 2 "--report" "simple report capture")
_assert_json_arg("${_simple_link}" 4 "--" "simple form")
_assert_json_arg("${_simple_link}" 5 "-r" "simple form")
string(JSON _simple_arch GET "${_simple_link}" 8)
string(JSON _simple_c GET "${_simple_link}" 9)
if(NOT _simple_arch MATCHES "/simple_arch\\.c\\.o$" OR
   NOT _simple_c MATCHES "/simple_c\\.c\\.o$")
    message(FATAL_ERROR "NativeKobj simple groups are not ARCH then C")
endif()
_assert_json_arg("${_simple_link}" 10 "-L" "simple form")
_assert_json_arg("${_simple_link}" 12 "-ldos" "simple auto libraries")
if(EXISTS "${_build}/out/full.kobj.native-kobj-stage.sets.report" OR
   EXISTS "${_build}/out/simple.kobj.native-kobj-stage.sets.report")
    message(FATAL_ERROR "NativeKobj retained a report for a no-skip link")
endif()

# The phony validator must reject a changed seal even when the output is current.
set(_config "${_build}/CMakeFiles/full_kobj-native-kobj.json")
file(APPEND "${_config}" " ")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj
    RESULT_VARIABLE _tamper_result
    OUTPUT_VARIABLE _tamper_stdout
    ERROR_VARIABLE _tamper_stderr)
if(_tamper_result EQUAL 0 OR
   NOT "${_tamper_stdout}${_tamper_stderr}" MATCHES "configuration digest changed after configure")
    message(FATAL_ERROR
        "NativeKobj no-op validation did not reject changed seal: ${_tamper_stdout}${_tamper_stderr}")
endif()
_configure_case(positive "${_build}" TRUE "")

set(_full_output "${_build}/out/full.kobj")
set(_outside "${_test_root}/outside-sentinel")
file(WRITE "${_outside}" "outside remains untouched")
file(REMOVE "${_full_output}")
file(CREATE_LINK "${_outside}" "${_full_output}" SYMBOLIC)
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj
    RESULT_VARIABLE _symlink_result
    OUTPUT_VARIABLE _symlink_stdout
    ERROR_VARIABLE _symlink_stderr)
if(_symlink_result EQUAL 0 OR
   NOT "${_symlink_stdout}${_symlink_stderr}" MATCHES "crosses symlinked path")
    message(FATAL_ERROR
        "NativeKobj no-op validation did not reject output symlink: ${_symlink_stdout}${_symlink_stderr}")
endif()
file(READ "${_outside}" _outside_contents)
if(NOT _outside_contents STREQUAL "outside remains untouched")
    message(FATAL_ERROR "NativeKobj changed the target of an output symlink")
endif()
file(REMOVE "${_full_output}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0)
    message(FATAL_ERROR
        "NativeKobj failed to rebuild after symlink refusal: ${_repair_stdout}${_repair_stderr}")
endif()

# Fail only at localization after a relink; the existing final must survive.
file(WRITE "${_full_output}" "sentinel-final-content")
string(JSON _first_object GET "${_full_link}" 13)
execute_process(COMMAND "${CMAKE_COMMAND}" -E touch "${_first_object}"
    RESULT_VARIABLE _touch_result)
if(NOT _touch_result EQUAL 0)
    message(FATAL_ERROR "Could not touch fixture object to force a relink")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "NATIVE_KOBJ_FAIL_LOCALIZE=1"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj
    RESULT_VARIABLE _failure_result
    OUTPUT_VARIABLE _failure_stdout
    ERROR_VARIABLE _failure_stderr)
if(_failure_result EQUAL 0)
    message(FATAL_ERROR "NativeKobj accepted a requested localization failure")
endif()
file(READ "${_full_output}" _final_contents)
if(NOT _final_contents STREQUAL "sentinel-final-content")
    message(FATAL_ERROR "NativeKobj replaced final output after localization failure")
endif()
if(EXISTS "${_full_output}.native-kobj-stage")
    message(FATAL_ERROR "NativeKobj left a failed localization stage")
endif()

# A nonempty collector report represents source entries the engine skipped.
# It must stop before localization/publication and remain available as evidence.
file(WRITE "${_full_output}" "sentinel-final-content")
execute_process(COMMAND "${CMAKE_COMMAND}" -E touch "${_first_object}"
    RESULT_VARIABLE _touch_result)
if(NOT _touch_result EQUAL 0)
    message(FATAL_ERROR "Could not touch fixture object to force report-path relink")
endif()
file(STRINGS "${_log}" _commands_before_report)
set(_localizations_before_report 0)
foreach(_command IN LISTS _commands_before_report)
    string(JSON _operation GET "${_command}" 0)
    if(_operation STREQUAL "--localize-kobj")
        math(EXPR _localizations_before_report "${_localizations_before_report} + 1")
    endif()
endforeach()
set(_report_path "${_full_output}.native-kobj-stage.sets.report")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "NATIVE_KOBJ_LOG=${_log}"
        "NATIVE_KOBJ_REPORT_CONTENT=skipped set entry\n"
        "${CMAKE_COMMAND}" --build "${_build}" --target full_kobj
    RESULT_VARIABLE _report_result
    OUTPUT_VARIABLE _report_stdout
    ERROR_VARIABLE _report_stderr)
if(_report_result EQUAL 0 OR
   NOT "${_report_stdout}${_report_stderr}" MATCHES
       "collector reported skipped sets")
    message(FATAL_ERROR
        "NativeKobj did not reject the nonempty report: ${_report_stdout}${_report_stderr}")
endif()
file(READ "${_report_path}" _report_contents)
if(NOT _report_contents STREQUAL "skipped set entry\n")
    message(FATAL_ERROR "NativeKobj did not retain the nonempty collector report")
endif()
file(READ "${_full_output}" _final_contents)
if(NOT _final_contents STREQUAL "sentinel-final-content")
    message(FATAL_ERROR "NativeKobj published a KOBJ despite skipped collector entries")
endif()
if(EXISTS "${_full_output}.native-kobj-stage")
    message(FATAL_ERROR "NativeKobj left its stage after rejecting the report")
endif()
file(STRINGS "${_log}" _commands_after_report)
set(_localizations_after_report 0)
foreach(_command IN LISTS _commands_after_report)
    string(JSON _operation GET "${_command}" 0)
    if(_operation STREQUAL "--localize-kobj")
        math(EXPR _localizations_after_report "${_localizations_after_report} + 1")
    endif()
endforeach()
if(NOT _localizations_after_report EQUAL _localizations_before_report)
    message(FATAL_ERROR "NativeKobj localized a link with a nonempty report")
endif()

set(_negative_cases
    missing-contract "has no recorded source module-macro fact"
    abi-contract "requests full form but source contract is abi-only"
    missing-groups "has no object inputs"
    simple-extra "simple form accepts only"
    outside-output "escapes the build tree"
    missing-input "is missing and is not an exact DEPENDS path"
    unsafe-flag "may not replace the helper-owned output"
    search-override "uses unsupported linker option"
    mode-override "uses unsupported linker option"
    script-override "uses unsupported linker option"
    alternate-library "uses unsupported linker option"
    unresolved-library "has no static target"
    duplicate-owner "owner is declared more than once"
    duplicate-output "output/staging/report path is already claimed"
    output-dependency "output collides with a file dependency"
    stage-dependency "resolves to its output/staging path"
    config-input-collision "sealed config collides with an input"
    output-alias "resolves to its output/staging path")
list(LENGTH _negative_cases _negative_count)
math(EXPR _negative_last "${_negative_count} - 1")
foreach(_index RANGE 0 ${_negative_last} 2)
    math(EXPR _error_index "${_index} + 1")
    list(GET _negative_cases ${_index} _case)
    list(GET _negative_cases ${_error_index} _expected_error)
    _configure_case("${_case}" "${_test_root}/negative-${_case}"
        FALSE "${_expected_error}")
endforeach()

message(STATUS "NativeKobj isolated helper tests passed")
