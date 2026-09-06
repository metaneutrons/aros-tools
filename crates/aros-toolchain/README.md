# aros-toolchain

Experimental TCP-M1 producer inspection behind `aros toolchain plan`. It is
not a compiler driver or a released management feature.

Implemented:

- closed recipe-v2 parsing with duplicate/unknown-field rejection, canonical
  lowercase identities, strict integer fields and safe, sorted patch paths;
- bounded recipe input and canonical UTF-8 JSON self-digest verification;
- the shared envelope with AX0101 (contract), AX0102 (identity), AX0201 (Git
  prerequisites), AX0202 (roots/resources) and AX0801 (work ownership/state),
  without input-document dumps;
- maintained native conformance/counter-probes against the independent M0
  encoding vectors. No second SHA-256 implementation or source-lock copy.

Recipe parsing performs no I/O. Planning inspects three explicit Git
roots/commit/tree pairs, raw committed profile/lock/patch identities and
proposed disjoint roots. It also checks the complete raw worktree and index of
each root and every initialized recursive gitlink against the selected trees.
This includes ignored/untracked files and empty directories, missing files,
executable bits, staged conflicts/changes, raw symlink targets and binary blobs.
Git index stat caches, `assume-unchanged`, `skip-worktree`, ignore rules and clean
filters cannot waive these comparisons. Selected metadata must be regular;
other committed symlinks are compared as link bytes, never followed.
Descriptor-relative reads reject special files and symlink directories;
bounded trusted Git queries disable filters, fsmonitor, hooks, lazy
fetching and inherited Git/credential settings. No build, source script,
download, cache scan, directory reservation, installation or cleanup runs.

Plans currently have `readiness: blocked`, even with complete resource options.
Isolated execution snapshots, source capabilities, lock semantics, prerequisites/cache,
executor origin, ownership and cancellation remain unqualified. A blocked
inspection exits 0; invalid input exits 1 without a result. Neither a plan nor
a valid self-digest grants build permission. The recursive read-only check
does not freeze a concurrently mutable tree, create an isolated snapshot,
re-hash Git's object database independently, or establish trusted origin.
It compares against the raw material returned by the selected local Git
database. Git metadata is excluded only at each verified repository root;
linked worktree/submodule metadata may be external. This is not a sandbox.
Boundary metadata/name checks catch observed races, not every possible change
by a hostile same-user process. Execution still requires isolated, revalidated
material and trusted executor evidence; no caller can reuse an audit as a receipt.
AX0102 names the selected root and, for committed-file mismatches, the validated
root-relative path. Undeclared names, private file bytes and Git stderr are not
echoed. No ignored files are removed or automatically exempted: use separately
prepared clean checkouts with Cargo targets and transport caches outside them.

The measured frontend file digest is not an attestation. An unproven frontend
commit is null in blocked plans, **not** the recipe's collector commit.
Execution/results/receipts may not adopt this plan-only exception.
Git queries use 10-second group limits within a 60-second Git budget;
documents/individual selected metadata blobs are capped at 1 MiB,
profiles/toolchains directory entries at 128, frontend hashing at 512 MiB.
Recursive inspection shares the 60-second budget across all three roots and
submodules: at most 200,000 entries, 8 GiB of declared blob bytes, 64 levels,
4,096-byte UTF-8 relative paths, 64 MiB per blob and 32 MiB per tree/index
listing. Raw `cat-file --batch` reads are grouped (normally 8 MiB, at most
4,096 blobs); one larger allowed blob gets its own bounded batch. NUL-framed
inventories and exact ID/type/size headers are validated, never truncated.
These are inspection bounds, not measured compiler resource defaults.

## Work ownership primitive, not a build command

`workspace::RunDirectories` is a lower-level M1 building block, not called by
`plan`. It resolves the original selections afresh and exclusively creates only
fresh work/output leaves under existing parents. Sources and cache are untouched.
Directory locks remain held by the owning process; private ownership markers,
directory identities, parent bindings and marker bytes are rechecked at each
explicit `revalidate` boundary. Existing roots, even empty ones, are never
adopted. Descriptor-relative no-follow traversal is shared with inspection.

Reservation is not atomic across the two roots. A failure can retain a partially
created reservation; AX0801 says to inspect the selected paths. After all child
activity has ended, consume the guard with `release()` to explicitly unlock
both original directory descriptions and report any AX0801 failure. Cancellation
or namespace changes do not suppress unlocking. Drop performs fallback unlocking
and logs failures through the existing tracing subscriber, but cannot return a
successful cleanup result; it never deletes data. Explicit unlocking prevents
an inherited/duplicated descriptor from extending the owner's lock lifetime
past drop; `CLOEXEC` alone only closes such references at exec, not at fork.
Only the creating process may unlock; an inherited non-owner guard closes its
own descriptor without releasing the parent's still-active lock.
The lock guard exists before fallible marker construction, so partial setup
failures receive the same fallback cleanup. There is no implicit resume,
stale-state adoption, automatic cleanup, phase receipt or candidate publication. The caller-provided
owner digest binds its chosen operation, not trusted executor origin. Locks and
boundary checks do not sandbox a hostile process with the same user's access.

The shared process runner now provides a one-way `CancellationToken` and bounded
controlled execution with distinct cancellation/timeout/status outcomes. The
input-capable variant uses the same runner, including bounded blocked-writer
cleanup. A stdin pipe closed by cancellation/timeout preserves that outcome
and captured output; normal exit still requires complete input delivery.
Unix pipe workers wait for OS readiness instead of imposing a fixed sleep on
each temporary `WouldBlock`; the existing post-cleanup drain bound is unchanged.
Other pipe/drain/cleanup failures remain errors. The frontend's signal handling,
complete build deadline, source/executor readiness
and isolated adapter still need integration before any compiler can launch.
Guard tests and subprocess fixtures do not qualify a real toolchain build.

## Current CLI scope

The CLI consumer `install`, `list`, `verify` and `path` commands are unchanged.
The implemented plan requires explicit `--backend legacy-preview`; native
fails before file reads. `build`, the adapter, integrated ownership/cancellation,
and real Linux x86-64/macOS AArch64 preview evidence remain later M1 work.
No extra executable is exposed. `aros-fetch` is added as a dependency
only when native transport is actually implemented, not as an unused promise.

See the [producer contract](../../docs/toolchain-producer-contract.md),
[delivery plan](../../docs/toolchain-producer-plan.md) and
[M1 issue](https://github.com/metaneutrons/aros-tools/issues/29).

Run the maintained native tests with:

```sh
cargo test --locked -p aros-toolchain
cargo test --locked -p aros-cli --test toolchain_plan_cli
```

The normal workspace test gate includes this library on each native CI host.
No compiler build or full A/B matrix is needed for this non-executing change.
