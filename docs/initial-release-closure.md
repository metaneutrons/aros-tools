# Initial release closure

Status: in progress, 2026-09-05. This closes the repository split and first
package release; it does not advance the native toolchain producer roadmap.

## Decisions

- AROS-NG stays intact. Existing tags and releases are never retargeted.
- The already published toolchain RC3 remains the qualified consumer input.
- Release Please owns the tools version. The untagged 0.1.0 candidate in PR #9
  was superseded before publication; PR #38 now proposes 0.1.1 from current
  reviewed history. Do not merge or tag it before the closure changes pass.
- Fabian confirmed on 2026-09-05 that APT belongs exclusively to
  `metaneutrons/apt-archive`. The tools release provides attestations and `.deb`
  payloads, not an archive signing key or storage credentials. The central
  archive owns signing, retention, metadata refresh and publication.
- The public installation contract is `https://deb.metaneutrons.cc/aros-tools`,
  suite `rolling`, component `main`, with the central domain keyring. Publication
  is incomplete until both architectures install the exact released bytes.
- Fabian also approved a separate, account-private `metaneutrons-apt-dispatch`
  GitHub App instead of widening Release Please. It has only Actions write,
  Contents read and mandatory Metadata read; installation must be restricted to
  `apt-archive`. The tools credential environment accepts only `v*` tags.

## Acceptance evidence

- [x] AROS-NX PR #27: twelve product checks and filename checks passed; merged
  normally as `909df75879f278eec08a88c3f0a4aa3e963d888f`.
- [ ] AROS-NX PR #25: refreshed onto current main as
  `a2698c5e3e8e2bc8cf8934777bccbac081c9a42c`; matrix run `33960448985`.
- [ ] AROS-NX PR #28: follow-up `e6ca85381f` updates the tools input, runs the
  companion-header test from the selected engine after building its tools, and
  uses their locked Rust toolchain. Local release build, fixture and actionlint
  pass; remote qualification follows PR #25 to avoid a redundant matrix.
- [ ] Resume the fail-closed upstream synchronizer after the mirrored upstream
  head is contained in main; qualify its next proposal normally.
- [ ] Remove the tools-owned APT publisher and qualify central archive
  consumption, trust, by-hash metadata, retained versions and negative cases.
  Implementation is locally complete: 27 signed-archive tests, 18 dispatch and
  manifest tests, the entire workspace quality gate and the documentation gate
  pass. GitHub qualification and merge are still required.
- [ ] Verify publication credentials and environment isolation without exposing
  secret values. No archive signing or storage secret remains in aros-tools.
  The new App and tag-only environment exist. Its first key download was blocked
  by Chrome; a manual replacement download, secure credential handoff and removal
  of the unused first key remain. Old publisher credential copies are not yet
  removed. AUR's public pinned host identity is now available at repository scope
  for credential-free preflight, as well as in its protected environment.
- [ ] Merge the release candidate only after every protected check passes;
  create a new immutable annotated tag at that exact reviewed commit.
- [ ] Complete the four-host first-release A/B, isolated GitHub verification,
  central APT publication/install, Homebrew and AUR gates.
- [ ] Record exact release IDs, commits, runs and final verification results;
  reconcile the legacy HANDOFF and migration documents.

## Explicitly separate, unproven work

Fresh ARM/AArch64/RISC-V legacy KOBJ triplets, the four-host RISC-V toolchain
release, and physical UART boot evidence on Pi 3B+, Pi 5 and Milk-V Titan are
not established by packaging or CI. Keep them visibly open; do not describe
the first tools release as proof of those hardware capabilities.
