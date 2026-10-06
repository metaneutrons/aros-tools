cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED ENGINE_DIR OR "${ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "ENGINE_DIR must name the CMake engine directory")
endif()
cmake_path(ABSOLUTE_PATH ENGINE_DIR NORMALIZE OUTPUT_VARIABLE _engine_dir)
set(_directory_setup_module "${_engine_dir}/DirectorySetup.cmake")
if(NOT EXISTS "${_directory_setup_module}")
    message(FATAL_ERROR "DirectorySetup.cmake not found at ${_directory_setup_module}")
endif()

find_program(_ninja_executable NAMES ninja)
if(NOT _ninja_executable)
    message(FATAL_ERROR "Ninja is required for DirectorySetupTest.cmake")
endif()

if(DEFINED TEST_BINARY_DIR AND NOT "${TEST_BINARY_DIR}" STREQUAL "")
    cmake_path(ABSOLUTE_PATH TEST_BINARY_DIR
        BASE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
        NORMALIZE OUTPUT_VARIABLE _temp_base)
else()
    if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
        set(_temp_base "$ENV{TMPDIR}")
    else()
        set(_temp_base "/tmp")
    endif()
endif()
file(MAKE_DIRECTORY "${_temp_base}")

string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_test_root "${_temp_base}/directory setup ${_suffix}")
while(EXISTS "${_test_root}")
    string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
    set(_test_root "${_temp_base}/directory setup ${_suffix}")
endwhile()
file(MAKE_DIRECTORY "${_test_root}")

function(_bracket_quote value output_variable)
    set(${output_variable} "[==[${value}]==]" PARENT_SCOPE)
endfunction()

function(_configure_case case_name expected_failure expected_diagnostic sdk_root)
    set(_case_root "${_test_root}/${case_name}")
    set(_source_dir "${_case_root}/source tree")
    set(_build_dir "${_case_root}/build tree")
    file(MAKE_DIRECTORY "${_source_dir}")

    set(_project "cmake_minimum_required(VERSION 3.22)\n")
    string(APPEND _project "project(directory_setup_fixture NONE)\n")
    foreach(_root_variable IN ITEMS
            AROS_BUILD_DIR AROS_GENINC_DIR AROS_DEVELOPER_INCLUDE_DIR)
        _bracket_quote("${_case_root}/${_root_variable} root" _quoted_root)
        string(APPEND _project "set(${_root_variable} ${_quoted_root})\n")
    endforeach()
    if(DEFINED _test_developer_lib_root)
        set(_developer_lib_root "${_test_developer_lib_root}")
    else()
        set(_developer_lib_root "${_case_root}/AROS_DEVELOPER_LIB_DIR root")
    endif()
    _bracket_quote("${_developer_lib_root}" _quoted_developer_lib_root)
    string(APPEND _project
        "set(AROS_DEVELOPER_LIB_DIR ${_quoted_developer_lib_root})\n")
    _bracket_quote("${sdk_root}" _quoted_sdk_root)
    string(APPEND _project "set(AROS_SDK_INCLUDE_DIR ${_quoted_sdk_root})\n")
    _bracket_quote("${_directory_setup_module}" _quoted_module)
    string(APPEND _project "include(${_quoted_module})\n")
    string(APPEND _project
        "aros_prepare_directories(NAME prepare_directories DIRECTORIES\n")
    set(_directories ${ARGN})
    if(NOT _directories)
        message(FATAL_ERROR "${case_name}: test fixture requires a directory")
    endif()
    foreach(_directory IN LISTS _directories)
        _bracket_quote("${_directory}" _quoted_directory)
        string(APPEND _project "    ${_quoted_directory}\n")
    endforeach()
    string(APPEND _project ")\n")
    file(WRITE "${_source_dir}/CMakeLists.txt" "${_project}")

    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source_dir}" -B "${_build_dir}" -G Ninja
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)
    set(_configure_output "${_configure_stdout}\n${_configure_stderr}")
    string(REGEX REPLACE "[\r\n\t ]+" " " _normalized_configure_output
        "${_configure_output}")
    if(expected_failure)
        if("${_configure_result}" STREQUAL "0")
            message(FATAL_ERROR
                "${case_name}: configure unexpectedly succeeded\n${_configure_output}")
        endif()
        string(FIND "${_normalized_configure_output}" "${expected_diagnostic}"
            _diagnostic_index)
        if(_diagnostic_index EQUAL -1)
            message(FATAL_ERROR
                "${case_name}: configure failed without expected diagnostic "
                "'${expected_diagnostic}' (result ${_configure_result})\n"
                "${_configure_output}")
        endif()
        message(STATUS
            "${case_name}: configure rejected input with diagnostic "
            "'${expected_diagnostic}' (result ${_configure_result})")
    else()
        if(NOT "${_configure_result}" STREQUAL "0")
            message(FATAL_ERROR
                "${case_name}: configure failed (${_configure_result})\n"
                "${_configure_output}")
        endif()
        message(STATUS "${case_name}: Ninja configure succeeded")
    endif()

    set(_case_build_dir "${_build_dir}" PARENT_SCOPE)
endfunction()

function(_build_case case_name build_dir)
    execute_process(
        COMMAND "${_ninja_executable}" -C "${build_dir}" prepare_directories
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT "${_build_result}" STREQUAL "0")
        message(FATAL_ERROR
            "${case_name}: Ninja build failed (${_build_result})\n"
            "${_build_stdout}\n${_build_stderr}")
    endif()
    message(STATUS "${case_name}: Ninja build succeeded")
endfunction()

# A configured SDK root and a nested target may both be absent at configure
# time. The phony target must create them only when Ninja runs.
set(_sdk_root "${_test_root}/nonexistent sdk root")
set(_nested_directory "${_sdk_root}/nested include tree")
set(_developer_lib_root "${_test_root}/nonexistent Developer lib root")
set(_nested_library_directory "${_developer_lib_root}/nested library tree")
set(_test_developer_lib_root "${_developer_lib_root}")
_configure_case(nonexistent_root_and_nested FALSE "" "${_sdk_root}"
    "${_sdk_root}" "${_nested_directory}" "${_developer_lib_root}"
    "${_nested_library_directory}")
unset(_test_developer_lib_root)
if(EXISTS "${_sdk_root}" OR EXISTS "${_nested_directory}" OR
   EXISTS "${_developer_lib_root}" OR EXISTS "${_nested_library_directory}")
    message(FATAL_ERROR
        "nonexistent_root_and_nested: configure created an SDK directory")
endif()
_build_case(nonexistent_root_and_nested "${_case_build_dir}")
if(NOT IS_DIRECTORY "${_sdk_root}" OR NOT IS_DIRECTORY "${_nested_directory}" OR
   NOT IS_DIRECTORY "${_developer_lib_root}" OR
   NOT IS_DIRECTORY "${_nested_library_directory}")
    message(FATAL_ERROR
        "nonexistent_root_and_nested: Ninja did not create both SDK directories")
endif()

# An existing ancestor alias is valid. The configured SDK root below the
# symlink does not exist yet, so physical resolution must retain its suffix.
set(_alias_case_root "${_test_root}/ancestor alias case")
set(_alias_target "${_alias_case_root}/physical target")
set(_alias_path "${_alias_case_root}/existing path alias")
file(MAKE_DIRECTORY "${_alias_target}")
file(CREATE_LINK "${_alias_target}" "${_alias_path}" SYMBOLIC)
set(_alias_sdk_root "${_alias_path}/SDK include root")
set(_alias_nested "${_alias_sdk_root}/nested headers")
_configure_case(existing_ancestor_alias FALSE "" "${_alias_sdk_root}"
    "${_alias_sdk_root}" "${_alias_nested}")
if(EXISTS "${_alias_sdk_root}" OR EXISTS "${_alias_nested}")
    message(FATAL_ERROR
        "existing_ancestor_alias: configure created an SDK directory")
endif()
_build_case(existing_ancestor_alias "${_case_build_dir}")
if(NOT IS_DIRECTORY "${_alias_target}/SDK include root" OR
   NOT IS_DIRECTORY "${_alias_target}/SDK include root/nested headers")
    message(FATAL_ERROR
        "existing_ancestor_alias: Ninja did not create directories through the alias")
endif()

# A symlink that is itself the configured root is prohibited.
set(_root_symlink_case "${_test_root}/configured root symlink case")
set(_root_symlink_target "${_root_symlink_case}/real SDK root")
set(_root_symlink "${_root_symlink_case}/SDK root alias")
file(MAKE_DIRECTORY "${_root_symlink_target}")
file(CREATE_LINK "${_root_symlink_target}" "${_root_symlink}" SYMBOLIC)
_configure_case(configured_root_symlink TRUE "configured root is a symlink"
    "${_root_symlink}" "${_root_symlink}/include tree")

# The Developer library root has the same configured-root symlink boundary.
set(_developer_root_symlink_case "${_test_root}/configured Developer lib symlink case")
set(_developer_root_symlink_target "${_developer_root_symlink_case}/real lib root")
set(_developer_root_symlink "${_developer_root_symlink_case}/lib root alias")
file(MAKE_DIRECTORY "${_developer_root_symlink_target}")
file(CREATE_LINK "${_developer_root_symlink_target}"
    "${_developer_root_symlink}" SYMBOLIC)
set(_test_developer_lib_root "${_developer_root_symlink}")
_configure_case(configured_developer_lib_root_symlink TRUE
    "configured root is a symlink" "${_test_root}/safe SDK root"
    "${_developer_root_symlink}/archive tree")
unset(_test_developer_lib_root)

# A symlink below the root is rejected even when it points to another location
# inside the configured root.
set(_inside_symlink_case "${_test_root}/inside symlink case")
set(_inside_sdk_root "${_inside_symlink_case}/SDK root")
set(_inside_real_directory "${_inside_sdk_root}/real include tree")
set(_inside_symlink "${_inside_sdk_root}/redirected include tree")
file(MAKE_DIRECTORY "${_inside_real_directory}")
file(CREATE_LINK "${_inside_real_directory}" "${_inside_symlink}" SYMBOLIC)
_configure_case(existing_symlink_below_root TRUE "directory path crosses a symlink"
    "${_inside_sdk_root}" "${_inside_symlink}/headers")

# Dangling links below the root must also be recognized as links rather than
# treated as missing path components.
set(_dangling_case "${_test_root}/dangling symlink case")
set(_dangling_sdk_root "${_dangling_case}/SDK root")
set(_dangling_symlink "${_dangling_sdk_root}/dangling include alias")
file(MAKE_DIRECTORY "${_dangling_sdk_root}")
file(CREATE_LINK "${_dangling_case}/target does not exist"
    "${_dangling_symlink}" SYMBOLIC)
_configure_case(dangling_symlink_below_root TRUE "directory path crosses a symlink"
    "${_dangling_sdk_root}" "${_dangling_symlink}/headers")

# A lexically outside path is rejected before physical path resolution.
set(_outside_case "${_test_root}/outside path case")
set(_outside_sdk_root "${_outside_case}/SDK root")
set(_outside_directory "${_outside_case}/outside include tree")
_configure_case(outside_configured_roots TRUE "escapes configured roots"
    "${_outside_sdk_root}" "${_outside_directory}")

# A regular file encountered as the nearest existing ancestor is not a valid
# directory anchor for a missing suffix.
set(_file_ancestor_case "${_test_root}/file ancestor case")
set(_file_ancestor_sdk_root "${_file_ancestor_case}/SDK root")
file(MAKE_DIRECTORY "${_file_ancestor_sdk_root}")
file(WRITE "${_file_ancestor_sdk_root}/ordinary file" "fixture file\n")
_configure_case(file_as_ancestor TRUE "ancestor is not a directory"
    "${_file_ancestor_sdk_root}"
    "${_file_ancestor_sdk_root}/ordinary file/include tree")

set(_library_file_ancestor_case "${_test_root}/developer_lib_file_as_ancestor")
set(_library_file_root "${_library_file_ancestor_case}/AROS_DEVELOPER_LIB_DIR root")
file(MAKE_DIRECTORY "${_library_file_root}")
file(WRITE "${_library_file_root}/ordinary file" "fixture file\n")
_configure_case(developer_lib_file_as_ancestor TRUE "ancestor is not a directory"
    "${_test_root}/safe SDK root"
    "${_library_file_root}/ordinary file/archive tree")

# A validated graph must refuse a directory redirected after configuration,
# before the phony mkdir action can write outside the configured tree.
set(_runtime_case "${_test_root}/runtime redirect case")
set(_runtime_sdk "${_runtime_case}/SDK root")
set(_runtime_lib "${_runtime_case}/Developer lib root")
set(_runtime_nested "${_runtime_lib}/nested libraries")
set(_runtime_outside "${_runtime_case}/outside tree")
file(MAKE_DIRECTORY "${_runtime_sdk}" "${_runtime_lib}" "${_runtime_outside}")
set(_test_developer_lib_root "${_runtime_lib}")
_configure_case(post_configure_symlink FALSE "" "${_runtime_sdk}"
    "${_runtime_nested}/child")
unset(_test_developer_lib_root)
file(CREATE_LINK "${_runtime_outside}" "${_runtime_nested}" SYMBOLIC)
execute_process(COMMAND "${_ninja_executable}" -C "${_case_build_dir}" prepare_directories
    RESULT_VARIABLE _runtime_result OUTPUT_VARIABLE _runtime_stdout ERROR_VARIABLE _runtime_stderr)
if(_runtime_result STREQUAL "0" OR EXISTS "${_runtime_outside}/child")
    message(FATAL_ERROR "post_configure_symlink: action accepted redirect or modified outside tree")
endif()
set(_runtime_text "${_runtime_stdout}\n${_runtime_stderr}")
if(NOT _runtime_text MATCHES "directory path crosses a symlink")
    message(FATAL_ERROR "post_configure_symlink: missing runtime refusal diagnostic: ${_runtime_text}")
endif()

file(REMOVE_RECURSE "${_test_root}")
message(STATUS "DirectorySetup physical path regression tests passed")
