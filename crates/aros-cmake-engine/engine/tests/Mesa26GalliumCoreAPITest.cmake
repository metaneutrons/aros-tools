set(_tmp "$ENV{TMPDIR}")
if(NOT _tmp)
    set(_tmp "/tmp")
endif()
string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "${_tmp}/mesa26-gca-contract-${_suffix}")
file(MAKE_DIRECTORY "${_root}")
set(_verify "${CMAKE_CURRENT_LIST_DIR}/../VerifyMesa26GalliumCoreAPI.cmake")
set(_product "${_root}/product")
set(_map "${_root}/map")
set(_nm "${_root}/nm")
file(WRITE "${_product}" "fixture\n")
file(WRITE "${_map}" "alpha __gca_alpha\nbeta __gca_beta\n")
file(WRITE "${_nm}" "#!/bin/sh\n"
    "if [ \"$1\" = \"-u\" ]; then exit 0; fi\n"
    "printf '00000000 T __gca_alpha\\n00000000 T __gca_beta\\n00000000 T gca_bind\\n00000000 T vc4_screen_create\\n'\n")
file(CHMOD "${_nm}" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE)
file(SHA256 "${_map}" _map_sha)
file(SHA256 "${_nm}" _nm_sha)

function(_gca_probe expected pattern map_sha nm_sha)
    execute_process(COMMAND "${CMAKE_COMMAND}"
        "-DGCA_PRODUCT=${_product}" "-DGCA_KIND=vc4"
        "-DGCA_NM=${_nm}" "-DGCA_NM_SHA256=${nm_sha}"
        "-DGCA_MAP=${_map}" "-DGCA_MAP_SHA256=${map_sha}"
        -P "${_verify}"
        RESULT_VARIABLE _status OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(expected STREQUAL "success")
        if(NOT "${_status}" STREQUAL "0" OR
           NOT _stdout MATCHES "2 slots, linked symbols closed")
            message(FATAL_ERROR "Mesa 26 GCA positive probe failed: ${_stdout}${_stderr}")
        endif()
    elseif("${_status}" STREQUAL "0" OR
           NOT "${_stdout}${_stderr}" MATCHES "${pattern}")
        message(FATAL_ERROR "Mesa 26 GCA counterprobe failed: ${_stdout}${_stderr}")
    endif()
endfunction()

_gca_probe(success "" "${_map_sha}" "${_nm_sha}")
_gca_probe(failure "import map changed" "0000000000000000000000000000000000000000000000000000000000000000" "${_nm_sha}")
file(WRITE "${_nm}" "#!/bin/sh\n"
    "if [ \"$1\" = \"-u\" ]; then exit 0; fi\n"
    "printf '00000000 T __gca_alpha\\n00000000 T gca_bind\\n00000000 T vc4_screen_create\\n'\n")
file(SHA256 "${_nm}" _missing_nm_sha)
_gca_probe(failure "trampoline is missing: beta" "${_map_sha}" "${_missing_nm_sha}")
_gca_probe(failure "verifier tool or import map changed" "${_map_sha}" "${_nm_sha}")
execute_process(COMMAND "${CMAKE_COMMAND}"
    "-DGCA_AR=${_nm}" "-DGCA_OBJCOPY=${_nm}" "-DGCA_NM=${_nm}"
    "-DGCA_GENERATED=${_root}" "-DGCA_RAW_VC4=${_product}"
    "-DGCA_VC4_OUTPUT=${_root}/archive" "-DGCA_BIND_OBJECTS=${_product}"
    "-DGCA_VC4_DRM_OBJECTS=${_product}"
    -P "${CMAKE_CURRENT_LIST_DIR}/../RunMesa26GalliumCoreAPI.cmake"
    RESULT_VARIABLE _missing_map_status ERROR_VARIABLE _missing_map_error)
if("${_missing_map_status}" STREQUAL "0" OR
   NOT _missing_map_error MATCHES "lacks GCA_EXPECTED_MAP_SHA256")
    message(FATAL_ERROR "Mesa 26 GCA archive rewrite accepted an unpinned import map")
endif()

# The ABI hash input is the consumer's own target, ISA flags and defines, in
# declared order. It has to be stable for equal input and move with any of it.
include("${CMAKE_CURRENT_LIST_DIR}/../Mesa26GalliumCoreAPI.cmake")
set(_defines GALLIUM_VC4 HAVE_STRUCT_TIMESPEC GCA_CONSUMER_MODULE AROS_MESA_MAJOR=26)
_aros_mesa26_gca_abi_flags(_aarch64 "aarch64-unknown-aros" "" "${_defines}")
if(NOT _aarch64 STREQUAL "--target=aarch64-unknown-aros -DGALLIUM_VC4 -DHAVE_STRUCT_TIMESPEC -DGCA_CONSUMER_MODULE -DAROS_MESA_MAJOR=26")
    message(FATAL_ERROR "Mesa 26 GCA aarch64 ABI flags are wrong: ${_aarch64}")
endif()
_aros_mesa26_gca_abi_flags(_again "aarch64-unknown-aros" "" "${_defines}")
if(NOT _again STREQUAL _aarch64)
    message(FATAL_ERROR "Mesa 26 GCA ABI flags are not deterministic")
endif()
_aros_mesa26_gca_abi_flags(_arm "arm-unknown-aros"
    " -mcpu=cortex-a7  -mfpu=neon-vfpv4 -mfloat-abi=hard" "${_defines}")
if(NOT _arm STREQUAL "--target=arm-unknown-aros -mcpu=cortex-a7 -mfpu=neon-vfpv4 -mfloat-abi=hard -DGALLIUM_VC4 -DHAVE_STRUCT_TIMESPEC -DGCA_CONSUMER_MODULE -DAROS_MESA_MAJOR=26")
    message(FATAL_ERROR "Mesa 26 GCA arm ABI flags are wrong: ${_arm}")
endif()
_aros_mesa26_gca_abi_flags(_changed "aarch64-unknown-aros" ""
    "GALLIUM_VC4;HAVE_STRUCT_TIMESPEC;GCA_CONSUMER_MODULE;AROS_MESA_MAJOR=27")
if(_changed STREQUAL _aarch64)
    message(FATAL_ERROR "Mesa 26 GCA ABI flags ignore a changed define")
endif()
file(WRITE "${_root}/empty-defines.cmake"
    "include(\"\${GCA_MODULE}\")\n"
    "_aros_mesa26_gca_abi_flags(_flags \"aarch64-unknown-aros\" \"\" \"\")\n")
execute_process(COMMAND "${CMAKE_COMMAND}"
    "-DGCA_MODULE=${CMAKE_CURRENT_LIST_DIR}/../Mesa26GalliumCoreAPI.cmake"
    -P "${_root}/empty-defines.cmake"
    RESULT_VARIABLE _empty_status ERROR_VARIABLE _empty_error)
if("${_empty_status}" STREQUAL "0" OR
   NOT _empty_error MATCHES "ABI flags need[ \n]+the consumer defines")
    message(FATAL_ERROR "Mesa 26 GCA ABI flags accepted a consumer without defines")
endif()
message(STATUS "Mesa 26 GalliumCoreAPI positive and mutated-input probes passed")
