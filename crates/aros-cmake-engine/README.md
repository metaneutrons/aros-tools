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

The embedded `manifests/` directory owns the AHI and GRUB product inventories.
Engine modules and build runners resolve those inventories from the selected
engine, never from a source checkout's legacy `cmake/` copy. Source recipes,
patches and input manifests remain source-owned. The contract fixtures exercise
that boundary against isolated source trees without a CMake engine.

Ordinary `%fetch` declarations can select `normalization=canonical-tar-gzip-v1`
with `normalized_size=<bytes>` and an explicit archive SHA-256. The transpiler
and both configure-time source inventory and build-time fetching preserve those
fields. `aros-fetch` validates the final canonical bytes; local/cache inputs
must already match that representation. Without a normalization declaration,
received archive bytes remain unchanged. Neither the engine nor the transpiler
infers this policy from a URL or replaces missing source checksums.

Target libc header selection preserves the source's compile policy. LLVM uses
ordinary POSIX-C, standard-C and SDK include paths in that order. GNU AROS
drivers already declare SDK system paths, so the engine uses explicit system
namespace paths to preserve the order despite GCC's duplicate-directory rules.
Generated/private headers keep their own precedence. A source `-noposixc`
disables the implicit POSIX namespace, including the GNU driver's specs; an
explicit source include can still opt in. The focused runtime namespace fixture
compiles C and C++ consumers and requires an explicit compiler for GNU probes.

The Titan `opensbi-uefi-artifacts` target also emits a versioned
`media-build-receipt.json` from its five staged files. The receipt generator
measures the files after staging; `opensbi-uefi-verify` recomputes and compares
the receipt without changing it. With an installed release-toolchain manifest,
the producer emits v2 and measures the clean source Git commit/tree plus the
toolchain release/tree/manifest identities. A local toolchain without that
manifest keeps the explicitly weaker v1 receipt. `aros image build` independently
remeasures a v2 identity against `--source-root` and `--toolchain-root`; external
files still require reviewed input locks. The receipt does not prove that the
legacy core-object dependency has been removed or that the board boots.
