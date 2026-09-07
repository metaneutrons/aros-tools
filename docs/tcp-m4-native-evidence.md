# TCP-M4 native package evidence

Status: accepted on 2026-09-08. This record covers synthetic package creation
and verification only. It is not a compiler build, a release qualification, an
attestation, an A/B comparison, or a hardware boot result.

## Fixed inputs

- Integrated aros-tools source: [`0fa0a37375afc00aafe2c848f1a2a58896d05512`](https://github.com/metaneutrons/aros-tools/commit/0fa0a37375afc00aafe2c848f1a2a58896d05512).
- Qualified AROS-NX source contract: `f3cfc243a84065166a46da28b0a5b22bbd0f8869`.
- Qualified producer contract: `metaneutrons/aros-toolchains` at
  `c8039cf2b7291097ad62c6750bd7367e91a068f4`.
- Canonical manifest-schema fixture SHA-256:
  `78344aa68113675107d954515029dc87f5a3d79fb92e87f142d5d8427bb9933c`.
- Independent historical PAX/XZ package fixture SHA-256:
  `542639b0a05230d4260976c7418c3d241e42c8aaddcccf4b9ac3d3bc295b7b57`.
  Its embedded archive is 848 bytes with SHA-256
  `555c7ff6340d9ddfc704e8a1a64f3b8a88d41afe444bfc3972b6bae532b2f01c`.

## Delivered boundary

The native package engine owns normalized tree inventory, deterministic
`.tar.xz` creation, package manifests, checksum sidecars, SBOM handling and
bounded read-back verification. The release-index verifier accepts only a
complete v1 4-by-3 matrix: twelve package sets and their 56 required files.
Each package is read back through the native archive verifier before the index
is emitted.

The implementation was integrated in dependency order:

- [PR #66](https://github.com/metaneutrons/aros-tools/pull/66), canonical
  payload inventory (`0f7803ed42510ec3015313af14c95817b2661177`);
- [PR #73](https://github.com/metaneutrons/aros-tools/pull/73), deterministic
  native package writer (`7eac504d4ba6fc3678cdf0e1aaebe83f9a4ae262`);
- [PR #71](https://github.com/metaneutrons/aros-tools/pull/71), native package
  read-back (`813c89f8fe4d1468856e562c6df7d4b33103d79b`); and
- [PR #72](https://github.com/metaneutrons/aros-tools/pull/72), complete
  release-inventory read-back (`0fa0a37375afc00aafe2c848f1a2a58896d05512`).

The project requires squash merges. The final PR #72 source tree was compared
byte-for-byte with its integrated `main` tree before this record was written.

## Verification results

The final full-host qualification was [Workspace CI run
34164982544](https://github.com/metaneutrons/aros-tools/actions/runs/34164982544)
on the exact final source tree. All required gates passed.

| Gate | Result |
| --- | --- |
| Commit hygiene | passed in 6 seconds |
| Formatting, architecture and Clippy | passed in 7:11 |
| Linux x86-64 | passed in 6:35 |
| Linux ARM64 | passed in 3:25 |
| macOS ARM64 | passed in 6:51 |
| macOS x86-64 | passed in 11:20 |

The integrated-main checkpoint, [Workspace CI run
34165692830](https://github.com/metaneutrons/aros-tools/actions/runs/34165692830),
then passed against commit `0fa0a37375afc00aafe2c848f1a2a58896d05512`:
formatting, architecture and Clippy in 7:26, and Linux x86-64 in 8:36. Main
uses the deliberately inexpensive single-host integration policy; it does not
replace the preceding full four-host M4 qualification.

The test suite exercises the full native twelve-package matrix, exact static
schema and support-file bytes, producer and consumer known-answer vectors,
archive identity and no-follow handling, missing or mismatched sidecars and
SBOMs, unsafe names, case-fold collisions, special archive members, malformed
XZ data, and required payload-path completeness. In particular, the release
index cannot merely count files: it invokes bounded package read-back for each
package set before accepting the index.

## Explicitly outside M4

- No real compiler or GRUB build was performed for this package-only change.
- No release tag, draft, publication, attestation, isolated download, or
  double-build A/B matrix was created.
- Compatibility, replay and recovery remain TCP-M5; workflow cutover and
  maintainer-facing CLI commands remain TCP-M6.

Rollback is a normal source rollback of the four integration commits above.
It must not reuse a rejected archive, package manifest or release index as
evidence for a later candidate.
