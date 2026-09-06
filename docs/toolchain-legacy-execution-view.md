# TCP-M1: isolated Git identities for the legacy adapter

Status: implemented lower-level consuming library API, **not an enabled
adapter**. Tracked by [M1 / #29](https://github.com/metaneutrons/aros-tools/issues/29).
This refines the [producer contract](toolchain-producer-contract.md); it does
not change recipe-v2, enable `build`, waive readiness gates or alter a producer
source pin. The [production-API tests](../crates/aros-toolchain/tests/source_snapshots/legacy_views.rs)
and [negative Git protocol probes](../crates/aros-toolchain/tests/legacy_git_contract.rs)
exercise synthetic repositories, not compiler qualification.

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
`snapshot::LegacySourceView` follows this boundary:

1. Hold the existing run guard and revalidate the selected raw snapshot. A
   conversion consumes that material guard; it must not leave a live
   `SourceSnapshot` falsely claiming that an augmented tree is metadata-free.
   The resulting execution-view guard is a different, non-serializable type.
2. Record each independently selected repository: role root and recursive
   gitlinks, exact commit/tree and raw inventory. A flattened path list alone
   loses this repository topology and is insufficient.
3. Relocate only our owned raw material from `<role>` to fresh
   `.<role>-legacy-pending`, using the shared durable no-clobber publisher.
   Construct private, exclusive metadata inside that unpublished staging tree.
   Never initialize
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
   authoritative. Publish the entire verified view as `<role>-legacy`, with
   shared no-clobber durability and descriptor/namespace checks at both
   relocation boundaries. No three-root transaction or resume/adoption exists.
8. Recheck actual commit/tree, index and complete raw material, plus the exact
   allowed metadata/topology. **No metadata entry is exempted**: generated files
   and directories join the same exact material inventory, with SHA-256/size
   records separate from Git object identities. Every byte and membership is
   checked before Git reuse and again afterwards. Optional index-cache writes
   are disabled; even those byte changes invalidate a view. The future driver
   must preserve this policy and revalidate before and after the legacy child.
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

The object transfer has shared per-role limits of 200,000 objects / 8 GiB and
64 MiB per object; at most 1,024 repositories are captured during raw preparation.
Normally an export batch contains at most 8 MiB plus framing / 4,096 objects;
a larger permitted object is transferred alone. Generated indexes/listings are
bounded at 32 MiB. The existing material count/byte/depth/path limits apply to
the complete augmented tree too. Each Git child is bounded to at most ten
seconds of the explicit shared operation deadline. Kernel I/O/fsync is not
preemptible, and a late deadline/failure may leave a complete retained tree.
Full metadata-byte rechecks before individual Git operations deliberately add
cost, especially when a repository needs many packs. Budget exhaustion is an
error, never permission to skip checks or use a partial view.

The shared AX diagnostic envelope preserves safe error classes, process exit/
timeout information and static Git step labels. It never prints arbitrary Git
stderr or private metadata. Any diagnostic emitted by isolated-store Git
validation, including a warning or truncated stderr, prevents success.

## What the maintained tests establish

- Exact original commit/tree IDs and clean legacy status survive the fresh
  shallow transfer; parent history, source-local configuration and extra Git
  metadata are absent. Later original mutations do not change the copied files.
- Source/producer/tools roles use distinct owned destinations. The consuming
  guard removes the old raw pathname, not its original checkout. Two levels of
  recursive gitlinks preserve independent identities, symlinks and raw bytes.
- A 9 MiB blob, a duplicate at another path and descendant trees exercise
  multiple independently strict pack imports through the production API.
- Strict import fails for a tree whose referenced objects are missing and for
  a damaged pack checksum. An otherwise valid pack carrying a different object
  succeeds at import, so the expected-vs-measured object-set comparison is a
  separate required check. A deliberately poisoned loose object cannot retain
  its claimed identity through this transfer; rejection may occur during export,
  import or the final object-set comparison, depending on Git's implementation.
- Restoring a corrupted original object database after raw snapshot capture
  cannot authorize poisoned snapshot bytes: imported material must also match
  the retained raw SHA-256 inventory, not just the original object ID strings.
- Modified config/HEAD/shallow/index/pack bytes, extra metadata, symlink/FIFO
  metadata, hardlinks, added directories and replaced view roots fail reuse.
  Occupied pending/final leaves are not adopted or removed. Missing/poisoned
  source objects, invalid/expired budgets and cancellation (including after
  metadata creation begins) fail with retained material and no successful guard.
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

A subsequent **production-API** macOS AArch64 probe on the same day used all
three actual selected roots: AROS `f3cfc243a84065166a46da28b0a5b22bbd0f8869`,
producer `c8039cf2b7291097ad62c6750bd7367e91a068f4` and tools
`707037be4f8ff37300a1a89166c35f661c28bafe`. Recipe digest:
`906ff611b34b1095fde00d86f164157c09d83ae6ee0fcfa3259b395280e14671`.
Using an optimized library build and explicit 180-second budgets per API
operation, the AROS raw snapshot took 49.629 s, conversion 104.387 s and later
revalidation 12.510 s. All **76** AROS repository stores (root + 75 gitlinks),
producer and tools passed; the selected producer's **unchanged** `verify-checkout`
accepted all three roots and lock/profile identities. Guard revalidation after
that credential-free verifier also passed. Producer raw/view/recheck timings
were 321/557/86 ms; tools 416/611/104 ms.

An earlier unoptimized run exhausted the same 180-second conversion budget and
retained its partial staging without a view. No limits/checks were relaxed for
the successful optimized run. These measurements are local source/interface
evidence, not cross-host/compiler qualification, a new timeout default, executor
origin or permission to reuse either probe's retained directories.

The previous successful-transfer test helper has been removed. Maintained
positive conversion/recursive tests call the production API; only deliberately
invalid pack fixtures retain direct low-level Git setup. There is no second
snapshot implementation or producer algorithm in tests.

## Remaining implementation and acceptance gates

- The consuming conversion, topology inventory, independently checked object
  graph and metadata guards are implemented, reusing raw auditors, filesystem
  publication and bounded process execution. Shared publication fault tests
  cover that primitive; view-level full-disk/post-rename fault injection is not
  yet separately qualified. Do not infer it from happy-path Git tests.
- Validate the full selected real inputs and the **unchanged** legacy
  `verify-checkout` command on Linux and macOS. Toy Git queries are not that
  integration evidence, and object-store validation alone is not a worktree.
- Finish frontend build identity, prerequisites/cache and sanitized child
  environment. Only then wire the explicitly selected coarse `legacy-driver`
  boundary and its existing failure/cancellation/result contracts.
- Demonstrate the two real local PC lanes and verified prefix use required by
  M1. Keep M1 open until those gates pass. This production source-contract slice
  warrants one explicit Linux integration run at its final PR head, not an
  additional release A/B matrix or repeated local GRUB build.
