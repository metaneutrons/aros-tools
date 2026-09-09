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
"$AROS" toolchain producer cache \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --cache-dir "$CACHE" --format json
```

The native build also compiles Rust helpers from the exact tools `Cargo.lock`.
Prepare its vendor closure once, while online, before requesting `--offline`.
This is Cargo cache preparation, not a producer stage and not a claim that the
subsequent build used the network:

```sh
(
  cd "$TOOLS"
  cargo vendor --locked --versioned-dirs "$CACHE/cargo-vendor" \
    > "$CACHE/cargo-vendor-config.toml"
)
python3 - "$CACHE/cargo-vendor-config.toml" "$CACHE/cargo-vendor" <<'PY'
from pathlib import Path
import sys

config = Path(sys.argv[1])
vendor = str(Path(sys.argv[2]).resolve())
content = config.read_text(encoding="utf-8")
if content.count(vendor) != 1:
    raise SystemExit("Cargo vendor configuration does not contain one expected directory")
config.write_text(content.replace(vendor, "__CARGO_VENDOR_DIRECTORY__"), encoding="utf-8")
PY

"$AROS" toolchain producer cache \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --cache-dir "$CACHE" --verify-only --offline --format json
```

The resulting `cargo-vendor` tree and its template are checked against the
selected `Cargo.lock` before the native lifecycle invokes Cargo. A changed tools
commit requires a new matching vendor closure. Never repair a missing entry
during an offline build.

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
  --jobs 8 --timeout-seconds 21600 --offline --format json
```

Proceed only when `readiness` is `ready`. A blocked or invalid plan is a
diagnostic, not an invitation to change its identities manually.

## Build and verify the local prefix

```sh
"$AROS" toolchain build --preset pc-x86_64 --recipe "$RECIPE" \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --work-dir "$WORK" --output-dir "$OUTPUT" --cache-dir "$CACHE" \
  --jobs 8 --timeout-seconds 21600 --release-id local-pc-candidate \
  --offline --format json

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
