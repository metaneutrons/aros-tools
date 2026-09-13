---
title: Command reference
description: Public aros commands, their boundaries, and the canonical option reference.
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

## Complete public CLI inventory

Every visible frontend leaf command is listed below. The source-derived
[generated CLI contract](/aros-tools/reference/cli-contract/) records its
current options, defaults, values, environment bindings, and parser-level
conflicts. Both references are checked against the built CLI, so exposing a
command or changing a public argument requires an intentional documentation
update. The hidden `__metamake-fetch` lifecycle bridge is deliberately excluded.

### Setup, source and product workflow

| Command | Effect and boundary |
| --- | --- |
| `aros setup` | Install the declared host compiler, or a selected/all cross-toolchains when a preset is supplied |
| `aros host-compiler install` | Install the managed host LLVM compiler |
| `aros build-tools build` | Build the CMake helper suite from the explicitly selected tools workspace |
| `aros build-tools check` | Verify the required CMake helper suite and version agreement |
| `aros source init` | Clone and configure a new AROS checkout atomically |
| `aros source sync` | Validate and fast-forward a clean attached checkout from its reviewed upstream |
| `aros install` | Publish one verified, extracted eight-program native suite atomically |
| `aros build` | Configure the embedded CMake engine and build one target preset |
| `aros clean` | Preview or remove exactly one selected preset, or explicitly the checkout's complete `build/` directory |
| `aros test` | Run the PC x86 QEMU boot checker and retain its evidence |
| `aros ccache` | Inspect or explicitly clear the selected compiler cache |
| `aros golden capture` | Capture a reviewed transpiler-output baseline |
| `aros golden verify` | Compare recorded transpiler output with a baseline, or update it explicitly |
| `aros info` | Report the discovered checkout, host and toolchain state |

### Released-toolchain consumer and local-store controls

| Command | Effect and boundary |
| --- | --- |
| `aros toolchain plan` | Read-only inspection of explicit native-producer inputs; experimental |
| `aros toolchain build` | Build one controlled local native candidate; experimental and local-only |
| `aros toolchain install` | Install the lock-selected host/profile artifact or verify an explicit local prefix |
| `aros toolchain list` | List the lock entries applicable to the current host |
| `aros toolchain inventory` | Read-only bounded scan of store metadata without downloading or executing a toolchain |
| `aros toolchain import` | Preview then import one verified local prefix into the owned managed store |
| `aros toolchain register` | Preview then record a non-owning receipt for one verified external prefix |
| `aros toolchain select` | Preview then atomically select one complete released lock for this checkout |
| `aros toolchain remove` | Preview then remove one exact, unreferenced owned import only |
| `aros toolchain gc` | Preview then reclaim eligible unreferenced owned imports only |
| `aros toolchain verify` | Verify an installed or explicit local cross-toolchain for one preset |
| `aros toolchain path` | Print the verified cross-toolchain prefix for one preset |

### Native producer control plane

These 16 commands are maintainer-only, explicit local stages. They have no
forge authority: none creates a tag, GitHub release, attestation or package
manager publication. Their exact required options come from the installed
`aros toolchain producer <command> --help`; the local candidate workflow uses
the safe entry stages and explains the required checkout/cache separation.

| Command | Effect and boundary |
| --- | --- |
| `aros toolchain producer recipe` | Construct one non-overwriting recipe from committed source, producer and tools inputs |
| `aros toolchain producer cache` | Acquire or verify the lock-selected source-cache closure |
| `aros toolchain producer compatibility-ports` | Acquire or verify the lock-selected upstream ports-source closure |
| `aros toolchain producer environment` | Write a deterministic build-environment receipt |
| `aros toolchain producer profile` | Read one recipe-bound profile without duplicating selectors |
| `aros toolchain producer materialize-engine-free-source` | Materialize an audited compatibility snapshot without the source-tree engine |
| `aros toolchain producer package` | Create one deterministic local package set from a completed candidate |
| `aros toolchain producer verify-package` | Read back and verify one complete local package set |
| `aros toolchain producer compare` | Compare two complete package sets byte-for-byte and write a receipt |
| `aros toolchain producer repackage` | Repackage a retained, evidence-bound package twice under a closed recovery request |
| `aros toolchain producer validate-recovery` | Re-evaluate recovery eligibility against one isolated release inventory |
| `aros toolchain producer record-qualification` | Record complete measured native qualification evidence from an isolated inventory |
| `aros toolchain producer prepare-recovery` | Create a closed recovery request from externally verified qualification facts |
| `aros toolchain producer index` | Advance a complete local release inventory through an explicit index stage |
| `aros toolchain producer compatibility-host-tools` | Print the measured command roles required for native compatibility |
| `aros toolchain producer compatibility` | Execute all six local package-compatibility phases |

### Physical-board workflow

| Command | Effect and boundary |
| --- | --- |
| `aros board init` | Print or explicitly create a typed model-specific profile template |
| `aros board scan` | Find USB CDC-ECM adapters eligible for local profile pairing |
| `aros board doctor` | Inspect a profile, host prerequisites and built artifacts without mutating hardware |
| `aros board build` | Build the selected board profile's CMake target with its locked toolchain |
| `aros board deploy` | Preview or explicitly stage one verified boot bundle into a local TFTP root |
| `aros board serve` | Run restricted DHCP and read-only TFTP, or inspect the plan without sockets |
| `aros board sd image` | Validate a pinned boot bundle and explicitly create a raw removable-media image |
| `aros board sd scan` | List safe removable disks and optionally generate artifact-bound write tokens |
| `aros board sd unmount` | Preview or explicitly unmount one opaque scanned disk identity |
| `aros board sd write` | Preview or explicitly write a verified image with an exact opaque token |
| `aros board console` | Launch or preview an external serial terminal; no UART driver is embedded |

## Source and repository

| Command | Checkout | Behavior |
| --- | --- | --- |
| `source init PATH` | No | Clone into a new destination; `--upstream URL`, `--fork URL`, optional `--ref REF` |
| `source sync` | Required | Validate a candidate and fast-forward a clean attached branch; `--upstream URL`, `--branch BRANCH`, `--no-transpile` |
| `info` | Optional | Report host/state paths and any discovered target/toolchain contracts |
| `install --source-bin DIR --prefix DIR` | No | Publish exactly eight pre-verified executable files without replacing existing programs |

`source init --ref` requires a full branch/tag ref or exact commit OID and
leaves HEAD detached, even for a branch ref. Omit it to use the clone's default
branch. `source sync --branch` takes a branch name **without** `refs/heads/`
and defaults to `master`. Both default to canonical upstream AROS unless
explicitly changed. Only sync reads `AROS_UPSTREAM_URL`.

Sync requires clean recursive submodules and checks ignored files as well.
It never implicitly merges divergent history.
See [source workflows](/aros-tools/workflows/source/).

Every user-supplied relative filesystem path is interpreted from the directory
where `aros` was invoked. Checkout-relative defaults remain checkout-relative;
producer recipe members and board-config members keep the origins documented by
their owning contracts. `aros` never changes its process working directory
during repository discovery.

## Explicit cleanup

`aros clean` requires one scope: `--preset NAME` selects that preset's build
directory, while `--all` selects only the checkout's `build/` directory. They
are mutually exclusive. Add `--dry-run` to print the exact directory without
removing anything. Cleanup never includes archives, logs outside that build
tree, or installed toolchains.

The native installer requires an existing absolute prefix and an input
directory containing exactly the eight expected regular executable files. It
checks their inventory, modes, sizes, and snapshotted bytes before publishing;
release identity, version matching, and provenance verification are upstream
release-preparation responsibilities. A Cargo output directory contains
additional files and is **not** an `install --source-bin` input. Use PATH for a
source build or the verified archive installation procedure.

## Toolchains and helpers

Consumer toolchain/host-compiler commands require an AROS checkout, except
store inventory and explicit local management (`import`, `register`, `remove`, `gc`). The experimental native producer instead
requires three explicit source roots and works from any directory.

| Command | Inputs and effect |
| --- | --- |
| `setup` | No preset: install the managed host compiler; `--preset NAME`: install that target; `--all`: attempt every configured target |
| `host-compiler install` | Managed host LLVM installation; supports `--force`, `--offline` |
| `toolchain install` | Requires `--preset NAME`; supports `--force`, `--offline`, `--local DIR` |
| `toolchain list` | Show lock entries for the current host |
| `toolchain inventory` | Read-only metadata scan of the installed store; checkout optional; supports absolute `--store DIR`, bounded `--max-entries N`, and `--format human\|json` |
| `toolchain import` | Checkout optional; preview then token-confirmed bounded, no-follow import of a manifest-verified local prefix into a no-clobber managed envelope; supports absolute `--source DIR`, optional `--store DIR`, `--apply TOKEN`, and `--format human\|json` |
| `toolchain register` | Checkout optional; preview then token-confirmed bounded validation and non-owning receipt for an external local prefix; supports absolute `--source DIR`, optional `--store DIR`, `--apply TOKEN`, and `--format human\|json` |
| `toolchain select` | Requires an AROS checkout; preview then token-confirmed atomic selection of one complete TOML v1 release lock plus a derived non-authoritative project-reference receipt; supports absolute `--release-lock FILE`, optional `--store DIR`, `--apply TOKEN`, and `--format human\|json` |
| `toolchain remove` | Checkout optional; preview then token-confirmed removal of one exact, owned managed import only from a private single-user store; requires `--managed-id SHA256`, optional `--store DIR`, `--apply TOKEN`, and `--format human\|json` |
| `toolchain gc` | Checkout optional; preview then token-confirmed reclamation of eligible owned imports only from a private single-user store; supports optional `--store DIR`, `--apply TOKEN`, and `--format human\|json` |
| `toolchain verify` | Requires `--preset NAME`; optionally verify `--local DIR` |
| `toolchain path` | Requires `--preset NAME`; print the verified prefix; optionally `--local DIR` |
| `toolchain plan` | Experimental read-only producer inspection; explicit roots and recipe, no checkout discovery or build |
| `toolchain build` | Experimental controlled local native candidate; explicit roots, prepared offline cache, fresh isolated snapshots and bounded cancellation |
| `toolchain producer` | Low-level native producer operations for exact recipe, cache, package, comparison, compatibility and recovery inputs; maintainer-only, never a publication shortcut |
| `build-tools build` | Build helpers from the explicitly selected tools source workspace; checkout optional |
| `build-tools check` | Probe the six mandatory CMake helpers and their versions; checkout optional |

`setup` also accepts `--force` and `--offline`. Its `--local DIR`
requires `--preset` and conflicts with `--all`.
`--force` refreshes an archive cache, not an installed tree.

For helper source builds set `AROS_TOOLS_SOURCE_DIR` to the tools checkout.
Installed suites normally need only `build-tools check`.
See [toolchain workflows](/aros-tools/workflows/toolchains/).

### Experimental producer inspection and local candidate build

```sh
aros toolchain plan --preset pc-x86_64 \
  --recipe /work/recipe.json --source-dir /work/AROS \
  --producer-dir /work/aros-toolchains --tools-dir /work/collector-tools \
  --format json --offline
```

All five selections (`preset`, `recipe`, `source-dir`, `producer-dir`,
`tools-dir`) are mandatory. The roots must match the recipe's exact Git
commits/trees. `tools-dir` selects the recipe's collector source, not necessarily
the current frontend's source. A trusted Git supporting `--no-lazy-fetch` and
locally prepared Git objects are required; inspection never fetches them.
Use regular recipe/input files without symlink ancestors.

Optional `--work-dir`, `--output-dir`, `--cache-dir`, positive `--jobs` and
positive `--timeout-seconds` describe a native build; omitted values stay null.
No directories are created, no locks reserved and no cache contents scanned.
`--offline` also honors `AROS_OFFLINE`; planning itself is always offline.
`--format human|json` controls stdout independently of `--diagnostic-format`.
As with other commands, explicit `--log-file` can write the selected log;
keep that optional destination outside source roots.

Inspection checks recipe self-consistency, selected committed profile/lock/patch
identities, root overlap and the raw worktree/index of all three checkouts and
their recursive submodules, **not build readiness**. Dirty, ignored, untracked,
missing, wrong-mode or uninitialized material is rejected. Even empty
untracked directories count; keep build/cache directories outside the roots.
Raw symlink targets are compared without following them. Git filters and index
flags cannot hide changes, and inspection never cleans the checkouts.
The native lifecycle requires a committed producer declaration at
`toolchains/producer-executor-v1.toml`. It binds the selected contract, tools
commit, source lock and profile matrix to the recipe. Without that exact
declaration, native inspection returns AX0202 and never guesses historical
producer state. With it, `readiness` is `ready` only when all build roots and
positive resource budgets are present and `--offline` is set; cache verification and root reservation
still happen at build time. Every local result has `qualification: local-only`:
there is no origin attestation or release authorization. `fetch-guard`
describes the controlled no-network policy, not a proven OS sandbox. Exit 0 means inspection
completed; inspect `readiness` and `findings`. Invalid inputs exit 1 with no
result on stdout.

The native lifecycle is explicit about all material it controls:

```sh
aros toolchain build --preset pc-x86_64 \
  --recipe /work/recipe.json --source-dir /work/AROS \
  --producer-dir /work/aros-toolchains --tools-dir /work/aros-tools \
  --work-dir /work/toolchain-run --output-dir /work/toolchain-candidate \
  --cache-dir /work/source-cache --jobs 8 --timeout-seconds 21600 \
  --release-id local-pc-2026-09-06 --offline --format json
```

`toolchain build` rechecks every selection, verifies the prepared cache, and
reserves fresh non-overlapping work/output leaves. It snapshots committed source,
producer and tools material, prepares private locked Python/Cargo environments,
then runs unchanged AROS `configure` and source-owned
`crosstools-release`. A hidden Rust MetaMake bridge supplies only declared cache
payloads and records exact source use before calling the upstream helper. The
same lifecycle builds the vendored `aros-collect`, installs the collector aliases
and writes a canonical receipt after each completed phase. The child environment
has explicit PATH, HOME, TMPDIR, locale, timezone, Git and Cargo offline
settings. Ctrl-C and the whole-operation deadline terminate and reap process
groups; retained material is never adopted or deleted. The command requires
`--offline`; it does not publish, tag, package or authorize a release.

There is no backend switch or legacy fallback. The sole recovery boundary is
`--resume-from compiler`: it revalidates retained ownership, predecessor
receipts, snapshots, cache inputs, and measured compiler outputs before running
only the collector phase. Generic receipt reuse, candidate inventory, and real
host/profile qualification remain separately qualified capabilities.

For the required checkout layout, cache bootstrap, resource boundary and
failure handling, follow the [native producer workflow](/aros-tools/workflows/toolchain-producer/).
It is intentionally separate from the released-toolchain consumer guide.

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

`--profile NAME` selects a local profile; it is not a hardware-model argument.
`board init` additionally requires `--model rpi3|rpi4|rpi5|milk-v-titan` and
accepts an optional reviewed `--transport`. Commands using existing profiles
also accept `--config PATH`.

| Command | Checkout | Behavior |
| --- | --- | --- |
| `board init --profile NAME --model MODEL` | No | Print a model-specific template; `--transport` selects a reviewed non-default transport and `--apply` creates a new config file |
| `board scan` | No | Discover USB CDC-ECM adapters |
| `board doctor --profile NAME` | Required | Inspect profile, host prerequisites and artifacts |
| `board build --profile NAME` | Required | Build the profile's target with its toolchain |
| `board deploy --profile NAME` | Required | Preview TFTP staging; `--apply` publishes; optional `--artifact-dir DIR` |
| `board serve --profile NAME` | No | Serve restricted DHCP/TFTP; `--dry-run` inspects without opening sockets |
| `board console --profile NAME` | No | Launch external serial terminal; `--program`, `--device`, `--baud`, `--dry-run` |

`board build` shares build options except `--preset`, which comes from the
profile. It additionally accepts `--dtb-path PATH` and `--core-kobj-dir DIR`;
these overrides apply to Raspberry Pi profiles. There are no CLI commands
for automated JTAG/SWD sessions or power control.

### Removable media

| Command | Behavior |
| --- | --- |
| `board sd image` | Requires `--profile`, `--boot-bundle DIR`, `--output DIR`; validates first, creates only with `--apply` |
| `board sd scan` | List safe unmounted removable disks; `--artifact DIR` also produces write tokens |
| `board sd unmount` | List/preview mounted candidates; `--device SCAN_ID --apply` unmounts one |
| `board sd write` | Requires `--profile`, `--artifact DIR`, `--device SCAN_ID`; writes only with exact `--confirm TOKEN` |

All four media commands work without an AROS checkout.
`image`, `unmount` and `write` support `--dry-run`.
Raw device paths are rejected where an opaque scan ID is required.
See [physical boards](/aros-tools/workflows/boards/) for preparation and limits.

## Specialized executables

The seven companion programs have separate interfaces. Their inputs, supported
formats and important limits are in
[standalone tools](/aros-tools/reference/standalone-tools/).
