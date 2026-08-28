# `aros-genmodule`

`aros-genmodule` parses AROS module `.conf` declarations and generates the SDK
headers, module sources, link-library sources, interfaces, and varargs glue
required by the CMake build. Generation is deterministic and writes a file
only when its bytes changed, avoiding timestamp-only rebuilds.

The library owns configuration interpretation and generated-source semantics.
Its binary is a thin argument/exit boundary; build scheduling and target
selection remain in CMake and `aros-cli`.

Run `aros-genmodule --help` for the current generated-output contract. Unit
tests cover configuration variants, architecture filtering, collision
handling, stale-output pruning, and generated ABI shapes.
