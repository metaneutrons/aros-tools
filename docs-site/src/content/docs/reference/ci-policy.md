---
title: CI policy
description: How AROS tools selects native host coverage without weakening executable-change qualification.
---

## Default pull-request coverage

Every pull request runs commit hygiene, the Linux x86-64 test job, the workspace
quality gate and the documentation build. The Linux job keeps its stable check
name, so the protected branch always has one portable test result to evaluate.

Only these paths are documentation-only:

- `README.md`
- `CONTRIBUTING.md`
- `docs/**`
- `docs-site/**`

When every changed path is in that set, CI runs the Linux portable suite. An
empty, malformed, duplicate or otherwise unclassified path list is not treated
as documentation-only.

## Native host matrix

Any change outside the documentation-only set runs the complete native matrix:

| Host | Runner |
| --- | --- |
| Linux x86-64 | `ubuntu-24.04` |
| Linux AArch64 | `ubuntu-24.04-arm` |
| macOS x86-64 | `macos-15-intel` |
| macOS AArch64 | `macos-15` |

That includes Rust crates, Cargo inputs, scripts, workflow definitions,
contracts, package metadata and unknown paths. Linux x86-64 uses the exact
qualified AROS-NX source; the other hosts run the closed portable suite.

After integration into `main`, the Linux lane also runs the compatible CMake
fixtures. This is the integrated product checkpoint, not a substitute for the
native matrix. The GRUB fixture is available only on Darwin/AArch64.

## Explicit and release qualification

**Workspace CI → Run workflow** defaults to the full matrix. Its `fast` option
is an explicit maintainer choice for a Linux source-coupled checkpoint. A
weekly full matrix detects runner and toolchain drift.

Release qualification does not run on ordinary pull requests. It runs for an
immutable version tag or an explicit **Release qualification → Run workflow**
dispatch. Release promotion still requires its independent artifact, package,
provenance and compatibility gates; a green CI matrix never publishes a
release.

For the local equivalents and milestone acceptance requirements, see the
[development workflow](/aros-tools/contributing/development/) and
[CONTRIBUTING.md](https://github.com/metaneutrons/aros-tools/blob/main/CONTRIBUTING.md).
