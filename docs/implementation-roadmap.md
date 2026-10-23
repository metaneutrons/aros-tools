# Implementation and qualification roadmap

Decision state: **proposed**, refreshed 2026-10-19. Not a release authorization.

This is the proposed common entry point for all 14 open aros-tools issues
confirmed open on this date (twelve work issues and two existing epic indexes).
The fresh inventory found no open Tools PRs. Existing issue history, accepted evidence and detailed
acceptance criteria are retained. No issue is closed or superseded by drafting
this document. Adoption requires review; until then the existing plans remain
the acceptance authorities.

## 1. Outcome and authority

Deliver working, reproducible compiler/SDK products and truthful native
build-to-board workflows. Keep three claims separate:

1. Tools functionality works against the selected real AROS-NX tree.
2. Compiler/SDK products are independently qualified and published.
3. A named boot medium and board workflow are qualified on the claimed hardware.

A passing fixture, merged implementation, package inventory or local compiler
probe cannot stand in for a later claim.

**Human hold:** finish Mesa, then review further issues before choosing the next
release contents. Even complete Mesa success does not permit a release.
Publication needs renewed explicit approval. No new compiler/SDK diagnostic,
protected producer activation, signing experiment or publication is authorized
by this roadmap. Development continues; the overall goal is not paused.

The first compiler/SDK distribution comprises four profiles across Linux
x86-64, Linux ARM64 and macOS Apple silicon:

- GNU RV32/P4;
- LLVM PC x86-64;
- LLVM Raspberry Pi ARM;
- LLVM Raspberry Pi AArch64.

RV64 remains part of the wider initiative, as a separate follow-on qualification.
It is not silently removed when the first distribution passes. macOS Intel is
not an active native Tools release target; macOS baseline is 15.

## 2. Records and consolidation

Use one roadmap, not one replacement mega-issue:

- This document coordinates sequence, cross-project interfaces and common gates.
- Existing detailed plans own their domain-specific acceptance criteria.
- Existing issues own live status and criterion-mapped evidence.
- [#319](https://github.com/metaneutrons/aros-tools/issues/319) remains the
  board/compiler initiative index.
- [#273](https://github.com/metaneutrons/aros-tools/issues/273) remains the
  boot-media initiative index.
- [#323](https://github.com/metaneutrons/aros-tools/issues/323) coordinates
  compiler/SDK release evidence, not every implementation detail.

On adoption, replace contradictory **current-status** paragraphs in existing
issues and cross-link this roadmap. Retain dated history and original scope.
Do not copy the same live checklist into all three indexes. Split the first
distribution and later RV64 evidence into sections of RV4; completing one must
not automatically close the other. No additional epic is necessary.

### Complete open-issue mapping

| Issue | Delivery group | Distinct outcome retained |
| --- | --- | --- |
| [#353](https://github.com/metaneutrons/aros-tools/issues/353) | M1 | Configured Mesa compilation and Make/GCA ABI parity |
| [#254](https://github.com/metaneutrons/aros-tools/issues/254) | M1 | Closed, reproducible GCA provider and driver-rewrite products |
| [#313](https://github.com/metaneutrons/aros-tools/issues/313) | M2 | One effective nested build budget |
| [#317](https://github.com/metaneutrons/aros-tools/issues/317) | M3 | Evidence-based Release Please finalizer diagnosis/correction |
| [#322](https://github.com/metaneutrons/aros-tools/issues/322) | M4 | Native P4 build and complete verified flash artifacts |
| [#323](https://github.com/metaneutrons/aros-tools/issues/323) | M5–M7 | Qualified released compiler/SDK inputs, first distribution and RV64 follow-on |
| [#324](https://github.com/metaneutrons/aros-tools/issues/324) | M8 | Released-input D1001 end-to-end workflow |
| [#277](https://github.com/metaneutrons/aros-tools/issues/277) | M8 | Model-specific native Pi SD media and physical boot |
| [#278](https://github.com/metaneutrons/aros-tools/issues/278) | M8 | Qualified RV64/native Titan UEFI SD and physical boot |
| [#279](https://github.com/metaneutrons/aros-tools/issues/279) | M8 | Verbatim-tested platform documentation and truthful qualification policy |
| [#348](https://github.com/metaneutrons/aros-tools/issues/348) | M9 | Reviewed Rust edition 2024/resolver 3 migration |
| [#347](https://github.com/metaneutrons/aros-tools/issues/347) | M9 | Explicit assert-is-empty lint policy |
| [#319](https://github.com/metaneutrons/aros-tools/issues/319) | Index | RV1–RV5 plus the separate Titan dependency |
| [#273](https://github.com/metaneutrons/aros-tools/issues/273) | Index | BM1–BM6 acceptance, without duplicate milestone ownership |

## 3. Evidence baseline, not completion

The reviewed protected main is
`9cdd864584eac3bd60892971d9cf21ef1c54b03d`. No open Tools PR was present
at the review snapshot. Tools 0.6.0 publication is complete; it is not a
compiler/SDK release. Tools 0.6.1's version PR was merged, but its main Workspace
admission was explicitly cancelled. Do not treat that cancellation as green.

The Mesa future branch contains a local, unpushed measured-preflight commit
`29e798ba871083817c6ecd783597ae0cb6633ad8` and further uncommitted ordered-option
work. These are not current-main production integration. The local shared-budget
branch is also a feasibility prototype, not production acceptance.

The latest local Mesa evidence includes ARM/AArch64 tiny-object option
projection and a two-pass data-only configuration handoff. Neither establishes
complete target/compiler selection, actual preflight ownership in the native
executor, production compile/hash activation, full Mesa products or SDK
generation. In particular, a file plus its digest is not evidence that its
claimed producer executed. These implementation slices do not satisfy M1-G1
through M1-G4 by themselves.

The immutable full diagnostic `38777083911` passed 24 compiler owners and failed
all 12 SDK jobs. Local configured-source probes, a PC kernel build and SDK
consumer links do not supersede that failed end-to-end result.

RV1/RV2 and BM1/BM2 have accepted historical scope. BM3 additionally has a
recorded macOS ARM64 PC BIOS ISO/QEMU acceptance on its exact older inputs.
None of these accepts newer-code full products, Pi/Titan physical boot or a
current full compiler/SDK release.

## 4. Delivery sequence and success gates

### M1 — Complete Mesa correctness

Execution: #353 and #254, with separate evidence and closure decisions.
Requirements: [configured ABI plan](mesa-configured-abi-contract-plan.md) and
#254's original provider/rewrite acceptance.

- **M1-G1 — Measured input:** the selected compiler/source configuration runs
  in a closed environment with compiler construction disabled, before the first
  engine configure. Both CLI and native compatibility entry paths deliver its
  validated result and receipt to every transpiler pass. Source, compiler, environment and generated-output
  drift, missing selectors and changed recipes reject before graph publication.
  The execution owner retains and revalidates the completed producer before and
  after graph execution and binds it into aggregate readback. Match the full
  selected profile, platform, ABI/MMU and compiler identity, not only a CPU
  prefix. A reused cache must not activate stale configured inputs; source-local
  generated directories need one explicit producer/consumer exclusion contract.
- **M1-G2 — Actual compilation:** C, C++ and special bridge rules preserve
  source option order, repetitions, definitions, undefinitions, includes and
  language-specific ISA. Compare actual emitted compiler commands, not only
  component vectors or a serialized file. Keep configured recursive TARGET
  expressions responsive to declaration-local USER assignments.
- **M1-G3 — ABI parity:** on ARM and AArch64, identical measured configurations
  produce matching Make/engine GCA assignment strings and generated ABI hashes.
  Switch actual compilation and hashing together. Keep the existing honest
  own-compiled-flags hash until that gate passes.
- **M1-G4 — Product closure:** real AArch64 VC4/V3D HIDD and mesa3dgl products
  have the required symbol closure; generator outputs are reproducible. Changed
  import sets/order, wrong architecture and changed tool identity reject.
- **M1-G5 — Compatibility:** determine with evidence whether the supported
  Mesa 20 path needs the same treatment; implement required corrections and
  preserve its independently tested behavior. Gallivm/LLVMpipe may enter only
  with the selected target LLVM runtime. PC shader probes do not replace ARM
  product evidence.

Treat M1 as one integration workstream, not as two issues that can close each
other. Source-driven projection, provider graph integration and final
interoperability tests may be staged internally without circular issue closure.

**Decision checkpoint H1:** after M1, review remaining issues with Fabian.
Review the then-current evidence and explicitly select which subsequent work
to start, including M2/M3/M4/M8/M9 where relevant. This proposed ordering does
not itself authorize that work. Preserve specifically existing implementation
authority where applicable; resolve scope changes explicitly. Do not
automatically publish or start a diagnostic. Release approval H2 is separate.

### M2 — Make the build budget real

Execution: #313. Independent implementation, ordinarily after the Mesa review.

- **M2-G1:** document precedence for explicit/absent CLI jobs, user environment
  and inherited jobserver state. Define a global budget rather than multiplying
  per-child limits.
- **M2-G2:** exercise admitted Make, Ninja/CMake, Meson, configure and GRUB
  producers, including supported older versions. Under budget N, overlapping
  work peaks at no more than N; a deliberately oversubscribing case is detected.
- **M2-G3:** failure, interruption and cancellation do not leak workers or tokens.
  An unchanged-source bounded native LLVM build passes. CLI help and Astro
  describe the same verified semantics.

### M3 — Diagnose and fix release finalization narrowly

Execution: #317. No new publication is needed merely to test diagnostics.

- **M3-G1:** diagnostics identify the failing stage, method, sanitized endpoint,
  HTTP status and accepted-permissions information without exposing credentials.
  Positive and synthetic 403/malformed-response/secret-redaction cases pass.
- **M3-G2:** determine the actual failing request and token context before
  changing endpoint or permissions. Maintainer-token success is not proof for
  the workflow token. Do not broaden credential access speculatively.
- **M3-G3:** the corrected normal finalization path is idempotent and its
  postcondition is read back. Before closure, verify the path with the actual
  scoped workflow token in a metadata-only test, without release publication.
  Any necessary live mutation needs separate authority; leave the issue open
  until that evidence is authorized and obtained. Fixture success is not live closure.

### M4 — Finish native P4 build/artifact integration

Execution: #322, using accepted RV1/RV2 contracts.
Requirements: [RV3 criteria](riscv-board-integration-plan.md).

- **M4-G1:** fresh isolated native kernel, BSP and Developer outputs are produced
  from the selected source without Make-built payload substitution.
- **M4-G2:** source-bound bootloader, partition/board inputs and the complete
  flash plan are independently verified. Wrong ELF/board, missing members,
  altered ranges and incomplete plans reject without device mutation.
- **M4-G3:** CLI/Astro describe the tested native workflow and its limits.
  This milestone needs no flash or hardware readiness claim.

### M5 — Qualify the integrated unreleased Tools candidate locally

Execution coordination: #323; component issues remain their own owners.
Dependencies: M1 and the actually required consumer/finalization fixes selected
at H1, plus any explicitly selected maintenance work. M2, M3 and M4 are not
blanket prerequisites merely because their issues are open; an unsatisfied
contract actually exercised by the chosen release remains a blocker.

- **M5-G1:** freeze one exact Tools/source/dependency candidate. Run the complete
  source-coupled workspace integration gate and every actually affected native
  AROS-NX build/SDK workflow with the locally built, unreleased Tools binary.
  This is the local dirty-candidate integration path, not the source repository's
  published-installer nine-product matrix; that matrix belongs to M6-G3.
  Exercise all four intended SDK products on the developer host; do not count
  an isolated object, kernel or existing-SDK consumer link as SDK generation.
- **M5-G2:** bind identities, generated inventories and ordinary/relocated C/C++
  consumers to those fresh outputs. Include relevant corrupted/stale/wrong-ABI
  rejection cases. Functional failures are fixed here, not first discovered by
  release CI.
- **M5-G3:** final applicable Tools CI, source-coupled engine tests and native-host
  gates use the same candidate tree. Linux x86-64 is the development default;
  native platform behavior and the actual release matrix retain their required
  hosts. The source repository's released-installer product matrix follows at
  M6-G3; it cannot require an unpublished release as an already published input.

An unreleased Tools/source pairing is an explicit development input, not a fake
published version or permission to weaken source cleanliness/seals.

### M6 — Publish Tools, then qualify its source consumers

Execution coordination: #323 and the existing normal Release Please process.
Dependencies: M5 and **renewed explicit human release approval H2**.

- **M6-G1:** settle the exact candidate/version and normal release-admission
  continuation before any action that could resume publication. No manual tag,
  draft, bypass or implicit revival of cancelled runs.
- **M6-G2:** exact-candidate native release, independent comparisons, clean-room
  package checks, signatures/attestations and immutable inventory gates pass.
  Public GitHub, signed APT, Homebrew and AUR readback matches built bytes.
- **M6-G3:** only then replace source installer archive hashes/sizes and runtime
  pins with the authentic published Tools identities. Qualify all required
  nine-product AROS-NX matrix plus preparation/hygiene/aggregate gates and
  integrate normally.

This deliberately breaks the apparent dependency cycle: test the explicit
unreleased pairing locally first; publish Tools only with approval; then use
authentic released Tools in final source CI. It does not require publishing a
new Tools version for every debugging experiment.

### M7 — Qualify and release compiler/SDK products

Execution: #323. Dependencies: M4's relevant consumer evidence, M6 and coherent
qualified Source/Tools/Producer pins.
Requirements: [RV4-A1–A3](riscv-board-integration-plan.md#rv4-immutable-toolchain-releases)
remain binding for both the first distribution and RV64 follow-on, including
three-host qualification, immutable inventories/provenance, public readback and
fresh isolated consumer installations. This summary does not narrow them.

- **M7-G1 — First distribution:** all 24 authenticated compiler A/B owners,
  12 byte-comparison/compatibility/relocation lanes and 12 fresh SDK generations
  pass for the frozen release candidate's exact applicable inputs across the
  four profiles on three native hosts. Historical passes do not transfer after
  input changes; any reuse must be explicitly admitted and revalidated by the
  normal provenance contract, otherwise obtain fresh authorized qualification.
  Verify ordinary and relocated C/C++ consumers, cross-host SDK equality and authenticated final
  readback. The four independent initial SDK 0.1.0 products ship together.
- **M7-G2 — Publication:** normal protected signing/publication and independent
  public consumer verification pass for the exact qualified products. A
  diagnostic success is not publication readiness or a published SDK.
  **Decision checkpoint H3:** after qualification, obtain explicit approval for
  compiler/SDK publication. Tools release approval H2 is not that approval.
- **M7-G3 — RV64 follow-on:** independently satisfy the wider RV64 compiler,
  runtime, A/B, compatibility, consumer and publication contract. The first
  RV32-first distribution does not close this part of RV4.

New full diagnostics, protected activation/signing experiments and compiler/SDK
publication still require their own explicit authorization. Preserve original
failed attempts; never adopt failed SDK owners or relabel local reuse as new
authenticated execution.

### M8 — Qualify native media and real board workflows

Execution: #324, #277, #278 and #279; indexed by #319 and #273.
Requirements: [RV5](riscv-board-integration-plan.md) and
[BM4–BM6](boot-media-plan.md). Dependencies apply to the relevant released
products, not blindly to closure of an entire umbrella issue.

- **M8-G1 — D1001:** run the documented end-to-end workflow with released P4
  inputs, verify exact artifacts/deployment/readback and observe the required
  physical readiness with durable serial evidence. Applicable hardware safety
  procedures and live-user synchronization remain mandatory.
- **M8-G2 — Pi:** qualify native payloads, locked firmware/DTB profiles, SD
  composition/readback and actual boot separately for every claimed Pi 3/4/5
  model. Reject mismatched model inputs, missing payloads and corrupted media.
  Pi 4 USB-ECM is not native SD acceptance.
- **M8-G3 — Titan:** consume qualified RV64 inputs, produce native core/BSP/UEFI
  products including all five required UEFI payloads without legacy KOBJ
  substitution, verify media and physical readiness.
  Wrong PE architecture, absent BSP, changed startup input and corruption reject.
- **M8-G4 — Documentation:** execute published commands verbatim on their claimed
  hosts/boards. Update support claims only from accepted platform evidence.
  Public boot-image publication remains a separate decision.

### M9 — Optional workspace maintenance selected at H1

Execution: #348 and #347, distinct acceptance even if scheduled together.

- **M9-G1:** edition 2024/resolver 3 migration has reviewed semantic and formatting
  changes, a deliberate toolchain/MSRV/lockfile decision, and passing portable,
  source-coupled and affected engine integration tests.
- **M9-G2:** remove the assert-is-empty allowance with strict Clippy passing, or
  record an explicit permanent policy and remove the temporary revisit promise.
  An existing green run with the allowance is not a resolution.

Neither maintenance issue is an automatic compiler/SDK release blocker merely
because it is open. Decide whether to include it before the candidate freeze;
do not introduce an edition migration late into a qualified release candidate.

## 5. Verification and delivery discipline

For every claimed gate retain: exact revisions and candidate tree, source and
compiler identities, host/tool versions, command, exit result, artifact digests,
positive/counter results and omissions. Evidence for an older candidate remains
historical. Fixes which alter affected inputs invalidate the relevant result.

Use focused local tests during implementation. Run full integration at coherent
final-candidate checkpoints, not after every scaffolding edit. Integrate focused
PRs serially and qualify only the next merge candidate when base movement would
invalidate expensive work. Do not weaken required checks or use stale greens;
do not repeat unrelated expensive product gates for prose-only changes.

The common roadmap must not turn into “close every open issue before any
release.” A release train selects explicit scope; wider RV64 and hardware
requirements stay visible and independently open. Conversely, a smaller first
release must not be presented as completion of the full initiative.

## 6. Adoption decisions and migration

The immediate implementation priority remains M1. The next planning decision is
H1, not a new release: select the next bounded scope from measured blockers,
then freeze and qualify that candidate. New issue structure, issue closure and
publication are not side effects of accepting this draft for discussion.

Before adopting the plan:

1. Confirm M1 and the mandatory post-Mesa H1 scope review; later milestone
   ordering remains a proposal, not advance release authorization.
2. Approve this common entry point and issue mapping without replacing historical
   evidence or lowering any linked criterion.
3. Update current-status sections and backlinks in the two existing epics and
   affected issues. Keep one evidence owner per deliverable.
4. Only after those decisions, prepare the focused documentation PR. This draft
   creates no new GitHub epic, milestone, closure, run or release.

Open decisions: exact next release contents/version; authorized qualification
and publication steps after M5; hardware availability for M8; whether each M9
policy is adopted. No schedule or completion date is asserted without evidence.
