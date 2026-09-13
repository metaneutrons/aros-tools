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

## Prepare exact inputs

Set paths outside all three checkouts. The example names only locations; obtain
the actual revisions from `toolchains/producer-executor-v1.toml`, not from a
moving branch name.

```sh
export AROS_SOURCE=/absolute/path/to/AROS-NX
export PRODUCER=/absolute/path/to/aros-toolchains
export TOOLS=/absolute/path/to/aros-tools
export AROS="$TOOLS/target/release/aros"
export CACHE=/absolute/path/to/producer-cache
export WORK=/absolute/path/to/pc-candidate-work
export OUTPUT=/absolute/path/to/pc-candidate-output
export RECIPE=/absolute/path/to/pc-candidate.recipe.json

(
  cd "$TOOLS"
  cargo build --locked --release -p aros-cli
)
git -C "$AROS_SOURCE" status --short
git -C "$PRODUCER" status --short
git -C "$TOOLS" status --short
```

Each status command must print nothing. Do not place `CACHE`, `WORK`, `OUTPUT`,
or `RECIPE` below one of the checked-out roots.

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

The source lock and profile paths are producer-root-relative inputs. Recipe
creation never replaces an existing file.

```sh
"$AROS" toolchain producer recipe \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --source-lock toolchains/llvm-11.0.0.sources.json \
  --profiles toolchains/profiles-v1.json --output "$RECIPE" --format json

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
an AROS build with `--toolchain-dir`, but remains local-only. It has no release
provenance and cannot be promoted by copying it into a consumer lock.

## Failure and recovery boundary

The producer preserves owned work/output roots on failure, cancellation, and
deadline expiry. Inspect their lifecycle receipts and logs, correct the exact
input problem, and start with fresh roots. Do not delete or reuse retained
directories as an implicit resume mechanism. Packaging-only recovery and
release qualification have distinct evidence requirements; see the
[release reference](/aros-tools/reference/releases/) for the published-product
boundary.
