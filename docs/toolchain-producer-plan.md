# Toolchain producer integration: architecture and delivery plan

Status: implementation authorized; no producer milestone is complete yet.
Planning baseline: 2026-09-05. Maintainer and scope owner: Fabian Schmieder.
Tracking prefix: `TCP`. The acceptance gates below define completion, not a
percentage estimate. Command examples describe the proposed interface, not
commands available in the current release.

Execution is tracked in [epic #27](https://github.com/metaneutrons/aros-tools/issues/27).
The first active work package is [TCP-M0 / #28](https://github.com/metaneutrons/aros-tools/issues/28).
Its reviewable [interface and ownership contract](toolchain-producer-contract.md)
and [measured baseline](toolchain-producer-baseline.md) are now recorded. M0's
merge/acceptance status remains on its issue; later milestones have not started.

## 1. Decision and intended outcome

Integrate toolchain production into `aros-tools`, retaining `aros-toolchains`
as the independently versioned recipe, qualification and distribution
repository. A developer and a CI runner must execute the same producer code.
The compiler/runtime build rules remain in AROS; their translation to our
CMake engine is not part of this migration.

The intended user experience is:

1. select explicit AROS sources and an immutable producer recipe;
2. inspect requirements, identities, paths and expected work before building;
3. build one host/profile toolchain without a GitHub account or write token;
4. verify and use the resulting local prefix through existing CLI commands;
5. optionally qualify and release a complete matrix through the separate,
   protected toolchain workflow.

A successful build is not an attestation, an independent reproduction, a
complete application SDK or evidence that a physical board boots.

### Scope boundaries

- Support the currently qualified four native hosts: Linux x86-64/AArch64 and
  macOS x86-64/AArch64. Build for the running host; do not introduce host
  cross-compilation, remote execution or emulation.
- Preserve the three release profiles: `pc-x86_64`, `arm-raspi` and
  `rpi-aarch64`. The PC profile includes its existing i386 runtime/collector
  contract. RISC-V and new LLVM versions require separate qualification work.
- Preserve AROS `configure` followed by `make crosstools-release`, existing
  patches and the declared source closure. No compiler recipe rewrite.
- Keep ordinary downloaded-toolchain installation as the default user path.
  Never fall back to an expensive source build when a download fails.
- Do not merge repositories, move historical releases, retarget tags, change
  package endpoints, modify credentials or change repository governance.
- The normal tools suite keeps its current executable inventory. Add a
  library crate, not a second public frontend or compatibility aliases.
- This plan does not make producer integration a prerequisite for the first
  `aros-tools` release. Release the existing qualified scope independently;
  let Release Please determine the version of the later feature release.
- Remove our superseded orchestration scripts at cutover, not upstream's
  legitimate shell/Python build dependencies or maintained regression tests.

## 2. Verified baseline and migration constraints

The baseline was checked against executable source and workflow contracts,
not inferred from README claims. Commit references here are historical
evidence, not a new runtime pinning mechanism.

| Component | Inspected identity | Relevant implementation |
| --- | --- | --- |
| `aros-tools` | `39a7738d7b7d09d96fad6787afa45823629ea77c` | [CLI commands](https://github.com/metaneutrons/aros-tools/blob/39a7738d7b7d09d96fad6787afa45823629ea77c/crates/aros-cli/src/main.rs), [consumer verification](https://github.com/metaneutrons/aros-tools/blob/39a7738d7b7d09d96fad6787afa45823629ea77c/crates/aros-cli/src/toolchain.rs) |
| `aros-toolchains` | `c8039cf2b7291097ad62c6750bd7367e91a068f4` | [build driver](https://github.com/metaneutrons/aros-toolchains/blob/c8039cf2b7291097ad62c6750bd7367e91a068f4/scripts/toolchain/build-release.sh), [package/verify engine](https://github.com/metaneutrons/aros-toolchains/blob/c8039cf2b7291097ad62c6750bd7367e91a068f4/scripts/toolchain/producer.py), [workflow](https://github.com/metaneutrons/aros-toolchains/blob/c8039cf2b7291097ad62c6750bd7367e91a068f4/.github/workflows/toolchain-release.yml) |
| AROS producer input | `f3cfc243a84065166a46da28b0a5b22bbd0f8869` | Selected by the producer and [consumer source contract](../contracts/aros-source-v1.toml); it is not a moving `main` |
| Tools input of that producer | `707037be4f8ff37300a1a89166c35f661c28bafe` | Collector and compatibility helpers; distinct from current tools `main` |
| Published standalone baseline | `toolchain-v1-20260831-rc3` | [Immutable prerelease](https://github.com/metaneutrons/aros-toolchains/releases/tag/toolchain-v1-20260831-rc3), published 2026-08-31 |

Current code provides the following reusable boundaries:

- [`aros-common`](../crates/aros-common/README.md): typed hashes, manifest
  types, diagnostics, opt-in observability, bounded subprocess handling and
  durable no-clobber publication.
- [`aros-fetch`](../crates/aros-fetch/README.md): download, extraction,
  patching, offline and checksum-policy behavior with an explicit error model.
- [`aros-release`](../crates/aros-release/README.md): native tools-suite
  packaging. Its fixed eight-binary `.tar.gz` contract is **not** the
  symlink-bearing toolchain `.tar.xz` contract.
- [`aros-cmake-engine`](../crates/aros-cmake-engine/README.md): the tools-owned
  consumer engine; compatibility testing must use it explicitly.

The old producer separately owns recipe creation, an offline-fetch guard,
private Python runtime setup, compiler/collector production, package
normalization, inventory/SBOM generation, comparison and compatibility replay.
Each responsibility needs an explicit destination before removing its script.

Baseline discrepancies to resolve during the migration:

- Producer README/handoff text still describes an earlier migration/release
  state. Update it against measured standalone evidence, not by copying this
  plan's future-state claims.
- The current compatibility script locates CMake modules inside the AROS
  checkout. That assumption must be removed before qualifying sources without
  the copied engine; [AROS-NX PR #28](https://github.com/metaneutrons/AROS-NX/pull/28)
  was still open at planning time.
- The existing fetch guard validates a cache hit and then invokes upstream
  `fetch.sh`. This is not an OS-level network sandbox.
- Recipe generation and execution have different dirty/untracked-file checks.
  A new producer must specify its complete material-input policy rather than
  inherit that difference accidentally.

## 3. Ownership, dependency direction and bootstrap

### Target ownership

| Owner | Responsibilities | Must not own |
| --- | --- | --- |
| `aros-cli` | Parse commands, select explicit inputs, render results/diagnostics, cancellation | Compiler rules, package schemas duplicated in handlers, publication credentials |
| New `aros-toolchain` library | Typed plans/recipes, producer phases, source-use accounting, toolchain envelopes, qualification reports | Global CLI state, arbitrary recipe shell evaluation, GitHub release promotion |
| `aros-common` | Identical low-level contracts needed by multiple components | Toolchain release policy or profile-specific workflow decisions |
| `aros-fetch` | Verified transport/extraction/patch primitives | A second, independently maintained source-lock authority |
| `aros-release` | Existing tools-suite distribution contract; consume any genuinely shared extracted primitives | Reinterpreting its native-suite manifest as a toolchain manifest |
| `aros-toolchains` repository | Reviewed recipes/locks/profile matrix, workflow policy, release assets and evidence | A second implementation of native producer algorithms |
| AROS / AROS-NX | Configure/MetaMake compiler and runtime rules, source patches | Tools-owned CLI, embedded consumer engine or release-service credentials |

Dependency direction is `aros-cli -> aros-toolchain -> aros-common/aros-fetch`.
`aros-toolchain` must not depend on `aros-cli` or the product-specific
`aros-release` package API. Extract archive primitives into `aros-common` only
where two real consumers require identical semantics and regression tests
prove both contracts remain intact. Similar-looking code is not sufficient
reason to introduce a generic packaging framework.

Keep producer modules focused: `contract`, `plan`, `identity`, `sources`,
`environment`, `driver`, `state`, `package`, `verify`, `qualification`,
`recovery`, and a small diagnostics adapter. These are boundaries, not a
requirement to create empty modules up front. Keep the CLI consumer
installation path independent from the new producer dispatch.

### Bootstrap and version identity

- The producer is a native Rust tool built with the selected host Rust/C
  environment. Building it must not require the AROS toolchain it produces.
- The first integration can build the exact tools commit from source; a
  published binary of that commit is not a bootstrap prerequisite.
- `tools_commit` continues to identify the selected tools source; in the
  native path it must identify both the actual producer implementation and
  the collector input. `producer_commit` remains the recipe/workflow checkout
  in `aros-toolchains`. Neither field is silently repurposed.
- An installed CLI's SemVer alone is insufficient proof of executor identity.
  Define verifiable build identity for release executors. If the executable
  does not match the recipe, fail with the expected/actual identity and exact
  bootstrap instructions. Never claim that a newer executable ran an older
  tools commit, or silently download and execute a different producer.
- Release executors are built in credential-free jobs from verified source
  snapshots and locked dependencies, with their measured binary digest bound
  to qualification evidence. A prebuilt executor needs matching trusted
  artifact evidence; an embedded commit string or caller-supplied digest alone
  does not establish its origin.
- Reuse the existing Rust/Cargo lock contracts. CI must vendor and verify all
  dependencies needed by the selected producer and collector, not only the
  old collector subset. `cargo --locked` alone is not offline; the compile
  phase must also use offline resolution with prepared inputs. See the
  [Cargo build contract](https://doc.rust-lang.org/cargo/commands/cargo-build.html).
- A recipe's supported schema/capabilities are checked before compiling.
  Unknown required fields/features fail with update guidance. Introduce a
  versioned format when needed; do not loosen old parsers to accept ambiguity.

Avoid a reciprocal commit-pin cycle during rollout:

1. merge producer implementation at tools commit `T1`, retaining the previous
   qualified consumer contract;
2. merge producer configuration `P1` selecting `T1` and qualified AROS `S1`;
3. qualify and publish the immutable toolchain built from `P1/T1/S1`;
4. in tools commit `T2`, promote the measured release and update the consumer
   source contract to `P1/S1`;
5. release `T2` through the established tools workflow. Do not rewrite `T1` or
   `P1` to manufacture mutual references to each other's future hashes.

`T2` consuming that release does not make it an exact `T1` release executor.
Future source builds either use the exact selected executor or record a new
recipe identity; they never substitute one invisibly.

## 4. Proposed command and source contracts

### User-facing surface

Add `aros toolchain plan` and `aros toolchain build`; preserve the meaning of
`install`, `list`, `verify` and `path`. No aliases and no automatic installation
or release after a build.

Illustrative interface, detailed by the TCP-M0 contract (not yet implemented):

```text
aros toolchain plan --preset pc-x86_64 --recipe /work/recipe.json \
  --source-dir /work/AROS --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools

aros toolchain build --preset pc-x86_64 --recipe /work/recipe.json \
  --source-dir /work/AROS --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --work-dir /work/build-pc \
  --output-dir /work/candidate-pc --cache-dir /work/source-cache \
  --jobs 8 --timeout-seconds 21600 --offline
```

The example deadline is a caller-selected budget, not a measured default.
The first preview additionally requires explicit `--backend legacy-preview`.

- Inputs are explicit; execution works from outside every checkout. Reject
  conflicting CLI/recipe selections. Do not infer a neighboring repository,
  use a moving branch as an immutable recipe, or read a user shell rc file.
- The recipe selects locks/profiles by their recorded identities in the
  producer checkout. Relative declared paths are anchored to their declared
  root, never accidentally to the invocation's working directory.
- `plan` performs bounded, non-mutating inspection: selected identities,
  host/profile, capability checks, external prerequisites, cache misses,
  destination layout, resource settings and planned phases. It does not
  fetch, bootstrap, reserve directories, install prerequisites or execute
  source-provided build code. It may probe trusted host tools' versions.
- `build` produces a verified local candidate and a versioned result reporting
  its actual qualification level. Local paths can then be passed to the
  existing `toolchain verify --local` and `build --toolchain-dir` commands.
- The build result identifies `built`, `verified` and any independent
  comparison/compatibility evidence separately. No user-supplied release ID
  or CLI flag grants publication eligibility.
- A per-command `--format human|json` result option is proposed independently
  of the existing global `--diagnostic-format`; the latter continues to
  control failures on stderr. Freeze names, schemas and exit behavior before
  advertising the new commands.
- Keep low-level maintainer operations under one explicit
  `aros toolchain producer` namespace: recipe, package, verify-archive,
  compare, index, compatibility and repackage. Add them only as their native
  implementations become available; do not expose stub success paths.
  Both local/CI `build` and these stage operations call the same library.

Recipe creation is deliberate and separate from executing an already recorded
recipe. The first native release supports clean, explicitly selected commits.
Dirty-source snapshots and arbitrary local patches are a later feature unless
their complete material identity and non-release classification are designed
and tested first. There is no general `--allow-dirty` release escape hatch.

### Upstream compatibility is a capability check

An explicit pristine-upstream checkout is a valid input to inspection, but it
is not automatically a qualified source for the release-specific graph.
Check the actual configure/MetaMake capabilities and required declared patches.
If the selected source lacks them, report the missing contract and an explicit
supported path. Do not silently patch the checkout, switch to AROS-NX, select
another commit, or replace `crosstools-release` with a broader target.

Keep three claims separate in tests and documentation: the tools can inspect
upstream sources; a source supports the producer recipe; an already-built
toolchain works with the pinned pristine-upstream consumer. Supporting the
third does not prove the second for every upstream revision.

## 5. Security, reproducibility and operational contracts

### Inputs, supply chain and environment

- Preserve exact commit/tree identities, recursive submodule identities,
  lock/profile digests and declared patch hashes. Hash regular files through
  safe paths and audit source identities again after execution.
- Release material must exclude undeclared tracked, untracked and ignored
  files. Prepare an isolated clean execution snapshot where necessary;
  transport caches belong outside source roots. The user's checkout is never
  a cleanup target or a scratch area for configure-generated files.
- Producer source archives and host-runtime dependencies require reviewed,
  measured size/digest entries. A mismatch or newly reached source is fatal.
  Never learn a trusted checksum from a failed download or add one silently.
  Ordinary application/port-fetch checksum policy is unchanged.
- Every build-consumed source must be in the verified lock; the observed
  source set must equal the declared build closure. Account separately for
  host Python packages, Rust vendor inputs and declared patches. Report both
  undeclared usage and obsolete lock entries.
- Resolve explicit host tools, sanitize inherited compiler/Cargo/Python
  settings, set controlled locale/timezone/umask and record effective build
  flags. Reject unrecorded release-affecting overrides. Never run `sudo`,
  `brew install`, `apt install` or `pip install` inside the producer.
- Keep private host Python modules isolated from host site packages. Porting
  the environment launcher does not mean reimplementing Mako in Rust.
- Preserve prefix remapping, archive ordering/modes/owners and the existing
  recipe-derived `SOURCE_DATE_EPOCH` rule. A migrated executor may change
  recipe identity/epoch, so old and new release hashes need not match. The
  [SOURCE_DATE_EPOCH convention](https://reproducible-builds.org/docs/source-date-epoch/)
  controls a timestamp input; it does not itself prove reproducibility.
- Record stable output-affecting environment choices in the build contract.
  Keep runner observations, wall-clock timings and run IDs in separate
  evidence, outside byte-compared payloads. Do not hide a changing SDK/compiler
  input merely by calling it an observation. Document the reproducibility
  envelope; this migration does not make moving host package repositories
  historically reconstructible.

### Offline and execution trust

Separate prefetch from compilation. In offline mode, all producer-controlled
transport must reject network access and fail on a missing or modified cache
object; compiler/collector production uses prepared inputs only. Adapt the
MetaMake fetch entry point to the same verified fetch implementation without
constructing shell commands from untrusted strings.

AROS build rules execute code with the invoking user's privileges. A hash
establishes identity, not benign behavior. A fetch-level guard or sanitized
environment is not a security sandbox. Report the actual isolation level;
claim OS-enforced network isolation only on backends that implement and test
it. Run release builds with read-only repository permissions and without
publication/signing credentials. Fork/PR builds must not gain OIDC or secrets.

### Lifecycle, cancellation and filesystem safety

Use explicit, validated phase transitions:

```text
inspect -> preflight -> prepare sources -> configure -> build runtimes/compiler
        -> build collector -> normalize/package -> verify -> local candidate

local candidate -> independent comparison + compatibility -> release candidate
                -> protected publication (workflow-owned, not build-owned)
```

- A read-only plan is not a reservation. Acquire the work/output locks and
  revalidate paths, identities and free-space requirements before mutation.
- Work, source, cache and output roots must have a validated non-overlapping
  ownership model. Support explicitly selected symlinked root paths by
  canonicalizing them, while rejecting traversal or symlink escapes below
  protected roots. Test spaces, UTF-8 and case-sensitive/case-folding behavior.
- Write receipts/checkpoints only at validated phase boundaries. They bind
  the recipe/executor/host/profile, phase inputs and verified outputs; stale,
  malformed or tampered receipts never allow a phase to be skipped.
- The first release supports reuse of immutable source caches, not incremental
  release compilation. Release A/B work and install roots are always fresh.
  Developer resume is explicit and may restart an incomplete phase; a failed
  make directory is not automatically a valid compiler checkpoint.
- Route subprocesses through shared process primitives with process-group
  cleanup, bounded diagnostics and explicit deadlines. Distinguish cancellation,
  timeout, signal termination, spawn failure and non-zero exit. No blind retry
  of compiler, verification or publication failures.
- Validate positive job limits and propagate them to nested Make/CMake/Cargo
  phases. Avoid accidental nested oversubscription. Measure memory, storage and
  time on representative lanes before setting resource defaults; do not invent
  a fixed ETA or assume all hosts have the same capacity.
- Retry only bounded, classified transient source-transport failures, retaining
  TLS/integrity checks. Cache hits are reverified, not trusted by filename.
- Publish complete candidate directories through the shared no-clobber,
  same-filesystem staging contract. If post-rename durability is uncertain,
  retain the committed output and return its uncertain state; never report a
  rollback or delete the output as cleanup.
- Failed runs retain attributable evidence. Cleanup can remove only exact
  producer-owned entries whose ownership still verifies; no recursive cleanup
  of a checkout, shared cache, volume or arbitrary output parent.

### Diagnostics and logging

Use the existing `aros-tool-diagnostics-v1` envelope and shared logger/process
boundaries. Reserve a distinct producer diagnostic family in TCP-M0 after
checking all existing code assignments; do not duplicate/reuse `AT`, `AC`,
`AF`, `AR` or `AP` meanings.

Every failure must identify the operation/stage, stable code, safe context,
underlying cause, and actionable next step. Include expected/actual identities
or digests where relevant, process exit/signal/timeout metadata, retryability,
and publication state. Attach an existing log/evidence path when available;
never promise a log file that was not written. Preserve nested fetch/collector
diagnostics rather than reducing them to an undifferentiated CLI error.

- In JSON diagnostic mode, stdout is reserved for the result contract and
  stderr for exactly one failure document. Child output and progress bars must
  not corrupt either stream; retain a bounded, clearly truncated excerpt.
- Structured logging stays off by default and uses the existing explicit local
  log-file policy. No remote telemetry, environment dumps, tokens, URL query
  credentials or raw authenticated command lines. New extended subprocess-log
  capture, if needed, must be opt-in and documented separately.
- Necessary state receipts are not a covert log stream. Store identity, state
  and output references only. Upstream-generated configure/build logs may
  exist in the isolated work directory and should be referenced honestly.
- Separate operational duration/observation reports from deterministic
  diagnostics and archive metadata. Redact secrets before persistence and
  test logging failure, disk exhaustion, broken pipes and output limits.
- The legacy preview adapter reports its real observable boundary. If an old
  script does not provide a typed phase, report `legacy-driver` with retained
  cause; never infer a precise diagnostic by fragile parsing of prose.

## 6. Artifact compatibility, verification and recovery

### Preserve contracts, not accidental weaknesses

Keep the v1 consumer envelope and canonical profile/host naming until a
reviewed incompatible change actually requires a new format. Reuse common
manifest/digest types and run the shared known-answer vector. Any discrepancy
between the old producer, JSON schema and Rust verifier is an explicit M0
decision with tests; the permissive implementation does not automatically win.

The archive contract includes directories, regular files and safe relative
symlinks. In contrast, the release's outer asset inventory must contain only
regular files. Reject absolute/escaping links, duplicate or colliding names,
special files, traversal, excessive expansion, truncated streams and trailing
unaccounted archive data. Bound memory and extraction resources.

Verify the archive hash/size, embedded/external manifest agreement, canonical
payload inventory/tree digest, executable layout, required C++ headers and
runtimes, collector aliases, and forbidden producer prefixes before accepting
an output. Generate SPDX from actual locked components, patches and collector
dependencies; preserve licenses/notices. An unknown legal/source obligation
blocks publication rather than being filled with invented metadata.

The current complete v1 release has 12 archives, 12 manifests, 12 checksum
sidecars, 12 SBOMs and eight support files: 56 regular files in total.
`SHA256SUMS` covers the other 55 files; the provenance bundle attests the 54
pre-bundle, non-checksum subjects. These deliberately avoid circular hashing.
Test this exact set and its content relationships, not just the file count.
If new evidence files must become assets, version/update the inventory contract
and all consumers explicitly; otherwise keep detailed CI observations outside
that immutable asset set.

### Three different comparisons

1. **Format parity:** feed identical synthetic payloads and identical metadata
   into the old/new packagers; require canonical inventory/digest agreement
   and byte parity where the existing encoding contract is retained. Check
   tar headers, UTF-8 ordering and XZ settings, not only decompressed files.
2. **Migration parity:** compare real legacy/native behavior and qualified
   content for controlled inputs, explaining every identity/epoch difference.
   A new tools commit changes the recipe and may change the collector; do not
   promise byte equality with an old published release or falsify metadata
   to obtain it.
3. **Release reproducibility:** two genuinely independent fresh builds of the
   same new recipe on each host/profile must yield byte-identical archives.
   A copied output, packaging the same prefix twice, or shared compiled-object
   caches do not satisfy this gate. Verified immutable source downloads may be
   shared and must still be checked independently.

### Compatibility and release proof

Preserve two-root relocation, poisoned-PATH compiler/collector probes, PC
multilib checks, pristine-upstream includes/linklibs consumers and tools-owned
CMake consumers for every release lane. A source checkout without copied
CMake modules is a mandatory fixture. Use the selected tools helpers, never
leftover binaries from a shared build directory.

Checksums do not establish origin. In isolated downloads verify the expected
repository, signer workflow, source revision/ref, subject digests and approved
runner provenance, using the established trusted verifier rather than a new
signature implementation. The [GitHub attestation verifier](https://cli.github.com/manual/gh_attestation_verify)
provides identity/subject policy controls; importing a bundle or parsing its
JSON is not cryptographic verification.

The build API has no publishing credentials. Protected jobs accept only the
complete, exact qualified handoff; recheck tag/object identities and measured
assets before exposure. Consumer lock promotion uses measured values only and
follows final-URL verification. Preserve existing release rules and the single
deliberate human tag gate; do not introduce a fictitious second reviewer or
weaken checks because the repository has one maintainer.

Compatibility replay may reuse verified immutable candidate archives after a
harness-only fix. Packaging-only recovery requires proof that all compiler,
comparison and compatibility gates already passed and that packaging was the
sole failure. A new release identity gets new metadata/provenance while
retaining measured payload identity. Never use recovery to bypass a compiler
or compatibility failure, overwrite a historical asset, or retarget a tag.

## 7. Milestones and acceptance gates

No implementation milestone is complete at this planning baseline. Work starts
with TCP-M0; linked GitHub issues carry live execution status. Writing this
document or creating an issue does not satisfy an acceptance criterion.

| Milestone | Deliverable | Depends on | Promotion boundary |
| --- | --- | --- | --- |
| TCP-M0 | Frozen contracts, baseline and threat model | None | Ready to implement |
| TCP-M1 | CLI preview and isolated legacy adapter | M0 | Local experimental use only |
| TCP-M2 | Native identity, preflight, source and environment handling | M1 | Native input preparation |
| TCP-M3 | Native build lifecycle and resource/error handling | M2 | Verified native build candidate |
| TCP-M4 | Native toolchain envelope and complete verification | M3 | Artifact-format candidate |
| TCP-M5 | Compatibility, replay and recovery parity | M4 | Qualification candidate |
| TCP-M6 | CI cutover, legacy retirement and documentation | M5 | Release-ready implementation |
| TCP-M7 | Immutable release qualification and consumer promotion | M6 | Qualified, distributed feature |

### TCP-M0 — Contract and baseline freeze

- [ ] Inventory every producer script/function, workflow call site, fixture,
  source/profile/schema rule and compatibility/recovery path; assign its new
  owner. Record changes needed to the source-contract validator.
- [ ] Freeze public/maintainer commands, plan/result/receipt formats, executor
  identity, diagnostics, source cleanliness and schema compatibility policy.
  Resolve manifest/parser differences with negative fixtures.
- [ ] Record a versioned native producer capability contract in `aros-tools`;
  recipes in `aros-toolchains` reference it. Keep source locks/profiles in one
  authoritative location, with only explicitly synchronized conformance
  fixtures across repositories.
- [ ] Capture existing fixture results and small package golden vectors.
  Record representative local build resource observations when available;
  label missing measurements and claimed support separately.
- [ ] Review the trust/credential/path/cache model and the upstream source
  boundary. Create the tracking epic/acceptance issues when implementation is
  authorized, and link them here.

Exit evidence: approved contract/architecture PR, script-to-owner inventory,
baseline fixture results and recorded open risks. No production path changed.

### TCP-M1 — Shared library boundary and CLI preview

- [ ] Introduce `aros-toolchain` and thin CLI modules with library/process
  dependency gates; preserve existing consumer commands and package inventory.
- [ ] Implement read-only planning and one explicit, temporary legacy-driver
  adapter against the selected producer checkout. No copied shell logic,
  moving branch resolution or fallback after a native failure.
- [ ] Establish diagnostics/logging, cancellation and guarded work/output
  ownership before launching real builds. Isolate all legacy mutations.
- [ ] Exercise mock success/failure/cancellation and invocation from outside
  checkouts; demonstrate one single-build local PC lane on Linux x86-64 and
  macOS AArch64 when hosts are available, with verified local-prefix use.
- [ ] Mark the adapter experimental and record its remaining shell/Python
  dependencies and coarse diagnostic limits. Do not expose it as a finished
  enterprise producer or switch release CI yet.

Exit evidence: process-boundary tests plus two measured local lane reports.
Absent host evidence remains open; it is not inferred from mock tests.

### TCP-M2 — Native input and environment contracts

- [ ] Implement recipe/lock/profile parsing, identity checks, preflight and
  capability diagnostics; clean snapshots include recursive source material.
- [ ] Use `aros-fetch` through a reviewed API boundary for verified acquisition;
  implement the locked MetaMake fetch adapter and exact source-use accounting.
- [ ] Prepare the selected Rust vendor tree and private host Python runtime,
  with an explicit, sanitized execution environment and preserved prefix maps.
- [ ] Cover stale/poisoned caches, missing hashes, undeclared sources/patches,
  mutated checkouts, wrong executor, unsupported source contracts and offline
  cache misses. Verify that none reaches compiler execution.

Exit evidence: native preflight/source tests on all four hosts; locked offline
fixtures prove controlled network paths cannot be used. Do not claim an OS
sandbox unless it has independent platform evidence.

### TCP-M3 — Native build lifecycle

- [ ] Implement configure, crosstools-release and exact-collector phases;
  preserve existing target flags, aliases, runtime closure and normalization
  rules without a transpiler migration.
- [ ] Implement phase receipts, safe explicit resume boundaries and locking.
  Release builds reject reused compilation roots and foreign state.
- [ ] Test positive job limits, nested parallelism, deadlines, interrupts,
  orphan cleanup, disk failures and durable-publication uncertainty. Preserve
  source inputs and previous outputs in every failure case.
- [ ] Run real native single-build diagnostics for all three profiles on Linux
  x86-64 and macOS AArch64. Record exact inputs, output verification, resources
  and any legacy/native differences; do not claim the other two hosts yet.

Exit evidence: six native host/profile reports, lifecycle/fault tests, and
existing consumer installation/verification regressions green.

Until M4 replaces packaging, these reports may use the explicitly selected
legacy package/verifier stage as a named migration dependency. Record that
boundary; it is neither a fully native pipeline nor an automatic fallback.

### TCP-M4 — Native package, manifest and inventory engine

- [ ] Port normalization, prefix scans, canonical tree inventory, `.tar.xz`
  packaging, manifest/index, sidecar/SBOM and read-back verification.
- [ ] Reuse existing common types; extract only proven shared primitives.
  Keep tools-suite `.tar.gz` packaging byte-compatible and independent.
- [ ] Pass old/new golden vectors for files/directories/links, UTF-8 paths,
  modes, timestamps, tar headers, compression and manifest serialization.
  Treat intentional stricter validation as a documented contract decision.
- [ ] Add malformed-archive, resource-exhaustion, mixed-recipe/matrix, missing
  SBOM/support-file and unsafe-asset tests. Verify the full 56-file v1 set.

Exit evidence: reproducible synthetic packaging on all four hosts, both
producer/consumer known-answer tests and complete positive/negative inventories.
No new full compiler matrix is required for this packaging-only gate.

### TCP-M5 — Compatibility, replay and recovery

- [ ] Port the complete compatibility harness to explicit tools-owned engine
  and helper resolution; cover a source tree with no copied CMake engine.
- [ ] Verify two-root relocation, standalone C/C++ links, PC i386 aliases and
  target-runtime checks; preserve pristine-upstream and CMake consumer probes.
- [ ] Implement native replay/repackage policy with bound evidence identities;
  reject expired/missing/tampered artifacts and non-packaging failure recovery.
- [ ] Test tag/draft conflicts, interrupted handoffs, changed assets, missing
  attestations, incorrect signer/source claims and non-regular release assets.
  Exercise publication orchestration with fixtures, not real releases.

Exit evidence: compatibility reports for available native diagnostic lanes and
complete offline replay/recovery tests. All twelve real compatibility lanes
are mandatory at M7; unexecuted lanes remain explicitly pending until then.

### TCP-M6 — Workflow cutover, retirement and usability

- [ ] Pin a reviewed tools executor in the producer workflow and call the same
  native `aros toolchain build` path used locally. Replace Python stage calls
  with native stage operations; preserve privilege separation and exact-input
  source-contract checks across repositories.
- [ ] Retire the temporary legacy adapter and redundant owned build/package/
  fetch/environment/compatibility scripts from active paths. Retain relevant
  fixtures/tests; historical implementations remain available through Git.
  Document every remaining shell/Python dependency and its owner.
- [ ] Exercise native frontend and fixtures on all four hosts, plus explicit
  lean real-build diagnostics. Add dependency-direction, script-reference and
  shared-contract drift guards to the existing canonical gates.
- [ ] Update command/help reference, local/offline build tutorial, prerequisites,
  upstream limits, troubleshooting, maintainer release/recovery instructions
  and producer README/handoff. Public docs describe implemented capabilities
  only and contain no hosting-service administration details.
- [ ] Check installation/packaging fixtures for GitHub archives, Debian,
  Homebrew and AUR. Expensive source-build prerequisites are documented as
  optional rather than forced on ordinary binary-toolchain consumers.
- [ ] Prepare a Conventional Commit/Release Please-compatible change set,
  close architectural risks and record a reviewed rollback plan before the
  first native producer tag. Do not hand-edit release versions to bypass it.

Exit evidence: coordinated merged PRs, all applicable workspace/producer
contracts green, documentation and install smoke tests, no active duplicate
producer implementation. Full release evidence is still pending M7.

### TCP-M7 — Qualify once, publish and promote measured values

- [ ] Select the exact clean `P1/T1/S1` identities and a new immutable annotated
  toolchain tag through the existing maintainer gate. No tag is selected or
  authorized by this planning document.
- [ ] Run the full four-host/three-profile matrix once: 24 independent builds,
  12 byte comparisons and all 12 compatibility/relocation lanes must succeed.
- [ ] Verify the full inventory, every hash/size, manifests, SBOMs, recipe and
  cryptographic provenance from an isolated complete draft download. Fail on
  any missing, mixed or altered subject.
- [ ] Publish the verified release unchanged under its intended status, verify
  every final asset URL, and promote only measured values into consumer locks
  and the matching source contract. Never activate provisional entries.
- [ ] Test fetch/install/verify/local consumption of promoted artifacts, then
  release the producer-capable tools suite via Release Please and its existing
  tag/qualification/distribution workflows. Its own A/B policy still applies.
- [ ] Record final commits, run links, immutable evidence digests, supported
  matrix, consumer results and remaining unrelated limits in the handoff.

Exit evidence: immutable toolchain release, matching tested consumer promotion,
distributed tools feature and completed evidence ledger. No local-only success
or partially published channel counts as complete.

## 8. Test strategy and runner budget

| Tier | Scope | Trigger | What it proves |
| --- | --- | --- | --- |
| T0 | Unit, parser/schema, golden and fault-injection fixtures | Every affected PR | Contracts and failure behavior; no compiler qualification claim |
| T1 | Native CLI/mock lifecycle, package vectors, install fixtures on four hosts | Normal relevant CI | Host support of orchestration and formats |
| T2 | One real host/profile build by default; broader Linux diagnostics explicitly selected | Local development or explicit diagnostic dispatch | A measured native lane, no publication |
| T3 | 4 hosts x 3 profiles x 2 independent builds, all compatibility checks | New immutable toolchain release tag | Full release reproducibility/compatibility |

Keep the existing restriction on manual full-matrix dispatch. Do not run a
full prequalification and then repeat it for the tag. Source-cache acceleration
is allowed; sharing A/B compiled outputs is not. Record actual runner minutes,
wall time, peak memory/storage where measurable, cache behavior and the reason
for each expensive run. Set regression budgets from M0/M3 measurements.

Tests must include interruption and crash boundaries, cache/source changes
between inspection and use, malformed child diagnostics, output flooding,
read-only/full filesystems, concurrent builders, stale locks/checkpoints,
path collisions, missing helper binaries, invalid targets, unavailable host
tools, recursive submodule drift and private-credential redaction. Use
property/fuzz tests for archive/manifest/recipe boundaries with a bounded
ordinary-CI corpus; larger fuzz campaigns are separate, explicit work.

If a packaging or compatibility harness fails, use the proven replay/recovery
path where its preconditions hold. If an output-affecting producer defect is
fixed, qualify a new immutable identity; never edit failed release history.
Do not conflate toolchain qualification with the separate tools-suite release
policy described in [RELEASING.md](../RELEASING.md).

## 9. Repository tracking and handoff

### Single source of truth

This document owns the design, milestone dependencies, acceptance criteria and
scope decisions. Do not maintain another prose plan in `aros-toolchains` or
copy it into public user docs. That repository links to this plan and owns its
implementation issues/evidence.

Execution tracking:

- One `aros-tools` epic: [Integrate the native AROS toolchain producer](https://github.com/metaneutrons/aros-tools/issues/27).
- One acceptance issue per TCP-M0 through TCP-M7, with focused implementation
  issues in the repository that actually changes. Link cross-repository
  dependencies explicitly; a tools acceptance issue stays open until its
  producer/source dependencies have evidence.
- Group delivery into three GitHub milestones: [Toolchain build preview](https://github.com/metaneutrons/aros-tools/milestone/1)
  (M0-M1), [Native producer candidate](https://github.com/metaneutrons/aros-tools/milestone/2)
  (M2-M6), and [Native producer qualified](https://github.com/metaneutrons/aros-tools/milestone/3)
  (M7). GitHub milestones are repository-local collections of
  issues/PRs, so a cross-repository completion percentage is not inferred from
  one repository's counter. See [GitHub milestone semantics](https://docs.github.com/en/issues/using-labels-and-milestones-to-track-work/about-milestones).
- Issues own assignment, execution state and blockers. PRs own implementation
  and checks. This plan owns acceptance criteria; issues link the relevant
  section rather than copying mutable checklists. A GitHub Project is optional
  if the linked issue set becomes hard to navigate, not a fourth status store.
- Use the linked issues below for live status. The unchecked criteria here
  become checked only through evidence-linked PRs;
  do not duplicate a second manually maintained in-progress status table.

The epic, eight acceptance issues and three delivery milestones were created
on 2026-09-05 after implementation was authorized. No compiler matrix, release
tag or publication was started by this tracking setup.

| Acceptance issue | Link | Completion evidence |
| --- | --- | --- |
| TCP-M0 | [#28](https://github.com/metaneutrons/aros-tools/issues/28) | Pending |
| TCP-M1 | [#29](https://github.com/metaneutrons/aros-tools/issues/29) | Pending |
| TCP-M2 | [#30](https://github.com/metaneutrons/aros-tools/issues/30) | Pending |
| TCP-M3 | [#31](https://github.com/metaneutrons/aros-tools/issues/31) | Pending |
| TCP-M4 | [#32](https://github.com/metaneutrons/aros-tools/issues/32) | Pending |
| TCP-M5 | [#33](https://github.com/metaneutrons/aros-tools/issues/33) | Pending |
| TCP-M6 | [#34](https://github.com/metaneutrons/aros-tools/issues/34) | Pending |
| TCP-M7 | [#35](https://github.com/metaneutrons/aros-tools/issues/35) | Pending |

### Issue and pull-request contract

An implementation issue records its TCP milestone, owning repository, problem,
bounded scope, dependencies, affected contracts, linked acceptance criteria,
test/evidence plan, rollout/rollback and estimated effort range. Estimates are
planning inputs, not completion evidence; calibrate them after M0 and M3.

Every implementation PR must include:

1. the linked issue and acceptance criteria it advances;
2. production code, maintained regression tests and directly affected docs;
3. exact tested source/tools/producer revisions and platform omissions;
4. commands/run URLs, results and artifact/evidence digests where applicable;
5. compatibility, security and resource-cost consequences; and
6. any remaining blocker and the next safe action.

Use focused `fix/toolchain-*` branches under the repository convention and
appropriate Conventional Commit titles. No automated-assistant attribution or
`Co-Authored-By` trailers. Approval and tag authority follow existing repository
rules; this plan adds no second-human requirement.

Keep a concise evidence record with each milestone closure: exact revisions,
host/profile, gate, run/result, retained evidence location/digest and omissions.
Do not rely exclusively on expiring Actions artifact URLs. Preserve durable
public-safe summaries/digests with the repository or immutable release; never
commit credentials or uncontrolled raw logs. Interrupted work hands off the
current issue/PR, verified state and next gate rather than a guessed percent.

### Definition of done

The integration is done only when M7 closes with evidence, the native producer
is the single active implementation, existing consumer/distribution contracts
remain valid, every claimed host/profile is qualified, fault paths have stable
diagnostics, and documentation matches shipped commands. Known exceptions must
be explicitly scoped/deferred; they cannot be hidden behind "100%".

## 10. Rollout risks and decisions that must remain explicit

| Risk | Control / stopping condition |
| --- | --- |
| Current tools differ from recipe-pinned tools | Verify actual executor identity; reject mismatch before execution |
| Pristine upstream lacks the release-specific closure | Capability failure with precise remediation; no silent source patch/fallback |
| Native archive serialization differs from Python | M4 golden/encoding tests; new identity with explained differences, never fabricated parity |
| Shared packaging refactor changes tools archives | Preserve native-suite vectors and package-manager fixtures before merge |
| Host compiler/SDK drift changes output | Record output-affecting environment, retain observations, require A/B; no unsupported hermetic claim |
| Checkpoint/cache reuse contaminates proof | Verify receipts and inputs; fresh A/B compilation/install roots |
| Source/engine relocation invalidates compatibility | Explicit embedded-engine contract and engine-free-source fixture |
| Cross-repository pins block each other | Follow the T1 -> P1/S1 -> T2 rollout; no reciprocal future-hash requirement |
| Tag/draft failure consumes runner capacity | Proven replay/recovery only when eligible; no repeated full prequalification |
| Work expands into compiler, SDK, RISC-V or board refactoring | Separate proposal/issue; do not broaden this migration implicitly |

M0 must also validate whether a complete new receipt/executor contract fits
the existing recipe/manifest schemas. Preserve v1 consumer compatibility where
possible, but explicitly version a real incompatibility rather than forcing
new semantics into old fields or suppressing unknown-field validation.

Rollback before release means reverting workflow selection in a new reviewed
commit to an already verified producer, not silently switching backend during
a failed run. After release, preserve published bytes and stop further
promotion; remediate with a new immutable release and an explicit consumer
lock update. Existing valid installations remain untouched.

The first implementation step is TCP-M0. Do not start native rewrites,
compiler builds, full matrices or release publication as part of approving
or editing the plan alone.
