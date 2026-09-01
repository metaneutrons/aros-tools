---
title: Pristine upstream AROS
description: What works without carrying AROS-NX patches, and where the compatibility boundary is today.
---

`aros-tools` treats the selected AROS checkout as an explicit input. It does
not infer a sibling directory, require the repository to be named `AROS-NX`,
or expect the Rust workspace inside the operating-system tree.

Create a pristine checkout from any directory:

```sh
aros source init ./AROS
cd ./AROS
```

The default canonical source is
`https://github.com/aros-development-team/AROS.git`. Use `--upstream` to select
another canonical source, or combine `--fork` and `--upstream` so `origin`
points at a contributor fork while the reviewed upstream identity remains
explicit. Initialization uses a sibling staging directory and publishes the
destination only after clone, remote, submodule, repository-layout and
clean-tree validation succeed.
Nested submodules are initialized one level at a time so every child
`.gitmodules` file and URL is validated before Git contacts the next level.
HTTPS and SSH are non-interactive, arbitrary Git remote-helper schemes and
embedded web credentials are rejected, and explicit relative local sources are
canonicalized before staging.

The independent host tools can be built and exercised without modifying that
checkout:

```sh
cargo test --locked -p aros-collect -p aros-fetch -p aros-genmodule
```

Repository discovery and installed-tool resolution do not require AROS-NX.
Individual parsers, the collector, fetcher and generators model upstream
source contracts directly. The complete workspace gate, however, currently
uses the immutable AROS-NX checkout named by CI because its translation and
verification tests require the consumer bridge and qualified denominators.
The classic MetaMake/GNU Make build stays available in upstream AROS and is not
replaced by a shell fallback.

`aros source sync` is also safe for an upstream checkout. It requires a clean
attached branch, validates the configured upstream remote, fetches the selected
`refs/heads/` branch into a run-owned ref, and resolves one exact OID. It rejects
divergence and tests that OID in a standalone repository with an independent
object database before compare-and-swap publication. The external fetch first
lands in a temporary quarantine and never overwrites the checkout's
`FETCH_HEAD`. Recursive submodule commits are copied from that validated
candidate before CAS; post-CAS submodule checkout uses only candidate-local
URLs with network protocols disabled.

A persistent kernel-backed repository lock serializes tool-driven syncs; its
identity and the captured branch, index, worktree, submodules and local Git
semantics are rechecked so a concurrent change is refused rather than reset.
The lock's owner-record file remains after the process exits and must not be
deleted. For a pristine tree without the AROS-NX
CMake bridge, select `--no-transpile` explicitly; the omitted validation is
reported as a deliberate skip.

:::caution[Current build frontend boundary]
The integrated `aros build` CMake path still requires the small consumer bridge
carried by AROS-NX. A native GNU Make backend for an entirely pristine upstream
checkout is not yet a completed release claim. Until its acceptance tests are
green, use upstream's documented configure/MetaMake path for full products and
use `aros-tools` for the independently supported component workflows.
:::
