---
title: Versions and verification
description: Understand tools versions, release evidence, and what a successful verification establishes.
---

## Version preparation

AROS tools uses one SemVer for the Rust workspace. Release Please derives
version changes and the changelog from Conventional Commits. A source version
or a merged version change is not proof that downloadable artifacts exist.

Check the installed frontend with `aros --version`. Keep its seven companions
at the same version. Cross-toolchains have their own release identities in the
selected AROS checkout's lock file.

## Supported native archives

The four archive targets are listed under
[platform support](/aros-tools/reference/platform-support/#native-host-matrix).
Each archive contains eight public programs: `aros`, `aros-ahi-runner`,
`aros-collect`, `aros-fetch`, `aros-genmodule`, `aros-romtool`,
`aros-transpiler` and `aros-verify`. It does not install the internal
release producer.

## Immutable identity

A public release binds a versioned tag to one source commit and an immutable
asset inventory. Existing tags and released assets are not repaired in place.
A payload defect requires a new version.

Use the tagged release's evidence. A successful build of a moving branch, a
tool's `--version`, or the existence of a checksum file alone does not
establish the full release identity.

## Producer contract

Canonical archives have normalized metadata and measured payload manifests.
The qualification policy requires independent A/B builds for the first trusted
stable baseline, minor/major baselines, and changes to dependencies or the
build/release graph. Eligible low-risk patches may use one build per host.
Do not assume that every patch release has a duplicate full compiler build.

Qualification also checks native execution, binary compatibility floors,
archive read-back and package payload identity.

## Promotion gates

The downloadable evidence ties each artifact to its source and bytes:

| Evidence | What it establishes |
| --- | --- |
| SHA-256 sidecar/inventory | Downloaded bytes match the declared digest |
| Payload manifest | Expected programs, file sizes, modes and individual digests |
| SPDX SBOM | Components and the artifact identity described by that release |
| Sigstore bundle | Artifact signature bound to the expected signing identity |
| GitHub attestation | Provenance bound to the repository, tag and source commit |

Checksums identify bytes; signatures and provenance establish who produced
those bytes and from which source. Verify both. The
[native installation procedure](/aros-tools/getting-started/installation/#native-release-archive)
shows the exact commands.

Package availability is a separate observation; see
[package channels](/aros-tools/reference/publication/).

## What verification does not prove

A verified compiler archive does not prove an AROS image boots. A successful
source translation does not prove every target was compiled. A passing PC QEMU
boot check does not prove Pi or Milk-V hardware support.

Use [release status](/aros-tools/reference/release-status/) for publication,
[platform support](/aros-tools/reference/platform-support/) for implemented
boundaries, and the selected AROS project's hardware evidence for a board.
