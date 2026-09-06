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
The [legacy execution-view design](../../docs/toolchain-legacy-execution-view.md)
explains why metadata-free material cannot yet be passed to the historical
Git-aware driver. Its maintained Git fixtures establish an interface approach,
not an implemented conversion, adapter or build-readiness result.
Integrated execution snapshots, source capabilities, lock semantics, prerequisites/cache,
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

## Isolated source material primitive

`snapshot::SourceSnapshot::prepare` is a separate library API, not a CLI command
or an execution-ready plan. It borrows a live `RunDirectories` guard and selects
exactly one recipe root with `SourceRole::{Source, Producer, Tools}`. The two
complete inspections share one explicitly supplied deadline: a read-only
preflight before staging, then raw-object copying plus repeated source checks.
It never copies working-file bytes or hardlinks, runs filters, fetches objects,
executes producer scripts, or changes the original checkouts/cache/output.

Each role exclusively creates `.<role>-pending` under the held work descriptor.
Committed files retain their raw bytes and Git executable bit (private 0600/0700
files); recursively selected gitlinks become ordinary directories. All `.git`
metadata is omitted. Unicode/spaces are preserved; unsafe names and case-folded
sibling collisions fail. Source timestamps are not yet normalized for builds.

Symlinks are created only after resolving their complete graph against the
flattened committed inventory. Targets must be bounded relative UTF-8 paths
inside that input; absolute/escaping/dangling targets, link-expansion cycles, non-directory
traversal and more than 40 link expansions fail. This is intentionally stricter
than read-only `plan`, which can inspect dangling/escaping links without using
them. Links between the three selected roots are not authorized implicitly.

The shared no-follow/no-clobber publication primitive syncs and atomically
renames the complete staging tree to `<role>`. Revalidation before and after
publication checks the exact bytes, modes, link targets, membership and root
identity; snapshot regular files must have a single link. Later `revalidate`
uses the retained in-process inventory without Git. Editing an original file
cannot change the copy. Modified snapshots, extra `.git` or other entries,
special files, hardlinks and changed work/output ownership fail closed.

Failures retain partial or complete material and return AX diagnostics, including
publication failure classification; a retry never adopts it. Drop deletes nothing.
There is no persisted success receipt or automatic recovery. Each role is
independent, **not** one transaction across all three inputs. The future executor
must hold and revalidate all selected snapshots and its separate readiness gates.

Per preparation pass/input, the existing recursive count/byte/depth/blob limits
apply. Git children use at most ten seconds of the remaining explicit operation
deadline; cancellation is passed into the shared process runner. Filesystem loops
check budgets between entries and 64 KiB write chunks. Kernel I/O/fsync and the
shared publication traversal are not preemptible; timeout/cancellation observed
after publication is a failure with the complete tree retained, never success.
This is neither a same-user sandbox nor independent Git-object/origin verification.

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
