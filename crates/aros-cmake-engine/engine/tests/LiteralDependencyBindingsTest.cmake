cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED ENGINE_DIR OR NOT EXISTS "${ENGINE_DIR}/LiteralObjects.cmake")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine")
endif()
if(NOT DEFINED TEST_BINARY_DIR OR "${TEST_BINARY_DIR}" STREQUAL "")
    message(FATAL_ERROR "TEST_BINARY_DIR must name a fresh test root")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _test_binary_dir)
find_program(_cmake NAMES cmake REQUIRED)
if(APPLE)
    find_program(_brew NAMES brew REQUIRED)
    execute_process(COMMAND "${_brew}" --prefix llvm
        RESULT_VARIABLE _llvm_result
        OUTPUT_VARIABLE _llvm_prefix
        ERROR_VARIABLE _llvm_error
        OUTPUT_STRIP_TRAILING_WHITESPACE
        TIMEOUT 30)
    if(NOT _llvm_result STREQUAL "0")
        message(FATAL_ERROR "Literal dependency binding tests require installed LLVM: ${_llvm_error}")
    endif()
    find_program(_clang NAMES clang HINTS "${_llvm_prefix}/bin"
        NO_DEFAULT_PATH REQUIRED)
    find_program(_clangxx NAMES clang++ HINTS "${_llvm_prefix}/bin"
        NO_DEFAULT_PATH REQUIRED)
else()
    find_program(_clang NAMES clang REQUIRED)
    find_program(_clangxx NAMES clang++ REQUIRED)
endif()
file(REAL_PATH "${_clang}" _clang)
file(REAL_PATH "${_clangxx}" _clangxx)

file(MAKE_DIRECTORY "${_test_binary_dir}")
file(REAL_PATH "${_test_binary_dir}" _test_binary_dir)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_test_binary_dir}/literal-dependency-bindings-${_suffix}")
while(EXISTS "${_test_root}" OR IS_SYMLINK "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_test_binary_dir}/literal-dependency-bindings-${_suffix}")
endwhile()
file(MAKE_DIRECTORY "${_test_root}/fixture")
file(REAL_PATH "${_test_root}" _test_root)
file(WRITE "${_test_root}/fixture/NativeToolchain.cmake" [=[
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
set(CMAKE_C_COMPILER "${BINDING_CLANG}" CACHE FILEPATH "" FORCE)
set(CMAKE_CXX_COMPILER "${BINDING_CLANGXX}" CACHE FILEPATH "" FORCE)
]=])

file(WRITE "${_test_root}/fixture/CMakeLists.txt" [=[
cmake_minimum_required(VERSION 3.22)
project(LiteralDependencyBindingsFixture LANGUAGES C CXX)
file(REAL_PATH "${CMAKE_C_COMPILER}" _real_c_compiler)
file(REAL_PATH "${CMAKE_CXX_COMPILER}" _real_cxx_compiler)
set(CMAKE_C_COMPILER "${_real_c_compiler}")
set(CMAKE_CXX_COMPILER "${_real_cxx_compiler}")
include("${ENGINE_DIR}/LiteralObjects.cmake")

set(AROS_SOURCE_DIR "${CMAKE_CURRENT_BINARY_DIR}/source")
set(AROS_BUILD_DIR "${CMAKE_CURRENT_BINARY_DIR}/declared-build")
file(MAKE_DIRECTORY
    "${AROS_SOURCE_DIR}/c" "${AROS_SOURCE_DIR}/cxx"
    "${AROS_BUILD_DIR}/gen/c")
file(WRITE "${AROS_SOURCE_DIR}/c/main.c"
    "#if defined(OBJECT_CONTEXT)\n#include \"compile-only.h\"\n#elif defined(DEPENDENCY_CONTEXT)\n#include \"dependency-only.h\"\n#else\n#error missing literal compiler context\n#endif\nint literal_binding(void) { return VALUE; }\n")
file(WRITE "${AROS_SOURCE_DIR}/c/other.c"
    "#include \"dependency-only.h\"\nint other_binding(void) { return VALUE; }\n")
file(WRITE "${AROS_SOURCE_DIR}/c/main.cpp"
    "#include \"dependency-only.h\"\nint cpp_binding() { return VALUE; }\n")
file(WRITE "${AROS_SOURCE_DIR}/c/compile-only.h" "#define VALUE 19\n")
file(WRITE "${AROS_SOURCE_DIR}/c/dependency-only.h" "#define VALUE 7\n")

set(_object "${AROS_BUILD_DIR}/gen/c/main.o")
set(_dependencies "${AROS_BUILD_DIR}/gen/c/main.d")
set(_other_object "${AROS_BUILD_DIR}/gen/c/other.o")

if(LITERAL_CASE STREQUAL "missing-producer")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "arbitrary-sidecar")
    file(WRITE "${_dependencies}" "${_object}: ${AROS_SOURCE_DIR}/c/dependency-only.h\n")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "outside-sidecar")
    aros_generate_literal_dependencies(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_dependencies}"
        OBJECT_OUTPUT "${_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/outside/main.d"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "assembly-sidecar")
    aros_compile_literal_object(NAME literal-assembly
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${AROS_BUILD_DIR}/gen/c/main.s"
        LANGUAGE C OUTPUT_MODE ASSEMBLY
        SOURCE_DEPENDENCY_OUTPUT "${_dependencies}")
elseif(LITERAL_CASE STREQUAL "dependencies-sidecar")
    aros_compile_literal_object(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_dependencies}"
        LANGUAGE C OUTPUT_MODE DEPENDENCIES
        DEPENDENCY_TARGET_OUTPUT "${_object}"
        SOURCE_DEPENDENCY_OUTPUT "${_dependencies}")
elseif(LITERAL_CASE STREQUAL "wrong-source")
    aros_generate_literal_dependencies(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/other.c"
        OUTPUT "${_dependencies}"
        OBJECT_OUTPUT "${_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "wrong-language")
    aros_generate_literal_dependencies(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_dependencies}"
        OBJECT_OUTPUT "${_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    set_property(TARGET literal-dependencies PROPERTY
        AROS_LITERAL_DEPENDENCY_LANGUAGE CXX)
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "wrong-object")
    aros_generate_literal_dependencies(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_dependencies}"
        OBJECT_OUTPUT "${_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    set_property(TARGET literal-dependencies PROPERTY
        AROS_LITERAL_DEPENDENCY_TARGET_OUTPUT "${_other_object}")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
elseif(LITERAL_CASE STREQUAL "duplicate-claim")
    foreach(_index RANGE 1 2)
        aros_generate_literal_dependencies(NAME "literal-dependencies-${_index}"
            SOURCE "${AROS_SOURCE_DIR}/c/main.c"
            OUTPUT "${_dependencies}"
            OBJECT_OUTPUT "${_object}"
            LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    endforeach()
else()
    aros_generate_literal_dependencies(NAME literal-dependencies
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_dependencies}"
        OBJECT_OUTPUT "${_object}"
        LANGUAGE C ARGUMENTS -DDEPENDENCY_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
    aros_compile_literal_object(NAME literal-object
        SOURCE "${AROS_SOURCE_DIR}/c/main.c"
        OUTPUT "${_object}"
        LANGUAGE C SOURCE_DEPENDENCY_OUTPUT "${_dependencies}"
        ARGUMENTS -DOBJECT_CONTEXT=1 -I "${AROS_SOURCE_DIR}/c")
endif()
]=])

function(_binding_configure case_name should_succeed expected_message)
    string(RANDOM LENGTH 8 ALPHABET 0123456789abcdef _case_suffix)
    set(_build "${_test_root}/${case_name}-${_case_suffix}")
    set(_configure_command
        "${_cmake}" -S "${_test_root}/fixture" -B "${_build}" -G Ninja
        "-DENGINE_DIR=${_engine_dir}"
        "-DLITERAL_CASE=${case_name}"
        "-DBINDING_CLANG=${_clang}"
        "-DBINDING_CLANGXX=${_clangxx}"
        "-DCMAKE_TOOLCHAIN_FILE=${_test_root}/fixture/NativeToolchain.cmake")
    execute_process(
        COMMAND ${_configure_command}
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
            message(FATAL_ERROR "${case_name}: expected '${expected_message}'\n${_log}")
        endif()
        message(STATUS "${case_name}: rejected (${expected_message})")
    endif()
    set(_LITERAL_TEST_BUILD "${_build}" PARENT_SCOPE)
endfunction()

function(_binding_build build_dir target label should_succeed expected_message)
    execute_process(
        COMMAND "${_cmake}" --build "${build_dir}" --target "${target}" --parallel 2
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 90)
    set(_log "${_stdout}\n${_stderr}")
    if(should_succeed)
        if(NOT _result STREQUAL "0")
            message(FATAL_ERROR "${label}: build failed (${_result})\n${_log}")
        endif()
    else()
        if(_result STREQUAL "0")
            message(FATAL_ERROR "${label}: build unexpectedly succeeded\n${_log}")
        endif()
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "${label}: expected '${expected_message}'\n${_log}")
        endif()
    endif()
    set(_LITERAL_BUILD_LOG "${_log}" PARENT_SCOPE)
endfunction()

_binding_configure(missing-producer FALSE "has no registered literal")
_binding_configure(arbitrary-sidecar FALSE "has no registered literal")
_binding_configure(outside-sidecar FALSE "must be the .d sibling")
_binding_configure(assembly-sidecar FALSE "only valid for OBJECT")
_binding_configure(dependencies-sidecar FALSE "only valid for OBJECT")
_binding_configure(wrong-source FALSE "registered source dependency producer")
_binding_configure(wrong-language FALSE "registered source dependency producer")
_binding_configure(wrong-object FALSE "registered source dependency producer")
_binding_configure(duplicate-claim FALSE "output already owned by literal-dependencies-1")

_binding_configure(valid TRUE "")
set(_build "${_LITERAL_TEST_BUILD}")
set(_declared_build "${_build}/declared-build")
set(_source_root "${_build}/source")
set(_object "${_declared_build}/gen/c/main.o")
set(_source_dependencies "${_declared_build}/gen/c/main.d")
set(_private_dependencies "${_object}.d")
string(SHA256 _dependency_tracker_key "${_source_dependencies}")
set(_dependency_tracker
    "${_build}/CMakeFiles/aros-literal-dependency-tracker-${_dependency_tracker_key}.d")

# The cold build must run the declared .d producer before the object command.
_binding_build("${_build}" literal-object "cold source dependency build" TRUE "")
string(FIND "${_LITERAL_BUILD_LOG}" "Generating literal dependencies main.d" _dependency_index)
string(FIND "${_LITERAL_BUILD_LOG}" "Compiling literal object main.o" _object_index)
if(_dependency_index LESS 0 OR _object_index LESS 0 OR
   NOT _dependency_index LESS _object_index)
    message(FATAL_ERROR
        "cold build did not generate the source dependency file before compiling the object\n${_LITERAL_BUILD_LOG}")
endif()
foreach(_path IN ITEMS "${_object}" "${_source_dependencies}" "${_private_dependencies}")
    if(NOT EXISTS "${_path}" OR IS_SYMLINK "${_path}")
        message(FATAL_ERROR "successful binding build omitted a regular output: ${_path}")
    endif()
endforeach()

file(READ "${_source_dependencies}" _source_dependency_contents)
file(READ "${_private_dependencies}" _private_dependency_contents)
file(READ "${_dependency_tracker}" _dependency_tracker_contents)
string(FIND "${_source_dependency_contents}" "${_object}:" _source_object_rule)
if(_source_object_rule LESS 0)
    message(FATAL_ERROR "source dependency file did not name the exact object target\n${_source_dependency_contents}")
endif()
string(FIND "${_private_dependency_contents}" "${_object}:" _first_object_rule)
if(_first_object_rule LESS 0)
    message(FATAL_ERROR "private dependency file did not name the exact object target\n${_private_dependency_contents}")
endif()
math(EXPR _after_first_rule "${_first_object_rule} + 1")
string(SUBSTRING "${_private_dependency_contents}" ${_after_first_rule} -1 _after_first_contents)
string(FIND "${_after_first_contents}" "${_object}:" _second_object_rule)
if(_second_object_rule LESS 0)
    message(FATAL_ERROR "combined dependency file omitted the source-declared object rule\n${_private_dependency_contents}")
endif()
if(NOT _source_dependency_contents MATCHES "dependency-only[.]h" OR
   _source_dependency_contents MATCHES "compile-only[.]h" OR
   NOT _private_dependency_contents MATCHES "compile-only[.]h" OR
   NOT _private_dependency_contents MATCHES "dependency-only[.]h")
    message(FATAL_ERROR
        "object and source dependency contexts were not preserved separately and together\n"
        "source .d:\n${_source_dependency_contents}\nprivate .o.d:\n${_private_dependency_contents}")
endif()
if(NOT EXISTS "${_dependency_tracker}" OR IS_SYMLINK "${_dependency_tracker}" OR
   NOT _dependency_tracker_contents MATCHES "${_source_dependencies}:" OR
   _dependency_tracker_contents MATCHES "${_object}:" OR
   NOT _dependency_tracker_contents MATCHES "dependency-only[.]h" OR
   _dependency_tracker_contents MATCHES "compile-only[.]h" OR
   _source_dependency_contents MATCHES "${_source_dependencies}:")
    message(FATAL_ERROR
        "dependency tracker and public dependency output did not keep their distinct targets\n"
        "public .d:\n${_source_dependency_contents}\ntracker:\n${_dependency_tracker_contents}")
endif()
file(SHA256 "${_object}" _initial_object_hash)

# The private tracker records the dependency compiler's own include graph.
# Changing a header to include a new header must refresh the public .d, and
# subsequent edits to the new header must rebuild both dependency output and
# the bound object. Removing the include must remove that edge again.
set(_dependency_only_header "${_source_root}/c/dependency-only.h")
set(_dependency_added_header "${_source_root}/c/dependency-added.h")
file(WRITE "${_dependency_only_header}"
    "#include \"dependency-added.h\"\n#define VALUE 7\n")
file(WRITE "${_dependency_added_header}" "#define ADDED_DEPENDENCY 1\n")
_binding_build("${_build}" literal-object "new dependency-only include" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Generating literal dependencies main[.]d" OR
   NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "adding a dependency-only include did not refresh both outputs\n${_LITERAL_BUILD_LOG}")
endif()
file(READ "${_source_dependencies}" _updated_source_dependency_contents)
file(READ "${_dependency_tracker}" _updated_dependency_tracker_contents)
if(NOT _updated_source_dependency_contents MATCHES "dependency-added[.]h" OR
   NOT _updated_dependency_tracker_contents MATCHES "dependency-added[.]h" OR
   _updated_dependency_tracker_contents MATCHES "${_object}:" OR
   NOT _updated_source_dependency_contents MATCHES "${_object}:" OR
   _updated_source_dependency_contents MATCHES "${_source_dependencies}:")
    message(FATAL_ERROR
        "dependency include update did not publish the public object rule and private tracker rule\n"
        "public .d:\n${_updated_source_dependency_contents}\n"
        "tracker:\n${_updated_dependency_tracker_contents}")
endif()

file(WRITE "${_dependency_added_header}" "#define ADDED_DEPENDENCY 2\n")
_binding_build("${_build}" literal-object "new dependency header rebuild" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Generating literal dependencies main[.]d" OR
   NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "editing the newly included dependency did not refresh both outputs\n${_LITERAL_BUILD_LOG}")
endif()

file(WRITE "${_dependency_only_header}" "#define VALUE 7\n")
_binding_build("${_build}" literal-object "removed dependency-only include" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Generating literal dependencies main[.]d" OR
   NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "removing a dependency-only include did not refresh both outputs\n${_LITERAL_BUILD_LOG}")
endif()
file(READ "${_source_dependencies}" _removed_source_dependency_contents)
file(READ "${_dependency_tracker}" _removed_dependency_tracker_contents)
if(_removed_source_dependency_contents MATCHES "dependency-added[.]h" OR
   _removed_dependency_tracker_contents MATCHES "dependency-added[.]h")
    message(FATAL_ERROR
        "removed dependency remained in the public .d or private tracker\n"
        "public .d:\n${_removed_source_dependency_contents}\n"
        "tracker:\n${_removed_dependency_tracker_contents}")
endif()
file(SHA256 "${_object}" _object_before_removed_dependency_edit)
file(SHA256 "${_source_dependencies}" _source_dependencies_before_removed_dependency_edit)
file(WRITE "${_dependency_added_header}" "#define ADDED_DEPENDENCY 3\n")
_binding_build("${_build}" literal-object "removed dependency edit" TRUE "")
if("${_LITERAL_BUILD_LOG}" MATCHES "Generating literal dependencies main[.]d" OR
   "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "editing a removed dependency still rebuilt an output\n${_LITERAL_BUILD_LOG}")
endif()
file(SHA256 "${_object}" _object_after_removed_dependency_edit)
file(SHA256 "${_source_dependencies}" _source_dependencies_after_removed_dependency_edit)
if(NOT _object_after_removed_dependency_edit STREQUAL _object_before_removed_dependency_edit OR
   NOT _source_dependencies_after_removed_dependency_edit STREQUAL
       _source_dependencies_before_removed_dependency_edit)
    message(FATAL_ERROR "editing a removed dependency changed a published output")
endif()

# Fail specifically on the second preprocessor call, after the public .d
# compiler call has succeeded. Neither published output may be replaced.
set(_tracker_failure_wrapper "${_build}/tracker-failure-compiler.sh")
set(_tracker_failure_marker "${_build}/tracker-failure-compiler.marker")
file(WRITE "${_tracker_failure_wrapper}"
    "#!/bin/sh\nif [ ! -f '${_tracker_failure_marker}' ]; then\n"
    "  : > '${_tracker_failure_marker}'\n"
    "  exec '${_clang}' \"$@\"\nfi\nexit 42\n")
file(CHMOD "${_tracker_failure_wrapper}" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
set(_dependency_command_file
    "${_build}/CMakeFiles/aros-literal-object-${_dependency_tracker_key}.cmake")
file(READ "${_dependency_command_file}" _tracker_failure_command_contents)
string(REPLACE "set(COMPILER [==[${_clang}]==])"
    "set(COMPILER [==[${_tracker_failure_wrapper}]==])"
    _tracker_failure_command_contents "${_tracker_failure_command_contents}")
file(SHA256 "${_clang}" _original_compiler_sha256)
file(SHA256 "${_tracker_failure_wrapper}" _wrapper_compiler_sha256)
string(REPLACE "set(COMPILER_SHA256 [==[${_original_compiler_sha256}]==])"
    "set(COMPILER_SHA256 [==[${_wrapper_compiler_sha256}]==])"
    _tracker_failure_command_contents "${_tracker_failure_command_contents}")
if(NOT _tracker_failure_command_contents MATCHES "${_tracker_failure_wrapper}")
    message(FATAL_ERROR "could not prepare the tracker compiler failure fixture")
endif()
set(_tracker_failure_command_file
    "${_build}/CMakeFiles/aros-literal-object-tracker-failure-${_dependency_tracker_key}.cmake")
file(WRITE "${_tracker_failure_command_file}" "${_tracker_failure_command_contents}")
file(SHA256 "${_tracker_failure_command_file}" _tracker_failure_command_sha256)
file(SHA256 "${_source_dependencies}" _source_dependencies_before_tracker_failure)
file(SHA256 "${_dependency_tracker}" _dependency_tracker_before_tracker_failure)
execute_process(
    COMMAND "${_cmake}"
        "-DCOMMAND_FILE=${_tracker_failure_command_file}"
        "-DCOMMAND_ROOT=${_build}"
        "-DCOMMAND_SHA256=${_tracker_failure_command_sha256}"
        -DVERIFY_ONLY=OFF
        -P "${_engine_dir}/RunLiteralObject.cmake"
    RESULT_VARIABLE _tracker_failure_result
    OUTPUT_VARIABLE _tracker_failure_stdout
    ERROR_VARIABLE _tracker_failure_stderr
    TIMEOUT 90)
set(_tracker_failure_log "${_tracker_failure_stdout}\n${_tracker_failure_stderr}")
if(_tracker_failure_result STREQUAL "0" OR
   NOT _tracker_failure_log MATCHES "Literal dependency tracker compiler failed" OR
   NOT EXISTS "${_tracker_failure_marker}")
    message(FATAL_ERROR
        "tracker compiler failure fixture returned ${_tracker_failure_result}\n${_tracker_failure_log}")
endif()
file(SHA256 "${_source_dependencies}" _source_dependencies_after_tracker_failure)
file(SHA256 "${_dependency_tracker}" _dependency_tracker_after_tracker_failure)
if(NOT _source_dependencies_after_tracker_failure STREQUAL
       _source_dependencies_before_tracker_failure OR
   NOT _dependency_tracker_after_tracker_failure STREQUAL
       _dependency_tracker_before_tracker_failure)
    message(FATAL_ERROR "tracker compiler failure changed a published .d or tracker")
endif()
file(GLOB _tracker_failure_temporaries
    "${_source_dependencies}.literal-*.tmp"
    "${_dependency_tracker}.literal-*.tmp")
if(_tracker_failure_temporaries)
    message(FATAL_ERROR
        "tracker compiler failure left temporary outputs: ${_tracker_failure_temporaries}")
endif()

file(WRITE "${_source_root}/c/compile-only.h" "#define VALUE 31\n")
_binding_build("${_build}" literal-object "object-only header rebuild" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "compile-only header edit did not rebuild the object\n${_LITERAL_BUILD_LOG}")
endif()
file(SHA256 "${_object}" _compile_header_object_hash)
if(_compile_header_object_hash STREQUAL _initial_object_hash)
    message(FATAL_ERROR "compile-only header edit did not change the object")
endif()

file(WRITE "${_source_root}/c/dependency-only.h" "#define VALUE 7\n")
_binding_build("${_build}" literal-object "source-dependency-only header rebuild" TRUE "")
if(NOT "${_LITERAL_BUILD_LOG}" MATCHES "Compiling literal object main[.]o")
    message(FATAL_ERROR
        "source-declared dependency header edit did not rebuild the object\n${_LITERAL_BUILD_LOG}")
endif()
file(SHA256 "${_object}" _dependency_header_object_hash)
if(NOT _dependency_header_object_hash STREQUAL _compile_header_object_hash)
    message(FATAL_ERROR
        "dependency-only header edit unexpectedly changed an object compiled with the other include context")
endif()

# The always-run preflight must refuse a sidecar symlink and leave published
# outputs untouched. The object build must fail for the same invalid input.
file(SHA256 "${_object}" _object_before_symlink)
file(SHA256 "${_private_dependencies}" _private_depfile_before_symlink)
set(_saved_source_dependencies "${_source_dependencies}.saved")
file(RENAME "${_source_dependencies}" "${_saved_source_dependencies}")
file(CREATE_LINK "${_saved_source_dependencies}" "${_source_dependencies}" SYMBOLIC)
_binding_build("${_build}" literal-object-check "source dependency symlink preflight" FALSE
    "symlink source dependency output")
_binding_build("${_build}" literal-object "source dependency symlink build" FALSE
    "symlink source dependency output")
file(SHA256 "${_object}" _object_after_symlink)
file(SHA256 "${_private_dependencies}" _private_depfile_after_symlink)
if(NOT _object_after_symlink STREQUAL _object_before_symlink OR
   NOT _private_depfile_after_symlink STREQUAL _private_depfile_before_symlink)
    message(FATAL_ERROR "symlink rejection changed the previously published object outputs")
endif()
file(REMOVE "${_source_dependencies}")
file(RENAME "${_saved_source_dependencies}" "${_source_dependencies}")

# A source-declared depfile is bounded before it is read or appended.
string(REPEAT "x" 4194305 _oversized_source_dependencies)
file(WRITE "${_source_dependencies}" "${_oversized_source_dependencies}")
file(SHA256 "${_object}" _object_before_oversize)
file(SHA256 "${_private_dependencies}" _private_depfile_before_oversize)
string(SHA256 _object_command_key "${_object}")
set(_object_command_file
    "${_build}/CMakeFiles/aros-literal-object-${_object_command_key}.cmake")
file(SHA256 "${_object_command_file}" _object_command_sha256)
execute_process(
    COMMAND "${_cmake}"
        "-DCOMMAND_FILE=${_object_command_file}"
        "-DCOMMAND_ROOT=${_build}"
        "-DCOMMAND_SHA256=${_object_command_sha256}"
        -DVERIFY_ONLY=OFF
        -P "${_engine_dir}/RunLiteralObject.cmake"
    RESULT_VARIABLE _oversized_result
    OUTPUT_VARIABLE _oversized_stdout
    ERROR_VARIABLE _oversized_stderr
    TIMEOUT 90)
set(_oversized_log "${_oversized_stdout}\n${_oversized_stderr}")
if(_oversized_result STREQUAL "0" OR
   NOT _oversized_log MATCHES "source dependency output exceeds 4 MiB")
    message(FATAL_ERROR
        "oversized source dependency rejection failed (${_oversized_result})\n${_oversized_log}")
endif()
file(SHA256 "${_object}" _object_after_oversize)
file(SHA256 "${_private_dependencies}" _private_depfile_after_oversize)
if(NOT _object_after_oversize STREQUAL _object_before_oversize OR
   NOT _private_depfile_after_oversize STREQUAL _private_depfile_before_oversize)
    message(FATAL_ERROR "oversized source dependency rejection changed the published object outputs")
endif()

# A compiler failure after both outputs exist leaves the last public .d and
# tracker intact. The private preflight also refuses a tracker symlink.
file(SHA256 "${_source_dependencies}" _source_dependencies_before_failure)
file(SHA256 "${_dependency_tracker}" _dependency_tracker_before_failure)
file(READ "${_source_root}/c/main.c" _source_before_failure)
file(APPEND "${_source_root}/c/main.c" "#error dependency compiler failure\n")
_binding_build("${_build}" literal-dependencies "dependency compiler failure" FALSE
    "Literal dependency compiler failed")
file(SHA256 "${_source_dependencies}" _source_dependencies_after_failure)
file(SHA256 "${_dependency_tracker}" _dependency_tracker_after_failure)
if(NOT _source_dependencies_after_failure STREQUAL _source_dependencies_before_failure OR
   NOT _dependency_tracker_after_failure STREQUAL _dependency_tracker_before_failure)
    message(FATAL_ERROR "failed dependency compiler changed a published .d or tracker")
endif()
file(GLOB _dependency_failure_temporaries
    "${_source_dependencies}.literal-*.tmp"
    "${_dependency_tracker}.literal-*.tmp")
if(_dependency_failure_temporaries)
    message(FATAL_ERROR
        "failed dependency compiler left temporary outputs: ${_dependency_failure_temporaries}")
endif()

set(_saved_dependency_tracker "${_dependency_tracker}.saved")
file(RENAME "${_dependency_tracker}" "${_saved_dependency_tracker}")
file(CREATE_LINK "${_saved_dependency_tracker}" "${_dependency_tracker}" SYMBOLIC)
file(SHA256 "${_source_dependencies}" _source_dependencies_before_tracker_symlink)
file(SHA256 "${_saved_dependency_tracker}" _saved_dependency_tracker_hash)
_binding_build("${_build}" literal-dependencies-check "dependency tracker symlink preflight" FALSE
    "symlink dependency tracker")
file(SHA256 "${_source_dependencies}" _source_dependencies_after_tracker_symlink)
file(SHA256 "${_saved_dependency_tracker}" _saved_dependency_tracker_after)
if(NOT _source_dependencies_after_tracker_symlink STREQUAL
       _source_dependencies_before_tracker_symlink OR
   NOT _saved_dependency_tracker_after STREQUAL _saved_dependency_tracker_hash)
    message(FATAL_ERROR "dependency tracker symlink rejection changed published outputs")
endif()
file(REMOVE "${_dependency_tracker}")
file(RENAME "${_saved_dependency_tracker}" "${_dependency_tracker}")

message(STATUS "literal source dependency bindings: passed")
