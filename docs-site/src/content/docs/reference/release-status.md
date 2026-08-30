---
title: Release status
description: Claims that are green and gates that remain open.
---

## Verified

- Complete Rust workspace tests and architecture boundaries on the extracted history.
- No third-party assistant attribution in the rewritten `aros-tools` history or source tree.
- Translation and differential verification against the immutable AROS-NX
  source contract selected by CI.
- A complete local AROS-NX `pc-x86_64` product build using the published RC3 cross-toolchain.
- Deterministic native archive production, byte-identity tests, strict read-back
  verification and a clean-room smoke test on macOS ARM64.
- A fail-closed four-host archive workflow with SPDX SBOM, keyless signatures,
  GitHub provenance and isolated draft verification.
- Clean-room installation of both Debian packages, the measured Homebrew
  formula on all four native hosts, and the AUR package on x86-64 and ARM64.
- The Astro documentation build and GitHub Pages deployment at
  `https://metaneutrons.github.io/aros-tools/`.

## Still required before 1.0

- Signed APT repository publication and rollback-safe R2 staging.
- Credential-isolated publication to Homebrew and AUR followed by public
  installation verification.
- SBOM, provenance, signature and checksum verification for the first tagged
  release artifact set.
- A fully qualified GNU Make backend for pristine upstream product builds.
