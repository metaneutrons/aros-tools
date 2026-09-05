---
title: Release status
description: What you can use today, and which claims still need a published release or hardware evidence.
---

## Published state

**AROS tools is in beta.** No stable `aros-tools` release is published.
Build from source using [installation](/aros-tools/getting-started/installation/).
The planned native archives, Homebrew, APT and AUR paths must not be treated
as available until they have public release evidence.

The tools release and the cross-toolchain release are separate. An available
toolchain prerelease does not imply an available tools package.

| Area | Current boundary |
| --- | --- |
| Tools installation | Source build |
| Native tools archives and package managers | First stable publication pending |
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

## Required for the first public release

The candidate must pass its four native hosts, binary compatibility checks,
archive and SBOM verification, signatures and provenance, isolated-download
checks, and applicable package-channel qualification.

A further claim of full product support from pristine upstream requires its
own source/build acceptance evidence. Physical boot support requires the
matching board, firmware, source revision, artifacts and UART evidence.

Check [GitHub Releases](https://github.com/metaneutrons/aros-tools/releases)
for immutable public tools versions and
[toolchain releases](https://github.com/metaneutrons/aros-toolchains/releases)
for the separate compiler artifacts.
