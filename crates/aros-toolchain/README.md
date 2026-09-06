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
proposed disjoint roots. Descriptor-relative reads reject links and special
files; bounded trusted Git queries disable filters, fsmonitor, hooks, lazy
fetching and inherited Git/credential settings. No build, source script,
download, cache scan, directory reservation, installation or cleanup runs.

Plans currently have `readiness: blocked`, even with complete resource options.
Recursive snapshots, source capabilities, lock semantics, prerequisites/cache,
executor origin, ownership and cancellation remain unqualified. A blocked
inspection exits 0; invalid input exits 1 without a result. Neither a plan nor
a valid self-digest grants build permission. Raw-file comparison does not
certify a concurrently mutable tree or ignored/untracked/submodule files.

The measured frontend file digest is not an attestation. An unproven frontend
commit is null in blocked plans, **not** the recipe's collector commit.
Execution/results/receipts may not adopt this plan-only exception.
Git queries use 10-second group limits within a 60-second Git budget;
documents/individual selected blobs and captured streams are capped at 1 MiB,
profiles/toolchains directory entries at 128, frontend hashing at 512 MiB.
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
frontend's signal handling, complete build deadline, source/executor readiness
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
