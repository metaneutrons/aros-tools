---
title: Standalone tools
description: The seven companion programs, their supported inputs, and when to invoke them directly.
---

Most users enter through `aros build`. Direct invocation is useful for
integration work, inspecting artifacts, and reproducing one failing step.
All tools offer `--help`, `--version`, human/JSON diagnostics and explicit
local logging. Keep them at the same version as the frontend.

## aros-transpiler

**Input:** an AROS source tree and target selectors.
**Output:** a generated CMake graph, inventories and coverage reports.

The CLI accepts `--source-dir`, `--output`, `--ports-dir`, and the target
selectors `--cpu`, `--platform`, `--family`, `--variant`,
`--toolchain`, `--cpu32`, `--use-mmu`, `--float-abi`.
Prefer the recorded invocation from a configured build so you retain its exact
target context.

The implementation follows supported MetaMake constructs. It is not a general
GNU Make interpreter that can execute arbitrary recipes. Recognized capability
drift is fatal; other uncovered declarations remain visible in the generated
coverage reports. A successful translation alone is not 100% product coverage.

Ordinary source fetch declarations retain upstream's version, origin and
optional checksums. The transpiler does not calculate new pins.
See [the transpiler contract](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-transpiler/README.md).

## aros-verify

**Input:** source, transpiler output and an explicit report/cache directory.
**Purpose:** compare with an independent expansion of upstream GenMF semantics.

```sh
aros-verify --source /path/to/AROS \
  --generated /path/to/generated_targets.cmake \
  --work /path/to/verification \
  --cpu x86_64 --platform pc --toolchain llvm --profile architecture
```

Replace the illustrative paths with matching files from one build.
`--build-dir` additionally checks configured CMake target realization.
`--refresh` reruns GenMF instead of using cached expansions.
`--genmf-timeout-seconds` accepts 1–3600 seconds and defaults to 30.

The only supported coverage profile is **`architecture`**. Core/distribution
reachability is not exposed as another verified profile.
`--no-gate` is report-only mode and must not be interpreted as a passing
coverage gate. There is no `aros-verify genmf` subcommand.

Source: [verifier CLI and profile model](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-verify/src/lib.rs).

## aros-collect

**Input:** linker arguments and an explicit linker.
**Purpose:** link relocatable AROS objects and materialize symbol sets.

```sh
aros-collect --ld /path/to/ld.lld -- -r -o output.o input.o
```

This is the direct linking form. It preserves the caller's link contract and
supports `--keep-script PATH` and `--report PATH`.

The compiler-driver entry points `collect-aros` and `collect-aros32` use
the same collection engine with additional sysroot, undefined-symbol,
ABI-marking, stripping and executable-mode policy. They belong to the
cross-toolchain layout, not the eight-file tools archive.
Direct mode does not automatically enable those driver policies.

Source: [collector modes and diagnostics](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-collect/README.md).

## aros-fetch

**Input:** archive origins/name/suffixes, optional patches and checksum declarations.
**Purpose:** transport, verify, safely extract and patch third-party sources.

Primary options are `--archive-origins`, `--archive`, `--suffixes`,
`--destination`, `--location` (cache), `--patch-origins`,
`--patches`, `--base`, `--checksums`, `--require-checksums`,
`--offline` and `--force`.

Checksum entries use `filename=sha256:<64-hex-digest>`.
Strict mode requires complete archive and remote-patch coverage; a mismatch
always fails. Patch declarations use `name[:subdirectory[:option,...]]`
with the supported `-p0` through `-p9`, `-f`, `-N`, and `--forward`
options. The generated build supplies these values from source recipes.

`--rename-directory` is parsed for historical compatibility but rejects a
nonempty value; renaming is not an implemented operation.

Source: [fetch CLI contract](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-fetch/src/contract.rs).

## aros-genmodule

**Input:** AROS `.conf` module declarations.
**Output:** SDK headers and optionally module-private headers, library-base
inventories and link-library sources.

Use `--scan-dir` and required `--output-inc`; optional destinations are
`--output-gen`, `--output-libbases` and `--output-linklib`.
Outputs must share a writable build root for journaled publication.

Set `--arch-dirs` to the exact architecture directories of the configured
target. Without that filter the scanner visits all architecture subtrees,
which can contain modules with the same name.

Source: [generator arguments](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-genmodule/src/lib.rs).

## aros-romtool

**Supported format:** the kickstart **PKG** container.
The current executable is not a general disk-image or arbitrary ROM-format tool.

| Command | Purpose |
| --- | --- |
| `pkg create --output FILE MODULE...` | Pack modules in load order |
| `pkg list FILE` | Inspect package members |
| `pkg extract FILE --directory DIR` | Extract the package |

Create options include `--basename`, `--allow-non-elf`, and
`--replace-if-sha256 SHA256`. By default, creation does not replace an
existing file and expects ELF members. Conditional replacement requires the
existing file's exact digest.

Source: [ROM tool command model](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-romtool/src/main.rs).

## aros-ahi-runner

**Input:** one generated, declarative AHI contract.
**Purpose:** validate the input closure or execute its fixed build stages.

```sh
aros-ahi-runner --contract /path/to/ahi-contract.cmake --validate-only
```

Without `--validate-only`, it executes the validated build.
The implemented AHI modes are x86-64, ARM and AArch64; there is no RISC-V AHI mode.
It is not a generic shell-script runner: the contract fixes supported inputs,
paths, identities, build stages and products.

Source: [AHI runner CLI](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-ahi-runner/src/main.rs).

## Libraries and internal tools

`aros-board`, `aros-common`, `aros-cmake-engine` and
`aros-macos-disk-claim` are workspace libraries, not additional installed
commands. `aros-release` is the internal archive producer.
See [architecture](/aros-tools/reference/architecture/) for ownership.
