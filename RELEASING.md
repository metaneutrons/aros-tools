# Releasing aros-tools

Releases have two deliberately separate control planes: Release Please owns the
SemVer/changelog pull request, while an immutable annotated tag starts the
credential-free qualification and protected publication pipeline. Release
Please never creates a tag or GitHub Release for this repository.

## 1. Prepare the release pull request

The SHA-pinned Release Please workflow runs on `main`. Review its pull request
like any other change. Before merge, require:

- the intended SemVer in `Cargo.toml`, `Cargo.lock` and
  `.release-please-manifest.json`;
- a complete matching `CHANGELOG.md` entry;
- updated release status and installation documentation;
- the complete workspace, Rustdoc, dependency and documentation gates; and
- an explicit source-contract update if the toolchain producer changed.

Merging this pull request authorizes no publication. Never hand-edit a version
to bypass it.

## 2. Create the immutable candidate tag

Start from a clean, freshly fetched `origin/main`. Select the exact reviewed
release-PR merge commit and verify that it remains reachable from the protected
default branch. It will normally be the current tip, but later unrelated main
commits do not invalidate an otherwise unchanged release commit. Create an
annotated `vX.Y.Z` tag whose version matches the workspace and Release Please
manifest, then push only that tag.

The repository must already have an active tag ruleset covering
`refs/tags/v*` with both update and deletion forbidden. Qualification checks
the live ruleset as well as the tag object; branch protection alone is not a
release-tag immutability control.

Never retarget, delete or reuse a release tag. If qualification exposes a
source defect, fix it through a new pull request and choose a new SemVer. If the
payload is valid but publication infrastructure failed, rerun the workflow for
the exact unchanged tag. That is the only resume path: it must byte-verify the
existing release and public channel state before continuing.

## 3. Qualification

The tag workflow must prove all of the following before any publication
credential becomes available:

1. the annotated tag peels to a commit still reachable from protected `main`;
2. every native archive builds and passes its host smoke tests;
   Linux builds install no moving apt packages, and every host statically uses
   the `Cargo.lock`-selected vendored XZ implementation;
3. releases selected by the risk policy compile a second time on independent
   runners and reproduce archives, manifests and checksums byte-for-byte;
4. SPDX 2.3 SBOM subjects bind the measured artifact and exactly eight shipped
   binary paths and SHA-256 digests;
5. signatures and GitHub attestations verify in an isolated download against
   the exact signer workflow, tag ref and source commit, while self-hosted
   provenance is rejected;
6. Debian, Homebrew and AUR candidates derive from the same native payloads;
7. the complete candidate is sealed as the immutable, run-scoped
   `qualified-release-staging` Actions artifact; and
8. package installations pass on every claimed platform.

Full A/B is mandatory for the first trusted stable baseline, every `X.Y.0`,
every release/build-graph or dependency change, every path outside the closed
low-risk application-source/documentation allowlist, and every annotated tag containing the exact line
`AROS-Release-Qualification: full-ab`. An ordinary patch may use one native
build per host only when the previous baseline is an immutable published stable
release and `Cargo.toml`/`Cargo.lock` differ solely in workspace-local release
versions; deterministic packaging fixtures still run. Missing or malformed
history, unknown paths and semantic manifest/lockfile changes select full A/B.
Security or other high-risk patch releases must carry that tag line. The
workflow records the selected reason. This is a cost policy, not a claim that
an unselected patch received independent reproduction.

The Actions artifact is the private immutable staging object set. It is not a
GitHub Release, is not publicly addressable as a package source, and makes no
partial-release claim.

## 4. Protected publication

Stable package promotion is globally serialized. Five protected environments
isolate the Administration-read immutable-release check (`release`), APT
signing (`apt-signing`), R2 publication (`apt-publication`), Homebrew
(`homebrew-publication`) and AUR (`aur-publication`). The APT and R2
environments deliberately admit both annotated `v*` tags and protected `main`:
the tag publication and scheduled refresh workflows reuse the same narrowly
scoped identities but keep them on separate jobs/runners and apply their own
exact-ref checks. No runner or job may combine credential domains.
`RELEASE_ADMIN_READ_TOKEN` is a
fine-grained, repository-only Administration-read token used only by the
checkout-free `release` preflight; a user-owned repository cannot enable
owner-enforced immutable releases, so the workflow requires the supported
repository `enabled=true` policy. PR jobs and ordinary qualification jobs must not receive OIDC
or publication permissions; public verification jobs enter no environment.
Its preflight exercises the R2 credential only with a run-scoped, non-channel
write/read/delete probe and removes that object before the gate succeeds.

An OIDC-free, checkout-free `contents:write` recovery job lists every release
page, resolves a private draft by numeric release ID, and downloads every
asset by numeric ID before any signing job runs. Signing jobs receive only that
verified handoff and either reuse its valid historical Sigstore bundles or fail
closed. The workflow creates a metadata-only private GitHub draft, uploads
subjects and bundles by numeric release ID, and uploads `SHA256SUMS` plus its
bundle last so an interrupted partial draft remains recoverable without
changing already checksummed keyless evidence. It uses the signed deterministic
`RELEASE_NOTES.md` as both asset and
exact release body, downloads and verifies it in isolation, and publishes it
exactly once with its final status. A stable tag is published stable/latest in that one operation; a
prerelease tag is published as a non-latest prerelease. GitHub immutable
releases do not permit a later prerelease-to-stable transition, so the workflow
never attempts one.

For a stable release, package channels then roll forward from the same sealed
staging bytes. An APT-key-only runner produces a closed public artifact; an
R2-only runner re-verifies that exact handoff, uploads immutable packages and
content-addressed by-hash indexes before mutable aliases, and writes signed
`InRelease` last. Homebrew merges only the exact checked PR head. An AUR-key-only
runner publishes the measured `PKGBUILD`/`.SRCINFO`, while a separate public
runner verifies its closed three-file evidence handoff. No destination
repository helper is executed while a write PAT is present. Every private-key
step removes its credential files through an EXIT trap and terminates GnuPG
agents before a later step can run.

The deliberate creation and push of the immutable annotated release tag is the
single human promotion gate. Homebrew adds no redundant self-review ceremony:
the protected `Formula qualification` check must pass on all four hosts, the
publication job remeasures the exact final head and revalidates both repositories,
then merges only that recorded SHA through GitHub's `match-head-commit`
precondition. Do not push a follow-up commit or merge the PR manually. If the
job stops, rerun only the unchanged release tag: recovery must reuse the same
version branch, pull request and byte-identical head.

Before stable exposure, a credential-free gate examines GitHub, signed APT
(including by-hash), Homebrew and AUR. A newer public version rejects the run;
an existing same version is accepted only with exact candidate bytes. The
final audit repeats the four-channel check after convergence. Prereleases never
update stable APT, Homebrew or AUR channels.

## 5. Recovery and rollback

Rerunning the exact tag workflow accepts an existing draft or immutable final
release only when every remote asset is byte-identical to the candidate
inventory and the tag still peels to the same protected commit. Existing
keyless Sigstore bundles are copied only after their certificate identity and
subject verify and the freshly reproduced subject is byte-identical. A missing
bundle in an immutable release is fatal; it is never regenerated or replaced.
Homebrew uses one deterministic version branch/PR and exact head SHA, while AUR
accepts an already identical `PKGBUILD`/`.SRCINFO`.

The seven-day tag window protects creation and publication of the GitHub
Release. Once that canonical immutable release exists, package-channel recovery
may continue later: every write still rechecks the tag, protected source and
complete immutable GitHub asset inventory. This is necessary for honest
roll-forward after a remote outage.

Cross-service atomic publication is impossible. GitHub is canonical and becomes
public first; APT, Homebrew and AUR are an ordered saga. A failure after any
commit point can therefore leave later channels behind. Do not delete or
rewrite successful immutable state and do not describe it as rolled back.
Rerun the exact tag to roll the remaining stages forward, record an incident if
convergence is delayed, and publish a new fixed version for a payload defect.

APT `Valid-Until` is refreshed weekly by the protected
`refresh-apt-metadata.yml` workflow. A refresh must reproduce the public Debian
packages and by-hash objects byte-for-byte from the current immutable stable
GitHub release; in the healthy case it signs only a new `Release`/`Release.gpg`/
`InRelease` triplet. Before any write it captures and validates one R2 snapshot
of all seven mutable objects. Missing or divergent `Packages*` aliases are
reconciled with the deterministic bytes, while every write uses only the
original ETag or an original absence condition; `InRelease` commits last.
The initial channel commit likewise captures one current metadata epoch rather
than baking the tag time into an expiring repository. A tag rerun accepts an
already valid protected refresh only after both index forms reproduce exactly;
if same-version metadata has expired or a mutable alias is damaged, it repairs
the complete mutable set without changing immutable package/by-hash bytes.
Public verification then downloads the complete key, triplet, alias, by-hash
and package inventory and requires exactly one primary trust anchor plus one
`VALIDSIG` bound to its configured fingerprint.
The renderer runs in a digest-pinned, network-disabled Python container. A
downgrade, package change, index change, mutable release or race fails closed.
Refresh signing and R2 mutation also run on different runners under the shared
`apt-signing` and `apt-publication` environments. Configure both environments
to admit only annotated `v*` release tags and protected `main`, without a second
approval after the deliberate release tag, and retain the workflows'
independent exact-ref checks. Put the non-secret APT/R2 identity values in
repository variables so preparation and final verification remain
credential-free.
