# Boot media initiative plan

Epic: https://github.com/metaneutrons/aros-tools/issues/273
Decision state: proposed for review; Fabian authorized planning and implementation on 2026-09-28.

## Outcome and boundaries

From a supported AROS checkout and locked external inputs, `aros-tools` must
produce a verifiable boot medium without requiring a second, legacy Make build.
The user selects a build preset and a compatible media profile; the resulting
artifact records the exact source, toolchain, profile and input identities.
Creating an image and proving that it boots are distinct outcomes.

The measured baseline is uneven. The CMake `boot-iso` target packages an
existing x86-64 PC SYS tree into a BIOS ISO. `rpi-artifacts` stages Pi payloads
but does not assemble a complete native SD medium. `opensbi-uefi-artifacts`
stages the five Milk-V Titan UEFI files and emits a `boot-bundle.toml` that the
existing SD-image command can consume. Pi and OpenSBI CMake payloads still
require architecture-matched kernel/exec/task objects from a legacy build.
The current SD implementation knows fixed Pi 4 and UEFI file lists. There is
no published RISC-V toolchain archive or qualified Milk-V physical boot.
These statements describe capabilities, not release qualification.
The engine already has a kickstart-object generator, but the Pi and OpenSBI
paths do not request its output. Its current fallback to a whole runtime module
must not be used as a substitute for a core KOBJ.

In scope: PC x86-64 BIOS ISO, native SD boot media for Raspberry Pi 3/4/5,
and UEFI SD boot media for Milk-V Titan. The existing Pi 4 U-Boot USB-ECM
transport must remain compatible but is not a substitute for native SD boot.
The build and media path must work with a pristine upstream AROS checkout when
its required source capabilities exist; AROS-NX-only behavior remains explicit.

Out of scope: an ISO for every board, new AROS installation images, a generic
script runner hidden in profile data, automatic writes to removable devices,
and publication of unqualified media. This plan does not authorize edits to
AROS-NX source; obtain Fabian's separate approval before any such edit.

## Design and decisions

Portable, reviewed media profiles live in `profiles/media/` in this repository.
Each profile declares a stable ID, compatible target preset(s), board model or
PC platform, required artifact roles, destination paths, boot protocol, medium
layout and references to separately locked external inputs. `aros-targets.toml`
continues to define target/compiler semantics: one target can serve several
media profiles. A build preset may select a default only when the mapping is
unambiguous. Local `boards.toml` selects a profile and holds lab-specific
devices, paths and networking; its arbitrary board name is not a portable
artifact identity.

CMake owns source-dependent compilation, linking and staging. It emits a
versioned receipt for the files actually built. A composition step combines
that receipt, the reviewed media profile and hash-verified external inputs.
`aros-tools` owns closed-schema validation, safe paths, deterministic assembly,
read-back verification and guarded device writing. Format backends may use
audited external tools; profile files never contain commands to execute.
Architecture-specific filenames and role lists belong in profiles, not Rust.

The boot-bundle schema must distinguish the stable model from a local profile
alias. Every output records the media-profile digest, source and toolchain
identities, exact regular-file inventory, input digests and final image digest.
Self-declared checksums alone are not origin authentication: profile and
external-input locks must be bound to reviewed repository revisions.

The proposed general CLI is `aros image build`, `aros image inspect` and
`aros image verify`. The existing `aros board sd image` and `sd write` paths
remain available during migration. Device writes remain a separate action
requiring exact device selection and explicit confirmation.

## Delivery and acceptance

### BM1: Versioned media and artifact contracts

Execution: https://github.com/metaneutrons/aros-tools/issues/274
Dependencies: none

- BM1-A1: A closed, versioned media-profile schema and built-artifact receipt
  distinguish target preset, stable platform/model, media format, required
  roles, paths, locked external inputs and local board aliases. Valid PC, Pi
  and Titan fixtures pass; unknown fields, duplicate destinations, path
  traversal, symlinks, incompatible targets and altered digests fail with
  tested exit status and diagnostics.
- BM1-A2: One repository registry supplies the portable profiles without
  copying layouts into Rust or target presets. Profile selection is explicit
  when several media profiles match a target. The resolved profile digest is
  recorded in a receipt. Pristine-upstream and AROS-NX selection fixtures
  remain distinct and documented.
- BM1-A3: The existing Pi 4 and Titan bundle contracts have an explicit,
  tested migration path; no currently valid user command silently changes
  meaning. Astro reference and CLI examples agree with the implemented state.

### BM2: Generic media composition and verification

Execution: https://github.com/metaneutrons/aros-tools/issues/275
Dependencies: BM1

- BM2-A1: One planner validates the build receipt, selected profile and
  external locks before any output mutation. It emits a complete intended
  filesystem/partition plan, including exact file placement and capacity.
  Positive and deliberately corrupted input probes prove rejection behavior.
- BM2-A2: Supported ISO and MBR/FAT32 backends create images in isolated
  staging and atomically publish them. Repeated builds from the same inputs
  produce byte-identical outputs or record an explicit, reviewed exception.
  `inspect` and `verify` independently read back the final image, not merely
  the input manifest.
- BM2-A3: CLI process-boundary tests cover dry run, exit codes, JSON
  diagnostics, preserved prior output on failure and unchanged guarded SD
  writing. No profile can execute arbitrary shell commands or select a host
  device for writing.

### BM3: PC BIOS ISO from a native CMake build

Execution: https://github.com/metaneutrons/aros-tools/issues/276
Dependencies: BM1, BM2

- BM3-A1: A fresh supported host builds the required PC SYS modules, GRUB
  inputs and ISO through the declared target graph, without an untracked
  pre-existing SYS tree or legacy Make output. The ISO inventory, El Torito
  entry and all recorded inputs pass read-back verification.
- BM3-A2: QEMU boots the generated ISO beyond the firmware/GRUB loader to a
  documented AROS guest readiness signal. A missing kernel or altered GRUB
  input fails before publication. The result names the exact source, tools
  and compiler revisions and ISO digest.

### BM4: Native Raspberry Pi SD media

Execution: https://github.com/metaneutrons/aros-tools/issues/277
Dependencies: BM1, BM2

- BM4-A1: Pi 3, Pi 4 and Pi 5 CMake builds produce their required AROS
  payloads without `AROS_RPI_CORE_KOBJ_DIR` or another legacy build. The
  source-derived linker and bootstrap semantics are verified independently;
  missing mirrored objects fail rather than falling back to runtime modules.
- BM4-A2: Each model has a reviewed media profile and hash-locked firmware/
  DTB inputs. Real SD images pass filesystem, placement and hash read-back
  tests; one model cannot silently accept another model's inputs. The Pi 4
  U-Boot USB-ECM path remains separate and compatible.
- BM4-A3: An actual boot of each claimed model reaches a documented AROS
  readiness signal, with durable serial/video evidence and exact image digest.
  Without hardware evidence the model remains experimental, not accepted.

### BM5: Milk-V Titan OpenSBI/UEFI SD media

Execution: https://github.com/metaneutrons/aros-tools/issues/278
Dependencies: BM1, BM2, a qualified RISC-V compiler/toolchain input

- BM5-A1: A fresh CMake build produces its OpenSBI core, BSP and five UEFI
  payload files without `AROS_OPENSBI_CORE_KOBJ_DIR` or another legacy build.
  Missing mirrored core objects fail rather than linking runtime modules.
  The payload tree and build receipt pass independent verification.
- BM5-A2: A model-specific profile composes and read-back verifies the UEFI
  SD image; firmware/toolchain inputs are pinned and the bundle is reusable
  across local board aliases. Wrong PE architecture, missing BSP, changed
  startup command or corrupted file fails before image creation.
- BM5-A3: A physical Titan boots the exact image to a documented AROS
  readiness signal. An emulator-only result does not qualify Titan hardware.

### BM6: User documentation and qualification policy

Execution: https://github.com/metaneutrons/aros-tools/issues/279
Dependencies: BM3, BM4, BM5

- BM6-A1: Astro documents tested build, inspect, verify and guarded write
  commands for each accepted medium, with supported host/board matrices and
  explicit experimental or unavailable states. Documented commands are run
  verbatim against qualified inputs.
- BM6-A2: Cheap schema, corruption and fixture tests run on PRs; expensive
  real build/boot qualification runs at the relevant integration or release
  gate. Durable evidence records exact source/tool/profile/image identities,
  supported boards and omissions. Neither an image build nor a green fixture
  alone is reported as a successful hardware boot.

The initiative completes only when BM1-BM6 are accepted on their stated
platform scope. A public boot-media release or a new image repository requires
a separate publication decision and its own immutable release qualification.

## Migration, risks and verification cost

The current commands remain operational while the new profile registry and
image CLI are introduced. Old bundle manifests remain readable until a
documented version transition. Rollback is selecting the previous tools
release; no historical image or tag is rewritten.

The principal risks are the legacy KOBJ bridge, bootloader/firmware licensing
and availability, the unpublished RISC-V toolchain, host-specific ISO tools,
and access to all physical boards. Missing prerequisites block only the
affected platform claim, not contract work. No AROS-NX source change occurs
without Fabian's separate approval. Focused fixtures and synthetic images
precede full builds; full host/board qualification is reserved for milestone
acceptance and release candidates. No duration or runner budget is asserted
before the native build gaps are measured.

## Decision changes

Record reviewed changes to scope or acceptance here with date, PR and the
evidence affected. Milestone issues reference an exact plan revision when
claiming acceptance.
