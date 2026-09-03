---
title: Release status
description: Current source state, published state, and remaining public-release gates.
---

## Current source state

The repository contains and continuously tests:

- complete Rust workspace, architecture, formatting, Clippy, Rustdoc,
  dependency-advisory and license/source-policy gates;
- an explicit AROS-NX/toolchain-producer source contract rather than a moving
  branch or sibling-checkout assumption;
- fail-closed diagnostics and transactional output for every shipped host
  executable;
- deterministic archive production, duplicate-production comparison, strict
  read-back, clean-room smoke tests, SPDX validation, signatures, provenance
  and isolated-draft verification in the release workflow; and
- Debian, signed APT, Homebrew and AUR qualification paths that consume the
  measured canonical archive payloads rather than rebuilding them.

Those are source and workflow capabilities. A workflow definition is not proof
that a particular public release passed it.

## Published state

No stable `aros-tools` release has been published yet. Consequently there is
no supported public archive, Homebrew formula, APT package or AUR package to
claim. Build from source until a release and its measured evidence appear on
GitHub. Documentation is configured for
`https://aros.metaneutrons.cc/aros-tools/`; availability at that URL depends on
the Pages deployment and DNS state.

## Required for the first public release

- Merge a Release Please version pull request, then create a separately
  protected annotated tag on that exact qualified `main` commit.
- Pass the complete four-host archive, ABI-floor, SBOM, checksum, signature,
  provenance, isolated-download and clean-room verification matrix for that
  immutable tag.
- For a stable candidate, pass the credential-free APT, Homebrew and AUR
  qualification gates before the one-time final GitHub publication. Then roll
  the exact immutable release bytes forward to signed APT/R2, protected
  Homebrew and AUR and verify every public channel.
- Complete qualification of the remaining source compatibility and
  released-toolchain-selection boundary before claiming translated product
  builds from an entirely pristine upstream checkout as generally supported.

The page is intentionally conservative: it records no release as verified
until the immutable release inventory itself supplies the evidence.
