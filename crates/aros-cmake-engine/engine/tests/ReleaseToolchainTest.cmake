cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-release-toolchain-${_suffix}")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/release-toolchain")
set(_toolchain "${CMAKE_CURRENT_LIST_DIR}/../toolchains/AROS.cmake")

set(_tools clang clang++ ld.lld llvm-ar llvm-ranlib llvm-nm llvm-strip
    llvm-objcopy llvm-objdump)
set(_profiles
    "x86_64|pc|pc-x86_64|x86_64-unknown-aros|x86_64|i386"
    "arm|raspi|arm-raspi|arm-unknown-aros|armhf|none"
    "aarch64|raspi|rpi-aarch64|aarch64-unknown-aros|aarch64|none"
    "riscv64|opensbi|opensbi-riscv64|riscv64-unknown-aros|riscv64|none")

foreach(_profile IN LISTS _profiles)
    string(REPLACE "|" ";" _fields "${_profile}")
    list(GET _fields 0 _cpu)
    list(GET _fields 1 _platform)
    list(GET _fields 2 _name)
    list(GET _fields 3 _triple)
    list(GET _fields 4 _builtins)
    list(GET _fields 5 _companion)
    set(_prefix "${_root}/prefix-${_cpu}")
    set(_build "${_root}/build-${_cpu}")
    file(MAKE_DIRECTORY
        "${_prefix}/bin"
        "${_prefix}/include/c++/v1"
        "${_prefix}/lib/clang/11.0.0/lib/aros")
    foreach(_tool IN LISTS _tools)
        file(WRITE "${_prefix}/bin/${_tool}" "fixture\n")
    endforeach()
    foreach(_runtime IN ITEMS
            algorithm cerrno cinttypes cstddef cstdint deque memory string
            system_error vector libc++.a libc++abi.a libunwind.a)
        if(NOT _runtime MATCHES "\\.a$")
            file(WRITE "${_prefix}/include/c++/v1/${_runtime}" "// fixture\n")
        else()
            file(WRITE "${_prefix}/lib/${_runtime}" "fixture\n")
        endif()
    endforeach()
    file(WRITE
        "${_prefix}/lib/clang/11.0.0/lib/aros/libclang_rt.builtins-${_builtins}.a"
        "fixture\n")
    if(NOT _companion STREQUAL "none")
        file(WRITE
            "${_prefix}/lib/clang/11.0.0/lib/aros/libclang_rt.builtins-${_companion}.a"
            "fixture\n")
    endif()
    file(WRITE "${_prefix}/toolchain-manifest.json"
        "{\n"
        "  \"schema\": 1,\n"
        "  \"release_id\": \"fixture-v1\",\n"
        "  \"host\": \"fixture-host\",\n"
        "  \"target_profile\": \"${_name}\",\n"
        "  \"target_triple\": \"${_triple}\",\n"
        "  \"tree_sha256\": \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\n"
        "  \"llvm_version\": \"11.0.0\"\n"
        "}\n")

    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
            "-DAROS_CROSS_TOOLCHAIN_ROOT=${_prefix}"
            "-DAROS_TARGET_CPU=${_cpu}"
            "-DAROS_TARGET_PLATFORM=${_platform}"
            "-DEXPECTED_PROFILE=${_name}"
            "-DEXPECTED_TRIPLE=${_triple}"
            "-DEXPECTED_BUILTINS=${_builtins}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "release-toolchain ${_name} fixture failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
endforeach()

# A configured build tree belongs to one immutable release identity. Even a
# prefix with the same target profile must not replace it silently.
file(READ "${_root}/prefix-x86_64/toolchain-manifest.json" _x86_manifest)
string(REPLACE "fixture-v1" "fixture-v2" _x86_manifest_v2 "${_x86_manifest}")
file(WRITE "${_root}/prefix-x86_64/toolchain-manifest.json" "${_x86_manifest_v2}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_root}/build-x86_64"
        "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
        "-DAROS_CROSS_TOOLCHAIN_ROOT=${_root}/prefix-x86_64"
        "-DAROS_TARGET_CPU=x86_64"
        "-DAROS_TARGET_PLATFORM=pc"
        "-DEXPECTED_PROFILE=pc-x86_64"
        "-DEXPECTED_TRIPLE=x86_64-unknown-aros"
    RESULT_VARIABLE _identity_result
    OUTPUT_VARIABLE _identity_stdout
    ERROR_VARIABLE _identity_stderr)
if(_identity_result EQUAL 0 OR
   NOT "${_identity_stdout}\n${_identity_stderr}" MATCHES
       "already belongs to a different AROS toolchain")
    message(FATAL_ERROR
        "release-toolchain identity changed inside an existing build tree\n"
        "${_identity_stdout}\n${_identity_stderr}")
endif()

# A release for one profile must never be silently accepted for another.
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_root}/mismatch" -G Ninja
        "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
        "-DAROS_CROSS_TOOLCHAIN_ROOT=${_root}/prefix-x86_64"
        "-DAROS_TARGET_CPU=arm"
        "-DAROS_TARGET_PLATFORM=raspi"
        "-DEXPECTED_PROFILE=arm-raspi"
        "-DEXPECTED_TRIPLE=arm-unknown-aros"
    RESULT_VARIABLE _mismatch_result
    OUTPUT_VARIABLE _mismatch_stdout
    ERROR_VARIABLE _mismatch_stderr)
if(_mismatch_result EQUAL 0 OR
   NOT "${_mismatch_stdout}\n${_mismatch_stderr}" MATCHES
       "manifest selects pc-x86_64/x86_64-unknown-aros")
    message(FATAL_ERROR
        "release-toolchain accepted the wrong profile\n"
        "${_mismatch_stdout}\n${_mismatch_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "release toolchain contract test passed")
