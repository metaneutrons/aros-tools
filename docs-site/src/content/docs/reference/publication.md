---
title: Package publication
description: Least-privilege credentials and fail-closed promotion across APT, Homebrew, and AUR.
---

Stable package publication is a gated continuation of the immutable GitHub
release workflow. It never rebuilds a binary. Every downstream package consumes
the exact archives or Debian packages already sealed in the private,
run-scoped `qualified-release-staging` Actions artifact. That artifact is the
staging object set; it is not a public prerelease or package source.

## Promotion sequence

1. Complete all credential-free host, package, provenance and staging gates.
2. Enter one narrowly scoped protected environment at a time. Signing,
   object-store publication, Homebrew publication and AUR publication never
   share a runner or credential domain.
3. Create a private GitHub draft from staging, download it in isolation, and
   verify its exact inventory, checksums, Sigstore bundles and attestations.
4. Publish that draft exactly once with its final status: stable/latest for a
   stable tag, or non-latest prerelease for a prerelease tag. Verify GitHub's
   immutable flag and server-computed asset digests.
5. For stable releases, an APT-only runner signs a closed, public object set.
   A different R2-only runner accepts exactly that inventoried handoff,
   publishes package objects before signed metadata, and a credential-free
   runner verifies the public bytes and signature.
6. Open a formula PR in `metaneutrons/homebrew-tap`; its protected
   `Formula qualification` gate installs and tests the measured formula on
   Intel and ARM macOS plus x86-64 and ARM64 Linux. Recheck its exact head SHA
   and require one independent maintainer to approve that exact final head.
   Merge with GitHub's `match-head-commit` precondition. Tap `main` must enforce
   strict checks for administrators and forbid force-pushes/deletion.
7. Push the measured `PKGBUILD` and freshly generated `.SRCINFO` to
   `aros-tools-bin` from an AUR-key-only runner. A new credential-free runner
   consumes a closed `PKGBUILD`/`SRCINFO`/commit evidence handoff and verifies
   both Git and AUR RPC state.

A prerelease tag remains a GitHub prerelease and deliberately does not update
the stable package-manager channels. It also never enters any stable
publication environment, so it cannot read those credentials. GitHub immutable
releases cannot be promoted from prerelease to stable; the workflow therefore
never publishes a stable tag as an intermediate prerelease. Rerunning the
exact unchanged tag is the only resume path. It re-verifies the tag, immutable
release inventory, public bytes and channel state and rejects any differing
object.

## Protected credential domains

Configure five independent GitHub environments. APT signing and R2 publication
intentionally reuse their narrowly scoped identities for tag publication and
metadata refresh, but those operations still run on separate jobs/runners and
enforce their exact tag or `main` ref independently. Other credential domains
stay in their one named environment:

| Environment | Allowed ref | Secrets |
| --- | --- | --- |
| `release` | annotated release tags matching `v*` | `RELEASE_ADMIN_READ_TOKEN` (this repository only, Administration read-only) |
| `apt-signing` | annotated release tags matching `v*` and protected `main` | `APT_GPG_PRIVATE_KEY`, `APT_GPG_PASSPHRASE` |
| `apt-publication` | annotated release tags matching `v*` and protected `main` | `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` |
| `homebrew-publication` | annotated release tags matching `v*` | `PACKAGE_PUBLISH_TOKEN` |
| `aur-publication` | annotated release tags matching `v*` | `AUR_SSH_PRIVATE_KEY` |

Require review for every environment. Pull requests and ordinary qualification
jobs receive none of them. The refresh workflow independently refuses every ref
except the current remote `main` tip, so manual dispatch cannot select an older
branch or tag even if an environment policy is misconfigured.

Environment review and pull-request review are separate controls. Once the
Homebrew job exposes its generated tap PR, an independent maintainer must
approve the exact recorded head after the final push and allow the protected
`Formula qualification` check to finish. The job waits for no more than 180
minutes. Never amend that branch or merge it manually. If the wait expires,
rerun only the same immutable release tag; the workflow must reuse the same
version branch, PR and byte-identical head before it may merge, and the approval
must cover that exact head.

| Secret | Required scope |
| --- | --- |
| `RELEASE_ADMIN_READ_TOKEN` | Fine-grained access only to this repository, Administration read-only; used by one checkout-free preflight to read immutable-release policy |
| `PACKAGE_PUBLISH_TOKEN` | Fine-grained access only to `metaneutrons/homebrew-tap`, with Contents and Pull requests read/write plus Administration read for the branch-protection preflight |
| `AUR_SSH_PRIVATE_KEY` | Dedicated unencrypted CI key whose public half is registered in the AUR account |
| `APT_GPG_PRIVATE_KEY` | Dedicated archive-signing private key |
| `APT_GPG_PASSPHRASE` | Passphrase for that signing key |
| `R2_ACCESS_KEY_ID` | R2 S3 credential with Object Read & Write on the shared distribution bucket |
| `R2_SECRET_ACCESS_KEY` | Secret half of the same bucket-scoped credential |

Treat the APT signing key as recoverable release infrastructure, not merely as
a CI secret. Before the first stable tag, keep an operator-controlled encrypted
backup of the private key, store its passphrase separately, and exercise a
clean import, signature and verification round trip. A GitHub Actions secret
cannot be exported and therefore does not count as a backup. Record only the
public fingerprint in the repository; never commit the private key, its
passphrase or an operator-specific backup path.

Store the following non-secret values as repository variables. Credential-free
preparation and verification jobs deliberately need them without entering a
protected environment:

| Variable | Contract |
| --- | --- |
| `AUR_SSH_KNOWN_HOSTS` | Reviewed `aur.archlinux.org` host-key entry; dynamic trust-on-first-use is rejected |
| `APT_GPG_FINGERPRINT` | Full 40-hex primary-key fingerprint |
| `APT_PUBLIC_BASE_URL` | `https://deb.metaneutrons.cc/aros-tools` |
| `R2_ACCOUNT_ID` | Exact 32-hex Cloudflare account ID for the `lexICT` account |
| `R2_BUCKET_NAME` | Dedicated Standard-storage bucket `aros-distributions` |

The `release` preflight requires GitHub immutable releases to be enabled before
any public exposure. `metaneutrons` is a user account, so GitHub cannot report
owner enforcement; repository-level `enabled=true` is the strongest available
state. Secrets are attached only to the individual step that needs them. The
Homebrew PAT is absent from checkout, staging download and source verification;
destination-repository scripts are never executed in its presence. APT signing
and R2 upload use separate jobs and separate runners. Their handoff is accepted
only when it contains the exact regular-file inventory of public packages,
indexes, by-hash objects, key and signed metadata. The AUR key is destroyed
before its three-file public verification handoff is sealed; public AUR
verification runs later without that key. Every secret-bearing step installs an
EXIT/signal cleanup trap and kills its GnuPG agent before the runner can proceed.

Before GitHub publication, separate runners exercise the Homebrew, R2, APT and
AUR credentials, followed by a credential-free aggregator. Another
credential-free gate checks all four public version states and refuses a
downgrade; same-version APT recovery requires the exact immutable package and
by-hash base before it may reconcile mutable aliases. The R2 preflight verifies actual conditional write,
read-back, and single-object delete capability under the non-channel
`aros-tools/.publication-probes/` prefix. The probe is run/attempt-scoped,
contains no release payload, and is removed before the gate succeeds; package
or index keys are never used for capability tests.

The R2 bucket and its production custom domain are infrastructure prerequisites;
the release workflow does not create, delete, or reconfigure Cloudflare
resources. R2's S3 endpoint is used only for object access. Uploads never use a
recursive delete: immutable package objects and `by-hash/SHA256` indexes go
first, mutable index aliases next, and the signed `InRelease` file is the sole
commit point. `Acquire-By-Hash: yes` lets a client that observed an older signed
commit keep resolving its immutable indexes during promotion.
The archive key object is also absent-or-byte-identical; key rotation is a
separate reviewed migration and cannot be smuggled into a package release.

Every public write immediately revalidates the annotated tag, source commit,
active `refs/tags/v*` update/deletion ruleset and complete GitHub Release asset
inventory. Main must require strict status checks for administrators, linear
history, and prohibit force-pushes and deletion. Asset names, sizes and
GitHub-computed SHA-256 digests must still match staging, and the release must
carry GitHub's immutable status. The seven-day tag window applies to creating
and publishing the GitHub Release. Once it is canonical and immutable, a
channel outage may be recovered later by rolling that exact release forward.

APT metadata has a bounded `Valid-Until`, but package availability does not
depend on a new product release every 90 days. The protected weekly refresh
workflow reconstructs metadata from the current immutable stable release,
requires the public `.deb` and every by-hash object to remain byte-identical,
and normally changes only the signed release triplet. An
uncredentialed job seals the single unsigned `Release` input, the signing job
returns exactly the public key and signed triplet, an R2-only job revalidates
the release and immutable public bytes, captures one validated snapshot of all
seven mutable objects, repairs missing or divergent `Packages*` aliases when
needed, and uses only that snapshot's ETags or absence conditions for writes.
It never authorizes a write from a fresh post-validation ETag. A final
credential-free job downloads and verifies the complete public inventory. Its
standard-library renderer runs in
`python:3.14.2-slim-bookworm@sha256:e87711ef…70941` with networking disabled.
R2 conditional writes reject concurrent refresh/publication races and
`InRelease` remains the final commit point.

Production captures a current metadata epoch when the APT channel first commits;
the immutable package and index bytes do not depend on that epoch. A later tag
rerun preserves a still-valid protected refresh after reconstructing both index
forms exactly. If same-version signed metadata has expired or a mutable alias is
missing, the recovery path reconciles `Packages`, `Packages.gz`, `Release`,
`Release.gpg`, and `InRelease` under their original snapshot and still commits
`InRelease` last. A present commit point must have one canonical clear-sign
envelope, exactly one valid signature, and the configured primary fingerprint.

Public APT qualification downloads the archive key, complete signed triplet,
both plain and compressed indexes, all four by-hash objects and both packages.
It accepts exactly one primary key and one `VALIDSIG` bound to that fingerprint,
then installs the exact expected package version from the public hostname. AUR
qualification compares the pushed `PKGBUILD` and `.SRCINFO` byte-for-byte with
the generated candidates before accepting convergence. Homebrew likewise tests
the exact formula commit produced from measured archive values.
The final read-only audit repeats GitHub metadata/assets, signed APT/by-hash,
Homebrew formula and AUR RPC/Git checks so a successful ecosystem subworkflow
cannot mask a later channel race.

The bucket is namespaced by product. This workflow owns only `aros-tools/`; the
reserved `toolchains/` and `images/` prefixes can be published independently
without changing existing package URLs. `deb.metaneutrons.cc` is the dedicated
APT hostname; `aros.metaneutrons.cc` remains the general AROS distribution and
documentation hostname.

## Failure contract

Preflight emits stable `AP71xx` diagnostics for missing or malformed release
configuration. APT construction and public verification use `AP72xx`, Homebrew
publication uses `AP73xx`, and AUR convergence uses `AP74xx`. Secret values are
never printed.

The cross-channel transaction is deliberately described as a saga, not as
atomic publication. GitHub's immutable stable release is canonical and public
before package channels commit. APT, Homebrew and AUR then converge in order.
If a later service fails, earlier successful state is not rolled back or
deleted; the run reports the exact incomplete phase and the exact tag must be
rerun until every channel converges. A payload defect requires a new version.
