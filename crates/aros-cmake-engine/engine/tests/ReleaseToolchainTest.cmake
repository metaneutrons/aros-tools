cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

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

function(_expect_schema2_rejection name expected_message)
    set(_build "${_root}/schema2-${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
            "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
            "-DAROS_CROSS_TOOLCHAIN_ROOT=${_root}/prefix-x86_64"
            "-DAROS_TARGET_CPU=x86_64"
            "-DAROS_TARGET_PLATFORM=pc"
            "-DEXPECTED_PROFILE=pc-x86_64"
            "-DEXPECTED_TRIPLE=x86_64-unknown-aros"
            "-DEXPECTED_BUILTINS=x86_64"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(_result EQUAL 0 OR
       NOT "${_stdout}\n${_stderr}" MATCHES "${expected_message}")
        message(FATAL_ERROR
            "schema-2 ${name} counterprobe did not fail as expected\n"
            "${_stdout}\n${_stderr}")
    endif()
endfunction()

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
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
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
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
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
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
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

# Family-v2 LLVM manifests replace the legacy top-level llvm_version with a
# closed compiler identity. Keep the other common manifest fields populated so
# this fixture has the shape emitted for extracted release packages.
set(_schema2_manifest [=[
{
  "schema": 2,
  "release_id": "family-v2-fixture",
  "host": "fixture-host",
  "target_profile": "pc-x86_64",
  "target_triple": "x86_64-unknown-aros",
  "tree_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "compiler": {"family": "llvm", "version": "11.0.0"},
  "recipe_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
  "source_lock_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
  "profiles_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
  "source_commit": "1111111111111111111111111111111111111111",
  "producer_commit": "2222222222222222222222222222222222222222",
  "tools_commit": "3333333333333333333333333333333333333333",
  "source_date_epoch": 1700000000,
  "capabilities": ["clang", "llvm"],
  "build_environment": {"fixture": "synthetic"},
  "files": [{"path": "share", "mode": "0755", "type": "directory"}]
}
]=])
set(_schema2_prefix "${_root}/prefix-x86_64")
set(_schema2_build "${_root}/build-x86_64-v2")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_manifest}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_schema2_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
        "-DAROS_CROSS_TOOLCHAIN_ROOT=${_schema2_prefix}"
        "-DAROS_TARGET_CPU=x86_64"
        "-DAROS_TARGET_PLATFORM=pc"
        "-DEXPECTED_PROFILE=pc-x86_64"
        "-DEXPECTED_TRIPLE=x86_64-unknown-aros"
        "-DEXPECTED_BUILTINS=x86_64"
    RESULT_VARIABLE _schema2_result
    OUTPUT_VARIABLE _schema2_stdout
    ERROR_VARIABLE _schema2_stderr)
if(NOT _schema2_result EQUAL 0)
    message(FATAL_ERROR
        "valid schema-2 LLVM release-toolchain fixture failed (${_schema2_result})\n"
        "${_schema2_stdout}\n${_schema2_stderr}")
endif()

string(REPLACE "\"family\": \"llvm\"" "\"family\": \"gnu\""
    _schema2_wrong_family "${_schema2_manifest}")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_wrong_family}")
_expect_schema2_rejection("gnu-family" "non-LLVM compiler identity")

string(REPLACE "\"version\": \"11.0.0\"" "\"version\": \"11.bad.0\""
    _schema2_bad_version "${_schema2_manifest}")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_bad_version}")
_expect_schema2_rejection("malformed-version" "three numeric components")

foreach(_bounded_version IN ITEMS 2147483648.0.0 00000000000.0.0)
    string(REPLACE "\"version\": \"11.0.0\"" "\"version\": \"${_bounded_version}\""
        _schema2_unbounded_version "${_schema2_manifest}")
    file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_unbounded_version}")
    _expect_schema2_rejection("bounded-version-${_bounded_version}" "invalid component")
endforeach()

string(REPLACE "\"version\": \"11.0.0\"" "\"version\": \"12.0.0\""
    _schema2_wrong_version "${_schema2_manifest}")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_wrong_version}")
_expect_schema2_rejection("runtime-version-mismatch" "do not match LLVM version")

string(REPLACE "\"target_triple\": \"x86_64-unknown-aros\""
    "\"target_triple\": \"arm-unknown-aros\""
    _schema2_wrong_triple "${_schema2_manifest}")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_wrong_triple}")
_expect_schema2_rejection("target-triple" "manifest selects pc-x86_64/arm-unknown-aros")

# A changed release identity remains forbidden when the initial build used a
# family-v2 manifest too.
string(REPLACE "family-v2-fixture" "family-v2-replaced"
    _schema2_replaced_identity "${_schema2_manifest}")
file(WRITE "${_schema2_prefix}/toolchain-manifest.json" "${_schema2_replaced_identity}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_schema2_build}"
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
        "-DAROS_CROSS_TOOLCHAIN_ROOT=${_schema2_prefix}"
        "-DAROS_TARGET_CPU=x86_64"
        "-DAROS_TARGET_PLATFORM=pc"
        "-DEXPECTED_PROFILE=pc-x86_64"
        "-DEXPECTED_TRIPLE=x86_64-unknown-aros"
    RESULT_VARIABLE _schema2_identity_result
    OUTPUT_VARIABLE _schema2_identity_stdout
    ERROR_VARIABLE _schema2_identity_stderr)
if(_schema2_identity_result EQUAL 0 OR
   NOT "${_schema2_identity_stdout}\n${_schema2_identity_stderr}" MATCHES
       "already belongs to a different AROS toolchain")
    message(FATAL_ERROR
        "schema-2 release identity changed inside an existing build tree\n"
        "${_schema2_identity_stdout}\n${_schema2_identity_stderr}")
endif()

# Schema 1 intentionally predates the bounded compiler-version grammar. Its
# old numeric forms must still configure when the selected runtime matches.
set(_legacy_version "4294967296.0.0")
set(_legacy_prefix "${_root}/prefix-legacy-unbounded-version")
file(COPY "${_root}/prefix-x86_64/" DESTINATION "${_legacy_prefix}")
file(RENAME "${_legacy_prefix}/lib/clang/11.0.0"
    "${_legacy_prefix}/lib/clang/${_legacy_version}")
string(REPLACE "\"llvm_version\": \"11.0.0\""
    "\"llvm_version\": \"${_legacy_version}\"" _legacy_manifest "${_x86_manifest}")
file(WRITE "${_legacy_prefix}/toolchain-manifest.json" "${_legacy_manifest}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_root}/legacy-unbounded-version" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
        "-DCMAKE_TOOLCHAIN_FILE=${_toolchain}"
        "-DAROS_CROSS_TOOLCHAIN_ROOT=${_legacy_prefix}"
        "-DAROS_TARGET_CPU=x86_64"
        "-DAROS_TARGET_PLATFORM=pc"
        "-DEXPECTED_PROFILE=pc-x86_64"
        "-DEXPECTED_TRIPLE=x86_64-unknown-aros"
        "-DEXPECTED_BUILTINS=x86_64"
        "-DEXPECTED_LLVM_VERSION=${_legacy_version}"
    RESULT_VARIABLE _legacy_result
    OUTPUT_VARIABLE _legacy_stdout
    ERROR_VARIABLE _legacy_stderr)
if(NOT _legacy_result EQUAL 0)
    message(FATAL_ERROR
        "schema-1 legacy version fixture failed (${_legacy_result})\n"
        "${_legacy_stdout}\n${_legacy_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "release toolchain contract test passed")
