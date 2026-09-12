# Toolchain management: pre-M4 design review

Status: historical design review, 2026-09-06. No management command, state migration or
cleanup is implemented or authorized by this document. The
[TCP-M8 acceptance criteria](toolchain-producer-plan.md#tcp-m8--local-toolchain-management)
remain authoritative; [issue #41](https://github.com/metaneutrons/aros-tools/issues/41)
owns execution status. Implementation depends on accepted M4 envelopes and
verification. This review can proceed without blocking M1–M7.

The successor [M8 management contract](toolchain-management-contract.md)
freezes the implementation boundary and supersedes the proposal and unresolved
decision sections below where they differ. This document remains as the
source-review record; it is not public CLI documentation.

## What exists, and what must be reused

Source review baseline: tools `e1cee6cbadfa0038bd9db761c7fc8ed1c6f3d6df`.
These observations are about code, not a scan of anyone's installed tools.

| Existing owner | Observed behavior | Management consequence |
| --- | --- | --- |
| [CLI toolchain module](../crates/aros-cli/src/toolchain.rs), `default_store_root` / `locked_store_path` | Explicit absolute `AROS_CROSS_TOOLCHAINS_DIR`, otherwise AROS home / `cross-toolchains`; release/host/profile/archive-digest envelope containing `toolchain` | Preserve installed paths; multiple versions already coexist |
| Same module, `list` | Only the current project lock and running host; verification can execute tools and maps an invalid installation to `available` | Preserve this default; implement separate bounded metadata inventory, with explicit invalid/unknown states and no executable probes |
| Same module, `install` / `resolve_local` | `--local` resolves without copying; manifests and legacy marker-based prefixes have distinct source classes | Registration must remain optional and non-owning; never silently import or upgrade trust |
| Same module, `verify_locked_install` | Lock/manifest/inventory/layout checks, followed by executable probes; `.complete` is a regular file containing `complete` plus LF | Split metadata integrity from active compatibility probes; `.complete` is neither a signature nor deletion ownership |
| [Artifact module](../crates/aros-cli/src/artifact.rs) | Shared download cache, staged extraction, tree inventory, shared no-clobber publication | Move genuinely shared consumer logic behind the library boundary; do not build a parallel manager installer |
| [Common manifest types](../crates/aros-common/src/toolchain_manifest.rs) | Closed v1 lock with one release ID/base URL and host/profile artifacts; manifest and lock are distinct types | Preserve v1 interpretation; no per-profile mixed release or invented download fields for a local candidate |
| [Shared publication module](../crates/aros-common/src/publication.rs) | Durable, no-clobber publication with explicit uncertain outcomes | Retain committed data if durability/reporting fails; do not claim rollback after a rename |

The older `aros-common::toolchain::Toolchain` GCC/PATH detector is not the
content-addressed consumer store. Likewise, host-compiler management is a
separate facility. Neither becomes a second implementation of M8 semantics.

## Command decisions to freeze after M4

The following names are a proposal, not CLI documentation or parser contracts:

| Proposed operation | Authority and scope |
| --- | --- |
| `toolchain inventory` | Read all versions in one explicitly resolved store; optional explicit registered-project evidence; no filesystem-wide project discovery |
| `toolchain import` | Copy a verified local envelope into a newly owned destination; retain the original and its measured qualification |
| `toolchain register` | Record an external prefix by canonical identity without taking ownership; unchanged `--local` use needs no record |
| `toolchain select` | Preview and explicitly apply a project-lock change; never change a global active version |
| `toolchain remove` | Preview or remove one exact owned, unreferenced managed envelope |
| `toolchain gc` | Preview or remove only explicitly approved, revalidated eligible envelopes; not a recursive store sweep |

Freeze flags, parser fixtures and versioned human/JSON results before adding
these commands. Mutations need an explicit apply choice binding the previewed
identities, not a reusable global `--force`. Store identity, exact targets,
reference snapshot and expected project-lock digest belong in that binding.
A preview is not a lock or a reservation. No command silently downloads,
builds, imports, selects or deletes as a convenience side effect.

Inventory should distinguish metadata-only, integrity-verified and
compatibility-probed observations. A release-shaped manifest or matching
checksum alone does not prove release provenance. Track origin, integrity and
qualification separately; missing evidence is `unknown`, not `passed`.
Traversal is bounded by depth, entry count and byte budgets. Enumerate without
following envelope symlinks and report inaccessible, malformed or truncated
inventory explicitly. Do not run clang or a collector during discovery.

## Store ownership, references and concurrency

Keep the existing released-artifact envelope layout. Final imported-candidate
layout and receipt fields must follow M4's measured envelope contract. A
derived searchable index can be rebuilt; it cannot independently establish
trust, ownership, project selection or eligibility for removal.

Introduce a reviewed, versioned management-ownership record distinct from the
current `.complete` marker. Old installations initially remain legacy-managed
or unknown-owned, not automatically deletable. Explicit adoption must validate
their full envelope and stable filesystem identity without rewriting payloads.
External registrations are permanently non-owning. No ownership record is a
security boundary against another process with the same user's privileges.

Use a common store coordination protocol for installation, import, selection,
removal and build leases. Specify one lock order before implementation; all
consumers that can retain an artifact must participate before GC is enabled.
An in-process mutex or a list of PIDs alone is insufficient. Live leases bind
an artifact and operation identity and use OS-held locks; PID reuse or an
expired timestamp never proves that a lease is safe to reclaim.

Project registrations identify the project root and authoritative lock file,
including worktree identity where applicable. A missing/moved/unreadable
project, changed lock, incomplete scan or unknown consumer blocks automatic
reclamation. An explicit registration set is not proof that no unregistered
project uses an artifact: report that coverage limit. Existing non-participating
builds and pre-M8 clients therefore prevent an unconditional “unused” claim.
Conservative first delivery may leave GC blocked until adoption and lease
coverage are established; it must not substitute best-effort deletion.

Before mutation, reacquire locks and revalidate root/target filesystem identity,
no-follow containment, ownership, reference digests and active leases. Work with
exact managed entries, never arbitrary parents, the shared archive cache,
source checkouts or volume roots. Quarantine, if offered, is a recoverable
management state with its own validated restore/purge contract, not an excuse
to ignore active references. Uncertain publication/selection durability must
retain the committed output and describe the uncertainty.

## Project-selection and migration decision

First preserve the v1 lock: selection of a released toolchain updates the
whole coherent release lock after verified metadata projection, not one entry
under another release's global URL. Reuse normal artifact validation and
profile compatibility. Preview expected/actual identities, then compare the
old file digest again under a project lock before atomic publication. Concurrent
user edits remain untouched. Do not automatically fetch missing artifacts.

Persisting local candidates or mixed-release per-profile selections needs a
separately reviewed versioned lock extension. It must represent released,
imported-local and external-local variants without fabricated release IDs,
URLs or attestations. Specify old-reader failure, portable project references,
machine-local path handling and downgrade/rollback before enabling such writes.
Until then, existing explicit local overrides remain the supported path.
Do not add an ambient sidecar that secretly overrides the authoritative lock.

## Tests and decisions before implementation

Use tiny verified envelopes on the three active native hosts; Intel macOS is
explicitly suspended under [aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27). Management changes do
not warrant another compiler A/B matrix. Required tests include positive
inventory/import/registration/selection/removal, plus corrupt markers and
manifests, stale indexes, active and uncertain references, symlink substitution,
root replacement, concurrent select/install/remove, changed project locks,
crashes, PID reuse, full/read-only filesystems and post-rename uncertainty.
Assert both the diagnostic/commit state and preservation of sentinel inputs,
previous valid locks and external paths. Test metadata inventory with poisoned
executables that fail if invoked.

The following remain explicit implementation decisions, not claimed solutions:

- M4-compatible imported envelope and versioned ownership/selection formats;
- store lock ordering and participation of every existing consumer;
- reference coverage/adoption policy that can safely enable GC;
- bounded traversal budgets measured from representative stores;
- a reviewed local-selection schema migration and reversible downgrade path.

Implement metadata inventory and shared consumer extraction first, then
verified import/registration, project selection, and only finally removal/GC.
Keep destructive operations unavailable until their safety and migration gates
pass. Record measurements before estimating implementation effort; this design
does not supply fabricated deadlines or declare any M8 criterion complete.
