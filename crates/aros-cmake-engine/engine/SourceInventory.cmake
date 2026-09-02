include_guard(GLOBAL)
include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

# Some upstream modules derive their complete source inventory with a Make
# wildcard below PORTSDIR. Fetched public-header trees must also exist before
# the transitive header-owner pass can inspect their consumers. The transpiler
# reports only those exact owning fetches on its first cold pass; CMake
# materialises them and runs the transpiler again before generated_targets.cmake
# is included.
function(aros_fetch_source_inventory)
    set(oneValueArgs NAME ARCHIVE SUFFIXES ORIGINS CHECKSUMS LOCATION DESTINATION BASE
        PATCH_ORIGINS PATCHES)
    cmake_parse_arguments(SI "" "${oneValueArgs}" "" ${ARGN})
    if(SI_UNPARSED_ARGUMENTS OR NOT SI_NAME OR NOT SI_ARCHIVE OR
       NOT SI_DESTINATION OR NOT AROS_FETCH_BIN)
        message(FATAL_ERROR
            "aros_fetch_source_inventory received an incomplete fetch declaration")
    endif()
    aros_path_is_executable("${AROS_FETCH_BIN}" _aros_fetch_executable)
    if(NOT _aros_fetch_executable)
        message(FATAL_ERROR
            "${SI_NAME}: required aros-fetch executable is unavailable at ${AROS_FETCH_BIN}. "
            "Run `aros build-tools build` or set AROS_FETCH_BIN explicitly.")
    endif()
    set(_location "${SI_LOCATION}")
    if(NOT _location)
        set(_location "${SI_DESTINATION}")
    endif()
    set(_base "${SI_BASE}")
    if(NOT _base)
        set(_base "${SI_DESTINATION}")
    endif()
    file(MAKE_DIRECTORY "${_location}" "${_base}" "${SI_DESTINATION}")
    message(STATUS
        "🌐 AROS-NX: fetching ${SI_NAME} to determine its source/header inventory")
    set(_fetch_policy_args "")
    if(AROS_FETCH_OFFLINE)
        list(APPEND _fetch_policy_args --offline)
    endif()
    if(AROS_FETCH_REQUIRE_CHECKSUMS)
        list(APPEND _fetch_policy_args --require-checksums)
    endif()
    execute_process(
        COMMAND "${AROS_FETCH_BIN}"
            --archive-origins "${SI_ORIGINS}"
            --archive "${SI_ARCHIVE}"
            --suffixes "${SI_SUFFIXES}"
            --checksums "${SI_CHECKSUMS}"
            --location "${_location}"
            --destination "${SI_DESTINATION}"
            --base "${_base}"
            --patch-origins "${SI_PATCH_ORIGINS}"
            --patches "${SI_PATCHES}"
            ${_fetch_policy_args}
        RESULT_VARIABLE _result
        ERROR_VARIABLE _error)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "${SI_NAME}: configure-time source-inventory fetch failed "
            "(${_result})\n${_error}")
    endif()
    file(TOUCH "${SI_DESTINATION}/.${SI_ARCHIVE}-fetched")
endfunction()
