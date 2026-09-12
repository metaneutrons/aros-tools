# TCP-M8 local toolchain management evidence

Status: accepted implementation evidence, recorded 2026-09-12.

This ledger closes the local toolchain-management boundary. It qualifies the
CLI's inventory, import, registration, project-selection, lease, removal and
garbage-collection controls; it is not a compiler qualification, a toolchain
release, or authority to remove a real operator-owned installation.

## Exact accepted identities

| Component | Identity | Role |
| --- | --- | --- |
| Integrated `aros-tools` | [`c4a6199782056d10bb5f8c20cb06f1139f989bcc`](https://github.com/metaneutrons/aros-tools/commit/c4a6199782056d10bb5f8c20cb06f1139f989bcc) | Squash merge of the accepted implementation |
| Qualified PR head | [`d9195bc4c103169a233da206f084f4f09b0939e1`](https://github.com/metaneutrons/aros-tools/commit/d9195bc4c103169a233da206f084f4f09b0939e1) | Exact CI input for [PR #134](https://github.com/metaneutrons/aros-tools/pull/134) |
| Source tree | `da326ef2046aa3da6f2499ab881be11545a72fb6` | Identical for the PR head and squash merge |
| Source-aware AROS-NX contract | [`f3cfc243a84065166a46da28b0a5b22bbd0f8869`](https://github.com/metaneutrons/AROS-NX/commit/f3cfc243a84065166a46da28b0a5b22bbd0f8869) | Bound in the Linux x86-64 source-coupled lane |
| State schema | `aros-toolchain-management-v1`, `aros-toolchain-lifecycle-v1` | Frozen by the [management contract](toolchain-management-contract.md) |

The merge is a content-preserving squash: the source-tree identity above was
measured from both commits after merge. Therefore the successful PR checks
exercise the exact integrated implementation, not merely a related branch.

## Native qualification

The [Workspace CI run 34722909249](https://github.com/metaneutrons/aros-tools/actions/runs/34722909249)
completed successfully for the qualified PR head. It passed formatting,
architecture, Clippy, dependency/security checks and the active native matrix.

| Active host | Gate | Result | Scope |
| --- | --- | --- | --- |
| Linux x86-64 | [job 103631935994](https://github.com/metaneutrons/aros-tools/actions/runs/34722909249/job/103631935994) | passed, 6m 51s | Exact AROS-NX source identity, source-coupled Rust tests, complete integration checkpoint and portable workspace suite |
| Linux AArch64 | [job 103631935930](https://github.com/metaneutrons/aros-tools/actions/runs/34722909249/job/103631935930) | passed, 2m 55s | Portable workspace suite, including lifecycle and adversarial fixtures |
| macOS AArch64 | [job 103631935936](https://github.com/metaneutrons/aros-tools/actions/runs/34722909249/job/103631935936) | passed, 5m 15s | Portable workspace suite, including lifecycle and adversarial fixtures |

The [Rust CodeQL run 34722909232](https://github.com/metaneutrons/aros-tools/actions/runs/34722909232)
and [documentation run 34722909384](https://github.com/metaneutrons/aros-tools/actions/runs/34722909384)
also passed for the same tree. Intel macOS remains schema-supported but its
native runner is explicitly suspended under
[aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27).
It is neither represented as a passing result nor silently omitted.

## Safety and user-facing demonstrations

The native runs executed the same portable Rust suite, rather than a separate
management-only substitute. The focused black-box CLI fixture
`owned_import_removal_is_preview_gated_and_checkout_independent` constructs a
temporary owned import outside an AROS checkout, observes the non-mutating
`remove` preview, rejects an invalid confirmation, and proves that cleanup
occurs only after the exact preview token is confirmed. Project-selection
fixtures separately prove that a complete released lock is previewed before an
atomic change and that a local or external prefix cannot become a selection.

The common snapshot-bound removal tests cover a symlink escaping the owned
tree, changed content, a group-writable payload, hard links, a non-sticky
writable ancestor, a permitted sticky temporary ancestor, and a same-name
replacement before unlink. Lifecycle fixtures additionally retain imports
referenced by a project or an external registration and reject cleanup while a
participating build lease is live. These tests operate only in temporary,
owned fixtures; no existing toolchain installation was touched.

A local re-execution on the same source tree passed the seven snapshot-bound
removal fixtures, the black-box owned-import removal fixture, and all eight
project-selection CLI fixtures. This is a focused confirmation of the three
native CI suites, not a replacement for them.

`aros toolchain remove` and `aros toolchain gc` therefore have no force mode.
Both first produce a fixed-layout, no-follow, snapshot-bound plan and require
its exact apply token. A malformed receipt, an unreadable reference, changed
content, unsafe Unix permissions, a live lease, or a durable-journal ambiguity
retains the envelope and reports an error instead of broadening cleanup.

## State compatibility and rollback

M8 adds additive sidecar state below `.aros-management/v1`; it does not mutate
the immutable release lock format or adopt legacy/local/imported prefixes as
released selections. Publication is no-clobber and atomically guarded. A
failed selection preserves the prior valid lock. A cleanup journal is retained
when outcome cannot be proven, and recovery refuses to recurse or guess.

The [management contract](toolchain-management-contract.md#m8-state-formats)
freezes the v1 paths, record owners and lock order. It also defines the future
boundary: selecting non-release/local values would require a versioned
project-lock v2 migration, an old-reader refusal rule, a tested rollback and a
separate acceptance change. It is not an implicit compatibility interpretation
of v1.

## Explicit limits

This acceptance does not run a compiler build, a reproducibility A/B matrix,
a release, a tag, a package publication or a real-store cleanup. Those actions
remain governed by their existing release contracts. On Unix, deletion remains
limited to a private single-user store because POSIX has no inode-bound unlink;
sticky ancestors such as `/tmp` are accepted only within that narrow trust
model. The command rejects less constrained ancestry rather than claiming
multi-writer safety.
