# Initial release closure

Status: in progress, 2026-09-14. This document records the remaining gates for
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
  architecture installation tests are implemented. No production package has
  been published.

## Remaining gates

1. **Repair the Release Please state.** PR #38 merged the `0.2.0` version
   record without a tag. It now blocks a fresh Release Please PR, while the
   later breaking CACHE-M7 change is absent from the `0.2.0` changelog section.
   Do not tag the current tree as `v0.2.0`. Create a reviewed, complete release
   record and a new canonical SemVer candidate through Release Please policy.
2. **Freeze the reviewed candidate.** Merge only the candidate release PR and
   select its exact protected-main commit. Do not merge unrelated work between
   that review and the tag.
3. **Run the three-host release qualification.** The first trusted stable
   baseline requires independent A/B archive reproduction, exact inventory,
   SBOMs, Sigstore bundles, GitHub attestations, isolated-download checks and
   all package-candidate checks for the maintained matrix.
4. **Publish only after the gates pass.** The workflow creates and verifies an
   exact private draft before exposing an immutable GitHub release. It then
   dispatches the central APT archive, qualifies the App-authored Homebrew
   formula and AUR package, and re-verifies public bytes without credentials.
5. **Record measured evidence.** Add the tag object, source commit, run IDs,
   artifact hashes, channel versions and isolated verification result to the
   handoff and public release status.

## Explicit boundaries

The first tools release does not prove a pristine-upstream complete product
build, physical Pi/Milk-V boot, RISC-V toolchain publication or macOS Intel
support. Those claims need their own source, artifact and hardware evidence.
