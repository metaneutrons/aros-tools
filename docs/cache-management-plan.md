# Cache management: architecture and delivery plan

Decision state: proposed; implementation and acceptance require review of this
plan. Planning baseline: 2026-09-13, aros-tools
`1aed0c970f9a4df2fb047a1b5603ecea1d1a6bed`.
Owner: Fabian Schmieder. Tracking prefix: `CACHE`, independent of the completed
`TCP` producer milestones.

Epic: [Unified and safe cache management / #139](https://github.com/metaneutrons/aros-tools/issues/139).

This document owns requirements and design decisions. Linked milestone issues
own execution state and evidence. Proposed commands below are **not shipped
interfaces**. This initiative includes missing cache functionality, not just
renaming existing commands.

## Outcome and boundaries

Users should be able to answer: Which cache will this command use? What is
already available? Can I build without fetching? Is the data intact? What can
I safely remove, and what will need downloading or regenerating afterwards?

Use one discoverable resource-oriented entry point, `aros cache`, with short
family names and truthful verbs. Support both sccache and ccache without
pretending their capabilities are identical. Keep trusted source acquisition,
build acceleration and installed-toolchain lifecycle separate.

In scope: inventory, explicit population, integrity verification, compiler
statistics/control, retention, conservative removal, integration, diagnostics,
tests and public documentation for the cache families actually present.

Not in scope: package version changes or new hidden pins; an online native
producer build mode; a remote cache service or credentials manager; global
Cargo/rustup/Homebrew/pip caches; CI service cache administration; generic disk
cleanup; release publication; changing upstream AROS's Python dependencies.
Installed toolchains, external prefixes, source checkouts, build/work trees,
CMake configuration, reports, installed/source-publication receipts and
resumable execution state are not
disposable caches. Their owning commands retain lifecycle authority.

## Pre-M1 source-backed baseline

The table records the code inspection made before CACHE-M1 implementation, not
successful end-to-end qualification. It remains historical context for the
acceptance criteria below; implemented changes are recorded as M1 evidence and
must not be read as current behavior.
Only local backend help/version commands were run; no user cache was cleared,
backend daemon started, or payload downloaded for this analysis.

| Area | Current implementation and gap |
| --- | --- |
| Compiler frontend | [Parser](../crates/aros-cli/src/main.rs) exposes `aros ccache --stats --clear`; stats defaults to true. [Dispatch](../crates/aros-cli/src/commands.rs#L506) prints a successful clear after the backend command. |
| Incorrect sccache clear | [Backend mapping](../crates/aros-cli/src/build.rs#L386) maps clear to `sccache -z`, which resets counters, not entries. ccache uses `-C`, which really clears entries. No distinct reset-statistics operation exists. |
| Compiler selection | Rust detection prefers sccache. [CMake](../crates/aros-cmake-engine/engine/CompilerCache.cmake) independently rediscovers launchers and writes cached launcher settings, including an ineffective ASM variable. There is no single resolved selection passed from frontend to engine. |
| Consumer archives | [artifact.rs](../crates/aros-cli/src/artifact.rs) shares a raw-SHA-256 download cache between host and cross compilers: `AROS_CACHE_DIR` or `AROS_HOME/cache`, then `downloads/sha256/<digest>.tar.xz`. Acquisition is coupled to consumers, with no standalone cache inventory/population interface. |
| Archive selection | [host_compiler.rs](../crates/aros-cli/src/host_compiler.rs) selects host inputs from effective `aros-targets.toml`; [toolchain.rs](../crates/aros-cli/src/toolchain.rs) uses the cross-toolchain lock. Installed prefixes and their receipts are separate. `force` bypasses a valid cached archive and then fails offline; it does not replace an installed tree. |
| Locked producer sources | [source_cache.rs](../crates/aros-toolchain/src/source_cache.rs), [compatibility_ports.rs](../crates/aros-toolchain/src/compatibility_ports.rs) and [aros-fetch cache API](../crates/aros-fetch/src/engine/cache.rs) already acquire/verify declared inputs. Exact-byte and canonical-tar-gzip identities differ. Producer CLI combines acquisition with `--verify-only`. |
| Product source and patch caches | [aros-fetch](../crates/aros-fetch/src/engine.rs) has archive candidates and a declaration-scoped patch cache with downloaded candidates, normalized patch payloads and receipts. Its public transaction also extracts/applies patches; it is not yet a cache-only prefetch operation. |
| Cargo vendor inputs | [cargo_vendor.rs](../crates/aros-toolchain/src/cargo_vendor.rs) validates/copies a prepared vendor tree and templated Cargo configuration against selected lock inputs. [The documented bootstrap](../docs-site/src/content/docs/workflows/toolchain-producer.md) still requires manual Cargo invocation and a Python configuration-rewrite snippet. |
| GenMF reference expansions | [genmf.rs](../crates/aros-verify/src/genmf.rs) caches relative-path expansions and uses dependency mtimes for freshness. [aros-verify](../crates/aros-verify/src/lib.rs#L790) places these in `<work>/genmf`, separate from reports. There is no content-identity inventory or standalone management interface. |
| Concurrency boundary | Existing candidate, destination, patch and toolchain-store guards have different scopes. They are not a universal cache reader/collector protocol. A lock or verified file does not by itself make a generic pruning pass safe. |

Local help inspected: sccache 0.17.0 and ccache 4.14. Backend semantics are
confirmed by the [ccache manual](https://ccache.dev/manual/latest.html) and
[sccache command implementation](https://github.com/mozilla/sccache/blob/main/src/commands.rs).
sccache statistics may start its server; even apparently observational backend
commands cannot silently be treated as passive inventory.
[sccache configuration](https://github.com/mozilla/sccache/blob/main/docs/Configuration.md)
also allows remote/multi-level stores. A local directory is not necessarily
the complete effective cache.

The M1 backend-contract qualification floor is ccache 4.14.0 and sccache
0.17.0. Older backends can still be passively observed, but no lifecycle
behavior is promised for them. The repeatable
[`verify-compiler-cache-backends.sh`](../scripts/verify-compiler-cache-backends.sh)
probe starts from an empty private temporary root, Home, config, cache and
sccache Unix-domain socket. It proves that ccache reset preserves a cached
entry, ccache clear removes it, and sccache reset preserves an entry; it always
stops only that private server. It never reads, creates or clears user state.

## CLI contract

### Vocabulary and capability boundaries

`aros cache` without a verb shows help. `aros cache status` gives a bounded,
passive overview. Per-family help states what is managed and what is excluded.
No hidden aliases and no generic `aros cache clear --all`.

| Family | Meaning | Proposed operations |
| --- | --- | --- |
| `compiler` | Compiler-result acceleration, with explicit backend and storage scope | `status`, `stats`, `reset-stats`, `clear` |
| `sources` | Product/producer source archives and reusable patch payloads | `status`, `list`, `fetch`, `verify`, `keep`, `release`, `remove`, `prune` |
| `archives` | Downloaded host/cross-compiler packages, not installed toolchains | `status`, `list`, `fetch`, `verify`, `keep`, `release`, `remove`, `prune` |
| `cargo` | AROS-managed Cargo vendor inputs, not the user's global Cargo home | `status`, `list`, `fetch`, `verify`, `keep`, `release`, `remove`, `prune` |
| `genmf` | Reusable GenMF reference expansions, not verification reports | `status`, `list`, `verify`, `refresh`, `remove`, `prune` |

Expose only meaningful capabilities; do not add compiler `fetch` or pretend
that a statistics reset verifies compiler outputs. `keep` creates a named,
immutable retention reference for an exact input set; `release` removes that
reference, not its payload. Unsupported capabilities are explicit errors,
never successful no-ops.

### Discovery, roots and selection

- Passive `status` does not create directories, contact networks, start a
  daemon, generate files, recursively hash payloads or reserve new locks.
  Missing and inaccessible are distinct; unknown size is not zero.
- `list` enumerates bounded metadata for one selected family/root. `verify`
  performs explicit bounded integrity work. Report truncation and incomplete
  coverage, never call a partial scan a complete success.
- Preserve existing root selectors and show their provenance: explicit flag,
  project configuration, environment or default. Use `--dir PATH` consistently
  for an explicit family root and `--project DIR` when selecting consumer
  configuration. Resolve paths before acting; never infer a producer source
  cache from the consumer archive-cache override.
- `status` may list configured/registered roots only, not crawl disks or home
  directories. Unregistered project/producer roots require explicit selection.
  Shared bytes are counted once per object, not once per consumer reference.
- Compiler commands accept `--backend auto|sccache|ccache`; auto selection must
  match the effective build selection, not merely the first program on PATH.
  Show executable, supported version/capabilities, local/remote/mixed scope,
  configuration provenance and whether observations are passive or queried.
- Sources use explicit schema-discriminated lock/request adapters, not layout
  guessing. Compatibility acquisition/verification covers its complete lock;
  profile-filtered materialization remains an execution operation. Host-Python
  source-lock payloads are included and labelled by role, not filename guesses.
- Archive fetch selects exactly one purpose: `--toolchain --preset NAME` or
  `--host-compiler`. Resolve current lock/target configuration through existing
  shared selectors. `--host HOST` allows prefetching for another supported host
  without executing or installing its binaries.
- Sources fetch/verify accepts one reviewed source/compatibility lock or one
  validated source-fetch plan. The plan records candidate origins, integrity
  policy and patch declarations; it must not execute arbitrary build scripts
  to discover the closure. No implicit full-repository source download.
- Cargo fetch/verify requires the exact selected tools checkout and its locked
  dependency closure, plus explicit cache root. GenMF operations require an
  explicit expansion root; verify/refresh additionally select the source tree.

### M1 status contract

The first shipped CACHE-M1 surface is intentionally limited to passive status:

```text
aros cache status [--format human|json]
aros cache compiler status [--backend auto|sccache|ccache] [--dir DIR] [--format human|json]
```

`aros cache` with no subcommand remains help-only. The command does not expose
future `fetch`, `verify`, `remove`, `prune`, `keep`, `release`, `clear`,
`reset-stats`, or `stats` operations before their owning acceptance criteria
are met. There are no compatibility aliases for those future names.

`cache status` returns `aros-cache-status-v1`. It contains exactly one
non-recursive, no-follow observation of the selected archive root and passive
compiler backend observations. Archive-root precedence is `AROS_CACHE_DIR`,
then `AROS_HOME/cache`, then `$HOME/.aros/cache`; configured values must be
absolute. Sources, Cargo and GenMF have no safe implicit root at this stage, so
the result says `requires_explicit_selection` instead of guessing. A missing,
symlinked, non-directory or inaccessible root is reported as such. Status does
not create a path, reserve a lock, enumerate content, hash a file, contact a
network, invoke a backend or parse a backend configuration file.

`cache compiler status` returns `aros-cache-compiler-status-v1`. It exposes
the observed executable path, availability and names of recognized backend
environment variables only; their values are not serialized. `auto` selects
available `sccache` first, then `ccache`; an explicit missing backend leaves
`selected_backend` null rather than falling back. Effective local, remote and
mixed storage remains `configuration_uninspected` until an explicit later
query can prove it. The status command has no backend-version requirement
because it never invokes a backend. Any future stats/reset/clear contract must
declare its minimum backend versions and query side effects before becoming
public. `--dir DIR` is absolute and observes one explicit root with an
`explicit` provenance; without it, an available selected backend exposes only
the AROS-owned candidate below `AROS_HOME/cache/compiler/v1/<backend>`. Both
forms have `root_binding: status_only_not_applied`: status never configures or
claims that a backend uses the root. Both status schemas include their stable
operation name, all false side-effect flags, current capabilities,
selection-basis and effective-build-selection boundary so automation need not
infer safety from prose.

The normal result schema is independent of the existing diagnostics envelope.
An invalid relative state override is an ordinary frontend configuration error;
an unavailable backend is a successful status observation. Both human and JSON
rendering preserve these distinctions. The canonical Astro reference and
parser-backed example gate are part of this contract.

### M2 source-cache contract

CACHE-M2 adds only these source-cache commands. They operate on an explicit,
existing absolute `--dir`; the command never infers a producer cache from an
archive-cache setting and it never creates an extracted source tree, a build
directory, a worktree, a package or a release artifact.

```text
aros cache sources status --dir DIR [--format human|json]
aros cache sources list --dir DIR SELECTOR [--format human|json]
aros cache sources fetch --dir DIR SELECTOR [--offline] [--format human|json]
aros cache sources verify --dir DIR SELECTOR [--format human|json]
```

`SELECTOR` is exactly one of `--source-lock FILE`,
`--compatibility-ports-lock FILE`, or `--source-fetch-plan FILE`. The first
selects `aros-toolchain-source-lock-v2`; the second selects
`aros-toolchain-compatibility-ports-v2`; the third selects a new,
schema-discriminated `aros-cache-source-fetch-plan-v1`. A selector is a
regular, bounded input read without following a final symlink. Its raw-byte
SHA-256 is the `request_sha256` recorded by every list, fetch and verify
result. Selection is never based on a filename, inferred project state or a
source-tree scan.

The source-fetch plan represents the exact data formerly implicit in one
product source-fetch declaration: a stable role, one portable direct cache
filename, ordered credential-free HTTPS candidates, an explicit `archive` or
`patch` representation, normalization policy, and either a size/SHA-256
identity or an explicit `unverified` integrity classification. A patch also
binds its relative target subdirectory and reviewed option sequence. The parser
rejects duplicate role/candidate identities, unsafe names, relative paths,
credentials, non-HTTPS origins, unbounded input sets and unsupported
normalization. An unverified declaration is not upgraded to a pin: a
successful fetch records its measured identity as
`measured_unpinned`, and verify can report presence but cannot report upstream
integrity. Product-plan acquisition is permitted only with an explicit
`--allow-unverified`; producer and compatibility locks remain strict.

`status` is passive: it observes only the selected root metadata and emits
`aros-cache-sources-status-v1`. `list` is a bounded metadata projection of the
selector's declared entries. It emits `aros-cache-sources-list-v1`, an entry
role, normalization, declared identity, object location class and one of
`missing`, `present_unverified`, `unsafe` or `inaccessible`; it does not hash
payload bytes and never calls a present entry verified. `verify` emits
`aros-cache-sources-verify-v1`, snapshots and hashes every selected object and
reports exact consumed object identities. `fetch` emits
`aros-cache-sources-fetch-v1`; it first verifies every existing selected object
and only then downloads missing declared objects. `--offline` forbids every
transfer and turns each selected miss into a typed diagnostic. `fetch` never
refreshes, replaces, deletes or silently repairs an object; those lifecycle
operations remain reserved for CACHE-M6.

All three selector adapters yield one shared `SourceCacheRequest` with the
request digest, role, candidate/normalization policy and declared identity.
Its writer takes the same per-object no-clobber guard as its reader's
no-follow snapshot. Staging is private and an interrupted or competing writer
cannot expose a partial object. Every producer/compatibility consumer is
migrated to that shared request path before its old public cache/verify-only
frontend is removed. The shared path records the request digest and every
consumed object identity in its observation, so a resumed operation cannot
silently substitute a changed closure.

The M2 root is deliberately an explicitly selected cache *view*, not a global
garbage-collection authority. Matching immutable entries may be reused, but a
same-name/different-identity declaration fails closed. Source-archive
deduplication, retention references, removal and pruning are separate later
contracts; CACHE-M2 does not infer ownership from an arbitrary directory.

### M3 compiler-archive contract

CACHE-M3 adds only the following standalone archive operations. They reuse the
consumer archive cache and its verified acquisition primitive but never invoke
an extractor, compiler, collector, installer, tree verifier, receipt verifier,
provenance verifier, or attestation verifier.

```text
aros cache archives status [--format human|json]
aros cache archives list --project DIR SELECTOR [--format human|json]
aros cache archives fetch --project DIR SELECTOR [--offline|--refresh] [--format human|json]
aros cache archives verify --project DIR SELECTOR [--format human|json]
```

`SELECTOR` is exactly one of `--host-compiler` or
`--toolchain --preset NAME`. `--host HOST` is optional; its absence selects the
running host and its presence selects a declared release-matrix host without
executing that host's binaries. `--project DIR` resolves to one explicit AROS
source checkout. Host compiler selection reads its effective `aros-targets.toml`
contract; cross-toolchain selection reads its `aros-toolchains.lock.toml` and
checks the selected artifact triple against the target profile. JSON output
records the canonical project, configuration path/kind, host and host-selection
origin, release/profile/triple when relevant, archive URL, SHA-256, known or
unknown expected size, content-addressed cache path, and transport provenance.
An explicit `AROS_HOST_COMPILER_URL` override is honored for host-compiler
transport while its configured version and digest remain authoritative.

`status` emits `aros-cache-archives-status-v1` and observes only the archive
root metadata. `list` emits `aros-cache-archives-list-v1`; it observes only the
selected final archive path and labels it `missing`, `present_unverified`,
`unsafe`, or `inaccessible`. `fetch` emits `aros-cache-archives-fetch-v1`; it
uses the shared `obtain_archive` identity/path, revalidates an available object,
and stages a missing transfer before no-clobber publication. `--offline`
forbids transfer and creates no state; `--refresh` conflicts with it and only
reacquires the already declared identity. Neither option replaces an existing
content-addressed object or an installed tree. `verify` emits
`aros-cache-archives-verify-v1` and proves only archive file regularity, exact
known size and SHA-256. It explicitly does not claim extraction safety,
payload-tree, installed-receipt, provenance, or attestation validity.

Host compiler assets may not declare a size. That is a distinct `unknown`
contract value, never zero: downloads retain the bounded maximum enforced by
the common acquisition primitive. Missing SHA-256 values are rejected before a
cache identity is derived. Archive lifecycle, ownership, retention and removal
remain CACHE-M6 work.

### Everyday examples

The examples describe the target interface; exact parser/schema fixtures are
frozen in CACHE-M1 and refined through reviewed plan changes.

```sh
aros cache status
aros cache compiler status
aros cache compiler stats --backend sccache --format json

aros cache sources status --dir /work/source-cache
aros cache sources fetch --source-lock sources.lock.json --dir /work/source-cache
aros cache sources verify --compatibility-ports-lock ports.lock.json --dir /work/source-cache
aros cache sources list --source-fetch-plan grub.fetch-plan.json --dir /work/source-cache
aros cache archives fetch --toolchain --preset pc-x86_64 --project /work/AROS
aros cache archives fetch --host-compiler --project /work/AROS
aros cache cargo fetch --tools-source /work/aros-tools --dir /work/source-cache
aros cache cargo verify --tools-source /work/aros-tools --dir /work/source-cache
aros cache genmf verify --source /work/AROS --dir /work/verify/genmf

aros cache sources keep --name pc-offline --source-lock sources.lock.json --dir /work/source-cache
aros cache sources prune --dir /work/source-cache --older-than 30d
# Inspect the preview, then rerun the exact request with its returned token:
aros cache sources prune --dir /work/source-cache --older-than 30d --apply TOKEN
```

`fetch` is the explicit population/network boundary; normal verified writes
do not require deletion confirmation. `fetch --offline` is meaningful:
resolve only already available inputs and report exact misses. `--refresh`
means reacquire the same declared identity, not update package versions; it
conflicts with `--offline`. Corrupt existing entries produce an actionable
error and remain intact until a separately authorized targeted removal.
Refresh failures must not destroy the last valid object.

Native `toolchain build` has no online mode. Remove its redundant required
`--offline` switch and the corresponding plan toggle; encode the invariant in
the typed plan and receipt. Keep offline controls on operations that genuinely
have online/offline alternatives. Describe the policy as controlled upstream
fetching, not as an OS-level network sandbox.

### Compiler behavior

- Introduce one resolved compiler-cache selection, including the absolute
  executable and effective storage/network policy. Pass it to CMake and record
  it in build observations. Do not let CMake independently reselect a backend
  or retain a stale launcher after selecting `off`.
- Use the explicit build option
  `--compiler-cache auto|off|sccache|ccache`, not ambiguous `--cache`.
  Auto retains sccache-first preference when usable and policy-compatible;
  explicit missing/incompatible selection fails with a remedy. Never silently
  substitute another backend after an explicit choice.
- CACHE-M1 establishes exact launcher selection, not a false claim of storage
  ownership. Status may show the candidate
  `AROS_HOME/cache/compiler/v1/<backend>` namespace, but no build assigns it
  until its backend/server lifecycle is proven. Existing backend configuration
  is observed as external/uninspected, never silently adopted or rewritten.
  A build-level `--compiler-cache-dir PATH` is deliberately deferred to
  CACHE-M6: it requires a fresh/recognized owned root, rejection of conflicting
  external configuration and a dedicated sccache server identity derived from
  that root. Exposing the flag earlier would make `DIR` look isolated while it
  could still connect to an unrelated daemon. `AROS_CACHE_DIR` remains
  archive-only.
- `stats` is an explicit backend query and documents potential daemon startup.
  `reset-stats` resets counters only. `clear` removes entries only when that
  operation's exact storage scope is proven and supported.
- Fix the existing false sccache clear first: reject it truthfully until real
  clearing is implemented. Never map `-z` to cache deletion again.
- Provide real local clearing for an AROS-owned private namespace for both
  backends. A private sccache namespace also requires its own proven server
  endpoint/lifecycle and participating builds; changing SCCACHE_DIR while
  connecting to an existing daemon is not isolation.
- External/shared/remote/multi-level configurations remain visible and usable
  under explicit build policy, but unsupported deletion must fail closed.
  Do not stop a user's unrelated daemon, delete guessed directories, flush a
  Redis bucket, or claim local clearing removed remote entries.
- Offline builds must not silently use a remote compiler cache. Select a
  demonstrably local-only configuration or disable caching with a clear
  reason; an explicitly requested incompatible configuration fails.
- Do not wrap unsupported language/compiler invocations by default. CMake's
  supported compiler-launcher interface covers C and C++, not ASM. Test C and
  C++ launcher invocation and the unwrapped AROS assembly path independently;
  record the ASM bypass rather than setting an ineffective variable or using
  CMake's internal-only `RULE_LAUNCH_COMPILE` escape hatch.
  New cache use must not alter native producer release identities or defaults
  implicitly. Non-cacheable work is a supported result, not fabricated hits.

### Safe lifecycle, retention and recovery

All destructive cache operations are previews by default, including targeted
remove, prune, clear and retention-release. Reset-stats also previews its
scope because statistics cannot be restored. Mutation requires `--apply TOKEN`
bound to the exact operation, canonical root identity, configuration, policy,
candidate/reference set and expiry. No `--yes` bypass. Preview output explains
kept/in-use/unknown entries and consequences for offline builds.

Implement cache-specific cooperating reader/writer leases before enabling
removal. Every affected fetch, extraction, install, producer snapshot, vendor
copy and GenMF reader must participate for its actual consumption window.
Recheck identities under the operation's guards and fail on changed roots,
objects, policy, active leases, malformed metadata or unsupported schema.
Never delete an aged lockfile to infer that a cache is unused.

Limit mutation to recognized owned namespaces on supported local filesystems.
Refuse broad roots, symlink traversal, special files, overlapping source/work/
store roots, unknown trees and inaccessible inventories. Preserve foreign
entries. Existing private-store guard assumptions do not establish protection
against hostile same-user mutation: scope the threat model honestly and use
no-follow/identity-bound filesystem operations, not just stat-then-delete.

Named retention references pin cache availability, **not software versions**.
They record exact selected manifests/identities and all shared dependants,
including patch payloads and vendor generations. An absent global project
reference does not prove an object is unused. Prune only explicitly selected
owned roots using documented retention/age/size policies; age uses recorded
access metadata, not filesystem atime as truth. Missing trustworthy age means
keep. A byte budget is best-effort with the non-removable remainder reported.
Compiler eviction remains backend-specific, not this object-graph algorithm.

Publish new verified data atomically; vendor tree/config form one generation.
Bound download, enumeration, capture and hashing work; cancellation releases
leases and reports whether anything committed. Abandoned staging cleanup needs
proven ownership and inactive generation, not a filename prefix. Removing an
object is irreversible; the preview and result must say whether it can be
downloaded/regenerated and that unpinned upstream bytes may have changed.

Mixed old/new clients cannot be assumed to honor new leases. Use a versioned
ownership/lease fence or a new owned namespace; legacy roots remain inspectable
but are not automatically prunable. Never silently move user caches. Validate
an explicit migration from known layouts before enabling their management.

### Errors, results and observability

Reuse `aros-common` diagnostics, process boundaries, cancellation, logger and
commit-state semantics. Do not introduce a parallel cache logging framework.
Return versioned `--format json` result schemas, independent of the existing
`--diagnostic-format json` error channel; normal result data goes to stdout.

Define stable errors for unsupported capability, invalid selection, offline
miss, integrity mismatch, incomplete inventory, busy cache, changed plan/root,
unsafe ownership, external backend failure, timeout/cancellation and partial
commit. Include stage, family, safe path/object identifier, backend where
relevant, retryability and a concrete next command. Preserve source diagnostics
and subprocess exit/signal information rather than flattening their messages.

Use existing CLI success/failure exit behavior; do not invent a cache-only
exit-code convention. `status` can successfully report missing/not-configured;
an incomplete scan is explicitly incomplete. Required verification misses,
failed mutations and incomplete requested verification return failure.
Report metadata sizes separately from measured sizes and digest verification.
Backend unavailability/unknown stats is not a zero hit rate. Logs redact
credentials, authorization headers and sensitive origin/configuration fields.
Post-commit output/logging failure must not tell users nothing happened.

## Architecture and ownership

Use shared primitives and typed family adapters rather than a universal cache
directory sweep or another downloader.

- `aros-common`: existing diagnostics, safe filesystem/process primitives,
  typed identifiers and reusable locking infrastructure.
- A focused `aros-cache` library: inventory/result types, capability model,
  ownership, leases, retention and preview/apply contracts. It must not depend
  on CLI, producer, verifier or transport-specific business logic.
- `aros-fetch`: transport, source integrity/normalization, cache publication;
  extract generic raw archive acquisition from CLI here where reusable.
  Reuse its bounded HTTPS and verification logic, not a second HTTP client.
- `aros-toolchain`: source-lock, compatibility-lock, vendor and producer
  adapters; preserve existing snapshot and integrity guarantees.
- `aros-verify`: GenMF input identity, generation and expansion adapter.
- `aros-cli`: command parsing, selection and rendering; compiler adapter can
  live in a bounded backend module sharing the same resolved build selection.
  CMake consumes that selection, not a second detector.

Dependency direction is `aros-common <- aros-cache <- family owners <- CLI`.
Family registration is compile-time typed composition, not dynamic plugins.
Avoid putting domain-specific locks in the generic core. Preserve separate
identities for raw archives, canonicalized source archives, patch payloads,
vendor generations and derived expansions; a single SHA column does not make
these objects interchangeable.

Optional integrity policy for ordinary upstream source declarations remains
optional. Measured unpinned content is labelled as such and never upgraded to
trusted lock verification. Producer/release locked inputs remain strict.
Archive cache verification proves bytes against the selected lock, not an
installed tree's validity or provenance; those remain separate checks.

## Delivery and acceptance

### Shared CLI ownership and continuous documentation

The [public CLI plan](public-cli-contract-plan.md),
tracked by [epic #148](https://github.com/metaneutrons/aros-tools/issues/148),
owns current consumer force/local/offline validation (CLI-M2) and native
build/plan cache-only invocation migration (CLI-M3). CACHE-M3-A3 and
CACHE-M7-A1 consume that exact implementation evidence and extend it for the
new cache interfaces; they do not schedule duplicate implementations. CLI-M3
does not depend on CACHE-M7. CACHE-M1 owns early repair of both the redundant
legacy stats option and false sccache clear, independently of final migration.

Every milestone and every implementation PR that changes a public interface
must update the affected Astro/Starlight reference, task examples,
  configuration and troubleshooting in the same slice. Reuse the CLI-M1
  source-derived reference and semantic-example gate, alongside the canonical
  docs gate. Proposed interfaces remain in this repository plan, not presented as
shipped commands on the website. CACHE-M7 checks integrated completeness; it
does not permit stale documentation in earlier milestones.

### CACHE-M1

Contracts, shared foundations and truthful compiler-cache behavior.
Execution: [#140](https://github.com/metaneutrons/aros-tools/issues/140). Dependencies: none.

- CACHE-M1-A1: Approve parser examples, family root/selection precedence,
  capability and JSON/error contracts through a reviewed plan/contract PR.
  Help identifies mutation, daemon/network side effects and unsupported cases.
- CACHE-M1-A2: Implement passive root/compiler status and the shared core
  boundaries; missing roots, inaccessible paths and bounded incomplete
  observations are represented truthfully without creating state.
- CACHE-M1-A3: Prove distinct stats/reset/clear effects with fake-backend tests
  and isolated real sccache/ccache tests. False sccache clearing is rejected
  until the owned-namespace implementation can demonstrate actual removal.
  Remove the redundant legacy `ccache --stats` option in this early slice;
  if the legacy frontend remains, bare `ccache` keeps its documented stats
  operation. Record breaking-change notes and updated callers. This supplies
  the CLI audit F05/F06 repair evidence without waiting for CACHE-M7.
- CACHE-M1-A4: Build/CMake share exact backend selection, explicit off removes
  stale launchers, offline policy handles remote backends, and focused
  integration evidence proves C/C++ launcher coverage plus the explicit ASM
  bypass required by CMake's public API. No real user daemon/cache is mutated
  by tests. Shared compiler deletion remains disabled until CACHE-M6.
- CACHE-M1-A5: Update Astro compiler-cache usage, options, side effects and
  unsupported cases with the shipped changes; prove copied examples and run
  the canonical docs gate before accepting this milestone.

### CACHE-M2

Source and patch cache operations.
Execution: [#141](https://github.com/metaneutrons/aros-tools/issues/141). Dependencies: CACHE-M1.

- CACHE-M2-A1: Implement sources status/list/fetch/verify for producer source,
  compatibility-port and product source-fetch declarations through typed
  adapters, without running a build or publishing an extracted source tree.
- CACHE-M2-A2: Preserve exact/canonical normalization, optional upstream
  integrity, candidate collision detection and declaration-scoped patch
  identity. Tests cover same basename/different content, changed patch
  options, wrong hash/size, invalid origin, offline miss and corrupt cache.
- CACHE-M2-A3: Every participating consumer uses coherent publication and
  consumption guards. Cold fetch then offline reuse performs no network;
  cancellation and competing writers cannot expose partial objects.
  Cache observations retain the lock/request digest, role, normalization and
  consumed object identities so a resumed run cannot hide a changed input set.
- CACHE-M2-A4: Replace the public producer cache/verify-only combination with
  the shared source operations; record migration requirements for pinned
  producer workflows before removing their old parser path.
- CACHE-M2-A5: Update Astro source preparation, producer migration, offline
  and integrity examples with each public slice; validate their semantics
  against the implementation and pass the canonical docs gate.

### CACHE-M3

Standalone host/cross-compiler archive management.
Execution: [#142](https://github.com/metaneutrons/aros-tools/issues/142). Dependencies: CACHE-M1.

- CACHE-M3-A1: Implement archives status/list/fetch/verify without installing
  compilers. Both consumer types reuse the same verified acquisition API and
  object identity, with effective lock/config/root provenance in output.
- CACHE-M3-A2: Prove cross-host prefetch without executing foreign binaries,
  host configuration versus toolchain-lock selection, shared-object dedup,
  expected/unknown size handling and checksum failure behavior.
- CACHE-M3-A3: Freeze consistent refresh/offline semantics across direct cache
  and consumer entry points. Reuse CLI-M2 evidence for existing consumer
  force/local/offline conflicts and extend tests to the new cache/refresh
  interfaces; no accepted flag may silently mean something else or be ignored.
- CACHE-M3-A4: Archive verification never claims install/tree/attestation
  validity; active obtain/extract consumers hold participating guards.
  Installed stores and external prefixes remain untouched.
- CACHE-M3-A5: Update Astro archive acquisition and consumer installation
  examples, including refresh/offline conflicts and verification limits, in
  the introducing PRs; prove examples and pass the canonical docs gate.

### CACHE-M4

Rust-driven Cargo vendor cache preparation.
Execution: [#143](https://github.com/metaneutrons/aros-tools/issues/143). Dependencies: CACHE-M1.

- CACHE-M4-A1: Implement cargo status/list/fetch/verify for the exact selected
  tools source/lock and pinned Cargo tool, including registry and Git inputs.
  Invoke Cargo through existing bounded process control; do not reimplement
  Cargo's dependency resolver or modify the selected lockfile.
- CACHE-M4-A2: Replace the manual configuration-rewrite snippet with validated
  Rust generation using the existing one-placeholder vendor contract.
  Publish config and vendor content as one coherent generation.
- CACHE-M4-A3: Wrong source/lock/tool, missing Git dependency, bad checksums,
  unsupported configuration, interruption and racing readers/writers have
  negative tests. No user Cargo credentials/config are printed or overwritten.
- CACHE-M4-A4: Demonstrate a cold population followed by verified offline
  producer consumption, with no pip/Python bootstrap glue and no implicit
  access to global Cargo state. Existing upstream Python use is unchanged.
- CACHE-M4-A5: Replace the public Astro manual-bootstrap examples alongside the
  implementation; document exact vendor inputs and offline limits, validate
  copied commands and pass the canonical docs gate.

### CACHE-M5

Content-aware GenMF reference cache.
Execution: [#144](https://github.com/metaneutrons/aros-tools/issues/144). Dependencies: CACHE-M1.

- CACHE-M5-A1: Implement genmf status/list/verify/refresh in its explicit
  expansion namespace; keep reports and source/build trees outside authority.
- CACHE-M5-A2: Key freshness by actual source and template-include closure,
  generator and interpreter identity, relevant options and format version.
  Tests change contents while preserving mtimes and reuse relative filenames
  across source roots; stale expansion must not pass as current.
  Distinct paths that collide under the old slash-to-percent encoding must
  receive distinct identities.
- CACHE-M5-A3: Bounded generation publishes only successful complete entries;
  cancellation, missing includes, failed generator and concurrent verify/
  refresh preserve truthful state and cooperating leases.
- CACHE-M5-A4: `aros-verify --refresh` and cache commands share implementation.
  Existing mtime-only entries are unverified until regenerated; migration
  never deletes verification evidence or rewrites upstream generator code.
- CACHE-M5-A5: Update Astro verification and refresh workflows alongside the
  implementation, with tested freshness/migration examples and a passing
  canonical docs gate.

### CACHE-M6

Safe removal, retention and concurrent lifecycle.
Execution: [#145](https://github.com/metaneutrons/aros-tools/issues/145).
Dependencies: CACHE-M1 through CACHE-M5 for each affected adapter; no generic
pruning of an adapter whose consumers have not migrated.

- CACHE-M6-A1: Implement family remove/prune, named keep/release references,
  and real owned-local compiler clear/reset through the reviewed preview/
  token/apply contract. Display exact scope, recoverability and offline impact.
- CACHE-M6-A2: Prove no deletion of active, retained, foreign, unknown or
  unsupported entries; shared-source/patch/vendor references survive until
  the final applicable reference is released. Legacy nonparticipating clients
  keep their roots non-prunable.
- CACHE-M6-A3: Adversarial tests cover root swaps, traversal/symlinks, special
  files, tampered/expired tokens, policy/config changes, interrupted writes,
  busy readers, corrupt metadata, byte budgets and failed/partial operations.
- CACHE-M6-A4: Real isolated sccache and ccache tests demonstrate zeroing
  counters without deleting objects, actual local clearing, scope containment
  and correct server isolation. Unsupported external/remote deletion fails
  with an actionable explanation, never a guessed filesystem fallback.
- CACHE-M6-A5: Update Astro retention, preview/apply cleanup and recovery
  instructions alongside each exposed operation. Validate safe fixture
  examples, clearly state excluded storage, and pass the canonical docs gate.

### CACHE-M7

CLI/workflow migration, documentation and qualification.
Execution: [#146](https://github.com/metaneutrons/aros-tools/issues/146).
Dependencies: CACHE-M1 through CACHE-M6.

- CACHE-M7-A1: Migrate CLI, CMake, producer consumers, tests, completions and
  public Astro/Starlight docs together. Remove `aros ccache` and redundant
  legacy syntax without hidden aliases. Reuse CLI-M3's native build/plan
  offline-flag migration and CACHE-M1's stats-option repair evidence; verify
  that current callers remain compatible. Coordinated aros-toolchains PRs
  pin an exact compatible tools revision.
- CACHE-M7-A2: Publish task-oriented examples for inspection, offline
  preparation, both compiler backends, integrity failure, retained inputs and
  preview/apply cleanup. Every documented command has parser/behavior coverage,
  not just a command-name-count assertion.
- CACHE-M7-A3: Portable tests cover the currently active host policy; focused
  native cache/backend and build-launcher proofs run on Linux x86-64 and
  macOS ARM64. Include warm reuse, cold/offline misses, cancellation and
  surviving cached compiler output after resetting counters.
- CACHE-M7-A4: Qualify source/vendor/consumer integration with the native
  producer at an appropriate integration point. Preserve existing artifact
  integrity and reproducibility policies; if identity-affecting producer
  changes occur, require their existing qualification gates. Otherwise no
  full compiler A/B release matrix is required for this cache refactor.
- CACHE-M7-A5: Record exact plan/code/input identities and durable evidence for
  all criterion IDs, plus explicit supported/unsupported storage scopes.
  All dependent issues must be accepted before closing the epic. PR merge,
  implementation qualification and release publication remain separate claims.

## Migration, risks and verification cost

Ship bounded slices behind accurate capability reporting. Missing capabilities
remain explicitly unsupported until their milestone is qualified. The
false-clear defect may be fixed early without waiting for the full hierarchy.
Do not merge a public removal before its consumers and documentation have a
coherent migration path.

Old immutable releases and executor snapshots retain their old command
contract. Update current aros-toolchains workflows in a linked narrow PR;
never edit old release tags or silently repoint an executor pin. New CLI
removals require an explicit breaking-change/release-note decision even if the
project is still young. Do not assume it has never been released.

No blanket cache layout migration. Prefer inspectable old layouts plus new
versioned managed namespaces. A failed migration preserves originals and
reports completed steps. Rolling back software does not grant an older
binary permission to manage a newer ownership schema. Unreadable new state
must fail closed rather than be erased.

Compiler daemon configuration, source normalization, legacy lease adoption
and shared retention are the principal risks. The first contracts PR must
record minimum supported backend versions and prove private sccache server
isolation; inability to prove it blocks owned clearing, not a false success.
GenMF closure hashing must detect changed/missing included inputs without
turning inventory into implicit generator execution.

Use fixture/unit/CLI tests during iteration. Run focused native integration at
CACHE-M1/M6 and combined consumer proof at CACHE-M7; reuse unchanged evidence
with exact input identity. Follow the current Intel-macOS suspension and
repository CI policy, not the historical four-host release matrix. This
planning change does not authorize new scheduled monitors or expensive builds.
No completion-time estimate is asserted before fixture and native timings
exist.

The initiative is complete when all applicable milestone criteria are
evidenced, consumer migration and public docs are current, and unsupported
capabilities are accurately documented. Publishing a new release is a separate
maintainer action, not an implied effect of closing this epic.

## Reference decisions

- [Cargo vendor documentation](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html):
  use Cargo's locked vendor operation, then validate its output through our
  existing Rust contracts.
- Existing [toolchain producer contract](toolchain-producer-contract.md) and
  [M8 lifecycle evidence](tcp-m8-lifecycle-evidence.md) remain applicable within
  their original scope; they do not certify this proposed cache lifecycle.
- Alternatives rejected: brand-named top-level cache command; identical verbs
  implemented as backend no-ops; generic directory deletion; global Cargo
  cleanup; converting every input to one new raw-hash layout; implicit source
  version pinning; OS-sandbox claims based solely on an offline flag.

## Decision changes

2026-09-13: Initial source-backed proposal, including missing acquisition,
verification, retention, cleanup and GenMF identity capabilities. No
implementation or qualification is claimed by this planning record.

2026-09-13: Rebased onto the accepted CLI-M1 through CLI-M5 contracts. CACHE-M1
uses their existing reference and semantic-example gates; its early F05/F06
repair remains the sole cache dependency of CLI-M6.

2026-09-13: Defined the reviewable CACHE-M1 passive-status contract: versioned
status schemas, absolute archive-root precedence, no-follow single-root
observation, explicit compiler projection and no backend invocation. Future
cache verbs remain proposal-only until their individual acceptance criteria are
implemented and qualified.
