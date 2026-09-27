---
title: Release status
description: What you can use today, and which claims still need a published release or hardware evidence.
---

## Published state

**AROS tools is in beta.** The stable, immutable
[`v0.3.12` release](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.12)
provides native archives for Linux x86-64, Linux ARM64 and macOS Apple silicon.
Its 40 assets, signatures, attestations and public URLs were independently
verified. Signed APT packages, Homebrew and AUR were also qualified. See
[installation](/aros-tools/getting-started/installation/) for the available
paths.

The tools release and the cross-toolchain release are separate. The
[`v0.1.4` toolchain release](https://github.com/metaneutrons/aros-toolchains/releases/tag/v0.1.4)
contains nine artifacts for three hosts and three target profiles; it does not
imply that a complete AROS distribution or any board boot was qualified.

| Area | Current boundary |
| --- | --- |
| Tools installation | Native `v0.3.12` release or source build |
| Native tools archives and package managers | Public native archives, signed APT, Homebrew and AUR at `v0.3.12` |
| Upstream source lifecycle | Implemented with explicit source identity and graph validation |
| Integrated product build | Uses the tools-owned engine; requires compatible AROS sources and compiler inputs |
| Pristine upstream full product | Not yet a generally qualified product-build claim |
| PC boot check | x86 QEMU implementation with retained evidence |
| Pi/Milk-V models | Profile/artifact support; not a blanket hardware boot guarantee |
| External application workflow | Matching compiler and SDK inputs; no application packaging frontend |

## Current source state

The source implements typed target and toolchain contracts, standalone build
tools, structured diagnostics, and board deployment/media validation.
See the [command reference](/aros-tools/reference/cli/) and
[standalone tools](/aros-tools/reference/standalone-tools/) for the implemented
surface.

The workspace also has tests for deterministic archive assembly, source
transactions, toolchain identity, package payloads and failure handling.
A test or workflow definition describes a capability; evidence for a specific
release must identify its exact tag and measured artifacts.

## Release qualification boundary

Each new candidate must pass every native archive host it declares, binary
compatibility checks, archive and SBOM verification, signatures and provenance,
isolated-download checks, and applicable package-channel qualification. The
native release matrix is Linux x86-64, Linux ARM64 and macOS Apple silicon.
macOS Intel is not a release target.

A further claim of full product support from pristine upstream requires its
own source/build acceptance evidence. Physical boot support requires the
matching board, firmware, source revision, artifacts and UART evidence.

Check [GitHub Releases](https://github.com/metaneutrons/aros-tools/releases)
for immutable public tools versions and
[toolchain releases](https://github.com/metaneutrons/aros-toolchains/releases)
for the separate compiler artifacts.
