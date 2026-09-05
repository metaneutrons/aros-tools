---
title: Package channels
description: Choose a distribution channel and understand how its packages relate to the native release.
---

:::caution[Availability]
The first stable tools release is not yet public. These are the intended
package contracts. [Release status](/aros-tools/reference/release-status/)
records which channels are actually available.
:::

## Choose a channel

| Channel | Platforms | Package or location |
| --- | --- | --- |
| GitHub native archive | macOS Intel/Apple silicon; Linux x86-64/ARM64 | [aros-tools releases](https://github.com/metaneutrons/aros-tools/releases) |
| Homebrew | The same four hosts | `metaneutrons/tap/aros-tools` |
| Signed APT | Debian/Ubuntu on `amd64` or `arm64`, within the binary compatibility floor | `https://deb.metaneutrons.cc/aros-tools` |
| AUR | Arch Linux on `x86_64` or `aarch64` | `aros-tools-bin` |

[Installation](/aros-tools/getting-started/installation/) contains the commands
and verification procedure. [Platform support](/aros-tools/reference/platform-support/)
lists the operating-system compatibility floors.

## One native payload per host

The native archive is the canonical binary payload. Debian, Homebrew and AUR
packages consume those same measured executables; package channels are not
independent compiler builds. Keep the complete suite at one version.

A public GitHub release does not, by itself, prove that every package channel
has completed publication. Consult the package's version and release status
before installing or upgrading. Prereleases do not update stable channels.

## What to verify

- **Native archive:** match the host triple, checksum, manifest and signature/
  attestation to the selected immutable tag.
- **Homebrew:** use the documented tap. The formula selects and checks the
  corresponding archive digest.
- **APT:** verify the full public signing-key fingerprint, scope it with
  `signed-by`, and allow APT to authenticate the repository metadata.
- **AUR:** review `PKGBUILD` and `.SRCINFO`; check that the source and SHA-256
  refer to the intended version's canonical archive.

The APT key fingerprint and complete verification commands live in
[the installation guide](/aros-tools/getting-started/installation/#debian-and-ubuntu).
Do not bypass an expired signature or checksum mismatch to complete an install.

## Updates and failures

Use the package manager that installed the suite. An unavailable package or
mismatched version is a channel issue; it is not a reason to mix executables
from separate releases. See
[update and uninstall](/aros-tools/getting-started/update-uninstall/) and
[installation troubleshooting](/aros-tools/reference/troubleshooting/#package-or-release-installation).
