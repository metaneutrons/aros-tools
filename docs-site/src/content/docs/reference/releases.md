---
title: Release engineering
description: Immutable release identities, artifact verification, and publication gates.
---

`aros-tools` releases are built for four native hosts from one reviewed source
revision. GitHub Releases are the canonical binary source; package managers
must point to those immutable archives and exact measured SHA-256 values.

## Version preparation

Release Please derives the next SemVer and `CHANGELOG.md` from Conventional
Commits and opens a reviewable release pull request. It updates the Cargo
workspace as one version. It deliberately does not create a tag or GitHub
Release: merging a version PR changes source metadata only.

## Supported native archives

| Host | Target triple |
| --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |

Every archive contains the `aros` frontend and the independently executable
collector, fetcher, transpiler, verifier, module generator, ROM tool and AHI
runner. The internal `aros-release` producer is not installed.

## Immutable identity

A release starts only from an annotated tag named `vX.Y.Z` whose peeled commit
is reachable from protected `main`. The tag version must equal the Cargo
workspace version, Release Please manifest and changelog heading. The workflow
verifies the tag object, peeled commit and the live immutable
`refs/tags/v*` ruleset before building. The source commit time normalizes the
binary payload; the annotated tagger time independently bounds publication.
Existing artifacts, tags and releases are never overwritten or retargeted; a
failed candidate gets a new version.

Pull requests exercise the same four archive lanes with the workspace SemVer
and an isolated run-scoped candidate identity, but cannot sign or publish a
release. Untrusted PR jobs receive no OIDC token; signing and attestation live
only in the protected tag path. Package metadata, generated provenance strings
and compiled `--version` output therefore remain identical during
qualification.

## Producer contract

Each native lane builds the complete workspace once and those binaries feed
every package for that host. The risk policy may schedule a separate,
independent A/B lane; that lane exists only to reproduce and compare the
canonical archive and is never an alternative package source. Full A/B is
mandatory for the first trusted stable baseline, every `X.Y.0`, every
dependency/build/release-graph change, every path outside a closed low-risk
application-source/documentation allowlist, and a tag carrying
`AROS-Release-Qualification: full-ab`. A patch uses one build per host only
when immutable release history is available and manifest/lockfile differences
are workspace-local version changes; uncertainty selects A/B. The
repository-internal producer then:

1. validates the explicit version, commit, timestamp, target and input paths;
2. rejects missing binaries, symbolic links, special files and existing output;
3. normalizes gzip and tar metadata and writes an exact payload manifest;
4. writes a SHA-256 sidecar atomically; and
5. reopens the archive and verifies every path, type, mode, timestamp, size and
   digest.

Independent tests create two copies and require byte identity. A mutation test
proves that read-back verification fails closed.

## Promotion gates

For a real tag, each archive and its manifest, checksum and SPDX SBOM receive a
keyless Sigstore signature and GitHub build-provenance attestation. The
aggregator requires the complete four-host inventory and verifies all payloads,
signatures and attestations before creating a draft. SBOM validation is
semantic: the document must be SPDX 2.3, bind the measured artifact SHA-256 and
name/version, carry its digest-derived namespace, and describe exactly the
eight shipped executable paths and individual SHA-256 digests. Syft itself is
downloaded as one of four platform-specific v1.51.1 assets whose SHA-256 is
hard-coded and checked before execution; no mutable installer is used.

| Producer host | Syft asset | SHA-256 |
| --- | --- | --- |
| Linux x86-64 | `syft_1.51.1_linux_amd64.tar.gz` | `8fcb33017a0dc1058298c923c436d19dfa68ae93968e0b423248542e3afb9fc3` |
| Linux ARM64 | `syft_1.51.1_linux_arm64.tar.gz` | `a7fd2b784e6664acd44719270574f6cd8c6864fc2b1700bf9099bd1cccda7d7f` |
| macOS Intel | `syft_1.51.1_darwin_amd64.tar.gz` | `0e186ce1d4351ec276126851ca3ff258ed070e93e73574ed64858d4fc2339867` |
| macOS Apple silicon | `syft_1.51.1_darwin_arm64.tar.gz` | `ac063af3b9874769deb7ea1e6d76841e68f9e3bb50cd654226fc977de65532c1` |

The Linux archives are checked against the documented maximum glibc symbol
version; macOS builds set and inspect the documented deployment target. This
prevents a runner-image update from silently raising the supported host floor.
The Linux producer installs no packages from a moving distribution repository
before building. Its compiler and linker come from the digest-pinned Rust
container, while `lzma-sys` is forced to compile the vendored XZ source selected
by `Cargo.lock` on every host. A linkage gate rejects any delivered binary that
still resolves a host `liblzma`.

The complete candidate is sealed as an immutable run-scoped Actions artifact.
Its changelog section is rendered deterministically as signed
`RELEASE_NOTES.md`; the GitHub title and body must remain byte-exact. An
OIDC-free, checkout-free recovery job resolves private drafts through paginated
release lists and numeric release/asset IDs, then hands verified historical
bytes to the signing jobs. A metadata-only private draft receives subjects and
keyless bundles by numeric release ID before the checksum inventory and its
bundle are uploaded last; this preserves recoverability across partial uploads.
The populated draft is downloaded
into an isolated directory and
its checksum inventory, signatures and attestations are verified again. Only
that unchanged draft may be published, exactly once, with its final status.
GitHub's immutable flag and server-computed asset digests are mandatory.

Debian, Homebrew and AUR candidates are mandatory gates before the first public
tag. Public package-channel mutation occurs only after the immutable GitHub
release exists. The APT repository uses its own R2 bucket and is not a second
binary source for native archives.
Each Debian lane installs its generated package, verifies the installed files
with `dpkg`, starts all eight commands and compares every installed executable
byte-for-byte with the corresponding canonical archive payload.
The generated Homebrew formula is installed and tested natively on all four
supported hosts. The AUR recipe is built for both Linux architectures; its
packaged executables are compared with the canonical archive payloads, and the
x86-64 package is additionally installed and exercised in an Arch Linux
environment. These qualification copies substitute only a loopback download
base while the immutable GitHub draft is still private.

Stable tags use a second, credential-isolated boundary. The private draft is
published stable/latest once; no intermediate public prerelease exists. APT,
Homebrew and AUR then roll forward from the same sealed staging bytes. See
[Package publication](/aros-tools/reference/publication/) for the exact
credential, commit-point and saga contract.

Publication is globally serialized, so two tags cannot update mutable package
indexes concurrently. An interrupted run resumes only from verified public
state. Existing Sigstore bundles are reused only after certificate and subject
verification; nondeterministic keyless evidence is never regenerated over an
immutable release. The cross-service flow is a roll-forward saga: an APT
commit followed by a Homebrew outage is visible partial channel convergence,
not an atomic rollback.

Before exposure, the workflow requires immutable releases to be enabled and
rejects downgrade or divergent same-version state across GitHub, signed APT,
Homebrew and AUR. Immediately before the one-time final publication, the workflow compares the
private draft with staging, rechecks `SHA256SUMS`, SBOM semantics, every
Sigstore bundle and every strictly scoped attestation, and closes the tag
TOCTOU window. Every later package-channel write checks the immutable public
asset names, sizes and GitHub-computed SHA-256 digests. A final read-only audit
repeats both those checks and all four public-channel checks after convergence.

## Release diagnostics

The producer uses stable `APxxxx` diagnostic codes and the shared observability
contract:

```sh
aros-release verify \
  --archive aros-tools-v1.0.0-aarch64-apple-darwin.tar.gz \
  --manifest aros-tools-v1.0.0-aarch64-apple-darwin.tar.gz.manifest.json
```

Machine consumers can select `--diagnostic-format json`. Local logging is off
by default and requires both an explicit level and file:

```sh
aros-release --diagnostic-format json \
  --log-level debug --log-format jsonl --log-file ./release.jsonl \
  verify --archive ./archive.tar.gz --manifest ./archive.manifest.json
```

No release command infers a tag, generates a checksum pin, silently chooses a
different artifact, or replaces an existing destination.
