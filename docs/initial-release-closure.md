# Initial release closure

Status: in progress, 2026-09-06. This closes the repository split and first
package release; it does not advance the native toolchain producer roadmap.

## Decisions

- AROS-NG stays intact. Existing tags and releases are never retargeted.
- The already published toolchain RC3 remains the qualified consumer input.
- Release Please owns the tools version. The untagged 0.1.0 candidate in PR #9
  was superseded before publication; PR #38 now proposes 0.1.1 from current
  reviewed history. Do not merge or tag it before the closure changes pass.
- Release Please authenticates with its existing dedicated App, through the
  separate `release-please` environment restricted to branch `main`.
  The issued token reaches only this repository with Contents/Pull requests
  write. The existing tag-only `release` environment remains unchanged.
  Normal App-authored PR events replace the old explicit check dispatches;
  `skip-github-release: true` and the reviewed version/tag gates remain.
- Fabian confirmed on 2026-09-05 that APT belongs exclusively to
  `metaneutrons/apt-archive`. The tools release provides attestations and `.deb`
  payloads, not an archive signing key or storage credentials. The central
  archive owns signing, retention, metadata refresh and publication.
- The intended public installation contract is the shared root
  `https://deb.metaneutrons.cc`, suite `rolling`, component `main`, with the
  central domain keyring. The consumer contract and public instructions now
  use that root and verify the signed shared-domain inventory. Publication is incomplete until both architectures
  install the exact released bytes.
- Fabian also approved a separate, account-private `metaneutrons-apt-dispatch`
  GitHub App instead of widening Release Please. It has only Actions write,
  Contents read and mandatory Metadata read; installation must be restricted to
  `apt-archive`. The tools credential environment accepts only `v*` tags.
- Homebrew uses its separate tap-only App, not the APT or Release-Please App.
  See the [authentication and qualification contract](homebrew-publication.md).
- Fabian approved [HB-2026-09-05](homebrew-intel-exception.md): pause only the
  Intel Homebrew PR-install lane until 2026-10-05, 00:00 UTC, with enforced
  expiry. Native Intel builds/tests remain required. This permits development
  PR acceptance with explicitly incomplete Homebrew coverage, never a release;
  tag/manual runs and shared tap CI retain all four hosts.

## Acceptance evidence

- [x] AROS-NX PR #27: twelve product checks and filename checks passed; merged
  normally as `909df75879f278eec08a88c3f0a4aa3e963d888f`.
- [x] AROS-NX PR #25: twelve product checks and filename checks passed on
  `a2698c5e3e8e2bc8cf8934777bccbac081c9a42c`; matrix run `33960448985`.
  Merged normally as `844630f1d91dfcaa21fc05100f8c586920ee385a` on
  2026-09-05 at 22:09:44 UTC, preserving the upstream history.
- [ ] AROS-NX PR #28: follow-up `e6ca85381f` corrected the tools input,
  companion-header fixture placement and locked Rust selection. After merging
  forward from new main, matrix `33995179765` tested `c4af20b16b`.
  The companion-header fixture passed on all six Linux lanes, but product
  configuration then failed: AHI still required a manifest from the removed
  source-side `cmake/` directory. The remaining matrix was cancelled after
  independent Linux hosts reproduced the defect. The
  [engine-resource ownership correction](cmake-engine-migration.md#product-manifests-belong-to-the-selected-engine)
  must qualify and merge in tools first; then update #28 to that exact tools
  commit and require a fresh complete twelve-lane product matrix. It remains
  open; a cancelled run is not acceptance evidence.
- [ ] Observe the next normal fail-closed upstream synchronization and qualify
  its proposal. PR #25 restored the previously failing invariant: main now
  contains mirrored upstream `966097d4c07fe5a1af3b20ebaa41d40dd8311c09`.
  The workflow remains active. At the 2026-09-05 check, upstream `82b41642`
  was another 105 commits ahead; those commits are separate, unqualified work.
- [x] Remove the tools-owned APT publisher and qualify central archive
  consumption, trust, by-hash metadata, retained versions and negative cases.
  Shared-root implementation: 33 signed-archive tests and 18 dispatch/manifest
  tests pass, including other projects, missing consumer packages, mixed
  architectures and state/index disagreement. All required and publication
  qualification workflows passed on PR #39 head `5e1c065498ac8eba373e7e2692048619e118678c`;
  normal protected squash merge `2f23c512ac3ae876701a12ad907402d5f6d26a63`
  on 2026-09-05 at 22:14:35 UTC has exactly the tested source tree.
  PR acceptance used the approved three-host Homebrew exception; this does
  not establish four-host release acceptance or production publication.
- [x] Central archive PR #15 merged as
  `0319276b951412c2a433e03841832fb718b58e11`. Its publisher and verifier passed
  a real isolated private-R2 test on 2026-09-05: bootstrap with an empty
  architecture index, second-project import, native dual-architecture upgrade,
  metadata-only refresh and byte-exact isolated downloads of every candidate.
  Stale baselines and immutable collisions were rejected without overwrites.
  All synthetic objects, the disposable bucket and its short-lived token were
  removed and cleanup verified. Production packages were not published by this
  qualification.
- [x] Source-repository ruleset restored to the existing six-check governance
  contract, including docs `build`, with all checks bound to GitHub Actions
  App `15368`. No bypass or other ruleset field changed. The protection
  verifier now handles rulesets and classic protection, with positive and
  negative HTTP, identity, scope and policy fixtures; the actual read-only
  release-policy credential passed the ruleset preflight.
- [x] Verify publication credentials and environment isolation without exposing
  secret values. The central APT App credential and tag-only environment are
  configured, with a successful read-only consumer preflight. After PR #39's
  merge, exactly five approved obsolete secret copies were removed on
  2026-09-05 at 22:16:46 UTC: the two `R2_*` copies from `apt-publication`,
  `APT_GPG_PRIVATE_KEY` and `APT_GPG_PASSPHRASE` from `apt-signing`, and
  `HOMEBREW_TAP_TOKEN` from `homebrew-publication`. Exact metadata preflight
  and read-back proved both old APT environments empty and the Homebrew App
  key unchanged. No shared credential, App grant, vault or keychain changed.
  AUR's public pinned host identity is now available at repository scope
  for credential-free preflight, as well as in its protected environment.
- [x] Homebrew App identity, full grant and real read-only consumer/protection
  preflight passed on 2026-09-05 at 17:18 UTC. The App key and client ID are
  configured in the existing tag-only `homebrew-publication` environment.
  Local regression gates cover scope, identity, token renewal and legacy PAT
  rejection. The check-registration race is bounded to five minutes and covered
  by App/publication tests, with additional native-host and installed-byte
  counter-probes. The dated PR exception has separate matrix/expiry and actual
  publication-condition counter-probes. No tap protection rule or bypass
  changed.
- [ ] Qualify the App-authenticated formula PR, four-host tap CI, exact-head
  merge and final byte read-back in the first release. No release was triggered
  by credential setup. The old tools-local PAT copy has been removed; the
  workflow uses only the restricted Homebrew App.
- [ ] Requalify genuine Intel Homebrew installation. Run `33985724773` reported
  all four Homebrew labels green, but its `macos-x86_64` job used the ARM
  `macos-14` runner. That result is not Intel evidence. The correction selects
  `macos-15-intel` and verifies real host identity plus installed native bytes
  against the selected staging manifest. Run `33987257343` on `3ed80c5` passed
  every other package host but failed at OpenSSL's Intel post-install; the
  installer returned nonzero and the new `AP7322` gate rejected it. The tools
  archive was installed, but its Intel byte verification did not run. The
  job-only debug replay (attempt 2, job `101367689886`) reproduced the failure
  and exposed its cause: Homebrew 6.0.22 rejects its own API-cache formula path
  in `FormulaInstaller#post_install_formula_path`, before OpenSSL's hook runs.
  Independent isolated ARM checks also reproduce the automatic API-source
  failure on the exact runner revision and upstream main; Intel exposes but
  does not cause the bug. No alternate core source mode or Homebrew production
  fix has been applied. Dependency/post-install errors remain blocking for
  releases. See the [debug evidence](https://github.com/metaneutrons/aros-tools/actions/runs/33987257343/job/101367689886)
  and the [dated PR exception/removal checklist](homebrew-intel-exception.md).
- [ ] Remove `HB-2026-09-05` after genuine four-host evidence. The PR-only
  exception expires at 2026-10-05, 00:00 UTC; it must not silently become the
  permanent matrix or be counted as first-release acceptance.
- [x] Release Please App preflight and main-only environment configured on
  2026-09-05 at 17:36 UTC. Exact repository grant, configuration/PR/label
  reads, key fingerprint and secret/variable metadata verified. No App rights
  widened. Thirteen isolated workflow/scope counter-probes pass.
- [x] Merge the authentication changes and observe the first actual
  App-authored release PR update with naturally triggered protected checks.
  Main-push run `33995343916` succeeded on `2f23c512`, including exact token
  scope/identity, release-PR verification and post-job revocation. The
  `metaneutrons-release-please[bot]` App updated PR #38 to
  `48eab742b98f76e42c2718ca63a5001307cb37a3` and triggered release, workspace,
  docs and CodeQL runs `33995370040`, `33995369750`, `33995369748` and
  `33995369734` through normal `pull_request` events. Both actor fields match
  the App; no manual dispatch substituted for those events. PR #38 remains
  open. This proves App integration, not release qualification or permission
  to merge/tag the candidate.
- [ ] Merge the release candidate only after every protected check passes;
  create a new immutable annotated tag at that exact reviewed commit.
- [ ] Complete the four-host first-release A/B, isolated GitHub verification,
  central APT publication/install, Homebrew and AUR gates.
- [ ] Record exact release IDs, commits, runs and final verification results;
  reconcile the legacy HANDOFF and migration documents.

## Explicitly separate, unproven work

The shared-domain archive implementation is merged and live-R2 qualified; the
tools consumer migration is also merged through PR #39. No first tools
release was published. Archive tests include signed two-project transactions,
conditional S3 uploads, byte-identical container rendering and actual
Debian 12/13 APT consumers with a wrong-key counter-probe. Obsolete tools-local
APT signing/storage and Homebrew PAT copies have been removed with exact
before/after metadata checks. Actual first-release publication remains open.

Native producer integration remains separate from initial release acceptance:
[epic #27](https://github.com/metaneutrons/aros-tools/issues/27),
M0 contract PR #37 and M1–M7 are open. Do not wait for that feature to release
the existing tools suite.

Fresh ARM/AArch64/RISC-V legacy KOBJ triplets, the four-host RISC-V toolchain
release, and physical UART boot evidence on Pi 3B+, Pi 5 and Milk-V Titan are
not established by packaging or CI. Keep them visibly open; do not describe
the first tools release as proof of those hardware capabilities.
