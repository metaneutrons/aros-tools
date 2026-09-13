---
title: Inspect cache state
description: Inspect, verify, and deliberately populate reviewed AROS cache inputs without granting cleanup authority.
---

`aros cache` is the resource-oriented cache interface. Passive status commands
do not create a directory, acquire a lock, hash a tree, access a network, start
a compiler-cache daemon, or change a backend. Source-cache `fetch` is the
separate, explicit population boundary.

```sh
aros cache status
aros cache status --format json

aros cache compiler status
aros cache compiler status --backend ccache --format json
aros cache compiler status --backend sccache --dir /work/aros-compiler-cache

aros cache sources status --dir /work/aros-source-cache
aros cache sources list --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache --format json
aros cache sources fetch --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources verify --compatibility-ports-lock toolchains/ports.json \
  --dir /work/aros-source-cache --format json
```

Use the JSON documents for scripts. `--format json` changes normal stdout only;
use the global `--diagnostic-format json` option if an error must be parsed.

## What the status reports

`aros cache status` reports five cache families without scanning their
contents:

| Family | Current status boundary |
| --- | --- |
| `compiler` | Discovers `sccache` and `ccache` on `PATH`, then records recognized configuration-variable names without reading their values or starting either backend. |
| `archives` | Reports the existing state of the shared host/cross-compiler archive root only. Installed host compilers and cross-toolchains are outside this cache family. |
| `sources` | `status` never guesses a root. `list`, `fetch`, and `verify` require an explicit reviewed selector and root. |
| `cargo` | Requires a future explicit tools checkout and managed vendor root. Your global Cargo home is excluded. |
| `genmf` | Requires a future explicit expansion root and source selection. Verification reports and build trees are excluded. |

Archive-root resolution is deterministic: `AROS_CACHE_DIR` wins when set;
otherwise AROS uses `AROS_HOME/cache`, and `AROS_HOME` defaults to
`$HOME/.aros`. Every configured root must be absolute. Status does not follow a
root symlink: it reports `symlink` instead, so a later lifecycle operation can
make an explicit safety decision.

## Compiler backend observations

`aros cache compiler status --backend auto` uses the stable selection order:
`sccache`, then `ccache`. `--backend sccache` and `--backend ccache` project
one backend without silently substituting the other. An unavailable backend is
a successful status result with `selected_backend: null`; it is not a build
fallback.

The report intentionally labels backend storage as `unconfigured` or
`configuration_uninspected`. Reading a backend's effective settings may require
processing a configuration file or talking to a server, and sccache statistics
can start that server. This passive command therefore reports only the names of
recognized environment variables, never their potentially sensitive values, and
does not claim that a configured backend is local, remote, or mixed.

`--dir DIR` requires an absolute path and observes that root without following
a symlink, creating it, or assigning it to sccache or ccache. Without `--dir`,
the selected backend's future AROS-owned candidate is shown below
`AROS_HOME/cache/compiler/v1/<backend>`. Both are status-only candidates, not
evidence that the running backend currently uses that location. JSON reports
the same boundary through `root_binding: "status_only_not_applied"`, an
explicit `selected_backend` value (or `null`), `selection_basis`,
`effective_build_selection`, and a complete `side_effects` object. This makes
the limits safe for automation to check rather than infer from prose.

## Reviewed source-cache operations

Source-cache commands always require an explicit absolute `--dir` that already
exists and has no symlink components. A selector is exactly one of:

- `--source-lock FILE` for `aros-toolchain-source-lock-v2` producer inputs;
- `--compatibility-ports-lock FILE` for
  `aros-toolchain-compatibility-ports-v2` inputs; or
- `--source-fetch-plan FILE` for one product
  `aros-cache-source-fetch-plan-v1` closure.

Selectors are bounded regular files read without following their final
symlink. The command records the SHA-256 of the exact selector bytes as
`request_sha256`; filenames and checkout state never select a closure
implicitly.

`status` observes root metadata only. `list` reports direct-child metadata
states (`missing`, `present_unverified`, `unsafe`, or `inaccessible`) and never
hashes a payload. `verify` takes private no-follow snapshots, measures every
object, and requires exact size/SHA-256 agreement for strict declarations.
`fetch` verifies every already-present object in the selected closure before
any transport; a missing object may then be acquired from its reviewed HTTPS
candidates in declaration order. It uses a per-object no-clobber guard and
private staging, so cancellation or a competing writer cannot expose a partial
file. It never refreshes, replaces, deletes or repairs an existing cache
object. `--offline` forbids every transfer and turns a selected miss into a
diagnostic.

### Product source-fetch plans

A product plan has stable entry roles, ordered credential-free HTTPS candidates
that all select one direct cache filename, an explicit `archive` or `patch`
representation, a normalization policy, and an integrity declaration. `locked`
contains exact `sha256` and `size`. An
`unverified` entry must additionally set a finite `max_size`, may use only
`exact-bytes-v1`, and requires the deliberate opt-in below:

```json
{
  "schema": "aros-cache-source-fetch-plan-v1",
  "entries": [
    {
      "role": "product:grub@2.12",
      "filename": "grub-2.12.tar.xz",
      "candidates": [
        {
          "url": "https://mirror.example.invalid/grub-2.12.tar.xz"
        }
      ],
      "representation": "archive",
      "normalization": "exact-bytes-v1",
      "integrity": {
        "kind": "locked",
        "sha256": "8e7e7c3d2f3a8ecdd1224c7984436b074bfe04c6e9a35a10c129cd4ec5e734ef",
        "size": 123456
      }
    }
  ]
}
```

This is an input schema, not a source recipe: it does not execute a script,
discover additional files, extract an archive, or apply a patch. A direct
patch uses `"representation": "patch"`, a required `patch` object with a
relative optional `subdirectory` and reviewed `options` (`-p0` through `-p9`,
`-f`, `-N`, or `--forward`), and remains `exact-bytes-v1`. Patch application
stays with the consumer that owns its target tree; the metadata remains part of
the measured request identity.

```sh
aros cache sources fetch --source-fetch-plan grub.fetch-plan.json \
  --dir /work/aros-source-cache --allow-unverified --format json
```

The result labels such an object's locally measured digest and size as
`measured_unpinned`. It is not an upstream checksum, is never converted into a
hidden pin, and cannot satisfy a producer or compatibility lock. Prefer a
strict product plan whenever the upstream offers a stable content identity.

### Producer migration

The historical `aros toolchain producer cache` and `aros toolchain producer
compatibility-ports` frontends are removed. Bootstrap and prove native inputs
with the common commands instead:

```sh
aros cache sources fetch --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources verify --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources fetch --compatibility-ports-lock toolchains/ports.json \
  --dir /work/aros-source-cache
```

The native producer and compatibility executor independently reverify the
same typed request before consuming its cache objects. A pinned workflow must
replace every removed producer-cache invocation before upgrading aros-tools.

## Current limits

The following operations are not yet public cache commands: archive/Cargo
population and verification, GenMF refresh, retention, removal, prune,
compiler statistics reset, and compiler cache clearing. Do not replace them
with ad-hoc directory deletion. Their interfaces require verified ownership,
cooperating reader/writer leases, preview/apply protection, and explicit scope
proof; they are delivered in the tracked cache milestones.

`aros ccache` remains the legacy statistics frontend during the transition. It
may start sccache because it queries backend statistics. Its former `--clear`
flag is intentionally rejected at parser level: the command had neither a
shared ownership boundary nor preview/apply protection, and `sccache -z`
resets counters rather than deleting entries. Managed clearing, reset and
retention arrive only with their dedicated lifecycle contract.

## Build launcher policy

`aros build` and `aros board build` share
`--compiler-cache auto|off|sccache|ccache`. The frontend resolves the exact
absolute executable once and passes that selection to CMake for C and C++;
CMake never independently chooses a backend. CMake does not expose a supported
language-specific compiler launcher for ASM, so assembly stays a direct
deterministic invocation. `off` also removes stale launcher settings from an
existing CMake build tree on the next configure.

In an offline build, `auto` selects `off` without probing a backend. Explicit
`sccache` and `ccache` fail because a passive command cannot prove that their
effective configuration is local-only: configuration files may select remote,
multi-level or shared storage. This is deliberate, not a fallback defect. Use
`--compiler-cache off` for an offline build until the managed local namespace
and server-isolation lifecycle is available.

The current build option selects a launcher only. It does not set `CCACHE_DIR`,
`SCCACHE_DIR`, a daemon endpoint or a compiler-cache directory. A `--dir` value
on `cache compiler status` remains a status-only observation; a build-level
directory option will be introduced only with owned-root and lifecycle proof.

For every other state path and precedence rule, see
[configuration](/aros-tools/reference/configuration/). For a toolchain install
or an offline build, use the current documented
[toolchain workflow](/aros-tools/workflows/toolchains/) rather than treating a
status result as an integrity or installation verification.
