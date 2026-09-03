# The AROS checkout a source-contract test reads.
#
# These tests exercise the engine against real AROS declarations, so they need a
# tree. They used to find it two directories above themselves, which held only
# while the engine lived inside a checkout. The tree is now named explicitly,
# through the same variable the Rust source-contract tests use, so both halves
# of the suite are configured the same way.
#
# Failing rather than skipping is deliberate: a silently skipped contract test
# reports success for a contract nobody checked.
include_guard(GLOBAL)

if(NOT DEFINED ENV{AROS_TEST_SOURCE_ROOT} OR "$ENV{AROS_TEST_SOURCE_ROOT}" STREQUAL "")
    message(FATAL_ERROR
        "AROS_TEST_SOURCE_ROOT must name an AROS checkout for this test. It "
        "reads real source declarations, and the engine no longer lives inside "
        "a tree it could infer.")
endif()
get_filename_component(AROS_TEST_TREE "$ENV{AROS_TEST_SOURCE_ROOT}" ABSOLUTE)
foreach(_marker configure Makefile.in arch compiler rom)
    if(NOT EXISTS "${AROS_TEST_TREE}/${_marker}")
        message(FATAL_ERROR
            "AROS_TEST_SOURCE_ROOT is missing the marker '${_marker}': "
            "${AROS_TEST_TREE}")
    endif()
endforeach()

# The aros-tools executables a fixture configure needs.
#
# A fixture used to inherit them from the checkout it was configured in. With
# the engine outside a tree they are named explicitly: from the environment when
# a caller sets one, otherwise from this workspace's own release directory,
# which is where `cargo build --release` leaves them.
if(DEFINED ENV{AROS_TEST_TOOLS_DIR} AND NOT "$ENV{AROS_TEST_TOOLS_DIR}" STREQUAL "")
    get_filename_component(AROS_TEST_TOOLS_DIR "$ENV{AROS_TEST_TOOLS_DIR}" ABSOLUTE)
else()
    get_filename_component(AROS_TEST_TOOLS_DIR
        "${CMAKE_CURRENT_LIST_DIR}/../../../../target/release" ABSOLUTE)
endif()
if(NOT EXISTS "${AROS_TEST_TOOLS_DIR}/aros-genmodule")
    message(FATAL_ERROR
        "No aros-tools executables at ${AROS_TEST_TOOLS_DIR}. Build the "
        "workspace with `cargo build --release`, or set AROS_TEST_TOOLS_DIR.")
endif()

# The individual executable variables, as `aros_configure_rust_tools()` would
# derive them. A fixture includes an engine module directly and never reaches
# that function, so the values it would have produced are supplied here.
set(AROS_TEST_TOOL_ARGS "")
foreach(_pair
        "AROS_TRANSPILER_BIN=aros-transpiler"
        "AROS_GENMODULE_BIN=aros-genmodule"
        "AROS_ROMTOOL_BIN=aros-romtool"
        "AROS_COLLECT_BIN=aros-collect"
        "AROS_AHI_RUNNER_BIN=aros-ahi-runner"
        "AROS_FETCH_BIN=aros-fetch"
        "AROS_VERIFY_BIN=aros-verify")
    string(REPLACE "=" ";" _parts "${_pair}")
    list(GET _parts 0 _variable)
    list(GET _parts 1 _executable)
    list(APPEND AROS_TEST_TOOL_ARGS
        "-D${_variable}=${AROS_TEST_TOOLS_DIR}/${_executable}")
endforeach()
