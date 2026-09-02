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
