cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros pc boot iso preparation ${_suffix}")
cmake_path(NORMAL_PATH _root)
set(_prepare_script "${CMAKE_CURRENT_LIST_DIR}/../PreparePcBootIso.cmake")
set(_source_probe_startup
    "${CMAKE_CURRENT_LIST_DIR}/runtime/llvmpipe-jit-startup")

function(_prepare_case case root_out helper_out startup_out)
    set(_case_root "${_root}/${case}")
    set(_build_dir "${_case_root}/build")
    set(_sys_dir "${_build_dir}/SYS")
    set(_stage_dir "${_build_dir}/gen/boot-iso/stage")
    set(_engine_dir "${_case_root}/engine")
    set(_startup_source "${_case_root}/source/Startup-Sequence")
    set(_config_source "${_case_root}/config/grub.cfg")
    set(_private_grub
        "${_build_dir}/gen/grub2-iso-assets/x86_64/pc/grub2_eltorito")
    set(_grub_stamp
        "${_build_dir}/gen/grub2-iso-assets/x86_64/.grub2-iso-assets.stamp")
    set(_probe_startup "${_engine_dir}/tests/runtime/llvmpipe-jit-startup")
    file(MAKE_DIRECTORY
        "${_sys_dir}/boot/pc"
        "${_sys_dir}/boot/grub/i386-pc"
        "${_sys_dir}/S"
        "${_sys_dir}/Devs/Drivers"
        "${_sys_dir}/Developer/Debug/Tests/graphics/gl"
        "${_case_root}/source"
        "${_case_root}/config"
        "${_build_dir}/gen/grub2-iso-assets/x86_64/pc"
        "${_engine_dir}/tests/runtime")
    file(COPY_FILE "${_prepare_script}"
        "${_engine_dir}/PreparePcBootIso.cmake")
    file(WRITE "${_sys_dir}/boot/pc/bootstrap" "fixture bootstrap\n")
    file(WRITE "${_sys_dir}/boot/pc/kernel" "fixture kernel\n")
    file(WRITE "${_sys_dir}/S/Startup-Sequence" "native startup stays unchanged\n")
    file(WRITE "${_startup_source}" "staged startup sequence\n")
    file(WRITE "${_config_source}" "    module2 /boot/pc/kernel\n")
    file(WRITE "${_sys_dir}/boot/grub/i386-pc/grub2_eltorito"
        "fixture audited grub image\n")
    file(COPY_FILE "${_sys_dir}/boot/grub/i386-pc/grub2_eltorito"
        "${_private_grub}")
    file(SHA256 "${_private_grub}" _grub_digest)
    file(WRITE "${_grub_stamp}"
        "GRUB2 ISO assets: 2.12 x86_64\nEl Torito SHA256: ${_grub_digest}\n")

    if(case MATCHES "^(opt-in|preexisting-user-startup|missing-hidd|symlink-hidd|missing-probe|symlink-probe|missing-runner|symlink-runner|missing-startup|symlink-startup)$")
        file(WRITE "${_sys_dir}/Devs/Drivers/llvmpipe.hidd"
            "fixture llvmpipe HIDD\n")
        file(WRITE "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit"
            "fixture llvmpipe probe executable\n")
        file(WRITE "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit-runner"
            "fixture llvmpipe supervisor executable\n")
        if(NOT case STREQUAL "missing-startup")
            file(COPY_FILE "${_source_probe_startup}" "${_probe_startup}")
        endif()
        if(case STREQUAL "preexisting-user-startup")
            file(WRITE "${_sys_dir}/S/User-Startup" "user owned\n")
        elseif(case STREQUAL "missing-hidd")
            file(REMOVE "${_sys_dir}/Devs/Drivers/llvmpipe.hidd")
        elseif(case STREQUAL "symlink-hidd")
            file(WRITE "${_case_root}/llvmpipe-hidd-target" "fixture HIDD\n")
            file(REMOVE "${_sys_dir}/Devs/Drivers/llvmpipe.hidd")
            file(CREATE_LINK "${_case_root}/llvmpipe-hidd-target"
                "${_sys_dir}/Devs/Drivers/llvmpipe.hidd" SYMBOLIC)
        elseif(case STREQUAL "missing-probe")
            file(REMOVE
                "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit")
        elseif(case STREQUAL "symlink-probe")
            file(WRITE "${_case_root}/llvmpipe-probe-target" "fixture probe\n")
            file(REMOVE
                "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit")
            file(CREATE_LINK "${_case_root}/llvmpipe-probe-target"
                "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit"
                SYMBOLIC)
        elseif(case STREQUAL "symlink-startup")
            file(WRITE "${_case_root}/probe-startup-target" "fixture startup\n")
            file(REMOVE "${_probe_startup}")
            file(CREATE_LINK "${_case_root}/probe-startup-target"
                "${_probe_startup}" SYMBOLIC)
        elseif(case STREQUAL "missing-runner")
            file(REMOVE "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit-runner")
        elseif(case STREQUAL "symlink-runner")
            file(REMOVE "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit-runner")
            file(CREATE_LINK "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit"
                "${_sys_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit-runner" SYMBOLIC)
        endif()
    endif()

    set(${root_out} "${_case_root}" PARENT_SCOPE)
    set(${helper_out} "${_engine_dir}/PreparePcBootIso.cmake" PARENT_SCOPE)
    set(${startup_out} "${_probe_startup}" PARENT_SCOPE)
endfunction()

function(_run_case case with_probe expect_success expected_message)
    _prepare_case("${case}" _case_root _helper _probe_startup)
    set(_build_dir "${_case_root}/build")
    set(_sys_dir "${_build_dir}/SYS")
    set(_stage_dir "${_build_dir}/gen/boot-iso/stage")
    set(_config_source "${_case_root}/config/grub.cfg")
    set(_startup_source "${_case_root}/source/Startup-Sequence")
    set(_private_grub
        "${_build_dir}/gen/grub2-iso-assets/x86_64/pc/grub2_eltorito")
    set(_grub_stamp
        "${_build_dir}/gen/grub2-iso-assets/x86_64/.grub2-iso-assets.stamp")
    set(_probe_definition "-DLLVMPIPE_PROBE_STARTUP=")
    if(with_probe)
        set(_probe_definition "-DLLVMPIPE_PROBE_STARTUP=${_probe_startup}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            "-DSYS_DIR=${_sys_dir}"
            "-DSTAGE_DIR=${_stage_dir}"
            "-DCONFIG_SOURCE=${_config_source}"
            "-DSTARTUP_SOURCE=${_startup_source}"
            "${_probe_definition}"
            -DCPU_SIGNATURE=pc
            "-DGRUB2_STAMP=${_grub_stamp}"
            "-DGRUB2_PRIVATE_IMAGE=${_private_grub}"
            -P "${_helper}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}\n${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "PreparePcBootIso ${case} failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "PreparePcBootIso ${case} unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "PreparePcBootIso ${case} missed '${expected_message}':\n${_log}")
        endif()
    endif()

    if(case STREQUAL "default-off")
        if(EXISTS "${_stage_dir}/S/User-Startup" OR
           EXISTS "${_stage_dir}/Devs/Drivers/llvmpipe.hidd" OR
           EXISTS "${_stage_dir}/Developer/Debug/Tests/graphics/gl/llvmpipe-jit")
            message(FATAL_ERROR "default boot ISO stage contains llvmpipe probe inputs")
        endif()
    elseif(case STREQUAL "opt-in")
        if(NOT EXISTS "${_stage_dir}/S/User-Startup" OR
           EXISTS "${_sys_dir}/S/User-Startup")
            message(FATAL_ERROR
                "opt-in User-Startup was not isolated to the boot ISO stage")
        endif()
        file(READ "${_stage_dir}/S/User-Startup" _staged_probe_startup)
        file(READ "${_probe_startup}" _source_probe_startup)
        file(READ "${_sys_dir}/S/Startup-Sequence" _native_startup)
        if(NOT _staged_probe_startup STREQUAL _source_probe_startup OR
           NOT _native_startup STREQUAL "native startup stays unchanged\n")
            message(FATAL_ERROR
                "boot ISO probe startup differs from source or altered native SYS")
        endif()
    endif()
endfunction()

_run_case(default-off FALSE TRUE "")
_run_case(opt-in TRUE TRUE "")
_run_case(preexisting-user-startup TRUE FALSE
    "boot-iso refuses an unsafe llvmpipe probe startup")
foreach(_input IN ITEMS hidd probe runner startup)
    _run_case("missing-${_input}" TRUE FALSE
        "boot-iso is missing a regular llvmpipe probe input")
    _run_case("symlink-${_input}" TRUE FALSE
        "boot-iso is missing a regular llvmpipe probe input")
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "PC boot ISO probe startup isolation tests passed")
