---
title: Command reference
description: All 29 frontend command paths, their inputs, defaults, and effects.
---

The executable is `aros`. The tables below cover the current
[command model](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/main.rs)
and [handlers](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/commands.rs).
Use `aros <command> --help` for the option list of your installed version.

**Checkout required** means run from within the intended AROS source tree.
Discovery searches upward; it does not select a neighboring repository.

## Global options

| Option | Values / behavior |
| --- | --- |
| `--diagnostic-format` | `human` (default) or `json`; errors go to stderr |
| `--log-level` | `off` (default), `error`, `warn`, `info`, `debug`, `trace` |
| `--log-format` | `human` (default) or `jsonl` |
| `--log-file PATH` | Explicit local log destination |

A log level requires a file. In the frontend, supplying a file with an
effective level of `off` enables `info`.
[Environment variables](/aros-tools/reference/configuration/#environment-variables)
and [component logging differences](/aros-tools/reference/diagnostics/) are
documented separately.

## Source and repository

| Command | Checkout | Behavior |
| --- | --- | --- |
| `source init PATH` | No | Clone into a new destination; `--upstream URL`, `--fork URL`, optional `--ref REF` |
| `source sync` | Required | Validate a candidate and fast-forward a clean attached branch; `--upstream URL`, `--ref BRANCH`, `--no-transpile` |
| `info` | Optional | Report host/state paths and any discovered target/toolchain contracts |
| `install --source-bin DIR --prefix DIR` | No | Publish exactly eight version-matched executables without replacing existing programs |

`source init --ref` requires a full branch/tag ref or exact commit OID and
leaves HEAD detached, even for a branch ref. Omit it to use the clone's default
branch. `source sync --ref` takes a branch name **without** `refs/heads/`
and defaults to `master`. Both default to canonical upstream AROS unless
explicitly changed. Only sync reads `AROS_UPSTREAM_URL`.

Sync requires clean recursive submodules and checks ignored files as well.
It never implicitly merges divergent history.
See [source workflows](/aros-tools/workflows/source/).

The native installer requires an existing absolute prefix and an input
directory containing exactly the eight programs. A Cargo output directory
contains additional files and is **not** an `install --source-bin` input.
Use PATH for a source build or the verified archive installation procedure.

## Toolchains and helpers

All toolchain/host-compiler commands require an AROS checkout.

| Command | Inputs and effect |
| --- | --- |
| `setup` | No preset: install the managed host compiler; `--preset NAME`: install that target; `--all`: attempt every configured target |
| `host-compiler install` | Managed host LLVM installation; supports `--force`, `--offline` |
| `toolchain install` | Requires `--preset NAME`; supports `--force`, `--offline`, `--local DIR` |
| `toolchain list` | Show lock entries for the current host |
| `toolchain verify` | Requires `--preset NAME`; optionally verify `--local DIR` |
| `toolchain path` | Requires `--preset NAME`; print the verified prefix; optionally `--local DIR` |
| `build-tools build` | Build helpers from the explicitly selected tools source workspace; checkout optional |
| `build-tools check` | Probe the six mandatory CMake helpers and their versions; checkout optional |

`setup` also accepts `--force` and `--offline`. Its `--local DIR`
requires `--preset` and conflicts with `--all`.
`--force` refreshes an archive cache, not an installed tree.

For helper source builds set `AROS_TOOLS_SOURCE_DIR` to the tools checkout.
Installed suites normally need only `build-tools check`.
See [toolchain workflows](/aros-tools/workflows/toolchains/).

## Build and inspect a product

| Command | Checkout | Behavior |
| --- | --- | --- |
| `build` | Required | Configure the embedded CMake engine and build with Ninja |
| `clean` | Required | Remove `build/<preset>` with `--preset`; otherwise remove all of `build/` |
| `test` | Required | Run the PC x86 QEMU boot checker against the selected build directory |
| `ccache` | No | Show statistics for discovered sccache/ccache; `--clear` clears that cache |
| `golden capture` | Required | Run recorded transpiler invocations twice and capture baselines |
| `golden verify` | Required | Compare with baselines; `--update` replaces them |

`build` options:

| Option | Meaning |
| --- | --- |
| `--preset NAME`, `-p` | Target/build directory; default `pc-x86_64` |
| `--target NAME`, `-t` | One CMake target instead of the default build |
| `--jobs N`, `-j` | Positive parallel job count |
| `--clean` | Delete this preset's build directory before configuring |
| `--verbose`, `-v` | Verbose CMake configure messages |
| `--debug` | Unoptimized build with debug information; default is Release |
| `--offline` | Require local toolchain/source inputs |
| `--require-fetch-checksums` | Require source-authored SHA-256 coverage for fetched inputs |
| `--toolchain-dir DIR` | Explicit local AROS cross-toolchain |
| `--engine-dir DIR` | Explicit development override for the embedded CMake engine |

`test` defaults to `--preset pc-x86_64 --timeout 20 --memory 512`.
`--packages` adds built packages; repeat `--module FILE` for explicit modules;
`--evidence DIR` selects the root for a new private evidence directory.
The implementation runs `qemu-system-x86_64` and expects PC bootstrap/kernel
paths. A different preset does not select an ARM or RISC-V emulator.

Golden commands take repeatable `--preset NAME` options. Run them from the
AROS repository root after configuring the selected builds; they consume
recorded transpiler invocations under `build/`.

:::caution[Build cleanup removes evidence too]
`clean` and `build --clean` delete the selected build directory without an
interactive confirmation. Preserve logs, SDK outputs, packages and boot evidence
you need first. `ccache --clear` affects the selected compiler cache, not just
one preset.
:::

## Boards

`--board NAME` selects a local profile; it is not a hardware-model argument.
Commands using profiles also accept `--config PATH`.

| Command | Checkout | Behavior |
| --- | --- | --- |
| `board init --board NAME` | No | Print the Pi-4 USB-ECM template; `--apply` creates a new config file |
| `board scan` | No | Discover USB CDC-ECM adapters |
| `board doctor --board NAME` | Required | Inspect profile, host prerequisites and artifacts |
| `board build --board NAME` | Required | Build the profile's target with its toolchain |
| `board deploy --board NAME` | Required | Preview TFTP staging; `--apply` publishes; optional `--artifact-dir DIR` |
| `board serve --board NAME` | No | Serve restricted DHCP/TFTP; `--dry-run` inspects without opening sockets |
| `board console --board NAME` | No | Launch external serial terminal; `--program`, `--device`, `--baud`, `--dry-run` |

`board build` shares build options except `--preset`, which comes from the
profile. It additionally accepts `--dtb-path PATH` and `--core-kobj-dir DIR`;
these overrides apply to Raspberry Pi profiles. There are no CLI commands
for automated JTAG/SWD sessions or power control.

### Removable media

| Command | Behavior |
| --- | --- |
| `board sd image` | Requires `--board`, `--boot-bundle DIR`, `--output DIR`; validates first, creates only with `--apply` |
| `board sd scan` | List safe unmounted removable disks; `--artifact DIR` also produces write tokens |
| `board sd unmount` | List/preview mounted candidates; `--device SCAN_ID --apply` unmounts one |
| `board sd write` | Requires `--board`, `--artifact DIR`, `--device SCAN_ID`; writes only with exact `--confirm TOKEN` |

All four media commands work without an AROS checkout.
`image`, `unmount` and `write` support `--dry-run`.
Raw device paths are rejected where an opaque scan ID is required.
See [physical boards](/aros-tools/workflows/boards/) for preparation and limits.

## Specialized executables

The seven companion programs have separate interfaces. Their inputs, supported
formats and important limits are in
[standalone tools](/aros-tools/reference/standalone-tools/).
