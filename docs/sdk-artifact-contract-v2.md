# Developer SDK artifact contract v2

Related initiative: [declarative boards and RISC-V integration](riscv-board-integration-plan.md).

## Identity and ownership

An SDK is one source-produced Developer sysroot for one target/ABI. It is not
a host compiler archive, a board firmware image, or a universal SDK for every
target. The selected AROS source owns its headers, libraries and ABI facts;
`aros-tools` packages and verifies those bytes. Compiler family, compiler
release and build host remain separate identities.

`SdkIdentity` records a portable SDK ID, an independent canonical SemVer
version, a source-selected target triple and scalar ABI facts, exact source
commit/Git tree/content digest, the source-owned SDK contract digest and a
source-date epoch. No board enum, host selector or executable extension is
introduced. ABI map keys are unique; map order does not change its canonical
digest. Changing a source fact changes that descriptor digest. Validation
checks representation and consistency, not that the chosen ABI is implemented.
Commit and Git tree identify the complete original checkout. The content digest
identifies its normalized raw-Git projection with top-level `cmake/` omitted,
including initialized submodule material. It is not a digest of an arbitrary
checkout or of the generated SDK payload.

The caller must select the identity document and its raw SHA-256 independently.
Packaging does not derive a version from the installed compiler or manufacture
a released source identity from an uncommitted tree. `aros-tools` builds and
manages SDKs; `aros-toolchains` distributes SDK and compiler artifacts together
in one release. No separate SDK repository is required. SDK product versions
remain independent of the enclosing release and compiler versions. The four
initial target products are selected at SDK version `0.1.0` and must be
published together; local package success does not make them published.

## Read-only source binding

`aros sdk verify-source` requires the independent identity document/hash pair,
the clean original Git checkout, its existing engine-free projection and an
explicit source preset. It recursively audits original worktree bytes and modes
against raw committed Git material, including initialized submodules. The
projection must contain exactly those normalized entries except top-level
`cmake/`: missing/extra entries, empty extra directories, altered executable
modes and link targets fail. Projected regular files must have exactly one link;
content-identical hardlinks to another path are rejected. Bounded descriptor
snapshots bracket verification; it never copies, repairs or executes source
material.

The selected profile must explicitly name exactly one consumer contract:
`native_consumer_contract` for the existing GNU native-consumer-v2 schema, or
`sdk_consumer_contract` for `aros-sdk-consumer-contract-v1`. Both require sealed
ordinary C/C++ SDK link probes. No fallback occurs after a declared contract
fails validation. Its exact digest, target triple and complete scalar ABI map
must equal the SDK identity. No board enum or host identity is inferred.
The library returns an opaque `VerifiedSdkSource` with read-only revalidation;
CLI JSON states `local-source-binding-not-execution-or-release-admission`.

The generic SDK contract owns a `target` descriptor instead of a RISC-V kernel
ABI. Its scalar `source_cpu`, `pointer_width` and `use_mmu` facts must agree
with the source profile. A declared `float_abi` or bootstrap ABI is also bound
exactly. The triple must select the source CPU and AROS. Additional scalar ABI
facts remain in the full identity; they are not discarded or interpreted as
executable extensions. Roots, source inputs, invocation policy and probe
validation use the same sealed graph validator as the native consumer.
Historical native-consumer-v1/v2 and native-build ABI validation stay unchanged.

The generic source binding covers LLVM-shaped x86-64, ARM hard-float and
AArch64 descriptors as well as GNU/RV32-shaped descriptors. The transpiler's
existing `--native-consumer-profile` selector admits exactly one declared
contract and repeats the sealed selection for inventory and graph export.
Generic consumers retain literal source roots and cannot authorize core,
package or boot-media builds. Pre-compiler JSON uses
`aros-sdk-consumer-validation-v1` with `target`; legacy consumers keep
`aros-native-consumer-validation-v1` with `abi`.

The read-only `aros toolchain producer profile` command also accepts this
generic contract when `--source-dir` and `--source-preset` are selected.
It verifies the contract and every sealed input against
the selected clean Git checkout and reports `native_sdk_required: true`.
Ambiguous native/generic declarations and invalid generic contracts fail
without falling back to a non-SDK policy. This projection is not execution,
authenticated CI evidence or release admission.

The owned CMake engine admits the generic response only for LLVM. Execution
requires explicit source `flavour` and `platform_smp` facts in addition to CPU,
pointer width and MMU selection. Before compiler detection, language-rule
hooks apply the sealed `compiler_arch` or `compiler_cpu`, `compiler_fpu`,
`compiler_code_model` and
ARM `float_abi` options to C, C++ and Assembly. Unrelated environment and
initial flags remain intact; explicit flags are not reconstructed. Repeated
configuration must not duplicate selected options. After compiler detection
it checks all three driver targets, pointer width and selected options. A
target/sysroot override, conflicting configuration flag or changed binding
fails without publishing a current consumer binding. The source-derived
Developer include root is shared by bootstrap and target producers.

These graph and configured-compiler checks are not native SDK generation or
ordinary application-link qualification. GNU/RV32 execution still uses its
existing native-consumer-v2 contract; a generic GNU descriptor is not admitted
by the LLVM CMake adapter.
The additive `compatibility::inspect_llvm_sdk_drivers` API measures both
payload-owned Clang drivers and queries their exact selected version and AROS
triple. A bounded compiler-tree snapshot includes executable bytes and aliases;
revalidation rejects subsequent changes. It neither authenticates the package
nor proves the scalar ABI facts, SDK completeness or ordinary application links.
The caller must independently verify its selected compiler package.
The additive `compatibility::plan_llvm_sdk_application_links` API binds four
ordinary driver commands to that measured compiler, the sealed source probes,
and equal, disjoint original/relocated SDK trees. It performs no build, link,
copy or filesystem write. Commands preserve startup and standard-library
defaults; additional library arguments are bounded names from the sealed
contract, not arbitrary compiler or linker options.

This planner requires explicit unsigned `pointer_width` (32 or 64),
`elf_machine` (1–65535), `elf_flags_mask` and `elf_flags_value` (32-bit values,
with no value bit outside the mask) in the source target's ABI map. Its ELF
check uses those facts, AROS OSABI/ABI-version marking and the collector symbol;
there is no CPU-to-machine table. Optional `float_abi` selects exactly one
driver flag. Optional `compiler_arch`, `compiler_cpu`, `compiler_fpu` and
`compiler_code_model` text facts select `-march`, `-mcpu`, `-mfpu` and
`-mcmodel`; each is a bounded ASCII token, not a
board name inferred by the tools. Architecture and CPU selections are explicit
alternatives: declaring both is rejected rather than depending on flag order.
The SDK build and all original/relocated application commands must use those
same source-owned selections. ARM selections require an explicit float ABI.
No code model is inferred from the architecture or board. An absent fact adds
no option; a declared model is applied once and conflicting or per-configuration
overrides fail before a consumer binding is published.

ELF32 ARM selections additionally require explicit unsigned `arm_cpu_arch`,
`arm_cpu_profile`, `arm_fp_arch`, `arm_simd_arch` and `arm_vfp_args` facts.
Optional `arm_arm_isa` and `arm_thumb_isa` facts select further checks.
These keys refer to standardized aeabi file-attribute tags, not a CPU capability
table. The reader compares each selected value with the produced
`.ARM.attributes` bytes, rejects ambiguous scopes and unknown mandatory tags,
and uses standardized omitted-zero defaults only without `Tag_nodefaults`.
Identical ELF flags cannot substitute for these measurements. ARM attribute
facts are rejected for non-ARM selections, and the VFP argument convention must
agree with the declared float ABI.

These structural checks do not establish full ABI compatibility,
C++ initialization, SDK completeness or which invocation produced an ELF.
The additive `compatibility::execute_llvm_sdk_links` API executes that four-link
selection through the shared supervised runner. It copies the complete SDK,
uses a private temporary directory and a closed environment, and requires
byte-identical original/relocated C and C++ outputs. Every traced input must
resolve inside the SDK or authenticated compiler tree, except the direct
short-lived Clang object derived from the sealed probe name. The trace is the
used-input list; maps are bounded retained evidence checked for original-SDK
leakage, not a second input-list parser.

An opaque completed handle revalidates source, compiler, both SDK trees,
commands, reports, logs, maps, outputs and the closed receipt without rerunning
or repairing anything. Only exact empty lock sidecars from atomic publication
are admitted; extra members fail. Cancelled, expired or failed execution cannot
return a completed handle. Filesystem validation is not preemptible, so the
elapsed-time budget is not a hard limit on filesystem I/O.

Its `aros-llvm-sdk-link-receipt-v1` is local link evidence, not SDK generation,
full ABI qualification, C++ execution or authenticated owning-job evidence.
Synthetic positive and tampering tests do not qualify a real compiler. The
generation owner must still connect these links to the generated SDK and an
independently authenticated portable export.

`compatibility::readback_llvm_sdk_links` reconstructs an existing local proof
from independent source/compiler selections and the execution owner's expected
receipt digest. It remeasures both SDK trees and derives the commands from the
sealed probes; it never links applications again, copies an SDK, creates output
or repairs evidence. Read-only source audits and compiler-tree remeasurement
still run; the reader does not repeat driver queries. Missing, extra or altered
receipt, inventory, report, log, map and ELF members fail; the prior in-memory
completed handle is not required.

`compatibility::bind_llvm_sdk_generated_root` validates the retained CMake
consumer binding against the exact committed source and a fresh response from
the measured transpiler. The source validator derives the SDK include/root path
from sealed source configuration; Rust does not assume a conventional directory.
The opaque result retains the exact binding bytes and complete SDK tree and
rejects changed selection/content on readback. It does not prove that a build
completed or that the SDK is complete.

`compatibility::execute_llvm_sdk_build_links` runs the four ordinary links only
against that source-selected build SDK, never a separately supplied candidate.
`compatibility::readback_llvm_sdk_build_links` joins retained links to the same
source/CMake selection and independently expected binding and receipt digests.
Neither local reader is rootless portable evidence or remote-job authentication.
Proof outputs must be outside the entire CMake build tree. One elapsed budget
covers source binding, links and final helper revalidation; filesystem I/O and
existing bounded Git/source audits are not preempted. Expiry prevents a
successful result; it is not a hard return-time guarantee. The generation owner
still needs independently authenticated portable job export before package
admission.

`compatibility::execute_native_compatibility` accepts an explicit verified
generic LLVM SDK source selection for family-v2 packages. It requires the exact
projected source, compiler target/profile and SDK epoch before creating outputs,
including the source preset's explicit compiler-profile mapping. All execution
outputs must be disjoint from both the committed and projected source trees.
It configures from the source's selectors and scalar bootstrap ABI, builds only
the declared SDK roots, then runs the six compatibility phases and four ordinary
C/C++ links. Its local v6 receipt binds the complete SDK identity, source/CMake
binding, link receipt and inventory digests. Legacy LLVM v2/v4 and GNU v3/v5
receipts keep their original shape. The retained/portable owning-job adapter
selects the generic SDK identity, source-validator response, sealed probes and
measured compiler roles before execution. Its closed LLVM namespace retains
22 files: the CMake binding, independently measured compiler inventory, link
receipt, SDK inventory, four ELF outputs, four maps, two reports and eight logs.
Rootless readback reconstructs the exact ordinary commands, resolves traced
inputs against the selected inventories and checks byte-identical relocation
without opening the original runner paths. The v6 aggregate joins the resulting
proof to the independently selected SDK product/source/ABI identity.

The SDK inventory is a claim about payload bytes, not proof of an SDK archive.
The local receipt's full retained-tree digest cannot be reconstructed from this
subset because the relocated SDK payload is not exported. Neither that field
nor a matching hash authenticates the external job or admits a release. Owning
origin, complete generated payload and package admission remain separate gates.
Input-claim readers require an explicit generic LLVM SDK identity; the legacy
GNU/no-SDK reader rejects such claims instead of silently dropping the policy.

`aros sdk build` uses this generic schema only with a family-v2 LLVM compiler
matching the sealed source profile. A native-consumer-v2 contract selects GNU;
mixed source schemas and compiler families are rejected before preparation.

The identity's epoch is explicit selection, not proof that generation consumed
it. Source verification alone cannot qualify or publish an SDK.

## Bound native generation and package API

`sdk_generation::generate` takes the opaque source selection, independently
selected native compatibility request and recipe-bound compiler profiles. It
requires the exact projected source, preset, content digest and explicit SDK
epoch before execution. The compiler package's source identity remains separate
from the SDK source identity. Native-consumer-v2 generation uses a family-v2 GNU
compiler; generic SDK-consumer-v1 generation uses a family-v2 LLVM compiler.
Both require ordinary original/relocated SDK application links.

The adapter runs all six compatibility phases and four original/relocated
ordinary C/C++ links, then reads back their complete retained evidence. It
derives the generated SDK root from the validated CMake source binding, never
from a caller-supplied candidate path. An absent record outside the protected
source and execution roots binds the SDK identity, source profile, generation
environment, compatibility/link receipt digests and complete SDK inventory.
GNU retains `aros-sdk-generation-record-v1`. LLVM uses the separate
`aros-llvm-sdk-generation-record-v1` and additionally binds the source-derived
CMake SDK binding digest. Record schema versions are independent of SDK product
versions. Local generation and packaging do not qualify the release matrix.

Only successful execution returns an opaque `CompletedSdkGeneration`.
`package_generated` accepts that value, an absent output and forbidden prefixes;
the caller cannot substitute another candidate, identity or inventory. It
revalidates the source and retained evidence around packaging and independently
reads back the complete archive against the generation selection. Changed
records, SDK bytes, maps, outputs or source inputs fail; diagnostic roots and
uncertain published local outputs are retained, not repaired.

This orchestration boundary has assurance
`local-generation-binding-not-release-admission`. `aros sdk build` verifies the
source selection before preparing execution roots, derives the epoch from its
identity, and runs generation and packaging in one process. It accepts no
independent candidate or SDK tree digest. The current generation executor
requires a family-v2 compiler matching the source preset's compiler profile and
family: GNU uses the native consumer contract, LLVM the generic SDK consumer
contract. Unsupported or mismatched selections fail before preparation.
The separate local CLI stages below
retain their byte-consistency claims. Public producer integration, authenticated
owning-job evidence, release index and full release qualification remain required;
neither this record nor its self-hash grants publication rights.

`sdk_generation::generate_with_export` executes the same generation once and
returns its opaque completion together with the existing native compatibility
export. Input claims are measured before execution; the export includes all
six phases and the complete original/relocated SDK link closure. It is not
reconstructed from a generation record or a downloaded receipt.

`aros sdk build --evidence-dir DIR` writes that local export after packaging and
retained-evidence revalidation. The absent directory must be separate from the
generation record, package output and every selected input/execution root. Its
`inputs.json` and `evidence/` are published together without clobbering existing
outputs. Failure retains diagnostic roots and any completed local package;
neither a portable export nor its digests authenticate an external owning job.

## Complete local package

The archive name is `aros-sdk-<sdk-id>-<version>.tar.xz`; there is no host
suffix. A complete local set contains exactly:

- the deterministic archive;
- `<archive>.manifest.json`, byte-identical to the embedded manifest;
- `<archive>.sha256`, selecting the archive filename and measured digest.

The archive begins with `sdk/`. Its `sdk-manifest.json` uses schema
`aros-sdk-manifest-v2`, the exact identity, canonical tree digest and complete
sorted payload inventory. Only this embedded manifest is excluded from the
payload digest. Every other input is preserved, including a file named
`toolchain-manifest.json`. An existing SDK manifest in the candidate is rejected
rather than removed or silently adopted.

The SDK shares the normalized inventory-entry and JSON-line digest primitive
with compiler packages, not their host/profile manifest. Files use modes 0644
or 0755, directories 0755, internal symbolic links 0777. Links must resolve
inside the payload to declared entries or its root; dangling/cyclic links,
case-folding aliases, special objects and archive hardlinks are rejected.
Source hardlinks are not accepted as independent package inputs.

Limits are 100,000 payload entries, 2 GiB of regular payload bytes, 128 path
components and 16 MiB of metadata. Inventory acquisition and file hashing are
bounded; the parser also has bounded XZ memory and stream budgets. Tar metadata
uses root ownership, the selected epoch and global lexical ordering. Long names
use the same deterministic PAX encoding as compiler archives.

## Writer and read-back

The candidate is read-only. The writer requires an independently selected tree
digest, acquires a no-follow bounded snapshot, copies it into private staging,
rechecks the complete inventory and scans selected absolute build prefixes.
It creates the entire package under an existing real output parent and makes
the final absent directory visible with no-clobber publication only after
successful package read-back. Source inputs are revalidated before commit.
It never adopts an existing output or deletes an unrelated directory.

Verification requires an independently selected identity, tree digest, archive
digest and archive byte size. A sidecar alone is not a trusted selection.
It checks the exact local member set, closed bounded metadata, identical
embedded/external manifest bytes, every decoded payload member and hash,
normalized metadata, link containment and the stream terminator. It rechecks
package bytes before returning. Extraction repeats verified stream parsing
into one fresh root, then inventories the materialized payload. A failed
extraction retains its root for diagnosis; it is never adopted as success.

These operations assume a quiescent owned workspace. Descriptor-bound
acquisition and before/after checks detect ordinary mutation; they do not claim
isolation from a hostile concurrent filesystem writer.

## CLI and assurance

`aros sdk package`, `aros sdk verify-package`, `aros sdk sbom` and `aros sdk extract` are explicit local
stages, usable outside an AROS checkout. Their required selectors are shown by
the generated CLI reference. JSON results state
`local-byte-consistency-not-release-admission`.

This boundary does not authenticate the producer, prove that a compiler ran,
establish cross-host SDK reproducibility, substitute for C/C++ relocated-link
qualification, or grant publication authority. The native SDK link receipt
remains separate evidence; an inventory assertion is not SDK archive bytes.

## Supplemental SPDX metadata

`sdk_sbom::build_measured_sdk_sbom` verifies the complete local package before
and after rendering deterministic SPDX 2.3 bytes. It writes nothing and does
not add a fourth member to that package. A shared release stage owns the
supplemental `<archive>.spdx.json` document and its closed release inventory.

`aros sdk sbom` bridges those measured bytes to one absent absolute `--output`
file named `<archive>.spdx.json` outside the three-member package directory.
It uses the same independent identity, tree and archive selectors as
`verify-package`, rechecks the package and identity, and writes through a
no-follow directory descriptor without replacing existing output. The JSON
result records the output path, measured SHA-256 and size. A failed write may
retain output for inspection; it does not authorize publication.

The single SPDX package identifies the SDK product/version and the actual
archive SHA-256, not the payload tree digest. Its timestamp is the selected
source epoch; `sourceInfo` records the exact source, SDK contract, payload
inventory and target descriptor identities. `filesAnalyzed` is false: no file
analysis, third-party component discovery or license completeness is asserted.
Licenses and copyright use `NOASSERTION`; the document data license is CC0-1.0.
The SDK manifest remains the complete payload inventory. This metadata does
not authenticate the generation job or grant release admission.

## Consumer release lock

### Shared release-input selection

The unreleased `aros-toolchain-release-inputs-v2` collection may explicitly
select host-independent SDKs in `sdks`. Each sorted unique selection binds an
exact identity-document filename/hash, source preset and sorted compiler
group/profile references. Its ID must equal the bound SDK product ID, not an
alias. The referenced identity is closed and validated;
its source, target/ABI and independent product version are not inferred from
the compiler recipe. Every referenced document is required, hash-checked and
remeasured during filesystem loading. Missing, extra, changed, ambiguous or
unbound selections fail. SDKs do not multiply the compiler host matrix.

An absent SDK list preserves compiler-only input behavior and index bytes.
Pure V2 index parsing binds exactly one SDK release lock per selected product,
in SDK-ID order, to the exact identity and enclosing release tag/base URL. It
derives the union inventory, including each identity document and archive,
manifest, sidecar and supplemental SPDX; SDKs do not multiply host lanes.
Missing, extra, reordered or swapped SDK locks and filename collisions fail.

This declaration does not admit SDK publication. On Unix, measured shared index
construction and readback verify independently selected SDK packages, exact
identity documents and deterministic SPDX. Final checksum/attestation stages
include all selected SDK members; archive and manifest hashes/sizes must equal
the indexed locks even if a checksum document is freshly regenerated.
Qualification still rejects SDK-bearing releases until authenticated generation
and owning-job evidence are joined. Compiler-only lanes cannot qualify a release
which selects SDKs. A selected ABI descriptor or compiler profile is not proof
of binary compatibility.

`aros-sdk-lock-v2` selects an exact release tag, canonical credential-free
HTTPS base URL, SDK identity, payload tree digest, archive SHA-256/size and
manifest SHA-256/size. Its release version may differ from the SDK version:
one release can distribute independently versioned compiler and SDK artifacts.
Asset names are derived from the SDK identity, not a host or board selector.
Unknown or duplicate fields, URL aliases/credentials, zero digests and oversized
metadata are rejected. The lock itself has a 1 MiB limit and its URL a 4096-byte
limit.

`sdk_release_selection::build_measured_sdk_release_lock` constructs that lock
from a complete local package. The caller independently selects its identity,
payload tree, archive hash/size, release tag and URL. The builder verifies the
closed package, measures the external manifest through bounded no-follow
descriptors, validates canonical lock serialization, and verifies the archive
against the resulting lock. Final package and manifest checks reject changed
inputs. It neither writes package files nor publishes the lock. This measured
selection is not authenticated owning-job evidence or release admission.

The consumer read-back API measures the exact opened archive, checks the
embedded canonical manifest against the independent lock, and verifies the
complete payload inventory. Installed verification repeats bounded no-follow
inventory acquisition, compares all bytes and rechecks the tree and manifest
before returning. Installed payload modes must equal the normalized archive
modes; changed permissions or set-ID bits are rejected, not normalized away.
The enclosing installation root remains the store's private access boundary.
Consumer verification does not infer build-root prefixes;
those are producer qualification inputs. A matching consumer lock proves
selected byte consistency, not publisher authentication or release admission.

## Managed store boundary

The Unix store API uses this host-independent namespace:

```text
<store>/v2/<sdk-id>/<sdk-version>/<canonical-lock-sha256>/
  lock.json
  .complete
  sdk/
    sdk-manifest.json
    ...complete SDK payload...
```

It verifies the selected archive before creating a store namespace. A writer
lock serializes installation; a private sibling stage is fully read back before
durable no-clobber publication. Reuse requires complete installed verification.
Corrupt envelopes are rejected, never repaired or overwritten. Prepublication
private staging is cleaned automatically; an uncertain published destination
is retained and reported with an explicit commit state. Persistent empty
publication locks are recognized only by names derived from actual envelopes.

Read-only inventory has an explicit namespace-entry budget and reports malformed
entries and incomplete coverage. It reads bounded control metadata, does not
hash or execute payloads, and marks every listed SDK `not-checked`. Full
verification is a separate operation. An absent store stays absent; a symlinked
store is reported as unsafe and is not followed. There is no global active SDK
or automatic removal in this API.

`aros sdk download` acquires the exact locked manifest/archive into a dedicated
lock-digest cache namespace and verifies the complete archive. `--offline`
uses existing verified bytes only; poisoned cache entries are not replaced.
`aros sdk install` uses the same retained private snapshots or one explicitly
selected local archive, then installs or strictly verifies reuse. The local
archive selector conflicts with cache/transport options, so no argument is
silently ignored. SDK lock files are bounded and remeasured before/after the
operation. External release locks require an independently selected raw hash;
application project locks are explicit local configuration authority.

`aros sdk list` exposes bounded metadata-only observation; `aros sdk verify`
checks the complete selected installation. `aros sdk path` verifies before
printing an application-build input root. The default store is `AROS_HOME/sdks`;
the default dedicated archive-cache parent is the AROS archive cache's `sdk-v2`
directory. Neither root is created by read-only observation. SDK input paths
never override `AROS_SDK_INCLUDE_DIR`, which is an AROS source-build output.

### Application-project selection

`aros sdk select --release-lock FILE --lock-sha256 SHA256 --project-lock FILE`
previews selection into an explicitly named application-owned file. It requires
an already installed SDK and verifies its complete payload. Preview creates no
project file, directories or guard. Applying requires `--apply TOKEN` from that
exact preview; the token binds normalized absolute paths, candidate bytes,
the prior file identity and bytes, and the SDK identity/tree digest. Changed
inputs require a new preview. It never downloads or installs implicitly.

The project document is the canonical SDK release lock, without local store
paths. Keep it in application version control. One file selects one target/ABI;
multi-target applications use explicitly separate lock files. Replacing a
selection with a different target or ABI is rejected. There is no project-root
discovery, global activation, board inference or compiler-compatibility claim.

Apply serializes cooperative writers with a path-specific sibling guard in the
existing real parent, recomputes the plan, then publishes with no-clobber or
file-identity/hash compare-and-swap and exact readback. An identical canonical
selection is read-only reuse. Symlinks, hard-linked lock files, malformed prior
locks and project destinations inside the SDK store are rejected, not repaired.
If a failure occurs after publication, the project file is retained and the
diagnostic reports the commit state; no rollback overwrites later user changes.

Download, install, verify and path all accept `--project-lock FILE`. A fresh
application checkout can therefore acquire and install its checked-in SDK
selection without an existing installation or a second selector document.
Download and install retain the same exact-byte verification and cache/store
policies; neither operation rewrites the project lock or creates a selection
guard. Offline misses leave absent roots absent. Local-archive installation
still conflicts with cache/transport options.

This is an alternative to the independently selected external
`--release-lock FILE --lock-sha256 SHA256` pair, not an additional selector.
The project lock is local configuration authority, not publisher
authentication. Verify/path create no absent state. All four operations
remeasure the lock after their work; a late failure retains any published
installation rather than rolling it back. `path` returns an application input
directory; it never changes source-generation settings.

This is not a claim of a published SDK. Source-input
SBOM/signatures/attestations, a published SDK index/consumer lock
and release/consumer qualification remain required distribution work. The
three local files are not an SDK release inventory and do not expand or
replace the compiler release asset contract.
