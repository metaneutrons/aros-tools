---
title: Architecture
description: Crate ownership and process boundaries.
---

The workspace separates shared contracts from product-specific policy. Every
crate is private to this workspace; the release installs commands, not Rust
library packages.

| Boundary | Owner |
| --- | --- |
| diagnostics, local-log mechanics, hashes, ELF and toolchain schemas | `aros-common` |
| repository orchestration and user-facing commands | `aros-cli` |
| MetaMake translation | `aros-transpiler` |
| independent reference verification | `aros-verify` |
| two-pass linking and set collection | `aros-collect` |
| module, interface and varargs source generation | `aros-genmodule` |
| ROM package parsing, layout validation and publication | `aros-romtool` |
| physical board safety and deployment | `aros-board` |
| narrow macOS removable-disk claim lifetime | `aros-macos-disk-claim` |
| source transport, cache, extraction and patching | `aros-fetch` |
| isolated AHI configure/build/product validation | `aros-ahi-runner` |
| deterministic native archive production and verification | `aros-release` |

The CLI executes build tools as standalone programs. It does not link their
implementations into one process. This preserves explicit contracts and keeps
component failures attributable. The verifier intentionally does not reuse the
transpiler implementation, because a shared defect must not satisfy both sides
of a differential check.

## Process and publication boundaries

`aros` resolves the complete installed tool suite from one directory, then
spawns the required executable through the shared command runner. A missing or
mixed installation fails before work begins. Child diagnostics and exit status
remain attributable to the executable that owns the operation.

New generated trees, ROM images, source checkouts and release artifacts are
staged beside their destination. Validation and recursive durability complete
before one no-clobber rename makes a new tree visible; an existing destination
is never silently replaced. A rename that completed before a directory-sync
error is retained and reported as an uncertain commit, never destructively
"rolled back".

Genmodule's updates to several existing files use a different, explicit
contract: one persistent lock serialises writers, an fsynced intent journal and
identity-plus-SHA-256 proofs provide deterministic crash rollback/recovery, and
the build graph orders readers after the generator. Multiple file renames are
not a live atomic snapshot for an uncoordinated reader. Board media writes also
cannot be atomic, so they use a stricter identity, exclusive-claim and
complete-readback contract instead.

The architecture gate rejects forbidden crate dependencies and direct process
execution that would bypass these boundaries. `aros-common` may be depended on
by the product crates; it contains contracts and mechanisms, but no CLI,
translation, release or board policy.
