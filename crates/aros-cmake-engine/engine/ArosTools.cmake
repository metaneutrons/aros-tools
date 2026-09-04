include_guard(GLOBAL)

include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

set(AROS_RUST_TOOLS_DIR "" CACHE PATH
    "Directory containing the installed aros-tools executable suite")

function(_aros_tools_default variable executable description)
    if(NOT DEFINED ${variable} OR "${${variable}}" STREQUAL "")
        set(${variable} "${AROS_RUST_TOOLS_DIR}/${executable}"
            CACHE FILEPATH "${description}" FORCE)
    endif()
endfunction()

# Resolve the complete host-tool suite once at the top-level boundary. An
# explicit directory is deterministic and is what aros-cli supplies. Direct
# CMake users may instead install aros-tools in PATH; aros-transpiler then acts
# as the suite anchor and every required sibling is validated below.
function(aros_configure_rust_tools)
    if(NOT AROS_RUST_TOOLS_DIR)
        if(DEFINED AROS_TRANSPILER_BIN AND AROS_TRANSPILER_BIN)
            get_filename_component(_aros_tools_dir
                "${AROS_TRANSPILER_BIN}" DIRECTORY ABSOLUTE)
        else()
            find_program(_aros_tools_transpiler NAMES aros-transpiler NO_CACHE)
            if(_aros_tools_transpiler)
                get_filename_component(_aros_tools_dir
                    "${_aros_tools_transpiler}" DIRECTORY ABSOLUTE)
            endif()
        endif()
        if(_aros_tools_dir)
            set(AROS_RUST_TOOLS_DIR "${_aros_tools_dir}" CACHE PATH
                "Directory containing the installed aros-tools executable suite"
                FORCE)
        endif()
    endif()

    if(NOT AROS_RUST_TOOLS_DIR)
        message(FATAL_ERROR
            "AROS-NX requires the aros-tools host executable suite. Install "
            "aros-tools so aros-transpiler is in PATH, or configure "
            "-DAROS_RUST_TOOLS_DIR=/absolute/path/to/bin. aros-cli supplies "
            "this setting automatically.")
    endif()

    _aros_tools_default(AROS_GENMODULE_BIN aros-genmodule
        "aros-genmodule executable used to bootstrap the SDK")
    _aros_tools_default(AROS_TRANSPILER_BIN aros-transpiler
        "aros-transpiler executable used to generate the build graph")
    _aros_tools_default(AROS_COLLECT_BIN aros-collect
        "aros-collect executable used to link and collect symbol sets")
    _aros_tools_default(AROS_AHI_RUNNER_BIN aros-ahi-runner
        "aros-ahi-runner executable for the closed AHI build contract")
    _aros_tools_default(AROS_FETCH_BIN aros-fetch
        "aros-fetch executable used by source-fetch targets")
    _aros_tools_default(AROS_VERIFY_BIN aros-verify
        "optional aros-verify executable used by verification targets")
    _aros_tools_default(AROS_ROMTOOL_BIN aros-romtool
        "optional aros-romtool executable used to create PKG containers")

    set(_aros_missing_tools "")
    foreach(_aros_tool IN ITEMS
            AROS_GENMODULE_BIN
            AROS_TRANSPILER_BIN
            AROS_COLLECT_BIN
            AROS_AHI_RUNNER_BIN
            AROS_FETCH_BIN)
        aros_path_is_executable("${${_aros_tool}}" _aros_tool_executable)
        if(NOT _aros_tool_executable)
            list(APPEND _aros_missing_tools
                "${_aros_tool}=${${_aros_tool}}")
        endif()
    endforeach()
    if(_aros_missing_tools)
        list(JOIN _aros_missing_tools "\n  " _aros_missing_report)
        message(FATAL_ERROR
            "The selected aros-tools suite is incomplete or not executable:\n"
            "  ${_aros_missing_report}\n"
            "Install one complete aros-tools release, or set the affected "
            "AROS_*_BIN cache entries explicitly.")
    endif()
endfunction()
