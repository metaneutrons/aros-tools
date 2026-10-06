cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/SdkObjects.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine directory")
endif()
if(NOT DEFINED TEST_BINARY_DIR OR "${TEST_BINARY_DIR}" STREQUAL "")
    message(FATAL_ERROR "TEST_BINARY_DIR must name a fresh test root")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
find_program(_ninja NAMES ninja REQUIRED)
find_program(_nm NAMES llvm-nm nm REQUIRED)

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/sdk-objects")
if(NOT EXISTS "${_fixture}/CMakeLists.txt")
    message(FATAL_ERROR "SDK object fixture is missing: ${_fixture}")
endif()
file(MAKE_DIRECTORY "${_test_binary_dir}")
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_test_binary_dir}/sdk-objects-${_suffix}")
while(EXISTS "${_test_root}" OR IS_SYMLINK "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_test_binary_dir}/sdk-objects-${_suffix}")
endwhile()
file(MAKE_DIRECTORY "${_test_root}")

function(_configure case_name should_succeed expected_message)
    set(_build "${_test_root}/${case_name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DENGINE_DIR=${_engine_dir}"
            "-DSDK_OBJECT_CASE=${case_name}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(should_succeed)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${case_name}: configure failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0")
            message(FATAL_ERROR "${case_name}: configure unexpectedly succeeded")
        endif()
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "${case_name}: expected '${expected_message}' in configure output\n${_log}")
        endif()
    endif()
    set(_SDK_OBJECT_BUILD "${_build}" PARENT_SCOPE)
    if(NOT should_succeed)
        message(STATUS "${case_name}: rejected with '${expected_message}'")
    endif()
endfunction()

function(_build_group build_dir label expect_success expected_message)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${build_dir}"
            --target sdk-objects --parallel 2
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(expect_success)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${label}: build failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0")
            message(FATAL_ERROR "${label}: build unexpectedly succeeded\n${_log}")
        endif()
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "${label}: expected '${expected_message}' in build output\n${_log}")
        endif()
    endif()
    set(_SDK_OBJECT_BUILD_LOG "${_log}" PARENT_SCOPE)
endfunction()

function(_assert_target_and_staged_bytes build_dir)
    foreach(_kind IN ITEMS plain xopen cpp)
        if(_kind STREQUAL "plain")
            set(_owner sdk-plain-choice-compile)
            set(_intermediate "${build_dir}/gen/sdk-objects/plain-choice.o")
            set(_output "${build_dir}/SYS/Developer/lib/plain-choice.o")
            set(_symbol sdk_plain_choice_symbol)
        elseif(_kind STREQUAL "xopen")
            set(_owner sdk-xopen-choice-compile)
            set(_intermediate "${build_dir}/gen/sdk-objects/xopen-choice.o")
            set(_output "${build_dir}/SYS/Developer/lib/xopen-choice.o")
            set(_symbol sdk_xopen_choice_symbol)
        else()
            set(_owner sdk-cpp-choice-compile)
            set(_intermediate "${build_dir}/gen/sdk-objects/cpp-choice.o")
            set(_output "${build_dir}/SYS/Developer/lib/cpp-choice.o")
            set(_symbol sdk_cpp_choice_symbol)
        endif()
        file(GLOB_RECURSE _target_objects LIST_DIRECTORIES FALSE
            "${build_dir}/CMakeFiles/${_owner}.dir/*.o")
        list(LENGTH _target_objects _object_count)
        if(NOT _object_count EQUAL 1)
            message(FATAL_ERROR "${_kind}: expected one private CMake object, found ${_object_count}: ${_target_objects}")
        endif()
        list(GET _target_objects 0 _target_object)
        foreach(_path IN ITEMS "${_target_object}" "${_intermediate}" "${_output}")
            if(NOT EXISTS "${_path}" OR IS_SYMLINK "${_path}")
                message(FATAL_ERROR "${_kind}: missing regular object ${_path}")
            endif()
        endforeach()
        file(SHA256 "${_target_object}" _target_hash)
        file(SHA256 "${_intermediate}" _intermediate_hash)
        file(SHA256 "${_output}" _output_hash)
        if(NOT _target_hash STREQUAL _intermediate_hash OR
           NOT _target_hash STREQUAL _output_hash)
            message(FATAL_ERROR
                "${_kind}: private target, generated intermediate, and staged bytes differ\n"
                "target=${_target_hash}\nintermediate=${_intermediate_hash}\noutput=${_output_hash}")
        endif()
        execute_process(
            COMMAND "${_nm}" "${_target_object}"
            RESULT_VARIABLE _nm_result
            OUTPUT_VARIABLE _nm_stdout
            ERROR_VARIABLE _nm_stderr)
        if(NOT _nm_result STREQUAL "0" OR
           NOT "${_nm_stdout}${_nm_stderr}" MATCHES "${_symbol}" OR
           "${_nm_stdout}${_nm_stderr}" MATCHES "sdk_stale_decoy_symbol")
            message(FATAL_ERROR
                "${_kind}: private object has the wrong symbol table\n${_nm_stdout}${_nm_stderr}")
        endif()
        set("_${_kind}_target_object" "${_target_object}" PARENT_SCOPE)
        set("_${_kind}_hash" "${_target_hash}" PARENT_SCOPE)
    endforeach()
endfunction()

# Configure failures exercise the helper's bounded declaration language.
_configure(duplicate-output FALSE "duplicate output owned by")
_configure(duplicate-owner FALSE "unsafe or duplicate owner")
_configure(duplicate-group-owner FALSE "duplicate owner")
_configure(outside-lib FALSE "output must be one Developer-library object")
_configure(symlink-build-root FALSE "symlink build root refused")
_configure(invalid-intermediate FALSE "intermediate must be the matching generated object")
_configure(invalid-language FALSE "language must be C or CXX")
_configure(language-mismatch FALSE "source extension disagrees with its declared language")
_configure(source-symlink FALSE "source must be a regular local source file")
_configure(missing-group-owner FALSE "object has no source-derived producer")
_configure(missing-program-role FALSE
    "Native program link contract lacks source-owned cxx-startup.o")
_configure(invalid-option FALSE "unsafe compile option")
_configure(plugin-option FALSE "unsafe compile option")
_configure(specs-option FALSE "unsafe compile option")
_configure(output-option FALSE "unsafe compile option")
_configure(invalid-define FALSE "unsafe preprocessor value")
_configure(optional-program-roles TRUE "")

# Compile all three explicit host projections: plain C, XOPEN C, and plain C++.
_configure(success TRUE "")
set(_build "${_SDK_OBJECT_BUILD}")
set(_stale_intermediate "${_build}/gen/sdk-objects/plain-choice.o")
file(SHA256 "${_stale_intermediate}" _stale_hash)
if(NOT EXISTS "${_build}/compile_commands.json")
    message(FATAL_ERROR "compile database missing; cannot verify source quote roles")
endif()
file(READ "${_build}/compile_commands.json" _compile_commands)
string(REGEX MATCHALL "-iquote" _quote_roles "${_compile_commands}")
list(LENGTH _quote_roles _quote_role_count)
if(NOT _quote_role_count EQUAL 3)
    message(FATAL_ERROR "expected one source-local -iquote role per object, found ${_quote_role_count}\n${_compile_commands}")
endif()
string(JSON _compile_count LENGTH "${_compile_commands}")
set(_seen_c_commands 0)
set(_seen_cxx_commands 0)
foreach(_index RANGE 0 ${_compile_count})
    if(_index GREATER_EQUAL _compile_count)
        break()
    endif()
    string(JSON _compile_file GET "${_compile_commands}" ${_index} file)
    string(JSON _compile_command GET "${_compile_commands}" ${_index} command)
    if(NOT _compile_command MATCHES "-fno-omit-frame-pointer")
        message(FATAL_ERROR "declaration-scoped compile option was omitted: ${_compile_command}")
    endif()
    if(_compile_file MATCHES "/source-root/(plain|xopen)/choice[.]c$")
        math(EXPR _seen_c_commands "${_seen_c_commands} + 1")
        if(NOT _compile_command MATCHES "-fno-common" OR
           _compile_command MATCHES "-fno-exceptions")
            message(FATAL_ERROR "C declaration received the wrong language flags: ${_compile_command}")
        endif()
    elseif(_compile_file MATCHES "/source-root/cpp/choice[.]cpp$")
        math(EXPR _seen_cxx_commands "${_seen_cxx_commands} + 1")
        if(NOT _compile_command MATCHES "-fno-exceptions" OR
           _compile_command MATCHES "-fno-common")
            message(FATAL_ERROR "C++ declaration received the wrong language flags: ${_compile_command}")
        endif()
    endif()
endforeach()
if(NOT _seen_c_commands EQUAL 2 OR NOT _seen_cxx_commands EQUAL 1)
    message(FATAL_ERROR "compile database did not expose two C and one C++ declaration")
endif()

_build_group("${_build}" initial TRUE "")
_assert_target_and_staged_bytes("${_build}")
if(_stale_hash STREQUAL _plain_hash)
    message(FATAL_ERROR "stale same-basename Make-style object survived SDK compilation")
endif()

# The always-run verifier makes an unchanged build perform safety checks, but
# it must not compile or republish any object.
set(_unchanged_paths
    "${_plain_target_object}"
    "${_build}/gen/sdk-objects/plain-choice.o"
    "${_build}/SYS/Developer/lib/plain-choice.o"
    "${_xopen_target_object}"
    "${_build}/gen/sdk-objects/xopen-choice.o"
    "${_build}/SYS/Developer/lib/xopen-choice.o"
    "${_cpp_target_object}"
    "${_build}/gen/sdk-objects/cpp-choice.o"
    "${_build}/SYS/Developer/lib/cpp-choice.o")
foreach(_path IN LISTS _unchanged_paths)
    string(SHA256 _path_key "${_path}")
    file(SHA256 "${_path}" "_before_hash_${_path_key}")
    file(TIMESTAMP "${_path}" "_before_time_${_path_key}" "%s" UTC)
endforeach()
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1)
_build_group("${_build}" no-op TRUE "")
if("${_SDK_OBJECT_BUILD_LOG}" MATCHES "Building C object|Building CXX object|Publishing source-declared SDK object")
    message(FATAL_ERROR "unchanged SDK object build compiled or republished\n${_SDK_OBJECT_BUILD_LOG}")
endif()
string(REGEX MATCHALL "VERIFY_ONLY=ON" _verify_steps "${_SDK_OBJECT_BUILD_LOG}")
list(LENGTH _verify_steps _verify_step_count)
if(NOT _verify_step_count EQUAL 3)
    message(FATAL_ERROR "unchanged build did not verify every owned object\n${_SDK_OBJECT_BUILD_LOG}")
endif()
foreach(_path IN LISTS _unchanged_paths)
    string(SHA256 _path_key "${_path}")
    file(SHA256 "${_path}" _after_hash)
    file(TIMESTAMP "${_path}" _after_time "%s" UTC)
    if(NOT _after_hash STREQUAL "${_before_hash_${_path_key}}" OR
       NOT _after_time STREQUAL "${_before_time_${_path_key}}")
        message(FATAL_ERROR "unchanged build modified ${_path}")
    endif()
endforeach()

# A missing staged output is repaired from the already compiled target object.
file(REMOVE "${_build}/SYS/Developer/lib/plain-choice.o")
_build_group("${_build}" output-repair TRUE "")
if("${_SDK_OBJECT_BUILD_LOG}" MATCHES "Building C object|Building CXX object")
    message(FATAL_ERROR "staging repair unexpectedly recompiled a source\n${_SDK_OBJECT_BUILD_LOG}")
endif()
_assert_target_and_staged_bytes("${_build}")

# Deleting a private C target object must compile that declaration again and
# publish those new object bytes to both owned paths.
file(REMOVE "${_plain_target_object}")
_build_group("${_build}" recompile TRUE "")
if(NOT "${_SDK_OBJECT_BUILD_LOG}" MATCHES "Building C object")
    message(FATAL_ERROR "missing private C object did not trigger recompilation\n${_SDK_OBJECT_BUILD_LOG}")
endif()
_assert_target_and_staged_bytes("${_build}")

# Native program-link roles must resolve to the registered owners and their
# exact Developer paths. The source objects are compiled and staged by Ninja.
_configure(program-role-bindings TRUE "")
set(_program_build "${_SDK_OBJECT_BUILD}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_program_build}"
        --target native-program-objects --parallel 3
    RESULT_VARIABLE _program_build_result
    OUTPUT_VARIABLE _program_build_stdout
    ERROR_VARIABLE _program_build_stderr
    TIMEOUT 90)
set(_program_build_log "${_program_build_stdout}\n${_program_build_stderr}")
if(NOT _program_build_result STREQUAL "0")
    message(FATAL_ERROR
        "native program source objects failed to build (${_program_build_result})\n${_program_build_log}")
endif()
if(NOT _program_build_log MATCHES "Building C object")
    message(FATAL_ERROR "native program role group did not compile host C objects\n${_program_build_log}")
endif()
set(_program_output_paths
    "${_program_build}/SYS/Developer/lib/startup.o"
    "${_program_build}/SYS/Developer/lib/detach.o"
    "${_program_build}/SYS/Developer/lib/cxx-startup.o")
set(_program_expected_outputs ${_program_output_paths})
file(GLOB _program_actual_outputs LIST_DIRECTORIES FALSE
    "${_program_build}/SYS/Developer/lib/*.o")
list(SORT _program_expected_outputs)
list(SORT _program_actual_outputs)
if(NOT "${_program_actual_outputs}" STREQUAL "${_program_expected_outputs}")
    message(FATAL_ERROR
        "native program role group staged guessed or missing outputs: ${_program_actual_outputs}")
endif()
foreach(_role IN ITEMS startup detach cxx-startup)
    if(_role STREQUAL "startup")
        set(_owner source-startup-owner)
        set(_symbol sdk_program_startup_symbol)
    elseif(_role STREQUAL "detach")
        set(_owner source-detach-owner)
        set(_symbol sdk_program_detach_symbol)
    else()
        set(_owner source-cxx-startup-owner)
        set(_symbol sdk_program_cxx_startup_symbol)
    endif()
    set(_intermediate "${_program_build}/gen/sdk-objects/${_role}.o")
    set(_output "${_program_build}/SYS/Developer/lib/${_role}.o")
    file(GLOB_RECURSE _role_objects LIST_DIRECTORIES FALSE
        "${_program_build}/CMakeFiles/${_owner}-compile.dir/*.o")
    list(LENGTH _role_objects _role_object_count)
    if(NOT _role_object_count EQUAL 1)
        message(FATAL_ERROR "${_role}: expected one private source-owned C object, found ${_role_objects}")
    endif()
    list(GET _role_objects 0 _role_object)
    foreach(_path IN ITEMS "${_role_object}" "${_intermediate}" "${_output}")
        if(NOT EXISTS "${_path}" OR IS_SYMLINK "${_path}")
            message(FATAL_ERROR "${_role}: missing regular source-owned object ${_path}")
        endif()
    endforeach()
    file(SHA256 "${_role_object}" _role_object_hash)
    file(SHA256 "${_intermediate}" _role_intermediate_hash)
    file(SHA256 "${_output}" _role_output_hash)
    if(NOT _role_object_hash STREQUAL _role_intermediate_hash OR
       NOT _role_object_hash STREQUAL _role_output_hash)
        message(FATAL_ERROR "${_role}: private, generated, and staged object bytes differ")
    endif()
    execute_process(
        COMMAND "${_nm}" "${_role_object}"
        RESULT_VARIABLE _role_nm_result
        OUTPUT_VARIABLE _role_nm_stdout
        ERROR_VARIABLE _role_nm_stderr)
    if(NOT _role_nm_result STREQUAL "0" OR
       NOT "${_role_nm_stdout}${_role_nm_stderr}" MATCHES "${_symbol}")
        message(FATAL_ERROR "${_role}: wrong source object symbol\n${_role_nm_stdout}${_role_nm_stderr}")
    endif()
endforeach()
message(STATUS "native program role owners, paths, private compile-owner list, and exact C object staging passed")

function(_runtime_symlink_probe case_name attack_kind)
    _configure("runtime-${attack_kind}-symlink" TRUE "")
    set(_runtime_build "${_SDK_OBJECT_BUILD}")
    _build_group("${_runtime_build}" "${case_name}-initial" TRUE "")
    _assert_target_and_staged_bytes("${_runtime_build}")
    set(_preserved_output "${_runtime_build}/SYS/Developer/lib/cpp-choice.o")
    file(SHA256 "${_preserved_output}" _preserved_hash)
    set(_outside "${_test_root}/${case_name}-outside")
    file(MAKE_DIRECTORY "${_outside}")
    set(_sentinel "${_test_root}/${case_name}-sentinel")
    file(WRITE "${_sentinel}" "preserve outside object sentinel\n")
    file(SHA256 "${_sentinel}" _sentinel_hash_before)

    if(attack_kind STREQUAL "output")
        set(_attacked "${_runtime_build}/SYS/Developer/lib/plain-choice.o")
        file(REMOVE "${_attacked}")
        file(CREATE_LINK "${_sentinel}" "${_attacked}" SYMBOLIC)
    elseif(attack_kind STREQUAL "intermediate")
        set(_attacked "${_runtime_build}/gen/sdk-objects/plain-choice.o")
        file(REMOVE "${_attacked}")
        file(CREATE_LINK "${_sentinel}" "${_attacked}" SYMBOLIC)
    else()
        set(_intermediate_parent "${_runtime_build}/gen/sdk-objects")
        file(RENAME "${_intermediate_parent}" "${_runtime_build}/gen/sdk-objects-real")
        file(CREATE_LINK "${_outside}" "${_intermediate_parent}" SYMBOLIC)
        set(_attacked "${_intermediate_parent}")
    endif()
    if(NOT IS_SYMLINK "${_attacked}")
        message(FATAL_ERROR "${case_name}: could not install the post-configure symlink")
    endif()

    _build_group("${_runtime_build}" "${case_name}" FALSE "SDK object staging: symlink path refused")
    if(NOT IS_SYMLINK "${_attacked}")
        message(FATAL_ERROR "${case_name}: failed staging removed or followed the attacker link")
    endif()
    file(SHA256 "${_sentinel}" _sentinel_hash_after)
    if(NOT _sentinel_hash_before STREQUAL _sentinel_hash_after)
        message(FATAL_ERROR "${case_name}: outside sentinel bytes changed")
    endif()
    file(SHA256 "${_preserved_output}" _preserved_hash_after)
    if(NOT _preserved_hash STREQUAL _preserved_hash_after)
        message(FATAL_ERROR "${case_name}: unrelated valid Developer object changed")
    endif()
    if(attack_kind STREQUAL "parent" AND EXISTS "${_outside}/plain-choice.o")
        message(FATAL_ERROR "${case_name}: object was written through a symlinked parent")
    endif()
    message(STATUS "${case_name}: build-time symlink refused; outside and valid output hashes retained")
endfunction()

_runtime_symlink_probe(post-configure-output output)
_runtime_symlink_probe(post-configure-intermediate intermediate)
_runtime_symlink_probe(post-configure-parent parent)

function(_runtime_input_symlink_probe case_name attack_kind expected_message)
    _configure("runtime-${attack_kind}-symlink" TRUE "")
    set(_runtime_build "${_SDK_OBJECT_BUILD}")
    _build_group("${_runtime_build}" "${case_name}-initial" TRUE "")
    _assert_target_and_staged_bytes("${_runtime_build}")

    set(_published_paths
        "${_runtime_build}/gen/sdk-objects/plain-choice.o"
        "${_runtime_build}/SYS/Developer/lib/plain-choice.o"
        "${_runtime_build}/gen/sdk-objects/xopen-choice.o"
        "${_runtime_build}/SYS/Developer/lib/xopen-choice.o"
        "${_runtime_build}/gen/sdk-objects/cpp-choice.o"
        "${_runtime_build}/SYS/Developer/lib/cpp-choice.o")
    set(_published_hashes)
    foreach(_path IN LISTS _published_paths)
        file(SHA256 "${_path}" _hash)
        list(APPEND _published_hashes "${_hash}")
    endforeach()

    set(_sentinel "${_test_root}/${case_name}-sentinel")
    file(WRITE "${_sentinel}" "preserve outside object sentinel\n")
    file(SHA256 "${_sentinel}" _sentinel_hash_before)
    if(attack_kind STREQUAL "source-file")
        set(_attacked "${_runtime_build}/source-root/plain/choice.c")
        set(_preserved "${_attacked}.preserved")
        file(RENAME "${_attacked}" "${_preserved}")
        file(CREATE_LINK "${_sentinel}" "${_attacked}" SYMBOLIC)
    elseif(attack_kind STREQUAL "source-parent")
        set(_attacked "${_runtime_build}/source-root/plain")
        set(_preserved "${_runtime_build}/source-root/plain-physical")
        file(RENAME "${_attacked}" "${_preserved}")
        file(CREATE_LINK "${_preserved}" "${_attacked}" SYMBOLIC)
    else()
        set(_attacked "${_plain_target_object}")
        set(_preserved "${_attacked}.preserved")
        file(SHA256 "${_attacked}" _compiled_object_hash_before)
        file(RENAME "${_attacked}" "${_preserved}")
        file(CREATE_LINK "${_sentinel}" "${_attacked}" SYMBOLIC)
    endif()
    if(NOT IS_SYMLINK "${_attacked}")
        message(FATAL_ERROR "${case_name}: could not install the post-configure symlink")
    endif()

    _build_group("${_runtime_build}" "${case_name}" FALSE "${expected_message}")
    if("${_SDK_OBJECT_BUILD_LOG}" MATCHES "Building C object|Building CXX object|Publishing source-declared SDK object")
        message(FATAL_ERROR "${case_name}: compiler or publisher ran after a failed preflight\n${_SDK_OBJECT_BUILD_LOG}")
    endif()
    if(NOT IS_SYMLINK "${_attacked}" OR NOT EXISTS "${_preserved}")
        message(FATAL_ERROR "${case_name}: failed preflight changed the attacked input path")
    endif()
    file(SHA256 "${_sentinel}" _sentinel_hash_after)
    if(NOT _sentinel_hash_before STREQUAL _sentinel_hash_after)
        message(FATAL_ERROR "${case_name}: outside sentinel bytes changed")
    endif()
    list(LENGTH _published_paths _published_count)
    math(EXPR _last_published "${_published_count} - 1")
    foreach(_index RANGE 0 ${_last_published})
        list(GET _published_paths ${_index} _path)
        list(GET _published_hashes ${_index} _before_hash)
        file(SHA256 "${_path}" _after_hash)
        if(NOT _before_hash STREQUAL _after_hash)
            message(FATAL_ERROR "${case_name}: valid staged bytes changed at ${_path}")
        endif()
    endforeach()
    if(attack_kind STREQUAL "compiled-object")
        file(SHA256 "${_preserved}" _compiled_object_hash_after)
        if(NOT _compiled_object_hash_before STREQUAL _compiled_object_hash_after)
            message(FATAL_ERROR "${case_name}: retained private object bytes changed")
        endif()
    endif()
    message(STATUS "${case_name}: preflight refused symlink before compilation/publication; valid staged hashes retained")
endfunction()

_runtime_input_symlink_probe(post-configure-source-file source-file
    "SDK object staging: symlink source path refused")
_runtime_input_symlink_probe(post-configure-source-parent source-parent
    "SDK object staging: symlink source path refused")
_runtime_input_symlink_probe(post-configure-compiled-object compiled-object
    "SDK object staging: symlink path refused")

function(_runtime_build_root_probe)
    _configure(runtime-root-symlink TRUE "")
    set(_runtime_build "${_SDK_OBJECT_BUILD}")
    set(_moved_build "${_runtime_build}-physical")
    file(SHA256 "${_runtime_build}/CMakeCache.txt" _cache_hash_before)
    file(RENAME "${_runtime_build}" "${_moved_build}")
    file(CREATE_LINK "${_moved_build}" "${_runtime_build}" SYMBOLIC)
    if(NOT IS_SYMLINK "${_runtime_build}")
        message(FATAL_ERROR "post-configure-build-root: could not install replacement symlink")
    endif()
    _build_group("${_runtime_build}" post-configure-build-root FALSE
        "SDK object staging: symlink build root refused")
    file(SHA256 "${_runtime_build}/CMakeCache.txt" _cache_hash_after)
    if(NOT _cache_hash_before STREQUAL _cache_hash_after)
        message(FATAL_ERROR "post-configure-build-root: build cache changed through the symlink")
    endif()
    if(EXISTS "${_runtime_build}/SYS/Developer/lib/plain-choice.o" OR
       EXISTS "${_moved_build}/CMakeFiles/sdk-plain-choice-compile.dir/source-root/plain/choice.c.o")
        message(FATAL_ERROR "post-configure-build-root: build wrote through the replacement symlink")
    endif()
    message(STATUS "post-configure-build-root: staging refused replacement symlink without changing the build tree")
endfunction()

_runtime_build_root_probe()

file(REMOVE_RECURSE "${_test_root}")
message(STATUS "SDK object standalone C/C++ compile, exact staging, repair, recompile, quote-role, and refusal tests passed")
