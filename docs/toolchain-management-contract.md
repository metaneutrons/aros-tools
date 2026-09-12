# Toolchain management contract

Status: M8 implementation contract and accepted evidence boundary, 2026-09-12. This document turns the
pre-M4 design review into the compatibility boundary for the M8 command family.
It does not make an installed payload trusted, selected or removable merely
because it was found by an inventory scan. The M8.1–M8.4 command contracts
below are implemented; the completed three-host evidence is recorded in the
[M8 lifecycle ledger](tcp-m8-lifecycle-evidence.md).

The [TCP-M8 acceptance criteria](toolchain-producer-plan.md#tcp-m8--local-toolchain-management)
and [issue #41](https://github.com/metaneutrons/aros-tools/issues/41) remain
the completion authority. The older
[design review](toolchain-management-design.md) preserves the source review
which informed these decisions; where the two documents differ, this contract
wins.

## Compatibility and authority

The existing released-artifact store is authoritative for its payload layout:

```text
$AROS_CROSS_TOOLCHAINS_DIR/
  <release-id>/<host>/<target-profile>/<archive-sha256>/
    .complete
    toolchain/
      toolchain-manifest.json
      ... payload
```

`AROS_CROSS_TOOLCHAINS_DIR` and `AROS_HOME` remain absolute-path-only inputs.
Existing `toolchain install`, `list`, `verify`, `path`, and explicit `--local`
keep their current meanings. In particular, `--local` neither registers nor
copies a prefix. No M8 command changes host-compiler state, downloads a
toolchain as a side effect, chooses a global active version, or treats a
candidate's `release_id` string as proof that a release was published.

The project file `aros-toolchains.lock.toml` remains the only selection source
of truth. A derived inventory, an ownership receipt, a registration, and a
lease can constrain an operation, but none can override a project lock.

## Result and diagnostic boundary

Every management command has a `--format human|json` option. JSON writes one
versioned document to stdout; diagnostics retain the existing `AR0401`
toolchain boundary and selected diagnostic format on stderr. Human output is a
summary of that same state, not a separate authority.

Commands distinguish the following facts rather than merging them into an
unsafe `installed` boolean:

| Fact | Meaning |
| --- | --- |
| metadata | Fixed-envelope shape, completion marker, and embedded manifest are syntactically and mutually consistent. |
| integrity | The full canonical payload inventory was measured against an expected value. |
| compatibility | Required executables were actively probed. |
| provenance | Evidence links the candidate to a published release, an imported local receipt, or only an embedded claim. |
| qualification | Recorded build/replay evidence, if any; absence is `unknown`. |
| management | Ownership class, not a statement that another same-user process cannot alter files. |

`valid metadata` is deliberately weaker than integrity, compatibility,
provenance, qualification, selection, and ownership. Unknown evidence is
reported as `unknown`, never inferred as `passed`.

## `toolchain inventory` v1

The shipped inventory command remains read-only:

```text
aros toolchain inventory [--store DIR] [--max-entries N] [--format human|json]
```

Without `--store`, it resolves the normal cross-toolchain store. `--store`
must be absolute. It examines legacy release envelopes and managed imports
through fixed path components, the `.complete` marker, embedded manifest and,
for a managed import, the ownership receipt. It neither reads a payload tree
nor invokes an executable. The default ceiling is 10,000 fixed-layout entries
and the hard ceiling is 100,000; a truncated scan is explicit and is not
coverage evidence.

Its JSON schema is `aros-toolchain-inventory-v1`. It contains the operation,
resolved store, store state, declared and used entry budget, `truncated`, a
coverage state (`complete`, `truncated` or `incomplete`), candidate `entries`,
and malformed-layout `findings`. Every candidate reports
the envelope location and selectors, marker and metadata state, and the
constant observations `integrity: "not-checked"` and
`compatibility: "not-checked"`. A legacy envelope reports
`provenance: "embedded-manifest-claim"`, `qualification: "unknown"`, and
`management: "unowned-legacy"`; a managed import with a receipt binding its
manifest identity reports `provenance: "imported-local-receipt"` and
`management: "owned-import"`. A malformed import is retained as an explicit
`unverified-local-import` / `invalid-import-envelope` observation. None of
these labels manufactures published-release, attestation or selection evidence
from a pathname.

All inspected directories and the manifest are no-follow objects. A symlink,
special object, unreadable name, malformed manifest, bad marker, unsafe path
segment, or invalid archive digest is preserved as an explicit observation.
It cannot be silently skipped and it cannot cause a payload to run. The
manifest reader itself now uses a descriptor-relative no-follow read and a
4 MiB document limit, so the same safety property also applies to the existing
installer/verifier path.

## M8 state formats

The first mutation increment writes only the following records. Their names,
locations and authority are frozen so later selection and cleanup cannot
introduce an ambient second selector.

```text
$STORE/.aros-management/v1/
  store.lock
  registrations/<registration-id>.json
  projects/v1/<project-id>.json
  project-locks/v1/<project-id>.lock
  leases/v1/<lease-id>.json
  lease-locks/v1/<lease-id>.lock
  envelope-locks/v1/<envelope-id>.lock
  removals/v1/<journal-id>.json

$STORE/imports/v1/<host>/<target-profile>/<managed-id>/
  .complete
  ownership.json
  toolchain/
    toolchain-manifest.json
    ... payload
```

`store.lock` is an OS-held advisory lock, never a PID file. The M8 lock order
is: store lock, then deterministic lexicographic project-root locks, then an
exact envelope lock, then the per-build lease lock. A build using a released
project lock keeps its project lock and confirms or publishes the matching
derived reference before compilation. A build using an owned local import
keeps its envelope and lease locks for the CMake/Ninja transaction; it does not
create a project-lock orphan because it has no project-lock selection to
serialize. It rereads a released project lock after taking the project lock,
so a concurrent selection cannot silently mix inputs. An external or legacy
local prefix remains non-owning and receives no cleanup authority. Expiry,
clock values and PID reuse never make a lease reclaimable; only the actual
absence of an OS-held lock does.

The in-envelope ownership receipt is staged, reread, and published atomically
with its managed payload. It deliberately omits the local source pathname.
The state directory has no selection authority. `project-locks` are only
OS-held mutual-exclusion guards whose deterministic ID binds the canonical
checkout root; they do not record a selected release. `projects/v1` contains
derived project-reference receipts, each binding a canonical checkout, its
authoritative lock pathname, the selected release ID and the exact lock digest.
The checkout lock remains the sole selector. The receipt lets a later cleanup
operation rediscover and revalidate a known project; it cannot activate,
override or repair its lock. Registrations, project references and future
leases are independently atomic records. If selection publishes a project lock
but cannot prove its derived-reference publication, it reports an indeterminate
committed state and cleanup must remain blocked. A corrupt, absent or stale
receipt blocks mutation; it cannot select, overwrite, or remove anything.
Existing released envelopes start as `unowned-legacy` and are not automatically
adopted or deleted.

### Import and external registration

The import command accepts only an absolute, self-describing candidate. Its
bounded source snapshot is copied through no-follow descriptors into private
staging; the embedded manifest, canonical payload inventory, and tree digest
are remeasured there. The resulting no-clobber managed envelope contains a
versioned ownership receipt which is reread before atomic publication.
Without the explicit apply token it emits a preview binding source snapshot,
candidate identity, and destination; no managed path or receipt is published.
The original source remains untouched. Imported is not released, attested,
selected, or executable merely because import succeeds.

The register command runs the same bounded source validation but publishes
only a versioned registration receipt. The normalized absolute external prefix
remains permanently non-owning. An external prefix may be used with the
existing `--local` path without registering it, and no removal or GC command
will target an external prefix.

Both mutations require a preview followed by an explicit apply token that
binds the exact source/destination identity and precondition digest. A
general-purpose `--force` flag is prohibited.

### Project selection

`toolchain select --release-lock FILE` writes only one complete, validated
released TOML v1 lock. `FILE` is absolute, read no-follow under a 4 MiB limit,
and must describe the checkout's complete target-profile matrix with the same
host set per profile. Every target triple is checked against the checkout and
all enabled assets must resolve below a credential-free HTTPS release base URL
ending in that lock's immutable release ID. This is structural coherence, not
network attestation or proof that an operator-passed file was published.

The preview binds the old lock's absence or exact digest and identity, the new
lock bytes and release ID, the canonical checkout root and destination to an
explicit apply token. On apply it takes the store lock and then the deterministic
project guard; it rereads both lock inputs and publishes either no-clobber (no
previous lock) or an identity-and-digest CAS replacement. It never creates a
mixed-release lock, invents an asset URL, or activates a local/imported
candidate. Existing `--local` is the compatible path for local candidates until
a separately versioned project-lock v2 can represent portable local and
external references, old-reader refusal, migration, and rollback.

After a successful lock publication, selection publishes a separate derived
`aros-toolchain-project-reference-v1` receipt under `projects/v1`. Its token
also binds the prior receipt snapshot. Selection rereads the receipt and rejects
any disagreement with the existing project lock before changing either record.
The two locations cannot share one filesystem rename transaction: a failure
after the lock commit is therefore reported as a committed-but-indeterminate
selection and makes future cleanup conservatively unavailable until the project
reference can be independently reconciled. The receipt is a use-reference
index, never an alternate selector.

### Removal and garbage collection

```text
aros toolchain remove --managed-id SHA256 [--store DIR] [--apply TOKEN] [--format human|json]
aros toolchain gc [--store DIR] [--apply TOKEN] [--format human|json]
```

Both commands produce an `aros-toolchain-lifecycle-v1` preview before any
mutation. The preview enumerates only fixed-layout `imports/v1` envelopes and
binds every candidate's managed ID, canonical envelope, full no-follow tree
snapshot, entry count and regular-file byte count. It also binds all readable
project-reference receipts and current authoritative lock digests, external
registrations and build-lease receipts/lock observations. A preview is
read-only: it creates no store, management record or lock file.

`remove` accepts exactly one managed ID. `gc` is an explicitly confirmed
reclamation of every currently eligible *owned import*; it is not a claim that
an unregistered or pre-M8 client cannot use a path. Operators must therefore
read the preview and confirm its exact token. A candidate is retained when a
project reference declares the same release ID, a non-owning registration names
its payload, or a corresponding lease lock is actively held. Any malformed or
incomplete import, project reference, registration, lease, journal, symlink,
unexpected namespace entry, unreadable project lock or changed control record
blocks the destructive operation rather than being skipped.

On apply the command takes store and sorted project locks, rereads the entire
control plane, then takes the exact envelope lock. It remeasures the envelope
and requires its complete identity/content snapshot to remain equal to the
preview. Immediately before deletion it no-clobber publishes an immutable
`aros-toolchain-removal-v1` journal outside the envelope. The shared
descriptor-relative deletion primitive then removes only entries in that
snapshot; it never follows links or accepts an arbitrary caller-selected
parent.

Destructive cleanup has one explicit Unix trust boundary. Every directory in
the managed envelope and every regular payload file must be neither group- nor
world-writable; regular files must not have more than one link. Ancestors may
be group/world-writable only when they have the sticky bit, which protects the
store owner's path entry (for example, the standard `/tmp` directory). A
violation fails closed. POSIX provides no unlink operation that atomically
names an already-open inode, so it cannot distinguish an arbitrary same-UID
process replacing the entire privately owned store from the store owner
itself. That process is deliberately within the single-user store trust
domain. Advisory locks serialize cooperating AROS processes; permissions
exclude a different group/world principal from the short identity-check-to-
unlink interval. Multi-writer shared stores are not a supported cleanup
deployment.

If anything fails after journal publication, the result is indeterminate and
the still-present envelope blocks subsequent cleanup. A journal whose envelope
is absent is retained as completion evidence. There is no retry-by-recursion,
automatic repair, PID-based reclamation or `--force`.

No M8 cleanup command can target a release envelope, archive cache, checkout,
volume root or external prefix. The exact-format publication lock left beside
an imported envelope by the shared no-clobber importer is recognized as
control-plane state even after its envelope was removed; an unknown sibling
still blocks cleanup.

## Delivery and evidence sequence

1. Read-only fixed-layout inventory and parser/JSON/no-follow fixtures.
2. Shared managed-candidate envelope, staged import, external registration,
   ownership receipts, and adversarial copy tests.
3. Released-lock selection preview/apply with atomic concurrent-change tests.
4. Store/project locks, participating build leases, safe remove/GC and crash
   recovery tests. **Implemented and accepted with three-host evidence.**
5. Positive and adversarial lifecycle evidence on the three active native
   hosts: Linux x86-64, Linux AArch64, and macOS ARM64. Intel macOS is
   explicitly suspended under
   [aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27),
   not silently dropped.

M8 management changes do not warrant a compiler A/B release matrix. They do
require the normal relevant CLI, package and fixture gates. The
[accepted evidence](tcp-m8-lifecycle-evidence.md) links the exact PR, schema
identities, test runs, failure fixtures and a black-box safe-selection/cleanup
demonstration for #41.
