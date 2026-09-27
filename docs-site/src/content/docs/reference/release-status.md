---
title: Release status
description: Current tools, toolchains and image availability.
---

## Available now

| Product | Release | Platforms or targets |
| --- | --- | --- |
| [AROS tools](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.12) | `v0.3.12` | Linux x86-64, Linux ARM64, macOS Apple silicon; APT, Homebrew and AUR |
| [Cross-toolchains](https://github.com/metaneutrons/aros-toolchains/releases/tag/v0.1.4) | `v0.1.4` | Three hosts × `pc-x86_64`, `arm-raspi`, `rpi-aarch64` |
| System images | None | No distribution image or board boot release |

The tools and cross-toolchains are separate downloads. Start with
[installation](/aros-tools/getting-started/installation/), then
[select a toolchain](/aros-tools/workflows/toolchains/) for your AROS checkout.
macOS Intel is not a release target.

AROS-NX provides the integrated product-build path. Full builds from pristine
upstream AROS and physical board boots require separate qualification; neither
is established by these releases. See [platform support](/aros-tools/reference/platform-support/)
for current limits and [versions and verification](/aros-tools/reference/releases/)
for release checks.
