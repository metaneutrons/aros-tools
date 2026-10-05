---
title: Development workflow
description: Build and change the Rust workspace using its canonical quality and source-validation gates.
---

Start with [CONTRIBUTING.md](https://github.com/metaneutrons/aros-tools/blob/main/CONTRIBUTING.md).
Changes should keep behavior, diagnostics, tests and user documentation aligned.

## Prepare the contributor environment

The runtime versions are defined in
[`contracts/development-runtimes-v1.toml`](https://github.com/metaneutrons/aros-tools/blob/main/contracts/development-runtimes-v1.toml)
and `rust-toolchain.toml`. In addition to the user prerequisites, install
Node.js 24 or newer with npm, Python 3.11 or newer, actionlint, ShellCheck,
jq, GnuPG, dpkg-deb and the archive utilities named in CONTRIBUTING.

On macOS, after installing the Xcode Command Line Tools:

```sh
brew install actionlint cmake coreutils cosign curl dpkg gh git gnupg gnu-sed jq ninja node pkg-config python@3.14 shellcheck
```

On Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates cmake curl dpkg-dev gh git gnupg golang-go jq ninja-build patch pkg-config python3 shellcheck
go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12
```

Install Node.js 24 or newer through your normal distribution or version manager;
do not assume the distribution's default version meets the runtime contract.
Ensure Go's binary directory is on PATH after installing actionlint. Confirm
`node --version`, `python3 --version` and `actionlint -version` before running
the quality gate. Release-signature verification additionally needs a recent
cosign and GitHub CLI, as described in [prerequisites](/aros-tools/getting-started/prerequisites/).

Install the pinned Rust audit helpers:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-machete --version 0.9.2 --locked
```

## Run the relevant gate

From the tools repository:

| Command | Scope |
| --- | --- |
| `scripts/check-workspace.sh docs` | Locked docs dependency audit, Astro build, links and static-output validation |
| `scripts/check-workspace.sh quality` | Workspace quality, architecture and policy checks |
| `scripts/check-workspace.sh portable-test` | Source-independent tests |
| `scripts/check-workspace.sh source-test` | Full Rust tests with the exact qualified AROS-NX checkout; no CMake fixtures |
| `scripts/check-workspace.sh test` | Explicit integration checkpoint: source Rust + every host-compatible CMake fixture |
| `scripts/check-workspace.sh all` | Explicit complete gate: quality, docs and integration |
| `scripts/check-workspace.sh` | Default iteration gate: quality + portable Rust; no product build |

The exact-source gate requires a recursive checkout of the integration revision in
[`contracts/aros-source-v1.toml`](https://github.com/metaneutrons/aros-tools/blob/main/contracts/aros-source-v1.toml).
Use its `[integration]` pin, or `[source]` for an older contract without that
table. The `[source]`/`[producer]` pair records the qualified toolchain producer;
it is not changed merely to advance development tests.

Historical Mesa 20 regression tests also need a separate, clean recursive
AROS-NX checkout at `cb6974f1c3de43c6f1168d69039af7c32e56153c`.
Pass both checkouts explicitly; the Mesa 26 input is the current integration
checkout, not the historical tree:

```sh
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
AROS_TEST_MESA20_SOURCE_ROOT=/absolute/path/to/mesa20/AROS-NX \
AROS_TEST_MESA26_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh all
```

Do not substitute a moving branch. Tests use that source as input and create
their own temporary work where needed. CMake-engine fixtures require clang,
CMake and Ninja. Source-value parity tests additionally use GNU sed (`gsed`
from Homebrew on macOS) as an independent oracle, not a build dependency.
Platform-specific omissions are reported explicitly.

Every PR has a stable Linux x86-64 check. A narrow documentation-only set
(`README.md`, `CONTRIBUTING.md`, `docs/**` and `docs-site/**`)
uses that lane's portable test; all other changes are fail-closed to the three
active native hosts: Linux x86-64, Linux AArch64 and macOS AArch64. Intel macOS
is not an `aros-tools` release target. The full Linux CMake sweep runs after integration into
`main`. An explicit Workspace CI dispatch defaults to those three hosts, and a
weekly three-host sweep catches platform drift. Engine/source-boundary changes
require a full check of their final candidate before merge; cross-cutting
milestone acceptance and release candidates require Linux and Darwin/arm64
evidence. The real GRUB fixture runs only on Darwin/arm64, so Linux success
alone cannot prove it.
See the authoritative [CI policy](/aros-tools/reference/ci-policy/) and
[integration checkpoint policy](https://github.com/metaneutrons/aros-tools/blob/main/CONTRIBUTING.md#test-stages-and-integration-checkpoints).

Successful partial stages are not full qualification. Do not repeat a GRUB
build for docs-only changes; run `docs`. Do not replace a failed integration
check with a passing narrower stage.

## Change one behavior

Keep CLI orchestration in `aros-cli` and specialized work in its owning crate.
Use the shared process/diagnostic mechanisms. Add regression coverage where
a change affects public parsing, failure status, output publication or hardware
safety.

The independent verifier must remain independent of the transpiler.
See [architecture](/aros-tools/reference/architecture/) for these boundaries.

For a CMake-engine experiment, build the tools normally and pass
`aros build --engine-dir /absolute/path/to/engine` explicitly.
The default embedded engine is versioned with the tools.

When changing the native engine and its helpers together, rebuild the CLI,
transpiler and SDK header generator:

```sh
cargo build -p aros-genmodule -p aros-transpiler -p aros-cli
```

Building only `aros-cli` does not rebuild the standalone transpiler or header
generator. For an existing build tree, keep its cached engine source path: the
CLI refreshes that embedded engine in place. Do not switch `--engine-dir`
on a configured tree.

## Exercise the native PC llvmpipe path

This development lane requires the reviewed Mesa 26/Gallivm source recipes,
Target-LLVM 11 and CMake 3.24 or newer. It does not qualify ARM graphics.
From the configured AROS checkout, using the newly built tools:

```sh
CMAKE_BUILD_PARALLEL_LEVEL=4 AROS_LLVMPIPE_RUNTIME_PROBE=1 \
  aros build --preset pc-x86_64 --target boot-iso --jobs 4 --compiler-cache off
aros test --preset pc-x86_64 --iso build/pc-x86_64/aros-x86_64-pc.iso \
  --require-llvmpipe-jit --timeout 120 --memory 1024
```

The test ISO contains a tools-owned shader probe and an instrumented HIDD.
Its startup is staged into the image, not written into the AROS sources.
Success requires the expected renderer, a named non-null MCJIT shader address
and the correct rendered pixel in the same guest run, with no classified fault.
A guest supervisor supplies a 1 MiB workload stack and must confirm successful
return and ELF unloading before the CLI accepts the proof.
The CLI stops QEMU after complete proof or a definitive failure; deadline
expiry fails this strict test.
The retained evidence includes the image hash and complete logs.
Set both parallelism controls: `--jobs` bounds the outer build, while
`CMAKE_BUILD_PARALLEL_LEVEL` bounds the nested LLVM build. A single shared
job budget is tracked separately in
[issue #313](https://github.com/metaneutrons/aros-tools/issues/313).
Disable the cached probe option with `AROS_LLVMPIPE_RUNTIME_PROBE=0` when
building an ordinary image. See the
[native llvmpipe contract](https://github.com/metaneutrons/aros-tools/blob/main/docs/native-llvmpipe.md)
for the producer and evidence boundaries.

## Submit a focused change

Use a Conventional Commit PR title (`fix:`, `feat:`, `docs:`, etc.).
Explain the user-visible behavior, affected contract and actual verification.
Include documentation changes with the behavior they describe.

A passing unit test is not evidence of hardware boot or complete product
coverage. Name the host/source/target you exercised and identify any untested
boundary.
