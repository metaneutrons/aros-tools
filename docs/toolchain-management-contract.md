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

The first shipped M8 command is read-only:

```text
aros toolchain inventory [--store DIR] [--max-entries N] [--format human|json]
```

Without `--store`, it resolves the normal cross-toolchain store. `--store`
must be absolute. It examines only the four fixed envelope path components,
the `.complete` marker, and the embedded manifest. It neither reads a payload
tree nor invokes an executable. The default ceiling is 10,000 fixed-layout
entries and the hard ceiling is 100,000; a truncated scan is explicit and is
not coverage evidence.

Its JSON schema is `aros-toolchain-inventory-v1`. It contains the operation,
resolved store, store state, declared and used entry budget, `truncated`, a
coverage state (`complete`, `truncated` or `incomplete`), candidate `entries`,
and malformed-layout `findings`. Every candidate reports
the envelope location and selectors, marker and metadata state, and the
constant observations `integrity: "not-checked"` and
`compatibility: "not-checked"`. The initial release reports
`provenance: "embedded-manifest-claim"`, `qualification: "unknown"`, and
`management: "unowned-legacy"`; it does not manufacture release or ownership
evidence from a pathname.

All inspected directories and the manifest are no-follow objects. A symlink,
special object, unreadable name, malformed manifest, bad marker, unsafe path
segment, or invalid archive digest is preserved as an explicit observation.
It cannot be silently skipped and it cannot cause a payload to run. The
manifest reader itself now uses a descriptor-relative no-follow read and a
4 MiB document limit, so the same safety property also applies to the existing
installer/verifier path.

## State formats reserved for later M8 increments

The following records are not yet written by the inventory command. Their
names, locations and authority are frozen now so import, selection and cleanup
cannot introduce an ambient second selector later.

```text
$STORE/.aros-management/v1/
  store.lock
  ownership/<managed-id>.json
  registrations/<registration-id>.json
  leases/<lease-id>.json
```

`store.lock` is an OS-held advisory lock, never a PID file. The M8 lock order
is: store lock, then deterministic lexicographic project-root locks, then an
exact envelope lock. A process that needs a payload during a build must retain
an OS-held lease before M8 cleanup can consider that payload. Expiry and PID
reuse never make a lease reclaimable; only release of the held lock does.

The state directory has no selection authority and may be rebuilt from
validated receipts. A corrupt, absent or stale index/receipt blocks mutation;
it cannot select, overwrite, or remove anything. Existing released envelopes
start as `unowned-legacy` and are not automatically adopted or deleted.

### Import and external registration

`toolchain import` will accept only a self-describing candidate whose payload
manifest and canonical tree digest have been remeasured in a staging directory.
It will copy into a no-clobber managed candidate envelope and publish a
versioned ownership receipt only after the copy and receipt have been reread.
The original source remains untouched. Imported is not released or attested.

`toolchain register` will store a canonical external prefix identity and its
observed evidence without copying it. Registrations are permanently
non-owning. An external prefix may be used with the existing `--local` path
without registering it, and no removal or GC command will target an external
prefix.

Both mutations will require a read-only preview followed by an explicit apply
token that binds the exact source/destination identity and precondition
digest. A general-purpose `--force` flag is prohibited.

### Project selection

M8 v1 selection writes only a complete, validated released lock. It receives
the exact trusted release-lock document, verifies its coherent one-release
schema and selector compatibility, previews the old lock digest and new lock
digest, then uses a project lock plus an atomic compare-and-publish update.
It never creates a mixed-release lock, invents an asset URL, or activates a
local/imported candidate. Existing `--local` is the compatible path for local
candidates until a separately versioned project-lock v2 can represent portable
local and external references, old-reader refusal, migration, and rollback.

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
