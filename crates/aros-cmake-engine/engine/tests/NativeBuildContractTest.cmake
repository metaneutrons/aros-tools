cmake_minimum_required(VERSION 3.22)

if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_CHILD)
    include("${AROS_NATIVE_BUILD_CONTRACT_TEST_MODULE}")
    set(AROS_SOURCE_DIR "${AROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE}")
    set(AROS_NATIVE_BUILD_CONTRACT
        "${AROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE}/native-build-v1.json")
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_RELATIVE_PATH)
        set(AROS_NATIVE_BUILD_CONTRACT
            "${AROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE}/${AROS_NATIVE_BUILD_CONTRACT_TEST_RELATIVE_PATH}")
    endif()
    set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${AROS_NATIVE_BUILD_CONTRACT_TEST_SHA256}")
    set(AROS_TARGET_PROFILE "esp32p4-d1001")
    set(AROS_TARGET_CPU "riscv")
    set(AROS_TARGET_TRIPLE "riscv-aros")
    set(AROS_TOOLCHAIN "gnu")
    set(GCC_CONFIG_FLOAT_ABI "ilp32f")
    set(AROS_ABI_FLAVOUR "standalone")
    set(AROS_ABI_PLATFORM_SMP OFF)
    set(AROS_ENABLE_MMU OFF)
    set(AROS_GNU_TARGET_COMPILE_OPTIONS
        "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc;-mabi=ilp32f;-mcmodel=medany;-mstrict-align")

    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_FLOAT_ABI)
        set(GCC_CONFIG_FLOAT_ABI "${AROS_NATIVE_BUILD_CONTRACT_TEST_FLOAT_ABI}")
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_TARGET_OPTIONS)
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "${AROS_NATIVE_BUILD_CONTRACT_TEST_TARGET_OPTIONS}")
    elseif(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_BAD_TARGET)
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc;-mabi=ilp32;-mcmodel=medany;-mstrict-align")
    endif()

    aros_validate_native_build_contract()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_EXPECT_INCLUDE_BINDING)
        string(JSON _include_binding GET
            "${AROS_NATIVE_BUILD_MAKE_INCLUDE_BINDINGS_JSON}" config/aros.cfg)
        if(NOT _include_binding STREQUAL AROS_NATIVE_BUILD_CONTRACT_TEST_EXPECT_INCLUDE_BINDING)
            message(FATAL_ERROR "Source Make include binding was not exported exactly")
        endif()
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_MAKE_DEFAULTS)
        string(JSON _mode GET "${AROS_NATIVE_BUILD_MAKE_VARIABLES_JSON}" FEATURE_MODE)
        string(JSON _style GET "${AROS_NATIVE_BUILD_MAKE_VARIABLES_JSON}" SDK_STYLE)
        if(NOT _mode STREQUAL "" OR NOT _style STREQUAL "native-v1")
            message(FATAL_ERROR "Source Make defaults were not exported as literal data")
        endif()
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_GEOMETRY AND
       NOT AROS_NATIVE_BUILD_MEDIA_GEOMETRY_CONTRACT STREQUAL AROS_NATIVE_BUILD_CONTRACT_TEST_GEOMETRY)
        message(FATAL_ERROR "Optional media geometry reference was not exported exactly")
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_VALIDATE_COMPILER)
        aros_validate_native_build_compiler_target()
    endif()
    if(DEFINED AROS_NATIVE_BUILD_CONTRACT_TEST_LOCK_IDENTITY)
        aros_lock_native_build_contract_identity()
    endif()
    return()
endif()

set(_native_contract_module "${CMAKE_CURRENT_LIST_DIR}/../NativeBuildContract.cmake")
set(_native_contract_test_script "${CMAKE_CURRENT_LIST_FILE}")
set(_native_contract_fixture "${CMAKE_CURRENT_LIST_DIR}/native-build-contract/source")
if(NOT EXISTS "${_native_contract_fixture}/native-build-v1.json" OR
   NOT EXISTS "${_native_contract_fixture}/native-contract-input.txt")
    message(FATAL_ERROR "Native build contract test fixture is incomplete")
endif()

string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _random_suffix)
if(DEFINED ENV{TMPDIR} AND IS_DIRECTORY "$ENV{TMPDIR}")
    file(REAL_PATH "$ENV{TMPDIR}" _temporary_root)
else()
    file(REAL_PATH "/tmp" _temporary_root)
endif()
set(_native_contract_work "${_temporary_root}/aros-native-contract-${_random_suffix}")
if(EXISTS "${_native_contract_work}")
    message(FATAL_ERROR "Refusing to reuse existing native contract test directory")
endif()
file(MAKE_DIRECTORY "${_native_contract_work}")

function(_native_contract_copy_case case_name out_source)
    set(_destination "${_native_contract_work}/${case_name}")
    file(MAKE_DIRECTORY "${_destination}")
    file(COPY "${_native_contract_fixture}/" DESTINATION "${_destination}")
    set(${out_source} "${_destination}" PARENT_SCOPE)
endfunction()

function(_native_contract_run_failure case_name source expected_message)
    set(_extra_arguments ${ARGN})
    file(SHA256 "${source}/native-build-v1.json" _contract_sha)
    set(_arguments
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_CHILD=ON"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_MODULE=${_native_contract_module}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE=${source}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_contract_sha}")
    list(APPEND _arguments ${_extra_arguments})
    execute_process(
        COMMAND "${CMAKE_COMMAND}" ${_arguments} -P "${_native_contract_test_script}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_output "${_stdout}${_stderr}")
    if(_result EQUAL 0 OR NOT _output MATCHES "${expected_message}")
        message(FATAL_ERROR
            "${case_name} was not rejected as expected (result ${_result}):\n${_output}")
    endif()
endfunction()

function(_native_contract_run_success case_name source)
    set(_extra_arguments ${ARGN})
    file(SHA256 "${source}/native-build-v1.json" _contract_sha)
    set(_arguments
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_CHILD=ON"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_MODULE=${_native_contract_module}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE=${source}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_contract_sha}")
    list(APPEND _arguments ${_extra_arguments})
    execute_process(
        COMMAND "${CMAKE_COMMAND}" ${_arguments} -P "${_native_contract_test_script}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${case_name} failed (result ${_result}):\n${_stdout}${_stderr}")
    endif()
endfunction()

function(_native_contract_prepare_binding_case case_name out_source)
    _native_contract_copy_case("${case_name}" _source)
    file(MAKE_DIRECTORY "${_source}/config" "${_source}/arch")
    file(WRITE "${_source}/config/aros.cfg" "legacy configuration\n")
    file(WRITE "${_source}/config/secondary.cfg" "secondary configuration\n")
    file(WRITE "${_source}/arch/native-config.mk" "NATIVE_PROJECTION := 1\n")
    file(WRITE "${_source}/arch/foreign.mk" "FOREIGN_PROJECTION := 1\n")
    file(SHA256 "${_source}/native-contract-input.txt" _base_sha)
    file(SHA256 "${_source}/config/aros.cfg" _original_sha)
    file(SHA256 "${_source}/config/secondary.cfg" _secondary_sha)
    file(SHA256 "${_source}/arch/native-config.mk" _replacement_sha)
    set(_inputs
        "[{\"path\":\"native-contract-input.txt\",\"sha256\":\"${_base_sha}\"},{\"path\":\"config/aros.cfg\",\"sha256\":\"${_original_sha}\"},{\"path\":\"config/secondary.cfg\",\"sha256\":\"${_secondary_sha}\"},{\"path\":\"arch/native-config.mk\",\"sha256\":\"${_replacement_sha}\"}]")
    # Keep the complete JSON array explicit so each test can remove either
    # binding endpoint from the measured inventory without deleting the file.
    file(READ "${_source}/native-build-v1.json" _json)
    string(JSON _json SET "${_json}" inputs "${_inputs}")
    string(JSON _json SET "${_json}" make_include_bindings
        "{\"config/aros.cfg\":\"arch/native-config.mk\"}")
    file(WRITE "${_source}/native-build-v1.json" "${_json}\n")
    set(${out_source} "${_source}" PARENT_SCOPE)
endfunction()

_native_contract_copy_case("positive" _positive_source)
file(SHA256 "${_positive_source}/native-build-v1.json" _positive_sha)
include("${_native_contract_module}")
set(AROS_SOURCE_DIR "${_positive_source}")
set(AROS_NATIVE_BUILD_CONTRACT "${_positive_source}/native-build-v1.json")
set(AROS_NATIVE_BUILD_CONTRACT_SHA256 "${_positive_sha}")
set(AROS_TARGET_PROFILE "esp32p4-d1001")
set(AROS_TARGET_CPU "riscv")
set(AROS_TARGET_TRIPLE "riscv-aros")
set(AROS_TOOLCHAIN "gnu")
set(GCC_CONFIG_FLOAT_ABI "ilp32f")
set(AROS_ABI_FLAVOUR "standalone")
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)
set(AROS_GNU_TARGET_COMPILE_OPTIONS
    "-march=rv32imafc_zicsr_zifencei_zaamo_zalrsc;-mabi=ilp32f;-mcmodel=medany;-mstrict-align")

aros_validate_native_build_contract()
aros_native_transpiler_arguments(_selected_arguments)
if(NOT _selected_arguments STREQUAL "--native-profile;esp32p4-d1001;--native-contract-sha256;${_positive_sha}")
    message(FATAL_ERROR "Native transpiler selection differs from the validated source identity")
endif()
if(NOT AROS_NATIVE_BUILD_MAKE_VARIABLES_JSON STREQUAL "{}")
    message(FATAL_ERROR "Absent source Make defaults must stay empty, not be inferred")
endif()
if(NOT AROS_NATIVE_BUILD_MAKE_INCLUDE_BINDINGS_JSON STREQUAL "{}")
    message(FATAL_ERROR "Absent source Make include bindings must stay empty, not be inferred")
endif()
if(NOT AROS_NATIVE_BUILD_CONTRACT_VALIDATED OR
   NOT AROS_NATIVE_BUILD_PROFILE STREQUAL "esp32p4-d1001" OR
   NOT AROS_NATIVE_BUILD_ABI_ISA STREQUAL "rv32imafc_zicsr_zifencei_zaamo_zalrsc" OR
   NOT AROS_NATIVE_BUILD_CORE_RESOURCES STREQUAL "kernel;task" OR
   NOT AROS_NATIVE_BUILD_PACKAGE_TARGET STREQUAL "kernel-package-esp32p4-riscv" OR
   NOT AROS_NATIVE_BUILD_MEDIA_PARTITION_TABLE STREQUAL "native-contract-input.txt")
    message(FATAL_ERROR "Validated contract facts were not exported to AROS_NATIVE_BUILD_*")
endif()
aros_validate_native_build_compiler_target()
if(NOT AROS_NATIVE_BUILD_COMPILER_TARGET_VALIDATED)
    message(FATAL_ERROR "GNU compiler target was not marked as validated")
endif()
aros_lock_native_build_contract_identity()
if(NOT AROS_NATIVE_BUILD_CONTRACT_IDENTITY_LOCKED OR
   NOT AROS_NATIVE_BUILD_CONTRACT_LOCKED_SHA256 STREQUAL _positive_sha)
    message(FATAL_ERROR "Explicit build-tree contract identity lock failed")
endif()

_native_contract_prepare_binding_case("make-include-binding" _binding_source)
_native_contract_run_success("safe source Make include binding" "${_binding_source}"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_EXPECT_INCLUDE_BINDING=arch/native-config.mk")

_native_contract_prepare_binding_case("missing-binding-original" _missing_original_source)
file(SHA256 "${_missing_original_source}/native-contract-input.txt" _binding_base_sha)
file(SHA256 "${_missing_original_source}/arch/native-config.mk" _binding_replacement_sha)
file(READ "${_missing_original_source}/native-build-v1.json" _missing_original_json)
string(JSON _missing_original_json SET "${_missing_original_json}" inputs
    "[{\"path\":\"native-contract-input.txt\",\"sha256\":\"${_binding_base_sha}\"},{\"path\":\"arch/native-config.mk\",\"sha256\":\"${_binding_replacement_sha}\"}]")
file(WRITE "${_missing_original_source}/native-build-v1.json" "${_missing_original_json}\n")
_native_contract_run_failure("uninventoried original include" "${_missing_original_source}"
    "make_include_bindings key")

_native_contract_prepare_binding_case("missing-binding-replacement" _missing_replacement_source)
file(SHA256 "${_missing_replacement_source}/native-contract-input.txt" _binding_base_sha)
file(SHA256 "${_missing_replacement_source}/config/aros.cfg" _binding_original_sha)
file(READ "${_missing_replacement_source}/native-build-v1.json" _missing_replacement_json)
string(JSON _missing_replacement_json SET "${_missing_replacement_json}" inputs
    "[{\"path\":\"native-contract-input.txt\",\"sha256\":\"${_binding_base_sha}\"},{\"path\":\"config/aros.cfg\",\"sha256\":\"${_binding_original_sha}\"}]")
file(WRITE "${_missing_replacement_source}/native-build-v1.json" "${_missing_replacement_json}\n")
_native_contract_run_failure("uninventoried Make projection" "${_missing_replacement_source}"
    "make_include_bindings replacement")

_native_contract_prepare_binding_case("escaping-binding" _escaping_binding_source)
file(READ "${_escaping_binding_source}/native-build-v1.json" _escaping_binding_json)
string(JSON _escaping_binding_json SET "${_escaping_binding_json}" make_include_bindings
    "{\"config/aros.cfg\":\"../outside.mk\"}")
file(WRITE "${_escaping_binding_source}/native-build-v1.json" "${_escaping_binding_json}\n")
_native_contract_run_failure("escaping Make include binding" "${_escaping_binding_source}"
    "parent-directory segments")

_native_contract_prepare_binding_case("glob-binding" _glob_binding_source)
file(READ "${_glob_binding_source}/native-build-v1.json" _glob_binding_json)
string(JSON _glob_binding_json SET "${_glob_binding_json}" make_include_bindings
    "{\"config/*.cfg\":\"arch/native-config.mk\"}")
file(WRITE "${_glob_binding_source}/native-build-v1.json" "${_glob_binding_json}\n")
_native_contract_run_failure("glob Make include path" "${_glob_binding_source}"
    "source-relative path")

_native_contract_prepare_binding_case("foreign-binding-replacement" _foreign_binding_source)
file(READ "${_foreign_binding_source}/native-build-v1.json" _foreign_binding_json)
string(JSON _foreign_binding_json SET "${_foreign_binding_json}" make_include_bindings
    "{\"config/aros.cfg\":\"arch/foreign.mk\"}")
file(WRITE "${_foreign_binding_source}/native-build-v1.json" "${_foreign_binding_json}\n")
_native_contract_run_failure("foreign Make include replacement" "${_foreign_binding_source}"
    "make_include_bindings replacement")

_native_contract_prepare_binding_case("non-make-replacement" _non_make_replacement_source)
file(READ "${_non_make_replacement_source}/native-build-v1.json" _non_make_replacement_json)
string(JSON _non_make_replacement_json SET "${_non_make_replacement_json}" make_include_bindings
    "{\"config/aros.cfg\":\"native-contract-input.txt\"}")
file(WRITE "${_non_make_replacement_source}/native-build-v1.json" "${_non_make_replacement_json}\n")
_native_contract_run_failure("non-Make include replacement" "${_non_make_replacement_source}"
    ".mk file")

_native_contract_prepare_binding_case("self-binding" _self_binding_source)
file(READ "${_self_binding_source}/native-build-v1.json" _self_binding_json)
string(JSON _self_binding_json SET "${_self_binding_json}" make_include_bindings
    "{\"arch/native-config.mk\":\"arch/native-config.mk\"}")
file(WRITE "${_self_binding_source}/native-build-v1.json" "${_self_binding_json}\n")
_native_contract_run_success("inventoried original Make include binding" "${_self_binding_source}")

_native_contract_prepare_binding_case("duplicate-binding-replacement" _duplicate_binding_source)
file(READ "${_duplicate_binding_source}/native-build-v1.json" _duplicate_binding_json)
string(JSON _duplicate_binding_json SET "${_duplicate_binding_json}" make_include_bindings
    "{\"config/aros.cfg\":\"arch/native-config.mk\",\"config/secondary.cfg\":\"arch/native-config.mk\"}")
file(WRITE "${_duplicate_binding_source}/native-build-v1.json" "${_duplicate_binding_json}\n")
_native_contract_run_failure("duplicate Make include replacement" "${_duplicate_binding_source}"
    "duplicate replacement")

# CMake's JSON library may reject or collapse repeated raw object keys before
# this validator can inspect them. Exercise rejection whenever it preserves
# the duplicate member or rejects the raw object outright.
set(_raw_duplicate_map "{\"config/aros.cfg\":\"arch/native-config.mk\",\"config/aros.cfg\":\"arch/native-config.mk\"}")
string(JSON _raw_duplicate_length ERROR_VARIABLE _raw_duplicate_error
    LENGTH "${_raw_duplicate_map}")
if(NOT _raw_duplicate_error STREQUAL "NOTFOUND" OR _raw_duplicate_length GREATER 1)
    _native_contract_prepare_binding_case("duplicate-binding-key" _duplicate_key_source)
    file(READ "${_duplicate_key_source}/native-build-v1.json" _duplicate_key_json)
    string(REGEX REPLACE
        "\"make_include_bindings\"[ \t\r\n]*:[ \t\r\n]*\\{[^}]*\\}"
        "\"make_include_bindings\":${_raw_duplicate_map}"
        _duplicate_key_json "${_duplicate_key_json}")
    file(WRITE "${_duplicate_key_source}/native-build-v1.json" "${_duplicate_key_json}\n")
    _native_contract_run_failure("duplicate Make include key" "${_duplicate_key_source}"
        "Native build contract")
endif()

_native_contract_prepare_binding_case("null-bindings" _null_bindings_source)
file(READ "${_null_bindings_source}/native-build-v1.json" _null_bindings_json)
string(JSON _null_bindings_json SET "${_null_bindings_json}" make_include_bindings "null")
file(WRITE "${_null_bindings_source}/native-build-v1.json" "${_null_bindings_json}\n")
_native_contract_run_failure("null Make include bindings" "${_null_bindings_source}"
    "must be an object")

_native_contract_prepare_binding_case("null-binding-value" _null_binding_value_source)
file(READ "${_null_binding_value_source}/native-build-v1.json" _null_binding_value_json)
string(JSON _null_binding_value_json SET "${_null_binding_value_json}" make_include_bindings
    "{\"config/aros.cfg\":null}")
file(WRITE "${_null_binding_value_source}/native-build-v1.json" "${_null_binding_value_json}\n")
_native_contract_run_failure("null Make include replacement" "${_null_binding_value_source}"
    "string replacement path")

_native_contract_prepare_binding_case("too-many-bindings" _many_bindings_source)
set(_many_bindings "{}")
foreach(_binding_index RANGE 0 16)
    string(JSON _many_bindings SET "${_many_bindings}"
        "config/include-${_binding_index}.cfg" "\"arch/projection-${_binding_index}.mk\"")
endforeach()
file(READ "${_many_bindings_source}/native-build-v1.json" _many_bindings_json)
string(JSON _many_bindings_json SET "${_many_bindings_json}" make_include_bindings "${_many_bindings}")
file(WRITE "${_many_bindings_source}/native-build-v1.json" "${_many_bindings_json}\n")
_native_contract_run_failure("too many Make include bindings" "${_many_bindings_source}"
    "make_include_bindings.*exceeds 16")

if(NOT WIN32)
    set(_symlink_binding_index 0)
    foreach(_mapped_path IN ITEMS config/aros.cfg arch/native-config.mk)
        math(EXPR _symlink_binding_index "${_symlink_binding_index} + 1")
        _native_contract_prepare_binding_case(
            "symlink-binding-${_symlink_binding_index}" _symlink_binding_source)
        set(_mapped_file "${_symlink_binding_source}/${_mapped_path}")
        set(_symlink_target "${_symlink_binding_source}/symlink-target.txt")
        file(RENAME "${_mapped_file}" "${_symlink_target}")
        file(CREATE_LINK "${_symlink_target}" "${_mapped_file}" SYMBOLIC)
        _native_contract_run_failure("symlinked Make include path" "${_symlink_binding_source}"
            "crosses a symlink")
    endforeach()
endif()

_native_contract_copy_case("make-defaults" _make_source)
file(READ "${_make_source}/native-build-v1.json" _make_json)
string(JSON _make_json SET "${_make_json}" make_variables "{\"FEATURE_MODE\":\"\",\"SDK_STYLE\":\"native-v1\"}")
file(WRITE "${_make_source}/native-build-v1.json" "${_make_json}\n")
file(SHA256 "${_make_source}/native-build-v1.json" _make_sha)
execute_process(COMMAND "${CMAKE_COMMAND}"
    -DAROS_NATIVE_BUILD_CONTRACT_TEST_CHILD=ON
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_MODULE=${_native_contract_module}"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE=${_make_source}"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_make_sha}"
    -DAROS_NATIVE_BUILD_CONTRACT_TEST_MAKE_DEFAULTS=ON
    -P "${_native_contract_test_script}"
    RESULT_VARIABLE _make_result OUTPUT_VARIABLE _make_stdout ERROR_VARIABLE _make_stderr)
if(NOT _make_result EQUAL 0)
    message(FATAL_ERROR "Source Make defaults failed: ${_make_stdout}${_make_stderr}")
endif()
foreach(_bad_map IN ITEMS "{\"CPU\":\"riscv\"}" "{\"FEATURE\":\"a;b\"}" "{\"FEATURE\":0}" "{\"lowercase\":\"0\"}")
    _native_contract_copy_case("bad-make-defaults" _bad_make_source)
    file(READ "${_bad_make_source}/native-build-v1.json" _bad_make_json)
    string(JSON _bad_make_json SET "${_bad_make_json}" make_variables "${_bad_map}")
    file(WRITE "${_bad_make_source}/native-build-v1.json" "${_bad_make_json}\n")
    _native_contract_run_failure("unsafe Make configuration" "${_bad_make_source}" "make_variables")
endforeach()

# A media adapter requires this reference. Old core-only declarations may omit
# it, but no default or un-inventoried path is invented at the engine boundary.
_native_contract_copy_case("geometry-reference" _geometry_source)
# A referenced geometry is a measured input, not just an inventoried filename.
# Native packaging now consumes its numeric capacity before publication.
file(WRITE "${_geometry_source}/native-contract-input.txt"
    "{\"package_limit_bytes\":4063232}\n")
file(SHA256 "${_geometry_source}/native-contract-input.txt" _geometry_input_sha)
file(READ "${_geometry_source}/native-build-v1.json" _geometry_json)
string(JSON _geometry_json SET "${_geometry_json}" inputs 0 sha256
    "\"${_geometry_input_sha}\"")
string(JSON _geometry_json SET "${_geometry_json}" media geometry_contract "\"native-contract-input.txt\"")
file(WRITE "${_geometry_source}/native-build-v1.json" "${_geometry_json}")
file(SHA256 "${_geometry_source}/native-build-v1.json" _geometry_sha)
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_CHILD=ON"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_MODULE=${_native_contract_module}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SOURCE=${_geometry_source}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_geometry_sha}"
        "-DAROS_NATIVE_BUILD_CONTRACT_TEST_GEOMETRY=native-contract-input.txt"
        -P "${_native_contract_test_script}"
    RESULT_VARIABLE _geometry_result OUTPUT_VARIABLE _geometry_stdout ERROR_VARIABLE _geometry_stderr
    TIMEOUT 20)
if(NOT _geometry_result EQUAL 0)
    message(FATAL_ERROR "Inventoried media geometry reference failed: ${_geometry_stdout}${_geometry_stderr}")
endif()
set(_geometry_case 0)
foreach(_bad_geometry IN ITEMS "null" "\"missing.json\"" "\"../escape.json\"")
    math(EXPR _geometry_case "${_geometry_case} + 1")
    _native_contract_copy_case("geometry-refusal-${_geometry_case}" _bad_geometry_source)
    file(READ "${_bad_geometry_source}/native-build-v1.json" _bad_geometry_json)
    string(JSON _bad_geometry_json SET "${_bad_geometry_json}" media geometry_contract "${_bad_geometry}")
    file(WRITE "${_bad_geometry_source}/native-build-v1.json" "${_bad_geometry_json}")
    _native_contract_run_failure("unsafe/missing media geometry reference" "${_bad_geometry_source}" "Native build contract")
endforeach()

foreach(_bad_policy IN ITEMS command unknown-algorithm overlap reversed outside-rv32 fraction)
    _native_contract_copy_case("residency-${_bad_policy}" _policy_source)
    file(READ "${_policy_source}/native-build-v1.json" _policy_json)
    if(_bad_policy STREQUAL "command")
        string(JSON _policy_json SET "${_policy_json}" core residency_policy command "\"evil\"")
        set(_expected "unknown field 'command'")
    elseif(_bad_policy STREQUAL "unknown-algorithm")
        string(JSON _policy_json SET "${_policy_json}" core residency_policy algorithm "\"run-script\"")
        set(_expected "unsupported native residency")
    elseif(_bad_policy STREQUAL "overlap")
        string(JSON _policy_json SET "${_policy_json}" core residency_policy sram_start "1073741824")
        set(_expected "ranges must be nonempty")
    elseif(_bad_policy STREQUAL "reversed")
        string(JSON _policy_json SET "${_policy_json}" core residency_policy flash_end "1073741824")
        set(_expected "ranges must be nonempty")
    elseif(_bad_policy STREQUAL "outside-rv32")
        string(JSON _policy_json SET "${_policy_json}" core residency_policy flash_end "4294967297")
        set(_expected "RV32 range bound")
    else()
        string(JSON _policy_json SET "${_policy_json}" core residency_policy flash_end "1140850688.5")
        set(_expected "RV32 range bound")
    endif()
    file(WRITE "${_policy_source}/native-build-v1.json" "${_policy_json}")
    _native_contract_run_failure("invalid residency ${_bad_policy}" "${_policy_source}" "${_expected}")
endforeach()

_native_contract_copy_case("altered-contract" _altered_source)
file(SHA256 "${_native_contract_fixture}/native-build-v1.json" _fixture_contract_sha)
file(APPEND "${_altered_source}/native-build-v1.json" " ")
_native_contract_run_failure("altered contract bytes" "${_altered_source}"
    "contract bytes do not match"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_fixture_contract_sha}")

_native_contract_copy_case("wrong-contract-hash" _wrong_hash_source)
_native_contract_run_failure("wrong supplied contract hash" "${_wrong_hash_source}"
    "contract bytes do not match"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=0000000000000000000000000000000000000000000000000000000000000000")

_native_contract_copy_case("altered-input" _altered_input_source)
file(APPEND "${_altered_input_source}/native-contract-input.txt" "changed\n")
_native_contract_run_failure("altered source input" "${_altered_input_source}"
    "SHA-256 differs from the declared")

_native_contract_copy_case("unsafe-input-path" _unsafe_path_source)
file(READ "${_unsafe_path_source}/native-build-v1.json" _unsafe_path_json)
string(JSON _unsafe_path_json SET "${_unsafe_path_json}" inputs 0 path "\"../escape.txt\"")
file(WRITE "${_unsafe_path_source}/native-build-v1.json" "${_unsafe_path_json}")
_native_contract_run_failure("unsafe source input path" "${_unsafe_path_source}"
    "parent-directory segments")

_native_contract_copy_case("abi-mismatch" _abi_mismatch_source)
_native_contract_run_failure("ABI selector mismatch" "${_abi_mismatch_source}"
    "does not match GCC_CONFIG_FLOAT_ABI"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_FLOAT_ABI=ilp32")

_native_contract_copy_case("unknown-key" _unknown_key_source)
file(READ "${_unknown_key_source}/native-build-v1.json" _unknown_key_json)
string(JSON _unknown_key_json SET "${_unknown_key_json}" unrecognized "true")
file(WRITE "${_unknown_key_source}/native-build-v1.json" "${_unknown_key_json}")
_native_contract_run_failure("unknown contract key" "${_unknown_key_source}"
    "contract bytes do not match"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_fixture_contract_sha}")

# Recompute the expected digest for this case so rejection demonstrates the
# closed key set rather than only the top-level content digest check.
file(SHA256 "${_unknown_key_source}/native-build-v1.json" _unknown_key_sha)
_native_contract_run_failure("unknown contract key with matching digest"
    "${_unknown_key_source}" "unknown field 'unrecognized'"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_SHA256=${_unknown_key_sha}")

_native_contract_copy_case("wrong-gnu-target" _wrong_gnu_target_source)
_native_contract_run_failure("GNU compiler target mismatch" "${_wrong_gnu_target_source}"
    "GNU -mabi option differs"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_VALIDATE_COMPILER=ON"
    "-DAROS_NATIVE_BUILD_CONTRACT_TEST_BAD_TARGET=ON")

_native_contract_copy_case("oversized-contract" _oversized_source)
string(REPEAT "x" 1048577 _oversized_content)
file(WRITE "${_oversized_source}/native-build-v1.json" "${_oversized_content}")
_native_contract_run_failure("oversized contract" "${_oversized_source}"
    "contract exceeds the 1048576-byte limit")

_native_contract_copy_case("optional-meta" _optional_source)
file(WRITE "${_optional_source}/mmakefile.src" "#MM- includes : includes-$(AROS_TARGET_ARCH)\n")
file(SHA256 "${_optional_source}/mmakefile.src" _optional_recipe_sha)
file(READ "${_optional_source}/native-build-v1.json" _optional_json)
string(JSON _optional_input_count LENGTH "${_optional_json}" inputs)
string(JSON _optional_json SET "${_optional_json}" inputs ${_optional_input_count}
    "{\"path\":\"mmakefile.src\",\"sha256\":\"${_optional_recipe_sha}\"}")
set(_optional_edges [=[[{"recipe":"mmakefile.src","target":"includes","dependency":"includes-${AROS_TARGET_PLATFORM}"}]]=])
string(JSON _optional_json SET "${_optional_json}" optional_meta_dependencies "${_optional_edges}")
file(WRITE "${_optional_source}/native-build-v1.json" "${_optional_json}")
_native_contract_run_success("source-owned optional selector edge" "${_optional_source}")
foreach(_bad_optional IN ITEMS literal unknown duplicate null unbound)
    set(_bad_optional_json "${_optional_json}")
    if(_bad_optional STREQUAL "literal")
        string(JSON _bad_optional_json SET "${_bad_optional_json}" optional_meta_dependencies 0 dependency "\"missing-literal\"")
        set(_expected "selector-derived target template")
    elseif(_bad_optional STREQUAL "unknown")
        string(JSON _bad_optional_json SET "${_bad_optional_json}" optional_meta_dependencies 0 dependency [["includes-${UNKNOWN}"]])
        set(_expected "selector-derived target template")
    elseif(_bad_optional STREQUAL "duplicate")
        string(JSON _edge GET "${_optional_json}" optional_meta_dependencies 0)
        string(JSON _bad_optional_json SET "${_bad_optional_json}" optional_meta_dependencies 1 "${_edge}")
        set(_expected "repeats an edge")
    elseif(_bad_optional STREQUAL "null")
        string(JSON _bad_optional_json SET "${_bad_optional_json}" optional_meta_dependencies "null")
        set(_expected "must be an array")
    else()
        string(JSON _bad_optional_json SET "${_bad_optional_json}" optional_meta_dependencies 0 recipe "\"other/mmakefile.src\"")
        set(_expected "inventoried MetaMake")
    endif()
    file(WRITE "${_optional_source}/native-build-v1.json" "${_bad_optional_json}")
    _native_contract_run_failure("optional MetaMake ${_bad_optional}" "${_optional_source}" "${_expected}")
endforeach()

set(_disabled_json "${_optional_json}")
string(JSON _disabled_json SET "${_disabled_json}" optional_meta_dependencies 0 dependency "\"disabled-owner\"")
string(JSON _disabled_json SET "${_disabled_json}" optional_meta_dependencies 0 absence "\"disabled-owner\"")
file(WRITE "${_optional_source}/native-build-v1.json" "${_disabled_json}")
_native_contract_run_success("explicit disabled-owner contract boundary" "${_optional_source}")
foreach(_invalid_absence IN ITEMS unknown null boolean unsafe)
    set(_bad_disabled_json "${_disabled_json}")
    if(_invalid_absence STREQUAL "unknown")
        string(JSON _bad_disabled_json SET "${_bad_disabled_json}" optional_meta_dependencies 0 absence "\"ignore-missing\"")
        set(_expected "unknown absence")
    elseif(_invalid_absence STREQUAL "null")
        string(JSON _bad_disabled_json SET "${_bad_disabled_json}" optional_meta_dependencies 0 absence "null")
        set(_expected "must be STRING")
    elseif(_invalid_absence STREQUAL "boolean")
        string(JSON _bad_disabled_json SET "${_bad_disabled_json}" optional_meta_dependencies 0 absence "false")
        set(_expected "must be STRING")
    else()
        string(JSON _bad_disabled_json SET "${_bad_disabled_json}" optional_meta_dependencies 0 dependency [["${AROS_TARGET_CPU}"]])
        set(_expected "must be a literal target")
    endif()
    file(WRITE "${_optional_source}/native-build-v1.json" "${_bad_disabled_json}")
    _native_contract_run_failure("disabled-owner ${_invalid_absence}" "${_optional_source}" "${_expected}")
endforeach()

_native_contract_copy_case("input-bound" _input_bound_source)
file(READ "${_input_bound_source}/native-build-v1.json" _input_bound_json)
string(JSON _initial_input_count LENGTH "${_input_bound_json}" inputs)
foreach(_index RANGE ${_initial_input_count} 128)
    set(_input_name "additional-${_index}.src")
    file(WRITE "${_input_bound_source}/${_input_name}" "Source input ${_index}\n")
    file(SHA256 "${_input_bound_source}/${_input_name}" _input_sha)
    string(JSON _input_bound_json SET "${_input_bound_json}" inputs ${_index}
        "{\"path\":\"${_input_name}\",\"sha256\":\"${_input_sha}\"}")
    if(_index EQUAL 127)
        file(WRITE "${_input_bound_source}/native-build-v1.json" "${_input_bound_json}")
        _native_contract_run_success("128 source inputs" "${_input_bound_source}")
    endif()
endforeach()
file(WRITE "${_input_bound_source}/native-build-v1.json" "${_input_bound_json}")
_native_contract_run_failure("129 source inputs" "${_input_bound_source}"
    "must contain between 1 and 128 entries")

file(REMOVE_RECURSE "${_native_contract_work}")
message(STATUS "Native build contract boundary passed positive and negative cases")
