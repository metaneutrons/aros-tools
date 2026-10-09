---
title: Build a local native toolchain candidate
description: Prepare exact inputs and an offline cache, then build and verify a local AROS compiler candidate without publication authority.
---

This is a maintainer workflow. It produces a **local-only** candidate from
three exact Git checkouts; it does not create an archive, tag, release,
attestation, or package-manager publication. For a published compiler, use
[Choose and verify a toolchain](/aros-tools/workflows/toolchains/) instead.

The command does not discover a neighbouring checkout. All paths are explicit
and the selected commits must agree with the producer declaration. Keep source,
producer, tools, cache, work, output, and recipe paths separate. Use clean
checkouts and absent work/output leaves.

## What you need

The native lifecycle selects LLVM or GNU from the bound source lock and
profile. GNU uses source-lock v3, profiles v2 and explicit RISC-V ISA/ABI
contracts; all selected identities must agree. It selects the locked GCC and
binutils versions and installs the Rust collector with measured GNU tool roles.
Package and verify-package accept these candidates with a schema-2 manifest
and an inventory-bound executable layout. These are local development paths,
not a published RISC-V compiler, native board-build qualification or boot claim.
The examples below retain the existing PC/LLVM selection.

- A clean AROS source checkout, an `aros-toolchains` producer checkout, and an
  `aros-tools` checkout at the commits selected by the producer declaration.
- The `aros` executable built from that selected tools checkout.
- Cargo, the selected Rust toolchain, CMake, a compatible `make`, Git, a host
  C/C++ compiler, and Python for the unchanged upstream AROS build.
- An online cache-bootstrap step followed by an offline build. The offline
  build itself never falls back to transport.

The [prerequisites](/aros-tools/getting-started/prerequisites/#local-native-producer-candidates)
page records measured storage from the current PC proof. Treat those values as
an observed floor, not a reservation guarantee.

GNU candidates additionally require GNU Make 4.0 or newer, bison, flex, patch,
pkg-config and Ninja. On macOS select `gmake`; Apple's Make 3.81 cannot execute
the source's generated rules. Mako and MarkupSafe are mandatory locked Python
imports. A selected source that also needs PyYAML must declare its `yaml` import
and exact archive/version in the lock; ambient Python packages are not accepted.
GCC's format lists are narrowed to exactly one hash-verified lock entry before
the native Rust fetch bridge runs, never used as transport fallbacks. GNU
cache subdirectories are stamp namespaces, not alternative source locations.
Both compiler families give the Rust fetcher a private verified source copy and
enforce offline checksum validation. The bridge translates the source's
MetaMake fetch arguments without executing its `fetch.sh`; patch paths must
remain inside the measured source snapshot. LLVM keeps its two-package Python closure.

## Prepare exact inputs

Set paths outside all three checkouts. The example names only locations; obtain
the actual revisions from `toolchains/producer-executor-v1.toml`, not from a
moving branch name.

```sh
export AROS_SOURCE=/absolute/path/to/AROS-NX
export PRODUCER=/absolute/path/to/aros-toolchains
export TOOLS=/absolute/path/to/aros-tools
export TOOLS_TARGET=/absolute/path/to/producer-tools-target
export AROS="$TOOLS_TARGET/release/aros"
export CACHE=/absolute/path/to/producer-cache
export WORK=/absolute/path/to/pc-candidate-work
export OUTPUT=/absolute/path/to/pc-candidate-output
export RECIPE=/absolute/path/to/pc-candidate.recipe.json

(
  cd "$TOOLS"
  cargo build --locked --release -p aros-cli --target-dir "$TOOLS_TARGET"
)
git -C "$AROS_SOURCE" status --short
git -C "$PRODUCER" status --short
git -C "$TOOLS" status --short
```

Each status command must print nothing. Do not place `TOOLS_TARGET`, `CACHE`,
`WORK`, `OUTPUT`, or `RECIPE` below one of the checked-out roots. The native
input audit also rejects ignored build artifacts; `git status --short` alone
does not prove a clean raw checkout.

## Bootstrap and verify the cache

First acquire the source archives selected by the producer lock while transport
is allowed. The command verifies existing objects and refuses a mismatched
payload.

```sh
"$AROS" cache sources fetch \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --dir "$CACHE" --format json
```

The native build also compiles Rust helpers from the exact tools `Cargo.lock`.
Prepare the immutable vendor generation through `aros`; it selects the clean
tools Git tree, the producer's pinned Rust channel, and the exact Cargo
executable. It runs Cargo with a private `CARGO_HOME`, writes a single
validated directory placeholder, checks every vendor checksum and publishes
only a complete generation under `$CACHE/cargo/v1/…`. It never copies a user
Cargo configuration or credential into the cache.

```sh
"$AROS" cache cargo fetch \
  --producer-dir "$PRODUCER" --tools-dir "$TOOLS" --dir "$CACHE" \
  --format json

"$AROS" cache cargo verify \
  --producer-dir "$PRODUCER" --tools-dir "$TOOLS" --dir "$CACHE" \
  --format json

"$AROS" cache sources verify \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --dir "$CACHE" --format json
```

Use `cache cargo list` when only the selected generation receipt is needed; it
does not hash the vendor tree, but it runs bounded Git and `cargo --version`
probes to prove the selection. A changed tools tree, lockfile, producer Rust
pin, or Cargo executable identity selects a different generation. The native
lifecycle then accepts only that exact generation, copies it into a private
runtime directory and invokes its collector with `--locked --offline`. Never
repair a missing generation during an offline build; use `cache cargo fetch
--offline` only to require a previously verified one.

## Construct the recipe and inspect readiness

Select source lock and profile files inside the producer checkout using absolute
paths. The recipe records their producer-relative identities. Recipe creation
never replaces an existing file.

```sh
"$AROS" toolchain producer recipe \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --profiles "$PRODUCER/toolchains/profiles-v1.json" --output "$RECIPE" --format json

"$AROS" toolchain plan --preset pc-x86_64 --recipe "$RECIPE" \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --work-dir "$WORK" --output-dir "$OUTPUT" --cache-dir "$CACHE" \
  --jobs 8 --timeout-seconds 21600 --format json
```

Proceed only when `readiness` is `ready`. A blocked or invalid plan is a
diagnostic, not an invitation to change its identities manually.

## Build and verify the local prefix

```sh
"$AROS" toolchain build --preset pc-x86_64 --recipe "$RECIPE" \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --work-dir "$WORK" --output-dir "$OUTPUT" --cache-dir "$CACHE" \
  --jobs 8 --timeout-seconds 21600 --release-id local-pc-candidate \
  --format json

cd "$AROS_SOURCE"
"$AROS" toolchain verify --preset pc-x86_64 --local "$OUTPUT/toolchain"
"$OUTPUT/toolchain/bin/clang" --version
"$OUTPUT/toolchain/bin/ld.lld" --version
```

The result and its six lifecycle receipts bind the source, producer, tools,
executor, host, target, and cache inputs. The prefix may be used explicitly by
an AROS build with `--toolchain-dir` where its compiler family is supported by
that consumer; producer acceptance alone does not qualify a native GNU board
build. It remains local-only, has no release provenance, and cannot be promoted
by copying it into a consumer lock.

The separate `toolchain producer package` and `verify-package` stages accept
`--package-format legacy-v1|family-v2`. Omitting the option preserves the
existing family default: LLVM uses its historical schema-v1 manifest and v1
asset name, while GNU uses compiler-family schema v2. For an explicit LLVM
schema-v2 package, pass `--package-format family-v2`; its manifest records the
LLVM compiler family and version and its asset uses the v2 LLVM name. The
historical `legacy-v1` choice remains LLVM-only. This explicit local format
does not add LLVM v2 to the current release index, publication, qualification,
or recovery paths.

For GNU builds, configure records host compiler prefix maps in `HOST_*FLAGS`.
The compiler-build process does not export `CFLAGS` or `CXXFLAGS`: MetaMake
owns the target ISA flags, which must not reach host-built Binutils or GCC.
LLVM builds retain their existing compiler environment.
Compiler caching defaults to `off`, including release qualification. To reuse
host C/C++ compilation during local development, prepare a managed local
namespace and add `--compiler-cache sccache` (or `ccache`) to the build command:

```sh
aros cache compiler prepare --backend sccache
# Add to the toolchain build invocation above:
# --compiler-cache sccache
```

`--compiler-cache auto` selects a prepared local sccache namespace first, then
ccache, or remains off. An explicit `--compiler-cache-dir DIR` requires an
explicit backend and an already prepared namespace outside all producer roots.
Offline execution remains mandatory; ambient cache settings and remote storage
are not imported. The compiler phase receives controlled host C/C++ launchers;
target runtime compilation, assembly, linking and the Rust collector are not
cached by this integration. Receipts bind the backend executable, host
compilers and local configuration. Resume requires the same bindings and
unchanged launchers. Use `--compiler-cache off` for independent A/B builds.
On Unix, sccache needs short paths for both its managed socket and the startup
socket below the owned work directory. An oversized path is rejected before
configure; select shorter roots or use ccache instead.

GNU configure and MetaMake also resolve recursive `make` through a private
alias to the exact preflight-selected GNU Make, rather than a second executable
found elsewhere on the host PATH.

The v3 source lock can declare source-owned patches for target build
dependencies outside the compiler directory. Recipe creation reads each patch
from the exact clean source commit and binds its SHA-256; missing, unsafe or
duplicate paths and an incomplete recipe patch set are rejected. Compiler
component patches remain restricted to their selected family directory.

## Failure and recovery boundary

The producer preserves owned work/output roots on failure, cancellation, and
deadline expiry. Inspect their lifecycle receipts and logs, correct the exact
input problem, and start with fresh roots. Do not delete or reuse retained
directories as an implicit resume mechanism. Packaging-only recovery and
release qualification have distinct evidence requirements; see the
[release reference](/aros-tools/reference/releases/) for the published-product
boundary.
