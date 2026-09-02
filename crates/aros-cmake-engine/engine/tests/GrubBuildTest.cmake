cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/grub-build")
# CMake script mode deliberately leaves CMAKE_HOST_SYSTEM_* unset.  The
# configured fixture owns the configure-time host assertion, while the runner
# independently verifies uname before it can fetch, extract or remove paths.

string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-grub-build-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")

function(_grub_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
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
endfunction()

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/grub-2.12-aros.diff" _patch_before)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/mmakefile.src" _aros_mmake_before)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _host_mmake_before)

_grub_configure("" TRUE "")
set(_build "${CONFIGURED_BUILD}")
set(_closed_host_path
    "/opt/homebrew/opt/gettext/bin:/opt/homebrew/opt/texinfo/bin:/opt/homebrew/opt/gawk/bin:/opt/homebrew/opt/pkgconf/bin:/opt/homebrew/opt/python@3.14/bin:/opt/homebrew/opt/gnu-sed/bin:/opt/homebrew/opt/coreutils/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:/usr/bin:/bin:/usr/sbin:/sbin")
foreach(_contract_spec IN ITEMS
        "grub2-host|615"
        "grub2-efi-host|591"
        "grub2-efi32-host|593")
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
    if(NOT _actual_contract_count EQUAL _contract_count OR
       _host_path_position LESS 0)
        message(FATAL_ERROR
            "${_contract_target} omitted its complete products or closed host PATH")
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

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/grub-2.12-aros.diff" _patch_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-aros/mmakefile.src" _aros_mmake_after)
file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _host_mmake_after)
if(NOT _patch_before STREQUAL _patch_after OR
   NOT _aros_mmake_before STREQUAL _aros_mmake_after OR
   NOT _host_mmake_before STREQUAL _host_mmake_after)
    message(FATAL_ERROR "GRUB2 runner modified its source tree")
endif()

_grub_configure("wrong-identity" FALSE "target identity differs from the audited")
_grub_configure("symlink-binary" FALSE "GRUB2 build root escapes the build tree")

file(REMOVE_RECURSE "${_root}")
message(STATUS "GRUB2 host build test passed")
