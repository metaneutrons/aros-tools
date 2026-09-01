---
title: Command reference
description: Stable command groups, repository requirements, destructive boundaries and machine-readable output contracts.
---

The executable is `aros`. Run `aros --help` and `aros <command> --help` for the
authoritative option list of the installed version.

## Global options

Every frontend command accepts:

| Option | Purpose |
| --- | --- |
| `--diagnostic-format human|json` | Human error or one `aros-tool-diagnostics-v1` JSON document |
| `--log-level off|error|warn|info|debug|trace` | Minimum local log level |
| `--log-format human|jsonl` | Local log encoding |
| `--log-file PATH` | Explicit local log destination |

Logging is off by default. Selecting a level without a destination fails; a log
path without a level enables `info`. Equivalent `AROS_*` environment variables
are listed in `--help`.

## Source and repository

| Command | Checkout requirement | Responsibility |
| --- | --- | --- |
| `source init PATH` | none | Atomically clone upstream or a fork, configure remotes and optionally select a ref |
| `source sync` | required | Fetch one exact upstream-branch OID, validate it independently, and compare-and-swap fast-forward a clean branch |
| `info` | optional | Report host information and, when present, checkout contracts |
| `install --source-bin DIR --prefix DIR` | none | Validate and publish one complete native suite without replacing existing programs |

`source sync` never performs an implicit merge commit, works only with a clean
worktree (including ignored files) and clean recursive submodules, and refuses
an unexpected upstream identity. Local source identities are compared after
canonical filesystem resolution, so spelling and symlink aliases do not create
false mismatches. The canonical override is `--upstream`; there is no `sync` or
`--upstream-url` compatibility alias before the first release. `--ref` names a
safe branch below `refs/heads/` and is resolved once in an isolated fetch
quarantine. The caller's `FETCH_HEAD` is never used or replaced.

Candidate validation is independent of the mutable checkout. Source publication
rechecks a persistent kernel-lock identity and the captured Git semantics,
prefetches exact recursive submodule objects, then performs branch CAS and
network-disabled candidate-backed materialization. A post-CAS failure is rolled
back without force when that is provably safe. `AR0116` exposes the result as
typed `context.commit_state` (`rolled_back`, `committed`, or `indeterminate`).
`--no-transpile` skips graph validation
explicitly; it is not the default.

`install` is the native archive's privileged publication boundary. It accepts
exactly the eight version-matched executable files, snapshots them without
following links, preserves an existing `bin` directory's mode, and commits
through one locked crash-recoverable no-clobber transaction. A conflict leaves
the existing suite unchanged; an unprovable durability failure reports
`context.commit_state: "indeterminate"` and retains its recovery journal.

Each declared target is transpiled with a complete checkout-owned MetaMake
context. Prefer `[targets.transpiler]` in `aros-targets.toml`. The compatibility
bridge for older AROS-NX checkouts verifies the same-named CMake preset and its
reviewed defaults; any drift is an `AR0115` failure that requests an explicit
profile or a tools update.

## Toolchains and builds

| Command | Responsibility |
| --- | --- |
| `setup` | Install the declared host compiler, one locked target toolchain, or every locked target toolchain |
| `host-compiler install` | Manage only the host bootstrap compiler |
| `toolchain install|list|verify|path` | Manage exact released target toolchains |
| `build-tools build|check` | Build or verify the Rust helpers consumed by CMake |
| `build` | Configure and build an AROS target preset |
| `clean` | Remove only the selected checkout build directory |
| `test` | Run a bounded QEMU boot and evaluate evidence |
| `golden capture|verify` | Maintain deterministic transpiler baselines |
| `ccache` | Inspect or explicitly clear the selected compiler cache |

Use `--offline` to make network access a hard error. Use
`--require-fetch-checksums` when policy requires every third-party AROS source
archive to declare SHA-256.

## Boards

`board init`, `scan`, `doctor`, `build`, `deploy`, `serve`, `console` and the
`board sd` group are documented in the [physical-board workflow](/aros-tools/workflows/boards/).
Mutating deployment and image creation require `--apply`; raw media writing
requires an exact scan ID and confirmation token.

## Specialized executables

Release archives keep the specialized tools beside `aros`:

- `aros-ahi-runner` validates and executes a closed external AHI build contract;
- `aros-collect` performs deterministic two-pass linking;
- `aros-fetch` downloads, verifies, extracts and patches third-party sources;
- `aros-genmodule` generates module sources and SDK headers;
- `aros-romtool` assembles and inspects ROM/package formats;
- `aros-transpiler` translates MetaMake declarations to transactional CMake;
- `aros-verify` independently compares translated output with `genmf`.

Each has its own `--help`, `--version`, structured diagnostic and opt-in logging
boundary. Keep all executables on the same version.
