---
title: Build a local native toolchain candidate
description: Prepare exact inputs and an offline cache, then build and verify a local AROS compiler candidate without publication authority.
---

This is a maintainer workflow. It produces a **local-only** candidate from
three exact Git checkouts and can package it locally; it does not create a tag,
release, attestation, or package-manager publication. For a published compiler, use
[Choose and verify a toolchain](/aros-tools/workflows/toolchains/) instead.

The command does not discover a neighbouring checkout. All paths are explicit
and the selected commits must agree with the producer declaration. Keep source,
producer, tools, cache, work, output, and recipe paths separate. Use clean
checkouts and absent work/output leaves.

## What you need

The native lifecycle selects LLVM or GNU from the bound source lock and
profile. GNU uses source-lock v3, profiles v2 and explicit RISC-V ISA/ABI
contracts; all selected identities must agree. It selects the locked GCC and
binutils versions and installs the Rust collector with measured GNU tool roles.
Package and verify-package accept these candidates with a schema-2 manifest
and an inventory-bound executable layout. These are local development paths,
not a published RISC-V compiler, native board-build qualification or boot claim.
The examples below retain the existing PC/LLVM selection.

- A clean AROS source checkout, an `aros-toolchains` producer checkout, and an
  `aros-tools` checkout at the commits selected by the producer declaration.
- The `aros` executable built from that selected tools checkout.
- Cargo, the selected Rust toolchain, CMake, a compatible `make`, Git, a host
  C/C++ compiler, and Python for the unchanged upstream AROS build.
- An online cache-bootstrap step followed by an offline build. The offline
  build itself never falls back to transport.

The [prerequisites](/aros-tools/getting-started/prerequisites/#local-native-producer-candidates)
page records measured storage from the current PC proof. Treat those values as
an observed floor, not a reservation guarantee.

GNU candidates additionally require GNU Make 4.0 or newer, bison, flex, patch,
pkg-config and Ninja. On macOS select `gmake`; Apple's Make 3.81 cannot execute
the source's generated rules. With a Clang host compiler, `llvm-ar` and
`llvm-ranlib` must also be on `PATH`: preflight observes both before admitting
their directories to the isolated build environment. A missing tool fails
before source execution, not during a host-library build.
Mako and MarkupSafe are mandatory locked Python
imports. A selected source that also needs PyYAML must declare its `yaml` import
and exact archive/version in the lock; ambient Python packages are not accepted.
GCC's format lists are narrowed to exactly one hash-verified lock entry before
the native Rust fetch bridge runs, never used as transport fallbacks. GNU
cache subdirectories are stamp namespaces, not alternative source locations.
Both compiler families give the Rust fetcher a private verified source copy and
enforce offline checksum validation. The bridge translates the source's
MetaMake fetch arguments without executing its `fetch.sh`; patch paths must
remain inside the measured source snapshot. LLVM keeps its two-package Python closure.
Empty source-template checksum placeholders are filled from the exact lock.
Nonempty declarations must match it; repeated checksum options fail. Direct
downloads in source recipes are not covered by this bridge and must be replaced
by declared, verified fetch inputs before an offline build can be qualified.

## Prepare exact inputs

Set paths outside all three checkouts. The example names only locations; obtain
the actual revisions from `toolchains/producer-executor-v1.toml`, not from a
moving branch name.

For declaration schema 2, use the source lock and profile matrix of the group
containing your preset when creating the recipe. Planning and building verify
every declared group and select only the pair bound by that recipe's digests.

```sh
export AROS_SOURCE=/absolute/path/to/AROS-NX
export PRODUCER=/absolute/path/to/aros-toolchains
export TOOLS=/absolute/path/to/aros-tools
export TOOLS_TARGET=/absolute/path/to/producer-tools-target
export AROS="$TOOLS_TARGET/release/aros"
export CACHE=/absolute/path/to/producer-cache
export WORK=/absolute/path/to/pc-candidate-work
export OUTPUT=/absolute/path/to/pc-candidate-output
export RECIPE=/absolute/path/to/pc-candidate.recipe.json

(
  cd "$TOOLS"
  cargo build --locked --release -p aros-cli --target-dir "$TOOLS_TARGET"
)
git -C "$AROS_SOURCE" status --short
git -C "$PRODUCER" status --short
git -C "$TOOLS" status --short
```

Each status command must print nothing. Do not place `TOOLS_TARGET`, `CACHE`,
`WORK`, `OUTPUT`, or `RECIPE` below one of the checked-out roots. The native
input audit also rejects ignored build artifacts; `git status --short` alone
does not prove a clean raw checkout.

## Bootstrap and verify the cache

First acquire the source archives selected by the producer lock while transport
is allowed. The command verifies existing objects and refuses a mismatched
payload.

```sh
"$AROS" cache sources fetch \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --dir "$CACHE" --format json
```

The native build also compiles Rust helpers from the exact tools `Cargo.lock`.
Prepare the immutable vendor generation through `aros`; it selects the clean
tools Git tree, the producer's pinned Rust channel, and the exact Cargo
executable. It runs Cargo with a private `CARGO_HOME`, writes a single
validated directory placeholder, checks every vendor checksum and publishes
only a complete generation under `$CACHE/cargo/v1/…`. It never copies a user
Cargo configuration or credential into the cache.

The tools workspace's declared minimum Rust version must not exceed the
producer's pinned channel. An incompatible pin fails with `AX0401` before
Cargo is invoked or a vendor generation is created, and before the native
configure/compiler phases. Update the producer pin explicitly; there is no
fallback to the ambient Rust toolchain.

```sh
"$AROS" cache cargo fetch \
  --producer-dir "$PRODUCER" --tools-dir "$TOOLS" --dir "$CACHE" \
  --format json

"$AROS" cache cargo verify \
  --producer-dir "$PRODUCER" --tools-dir "$TOOLS" --dir "$CACHE" \
  --format json

"$AROS" cache sources verify \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --dir "$CACHE" --format json
```

Use `cache cargo list` when only the selected generation receipt is needed; it
does not hash the vendor tree, but it runs bounded Git and `cargo --version`
probes to prove the selection. A changed tools tree, lockfile, producer Rust
pin, or Cargo executable identity selects a different generation. The native
lifecycle then accepts only that exact generation, copies it into a private
runtime directory and invokes its collector with `--locked --offline`. Never
repair a missing generation during an offline build; use `cache cargo fetch
--offline` only to require a previously verified one.

## Construct the recipe and inspect readiness

Select source lock and profile files inside the producer checkout using absolute
paths. The recipe records their producer-relative identities. Recipe creation
never replaces an existing file.

```sh
"$AROS" toolchain producer recipe \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --source-lock "$PRODUCER/toolchains/llvm-11.0.0.sources.json" \
  --profiles "$PRODUCER/toolchains/profiles-v1.json" --output "$RECIPE" --format json

"$AROS" toolchain plan --preset pc-x86_64 --recipe "$RECIPE" \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --work-dir "$WORK" --output-dir "$OUTPUT" --cache-dir "$CACHE" \
  --jobs 8 --timeout-seconds 21600 --format json
```

Proceed only when `readiness` is `ready`. A blocked or invalid plan is a
diagnostic, not an invitation to change its identities manually.

## Build and verify the local prefix

```sh
"$AROS" toolchain build --preset pc-x86_64 --recipe "$RECIPE" \
  --source-dir "$AROS_SOURCE" --producer-dir "$PRODUCER" --tools-dir "$TOOLS" \
  --work-dir "$WORK" --output-dir "$OUTPUT" --cache-dir "$CACHE" \
  --jobs 8 --timeout-seconds 21600 --release-id local-pc-candidate \
  --format json

cd "$AROS_SOURCE"
"$AROS" toolchain verify --preset pc-x86_64 --local "$OUTPUT/toolchain"
"$OUTPUT/toolchain/bin/clang" --version
"$OUTPUT/toolchain/bin/ld.lld" --version
```

The six phase receipts bind the selected inputs. A separate
`finished-candidate.json` measures the complete tree after collector installation;
the result's `finished-candidate` evidence entry identifies it. Collector-only
outputs are not complete compiler evidence. These local receipts do not prove
authenticated execution or release readiness. The prefix may be used explicitly by
an AROS build with `--toolchain-dir` where its compiler family is supported by
that consumer; producer acceptance alone does not qualify a native GNU board
build. It remains local-only, has no release provenance, and cannot be promoted
by copying it into a consumer lock.

The separate `toolchain producer package` and `verify-package` stages accept
`--package-format legacy-v1|family-v2`. Omitting the option preserves the
existing family default: LLVM uses its historical schema-v1 manifest and v1
asset name, while GNU uses compiler-family schema v2. For an explicit LLVM
schema-v2 package, pass `--package-format family-v2`; its manifest records the
LLVM compiler family and version and its asset uses the v2 LLVM name. The
historical `legacy-v1` choice remains LLVM-only. Package and release-index
formats are separate explicit selections. A family-v2 package alone does not
qualify publication or recovery.

For a package bound to the complete finished build, retain the build's JSON
stdout as a regular file outside the checked-out and candidate roots. Record
its exact file SHA-256 separately. Pass that file, its selected digest and the
original work root through `--build-result`, `--build-result-sha256` and
`--build-work-dir`, with explicit `--package-format family-v2`. Partial selections
are rejected. The existing `--input-dir` must name the original
`$OUTPUT/toolchain` prefix. The command verifies all retained phases, complete
raw payload and normalized package before publishing the local package directory.
Its JSON output adds `finished_candidate` with the joined digests and a portable
measurement string with its exact SHA-256. The string retains the build result,
six phase receipts, finished record and four package measurements. Preserve its
bytes unchanged; these receipts belong in protected evidence artifacts, not
public package metadata. This transport does not authenticate execution.

`producer verify-release-evidence` joins the complete indexed A/B export set to
measured packages, comparisons, compatibility reports/logs/ELFs, final checksums
and unchanged pre-attestation subjects. It requires external lane selections
and retained raw digests; it does not qualify a release or authenticate jobs.

A local file hash is not authenticated execution provenance. Hashing an
untrusted result does not make it trusted; a release workflow must separately
verify its run/artifact origin. These options do not establish independent A/B
builds or release eligibility. Ordinary local packaging without them remains
available for already-built prefixes.

For GNU builds, configure records host compiler prefix maps in `HOST_*FLAGS`.
The compiler-build process does not export `CFLAGS` or `CXXFLAGS`: MetaMake
owns the target ISA flags, which must not reach host-built Binutils or GCC.
LLVM builds retain their existing compiler environment.
Compiler caching defaults to `off`, including release qualification. To reuse
host C/C++ compilation during local development, prepare a managed local
namespace and add `--compiler-cache sccache` (or `ccache`) to the build command:

```sh
aros cache compiler prepare --backend sccache
# Add to the toolchain build invocation above:
# --compiler-cache sccache
```

`--compiler-cache auto` selects a prepared local sccache namespace first, then
ccache, or remains off. An explicit `--compiler-cache-dir DIR` requires an
explicit backend and an already prepared namespace outside all producer roots.
Offline execution remains mandatory; ambient cache settings and remote storage
are not imported. The compiler phase receives controlled host C/C++ launchers;
target runtime compilation, assembly, linking and the Rust collector are not
cached by this integration. Receipts bind the backend executable, host
compilers and local configuration. Resume requires the same bindings and
unchanged launchers. Use `--compiler-cache off` for independent A/B builds.
On Unix, sccache needs short paths for both its managed socket and the startup
socket below the owned work directory. An oversized path is rejected before
configure; select shorter roots or use ccache instead.

GNU configure and MetaMake also resolve recursive `make` through a private
alias to the exact preflight-selected GNU Make, rather than a second executable
found elsewhere on the host PATH.

The v3 source lock can declare source-owned patches for target build
dependencies outside the compiler directory. Recipe creation reads each patch
from the exact clean source commit and binds its SHA-256; missing, unsafe or
duplicate paths and an incomplete recipe patch set are rejected. Compiler
component patches remain restricted to their selected family directory.

## Failure and recovery boundary

`producer materialize-engine-free-source` requires a clean committed source.
Use `--recipe` to bind its identity to the compiler build source, or both
`--source-commit` and `--source-tree` for a separately pinned SDK consumer.
The two selections cannot be mixed. A different consumer does not change the
compiler package's recipe or provenance. The snapshot is recursively audited,
has its source-tree engine removed and receives a measured content digest;
it is not compatibility evidence until the actual consumer phases pass.

The six-phase adapter accepts GNU packages with an explicit `--source-preset`
from the measured consumer source's `aros-targets.toml`. That preset selects
source rules and maps to the compiler profile through `toolchain_profile`.
When that preset binds a validated `native_consumer_contract`, the CMake
consumer phase configures the source and then builds only the SDK roots named
by the contract in the same phase. GNU inputs without that contract and the
existing LLVM adapters remain configure-only. This verifies the declared
source/build flow; it makes no board or hardware qualification claim.
A verified SDK consumer does not publish guest runtime selections such as
Mesa's `GL.default` or add dependencies on the guest GL implementation.
The full-build runtime checks remain separate.
If the source contract declares host file generators, `--ports-cache-dir` must
also contain their raw input files. Each declaration supplies the exact filename,
size and SHA-256. The adapter verifies them before configuration and supplies
private read-only copies to CMake, not the mutable cache. A missing or modified
file fails without a download. Supply an absolute cache path.
Driver roles and RISC-V flags come from the verified package and profile, not
PATH or board-name inference. GNU receipts use schema v3, LLVM family-v2 uses
package-bound schema v4, and legacy LLVM v1 retains schema v2. Family-v2
execution checks the source/profile binding, both complete inventories and
compiler-bound standalone ELF outputs. Complete V2 release-evidence and read-only
recovery checks admit GNU v3 and LLVM v4 receipts. Explicit family-v2 `repackage`
also supports both families. Successful packaging or synthetic adapter tests
are not a real consumer qualification.
Family-v2 compatibility also checks the exact retained report/log inventory
against pre-execution source and environment expectations, then reparses the
standalone outputs. This local read-back is not signature verification or
recovery admission.

For a complete local export, add `--package-format family-v2 --evidence-dir
/absolute/path/to/absent-export` to `producer compatibility`. The export parent
must exist and have no symlink components. Inputs and execution roots must be
separate. After all six phases and read-back pass, one no-clobber directory
publication exposes the exact pre-execution `inputs.json` and closed
`evidence/` reports, logs and ELF files. Existing output is never replaced.
Overlap checks recognize filesystem aliases and conservatively reject missing
path suffixes that differ only by ASCII case.
JSON adds the output directory, input-document digest and evidence-manifest
digest under `local_evidence`; retain those digests outside the uploaded files.
Keep this evidence private, outside the public release inventory.

`producer release-plan --directory DIR --inputs-sha256 SHA --format json`
projects the complete selected V2 input collection before any builds. Its
`groups` retain each source revision and exact document names/digests; `lanes`
contains every selected group/host/profile combination. Derive scheduling from
this result rather than copying profile lists into the workflow. Runner labels
and A/B scheduling are workflow policy. The command is read-only and its
`assurance: input-binding-only` result does not qualify execution or release.

`producer verify-release-evidence` reads the complete evidence on a collector
host without reopening original runner roots. Supply `--directory`,
`--release-id`, `--base-url`, `--inputs-sha256`, `--index-sha256`, `--selection`,
`--selection-sha256`, `--subject-manifest` and `--subject-manifest-sha256`.
The selection schema is `aros-toolchain-release-evidence-selection-v2`; it
contains every indexed archive's independent environment/required-path map,
two build exports and executor digests, comparison and compatibility inputs.
The lane set comes from release inputs, not discovered reports. JSON reports
`assurance: byte-consistency-only`; authentication, qualification and recovery
remain separate gates.

`producer verify-qualification` additionally requires `--qualification-evidence`,
`--qualification-sha256`, `--policy` and `--policy-sha256`. It joins complete V2
claims to that same actual evidence: raw A/B exports, comparison, compatibility
manifest, final checksums, subject list and provenance bytes. Diagnostic coverage
and self-hashes substituted for raw exports/manifests are rejected. It writes
nothing and remains `assurance: byte-consistency-only`; the run attempt,
signatures and producing jobs still need independent authentication.

`producer verify-recovery` uses those same complete selections and adds
`--recovery-request` and `--recovery-request-sha256`. It checks the exact V2
qualification digest, external run-attempt/tag/signer observations and a fresh
absent packaging handoff. It reacquires all original bytes and writes nothing.
Replay allows a compatibility-harness failure, not an actual compatibility
failure; compilation and comparison failures are also ineligible. Observations
still require external authentication; this check executes no recovery. The
historical `prepare-recovery` and `validate-recovery` commands remain V1-only.

For packaging-only execution, use `producer repackage --release-format family-v2`
with the same complete selection and independent digests. Rename `--directory`
to `--release-dir` and `--release-id` to `--source-release-id`; select one exact
archive basename with `--asset`. Add `--first-extraction-dir`,
`--second-extraction-dir`, `--first-output-dir`, `--second-output-dir` and
`--comparison-output`. Every destination must be absent, absolute and normalized,
with an existing nonsymlink parent, outside all selected evidence and the other
destinations. Declared forbidden build roots are also protected, including
existing aliases and prospective suffixes. The caller must exclusively own
quiescent inputs and outputs.

The command revalidates the complete original qualification, extracts the
selected package twice and packages both copies under the fresh recovery
identity. Only release identity changes; qualified payload and other manifest
fields remain unchanged. It compares all four package members, checks the
original closure again and writes a no-clobber comparison receipt. JSON includes
the fresh and original release IDs, original archive hash/size, output paths,
comparison digest and package-set digest. `build_count` and `lanes` describe
the original evidence, not new compiler executions.

This operation neither signs nor publishes, and it does not authenticate
GitHub observations. Replay requests are ineligible for packaging. A failure
after output creation preserves the selected new directories for diagnosis;
never treat them as a completed recovery or adopt them on a retry. Use fresh
paths. Without explicit format, `repackage` keeps the V1 contract; V1 context
flags and V2 complete-selection flags cannot be mixed.

`producer record-qualification --release-format family-v2` acquires that same
complete closure before deriving the claim file. Supply `--release-dir`, the
independent input/index/selection/subject digests and paths, and source
repository/workflow/run ID/positive attempt/tag object/peeled producer commit.
The index digest binds release ID and download URL. Add the selected signer
claims, creation/expiry times and an absent absolute `--output` outside every
input and evidence root, with an existing nonsymlink parent. The complete file
is written atomically without replacement; JSON includes its raw digest and
`assurance: byte-consistency-only`. External authentication remains mandatory.
Default recording and explicit `legacy-v1` keep the historical layout; legacy
report-root arguments cannot be combined with the V2 selection.

The v2 library also binds comparison-report claims for all four package members
to measured final checksum entries. This check does not authenticate independent
builds or authorize release publication; CLI release admission is still a
separate integration step.

`producer index --release-format family-v2` measures all compiler groups selected
by `toolchain-release-inputs-v2.json`. Supply `--lane-inputs` with independent
archive environment and required-path maps, plus an external `--subject-manifest`.
The `pre-attestation` stage writes the index and subject list, not final checksums.
After external attestation supplies provenance, `final` requires the original
`--subject-manifest-sha256`, verifies unchanged subjects and writes final checksums.
These local byte checks do not authenticate signatures. The default `legacy-v1`
index path remains available. Qualification recording and read-only recovery
verification and explicit `repackage --release-format family-v2` support V2.
Historical request creation and default V1 execution keep their original
contracts; no format is inferred from filenames or failed verification.

The producer preserves owned work/output roots on failure, cancellation, and
deadline expiry. Inspect their lifecycle receipts and logs, correct the exact
input problem, and start with fresh roots. Do not delete or reuse retained
directories as an implicit resume mechanism. Packaging-only recovery and
release qualification have distinct evidence requirements; see the
[release reference](/aros-tools/reference/releases/) for the published-product
boundary.
