# Upstream AROS capability migration

Decision state: implementation authorized on 2026-09-27; detailed Mesa 26
design remains subject to M1 review. Epic and milestone issue links are pending.

## Outcome and boundaries

Qualify `metaneutrons/AROS-NX` against upstream AROS master
`282a0454356dc2b9554ab023997d7f7ca24fc03e` without weakening the
fail-closed `aros-transpiler` capability contract. The candidate integration is
[AROS-NX PR #34](https://github.com/metaneutrons/AROS-NX/pull/34), currently a
draft. Its [nine-lane product run](https://github.com/metaneutrons/AROS-NX/actions/runs/36301736137)
failed before building because the source recipes now exceed the audited
GRUB 2.12, Mesa 20.0.8 and WirelessManager capabilities.

The implementation belongs primarily to `aros-tools`: `aros-transpiler` owns
recipe acceptance and the embedded `aros-cmake-engine` owns execution. AROS-NX
owns the upstream merge and the exact aros-tools input selected for product
qualification. The already published aros-tools v0.3.12 and aros-toolchains
v0.1.4 remain immutable. No toolchain release, source lock switch, or weakening
of existing capability checks is implied by this plan.

## Design and decisions

- Preserve support for the qualified older source recipes. Add versioned,
  independently audited capabilities for new source layouts rather than
  replacing a trusted fingerprint with the latest observed hash.
- Keep parser acceptance and CMake execution together. GRUB 2.16 requires an
  exact source lock, patch, product inventory and real build evidence; accepting
  the version in the parser alone is not sufficient.
- Mesa 26 is a separate capability family, not a numeric substitution in the
  Mesa 20 implementation. It changes source inventories, generators, driver
  layouts and the Gallivm policy. Design its closed inputs before coding it.
- WirelessManager's new `extracflags` value must be checked exactly. Whether
  its dependency-generation flag changes the private CMake execution is a
  measured M2 decision, not a pass-through of arbitrary Make arguments.
- The AROS-NX PR may select a reviewed exact aros-tools commit for its matrix.
  Publishing that tools commit is not a prerequisite for merging the upstream
  sync. A later stable aros-tools release follows its own hardened gates.

Holding GRUB/Mesa at old versions in AROS-NX is a possible emergency
compatibility branch, but it is not the chosen migration: it would conceal the
new upstream recipes from the product path and require explicit local reverts.

## Delivery and acceptance

### M1: Freeze and review the new capability contract

Execution: issue link pending. Dependencies: AROS-NX PR #34 source identity.

- M1-A1: Record every current AT0004 class and the exact upstream recipe,
  source archive, patch, generated output and target-profile boundary.
- M1-A2: Define closed version-specific GRUB 2.16 and Mesa 26 execution
  contracts with positive and altered-input counterprobes. Reject a plan that
  merely refreshes fingerprints or skips unreachable recipes.

### M2: Support GRUB 2.16 and WirelessManager

Execution: issue link pending. Dependencies: M1 design for those lanes.

- M2-A1: The three PC GRUB host-tool declarations select a measured GRUB
  2.16 source/patch/build contract. Source hashes, product manifests and counts
  come from an actual isolated build, not the 2.12 constants.
- M2-A2: The WirelessManager declaration accepts exactly the reviewed new
  argument set; a changed value or extra option still fails closed.
- M2-A3: Focused parser, CMake engine, source-integrity and real macOS ARM64
  host-tool tests pass. Existing 2.12 and prior WirelessManager cases retain
  their supported or explicitly retired behavior.

### M3: Support Mesa 26 without regressing Mesa 20

Execution: issue link pending. Dependencies: M1 Mesa design.

- M3-A1: Versioned source inventories, generators, compiler options, driver
  selection and Gallivm/LLVM boundaries are closed and independently tested
  for the supported AROS target profiles.
- M3-A2: Both valid Mesa 20 and Mesa 26 recipes pass their own positive
  probes; mutated versions, paths, manifests and recipe blocks fail for the
  intended reason.
- M3-A3: Relevant real Mesa 26 products build and verify on the supported
  hosts; omissions are explicit and consistent with the AROS-NX product matrix.

### M4: Qualify and integrate AROS-NX upstream

Execution: issue link pending. Dependencies: M2 and M3.

- M4-A1: AROS-NX PR #34 selects an exact reviewed aros-tools commit and its
  shared-source preflight plus all nine Linux/macOS product lanes pass on the
  same PR head. The merged tree equals that qualified tree.
- M4-A2: Merge normally without rewriting upstream or local history. Confirm
  main contains the exact upstream commit, then fast-forward the permanent
  `master` mirror to that commit and verify both remote refs.
- M4-A3: Update source/producer contracts only after their new exact commits
  are known and qualified. Do not mutate the published toolchain v0.1.4.

### M5: Release the new aros-tools capability set

Execution: issue link pending. Dependencies: M4 and the repository's normal
Release Please flow.

- M5-A1: A fresh SemVer tag passes all native, signing, inventory,
  attestation, isolated-download and package-channel gates. Verify final public
  URLs before claiming a new stable release.
- M5-A2: Public CLI/Astro documentation states the measured source and host
  support. Failed candidates remain immutable non-releases.

## Migration, risks and verification cost

Use focused parser and engine tests during M2/M3 development. The full
three-host, three-profile AROS-NX matrix is the M4 integration gate, not a
per-commit feedback loop. GRUB and Mesa source upgrades may expose additional
compiler or generated-file failures after configure; a green transpiler alone
is insufficient. Until M4 succeeds, protected AROS-NX main, its mirror and
all public release assets stay unchanged. PR #34 remains draft.

No completion date or runner-cost estimate is asserted: the available run
failed before compilation, so it does not measure the new source build time.
