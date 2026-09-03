# Moving the CMake engine out of the source tree

## Why

The engine is the set of CMake modules a transpiled target graph calls into. It
lived in the AROS source tree, but it does not describe a tree, it describes how
any tree is built. The transpiler emits calls into it, so the generated graph
and these modules are two halves of one contract, and that contract was split
across two repositories with a lock between them: a signature change needed two
synchronised commits.

Carrying the engine here also removes the reason the CMake product flow cannot
run against a pristine upstream checkout. A checkout that never heard of our
build system cannot be expected to hold it.

## What is done

**The engine is a crate.** `aros-cmake-engine` embeds all 158 files through a
build script that walks them into one sorted table and pins a SHA-256 over path,
length and content. `materialize()` places them in a build directory and makes
that directory hold exactly the engine, removing anything foreign, because a
module left by an older engine stays visible to `include()`. A stamp records the
digest so an unchanged directory is reused, and the stamp is verified against the
files it names.

**The API version has one source.** `engine/EngineVersion.cmake` declares it and
the build script reads it, so the Rust constant and the CMake declaration cannot
drift.

**The tree is an input.** `AROS_SOURCE_DIR` names the AROS checkout,
`AROS_CMAKE_ENGINE_DIR` names the engine, and `CMAKE_SOURCE_DIR` keeps only its
own meaning: the project being configured. It survives in the four places that
walk `BUILDSYSTEM_TARGETS`, which is CMake's directory structure rather than the
filesystem. Three root checks needed more than a rename, because they resolved an
input against the tree and then tested it against the project root; that is the
same directory today and would have rejected every tree input tomorrow.

The transpiler emitted the old name in 151 places. `SRCDIR` always meant the
tree, so they moved together.

**Proved, not assumed.** `cmake -S <engine> -B <build> -DAROS_SOURCE_DIR=<tree>`
configures from outside a checkout and writes 77,456 targets. The generated graph
holds 1,484 `AROS_SOURCE_DIR` references and no `CMAKE_SOURCE_DIR`, and the
source tree ends the run with no modified paths.

## The CLI configures from the engine

`aros build` no longer names a preset. It places the embedded engine in
`<build>/cmake-engine` and configures `-S <engine> -B <build>
-DAROS_SOURCE_DIR=<checkout>`, so nothing is written into the checkout.

The ten cache variables a preset carried are derived from the target profile,
because none is a free choice: the system name is fixed for a bare-metal target,
the processor is the architecture, the compilers are the LLVM path the target
graph assumes, and the bootloader follows the platform. `bootloader` became
profile data with that rule as its default. Build type and board model, the only
other things the debug presets varied, are `--debug` and the model the board
configuration already holds.

`--engine-dir` replaces the embedded engine and is honoured only when given.
An engine lying in the checkout is never preferred on its own. `aros info`
reports the digest, file count and API version in use, and the generated graph
opens with `aros_require_engine_api_version(N)` so a mismatch is named rather
than surfacing as an unknown function.

**Measured**: `aros build --preset pc-x86_64 --target kernel-exec --clean`
reaches step 22 of 3291 and stops on nine `AF0501` fetch errors. The unchanged
tools on `main` reach the same step and stop on the same nine. The path is
behaviour-equal and those failures are pre-existing, in `aros-fetch`, unrelated
to this work.

AROS-NX PR #28 removes the 159 files from that tree.

## What is left

**Two engine fixtures.** The engine's own CMake tests used to inherit the tree
and the tool paths from the checkout they sat in. `EngineTestTree.cmake` now
supplies both and fails loudly rather than skipping, and `AROS.cmake` reaches
BootstrapSDK through `CMAKE_MODULE_PATH` so a fixture can still substitute its
stub, which it previously did by relying on `CMAKE_SOURCE_DIR` naming the
fixture.

33 of 35 pass. `FetchArchivePatchTest` builds its fixture inline and does not
give it a compiler, which the real `BootstrapSDK` now needs. `AhiBuildTest` used
to fail immediately because it could not find `aros-ahi-runner`; it now finds it
and runs a real AHI build, which takes long enough that its result is not yet
recorded here.

**Built-in profiles.** `aros-targets.toml` holds nothing tree-specific, so the
same profiles can serve as defaults for a checkout without one. That is what
makes a pristine upstream tree configurable, and it is not written yet.

**The nine AF0501 fetch failures** are pre-existing and worth their own look:
`aros-fetch` refuses to publish into a temporary directory it just created, in a
freshly cleaned build tree.
