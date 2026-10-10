# Compiler-family release package read-back v2

`readback_indexed_packages` is a local, read-only boundary over a complete
compiler-family release directory. It accepts already validated
`ReleaseInputs` and `NativeReleaseIndexV2` values, rebinds the index to those
inputs, and returns packages only after every indexed archive has passed
bounded native package verification.

The caller supplies build-environment maps separately, keyed by each
canonical archive basename. The map must contain exactly the indexed archive
names. The read-back boundary never derives or copies those receipts from
archive contents.

## Filesystem and input checks

The release directory must be absolute, contain no `.` or `..` components,
and have only real directory ancestors. Before and after package verification,
the implementation requires the complete flat
`NativeReleaseIndexV2::expected_inventory()` to consist of regular files,
with no missing, extra, directory, or symlink members.

Bounded no-follow reads are limited to 16 MiB. The on-disk release-input
collection must hash to the exact collection digest in `ReleaseInputs`; every
referenced recipe, source-lock and profiles document must equal its bound raw
bytes. The on-disk index must parse against those inputs and equal the supplied
validated index.

The manifest-v2 schema and tree-digest fixture must match the exact bytes
embedded in this tools runtime, before and after package read-back. This binds
the support files; it is not a separate JSON Schema engine or a compiler proof.

For every artifact, the implementation resolves its group and profile from
the validated inputs and invokes the strict `CompilerFamilyV2` package
verifier. It checks the measured archive SHA-256 and size, payload tree digest,
schema-v2 manifest identity, compiler, host, profile, target triple, source
commit and release ID against the index. The package verifier also checks the
archive structure and payload inventory, embedded and external manifest
identity, checksum sidecar, SPDX document, and forbidden build-root content.
Every payload member must have a previously declared real directory parent;
children below files or symlinks and omitted parent entries are rejected before
inventory acceptance. Forbidden roots must be absolute UTF-8 paths, not silently
skipped when a path cannot be represented by the content-scanning contract.
Every indexed required path must be a file or symlink in the verified payload
inventory, except `toolchain-manifest.json`, whose embedded copy is checked by
the package verifier.

The returned package vector is asset-sorted and contains only complete results.
Any error returns a sanitized index-contract diagnostic; no partial vector is
exposed. The operation does not replace or write a release file.

## Scope and limits

Success means that every native archive and its package metadata passed this
read-back boundary against the supplied validated inputs and index. It does
not qualify the complete release for publication and exposes no readiness or
publication flag. Other inventory members are checked only for exact
regular-file presence, except that the release-input collection, its bound
documents and the index receive the checks described above. In particular,
this boundary does not verify `SHA256SUMS`, provenance signatures or
attestations, evidence,
recovery or CLI behavior, or release/publication qualification.

The directory ancestors and inventory are checked at entry and the inventory
is checked again at exit. There is not yet a directory-ownership lock, so
these checks do not provide a stable snapshot against a concurrent writer or
fully close parent-directory replacement races. No signature or attestation
is checked here. The historical v1 release path remains unchanged.
