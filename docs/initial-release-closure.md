# Initial release closure

Status: complete, 2026-09-22. This document records the measured closure of
the first public `aros-tools` release. It does not establish a toolchain or
hardware-boot claim.

## Fixed decisions

- Release Please owns SemVer and the changelog PR. An annotated immutable tag
  starts qualification; neither Release Please nor a maintainer may retarget a
  tag or release.
- APT publication belongs exclusively to `metaneutrons/apt-archive`. The tools
  repository supplies the verified `.deb` payloads and delegates publication
  through its restricted Archive Dispatch App.
- Homebrew uses its separate tap-only GitHub App; AUR uses its dedicated SSH
  identity. No package channel receives a fallback PAT.
- The maintained native release matrix is Linux x86-64, Linux ARM64 and macOS
  Apple silicon. macOS Intel is not a native archive, Homebrew or release
  target. The retired exception and its rationale are recorded in
  [macOS Intel release-target policy](macos-intel-release-policy.md).

## Evidence already established

- Cache and CLI milestones M1–M7/M1–M6 are closed. Their acceptance evidence
  is recorded at the exact tools commit
  `68873d74397e47f32879a3dfce8b98e4915f6d76`.
- `aros-toolchains` consumes the exact cache-compatible tools revision
  `2474bc3c89c21c80d23197c28ef63cd3c18603a4`; the consumer migration is
  verified at toolchains commit
  `6266ab047058cc2b87e38d9f6018bb25d7b658a2`.
- The Release Please, Archive Dispatch, Homebrew and AUR credentials exist in
  their isolated GitHub environments. Repository policy tests verify that the
  workflows do not mix those trust domains.
- The central APT archive contract, signed-key verification and dual Linux
  architecture installation tests are implemented.

## Closure evidence

- [`v0.3.9`](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.9)
  is public, stable and immutable. REST release ID `393428283` has exactly 40
  assets. Its tag object
  `9f1e79b36207ef82a18be6e6a9ebaf0891680f92` peels to the release source commit
  `a940dc435cc9d3710764263905651bd88dfa6449`.
- The initial qualification
  [35680796294](https://github.com/metaneutrons/aros-tools/actions/runs/35680796294)
  passed all three native producers, three independent A/B comparisons,
  signature, inventory and package-preflight gates. It published the sealed
  GitHub release, then stopped before any package-channel write at the
  reusable-workflow secret boundary.
- The source-free final recovery
  [35698720659](https://github.com/metaneutrons/aros-tools/actions/runs/35698720659)
  re-derived the immutable identity and passed every remaining channel gate:
  central APT publication and public verification, isolated APT installation
  on `amd64` and `arm64`, Homebrew publication/qualification, and AUR
  publication/verification.
- A fresh isolated download by numeric release ID verified the closed 40-file
  inventory and all checksums. It verified 20 Sigstore subjects and 40 GitHub
  provenance attestations against the release workflow, `refs/tags/v0.3.9`
  and the exact source commit. The exact signed APT channel exposes
  `aros-tools_0.3.9-1_{amd64,arm64}.deb`; the protected Homebrew main formula
  at [`258bc977`](https://github.com/metaneutrons/homebrew-tap/commit/258bc9776d3e983e139df8e133308a685fe468c6)
  and AUR `aros-tools-bin` at
  [`9006a1a3`](https://aur.archlinux.org/cgit/aur.git/commit/?h=aros-tools-bin&id=9006a1a3fefefbfc39e8a687f7502b573d2c04ba)
  are byte-identical to their v0.3.9 assets. AUR RPC reports `0.3.9-1`.
- The three canonical native archive hashes are recorded in
  [HANDOFF.md](../HANDOFF.md#v039-first-stable-release); the signed
  [`SHA256SUMS`](https://github.com/metaneutrons/aros-tools/releases/download/v0.3.9/SHA256SUMS)
  is the complete public asset record.

## Historical terminal states

`v0.3.5` through `v0.3.8` are immutable terminal non-releases. They remain
unpublished and must never be deleted, retargeted, replaced or promoted. The
release that closes this plan is only `v0.3.9`.

## Explicit boundaries

The first tools release does not prove a pristine-upstream complete product
build, physical Pi/Milk-V boot, RISC-V toolchain publication or macOS Intel
support. Those claims need their own source, artifact and hardware evidence.
