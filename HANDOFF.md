# Development handoff

Checkpoint verified on 2026-09-22. This is a compact entry point, not a second
backlog, live CI dashboard, or authorization to build, merge, or publish.
Recheck linked issues, runs, and release objects before acting. Git history
preserves earlier checkpoints.

## v0.3.9 first stable release

- [`v0.3.9`](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.9)
  is the first stable, public and immutable GitHub Release. REST release ID
  `393428283` contains the closed 40-file inventory. Its annotated tag object
  is `9f1e79b36207ef82a18be6e6a9ebaf0891680f92`, which peels to source commit
  `a940dc435cc9d3710764263905651bd88dfa6449`.
- The canonical native archive digests measured from that release are:
  `aarch64-apple-darwin` — `ea639de0d3320e2adfb679e9cc7c74152144505718f6df0c8b2ae8fcd9149c4c`
  (14,780,801 bytes); `aarch64-unknown-linux-gnu` —
  `d3e1e7ed73febd5f88d7cd9bc3e19c0b18cf871a2c255dfb8a051a98363ee001`
  (16,007,912 bytes); `x86_64-unknown-linux-gnu` —
  `eb700e423db98bbe762383fc7eaf5604c04657e0f14c25220f1943a4cbf406a8`
  (16,583,686 bytes). The release's signed
  [`SHA256SUMS`](https://github.com/metaneutrons/aros-tools/releases/download/v0.3.9/SHA256SUMS)
  remains the complete asset-level record.
- Initial qualification
  [35680796294](https://github.com/metaneutrons/aros-tools/actions/runs/35680796294)
  completed the three native producers, three independent A/B comparisons,
  signing, inventory and package preflights, then stopped safely at the
  reusable-workflow secret boundary after immutable GitHub publication. It did
  not mutate package channels.
- The narrowly scoped recovery fixes were merged as `2bf72d5`, `7823f30` and
  `d634fa7`. Final recovery
  [35698720659](https://github.com/metaneutrons/aros-tools/actions/runs/35698720659)
  passed immutable staging reconstruction, central APT publication and public
  verification, APT installation on `amd64` and `arm64`, Homebrew publication,
  AUR publication and AUR verification.
- A fresh isolated audit downloaded all 40 assets by numeric release ID,
  verified the closed signed inventory and `SHA256SUMS`, verified 20 Sigstore
  subjects and 40 GitHub provenance attestations against the release workflow,
  tag ref and source commit. It independently verified the central signed APT
  archive at `0.3.9-1`, the Homebrew formula on
  [`258bc977`](https://github.com/metaneutrons/homebrew-tap/commit/258bc9776d3e983e139df8e133308a685fe468c6),
  and the AUR `aros-tools-bin` package at `0.3.9-1` on
  [`9006a1a3`](https://aur.archlinux.org/cgit/aur.git/commit/?h=aros-tools-bin&id=9006a1a3fefefbfc39e8a687f7502b573d2c04ba).
  The public package metadata and source files were byte-identical to the
  immutable release assets.

## Last verified implementation checkpoint

- TCP-M0 through TCP-M6 remain accepted. Their durable implementation and
  evidence boundaries are recorded in the M3, M4, M5, and M6 evidence ledgers.
- The active `aros-toolchains` executor contract pins aros-tools
  [`2474bc3c89c21c80d23197c28ef63cd3c18603a4`](https://github.com/metaneutrons/aros-tools/commit/2474bc3c89c21c80d23197c28ef63cd3c18603a4), the integrated CACHE-M7 command migration. [aros-toolchains PR #53](https://github.com/metaneutrons/aros-toolchains/pull/53), merged as
  [`6266ab047058cc2b87e38d9f6018bb25d7b658a2`](https://github.com/metaneutrons/aros-toolchains/commit/6266ab047058cc2b87e38d9f6018bb25d7b658a2), passed both its PR and main producer-contract gates. The migration neither creates nor changes a release artifact.
- CACHE-M1 through CACHE-M7 and CLI-M1 through CLI-M6 have complete evidence in
  their linked issues and evidence ledgers. The cache evidence records the
  supported owned compiler namespace and excludes ambient, remote and
  external cache storage from cleanup authority.
- The immutable
  [`toolchain-v1-20260912-rc8`](https://github.com/metaneutrons/aros-toolchains/releases/tag/toolchain-v1-20260912-rc8)
  prerelease was built from producer commit
  `d018d11dd6f995fc1f37d0b2b431f94a6ec78fd5` and AROS-NX
  `9369cc8f8ba4f7d320945c78788c6e2a6d0d1eab`. Its tag object is
  `6b904632d5fda7fea8f9259de27b34e77c1556db`.
- Producer run
  [`34699919725`](https://github.com/metaneutrons/aros-toolchains/actions/runs/34699919725)
  passed 18 independent builds, nine byte-identical A/B comparisons, nine
  compatibility/relocation lanes, and draft creation for the active 3×3 matrix:
  Linux x86-64, Linux AArch64, and macOS AArch64 across `pc-x86_64`,
  `arm-raspi`, and `rpi-aarch64`.
- Independent draft and published audits each verified the exact 44-asset
  inventory, all 43 non-self checksums, manifests, SBOMs, qualification
  receipts, 42 signed pre-provenance subjects, and all final URLs. The release
  was published unchanged as a prerelease only after the draft audit passed.
- A fresh macOS AArch64 aros-cli store passed `toolchain list`, online install,
  verification, and offline re-install for all three released profiles.

## TCP-M8 management acceptance

- [PR #134](https://github.com/metaneutrons/aros-tools/pull/134) merged as
  [`c4a6199782056d10bb5f8c20cb06f1139f989bcc`](https://github.com/metaneutrons/aros-tools/commit/c4a6199782056d10bb5f8c20cb06f1139f989bcc).
  Its source tree (`da326ef2046aa3da6f2499ab881be11545a72fb6`) is identical to
  the qualified PR head `d9195bc4c103169a233da206f084f4f09b0939e1`.
- The [three-host Workspace CI run](https://github.com/metaneutrons/aros-tools/actions/runs/34722909249)
  passed Linux x86-64 source-coupled tests, Linux AArch64 portable tests,
  macOS AArch64 portable tests, formatting, architecture and Clippy. CodeQL
  and the documentation gate passed against the same tree.
- The accepted [M8 evidence ledger](docs/tcp-m8-lifecycle-evidence.md) records
  the lifecycle contract, black-box preview/confirmation tests, adversarial
  filesystem cases and explicit boundaries. macOS Intel is not an `aros-tools`
  native release target.

## Current bounded action

TCP-M7 and TCP-M8 are complete, and v0.3.9 completes the first stable release
closure. There is no pending release or package-channel recovery. RISC-V
remains a separate qualification; macOS Intel is not an `aros-tools` native
release target.

## Resume safely

1. Inspect the current tag object, release state and working trees before
   taking action. Historical `v0.3.5` through `v0.3.8` are terminal,
   unpublished non-releases; never retarget, delete, publish or replace them.
2. Use the immutable v0.3.9 release assets, `SHA256SUMS`, manifests and SBOMs
   as the sole source of consumer lock values. Never recover hashes, sizes or
   tree digests from retained output.
3. Follow [the test-stage policy](CONTRIBUTING.md#test-stages-and-integration-checkpoints).
   Do not repeat expensive compiler or A/B work for documentation-only changes.
4. Keep raw logs, machine-specific paths, private operational notes, and
   credentials outside Git. This handoff is developer documentation, not a
   public-site operations manual.
