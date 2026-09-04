# The contract between the transpiler's generated graph and this engine.
#
# The generated target graph calls into the functions this engine defines. Raise
# this number whenever a call the generator emits changes shape: a renamed
# function, a new required argument, a changed meaning. The generator records the
# version it emits for, and a mismatch stops configuration instead of failing
# somewhere inside eighty thousand generated lines.
#
# This file is the single source. `aros-cmake-engine`'s build script reads the
# number from here, so the Rust side cannot drift from the CMake side.
include_guard(GLOBAL)

set(AROS_CMAKE_ENGINE_API_VERSION 1)

# Checks that a generated target graph was produced for this engine.
#
# The generator emits this call as its first statement. Without it a graph built
# for a different contract would fail somewhere inside eighty thousand generated
# lines, with a message about an unknown function rather than about a version.
function(aros_require_engine_api_version required)
    if(NOT required EQUAL AROS_CMAKE_ENGINE_API_VERSION)
        message(FATAL_ERROR
            "This target graph was generated for CMake engine API version "
            "${required}, but this engine provides "
            "${AROS_CMAKE_ENGINE_API_VERSION}. Regenerate the graph with the "
            "matching aros-transpiler, or point --engine-dir at the engine the "
            "graph was generated for.")
    endif()
endfunction()
