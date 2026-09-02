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

## What is left

**The CLI still configures through `--preset` inside the checkout.** That is the
last thing tying a build to a tree that holds the engine, and it is not a rename:

- `CMakePresets.json` binds to the tree only through
  `binaryDir: "${sourceDir}/build/<preset>"`. With the engine placed elsewhere
  that would put build output inside the placed engine, and `--preset` cannot be
  combined with an explicit `-B`.
- The presets carry ten cache variables per profile. Seven are the same
  everywhere (`CMAKE_SYSTEM_NAME`, the three compilers, `AROS_TOOLCHAIN`,
  `CMAKE_BUILD_TYPE`, `CMAKE_EXPORT_COMPILE_COMMANDS`), two follow the profile
  and are already in `aros-targets.toml` (`AROS_TARGET_CPU`,
  `AROS_TARGET_PLATFORM`, plus `CMAKE_SYSTEM_PROCESSOR` which equals the arch),
  and one is profile data we do not carry yet: `AROS_TARGET_BOOTLOADER`.

So the step is: add `bootloader` to the target profiles, have the CLI set the
cache variables from the profile instead of naming a preset, and pass `-S`, `-B`
and `-DAROS_SOURCE_DIR` explicitly. `aros-targets.toml` holds nothing
tree-specific, so the same profiles work as built-in defaults for a checkout that
has no such file.

**Then the rest follows**: an `--engine-dir` override that is honoured only when
given explicitly, never inferred from a directory that happens to sit in the
tree; `aros info` reporting which engine is in use and its digest; a
configure-time check that the generated graph's required API version matches the
engine's; and finally removing `cmake/` and `CMakeLists.txt` from AROS-NX.

The override must stay explicit. A `cmake/` directory found in a checkout and
silently preferred is the same failure this project already had once, when a
stale generated header outranked the current one and a wrong `FUNCTIONS_COUNT`
sized a jump table short.
