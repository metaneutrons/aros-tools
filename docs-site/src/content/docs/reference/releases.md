---
title: Release engineering
description: Immutable release identities, artifact verification, and publication gates.
---

`aros-tools` releases are built for four native hosts from one reviewed source
revision. GitHub Releases are the canonical binary source; package managers
must point to those immutable archives and exact measured SHA-256 values.

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

A release starts only from an annotated tag named `vX.Y.Z`. The tag version
must equal the workspace version and the workflow verifies the tag object,
peeled commit and commit timestamp before building. Existing artifacts, tags
and releases are never overwritten or retargeted; a failed candidate gets a
new version.

Pull requests exercise the same four archive lanes with the workspace SemVer
and an isolated run-scoped candidate identity, but cannot sign or publish a
release. Package metadata and compiled `--version` output therefore remain
identical during qualification.

## Producer contract

Each native lane builds the complete workspace once. The resulting binaries
feed every package for that host; packaging is not allowed to compile a second
copy. The repository-internal producer then:

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
signatures and attestations before creating a draft.

The draft is downloaded into an isolated directory. Its complete checksum
inventory, signatures and attestations are verified again. Only that unchanged
draft may become a release; a prerelease version remains marked as such.

Debian packages, the signed APT repository, Homebrew formula and AUR package
are additional mandatory gates before the first public tag. The APT repository
uses its own R2 bucket and is not a second binary source for native archives.
Each Debian lane installs its generated package, verifies the installed files
with `dpkg`, starts all eight commands and compares every installed executable
byte-for-byte with the corresponding canonical archive payload.
The generated Homebrew formula is installed and tested natively on all four
supported hosts. The AUR recipe is built for both Linux architectures; its
packaged executables are compared with the canonical archive payloads, and the
x86-64 package is additionally installed and exercised in an Arch Linux
environment. These qualification copies substitute only a loopback download
base while the immutable GitHub draft is still private.

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
