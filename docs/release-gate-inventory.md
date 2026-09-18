# Release gate inventory

An operability review of the release pipeline. It asks one question of every
gate: what does a failure here cost, and could we have found it without
spending a version?

This is not a security review. It takes the controls as given and examines only
their reachability and failure economics.

## Method and limits

Read in full: `RELEASING.md`, the `metadata` job of `.github/workflows/release.yml`,
`scripts/release/verify-release-ref.sh`, `scripts/release/verify-release-window.sh`.
Read structurally (job graph, conditions, environments, permissions, secret
references, step names, verifier call sites): the remainder of
`.github/workflows/release.yml` (2034 lines) and
`.github/workflows/publish-ecosystem.yml` (675 lines).

Not read line by line: most of `scripts/release/` (6802 lines, of which about
2700 are its own tests). The "proves" column therefore states each gate's
declared purpose from its name, arguments and surrounding workflow context, not
a line-by-line audit of the checking logic.

Reachability claims are not read off the `if:` expressions alone. They are
confirmed against two real runs: the failed tag run
[35258666646](https://github.com/metaneutrons/aros-tools/actions/runs/35258666646)
and the green no-publish rehearsal
[35278945421](https://github.com/metaneutrons/aros-tools/actions/runs/35278945421),
both on the current job graph.

## The cost model

Two failure classes behave very differently, and `RELEASING.md` § 2 already
separates them.

**Version-class.** The fix requires a commit. A tag pins the workflow file and
every script it calls, so a re-run of the same tag re-reads the defect. The tag
is immutable and must not be retargeted, so the candidate is spent and the fix
takes a new SemVer. `v0.3.0` died this way.

**Rerun-class.** The fix is outside the repository: a missing secret, an
environment binding, a ruleset, an upstream service. The payload is still
valid, so the exact unchanged tag can be re-run once the cause is repaired. No
version is consumed.

One cross-cutting constraint converts the second class into the first.
`scripts/release/verify-release-window.sh` enforces a 604800-second window from
the annotated tag's tagger timestamp, checked inside the `publish` job. A
rerun-class failure that is not repaired within seven days of tagging can no
longer publish that tag, and the candidate becomes version-class after all.

## Reachability, in summary

Of 25 job definitions across both workflows, 5 execute outside a release tag,
and 3 of those only in a reduced form that skips their signature-dependent
steps. 20 execute only on a pushed `v*` tag.

All five credential paths are tag-only, and none has ever executed:

| Secret | Readers | Ever executed |
| --- | --- | --- |
| `RELEASE_ADMIN_READ_TOKEN` | `governance`, `release-config-preflight` | no |
| OIDC `id-token` (keyless signing) | `sign-native`, `sign-aggregate` | no |
| `HOMEBREW_APP_PRIVATE_KEY` | `homebrew-credential-preflight`, ecosystem `homebrew` | no |
| `ARCHIVE_DISPATCH_PRIVATE_KEY` | `archive-credential-preflight`, ecosystem `apt` | no |
| `AUR_SSH_PRIVATE_KEY` | `aur-credential-preflight`, ecosystem `aur-publish` | no |

## Gates in `release.yml`

| Gate | Proves | Identity | Reachable without a release tag | A failure costs |
| --- | --- | --- | --- | --- |
| `metadata` — Plan Homebrew coverage | Which Homebrew hosts this event must cover; a PR-only exception is never a release waiver | none | yes | version |
| `metadata` — Validate source and version | Canonical SemVer tag name, annotated (not lightweight) tag, valid tagger timestamp, tag peels to `GITHUB_SHA`, tag version equals workspace version equals manifest version, CHANGELOG section exists, stable/prerelease classification, A/B selection | `github.token` | **no** — the entire branch is gated on `GITHUB_REF_TYPE == tag`; on dispatch the job runs but proves nothing about a tag | version |
| `governance` | Protection contract of `main` and reachability of the candidate from protected `main` | `RELEASE_ADMIN_READ_TOKEN`, env `release` | **no** | rerun (secret or protection contract), version (verifier logic) |
| `release-recovery` | Resolves an existing private draft or immutable release by numeric ID and downloads every asset before any signing job runs | workflow token, `contents: write` | **no** | rerun |
| `native` (×3 hosts) | Producer tests, full native build, dynamic-linkage audit, normalized archive read-back, clean-room smoke test, Debian packaging on Linux, SPDX SBOM bound to the measured artifact | none | yes | version |
| `native-ab` (×3 hosts) | Byte-identical recompilation and repackaging on an independent runner | none | **no** — `requires_ab` is computed only for a stable tag, so it is always false on dispatch | version |
| `sign-native` (×3 hosts) | Re-proves tag identity, then reuses verified historical bundles or signs previously unseen subjects, and attests every native subject | OIDC `id-token: write`, `attestations: write` | **no** | rerun (OIDC/Sigstore), version (signing logic) |
| `aggregate` | Full 44-asset inventory and payload verification, Homebrew and AUR metadata, checksum inventory, closed staging inventory | `attestations: read` | partial — runs on dispatch, but skips signature verification and the signed release body | version |
| `sign-aggregate` | Signs and attests package-manager metadata and the checksum inventory; verifies the closed signed staging set | OIDC `id-token: write`, `attestations: write` | **no** | rerun (OIDC/Sigstore), version (signing logic) |
| `homebrew` (×3 hosts) | The measured formula installs and tests on each maintained release host | none | yes (unsigned inputs) | version |
| `aur` (×2 arch) | The measured `PKGBUILD` builds and verifies | none | yes (unsigned inputs) | version |
| `release-config-preflight` | The Administration-read immutable-release policy (`enabled=true`) and release configuration | `RELEASE_ADMIN_READ_TOKEN`, env `release` | **no** | rerun (repository policy) |
| `channel-preflight` | All four public channel states before any exposure; refuses a newer public version or divergent bytes | credential-free | **no** | rerun (public channel state), version (comparison logic) |
| `homebrew-credential-preflight` | Isolated Homebrew App token issues and reaches only the dedicated tap | `HOMEBREW_APP_PRIVATE_KEY`, env `homebrew-publication` | **no** | rerun |
| `archive-credential-preflight` | Protected archive input validates against `contracts/apt-archive-v1.toml` without publishing | `ARCHIVE_DISPATCH_PRIVATE_KEY`, env `apt-archive-publication` | **no** | rerun |
| `aur-credential-preflight` | Pinned host key and dedicated AUR identity | `AUR_SSH_PRIVATE_KEY`, env `aur-publication` | **no** | rerun |
| `publication-preflight` | Every isolated fail-closed gate reported success | none (aggregator) | **no** | follows its inputs |
| `publish` | Creates or resumes the private exact-byte draft, downloads it in isolation, verifies signatures and attestations, publishes exactly once with the final status, and enforces the seven-day tag window | workflow token, `contents: write`, `attestations: read` | **no** | rerun inside the window, version once it closes |
| `ecosystem` | Delegates to `publish-ecosystem.yml` | workflow_call | **no** | see below |
| `final-audit` | Re-verifies every immutable final asset after channel convergence, without mutation | `attestations: read` | **no** | rerun |

## Gates in `publish-ecosystem.yml`

Every job here is stable-tag-only and runs after the immutable GitHub release
exists. A failure never costs the version, because `RELEASING.md` § 5 allows
package-channel recovery to continue later. It does leave channels divergent
until it converges.

| Gate | Proves | Identity | A failure costs |
| --- | --- | --- | --- |
| `apt` | Re-proves tag identity, dispatches the central archive and follows only the numeric run ID GitHub returned; a missing or ambiguous response fails closed | `ARCHIVE_DISPATCH_PRIVATE_KEY`, env `apt-archive-publication` | channel divergence |
| `apt-verify` | Signing subkey, key status, both Release signatures, publication and expiry times, the four-index by-hash matrix, matching compressed and uncompressed indexes, both exact packages | credential-free | channel divergence |
| `apt-install` (×2 arch) | Installation through an isolated signed APT source; compares all eight installed binaries with the candidate | credential-free | channel divergence |
| `homebrew` | Opens the exact formula update, waits for tap qualification on all three release hosts, renews the token, revalidates and merges only the recorded head SHA, then verifies the formula on protected main | `HOMEBREW_APP_PRIVATE_KEY`, env `homebrew-publication` | channel divergence |
| `aur-publish` | Publishes the measured `PKGBUILD` and generated `.SRCINFO` and seals public verification evidence | `AUR_SSH_PRIVATE_KEY`, env `aur-publication` | channel divergence |
| `aur-verify` | Closed handoff, AUR Git state and public package metadata | credential-free | channel divergence |

## Findings

**F1. The verification that matters least is the only one that is testable.**
Build, packaging, inventory, Homebrew and AUR qualification all run on a
dispatch. Identity, governance, signing, credential isolation, publication and
channel convergence do not. The no-publish rehearsal is green across 24 jobs
while remaining structurally incapable of reproducing the failure mode that has
actually occurred.

**F2. `metadata` is reachable but hollow.** It is the one job with no `if:`
guard, which reads as coverage. Everything it verifies about a release lives
inside `if [[ "${GITHUB_REF_TYPE:-}" == tag ]]`. A green `metadata` on a
dispatch says nothing about tag name, annotation, peel target, version
agreement across three files, or changelog presence.

**F3. Five credential paths, zero executions.** Every secret in the pipeline is
read only by tag-gated jobs. Nothing establishes that any of them is present,
correctly scoped or unexpired until a tag is pushed. These are rerun-class
failures, so they do not burn a version, but each one costs a full cycle and
eats into the seven-day window.

**F4. The seven-day window turns patience into a version cost.** A
rerun-class failure is free only while the window is open. A credential that
takes more than a week to provision converts into a new SemVer.

**F5. The A/B path has never run.** `native-ab` is selected only for a stable
tag, so byte-identical independent reproduction, the most expensive guarantee
in the pipeline, is also the one with no execution history.

**F6. `verify-release-ref.sh` has 17 call sites.** This is deliberate, since
identity is re-proved between stages to catch tampering. It also means any
defect in that one script fails 17 times in 17 places, which is what made the
`v0.3.0` failure unrecoverable rather than local.

## Recommendations

**R1. Make the credential preflights dispatch-reachable.** Add an input to
`release.yml` that runs `governance`, `release-config-preflight` and the three
credential preflights without any publication job. This changes no permission
and no token, and leaves the reader count pinned by
`scripts/release/check-actions-policy.sh` untouched. It converts F3 and most of
F4 from tag-bound discoveries into ordinary CI.

**R2. Do not add a tag namespace for rehearsals.** It needs a carve-out in the
`v*` ruleset and turns the `is_release` decision from two cases into three. New
untested branches in the pipeline are the problem, not the remedy. Published
preview builds are a separate need and already have a supported form:
SemVer prerelease tags, `v0.3.1-rc.1`, which `RELEASING.md` § 4 publishes as a
non-latest prerelease. Note that Debian orders `0.3.1-rc.1` above `0.3.1`; the
archive contract needs `0.3.1~rc1` if prereleases ever reach APT.

**R3. Separate the tag branch of `metadata` into its own callable check.**
Given a tag name, a commit and the three version files, the verification in F2
is a pure function. It can run on the release pull request against the version
it proposes, before any tag exists.

**R4. Distinguish "immutable because public" from "immutable because created".**
A tag whose run died before any artifact existed has no consumer. The
`release-recovery` job already knows how to prove that no release and no asset
exists for a tag. That proof is a sound precondition for deleting and recreating
an unpublished tag, and it would have made `v0.3.0` recoverable without a
version bump.

**R5. Automate the `autorelease: pending` transition.** `skip-github-release:
true` means nothing ever relabels a merged release PR, so Release Please
deadlocks after every release until the label is removed by hand. The `publish`
job knows the tag and can resolve the release PR from it.
