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
`--toolchain`, `--cpu32`, `--use-mmu`, `--float-abi`. It also records optional
source selectors: `--mesa-version`, `--target-llvm-ver`,
`--target-llvm-runtimes-style`, `--target-rust`, and `--target-rust-ver`.
Prefer the recorded invocation from a configured build so you retain its exact
target context. Supplying a selector does not add support for an otherwise
unmodeled Mesa, LLVM, or Rust recipe.

For a source-bound native profile with a cold ports tree,
`--source-inventory-only` prepares only the
`*.source-inventory.cmake` sidecar. It does not create or replace a build graph
and is not a build qualification. Only source and fetched-header owners in the
profile's validated dependency closure enter this inventory; unrelated ports
are not prepared. Cold declaration identities do not create compile targets.
The CMake engine materializes the selected inventories, then requires a full
export before using any targets. Missing or ambiguous selected producers stop
preparation rather than becoming empty placeholder targets.

For diagnosis, add `--native-graph-audit report.json` to a native
`--source-inventory-only` invocation. It traverses all known reachable branches
and records missing endpoints, unproven recipes, capability failures and
unowned diagnostics. It writes only the separate JSON report, never a graph
or inventory. Audit success means the report was produced, not that the build
is supported; rejected declarations may still hide prerequisites.
The report distinguishes selected failures from diagnostics without proven
owners and lists every typed producer family. Native discovery honors only
literal directory exclusions from the contract-bound `mmake.config.in`;
unresolved configure substitutions do not exclude source directories.
`source_meta_semantics` records imported source-generated virtual aliases,
verified optional selector contracts and omitted absent architecture-hook
edges. A sealed invocation policy and exact source declaration are required;
uncontracted, literal, independently required and known-provider edges remain
mandatory. These metadata routes do not create executable producers or prove
a build. See [native inputs](/aros-tools/reference/native-media-inputs/) for the
admission rules.
Diagnostic ownership follows only complete, source-proven consumer chains.
If an alternative consumer is unresolved, the diagnostic stays unowned.
Macro evidence verifies the reachable helper definitions, not just the outer
macro. Unrepresented includes or inline macro effects veto attribution.
`build_prog` remains uncertain for diagnostic attribution until its implicit
objects and generated dependency inputs are sealed; native program generation
is a separate capability. Ownership searches have a shared work and path-size
limit. Exceeding either limit leaves the diagnostic unowned.
Inert Make `define` bodies and commented declarations never enable a producer;
attributing a disabled declaration for diagnosis does not activate it.
For a native profile with a sealed MetaMake invocation policy, complete
discovery and GenMF ownership metadata also establish which recipes are called.
An unowned parser capability failure outside that invocation is recorded under
`source_uninvoked_capability_failures`, with every actual parser-input origin.
A displayed diagnostic path alone cannot exclude a failure. Selected, shared,
unknown and global origins remain fatal; named failures retain target-closure
validation. Classic metadata and actual native parser-input provenance are
checked independently; implicit native link dependencies also protect their
declaring recipes. An available native endpoint without bound input provenance
prevents exclusions. Without the policy or resolved native roots, failures remain fatal.
Successful native exports and source preparation publish a
`*.native-invocation.json` sidecar with this scope evidence. It does not prove
support for excluded rules or a successful build. Sources are rechecked before
publication; full-tree translation keeps its existing capability requirements.
Source-owned `host_make_variables` selects only the actual native host's
configuration; a missing declared host fails. Shared defaults cannot supply
host identities. Verified compile macros may associate dependency sidecars
with complete object-owner proofs for diagnosis, without creating build edges
or proving the contents of runtime dependency files.
Make-expression evaluation also limits recursion, aggregate work, output bytes
and list items. Exhaustion rejects the expression; it never supplies an empty
result as a fallback.

The implementation follows supported MetaMake constructs. It is not a general
GNU Make interpreter that can execute arbitrary recipes. Recognized capability
drift is fatal; other uncovered declarations remain visible in the generated
coverage reports. A successful translation alone is not 100% product coverage.

Ordinary source fetch declarations retain upstream's version, origin and
optional checksums. The transpiler does not calculate new pins.
The GRUB host-tool capability accepts only audited 2.12 and 2.16 recipes for
the x86-64 PC target; it rejects other versions.
The current `aros-transpiler` source recognizes separately audited Mesa 20.0.8
and 26.0.0 recipes for the supported AROS target profiles. It checks their source
and generator inputs independently; selecting `--mesa-version` alone does not
qualify an archive or guarantee that an installed release contains that
capability. See [release status](/aros-tools/reference/release-status/) for the
published tools version.
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
The verifier stores only its content-addressed GenMF namespace below
`--work/genmf/v1/`; reports and legacy flat mtime entries remain outside that
namespace. `--refresh` reruns GenMF in a private interpreter environment and
requires any existing immutable expansion to match byte-for-byte instead of
replacing it. The matching `aros cache genmf` commands expose status, list, and
verification without running the verifier's coverage comparison.
`--genmf-timeout-seconds` accepts 1–3600 seconds and defaults to 30.

The only supported coverage profile is **`architecture`**. Core/distribution
reachability is not exposed as another verified profile.
`--no-gate` is report-only mode and must not be interpreted as a passing
coverage gate. There is no `aros-verify genmf` subcommand.

Source: [verifier CLI and profile model](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-verify/src/lib.rs).

## aros-collect

**Input:** linker arguments and an explicit linker, or a KOBJ and explicit
target `nm`/`objcopy` tools.
**Purpose:** collect AROS symbol sets or localize a completed KOBJ.

```sh
aros-collect --ld /path/to/ld.lld -- -r -o output.o input.o
```

Native KOBJ development also has a separate localization mode:

```sh
aros-collect --localize-kobj member.o --nm /path/to/target-nm \
  --objcopy /path/to/target-objcopy
```

It applies the source macro's plain-nm symbol filter and fixed library bases
to an already linked object. Both tools are explicit; linker arguments are
not accepted in this mode. Precommit failures leave the original unchanged.
A post-rename durability failure reports uncertain commit state and retains a
recovery journal; the new object may already be installed. This step alone does
not qualify a native core or board.

The `--ld` form preserves the caller's link contract and
supports `--keep-script PATH` and `--report PATH`.

The compiler-driver entry points `collect-aros` and `collect-aros32` use
the same collection engine with additional sysroot, undefined-symbol,
ABI-marking, stripping and executable-mode policy. They belong to the
cross-toolchain layout, not the eight-file tools archive.
Direct mode does not automatically enable those driver policies.

A GNU compiler-driver manifest permits the standard `a.out` output when
`-o` is omitted. Direct mode, LLVM drivers and manifest-free aliases require
an explicit output. Failed links preserve an existing output in either mode.

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
