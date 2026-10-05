cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_suite_root "${_temp_root}/aros-gnu-consumer-${_suffix}")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/gnu-release-toolchain")
set(_toolchain "${CMAKE_CURRENT_LIST_DIR}/../toolchains/AROS.cmake")
set(_profile "riscv64-consumer-fixture")
set(_compiler_profile "riscv64-compiler-fixture")
set(_platform "boardless-fixture")
set(_triple "riscv64-aros")
set(_compiler_identity
    "{\"family\":\"gnu\",\"gcc_version\":\"16.2.0\",\"binutils_version\":\"2.47\",\"target\":{\"schema\":\"aros-riscv-target-v1\",\"isa\":\"rva22u64\",\"abi\":\"lp64d\",\"code_model\":\"medany\",\"architecture\":\"rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0\",\"unaligned_access\":false,\"atomic_abi\":0,\"x3_reg_usage\":0}}")

function(_make_gnu_prefix out_prefix name case_name)
    set(_prefix "${_suite_root}/${name} with spaces")
    set(_manifest_profile "${_compiler_profile}")
    if(ARGC GREATER 3)
        set(_manifest_profile "${ARGV3}")
    endif()
    file(MAKE_DIRECTORY "${_prefix}/bin" "${_prefix}/lib/gcc")
    set(_tool_roles
        "c|bin/gcc"
        "cxx|bin/g++"
        "assembler|bin/as"
        "linker|bin/ld"
        "archive|bin/ar"
        "ranlib|bin/ranlib"
        "strip|bin/strip"
        "collector|bin/collect-aros"
        "nm|bin/nm"
        "objcopy|bin/objcopy")
    set(_layout_schema "aros-toolchain-tools-v2")
    if(case_name MATCHES "^v3-")
        set(_layout_schema "aros-toolchain-tools-v3")
        if(NOT case_name STREQUAL "v3-missing-objdump-role")
            list(APPEND _tool_roles "objdump|bin/objdump")
        endif()
    endif()
    set(_linker_role "bin/ld")
    if(case_name STREQUAL "unsafe-path")
        set(_linker_role "../bin/ld")
    elseif(case_name STREQUAL "escaped-symlink")
        set(_linker_role "bin/ld-link")
        set(_outside_linker "${_suite_root}/outside-linker")
        file(WRITE "${_outside_linker}" "outside linker\n")
        file(CHMOD "${_outside_linker}" PERMISSIONS OWNER_READ OWNER_EXECUTE
            GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
        file(REMOVE "${_prefix}/bin/ld")
        file(WRITE "${_prefix}/bin/ld-real" "unused in-root linker\n")
        file(CHMOD "${_prefix}/bin/ld-real" PERMISSIONS OWNER_READ OWNER_EXECUTE
            GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
        file(CREATE_LINK "${_outside_linker}" "${_prefix}/bin/ld-link"
            SYMBOLIC RESULT _link_result)
        if(NOT _link_result STREQUAL "0")
            message(FATAL_ERROR "Could not create escaped symlink fixture: ${_link_result}")
        endif()
    endif()

    set(_driver [=[#!/bin/sh
have_march=no
have_mabi=no
have_mcmodel=no
for arg in "$@"; do
    case "$arg" in
        -march=rva22u64) have_march=yes ;;
        -mabi=lp64d) have_mabi=yes ;;
        -mcmodel=medany) have_mcmodel=yes ;;
    esac
done
if [ "$have_march" != yes ] || [ "$have_mabi" != yes ] || [ "$have_mcmodel" != yes ]; then
    echo "missing manifest target flags" >&2
    exit 91
fi
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=$(CDPATH= cd -- "$script_dir/.." && pwd)
case "$*" in
    *-print-libgcc-file-name*)
        printf '%s\n' "@LIBGCC@"
        ;;
    *-print-file-name=libstdc++.a*)
        printf '%s\n' "$prefix/lib/gcc/libstdc++.a"
        ;;
    *-print-file-name=libsupc++.a*)
        printf '%s\n' "$prefix/lib/gcc/libsupc++.a"
        ;;
    *)
        exit 0
        ;;
esac
]=])
if(case_name STREQUAL "missing-runtime")
    set(_libgcc "${_prefix}/lib/gcc/missing-libgcc.a")
elseif(case_name STREQUAL "escaped-runtime")
    set(_libgcc "${_suite_root}/outside-libgcc.a")
    file(WRITE "${_libgcc}" "outside runtime\n")
elseif(case_name STREQUAL "wrong-runtime-flags")
    set(_libgcc "${_prefix}/lib/gcc/libgcc.a")
else()
    set(_libgcc "${_prefix}/lib/gcc/libgcc.a")
endif()
string(REPLACE "@LIBGCC@" "${_libgcc}" _driver "${_driver}")
foreach(_tool IN ITEMS bin/gcc bin/g++)
    file(WRITE "${_prefix}/${_tool}" "${_driver}")
    file(CHMOD "${_prefix}/${_tool}" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
endforeach()
set(_fixture_tools bin/as bin/ld bin/ar bin/ranlib bin/strip
    bin/collect-aros bin/nm bin/objcopy)
if(_layout_schema STREQUAL "aros-toolchain-tools-v3" AND
   NOT case_name STREQUAL "v3-missing-objdump-file")
    list(APPEND _fixture_tools bin/objdump)
endif()
foreach(_tool IN LISTS _fixture_tools)
    if(NOT case_name STREQUAL "escaped-symlink" OR NOT _tool STREQUAL "bin/ld")
        file(WRITE "${_prefix}/${_tool}" "#!/bin/sh\nexit 0\n")
        file(CHMOD "${_prefix}/${_tool}" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE
            GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
    endif()
endforeach()
foreach(_archive IN ITEMS libgcc.a libstdc++.a libsupc++.a)
    file(WRITE "${_prefix}/lib/gcc/${_archive}" "fixture archive ${_archive}\n")
endforeach()
if(case_name STREQUAL "escaped-symlink")
    # Keep the real role path in the manifest as a link; the resolver must
    # reject its canonical target before any tool is launched.
    set(_symlink_target "${_outside_linker}")
endif()

set(_family "gnu")
if(case_name STREQUAL "wrong-family")
    set(_family "llvm")
endif()
set(_target_tools "")
foreach(_pair IN LISTS _tool_roles)
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _role)
    list(GET _parts 1 _relative)
    if(_role STREQUAL "linker")
        set(_relative "${_linker_role}")
    elseif(_role STREQUAL "objdump" AND case_name STREQUAL "v3-unsafe-objdump")
        set(_relative "../bin/objdump")
    endif()
    string(APPEND _target_tools "\"${_role}\":\"${_relative}\",")
endforeach()
string(REGEX REPLACE ",$" "" _target_tools "${_target_tools}")
set(_layout_compiler "${_compiler_identity}")
if(NOT _family STREQUAL "gnu")
    string(REPLACE "\"family\":\"gnu\"" "\"family\":\"llvm\""
        _layout_compiler "${_layout_compiler}")
endif()
set(_layout_json
    "{\"schema\":\"${_layout_schema}\",\"compiler\":${_layout_compiler},\"target_triple\":\"${_triple}\",\"tools\":{${_target_tools}}}\n")
file(WRITE "${_prefix}/toolchain-tools.json" "${_layout_json}")

set(_manifest_paths
    "bin/gcc;bin/g++;bin/as;bin/ld;bin/ar;bin/ranlib;bin/strip;bin/collect-aros;bin/nm;bin/objcopy;lib/gcc/libgcc.a;lib/gcc/libstdc++.a;lib/gcc/libsupc++.a;toolchain-tools.json")
if(_layout_schema STREQUAL "aros-toolchain-tools-v3" AND
   NOT case_name STREQUAL "v3-missing-objdump-file")
    list(APPEND _manifest_paths "bin/objdump")
endif()
if(case_name STREQUAL "escaped-symlink")
    list(REMOVE_ITEM _manifest_paths "bin/ld")
    list(APPEND _manifest_paths "bin/ld-link")
endif()
list(SORT _manifest_paths)
set(_file_records "")
foreach(_relative IN LISTS _manifest_paths)
    if(_relative STREQUAL "bin/ld-link")
        set(_record "{\"path\":\"${_relative}\",\"mode\":\"0777\",\"type\":\"symlink\",\"target\":\"${_symlink_target}\"}")
    else()
        if(_relative STREQUAL "toolchain-tools.json")
            set(_mode "0644")
        elseif(_relative MATCHES "^lib/")
            set(_mode "0644")
        else()
            set(_mode "0755")
        endif()
        file(SHA256 "${_prefix}/${_relative}" _sha)
        file(SIZE "${_prefix}/${_relative}" _size)
        set(_record "{\"path\":\"${_relative}\",\"mode\":\"${_mode}\",\"type\":\"file\",\"sha256\":\"${_sha}\",\"size\":${_size}}")
    endif()
    if(NOT _file_records STREQUAL "")
        string(APPEND _file_records ",")
    endif()
    string(APPEND _file_records "${_record}")
endforeach()
set(_manifest_json
    "{\"schema\":2,\"release_id\":\"gnu-fixture-v1\",\"host\":\"fixture-host\",\"target_profile\":\"${_manifest_profile}\",\"target_triple\":\"${_triple}\",\"tree_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"compiler\":${_compiler_identity},\"recipe_sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"source_lock_sha256\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"profiles_sha256\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"source_commit\":\"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\",\"producer_commit\":\"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\",\"tools_commit\":\"ffffffffffffffffffffffffffffffffffffffff\",\"source_date_epoch\":1,\"capabilities\":[\"c\",\"libgcc\"],\"build_environment\":{},\"files\":[${_file_records}]}\n")
file(WRITE "${_prefix}/toolchain-manifest.json" "${_manifest_json}")
if(case_name STREQUAL "layout-hash")
    file(APPEND "${_prefix}/toolchain-tools.json" " ")
endif()
file(REAL_PATH "${_prefix}" _canonical_prefix)
set(_prefix "${_canonical_prefix}")
set(${out_prefix} "${_prefix}" PARENT_SCOPE)
endfunction()

function(_configure_gnu out_result out_text label prefix requested_triple c_compiler)
    set(_expected_layout_schema "aros-toolchain-tools-v2")
    if(ARGC GREATER 6)
        set(_expected_layout_schema "${ARGV6}")
    endif()
    if(_expected_layout_schema STREQUAL "aros-toolchain-tools-v3")
        set(_ambient_objdump "${prefix}/bin/objdump")
    else()
        # A v2 prefix has no objdump role; this ambient value must be cleared.
        set(_ambient_objdump "${prefix}/bin/objcopy")
    endif()
    if(ARGC GREATER 7)
        set(_ambient_objdump "${ARGV7}")
    endif()
    set(_requested_compiler_profile "${_compiler_profile}")
    set(_expected_compiler_profile "${_compiler_profile}")
    set(_set_compiler_profile TRUE)
    if(ARGC GREATER 8)
        if("${ARGV8}" STREQUAL "UNSET")
            set(_set_compiler_profile FALSE)
            set(_expected_compiler_profile "${_profile}")
        else()
            set(_requested_compiler_profile "${ARGV8}")
        endif()
    endif()
    set(_compiler_profile_argument "")
    if(_set_compiler_profile)
        list(APPEND _compiler_profile_argument
            "-DAROS_CROSS_TOOLCHAIN_PROFILE=${_requested_compiler_profile}")
    endif()
    set(_build "${_suite_root}/build-${label}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
            "-DAROS_TOOLCHAIN=gnu"
            "-DAROS_CROSS_TOOLCHAIN_ROOT=${prefix}"
            "-DAROS_TARGET_CPU=riscv64"
            "-DAROS_TARGET_PLATFORM=${_platform}"
            "-DAROS_TARGET_PROFILE=${_profile}"
            ${_compiler_profile_argument}
            "-DAROS_TARGET_TRIPLE=${requested_triple}"
            "-DEXPECTED_PROFILE=${_profile}"
            "-DEXPECTED_COMPILER_PROFILE=${_expected_compiler_profile}"
            "-DEXPECTED_PLATFORM=${_platform}"
            "-DEXPECTED_TRIPLE=${_triple}"
            "-DEXPECTED_TOOL_LAYOUT_SCHEMA=${_expected_layout_schema}"
            "-DCMAKE_C_COMPILER=${c_compiler}"
            "-DCMAKE_CXX_COMPILER=${prefix}/bin/g++"
            "-DCMAKE_ASM_COMPILER=${prefix}/bin/gcc"
            "-DCMAKE_AR=${prefix}/bin/ar"
            "-DCMAKE_RANLIB=${prefix}/bin/ranlib"
            "-DCMAKE_STRIP=${prefix}/bin/strip"
            "-DCMAKE_NM=${prefix}/bin/nm"
            "-DCMAKE_OBJCOPY=${prefix}/bin/objcopy"
            "-DCMAKE_OBJDUMP=${_ambient_objdump}"
            "-DAROS_AS_BIN=${prefix}/bin/as"
            "-DAROS_LINKER_BIN=${prefix}/bin/ld"
            "-DAROS_COLLECT_BIN=${prefix}/bin/collect-aros"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(${out_result} "${_result}" PARENT_SCOPE)
    set(${out_text} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

function(_expect_gnu_failure label prefix requested_triple c_compiler expected_text)
    _configure_gnu(_result _output "${label}" "${prefix}"
        "${requested_triple}" "${c_compiler}" ${ARGN})
    if(_result EQUAL 0)
        message(FATAL_ERROR "GNU fixture ${label} unexpectedly configured")
    endif()
    string(FIND "${_output}" "${expected_text}" _match)
    if(_match EQUAL -1)
        message(FATAL_ERROR
            "GNU fixture ${label} failed for the wrong reason (${_result})\n${_output}")
    endif()
endfunction()

file(MAKE_DIRECTORY "${_suite_root}")
_make_gnu_prefix(_good "positive" "positive")
_configure_gnu(_positive_result _positive_output positive "${_good}"
    "${_triple}" "${_good}/bin/gcc")
if(NOT _positive_result EQUAL 0)
    message(FATAL_ERROR
        "GNU arbitrary-profile toolchain fixture failed (${_positive_result})\n${_positive_output}")
endif()

_expect_gnu_failure(wrong-compiler-profile "${_good}" "${_triple}"
    "${_good}/bin/gcc" "manifest selects" "aros-toolchain-tools-v2"
    "${_good}/bin/objcopy" wrong-compiler-profile)

_make_gnu_prefix(_fallback "direct-cmake-fallback" positive "${_profile}")
_configure_gnu(_fallback_result _fallback_output direct-cmake-fallback "${_fallback}"
    "${_triple}" "${_fallback}/bin/gcc" "aros-toolchain-tools-v2"
    "${_fallback}/bin/objcopy" UNSET)
if(NOT _fallback_result EQUAL 0)
    message(FATAL_ERROR
        "GNU direct-CMake profile fallback failed (${_fallback_result})\n${_fallback_output}")
endif()

_make_gnu_prefix(_good_v3 positive-v3 v3-positive)
_configure_gnu(_v3_result _v3_output positive-v3 "${_good_v3}"
    "${_triple}" "${_good_v3}/bin/gcc" "aros-toolchain-tools-v3")
if(NOT _v3_result EQUAL 0)
    message(FATAL_ERROR "GNU v3 toolchain fixture failed (${_v3_result})\n${_v3_output}")
endif()

_make_gnu_prefix(_missing_v3_role missing-v3-role v3-missing-objdump-role)
_expect_gnu_failure(missing-v3-objdump-role "${_missing_v3_role}" "${_triple}"
    "${_missing_v3_role}/bin/gcc" "GNU toolchain GNU executable roles has 10 fields; expected 11"
    "aros-toolchain-tools-v3")

_make_gnu_prefix(_missing_v3_file missing-v3-file v3-missing-objdump-file)
_expect_gnu_failure(missing-v3-objdump-file "${_missing_v3_file}" "${_triple}"
    "${_missing_v3_file}/bin/gcc" "GNU toolchain objdump role is missing or not a file"
    "aros-toolchain-tools-v3")

_make_gnu_prefix(_unsafe_v3 unsafe-v3 v3-unsafe-objdump)
_expect_gnu_failure(unsafe-v3-objdump "${_unsafe_v3}" "${_triple}"
    "${_unsafe_v3}/bin/gcc" "GNU toolchain objdump role contains an unsafe path segment"
    "aros-toolchain-tools-v3")

_make_gnu_prefix(_ambient_v3 ambient-v3 v3-positive)
_expect_gnu_failure(ambient-v3-objdump "${_ambient_v3}" "${_triple}"
    "${_ambient_v3}/bin/gcc"
    "GNU toolchain CMAKE_OBJDUMP does not match its inventory-bound role"
    "aros-toolchain-tools-v3" "${_ambient_v3}/bin/objcopy")

_make_gnu_prefix(_wrong_family wrong-family wrong-family)
_expect_gnu_failure(wrong-family "${_wrong_family}" "${_triple}"
    "${_wrong_family}/bin/gcc" "compiler family differs")

_make_gnu_prefix(_wrong_triple wrong-triple positive)
_expect_gnu_failure(wrong-triple "${_wrong_triple}" "riscv-aros"
    "${_wrong_triple}/bin/gcc" "manifest selects")

_make_gnu_prefix(_bad_hash bad-hash layout-hash)
_expect_gnu_failure(layout-hash "${_bad_hash}" "${_triple}"
    "${_bad_hash}/bin/gcc" "toolchain-tools.json bytes differ")

_make_gnu_prefix(_unsafe unsafe-path unsafe-path)
_expect_gnu_failure(unsafe-path "${_unsafe}" "${_triple}"
    "${_unsafe}/bin/gcc" "unsafe path segment")

if(CMAKE_HOST_UNIX)
    _make_gnu_prefix(_escaped_link escaped-link escaped-symlink)
    _expect_gnu_failure(escaped-symlink "${_escaped_link}" "${_triple}"
        "${_escaped_link}/bin/gcc" "resolves outside its prefix")
endif()

_make_gnu_prefix(_missing_runtime missing-runtime missing-runtime)
_expect_gnu_failure(missing-runtime "${_missing_runtime}" "${_triple}"
    "${_missing_runtime}/bin/gcc" "missing or not a file")

_make_gnu_prefix(_escaped_runtime escaped-runtime escaped-runtime)
_expect_gnu_failure(escaped-runtime "${_escaped_runtime}" "${_triple}"
    "${_escaped_runtime}/bin/gcc" "resolves outside its prefix")

_make_gnu_prefix(_wrong_driver wrong-driver positive)
_expect_gnu_failure(wrong-driver "${_wrong_driver}" "${_triple}"
    "${_suite_root}/unrelated-gcc" "CMAKE_C_COMPILER does not match")

file(REMOVE_RECURSE "${_suite_root}")
message(STATUS "GNU release-toolchain contract test passed")
