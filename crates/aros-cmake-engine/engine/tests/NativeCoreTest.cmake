cmake_minimum_required(VERSION 3.22)

get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/native-core")
find_program(_native_core_clang NAMES clang)
find_program(_native_core_lld NAMES ld.lld)
find_program(_native_core_ninja NAMES ninja)
file(CHMOD "${_fixture}/fake_collector.py"
    PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
if(NOT _native_core_clang OR NOT _native_core_lld)
    message(FATAL_ERROR
        "native-core test requires clang and ld.lld to create a synthetic ELF fixture")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(CMAKE_HOST_UNIX)
    set(_temp_base "/tmp")
elseif(DEFINED ENV{TEMP} AND NOT "$ENV{TEMP}" STREQUAL "")
    set(_temp_base "$ENV{TEMP}")
else()
    message(FATAL_ERROR "native-core test needs /tmp or TEMP")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-native-core-${_suffix}")
file(MAKE_DIRECTORY "${_root}")

if(_native_core_ninja)
    set(_generator "Ninja")
else()
    set(_generator "Unix Makefiles")
endif()

function(_native_core_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}"
                -G "${_generator}"
                "-DCMAKE_C_COMPILER=${_native_core_clang}"
                "-DCMAKE_C_FLAGS=-target x86_64-unknown-linux-gnu -ffreestanding -fno-stack-protector -fno-pic"
                -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY
                "-DAROS_TEST_ENGINE_DIR=${_engine}"
                "-DNATIVE_CORE_CASE=${name}"
                "-DNATIVE_CORE_LINKER=${_native_core_lld}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "native-core ${name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "native-core ${name} unexpectedly configured\n${_log}")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "native-core ${name} missed '${expected_message}'\n${_log}")
        endif()
    endif()
    set(NATIVE_CORE_BUILD "${_build}" PARENT_SCOPE)
endfunction()

_native_core_configure(success TRUE "")
set(_success_build "${NATIVE_CORE_BUILD}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "native-core default build failed (${_build_result})\n${_build_stdout}${_build_stderr}")
endif()
file(GLOB_RECURSE _default_group_objects
    "${_success_build}/CMakeFiles/module-*-kobj-*.dir/*.o"
    "${_success_build}/CMakeFiles/module-*-kobj-*.dir/*.obj")
if(_default_group_objects)
    message(FATAL_ERROR
        "native-core KOBJ source groups entered the default build: ${_default_group_objects}")
endif()
if(EXISTS "${_success_build}/runtime-modules/module-kernel" OR
   EXISTS "${_success_build}/runtime-modules/module-task")
    message(FATAL_ERROR "native-core default build linked a runtime executable")
endif()
set(_collector_log "${_success_build}/native-core-collector.jsonl")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env
            "NATIVE_CORE_COLLECTOR_LOG=${_collector_log}"
            "${CMAKE_COMMAND}" --build "${_success_build}"
            --target native-core-fixture
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "native-core explicit link failed (${_build_result})\n${_build_stdout}${_build_stderr}")
endif()
file(GLOB_RECURSE _explicit_group_objects
    "${_success_build}/CMakeFiles/module-*-kobj-*.dir/*.o"
    "${_success_build}/CMakeFiles/module-*-kobj-*.dir/*.obj")
if(NOT _explicit_group_objects)
    message(FATAL_ERROR "native-core explicit target did not compile its KOBJ source groups")
endif()
if(EXISTS "${_success_build}/runtime-modules/module-kernel" OR
   EXISTS "${_success_build}/runtime-modules/module-task")
    message(FATAL_ERROR "native-core explicit target linked a runtime executable")
endif()
set(_elf "${_success_build}/gen/native-core/core.elf")
set(_map "${_elf}.map")
if(NOT EXISTS "${_elf}" OR NOT EXISTS "${_map}")
    message(FATAL_ERROR "native-core synthetic link omitted ELF or map output")
endif()
file(READ "${_elf}" _elf_magic LIMIT 4 HEX)
string(TOLOWER "${_elf_magic}" _elf_magic)
if(NOT _elf_magic STREQUAL "7f454c46")
    message(FATAL_ERROR "native-core output is not an ELF file: ${_elf_magic}")
endif()
foreach(_owner IN ITEMS module-kernel module-task)
    set(_kobj "${_success_build}/gen/native-core/${_owner}.kobj")
    if(NOT EXISTS "${_kobj}")
        message(FATAL_ERROR "native-core did not publish registered KOBJ ${_owner}")
    endif()
    file(READ "${_kobj}" _kobj_magic LIMIT 4 HEX)
    string(TOLOWER "${_kobj_magic}" _kobj_magic)
    if(NOT _kobj_magic STREQUAL "7f454c46")
        message(FATAL_ERROR "native-core KOBJ is not a synthetic ELF file: ${_owner}")
    endif()
endforeach()
file(STRINGS "${_collector_log}" _collector_operations)
set(_synthetic_ld_count 0)
set(_synthetic_localize_count 0)
foreach(_operation IN LISTS _collector_operations)
    string(JSON _operation_kind ERROR_VARIABLE _operation_error
        GET "${_operation}" operation)
    if(_operation_error)
        message(FATAL_ERROR "native-core fixture collector log is invalid: ${_operation}")
    elseif(_operation_kind STREQUAL "--ld")
        string(JSON _partial_mode ERROR_VARIABLE _partial_mode_error
            GET "${_operation}" args 0)
        if(_partial_mode_error OR NOT _partial_mode STREQUAL "-r")
            message(FATAL_ERROR "native-core fixture collector did not use ld -r: ${_operation}")
        endif()
        math(EXPR _synthetic_ld_count "${_synthetic_ld_count} + 1")
    elseif(_operation_kind STREQUAL "--localize-kobj")
        math(EXPR _synthetic_localize_count "${_synthetic_localize_count} + 1")
    endif()
endforeach()
if(NOT _synthetic_ld_count EQUAL 2 OR NOT _synthetic_localize_count EQUAL 2)
    message(FATAL_ERROR
        "native-core fixture collector did not dispatch both synthetic KOBJs: ld=${_synthetic_ld_count}, copy-localize=${_synthetic_localize_count}")
endif()
file(READ "${_success_build}/native-core.properties" _properties)
foreach(_expected IN ITEMS
        "${_elf}" "${_map}" "module-kernel;module-task"
        "native-kobj-module-kernel;native-kobj-module-task"
        "${_success_build}/gen/native-core/module-kernel.kobj;${_success_build}/gen/native-core/module-task.kobj")
    string(FIND "${_properties}" "${_expected}" _found)
    if(_found LESS 0)
        message(FATAL_ERROR
            "native-core target properties omitted '${_expected}':\n${_properties}")
    endif()
endforeach()

_native_core_configure(missing-module FALSE "native core module target is missing")
_native_core_configure(foreign-module FALSE "native core member is foreign")
_native_core_configure(duplicate-module FALSE "native core modules are empty or duplicated")
_native_core_configure(nonarchive-provider FALSE "not a static")
_native_core_configure(no-sources FALSE "native core member has no sources")
_native_core_configure(missing-kobj FALSE "has no registered KOBJ")
_native_core_configure(wrong-owner-kobj FALSE "owner/form/registration mismatch")
_native_core_configure(unregistered-kobj FALSE "owner/form/registration mismatch")
_native_core_configure(missing-script FALSE "missing or unsafe native core LINKER_SCRIPT")
_native_core_configure(missing-builtins FALSE "missing or unsafe native core BUILTINS")
_native_core_configure(invalid-builtins FALSE "regular static archive magic")
_native_core_configure(thin-builtins FALSE "regular static archive magic")
_native_core_configure(missing-linker FALSE "missing or unsafe native core LINKER")
_native_core_configure(unsafe-output FALSE "output escapes the build tree")
_native_core_configure(symlink-output FALSE "escapes the physical build tree through a symlink")
_native_core_configure(duplicate-target FALSE "Native core target already exists")
_native_core_configure(resolve-missing-module FALSE "has no concrete module target")
_native_core_configure(resolve-foreign-module FALSE "only from foreign")
_native_core_configure(resolve-ambiguous-module FALSE "is ambiguous among targets")
_native_core_configure(resolve-ambiguous-archive FALSE "is ambiguous among targets")
_native_core_configure(resolve-foreign-archive FALSE "only from foreign")
_native_core_configure(resolve-private-archive TRUE "resolved=linklibs-core")
_native_core_configure(resolve-private-only-archive FALSE "only private target(s)")
_native_core_configure(resolve-noncanonical-archive TRUE "resolved=linklibs-core")
_native_core_configure(resolve-noncanonical-only-archive FALSE "no canonical static target")
_native_core_configure(resolve-private-module TRUE "resolved=module-kernel")
_native_core_configure(resolve-private-only-module FALSE "has no concrete module target")
_native_core_configure(nonexecutable-linker FALSE "LINKER --version failed or timed out")
_native_core_configure(linker-timeout FALSE "LINKER --version failed or timed out")
_native_core_configure(noncanonical-provider-direct FALSE "private or outside the")
_native_core_configure(private-archive-provider FALSE "private or outside the")
_native_core_configure(foreign-archive-provider FALSE "link provider is foreign")
_native_core_configure(source-bridge-missing-metadata FALSE "has no recorded source")
_native_core_configure(source-bridge-unresolved-kobj-flags FALSE "cannot consume unresolved")
_native_core_configure(source-bridge-function-instrumentation FALSE "not yet bound")

_native_core_configure(late-generated-dependency TRUE "")
set(_late_dependency_build "${NATIVE_CORE_BUILD}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_late_dependency_build}"
            --target native-core-fixture
    RESULT_VARIABLE _late_build_result
    OUTPUT_VARIABLE _late_build_stdout
    ERROR_VARIABLE _late_build_stderr)
if(NOT _late_build_result EQUAL 0)
    message(FATAL_ERROR
        "native-core late-dependency link failed (${_late_build_result})\n${_late_build_stdout}${_late_build_stderr}")
endif()
if(NOT EXISTS "${_late_dependency_build}/late-generated-dependency.marker")
    message(FATAL_ERROR
        "native-core did not build the dependency attached to its source OBJECT group before KOBJ creation")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS
    "native-core helper test passed (synthetic host ELF only; fixture collector uses ld -r and copy-only localization, with no real set/localization qualification and no P4 build evidence)")
