# Native toolchain producer contract (TCP-M0)

Status: M1 and M2 are accepted. M3 now implements a controlled local native
lifecycle after declaration binding: isolated committed snapshots, verified
offline source cache, private Python/Cargo environments, source `configure`,
source-owned `crosstools-release`, the exact collector build and durable phase
receipts. Recursive raw worktree/index/submodule inspection remains mandatory.
Trusted executor-origin verification, receipt revalidation/resume, a complete
candidate inventory and real host/profile evidence remain M3 gates. This
document distinguishes implemented commands from later contractual decisions. See the
[library's exact limits](../crates/aros-toolchain/README.md) and implemented
[command reference](../docs-site/src/content/docs/reference/cli.md).
[Epic #27](https://github.com/metaneutrons/aros-tools/issues/27) tracks delivery;
[M0 / #28](https://github.com/metaneutrons/aros-tools/issues/28) tracks this freeze.
The [delivery plan](toolchain-producer-plan.md) supplies scope and acceptance
gates. The [versioned contract](../contracts/toolchain-producer-v1.toml) owns
names, field sets and invariant values; this document owns their semantics.

## 1. Compatibility and identity decisions

Keep recipe v2, source lock v2, profiles v1, manifest v1 and consumer lock v1.
Do not add fields to those documents to describe a new executor. Instead, the
native recipe checkout must contain `toolchains/producer-executor-v1.toml`:

```toml
schema_version = 1
contract_id = "aros-toolchain-producer-v1"
contract_path = "contracts/toolchain-producer-v1.toml"
contract_sha256 = "<measured digest in the selected tools commit>"
tools_commit = "<exact selected tools commit>"
source_lock = "toolchains/llvm-11.0.0.sources.json"
profiles = "toolchains/profiles-v1.json"
```

The example is deliberately not executable: no fabricated release hashes.
All seven fields are required; unknown fields fail. Paths are canonical relative
paths within their declared root, without traversal. `contract_path` is relative
to tools; lock/profiles paths are relative to producer. Each selected file must
be committed material, a regular file and match its measured identity.

The existing recipe binds the declaration through `producer_commit/tree`, and
the actual implementation through `tools_commit/tree`. Check the declaration's
tools commit against the recipe and the executor, its contract digest against
the selected tools file, and lock/profile digests against the recipe. No new
copy of the source lock, profile matrix or workflow SHA belongs in tools.
Only conformance fixtures are explicitly synchronized. Land this declaration
in `aros-toolchains` when T1 exists (M2/M6); M0 does not fabricate future SHAs
or switch the current producer. The T1 -> P1/S1 -> release -> T2 sequence in
the delivery plan avoids a circular commit dependency.

Native compiler production and the collector use the same exact tools commit. The
executor's embedded identity plus measured binary hash is necessary but not
proof of origin: release jobs also bind a verified, credential-free source
build or trusted artifact evidence. A caller-provided digest is not an
attestation. Local runs lacking trusted origin evidence stay `local-only`.
The explicit legacy preview reports its real frontend identity separately;
it must not claim the old collector commit identifies the new frontend.
Verification, compatibility replay and packaging-only recovery may use a
reviewed newer harness. Their executor record identifies that actual harness;
artifact `tools_commit` continues to identify original production. Qualification
must bind both identities and the authorized harness-only change. Such a replay
does not rewrite the old recipe or establish an independent compiler build.

The consumer source-contract validator currently checks the source repository
and commit against two producer workflow scalars. At cutover extend it to
resolve the selected declaration, validate its contract/tools identities and
lock/profile paths, and assert equality with the recipe and workflow inputs.
Retain the old mode for old qualified producers; do not rewrite historical
consumer source contracts or accept multiple contradictory selectors.

## 2. Command contract

All new commands are added only with implementations and parser tests. No
success-returning placeholders. Existing install/list/verify/path behavior is
unchanged. `--format human|json` controls results on stdout; the existing global
`--diagnostic-format` controls errors. Help/version exit 0; every handled failure,
including argument errors, cancellation and timeout, exits 1, matching the CLI.
Signals that prevent a handler running naturally cannot promise JSON output.

`plan` and `build` require `--preset`, `--recipe`, `--source-dir`,
`--producer-dir`, `--tools-dir`. Optional common options are declared in the
contract. `build` additionally requires explicit work/output/cache roots,
positive `--jobs` and positive `--timeout-seconds`. There is no unmeasured job
or compiler-timeout default. The deadline covers the whole operation and is
propagated as remaining time to children. M1 evidence may justify a later,
reviewed default; nested build tools must not multiply the job budget.

`--backend native` is the default. It requires a selected committed
`producer-executor-v1.toml` whose contract, tools commit, source lock and
profiles bind exactly to the recipe. It then requires `--offline`, a verified
prepared cache, fresh work/output roots, positive jobs/deadline and the
frontend's internal verified MetaMake bridge. It runs only unchanged source
programs (`configure` and `crosstools-release`) inside the controlled Rust
lifecycle. `legacy-preview` remains explicit; never automatically choose it
after a native failure. Remove the historical adapter at M6 with documented
update guidance rather than keeping an indefinite second implementation.

`plan` may omit output/resource choices; their fields are null and readiness is
`incomplete`. It does not fetch, execute source scripts, bootstrap helpers,
create directories, reserve locks or mutate caches. Existing trusted host
tools may be probed with bounded version requests. Missing source capabilities
produce `blocked` readiness and findings without switching sources. Invalid or
unreadable arguments/contracts are errors, not successful plans. Readiness is
an inspection result, not a promise that an ensuing build will succeed.

Build revalidates every selection after acquiring locks. `--offline` (including
the existing `AROS_OFFLINE` policy) prohibits producer-controlled network use;
compilation always uses prepared, verified inputs. No new ambient source,
recipe, compiler-flag or credential overrides are introduced. The current CLI
exposes only `--resume-from compiler` for an interrupted local candidate. It
reacquires the exact private roots only after owner-marker validation,
remeasures the three retained snapshots and every compiler output, and validates
every predecessor receipt against recomputed inputs, identities and predecessor
digest. It then starts the collector in a fresh Cargo target directory. A
partial compiler tree, a completed collector receipt, any altered
receipt/input/output, and every other phase are rejected. Release jobs always
select fresh work/install roots and no compiled-object cache.

The implemented M1 preview accepts the explicit `build --backend legacy-preview`
surface. It probes `git`, Python, CMake, Rust/Cargo and make before reservation,
requires an existing cache directory, then uses fresh work/output leaves and
three metadata-free snapshots converted to independent shallow Git views. The
legacy driver receives only those views, a copied recipe, the selected lock and
profiles, and a sanitized child environment (`PATH`, private `HOME`/`TMPDIR`,
`LC_ALL=C`, `LANG=C`, `TZ=UTC`, offline Git/Cargo settings). The shared bounded
process runner propagates the explicit deadline and Ctrl-C cancellation and
reaps the process group. Partial or complete material is retained on every
failure. A successful local result measures regular output files and host-tool
versions, marks `qualification` as `local-only`, and records executor origin as
`not-run`; it cannot publish or satisfy release provenance.

The implemented M3 native path preserves the upstream build boundary rather
than translating it: after new ownership reservations and three isolated
committed snapshots, it rebinds the producer declaration, checks exact cached
sources, prepares the locked Python and Cargo environments, then invokes AROS
`configure` and its `crosstools-release` target. The only MetaMake download
entry is a hidden Rust bridge in the same `aros` binary; it resolves a declared
cache payload, records it in the durable source-use ledger, and only then calls
the unchanged upstream helper. The lifecycle builds the declared vendored
`aros-collect`, installs the collector aliases, removes producer-only LLVM
configuration inputs and writes an ordered canonical receipt chain. Child
processes use the one whole-operation deadline and process-group cancellation;
phase logs and all owned material are retained on failure. It has no package,
publication, attestation or release authority.

Low-level commands use `aros toolchain producer <operation>`. Common result and
diagnostic options apply to all. Required options are frozen as follows:

| Operation | Required options | Additional options / constraints |
| --- | --- | --- |
| recipe | source-dir, producer-dir, tools-dir, output | Native declaration selects lock/profiles; no allow-dirty |
| package | root, recipe, producer-dir, release-id, host, preset, output-dir, build-environment | Repeated forbid-prefix; derive canonical asset name, never caller-supplied |
| verify-archive | archive, recipe, producer-dir, tools-dir, host, preset | Repeated forbid-prefix; bounded extraction and selected compiled probes |
| compare | left, right, output-dir | Byte comparison; never imply independent builds without input evidence |
| index | directory, recipe, producer-dir, base-url | Final complete inventory only; before/after attestation are explicit `--stage pre-attestation|final` (required) |
| compatibility | archive, recipe, source-dir, producer-dir, tools-dir, work-dir, output-dir, jobs, timeout-seconds | offline; uses selected tools-owned engine and pinned upstream input |
| repackage | archive, recipe, producer-dir, source-release-id, release-id, output-dir, qualification-evidence | Packaging-only eligibility must verify; new ID cannot replace an existing output |

These commands delegate to the library. Prefetch, checkout checking, source-use
checking and host Python setup are phases, not additional public executables.
The MetaMake fetch bridge must use an implemented private entry point of the
same executable or an equivalent in-process integration; its quoting/protocol
is an M2 design gate, not an extra user-facing command promised here.
For the qualified LLVM-11 closure each upstream `%fetch` selects one `tar.xz`
candidate. The bridge therefore rejects fallback suffix lists before the
unchanged upstream helper can choose a cache object; a future multi-format
source contract requires a reviewed lock/schema extension and native update.
The selected Rust vendor tree must match the exact external closure of the
tools snapshot's `Cargo.lock`, including registry package checksums. A private
host-Python environment records its interpreter and every lock-verified package
identity; it never inherits site packages or invokes `pip`.

## 3. Versioned JSON documents

All field sets are in the TOML contract. Unknown fields fail except explicitly
open maps in existing v1 formats. No floats, duplicate keys, non-UTF-8 strings,
non-finite numbers or silent integer/bool coercion. Integer sizes/epochs are
unsigned 64-bit; jobs/deadlines must also be positive and checked for overflow.
Digests are lowercase 64-hex SHA-256; Git objects here are lowercase 40-hex.
Paths in local reports are absolute, canonical paths or null when unselected;
artifact/receipt output paths are safe relative paths within the owned root.

No document adds an implicit publication permission. Results and receipts are
outside the toolchain payload and immutable 56-file release inventory. Necessary
state is not a log: no wall-clock timestamps, environment dump or secret values.

### Shared records

- `identity`: recipe/source/producer/tools identities, native host and profile,
  plus `executor`. Host and profile must resolve through the selected profile
  and supported-host contract, not a second Rust list of profile recipes.
  Host/profile are null for matrix-wide recipe/index operations; otherwise
  both are required. They are never null in build phase receipts.
- `executor`: contract ID/digest, actual tools commit and executable digest;
  origin-evidence digest is nullable for local work, required for release proof.
  Legacy preview can have null contract fields but must identify the frontend.
  Read-only inspection may additionally report a null executor `tools_commit`
  when no verifiable frontend build metadata exists, **only with blocked
  readiness and an explicit finding**. Its measured on-disk executable digest
  is an observation, not in-memory origin proof. This plan-only clarification
  does not permit null executor commit identities in execution results,
  receipts or release evidence, or substitution of the old collector commit.
- `output`: relative path, `kind` (`file` or `tree`), SHA-256, size. File size is
  measured bytes; tree size is null and its digest uses the existing inventory
  algorithm. Output entries are unique and sorted by path.
- `evidence`: check name (`integrity`, `relocation`, `compatibility`,
  `independent-comparison`, `origin`), status (`passed`, `failed`, `not-run`),
  report digest (null only for not-run). Reports must be present and reverified;
  a boolean supplied by a caller never establishes eligibility.

### Plan: `aros-toolchain-plan-v1`

`operation` is `plan`; `backend` is the selected backend. `identity` uses the
shared record. `paths` contains source/producer/tools/work/output/cache roots;
only the last three may be null. `resources` contains jobs/deadline (nullable
when omitted), offline (boolean), network isolation (`fetch-guard` or a future
independently verified `os-enforced` backend), free bytes (nullable when not
measurable). Do not report an OS sandbox that has not been implemented.

`steps` is the ordered list of applicable phase names; preview uses the one
honest `legacy-driver` boundary, not inferred internal stages. `readiness` is
`ready`, `incomplete` or `blocked`. `findings` uses the existing diagnostic item
shape with warnings/errors and hints. A valid inspection may report blocked
readiness with exit 0; execution must refuse it. No filesystem reservation is
encoded in a plan. JSON result is one complete document plus newline.

The legacy M1 inspector returns `blocked` for valid inspected inputs:
it checks root commit/tree identities, selected raw committed metadata, root
overlaps and complete recursive raw worktree/index material. The latter rejects
dirty, ignored, untracked, missing, wrong-mode and changed/uninitialized
submodule inputs without filters, source execution, fetching or cleanup.
This read-only comparison is not an isolated snapshot, an independent Git
object-database integrity audit or trusted origin proof; it cannot freeze a
concurrently mutable checkout. Source capabilities, lock semantics,
prerequisites/cache and executor/build lifecycle remain gates.
`network_isolation: fetch-guard` is the selected future legacy policy, not an
active OS isolation claim. Native selection reads only the matching committed
declaration and its bound inputs; it never guesses a declaration or driver from
historical producer state. Native readiness is `incomplete` when required
roots/resources are omitted and `ready` only after complete declaration-bound
inspection. Cache byte verification and reservations remain lifecycle work.
Legacy recipes lack lock paths: inspection requires exactly one direct
committed `toolchains/*.sources.json` matching the recipe digest, and uses
the historical `toolchains/profiles-v1.json` path. It does not pin an LLVM
version or accept an uncommitted lock/profile copy. Native declaration-based
resolution remains M2; full snapshot/source validation still gates execution.

### Result: `aros-toolchain-result-v1`

`operation` names the completed build or maintainer operation. `output_root` is
the canonical absolute root for output references (null only with no outputs).
`outputs` and
`evidence` contain shared records. `qualification` is `local-only` or
`qualification-candidate`; never `released`. A build alone is local-only even
when it passes its own integrity checks. The protected workflow must assemble
and independently validate complete matrix evidence before promotion.
`commit_state` uses the shared enum and must be `committed` on successful
mutating completion (null for read-only operations). Failures produce no success
result on stdout; their diagnostic includes commit state and retained evidence
path. A committed output is not deleted because logging/fsync/reporting failed.

### Receipt: `aros-toolchain-receipt-v1`

`phase` names a completed phase. `input_sha256` binds the effective stable
phase-input document, including resolved build tools/SDK/flags, recursive
source snapshots, contract/recipe and predecessor outputs. `output_root` is the
canonical absolute owned root; `outputs` records measured owned outputs.
`previous_receipt_sha256` is null only at the first
phase. `receipt_sha256` hashes the complete record excluding itself.

Canonical digest encoding preserves the existing Python `json_bytes` rule:
sorted keys, unescaped UTF-8 strings, separators `,` and `:`, no insignificant
whitespace and exactly one final LF. Only the supported integer/string/bool/null
domain is allowed. Native serialization must pass the captured UTF-8 vector;
ordinary pretty-printed JSON is not interchangeable for hashing.

Receipts are crash-consistency aids, not signed attestations. Recheck input and
output digests, ownership and predecessor links before reuse. The current
implementation permits that only after the complete `compiler` receipt and
only to rerun the collector with a fresh Cargo target tree. Never resume an
incomplete make directory or use local receipts as independent A/B proof. A
truncated receipt is retained as evidence, not silently repaired into success.

## 4. Parser decisions and package vectors

The portable [negative cases](../scripts/fixtures/toolchain-producer/manifest-cases.json)
record old producer behavior separately from the native requirement. They are
acceptance inputs for M2/M4, not a second production validator. Source inspection
also finds the following differences; the stronger applicable contract wins:

| Boundary | Current difference | Native decision |
| --- | --- | --- |
| Manifest extra top-level keys | Python/schema permit; Rust denies | Reject; version a real extension |
| Whitespace-only capabilities | Python permits; Rust denies | Reject; also forbid padded identifiers |
| Host/profile/triple pairing | Python checks exact mapping; Rust manifest checks generic names | Preserve producer mapping plus consumer validation |
| Release ID path safety | Python requires nonempty; Rust restricts a segment | Safe single segment, no traversal/credentials/control characters |
| Inventory numeric size | Manifest Python/Rust reject bool | Preserve this; source-lock Python has weaker integer checks |
| Source archive names/URLs | Source-lock schema/Python/Rust fetch differ | Strict basename, credential-free HTTPS, no dot segments; reject bool sizes |
| Recipe unknown keys | Legacy self-hash permits extra keys | Closed v2 keys; new semantic fields require a new schema |
| Index vs consumer lock | Index carries three commit fields; Rust lock denies them | Distinct parser types; validate bindings before explicit consumer projection |
| Archive/release filesystem | Legacy checks are not the complete safety model | Bounded read-back, unique names, contained links; outer assets use lstat regular-file checks |

Do not loosen Rust `deny_unknown_fields` or call the release index a directly
loadable consumer lock. No production parser is changed by M0. The archive
vector captures legacy PAX/XZ encoding on a tiny synthetic payload (not runnable
compiler output); M4 must compare native encoding and validate malformed archive
cases. Reuse the existing tree fixture rather than creating a third inventory
algorithm or copying profile/source locks into this repository.

## 5. Ownership inventory

This inventory covers the exact baseline producer commit recorded in the plan.
Names below are the complete top-level Python function inventory, not just
entry points. Scripts remain active until replacement parity and M6 cutover.

| Current owner / symbols | Destination |
| --- | --- |
| producer.py: fail, parser, main | CLI adapter plus shared diagnostics |
| json_bytes, read_json, sha256_file, files_equal | Shared canonical/hash primitives only where semantics are identical |
| canonical_asset_name, validate_manifest, required_paths, profile_by_name | toolchain contract; existing common manifest types plus producer validation |
| validate_source_lock, validate_host_python_packages, validate_recipe | toolchain contract/identity; source schemas remain in producer repository |
| verify_source, command_prefetch, command_verify_source_usage | toolchain sources through aros-fetch; exact source-use ledger |
| git, repository_identity, command_recipe, command_verify_checkout | toolchain identity; shared checked process runner |
| normalized_mode, normalize_tree, scan_prefixes, tree_inventory, add_tar_entry, write_spdx, command_package | toolchain package; extract the CLI's existing tree digest into common at M4 |
| safe_extract, executable, run_probe, verify_tree, command_verify | toolchain verify through existing safe archive/process primitives |
| command_compare, command_index | toolchain qualification/inventory |
| command_repackage | toolchain recovery; policy authorization stays in workflows |
| record-qualification | bounded final-inventory measurement plus complete 4×3 receipt binding; no attestation verification or release authority |
| prepare-recovery / validate-recovery | typed recovery request from externally verified claims, re-observed tags and isolated final inventory; no network or publication authority |
| build-release.sh: usage, embedded version probe and full shell body | CLI options; environment observations; toolchain driver/state/package |
| offline-fetch.py: die, value and module body | toolchain source bridge; upstream fetch dependencies remain declared |
| host-python-env.py: die, _inside, _safe_extract, _prepare_package, _verify_runtime, main | toolchain environment using fetch/extraction primitives; Python runtime remains |
| compatibility.sh: full shell body | toolchain qualification using selected tools-owned CMake engine |

Workflow call-site destinations (including inline policy, not merely scripts):

| File / jobs | Migration owner |
| --- | --- |
| ci.yml contract test | Fast native contract/fixture gate; keep before expensive builds |
| toolchain-release.yml contracts: recipe, schema checks, producer test | Native recipe + tools capability/source contract validation |
| toolchain-release.yml prefetch / source cache | Native verified preparation; shared immutable sources only |
| toolchain-release.yml build: build-release.sh | Same CLI/library as local build, isolated fresh A/B roots |
| toolchain-release.yml compare / compatibility | Native compare / compatibility; provenance of independent inputs required |
| toolchain-release.yml draft-release: support copies, index (twice), attestation, upload | Index library; GitHub identity, OIDC and promotion remain protected workflow responsibilities |
| toolchain-compatibility-replay.yml compatibility | Native harness, exact verified existing archive input; no compiler rebuild implied |
| toolchain-release-recovery.yml source-run eligibility, attestation verification, recovery request, repackage, index (twice), tag checks | Native recovery/verification; only an already persisted 56-member candidate plus 4×3 qualification record can be reused; GitHub attestation and immutable-tag observation remain protected workflow responsibilities |

Fixtures and rule ownership:

- `tests/test-producer.sh`: recipe/checkout/lock failures, mock package,
  compare/index/repackage, inventory, relocation, corruption and prefix checks
  migrate to maintained Rust/unit/integration fixtures in M2–M5. Keep baseline
  replay until replaced, not as release-time Python orchestration.
- `tests/mock-tool.sh`, `fixtures/smoke.c`, `fixtures/smoke.cpp`: deterministic
  process tests and actual C/C++ probes; mocks are never board/runtime proof.
- `tests/test-host-python-env.py`, `test-llvm-patch.py`,
  `test-crosstools-release.py`: retain as reference until native equivalents
  cover private Python, patch applicability and exact MetaMake release closure.
- `llvm-11.0.0.sources.json`, `source-lock-v2.schema.json`, `profiles-v1.json`,
  `rust-toolchain.toml`: reviewed producer inputs; remain in `aros-toolchains`.
- `toolchain-manifest-v1.schema.json`, `tree-digest-v1.fixture.json`: published
  conformance material. The existing tools fixture is explicitly synchronized;
  tighten the schema at M4 without changing valid historical manifest meaning.
- AROS configure/MetaMake, compiler patches and `crosstools-release`: stay in
  AROS. Toolchain profile mapping and pristine-upstream comparison commit stay
  in producer profiles; the native code interprets them, not duplicates them.
- Tools Cargo/Rust locks and consumer contracts stay in tools; collector and
  compatibility helper selection use the exact recipe tools commit.

## 6. Trust, state and error decisions

| Threat / failure | Required boundary and evidence |
| --- | --- |
| Dirty/ignored files, git filters, uninitialized or changed submodules | Inspect all roots; execute only snapshots of committed blobs and exact recursively recorded submodules. Missing objects block; no source cleanup. Compare snapshots before/after execution. |
| Source build code can run arbitrary programs | Explicit trusted source input; credential-free build jobs; fetch guard is not a sandbox. No source scripts during plan. |
| Root aliasing, nested symlink escape, concurrent output | Canonicalize selected roots, reject overlapping source/work/cache/output ownership, no-follow checks below roots, locks and revalidation; same-filesystem no-clobber publication. |
| Poisoned cache / missing offline object | Check size/digest at every use; atomic cache insertion; reject mismatch, never learn a hash or fall back to network offline. |
| Mutated compiler/SDK/Python/Cargo environment | Resolve and record real output-affecting inputs; private Python/Rust vendor trees; reject inherited unrecorded flags and never install host packages automatically. |
| Timeout, cancellation, broken pipe, disk full | Shared bounded process groups/deadlines; typed cause; retain owned evidence; no silent retry or blind recursive cleanup. |
| Post-rename durability/log failure | Report committed/indeterminate state; preserve valid output; no success document or false rollback. |
| Forged receipt or compatibility report | Revalidate all references; receipts alone cannot prove origin, independence or release eligibility. |
| Publication token leakage / partial matrix | Build library has no publishing capability; protected jobs revalidate exact inventory and signer evidence before exposure. |

Reserve `AX` for producer diagnostics. M2 registered AX0101, AX0102, AX0201,
AX0202, AX0301 (verified sources), AX0302 (source-use closure), AX0401
(controlled external environment) and AX0801 (work ownership). M3/M4 added the
build, package, verification, inventory and comparison codes; M5 now registers
AX0703 for compatibility qualification and AX0901 for recovery eligibility.
All other reserved codes remain unavailable until their implementations land.
Existing nested AF/AC diagnostics retain their codes in one shared failure
envelope. Use existing `DiagnosticContext` fields; richer producer identity,
digest and retry classifications go in typed evidence and safe messages/hints,
not undocumented additions to the shared JSON envelope.

| Codes | Meaning |
| --- | --- |
| AX0001 / AX0002 | Invocation / observability |
| AX0101 / AX0102 / AX0103 | Contract / identity / missing source capability |
| AX0201 / AX0202 | Host prerequisite / root and resource preflight |
| AX0301 / AX0302 / AX0401 | Verified sources / source-use closure / environment |
| AX0501 / AX0502 / AX0503 | Configure / compiler-runtime build / collector |
| AX0601 / AX0602 | Package / integrity verification |
| AX0701 / AX0702 / AX0703 | Release inventory / byte comparison / compatibility-relocation |
| AX0801 / AX0802 | State-receipt / durable publication |
| AX0901 / AX0999 | Recovery eligibility / internal invariant |

TCP-M5 adds the closed, offline
[qualification-evidence contract](toolchain-qualification-evidence.md). It
binds a measured index, source run, signer claim and lane reports before a
compatibility or recovery operation may rely on them. It does not parse or
replace the external cryptographic attestation verifier, and it has no release
or credential authority.

Use existing semantic stages such as configuration, integrity_validation,
build_execution, archive_packaging and release_integrity; phase identity belongs
in producer evidence. Only classified transient fetch failures may retry within
the total deadline. Missing logs are never linked as if written. Logging remains
opt-in, local and redacted. There is no remote telemetry.

## 7. Evidence and remaining gates

See [M0 acceptance evidence](toolchain-producer-baseline.md#m0-acceptance).
These are fixture and source-review findings, not a new toolchain qualification.
M0 through M4 are accepted; their linked evidence records implementation and
explicit limits. TCP-M5 is in active implementation. Do not activate an
official producer declaration or close a later milestone solely because local
lifecycle, package or synthetic compatibility fixtures exist. The native recipe
input/canonical output cap is 1 MiB and canonical nesting is limited to 64
levels. These are parser safety limits, not measured compiler resource defaults.

The subsequent lower-level guard slice adds operation-local shared cancellation
and fresh advisory-locked work/output leaves with explicit ownership revalidation.
It adds no CLI command, recipe/origin authority, snapshot, adapter or phase-resume
support. Internal `.aros-toolchain-owner-v1.json` markers bind an operation digest
and role, not build receipts; they never permit adoption of an existing directory.
Parents must already exist. A partial reservation is retained on failure, and
`release()` explicitly unlocks both held directory descriptions and returns
AX0801 on failure; it must follow completion of all child activity. Drop performs
fallback unlocking with tracing on failure, then closes descriptors, never
deletes data. A duplicated/inherited descriptor cannot substitute for the
owner's explicit unlock, and CLOEXEC is not close-on-fork. An inherited guard
must not unlock its parent's description; only the creating process releases
the lock. The caller must finish all readiness gates and
revalidate at execution boundaries before using these mechanisms together.

The source-material slice adds `SourceSnapshot`, an in-process guard borrowing
work ownership. It preflights one selected checkout and recursively copies only
raw committed blobs into fresh role-specific staging, without Git metadata,
filters or hardlinks. The shared verifier checks exact material before/after
durable no-clobber publication and at explicit reuse boundaries. Committed links
must resolve inside that flattened input; dangling, escaping and cyclic links
are rejected before any link is created. Failures retain evidence. Each role is
independent, not a three-root transaction, phase receipt or execution permission.
See the [snapshot limits](../crates/aros-toolchain/README.md#isolated-source-material-primitive).

The unchanged legacy driver additionally requires real Git HEAD/tree/status
queries in all three roots. Raw snapshots do not supply that interface. The
[isolated execution view](toolchain-legacy-execution-view.md) consumes one raw
snapshot and reconstructs independently verified shallow stores for every
recorded repository. Raw bytes and every generated metadata byte remain guarded;
there is no metadata exemption or copied user configuration. This library-only
conversion does not enable an adapter or compiler execution.

Remaining implementation gates include: private MetaMake fetch integration,
trusted executor evidence, integrated snapshot/adapter lifecycle, actual four-host process
and filesystem behavior, xz-library byte parity, archive resource limits, exact
native source-lock/parser fixtures, tools-owned compatibility engine and
measured CPU/RAM/storage/time defaults. None is waived by a specification test.
