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

Stable package promotion is globally serialized. Four protected environments
isolate the Administration-read immutable-release check (`release`), the
central archive request (`apt-archive-publication`), Homebrew
(`homebrew-publication`) and AUR (`aur-publication`). No runner may combine
credential domains. The archive request uses a short-lived GitHub App token
restricted to `metaneutrons/apt-archive`, with Actions write and Contents read.
Its private-key action revokes the installation token at job completion; no
archive signing key or storage credential belongs in this repository.

`RELEASE_ADMIN_READ_TOKEN` remains a repository-only Administration-read
credential used only in the checkout-free immutable-release preflight.
A user-owned repository cannot enable owner-enforced immutable releases, so
the supported repository `enabled=true` policy is required. PR and ordinary
qualification jobs receive neither OIDC nor publication permissions.

The archive's public contract is in `contracts/apt-archive-v1.toml`: primary
and domain-subkey fingerprints, keyring, origin, suite, architectures, validity
and retention. Preflight reads the protected archive main manifest as inert
data and requires an exact contract match. It does not execute archive code,
sign anything or request publication.

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

For a stable release, package channels roll forward from the same sealed
staging bytes. The central archive fetches the public, attested `.deb` files
and exclusively owns rendering, signing, retention, refresh and storage writes.
The tools dispatch its `publish.yml` for the fixed domain/project and follow
only the numeric run ID returned by GitHub, bound to the validated protected
archive commit. Missing or ambiguous dispatch responses fail closed; never
guess a run by selecting the latest one.

A separate credential-free job verifies what clients receive: the exact
primary and domain signing subkey, active key status, both Release signatures,
publication/expiry times, the complete four-index SHA-256/SHA-512 by-hash
matrix, matching compressed/uncompressed indexes and both exact candidate
packages. Retained older versions are allowed; a newer version, mixed
architecture versions or changed same-version bytes fail. Both native Linux
architectures install via an isolated signed APT source and compare all eight
installed binaries with the release candidate.

Homebrew merges only the exact checked PR head. An AUR-key-only runner
publishes the measured `PKGBUILD`/`.SRCINFO`, followed by independent public
verification. No destination repository helper runs with a write PAT present.
Every shell step that materializes a private key removes its temporary
credential files before another step can run.

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

APT refresh and damaged-index recovery belong to `metaneutrons/apt-archive`.
There is no project-owned scheduled refresh or second signing key. If its
publication fails after the immutable GitHub release is public, diagnose the
exact archive run and recover there; rerunning the unchanged tools tag can
request a new archive run and must reverify all public bytes before continuing.
Do not delete or overwrite successful immutable release assets.

The initial untagged 0.1.0 PR was superseded before publication. Release Please
prepared a fresh 0.1.1 candidate including later fixes. A merged but untagged
candidate can be explicitly retired by removing its `autorelease: pending`
label with an audit comment, then dispatching Release Please again. This does
not mark the version published, mutate a tag, hand-edit a version, or bypass
qualification of the new release PR.
