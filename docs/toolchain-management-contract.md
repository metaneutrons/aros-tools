# Toolchain management contract

Status: M8 implementation contract, 2026-09-12. This document turns the
pre-M4 design review into the compatibility boundary for the M8 command family.
It does not make an installed payload trusted, selected or removable merely
because it was found by an inventory scan.

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
  project-locks/v1/<project-id>.lock
  leases/<lease-id>.json

$STORE/imports/v1/<host>/<target-profile>/<managed-id>/
  .complete
  ownership.json
  toolchain/
    toolchain-manifest.json
    ... payload
```

`store.lock` is an OS-held advisory lock, never a PID file. The M8 lock order
is: store lock, then deterministic lexicographic project-root locks, then an
exact envelope lock. A process that needs a payload during a build must retain
an OS-held lease before M8 cleanup can consider that payload. Expiry and PID
reuse never make a lease reclaimable; only release of the held lock does.

The in-envelope ownership receipt is staged, reread, and published atomically
with its managed payload. It deliberately omits the local source pathname.
The state directory has no selection authority. `project-locks` are only
OS-held mutual-exclusion guards whose deterministic ID binds the canonical
checkout root; they do not record a selected release. Registrations and future
leases are independently atomic records. A corrupt, absent or stale receipt
blocks mutation; it cannot select, overwrite, or remove anything. Existing
released envelopes start as `unowned-legacy` and are not automatically adopted
or deleted.

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

### Removal and garbage collection

`remove` and `gc` remain unavailable until imports, registrations, project
locking, and leases participate in the common protocol. Their preview output
will bind exact managed IDs, locations, byte counts, ownership receipts,
reference snapshots, and lease observations. Before a mutation they reacquire
locks and revalidate every identity and lock digest. An unknown owner, missing
or unreadable registered project, incomplete reference scan, changed project
lock, active lease, untrusted symlink, or post-rename uncertainty blocks
destruction. They never recurse over a checkout, a volume, the archive cache,
a parent chosen by a caller, an external registration, or an old release
envelope merely because its name resembles a managed target.

## Delivery and evidence sequence

1. Read-only fixed-layout inventory and parser/JSON/no-follow fixtures.
2. Shared managed-candidate envelope, staged import, external registration,
   ownership receipts, and adversarial copy tests.
3. Released-lock selection preview/apply with atomic concurrent-change tests.
4. Store/project locks, participating build leases, safe remove/GC and crash
   recovery tests.
5. Positive and adversarial lifecycle evidence on the three active native
   hosts: Linux x86-64, Linux AArch64, and macOS ARM64. Intel macOS is
   explicitly suspended under
   [aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27),
   not silently dropped.

M8 management changes do not warrant a compiler A/B release matrix. They do
require the normal relevant CLI, package and fixture gates. The final evidence
must link exact PRs, schema identities, test runs, failure fixtures and a
manual safe-selection/cleanup demonstration before #41 can close.
