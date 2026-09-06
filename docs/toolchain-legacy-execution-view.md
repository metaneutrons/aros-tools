# TCP-M1: isolated Git identities for the legacy adapter

Status: interface characterization and implementation design, **not an enabled
adapter**. Tracked by [M1 / #29](https://github.com/metaneutrons/aros-tools/issues/29).
This refines the [producer contract](toolchain-producer-contract.md); it does
not change recipe-v2, enable `build`, waive readiness gates or alter a producer
source pin. The maintained [Git boundary tests](../crates/aros-toolchain/tests/legacy_git_contract.rs)
exercise tiny synthetic repositories, not compiler qualification.

## Why a raw snapshot is insufficient

The selected historical [build driver](https://github.com/metaneutrons/aros-toolchains/blob/c8039cf2b7291097ad62c6750bd7367e91a068f4/scripts/toolchain/build-release.sh#L76)
calls [verify-checkout](https://github.com/metaneutrons/aros-toolchains/blob/c8039cf2b7291097ad62c6750bd7367e91a068f4/scripts/toolchain/producer.py#L588).
For **each** source/producer/tools root, that verifier requires clean tracked
Git status and exact `HEAD` / `HEAD^{tree}` identities from the recipe. It also
checks the selected lock/profile digests. These are historical source references,
not a hidden runtime version selection.

`SourceSnapshot` deliberately contains no Git metadata, including at recursive
gitlinks. Its material inventory is necessary but cannot satisfy those Git
queries. Pointing the driver at original checkouts would reintroduce user state
and mutations. Copying `.git` would import unrelated history, configuration,
hooks, alternates and potentially credentials. Synthesizing a new commit would
change the recipe identity. A fake Git wrapper or bypassed `verify-checkout`
would defeat the selected producer's contract. None is the adapter solution.

## Selected approach: fresh stores containing the exact selected closure

Use Git's existing object implementation; do not add a Rust SHA-1 or pack writer.
The production execution-view implementation must:

1. Hold the existing run guard and revalidate the selected raw snapshot. A
   conversion consumes that material guard; it must not leave a live
   `SourceSnapshot` falsely claiming that an augmented tree is metadata-free.
   The resulting execution-view guard is a different, non-serializable type.
2. Record each independently selected repository: role root and recursive
   gitlinks, exact commit/tree and raw inventory. A flattened path list alone
   loses this repository topology and is insufficient.
3. Create private, exclusive metadata staging under owned work. Never initialize
   from user/system templates or copy an existing `.git`. Only reviewed fixed
   metadata, a detached exact `HEAD` and an explicit one-commit shallow boundary
   are generated. No remotes, credentials, hooks, replacement refs, alternates,
   promisor configuration, inherited Git settings or automatic maintenance.
4. Enumerate only that commit, its root/subtrees and ordinary/symlink blobs.
   Parent commits, refs, tags and unreachable objects are not inputs to this
   transfer. Gitlinks start independent stores; their commits are not objects
   of the parent repository. Missing local objects fail; never fetch them.
5. Transfer explicit object IDs in bounded batches using `pack-objects --stdout`
   **without `--revs`**, thin packs, existing-object reuse or delta searches.
   Import through `index-pack --stdin --strict` into the fresh store. Blobs must
   precede trees, descendant trees their ancestors, and the selected commit
   comes last when batches are independently strict. Bound compressed input,
   declared object sizes/counts, capture, memory, nesting and aggregate work;
   use one explicit operation deadline and cancellation token for every child.
6. Run strict full object validation in each isolated store and compare its
   **complete measured object set** with the expected set. Successful pack
   parsing alone does not prove that the expected objects were transferred.
   Do not downgrade fsck findings or substitute connectivity-only validation.
7. Construct indexes from the verified trees using `read-tree` without `-u`.
   Never check out files or run clean/smudge filters: raw snapshot bytes remain
   authoritative. Publish/attach only freshly verified owned metadata, with
   no-clobber durability and descriptor/namespace checks at every boundary.
8. Recheck actual commit/tree, index and complete raw material, plus the exact
   allowed metadata/topology. Metadata may be exempted from a raw material walk
   **only at independently verified repository roots**; a blanket `.git`
   exemption is not acceptable. Revalidate before and after the legacy child.
   Retain failures; never adopt staging or return the consumed guard as valid.

Git documents the explicit object-list input and self-contained pack format in
[pack-objects](https://git-scm.com/docs/git-pack-objects). Strict import rejects
broken objects/links; see [index-pack](https://git-scm.com/docs/git-index-pack).
[fsck](https://git-scm.com/docs/git-fsck) distinguishes full object validation
from connectivity-only checks, which do not inspect blob contents.

This is source/object integrity, not signature validation, trusted executor
origin, an OS sandbox, build permission or proof of independent reproduction.
Git SHA-1 identities keep their existing recipe meaning; they do not replace
the measured SHA-256 material/evidence contracts. Same-user races and malicious
source execution still require the existing trust and credential-free-job policy.

## What the maintained tests establish

- Exact original commit/tree IDs and clean legacy status survive the fresh
  shallow transfer; parent history, source-local configuration and extra Git
  metadata are absent. Later original mutations do not change the copied files.
- Strict import fails for a tree whose referenced objects are missing and for
  a damaged pack checksum. An otherwise valid pack carrying a different object
  succeeds at import, so the expected-vs-measured object-set comparison is a
  separate required check. A deliberately poisoned loose object cannot retain
  its claimed identity through this transfer; rejection may occur during export,
  import or the final object-set comparison, depending on Git's implementation.
- Parent fsck can succeed with a gitlink commit absent from its object database.
  Each child therefore needs its own object store, exact identity, raw checks
  and recursive topology; a green parent check cannot substitute for them.
- Metadata-free fixture material fails the historical Git identity/status
  queries. No compatibility result is inferred from the raw snapshot API.

A local macOS AArch64 scale probe on 2026-09-06 used Apple Git 2.50.1 against
the selected AROS root `f3cfc243a84065166a46da28b0a5b22bbd0f8869`, tree
`f5973eab2a8b0c40ebfb877caf357f0ec8ccf091`. It transferred and strictly checked
23,786 root-repository objects in 24 packs (190,809,172 bytes) in 4.063 seconds,
with an exact measured object-set comparison. The root contains **75 gitlinks**;
this probe did not transfer or qualify those child repositories, create a
complete worktree or run the legacy verifier/compiler. This is root-object-store
scale evidence, not an execution view, a timeout default or M1 acceptance.

The small test transfer helper is intentionally disposable **fixture setup**,
not a public implementation or another producer. Its assertions remain useful
regression contracts. It lacks operation ownership, aggregate source budgets,
durable publication and use-time guards; do not import it into production.
When the execution-view API exists, run these assertions through that API and
remove the superseded setup helper. Keep the tests maintained and cross-host.

## Remaining implementation and acceptance gates

- Implement the consuming view conversion and repository-topology inventory,
  reusing the existing raw visitor, filesystem and process boundaries.
- Bind the expected commit-to-tree graph to the imported object set, recheck
  metadata/index/material and test corrupted local object storage, replaced
  roots, hostile metadata, hardlinks, cancellation, full-disk and post-rename
  failures. Never weaken `SourceSnapshot::revalidate` to accept added `.git`.
- Validate the full selected real inputs and the **unchanged** legacy
  `verify-checkout` command on Linux and macOS. Toy Git queries are not that
  integration evidence, and object-store validation alone is not a worktree.
- Finish frontend build identity, prerequisites/cache and sanitized child
  environment. Only then wire the explicitly selected coarse `legacy-driver`
  boundary and its existing failure/cancellation/result contracts.
- Demonstrate the two real local PC lanes and verified prefix use required by
  M1. Keep M1 open until those gates pass; no extra release-matrix or GRUB run
  is justified by this design/fixture-only change.
