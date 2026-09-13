---
title: Inspect cache state
description: Safely inspect the cache roots and compiler-cache backends that AROS tools can currently observe.
---

`aros cache` is the resource-oriented cache interface. Its initial commands are
strictly observational: they do not create a directory, acquire a lock, hash a
tree, access the network, start a compiler-cache daemon, or change a backend.

```sh
aros cache status
aros cache status --format json

aros cache compiler status
aros cache compiler status --backend ccache --format json
aros cache compiler status --backend sccache --dir /work/aros-compiler-cache
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
| `sources` | Requires a future explicit, reviewed lock and root. The command does not guess a producer or product source cache. |
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

## Current limits

The following operations are not yet public cache commands: source/archive/
Cargo population and verification, GenMF refresh, retention, removal, prune,
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

For every other state path and precedence rule, see
[configuration](/aros-tools/reference/configuration/). For a toolchain install
or an offline build, use the current documented
[toolchain workflow](/aros-tools/workflows/toolchains/) rather than treating a
status result as an integrity or installation verification.
