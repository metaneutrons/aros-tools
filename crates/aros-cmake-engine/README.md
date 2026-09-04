# aros-cmake-engine

The CMake modules that turn a transpiled target graph into a build, embedded in
the tool that produces that graph.

The engine used to live in the AROS source tree. It does not describe a tree, it
describes how any tree is built, and the transpiler emits calls into it, so the
two are one contract and belong in one version. Carrying it here also lets the
CMake path work against a pristine upstream checkout, which cannot be expected
to hold our build system.

`materialize()` writes the embedded copy into a build directory. Nothing is
written into the source tree.
