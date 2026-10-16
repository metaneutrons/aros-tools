cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

set(_repo "${AROS_TEST_TREE}")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/grub-build")
# CMake script mode deliberately leaves CMAKE_HOST_SYSTEM_* unset.  The
# configured fixture owns the configure-time host assertion, while the runner
# independently verifies uname before it can fetch, extract or remove paths.

string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-grub-build-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")
set(_repo "${_root}/source")
aros_test_copy_source("${_repo}"
    "arch/all-pc/boot/grub2_def"
    "arch/all-pc/boot/grub2-aros" "arch/all-pc/boot/grub2-host")
file(READ "${_repo}/arch/all-pc/boot/grub2_def" _grub_version)
string(STRIP "${_grub_version}" _grub_version)
if(_grub_version STREQUAL "2.12")
    set(_pc_count 615)
    set(_efi64_count 591)
    set(_efi32_count 593)
    set(_primary_url "https://ftp.gnu.org/gnu/grub/grub-2.12.tar.xz")
    set(_fallback_url "https://ftpmirror.gnu.org/grub/grub-2.12.tar.xz")
elseif(_grub_version STREQUAL "2.16")
    set(_pc_count 686)
    set(_efi64_count 681)
    set(_efi32_count 679)
    set(_primary_url "https://gitlab.freedesktop.org/api/v4/projects/26558/packages/generic/source-assets/grub-2.16/grub-2.16.tar.xz")
    set(_fallback_url "")
else()
    message(FATAL_ERROR "GRUB build fixture has unsupported source version ${_grub_version}")
endif()

function(_grub_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    set(_selected_engine "${AROS_TEST_ENGINE_DIR}")
    if(name STREQUAL "python-fallback")
        set(_selected_engine "${_fallback_engine}")
    elseif(name MATCHES "^engine-(missing|symlink|symlink-parent)-manifest$")
        set(_defect "${CMAKE_MATCH_1}")
        set(_selected_engine "${_root}/${name}-engine")
        aros_test_copy_engine_defect("${_selected_engine}"
            "manifests/grub-${_grub_version}-pc.install" "${_defect}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
            "-DAROS_TEST_ENGINE_DIR=${_selected_engine}"
            "-DAROS_REPO_ROOT=${_repo}"
            "-DGRUB_BUILD_CASE=${name}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "grub-build ${name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "grub-build ${name} unexpectedly configured")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "grub-build ${name} missed '${expected_message}'\n${_log}")
        endif()
    endif()
    set(CONFIGURED_BUILD "${_build}" PARENT_SCOPE)
    set(CONFIGURE_LOG "${_log}" PARENT_SCOPE)
endfunction()

function(_grub_contract_value contract variable output)
    if(NOT EXISTS "${contract}")
        message(FATAL_ERROR "GRUB2 contract is unavailable: ${contract}")
    endif()
    file(STRINGS "${contract}" _matching_lines REGEX "^set\\(${variable} ")
    list(LENGTH _matching_lines _matching_count)
    if(NOT _matching_count EQUAL 1)
        message(FATAL_ERROR "GRUB2 contract has no unique ${variable} value")
    endif()
    list(GET _matching_lines 0 _line)
    set(_prefix "set(${variable} [==[")
    string(FIND "${_line}" "${_prefix}" _prefix_position)
    if(NOT _prefix_position EQUAL 0)
        message(FATAL_ERROR "GRUB2 contract ${variable} value has an unexpected format")
    endif()
    string(LENGTH "${_prefix}" _prefix_length)
    string(SUBSTRING "${_line}" ${_prefix_length} -1 _encoded_value)
    set(_suffix "]==])")
    string(LENGTH "${_encoded_value}" _encoded_length)
    string(LENGTH "${_suffix}" _suffix_length)
    if(_encoded_length LESS _suffix_length)
        message(FATAL_ERROR "GRUB2 contract ${variable} value is truncated")
    endif()
    math(EXPR _value_length "${_encoded_length} - ${_suffix_length}")
    string(SUBSTRING "${_encoded_value}" 0 ${_value_length} _value)
    string(SUBSTRING "${_encoded_value}" ${_value_length} -1 _actual_suffix)
    if(NOT _actual_suffix STREQUAL _suffix)
        message(FATAL_ERROR "GRUB2 contract ${variable} value has an unexpected terminator")
    endif()
    set(${output} "${_value}" PARENT_SCOPE)
endfunction()

function(_grub_assert_fallback_warning log context python version)
    string(REGEX REPLACE "[ \t\r\n]+" " " _normalized_log "${log}")
    set(_expected_warning
        "preferred Python 3.14 is not installed; using ${python} (${version})")
    string(FIND "${_normalized_log}" "${_expected_warning}" _warning_position)
    if(_warning_position LESS 0)
        message(FATAL_ERROR
            "python-fallback ${context} omitted the selected path/version warning: ${_expected_warning}\n${_normalized_log}")
    endif()
endfunction()

function(_grub_counterprobe_runner_python build hostile_path)
    set(_contract "${build}/.aros-grub2-host-grub2-contract.cmake")
    set(_binary "${build}/gen/configure/arch/all-pc/boot/grub2-host/pc")
    set(_install "${build}/hosttools/grub2/pc")
    set(_grub_mkimage "${_install}/grub-mkimage")
    set(_private_sentinel "${_binary}/runner-python-counterprobe.sentinel")
    set(_install_sentinel "${_install}/runner-python-counterprobe.sentinel")
    set(_probe_directory "${_root}/runner-python-counterprobes")
    if(NOT EXISTS "${_contract}" OR NOT EXISTS "${_grub_mkimage}")
        message(FATAL_ERROR "GRUB2 runner Python counterprobe inputs are unavailable")
    endif()
    file(MAKE_DIRECTORY "${_probe_directory}")
    file(WRITE "${_private_sentinel}" "private tree sentinel")
    file(WRITE "${_install_sentinel}" "install tree sentinel")
    file(SHA256 "${_grub_mkimage}" _grub_mkimage_before)
    set(_counterprobes
        "GB_PYTHON|/tmp/aros-grub-counterprobe-python|GRUB2 runner Python selection changed or was substituted"
        "GB_PYTHON_SELECTION|counterprobe-selection|GRUB2 runner Python selection changed or was substituted"
        "GB_PYTHON_REALPATH|/tmp/aros-grub-counterprobe-realpath|GRUB2 runner Python selection changed or was substituted"
        "GB_PYTHON_VERSION|3.0.0|GRUB2 runner Python version changed"
        "GB_PYTHON_SHA256|ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff|GRUB2 runner Python selection changed or was substituted")
    foreach(_counterprobe IN LISTS _counterprobes)
        string(REPLACE "|" ";" _parts "${_counterprobe}")
        list(GET _parts 0 _variable)
        list(GET _parts 1 _wrong_value)
        list(GET _parts 2 _expected_reason)
        set(_probe_contract "${_probe_directory}/${_variable}.cmake")
        file(COPY_FILE "${_contract}" "${_probe_contract}")
        file(APPEND "${_probe_contract}"
            "set(${_variable} [==[${_wrong_value}]==])\n")
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E env "PATH=${hostile_path}"
                "${CMAKE_COMMAND}" -DGB_ACTION=build
                "-DCONTRACT=${_probe_contract}"
                -P "${AROS_TEST_ENGINE_DIR}/RunGrubBuild.cmake"
            RESULT_VARIABLE _result
            OUTPUT_VARIABLE _stdout
            ERROR_VARIABLE _stderr)
        set(_log "${_stdout}${_stderr}")
        string(FIND "${_log}" "${_expected_reason}" _reason_position)
        if(_result EQUAL 0 OR _reason_position LESS 0)
            message(FATAL_ERROR
                "GRUB2 runner accepted or misdiagnosed ${_variable} counterprobe (${_result})\n${_log}")
        endif()
        foreach(_sentinel_pair IN ITEMS
                "${_private_sentinel}|private tree sentinel"
                "${_install_sentinel}|install tree sentinel")
            string(REPLACE "|" ";" _sentinel_parts "${_sentinel_pair}")
            list(GET _sentinel_parts 0 _sentinel)
            list(GET _sentinel_parts 1 _expected_content)
            if(NOT EXISTS "${_sentinel}")
                message(FATAL_ERROR
                    "GRUB2 runner removed a tree sentinel before rejecting ${_variable}")
            endif()
            file(READ "${_sentinel}" _actual_content)
            if(NOT _actual_content STREQUAL _expected_content)
                message(FATAL_ERROR
                    "GRUB2 runner changed a tree sentinel before rejecting ${_variable}")
            endif()
        endforeach()
        file(SHA256 "${_grub_mkimage}" _grub_mkimage_after)
        if(NOT _grub_mkimage_before STREQUAL _grub_mkimage_after)
            message(FATAL_ERROR
                "GRUB2 runner changed installed grub-mkimage before rejecting ${_variable}")
        endif()
    endforeach()
    file(REMOVE "${_private_sentinel}" "${_install_sentinel}")
endfunction()

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/grub-${_grub_version}-aros.diff" _patch_before)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/mmakefile.src" _aros_mmake_before)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _host_mmake_before)

_grub_configure("" TRUE "")
set(_build "${CONFIGURED_BUILD}")
set(_closed_host_path
    "/opt/homebrew/opt/gettext/bin:/opt/homebrew/opt/texinfo/bin:/opt/homebrew/opt/gawk/bin:/opt/homebrew/opt/pkgconf/bin:/opt/homebrew/opt/gnu-sed/bin:/opt/homebrew/opt/coreutils/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:/usr/bin:/bin:/usr/sbin:/sbin")
foreach(_contract_spec IN ITEMS
        "grub2-host|${_pc_count}"
        "grub2-efi-host|${_efi64_count}"
        "grub2-efi32-host|${_efi32_count}")
    string(REPLACE "|" ";" _contract_parts "${_contract_spec}")
    list(GET _contract_parts 0 _contract_target)
    list(GET _contract_parts 1 _contract_count)
    set(_contract "${_build}/.aros-${_contract_target}-grub2-contract.cmake")
    file(STRINGS "${_contract}" _contract_products
        REGEX "^list\\(APPEND GB_INSTALL_PRODUCTS ")
    list(LENGTH _contract_products _actual_contract_count)
    file(READ "${_contract}" _contract_content)
    string(FIND "${_contract_content}"
        "set(GB_HOST_PATH [==[${_closed_host_path}]==])" _host_path_position)
    string(FIND "${_contract_content}"
        "set(GB_SOURCE_URL_PRIMARY [==[${_primary_url}]==])"
        _primary_source_position)
    string(FIND "${_contract_content}"
        "set(GB_SOURCE_URL_FALLBACK [==[${_fallback_url}]==])"
        _fallback_source_position)
    if(NOT _actual_contract_count EQUAL _contract_count OR
       _host_path_position LESS 0 OR
       _primary_source_position LESS 0 OR
       _fallback_source_position LESS 0)
        message(FATAL_ERROR
            "${_contract_target} omitted its products, sources or closed host PATH")
    endif()
endforeach()

# The runner must not consult the caller's PATH. Every configure/make helper is
# passed through the closed, verified tool PATH stored in the lane contract.
set(_hostile_path "/tmp/aros-grub-inherited-path-must-not-be-used")
foreach(_target IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
            "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${_target} build failed (${_result})\n${_stdout}${_stderr}")
    endif()
endforeach()

foreach(_lane IN ITEMS pc efi-x86_64 efi-i386)
    if(_lane STREQUAL "pc")
        set(_platform i386-pc)
        set(_target grub2-host)
    elseif(_lane STREQUAL "efi-x86_64")
        set(_platform x86_64-efi)
        set(_target grub2-efi-host)
    else()
        set(_platform i386-efi)
        set(_target grub2-efi32-host)
    endif()
    foreach(_output IN ITEMS
            "${_build}/gen/configure/arch/all-pc/boot/grub2-host/${_lane}/build/grub-mkimage"
            "${_build}/gen/configure/arch/all-pc/boot/grub2-host/${_lane}/build/grub-core/kernel.img"
            "${_build}/hosttools/grub2/${_lane}/grub-mkimage"
            "${_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/normal.mod"
            "${_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/xzio.mod"
            "${_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/moddep.lst")
        if(NOT EXISTS "${_output}")
            message(FATAL_ERROR "${_target} omitted ${_output}")
        endif()
    endforeach()
endforeach()

_grub_counterprobe_runner_python("${_build}" "${_hostile_path}")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target grub2-host
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
set(_noop_log "${_noop_stdout}${_noop_stderr}")
if(NOT _noop_result EQUAL 0 OR NOT _noop_log MATCHES "no work to do")
    message(FATAL_ERROR "GRUB2 no-op check failed\n${_noop_log}")
endif()

_grub_configure("" TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${CONFIGURED_BUILD}" --target grub2-host
    RESULT_VARIABLE _reconfigure_result
    OUTPUT_VARIABLE _reconfigure_stdout
    ERROR_VARIABLE _reconfigure_stderr)
set(_reconfigure_log "${_reconfigure_stdout}${_reconfigure_stderr}")
if(NOT _reconfigure_result EQUAL 0 OR NOT _reconfigure_log MATCHES "no work to do")
    message(FATAL_ERROR "GRUB2 rebuilt after a no-op reconfigure\n${_reconfigure_log}")
endif()

set(_repair
    "${_build}/hosttools/grub2/efi-i386/lib/grub/i386-efi/fat.mod")
file(REMOVE "${_repair}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target grub2-efi32-host
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_repair}")
    message(FATAL_ERROR "GRUB2 repair check failed\n${_repair_stdout}${_repair_stderr}")
endif()

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/grub-${_grub_version}-aros.diff" _patch_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/mmakefile.src" _aros_mmake_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _host_mmake_after)
if(NOT _patch_before STREQUAL _patch_after OR
   NOT _aros_mmake_before STREQUAL _aros_mmake_after OR
   NOT _host_mmake_before STREQUAL _host_mmake_after)
    message(FATAL_ERROR "GRUB2 runner modified its source tree")
endif()

set(_fallback_engine "${_root}/python-fallback-engine")
file(MAKE_DIRECTORY "${_fallback_engine}")
file(COPY "${AROS_TEST_ENGINE_DIR}/" DESTINATION "${_fallback_engine}")
set(_fallback_python_helper "${_fallback_engine}/GrubPython.cmake")
set(_preferred_python "/opt/homebrew/opt/python@3.14/bin/python3.14")
set(_missing_preferred_python "${_root}/missing-preferred-python3.14")
if(NOT EXISTS "${_fallback_python_helper}" OR
   EXISTS "${_missing_preferred_python}" OR IS_SYMLINK "${_missing_preferred_python}")
    message(FATAL_ERROR "GRUB Python fallback fixture could not prepare its private engine copy")
endif()
file(READ "${_fallback_python_helper}" _fallback_policy)
string(FIND "${_fallback_policy}" "${_preferred_python}" _preferred_position)
if(_preferred_position LESS 0)
    message(FATAL_ERROR "GRUB Python preferred-path literal is absent from the copied policy")
endif()
string(LENGTH "${_preferred_python}" _preferred_length)
math(EXPR _after_preferred_position "${_preferred_position} + ${_preferred_length}")
string(SUBSTRING "${_fallback_policy}" ${_after_preferred_position} -1 _policy_after_preferred)
string(FIND "${_policy_after_preferred}" "${_preferred_python}" _duplicate_preferred_position)
if(NOT _duplicate_preferred_position EQUAL -1)
    message(FATAL_ERROR "GRUB Python preferred-path literal is not unique in the copied policy")
endif()
string(REPLACE "${_preferred_python}" "${_missing_preferred_python}"
    _modified_fallback_policy "${_fallback_policy}")
string(REPLACE "${_missing_preferred_python}" "${_preferred_python}"
    _restored_fallback_policy "${_modified_fallback_policy}")
if(NOT _restored_fallback_policy STREQUAL _fallback_policy)
    message(FATAL_ERROR "GRUB Python fallback fixture changed more than its preferred-path literal")
endif()
file(WRITE "${_fallback_python_helper}" "${_modified_fallback_policy}")
file(READ "${_fallback_python_helper}" _written_fallback_policy)
if(NOT _written_fallback_policy STREQUAL _modified_fallback_policy)
    message(FATAL_ERROR "GRUB Python fallback fixture did not write its private policy copy")
endif()

_grub_configure("python-fallback" TRUE "")
set(_fallback_build "${CONFIGURED_BUILD}")
set(_fallback_configure_log "${CONFIGURE_LOG}")
set(_fallback_contract "${_fallback_build}/.aros-grub2-host-grub2-contract.cmake")
_grub_contract_value("${_fallback_contract}" GB_PYTHON _fallback_python)
_grub_contract_value("${_fallback_contract}" GB_PYTHON_SELECTION _fallback_selection)
_grub_contract_value("${_fallback_contract}" GB_PYTHON_VERSION _fallback_version)
if(NOT _fallback_selection STREQUAL "fallback")
    message(FATAL_ERROR "python-fallback contract did not record a fallback selection")
endif()
_grub_assert_fallback_warning(
    "${_fallback_configure_log}" configure "${_fallback_python}" "${_fallback_version}")

foreach(_target IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
            "${CMAKE_COMMAND}" --build "${_fallback_build}" --target "${_target}"
        RESULT_VARIABLE _fallback_result
        OUTPUT_VARIABLE _fallback_stdout
        ERROR_VARIABLE _fallback_stderr)
    if(NOT _fallback_result EQUAL 0)
        message(FATAL_ERROR
            "python-fallback ${_target} build failed (${_fallback_result})\n${_fallback_stdout}${_fallback_stderr}")
    endif()
endforeach()

foreach(_lane IN ITEMS pc efi-x86_64 efi-i386)
    if(_lane STREQUAL "pc")
        set(_platform i386-pc)
        set(_target grub2-host)
    elseif(_lane STREQUAL "efi-x86_64")
        set(_platform x86_64-efi)
        set(_target grub2-efi-host)
    else()
        set(_platform i386-efi)
        set(_target grub2-efi32-host)
    endif()
    foreach(_output IN ITEMS
            "${_fallback_build}/gen/configure/arch/all-pc/boot/grub2-host/${_lane}/build/grub-mkimage"
            "${_fallback_build}/gen/configure/arch/all-pc/boot/grub2-host/${_lane}/build/grub-core/kernel.img"
            "${_fallback_build}/hosttools/grub2/${_lane}/grub-mkimage"
            "${_fallback_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/normal.mod"
            "${_fallback_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/xzio.mod"
            "${_fallback_build}/hosttools/grub2/${_lane}/lib/grub/${_platform}/moddep.lst")
        if(NOT EXISTS "${_output}")
            message(FATAL_ERROR "python-fallback ${_target} omitted ${_output}")
        endif()
    endforeach()
endforeach()

foreach(_target IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
            "${CMAKE_COMMAND}" --build "${_fallback_build}" --target "${_target}"
        RESULT_VARIABLE _fallback_noop_result
        OUTPUT_VARIABLE _fallback_noop_stdout
        ERROR_VARIABLE _fallback_noop_stderr)
    set(_fallback_noop_log "${_fallback_noop_stdout}${_fallback_noop_stderr}")
    if(NOT _fallback_noop_result EQUAL 0 OR
       NOT _fallback_noop_log MATCHES "no work to do")
        message(FATAL_ERROR
            "python-fallback ${_target} no-op build failed\n${_fallback_noop_log}")
    endif()
endforeach()

_grub_configure("python-fallback" TRUE "")
if(NOT CONFIGURED_BUILD STREQUAL _fallback_build)
    message(FATAL_ERROR "python-fallback reconfigure changed its build directory")
endif()
set(_fallback_reconfigure_log "${CONFIGURE_LOG}")
_grub_contract_value("${_fallback_contract}" GB_PYTHON _fallback_reconfigured_python)
_grub_contract_value("${_fallback_contract}" GB_PYTHON_SELECTION _fallback_reconfigured_selection)
_grub_contract_value("${_fallback_contract}" GB_PYTHON_VERSION _fallback_reconfigured_version)
if(NOT _fallback_reconfigured_selection STREQUAL "fallback" OR
   NOT _fallback_reconfigured_python STREQUAL _fallback_python OR
   NOT _fallback_reconfigured_version STREQUAL _fallback_version)
    message(FATAL_ERROR "python-fallback contract metadata changed during no-op reconfigure")
endif()
_grub_assert_fallback_warning("${_fallback_reconfigure_log}" reconfigure
    "${_fallback_reconfigured_python}" "${_fallback_reconfigured_version}")
foreach(_target IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
            "${CMAKE_COMMAND}" --build "${_fallback_build}" --target "${_target}"
        RESULT_VARIABLE _fallback_reconfigure_result
        OUTPUT_VARIABLE _fallback_reconfigure_stdout
        ERROR_VARIABLE _fallback_reconfigure_stderr)
    set(_fallback_reconfigure_log
        "${_fallback_reconfigure_stdout}${_fallback_reconfigure_stderr}")
    if(NOT _fallback_reconfigure_result EQUAL 0 OR
       NOT _fallback_reconfigure_log MATCHES "no work to do")
        message(FATAL_ERROR
            "python-fallback ${_target} rebuilt after a no-op reconfigure\n${_fallback_reconfigure_log}")
    endif()
endforeach()

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/grub-${_grub_version}-aros.diff" _fallback_patch_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/mmakefile.src" _fallback_aros_mmake_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _fallback_host_mmake_after)
if(NOT _patch_before STREQUAL _fallback_patch_after OR
   NOT _aros_mmake_before STREQUAL _fallback_aros_mmake_after OR
   NOT _host_mmake_before STREQUAL _fallback_host_mmake_after)
    message(FATAL_ERROR "GRUB2 python-fallback runner modified its source tree")
endif()

_grub_configure("wrong-identity" FALSE "target identity differs from the audited")
_grub_configure("symlink-binary" FALSE "GRUB2 build root escapes the build tree")
_grub_configure("engine-missing-manifest" FALSE "audited GRUB2 install manifest is unavailable")
_grub_configure("engine-symlink-manifest" FALSE "audited GRUB2 install manifest is unavailable")
_grub_configure("engine-symlink-parent-manifest" FALSE
    "GRUB2 engine manifest contains a symlinked path component")

file(REMOVE_RECURSE "${_root}")
message(STATUS "GRUB2 host build test passed")
