# Declarative boards and RISC-V integration plan

Epic: https://github.com/metaneutrons/aros-tools/issues/319
Decision state: Fabian approved the scope and authorized implementation on
2026-10-02. Plan review, implementation acceptance and release authority remain
separate. Milestone issues own execution status; this document owns requirements.

## Outcome and boundaries

Build the existing ESP32-P4/D1001 AROS port through native `aros-tools`, produce
verified flash artifacts, and qualify reusable RV32/P4 and RV64 AROS
compiler/runtime distributions. Board identities and defaults must come from
reviewed data, not one Rust enum variant per board. Titan is the first RV64
board consumer; it does not define the compiler's identity.

Inspected baseline:

- Tools main `25b2f3c2f66e5bacb5ac6efa192afced4ee081ec` has separate
  `BoardModel` and CLI `BoardInitModel` enums, model-specific defaults and
  templates, and local `boards.toml` format 2. The native producer explicitly
  selects LLVM. The media registry already separates portable layouts from
  local board aliases and guarded device operations.
- The maintainer-provided P4 source at
  `763a536f26a5e94176c821d48cedbcbff03b4189` contains the D1001 board rules,
  an ESP-IDF bootloader dependency and an existing GNU build path. This is an
  inspected port revision, not a new release pin or a reproduced hardware claim.
- Qualified AROS-NX main `b171bfcc6b5942a29d03c1cf1ce1ad31b74e974a`
  contains `opensbi-riscv64`; its source selects `rva22u64`, `lp64d` and
  `medany`. Current tools have an OpenSBI staging path but no published RISC-V
  compiler input. Toolchains v0.1.4 covers PC, ARM and AArch64 only.

In scope: current Pi/Titan configuration compatibility, P4/D1001 native build
and deployment integration, the two stated compiler targets, supported native
hosts, and source-aligned Astro documentation. Source selection remains
explicit; an ESP32 port checkout is not silently imported into AROS-NX.

Out of scope: a new operating-system port; other ESP32 families; unspecified
RV32 multilib variants; support for every RV64 processor; macOS Intel releases;
automatic device writes; arbitrary scripts in profile data; and pretending a
compiler/runtime archive contains a complete Developer SDK. AROS source edits
require Fabian's separate approval. This initiative does not weaken the
existing protection, publication or hardware-interaction rules.

## Design and decisions

### Ownership and identities

1. `aros-toolchains` owns immutable compiler recipes, input locks,
   qualification matrices and compiler/runtime archives. `aros-tools` executes
   the producer and verifies the resulting distributions.
2. The selected AROS source owns target semantics, BSP, linker/memory layout,
   partition rules and Developer sysroot production. Native CMake translates
   these rules; Rust must not duplicate GPIO or flash geometry. A versioned,
   source-bound export may be introduced only with separate source approval.
3. Portable board contracts live in `profiles/boards/` in `aros-tools` and
   reference source presets, compiler profiles, implemented transport
   capabilities and media contracts. Presets may reference these identities;
   they must not maintain parallel lists of board facts.
4. Existing media contracts in `profiles/media/` remain the composition
   authority. Local `boards.toml` selects a reviewed board contract and holds
   operator-specific paths, devices and network configuration.
5. Receipts bind exact board/media/target contracts, selected source revision,
   compiler/runtime identity, Developer inputs and output digests. RV32 and
   RV64 recipes may select different immutable source revisions. Their source
   contracts must remain independent of the existing qualified profiles.

### Registry and CLI migration

Replace board-name enums with validated stable IDs and a deterministic registry.
Existing model IDs and command meanings remain available. Known operation and
format implementations may remain typed; an unknown capability is rejected,
not treated as an executable plugin. A board using existing capabilities must
be addable through a reviewed descriptor without editing a Rust model list.

The initial registry is embedded in the reviewed tools build. Do not introduce
implicit working-directory discovery or shadowing of built-in IDs. Any later
external registry requires explicit selection, unique IDs and receipt binding
to its exact bytes. Profiles never contain commands, credentials or host device
paths. Help, discovery, completions and templates derive from the same catalog.
Existing local format-2 configurations remain readable; migration is explicit
and atomic, and preserves the previous file on validation or write failure.

### Compiler targets, not board-specific compilers

| Contract | Required starting semantics | First consumer |
| --- | --- | --- |
| RV32/P4 AROS | `rv32imafc_zicsr_zifencei_zaamo_zalrsc`, `ilp32f`; source-defined code model and AROS linking | ESP32-P4/D1001 |
| RV64 AROS | Current source `rva22u64`, `lp64d`, `medany`; independently checked compiler support | Milk-V Titan/OpenSBI |

A compatible second P4 or RV64 board can reuse the compiler distribution but
still needs its own source BSP and boot/media contract. A multilib host compiler
may be evaluated later; each target's runtime libraries and ABI remain explicit
qualification units. No additional generic RV32 release is required without an
actual AROS consumer.

The CMake boundary must preserve that distinction: `AROS_TARGET_PROFILE` names
the selected board/source preset; `AROS_CROSS_TOOLCHAIN_PROFILE` names its
declared compiler distribution. The GNU manifest checks the latter and the
build-tree identity binds both when they differ. Do not rename a measured
compiler manifest merely to match a new board preset.

The P4 GNU path is the initial reference to reproduce, not permission to label
an Espressif bare-metal compiler as an AROS toolchain. Source-declared GNU
versions must be resolved to exact available source/patch inputs and measured
compiler behavior before locking them. Existing LLVM-11 packages are not
assumed to support either new contract. LLVM selection or an upgrade requires
its own demonstrated compatibility; do not impose a global upgrade on the
qualified PC/ARM/AArch64 profiles.

### Native build and deployment boundary

Kernel, BSP, generated headers, Developer libraries and AROS link semantics
belong to the source-derived native build graph. The separately locked
ESP-IDF bootloader is an external producer, not the AROS platform SDK. Vendor
Python dependencies may be isolated and locked; our orchestration stays Rust.
No successful native result may substitute pre-existing Make core/BSP outputs.

Local RV3 build evidence must remain distinct from release provenance. An
explicit byte-verified local compiler descriptor carries no release ID or
producer attestation; the source export's recorded baseline is not the current
dirty checkout identity. Capture the actual source snapshot before building,
bind it together with the raw native-contract digest and the validated input
inventory, and remeasure before receipt publication and media composition.
Source measurement must include submodule content and uncommitted inputs,
exclude Git metadata and only the untracked CLI-owned generated build tree,
and reject unsafe links or special files. Do not weaken the existing clean
source/released-toolchain receipt contract to accommodate this local path.
Compiler admission alone is not native payload or media qualification.

The local native CLI records a pre-build input stamp, separate from the
successful-build/media receipt. It binds the whole measured checkout, native
contract, compiler descriptor/tree, engine files, suite executables, selected
CMake/Ninja/cache executors, configure arguments and hashes of the declared
build environment. Reconfiguration and build recheck these identities.
The selected generated namespace must be untracked and free of symlink/file
prefixes before cleanup. An engine override must contain regular files and
directories only. Existing unstamped output trees are not adopted. Persistent
CMake cache and the bounded literal Ninja include/subninja closure are sealed;
changed rules require a fresh tree or explicit cleanup after retaining evidence.
Dynamic Ninja include expressions are refused rather than approximated.
Cached program paths resolve before the generated-product exclusion, so a
build-local link cannot hide an external executable. Explicit configured DTB
and consumed KOBJ inputs remain byte-bound even inside the selected build;
those are direct inputs, not proven generated products. The closed member
set mirrors the existing CMake consumers, not every file in an external SDK.
These checks do not claim isolation from a hostile concurrent filesystem
replacement, a complete environment attestation or successful P4 compilation.

An active MetaMake dependency needs a real source-owned producer. A selector
or a commented-out owner does not make the edge optional. Legacy
`optional_meta_dependencies` declarations remain validated against their exact
hash-bound recipes, but cannot remove edges. Missing endpoints fail with the
recipe and `fix upstream`; supported siblings and unsupported active providers
retain their normal selection checks. A diagnostic-only graph audit retains
these failures and sibling evidence without publishing a graph or inventory.

Native owner projection and parsing use the same effective inputs:
`mmakefile.src` supersedes its generated `mmakefile`, while direct fragments
remain inputs. The complete discovery set, project ignore policy and captured
bytes are rechecked before admission. Added or removed recipes require a new
projection; a stale generated sibling cannot add edges to the selected graph.

Flash-image planning validates exact chip/silicon compatibility, partition
ranges, capacity, required roles and output identity before mutation. Wrong
ABI, image header, partition overlap or source/profile pairing fails closed.
Device writing is a separate explicitly authorized operation with exact device
and range selection, recoverable backup where applicable, and readback.
Bootloader/partition-table writes need specific approval. Interactive acceptance
requires Fabian to confirm availability; a headless log is not visual evidence.

## Delivery and acceptance

### RV1: Declarative board registry

Execution: https://github.com/metaneutrons/aros-tools/issues/320
Dependencies: none

- RV1-A1: One closed, versioned registry resolves unique stable board IDs,
  source-target/toolchain references, implemented transports and conservative
  defaults. Current Pi 3/4/5 and Titan fixtures pass. Unknown fields/versions,
  duplicate IDs, unsafe references and unsupported capabilities fail with
  tested diagnostics. Resolved descriptors record their raw-byte digest.
- RV1-A2: Remove the model enums and duplicated CLI model list. Existing
  init/build/deploy/SD commands and format-2 configurations retain their
  documented behavior. Discoverability and templates use the registry. Tests
  exercise actual CLI parsing, unknown models, incompatible transports,
  explicit overrides, preview/apply and failure without prior-file mutation.
- RV1-A3: Astro, examples and registry documentation match shipped capability.
  Portable tests cover the three active hosts; migration fixtures and retained
  Pi/Titan image/deploy tests pass. A new data-only board fixture resolves
  without adding a Rust model variant; it is not a hardware-support claim.

### RV2: Compiler and producer contracts

Execution: https://github.com/metaneutrons/aros-tools/issues/321
Dependencies: exact selected source inputs; no dependency on published new SDKs

- RV2-A1: Reproduce and record the RV32/P4 reference compiler's source/patch,
  host, ISA, ABI, linker, runtime and sysroot behavior. Probe the stated RV64
  contract separately. Validate real ELF class/machine/flags/attributes and
  C/C++/assembly links against AROS inputs. Wrong-width, wrong-FPU/ABI,
  incompatible runtime and host-library substitutions fail intentionally.
- RV2-A2: Recipe, producer, preflight, package, collector and verification
  contracts select a demonstrated compiler family instead of forcing LLVM
  filenames, versions or runtime layout. Local one-host build/package/relocate
  probes pass. Existing PC/ARM/AArch64 contracts retain regression coverage.
  Exact target/source/compiler relationships are tested independently.
- RV2-A3: Record the selected compiler strategy and unresolved limitations in
  durable evidence. GNU/LLVM version declarations alone are not build proof.
  No runtime capability, SDK completeness or physical boot is inferred.

### RV3: Native ESP32-P4 build and artifacts

Execution: https://github.com/metaneutrons/aros-tools/issues/322
Dependencies: relevant RV1/RV2 contracts; local verified RV32 prefix

- RV3-A1: A fresh isolated native graph builds the P4 kernel/BSP and required
  Developer inputs from the selected port source without Make-produced payload
  substitution. Compare source-derived object lists, sections, linking,
  generated ABI headers and package format against the reference rules.
- RV3-A2: Locked bootloader/external inputs and source-owned board/partition
  facts produce a complete declarative flash plan and independently verified
  artifacts. Altered input, wrong chip/revision, overlapping or oversized
  ranges, unsafe paths and mixed core/BSP contracts fail before publication.
- RV3-A3: CLI process-boundary tests and Astro describe the actual native
  workflow and experimental state. Record exact code/source/toolchain and
  artifact identities. No device write or hardware success is required or
  implied by this build-only milestone.

### RV4: Immutable toolchain releases

Execution: https://github.com/metaneutrons/aros-tools/issues/323
Dependencies: RV2; RV3 consumer evidence for RV32/P4

- RV4-A1: Publish the required tools runtime through its established checked
  Release Please/release path before using it in the producer. Separately
  qualify RV32/P4 and RV64 on Linux x86-64, Linux ARM64 and macOS ARM64,
  including first-target A/B, compatibility, AROS final links and relocation.
  Existing target releases and source pins are never retargeted.
- RV4-A2: Each compiler release has a complete declared inventory, measured
  hashes/sizes/tree digests, source/producer/tools identities, receipts, SBOMs,
  signatures and provenance. Independently verify the isolated draft before
  unchanged publication, then public URLs and immutable state. No partial
  matrix or fallback source build qualifies a failed release.
- RV4-A3: Consumer locks contain measured values only. Fresh isolated public
  installs/verifies and representative native AROS consumer builds pass.
  Published documentation distinguishes compiler/runtime distributions from
  separately produced Developer sysroots and board readiness.

RV64 design and local compiler work may proceed independently of RV3. Its
release need not wait for the P4 image; the two target contracts still require
their own complete evidence. Narrow producer implementation issues live in
`aros-toolchains` and link RV4 rather than duplicating milestone acceptance.

### RV5: D1001 end-to-end qualification

Execution: https://github.com/metaneutrons/aros-tools/issues/324
Dependencies: RV1, RV3, relevant RV4 released inputs

- RV5-A1: Run documented commands verbatim with released tools and compiler
  inputs to build, inspect and verify the exact P4 artifacts. Observe the
  authorized deployment/readback and actual D1001 AROS readiness, preserving
  source/board/toolchain/image identities and durable serial evidence.
- RV5-A2: Exercise the port's applicable safety/acceptance procedures without
  relabeling workarounds, warm resets or headless output as complete physical
  or interactive acceptance. Unavailable hardware leaves this gate open.

Titan native-media and physical acceptance remain exclusively in
[BM5](https://github.com/metaneutrons/aros-tools/issues/278), against the
[boot-media plan](boot-media-plan.md#bm5-milk-v-titan-opensbiuefi-sd-media).
BM5 consumes the RV1/RV2/RV4 results; it is not replaced by this initiative's
compiler probes. BM4 remains the independent Pi-native-media track. Astro
alignment accompanies every milestone, not a deferred final documentation task.

The initiative closes only after RV1-RV5 and the referenced BM5 acceptance are
satisfied. Compiler publication and physical board qualification are separate
claims. No separate public boot-image release is included.

## Migration, risks and verification cost

Deliver registry foundations, consumer migration, compiler-family support,
target recipes and native P4 rules as focused reviewed slices. Local verified
prefixes unblock native development before release; released tools unblock the
producer; released compiler inputs unblock final board acceptance. This avoids
a tools/toolchain/hardware dependency cycle.

Keep old tools/configuration/artifacts usable for rollback; do not rewrite
historical tags or locks. An explicit configuration migration preserves a
recoverable old file. New registry identities cannot silently override a
reviewed built-in model. Real writes are outside the first implementation slice.

Risks include GNU/LLVM consumer assumptions, compiler source availability,
source-dependent CMake translation, separate P4/NX source revisions, firmware
licensing and access to hardware. New AROS source changes require approval,
then the port's own progress/evidence rules. Never build concurrently in an
existing maintainer-owned port tree.

Use focused registry, parser, ELF/ABI, corruption and process-boundary probes
during iteration. Reserve full native matrices and first-target A/B compiler
builds for defined integration/release gates. The preliminary 26-52 active-hour
estimate includes CI implementation and diagnosis, excludes passive runner
waits, and is not a deadline; revise it after the compiler/native-build spike.

## Decision changes

Reviewed changes to scope or criteria must name their date, PR and affected
evidence here. Milestone acceptance references an exact plan revision.
