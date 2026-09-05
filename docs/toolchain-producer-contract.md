# Native toolchain producer contract (TCP-M0)

Status: specified, not implemented; awaiting architecture-PR review. This is
engineering documentation, not a list of commands available to users.
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

`--backend native` is the eventual default. Before native support exists,
building requires explicit `--backend legacy-preview`; unsupported selections
fail before side effects. Never automatically choose an old script after a
native failure. Remove the experimental adapter at M6, with documented update
guidance, rather than keeping an indefinite second implementation.

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
recipe, compiler-flag or credential overrides are introduced. `--resume` is
explicit, local-only and rejects foreign or invalid receipts. Release jobs
always select fresh work/install roots and no compiled-object cache.

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
output digests, ownership and predecessor links before reuse. Never resume an
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
| toolchain-release-recovery.yml eligibility, repackage, compare, index (twice), attestation, tag checks | Native recovery/verification; sole-packaging-failure authorization and immutable tag checks remain workflow-owned |

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

Reserve `AX` for producer diagnostics; add its enum variants in M1, not M0.
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
| AX0701 / AX0702 | Compatibility-relocation / byte comparison |
| AX0801 / AX0802 | State-receipt / durable publication |
| AX0901 / AX0999 | Recovery eligibility / internal invariant |

Use existing semantic stages such as configuration, integrity_validation,
build_execution, archive_packaging and release_integrity; phase identity belongs
in producer evidence. Only classified transient fetch failures may retry within
the total deadline. Missing logs are never linked as if written. Logging remains
opt-in, local and redacted. There is no remote telemetry.

## 7. Evidence and remaining gates

See [M0 baseline evidence](toolchain-producer-baseline.md). These are fixture and
source-review findings, not a new toolchain qualification. M0 is complete only
after this architecture/contract PR is reviewed and merged with green gates.
Do not close M1–M7 or activate native producer declarations on that basis.

Remaining implementation gates include: private MetaMake fetch integration,
trusted executor evidence, recursive clean snapshots, actual four-host process
and filesystem behavior, xz-library byte parity, archive resource limits, exact
native source-lock/parser fixtures, tools-owned compatibility engine and
measured CPU/RAM/storage/time defaults. None is waived by a specification test.
