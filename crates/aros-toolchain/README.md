# aros-toolchain

Experimental TCP-M1 producer inspection behind `aros toolchain plan`. It is
not a compiler driver or a released management feature.

Implemented:

- closed recipe-v2 parsing with duplicate/unknown-field rejection, canonical
  lowercase identities, strict integer fields and safe, sorted patch paths;
- bounded recipe input and canonical UTF-8 JSON self-digest verification;
- the shared envelope with AX0101 (contract), AX0102 (identity), AX0201 (Git
  prerequisites) and AX0202 (roots/resources), without input-document dumps;
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

The CLI consumer `install`, `list`, `verify` and `path` commands are unchanged.
The implemented plan requires explicit `--backend legacy-preview`; native
fails before file reads. `build`, the adapter, state ownership/cancellation,
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
