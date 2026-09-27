# Upstream AROS capability migration

Status: M1-M5 completed on 2026-09-27.
Tracking: [epic #241](https://github.com/metaneutrons/aros-tools/issues/241).

## Outcome and boundaries

Qualify `metaneutrons/AROS-NX` against upstream AROS master
`282a0454356dc2b9554ab023997d7f7ca24fc03e` without weakening the
fail-closed `aros-transpiler` capability contract. The initial
[nine-lane product run](https://github.com/metaneutrons/AROS-NX/actions/runs/36301736137)
failed before building because the source recipes exceeded the audited
GRUB 2.12, Mesa 20.0.8 and WirelessManager capabilities. The corrected
[AROS-NX PR #34](https://github.com/metaneutrons/AROS-NX/pull/34) passed its
locked-source preflight and all nine product lanes on exact head
`790f70607ef0687bc6becd6bec4c2e0b4beede47` in
[run 36336819657](https://github.com/metaneutrons/AROS-NX/actions/runs/36336819657).
It merged normally at `15230bef9d23e12cd9d4cca62253c4b62276e8ff`;
the merge tree matches the qualified PR tree. The same tree passed the
[protected-main product run](https://github.com/metaneutrons/AROS-NX/actions/runs/36338345571).

The implementation belongs primarily to `aros-tools`: `aros-transpiler` owns
recipe acceptance and the embedded `aros-cmake-engine` owns execution. AROS-NX
owns the upstream merge and the exact aros-tools input selected for product
qualification. The previously published aros-tools v0.3.12 and aros-toolchains
v0.1.4 remain immutable. This work did not require a new toolchain release, an
AROS-NX consumer toolchain lock switch, or weaker capability checks.

## Design and decisions

- Preserve support for the qualified older source recipes. Add versioned,
  independently audited capabilities for new source layouts rather than
  replacing a trusted fingerprint with the latest observed hash.
- Keep parser acceptance and CMake execution together. GRUB 2.16 requires an
  exact source lock, patch, product inventory and real build evidence; accepting
  the version in the parser alone is not sufficient.
- Mesa 26 is a separate capability family, not a numeric substitution in the
  Mesa 20 implementation. Its source inventories, generators, driver layouts
  and Gallivm policy have independently checked, versioned inputs.
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

Execution: [issue #244](https://github.com/metaneutrons/aros-tools/issues/244). Dependencies: AROS-NX PR #34 source identity.

- M1-A1: Record every current AT0004 class and the exact upstream recipe,
  source archive, patch, generated output and target-profile boundary.
- M1-A2: Define closed version-specific GRUB 2.16 and Mesa 26 execution
  contracts with positive and altered-input counterprobes. Reject a plan that
  merely refreshes fingerprints or skips unreachable recipes.

The initial source audit identified these exact boundaries; M1 closed after
the independent capability and product tests passed:

- GRUB 2.16 uses the source digest
  `f0db0104927df0b9a48bc41b735c702936190d17eb2d73d3018fa77474a0cabe`.
  The three macOS ARM64 PC/EFI host builds and ISO staging passed against the
  AROS-NX candidate with [aros-tools PR #249](https://github.com/metaneutrons/aros-tools/pull/249).
  The version lock, in-tree patch and measured per-lane install manifests are
  separate from the still-supported GRUB 2.12 contract.
- Mesa 26.0.0 uses the official 43,776,320-byte `tar.xz`, SHA-256
  `2a44e98e64d5c36cec64633de2d0ec7eff64703ee25b35364ba8fcaa84f33f72`.
  [AROS-NX PR #35](https://github.com/metaneutrons/AROS-NX/pull/35)
  pins that archive and its patch and selects the new `src/mesa/glapi` layout
  before fetching. The patch and closed source inventories were qualified
  against the official archive and merged into the source used by PR #34.
- Mesa 26 `glapi` compiles `shared-glapi/core` and a generated
  `public_glapi_wrappers.c`. Its two generated outputs depend on `mapi_abi.py`,
  `gl_and_es_API.xml`, `libgl-symbols.txt`, the public-symbol manifest and the
  wrapper shell adapter. The version-bound runner tracks those inputs and
  products; the Mesa 20 direct-Python recipe remains separate. Cold-tree
  source evaluation uses an exact source inventory rather than dropping the
  unresolved `top_srcdir` fragment. The remaining Mesa 26 core archives were
  closed and tested in [aros-tools PR #259](https://github.com/metaneutrons/aros-tools/pull/259).
- An explicit `OPT_MESAGL=26.0.0` initially caused the central Mesa fetch to
  be skipped. The target-aware fetch correction must accept a concrete
  selector only when Make has not assigned the variable, while still rejecting
  unknown conditional assignments. The correction passed profile and
  changed-branch counterprobes before product qualification.

### M2: Support GRUB 2.16 and WirelessManager

Execution: [issue #245](https://github.com/metaneutrons/aros-tools/issues/245). Dependencies: M1 design for those lanes.

- M2-A1: The three PC GRUB host-tool declarations select a measured GRUB
  2.16 source/patch/build contract. Source hashes, product manifests and counts
  come from an actual isolated build, not the 2.12 constants.
- M2-A2: The WirelessManager declaration accepts exactly the reviewed new
  argument set; a changed value or extra option still fails closed.
- M2-A3: Focused parser, CMake engine, source-integrity and real macOS ARM64
  host-tool tests pass. Existing 2.12 and prior WirelessManager cases retain
  their supported or explicitly retired behavior.

### M3: Support Mesa 26 without regressing Mesa 20

Execution: [issue #242](https://github.com/metaneutrons/aros-tools/issues/242). Dependencies: M1 Mesa design.

- M3-A1: Versioned source inventories, generators, compiler options, driver
  selection and Gallivm/LLVM boundaries are closed and independently tested
  for the supported AROS target profiles.
- M3-A2: Both valid Mesa 20 and Mesa 26 recipes pass their own positive
  probes; mutated versions, paths, manifests and recipe blocks fail for the
  intended reason.
- M3-A3: Relevant real Mesa 26 products build and verify on the supported
  hosts; omissions are explicit and consistent with the AROS-NX product matrix.

### M4: Qualify and integrate AROS-NX upstream

Execution: [issue #243](https://github.com/metaneutrons/aros-tools/issues/243). Dependencies: M2 and M3.

- M4-A1: AROS-NX PR #34 selects an exact reviewed aros-tools commit and its
  shared-source preflight plus all nine Linux/macOS product lanes pass on the
  same PR head. The merged tree equals that qualified tree.
- M4-A2: Merge normally without rewriting upstream or local history. Confirm
  main contains the exact upstream commit, then fast-forward the permanent
  `master` mirror to that commit and verify both remote refs.
- M4-A3: The aros-toolchains producer source pin merged at
  `f35ecea2918c8c8314bf6878b4401af6ae45132d` after its contracts passed.
  The aros-tools source/producer contract merged at
  `c204695815e176b016b82cb0bafbfe436b76d84b` after complete three-host
  CI. The published toolchain v0.1.4 was not changed.

### M5: Release the new aros-tools capability set

Execution: [issue #246](https://github.com/metaneutrons/aros-tools/issues/246). Dependencies: M4 and the repository's normal
Release Please flow.

- M5-A1: A fresh SemVer tag passes all native, signing, inventory,
  attestation, isolated-download and package-channel gates. Verify final public
  URLs before claiming a new stable release.
- M5-A2: Public CLI/Astro documentation states the measured source and host
  support. Failed candidates remain immutable non-releases.

M5 completed with the immutable public
[aros-tools v0.3.13 release](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.13)
(release ID `397777268`, source commit
`b5a2e9ea54bdaf50712b16f5c64dc89fd8c87e62`). The
[tag qualification](https://github.com/metaneutrons/aros-tools/actions/runs/36342801020)
passed all three native host builds and byte comparisons, signing, the exact
40-asset inventory, isolated download, 20 Sigstore bundles and 40 GitHub
attestations. Public package gates passed for signed APT `0.3.13-1` on amd64
and arm64, Homebrew on macOS ARM64 and both Linux hosts, and AUR `0.3.13-1`.
Independent checks confirmed all 40 public asset URLs, archive checksums,
signatures, attestations and exact public package bytes. The final read-only
channel audit passed. The workflow conclusion is nevertheless red because its
subsequent Release Please label finalizer accepted only GitHub's GraphQL actor
name, not the REST bot login. The narrow correction merged as
[PR #262](https://github.com/metaneutrons/aros-tools/pull/262) at
`62e2ec408f7a1b80b55f82361699ad48cfe0693d`; the corrected finalizer
resolved the exact merged PR and cleared only its pending label. No tag,
release or package was rerun or changed. The public
[release-status page](https://aros.metaneutrons.cc/aros-tools/reference/release-status/)
was rebuilt from GitHub release data and now lists v0.3.13 alongside the
unchanged toolchains v0.1.4.

## Migration, risks and verification cost

Use focused parser and engine tests during M2/M3 development. The full
three-host, three-profile AROS-NX matrix is the M4 integration gate, not a
per-commit feedback loop. GRUB and Mesa source upgrades may expose additional
compiler or generated-file failures after configure; a green transpiler alone
is insufficient. M4 is complete: protected AROS-NX main contains the qualified
source tree, the permanent `master` mirror fast-forwarded to exact upstream
commit `282a0454356dc2b9554ab023997d7f7ca24fc03e`, and both
source/producer contracts select their reviewed commits. M5 independently
qualified and published aros-tools v0.3.13 through its native and package
release gates. AROS-NX continues to consume the separately qualified
aros-toolchains v0.1.4; the M4 product matrix was not substituted for native
release or package-channel qualification.
