---
title: Release status
description: Claims that are green and gates that remain open.
---

## Verified

- Complete Rust workspace tests and architecture boundaries on the extracted history.
- No Claude or Anthropic attribution in the rewritten `aros-tools` history or source tree.
- Translation and differential verification against the immutable AROS-NX
  source contract selected by CI.
- A complete local AROS-NX `pc-x86_64` product build using the published RC3 cross-toolchain.
- Deterministic native archive production, byte-identity tests, strict read-back
  verification and a clean-room smoke test on macOS ARM64.
- A fail-closed four-host archive workflow with SPDX SBOM, keyless signatures,
  GitHub provenance and isolated draft verification.

## Still required before 1.0

- Clean-room installation tests for archives, Debian packages, Homebrew and AUR.
- Signed APT repository publication and rollback-safe R2 staging.
- SBOM, provenance, signature and checksum verification for every release artifact.
- A fully qualified GNU Make backend for pristine upstream product builds.
