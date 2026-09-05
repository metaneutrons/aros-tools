# Initial release closure

Status: in progress, 2026-09-05. This closes the repository split and first
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

## Acceptance evidence

- [x] AROS-NX PR #27: twelve product checks and filename checks passed; merged
  normally as `909df75879f278eec08a88c3f0a4aa3e963d888f`.
- [ ] AROS-NX PR #25: twelve product checks and filename checks passed on
  `a2698c5e3e8e2bc8cf8934777bccbac081c9a42c`; matrix run `33960448985`.
  Still open, not merged (2026-09-05).
- [ ] AROS-NX PR #28: follow-up `e6ca85381f` updates the tools input, runs the
  companion-header test from the selected engine after building its tools, and
  uses their locked Rust toolchain. Local release build, fixture and actionlint
  pass; remote qualification follows PR #25 to avoid a redundant matrix.
- [ ] Verify the fail-closed upstream synchronizer after the mirrored upstream
  head is contained in main; qualify its next proposal normally. The workflow
  is already active (verified 2026-09-05); no reactivation is necessary.
- [ ] Remove the tools-owned APT publisher and qualify central archive
  consumption, trust, by-hash metadata, retained versions and negative cases.
  Shared-root implementation: 33 signed-archive tests and 18 dispatch/manifest
  tests pass, including other projects, missing consumer packages, mixed
  architectures and state/index disagreement. Full CI and merge remain gates.
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
- [ ] Verify publication credentials and environment isolation without exposing
  secret values. The central APT App credential and tag-only environment are
  configured, with a successful read-only consumer preflight. Acceptance still
  requires removal of old archive signing/storage credential copies from the
  unused tools environments; those copies have not yet been removed.
  AUR's public pinned host identity is now available at repository scope
  for credential-free preflight, as well as in its protected environment.
- [x] Homebrew App identity, full grant and real read-only consumer/protection
  preflight passed on 2026-09-05 at 17:18 UTC. The App key and client ID are
  configured in the existing tag-only `homebrew-publication` environment.
  Local regression gates cover scope, identity, token renewal and legacy PAT
  rejection. The check-registration race is bounded to five minutes and covered
  by 33 App/publication tests, with 11 additional native-host and installed-byte
  counter-probes (44 Homebrew tests in total). No tap protection rule or bypass
  changed.
- [ ] Qualify the App-authenticated formula PR, four-host tap CI, exact-head
  merge and final byte read-back in the first release. No release was triggered
  by credential setup. The old PAT secret is retained for explicit cleanup,
  but the new workflow does not reference it.
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
  A current Git-backed core tap is a documented alternative source mode, but
  adopting it would need aligned Intel qualification, tap CI and installation
  instructions; it has not been implemented or qualified. Dependency/post-install
  errors remain blocking. See the [debug evidence](https://github.com/metaneutrons/aros-tools/actions/runs/33987257343/job/101367689886).
- [x] Release Please App preflight and main-only environment configured on
  2026-09-05 at 17:36 UTC. Exact repository grant, configuration/PR/label
  reads, key fingerprint and secret/variable metadata verified. No App rights
  widened. Thirteen isolated workflow/scope counter-probes pass.
- [ ] Merge the authentication changes and observe the first actual
  App-authored release PR update with its normal protected checks. A local
  API preflight is not this end-to-end evidence.
- [ ] Merge the release candidate only after every protected check passes;
  create a new immutable annotated tag at that exact reviewed commit.
- [ ] Complete the four-host first-release A/B, isolated GitHub verification,
  central APT publication/install, Homebrew and AUR gates.
- [ ] Record exact release IDs, commits, runs and final verification results;
  reconcile the legacy HANDOFF and migration documents.

## Explicitly separate, unproven work

The shared-domain archive implementation is merged and live-R2 qualified; the
tools consumer migration still requires its protected merge. No first tools
release was published. Archive tests include signed two-project transactions,
conditional S3 uploads, byte-identical container rendering and actual
Debian 12/13 APT consumers with a wrong-key counter-probe. Legacy APT signing/storage
secrets remain in the unused `apt-signing`/`apt-publication` environments.
Remove those copies only as an explicit, verified cleanup of the old publisher.

Native producer integration remains separate from initial release acceptance:
[epic #27](https://github.com/metaneutrons/aros-tools/issues/27),
M0 contract PR #37 and M1–M7 are open. Do not wait for that feature to release
the existing tools suite.

Fresh ARM/AArch64/RISC-V legacy KOBJ triplets, the four-host RISC-V toolchain
release, and physical UART boot evidence on Pi 3B+, Pi 5 and Milk-V Titan are
not established by packaging or CI. Keep them visibly open; do not describe
the first tools release as proof of those hardware capabilities.
