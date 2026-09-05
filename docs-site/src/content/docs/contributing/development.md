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
| `scripts/check-workspace.sh test` | Source-coupled tests with the exact qualified AROS-NX checkout |
| `scripts/check-workspace.sh` | Complete local gate |

The exact-source gate requires a recursive checkout of the immutable revision in
[`contracts/aros-source-v1.toml`](https://github.com/metaneutrons/aros-tools/blob/main/contracts/aros-source-v1.toml).
Pass it explicitly:

```sh
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh
```

Do not substitute a moving branch. Tests use that source as input and create
their own temporary work where needed. CMake-engine fixtures require clang,
CMake and Ninja; platform-specific omissions are reported explicitly.

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

## Submit a focused change

Use a Conventional Commit PR title (`fix:`, `feat:`, `docs:`, etc.).
Explain the user-visible behavior, affected contract and actual verification.
Include documentation changes with the behavior they describe.

A passing unit test is not evidence of hardware boot or complete product
coverage. Name the host/source/target you exercised and identify any untested
boundary.
