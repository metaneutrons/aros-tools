# Development handoff

Checkpoint verified on 2026-09-14. This is a compact transition record, not a
second backlog, live CI dashboard, or authorization to build, merge, tag, or
publish. Recheck linked issues, runs, release objects and working trees before
acting. Git history preserves earlier checkpoints.

## Current repository state

- The current remote `main` is
  [`16085b5a67d63c9a90cee923707d366ebff71b39`](https://github.com/metaneutrons/aros-tools/commit/16085b5a67d63c9a90cee923707d366ebff71b39).
  It includes the policy merge and subsequent Dependabot updates.
- [PR #182](https://github.com/metaneutrons/aros-tools/pull/182) merged at
  `2026-09-14T13:58:18Z` as
  [`478e2b7de7ac76eb8077e91da77ca1daf0380e8f`](https://github.com/metaneutrons/aros-tools/commit/478e2b7de7ac76eb8077e91da77ca1daf0380e8f).
  Its exact reviewed head was
  [`2f11de63dc47eed0013bca6e38538d4ffb3a8b23`](https://github.com/metaneutrons/aros-tools/commit/2f11de63dc47eed0013bca6e38538d4ffb3a8b23).
- PR #182 passed Workspace CI, Documentation, CodeQL, action analysis,
  commit hygiene, formatting/architecture/Clippy, and all three test lanes in
  run [`34818017426`](https://github.com/metaneutrons/aros-tools/actions/runs/34818017426).
  Documentation run [`34818017449`](https://github.com/metaneutrons/aros-tools/actions/runs/34818017449)
  and CodeQL run [`34818017423`](https://github.com/metaneutrons/aros-tools/actions/runs/34818017423)
  also passed. The documentation deploy was intentionally skipped.
- No public `aros-tools` tag or release has been verified yet. The merged
  Release Please `0.2.0` record must not be promoted by guessing or by tagging
  the current tree; the later cache, policy and dependency changes require a
  fresh reviewed release record.

## Permanent native distribution policy

The maintained native release matrix is exactly:

- Linux x86-64 (`x86_64-unknown-linux-gnu`);
- Linux ARM64 (`aarch64-unknown-linux-gnu`); and
- macOS Apple silicon (`aarch64-apple-darwin`).

macOS Intel is not a release, archive, package-manager, Homebrew, signing,
inventory or A/B target. The public signed GitHub asset contract is therefore
the exact 40-file three-host inventory. The Homebrew formula selects Apple
silicon on macOS and the two Linux architectures. A technically usable Intel
host may attempt a source build at its own risk, but that is outside the
distribution contract.

The active v1 producer matrix in `aros-toolchain` is the same three-host set.
`macos-x86_64` remains in the historical parser/schema set solely so existing
four-host indexes and releases remain readable; it cannot create a new release
claim. The former re-enable request
[`aros-toolchains#27`](https://github.com/metaneutrons/aros-toolchains/issues/27)
is closed as superseded by this permanent policy. The policy evidence is in
[`docs/macos-intel-release-policy.md`](docs/macos-intel-release-policy.md).

## Accepted implementation checkpoints

- TCP-M0 through TCP-M6 remain accepted. Their durable implementation and
  evidence boundaries are recorded in the M3, M4, M5 and M6 ledgers.
- CACHE-M1 through CACHE-M7 and CLI-M1 through CLI-M6 are complete. The cache
  evidence records the supported owned compiler namespace and excludes ambient,
  remote and external cache storage from cleanup authority.
- The active `aros-toolchains` executor contract consumes aros-tools
  [`2474bc3c89c21c80d23197c28ef63cd3c18603a4`](https://github.com/metaneutrons/aros-tools/commit/2474bc3c89c21c80d23197c28ef63cd3c18603a4),
  the integrated CACHE-M7 command migration. Consumer migration was accepted
  at aros-toolchains commit
  [`6266ab047058cc2b87e38d9f6018bb25d7b658a2`](https://github.com/metaneutrons/aros-toolchains/commit/6266ab047058cc2b87e38d9f6018bb25d7b658a2).
- TCP-M8 management acceptance is complete. The lifecycle contract,
  no-follow filesystem cases, preview/confirmation boundary and safe cleanup
  evidence are recorded in
  [`docs/tcp-m8-lifecycle-evidence.md`](docs/tcp-m8-lifecycle-evidence.md).
- The immutable
  [`toolchain-v1-20260912-rc8`](https://github.com/metaneutrons/aros-toolchains/releases/tag/toolchain-v1-20260912-rc8)
  prerelease remains the verified three-host, three-profile toolchain
  baseline. It was built from producer commit
  `d018d11dd6f995fc1f37d0b2b431f94a6ec78fd5` and AROS-NX
  `9369cc8f8ba4f7d320945c78788c6e2a6d0d1eab`; its tag object is
  `6b904632d5fda7fea8f9259de27b34e77c1556db`.

## Open release gates

1. **Stabilize the test boundary.**
   [Issue #183](https://github.com/metaneutrons/aros-tools/issues/183)
   records a reproducible macOS failure pattern under default Rust test
   parallelism: different process-/deadline-sensitive tests fail across
   runs, while the complete `aros-toolchain` library suite passes 142/142 with
   `--test-threads=1`. Identify the race or contain it narrowly in the
   canonical gate; do not silently turn a functional failure into a skip.
2. **Repair Release Please state.** The merged `0.2.0` record has no public
   tag and predates later breaking/cache and distribution-policy changes. Do
   not tag `v0.2.0` on the current tree. Create a new complete Release Please
   candidate, review its changelog and exact SemVer, and merge only that
   candidate.
3. **Freeze and qualify the first tools release.** Select the exact protected
   `main` commit, create one immutable annotated tag, then run the complete
   three-host native qualification: independent package reproduction, exact
   40-file inventory, manifests, SBOMs, checksums, Sigstore/provenance,
   isolated downloads, Homebrew and APT/AUR candidate checks. Do not restore
   Intel lanes by exception.
4. **Promote only measured bytes.** Publish the immutable GitHub release only
   after every gate passes. Dispatch signed Debian payloads exclusively to
   `metaneutrons/apt-archive`, publish the measured Homebrew and AUR metadata
   through their dedicated credentials, and reverify all public URLs without
   credentials. Transfer only those measured values into consumer locks and
   update the final release evidence.

## Safe resume procedure

1. Recheck the current `main` SHA, Issue #183, Release Please state, tags,
   releases, and all working trees. Never infer state from a stale handoff.
2. Keep raw logs, machine-specific paths, private operational notes and
   credentials outside Git. The local credential/app configuration is not
   public documentation.
3. Follow [the test-stage policy](CONTRIBUTING.md#test-stages-and-integration-checkpoints).
   Documentation-only changes do not justify compiler or A/B work; release
   qualification must use the frozen tag and the exact closed matrix.
4. Leave AROS-NG and historical toolchain artifacts intact. Never delete,
   retarget or replace existing tags or releases.
