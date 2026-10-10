# Compiler-family final checksums v2

`aros_toolchain::release_checksums_v2::verify_final_checksums_v2` verifies a
complete local final inventory against a validated release index. It writes
nothing and returns no partial result.

`SHA256SUMS` must cover every index-derived member except itself, including
the provenance bundle. Each line contains a lowercase SHA-256, two ASCII
spaces, the exact flat filename and LF. Complete lines are lexically sorted,
matching the v1 checksum convention, with exactly one final LF. Duplicate,
missing, extra, reordered or differently encoded lines fail byte comparison
against independently measured canonical checksums.

All ancestors must be real directories. Inventory members must be regular
files, not symlinks or directories. Archive streams are bounded by the same
32-GiB limit as the index; other metadata streams are bounded to 16 MiB.
Measured archive digests and lengths must match the bound index even if a
changed archive is accompanied by a consistently rewritten checksum file.
The on-disk index must match the exact canonical bytes serialized from the
supplied validated index. A rehashed substituted index, or different JSON
formatting, cannot bypass that binding.

The returned members are filename-sorted measured identities; their order is
not the checksum document's full-line order. The checksum document digest is
measured from the exact accepted bytes. Exact inventory and directory checks
run before and after measurement, and the checksum document is read again.
These observations do not establish a stable snapshot against concurrent
writers or replaced parents; no ownership lock is acquired.

This boundary does not parse or authenticate provenance, verify package
contents, bind input documents, execute compilers, attest qualification, write
an index/checksum file, revalidate the full input-document collection, or publish
anything. Indexed package read-back remains
a separate operation. A correct hash for a provenance bundle does not establish
a valid signature or trusted signer. The [explicit family-v2 index CLI
stages](compiler-family-index-cli-v2.md) use this local boundary; protected
release workflow and qualification/recovery integration remain separate.

## Exclusive local checksum output

The earlier [attestation subject manifest](compiler-family-attestation-manifest-v2.md)
has a separate identity and remains outside the release directory. It excludes
both final checksums and provenance; final checksums include the provenance
added after external attestation. The manifest must remain byte-identical across
that boundary.

On Unix, `release_checksums_v2_writer::write_final_checksums_v2` accepts an
`IndexedPackageReadbackRequestV2` over the complete final stage minus
`SHA256SUMS`. The provenance bundle must already be present. Before reserving
output it verifies the bound inputs, runtime support bytes, canonical index,
all packages, independent environments, required paths and forbidden prefixes.
It then measures every remaining member and constructs the same canonical
checksum document as the verifier.

The writer creates `SHA256SUMS` exclusively relative to a held no-follow
directory descriptor. Existing files, links and other destinations fail;
nothing is replaced. After file/directory synchronization it independently
verifies final checksums, repeats complete indexed-package read-back, and
checks the output's exact bytes, descriptor identity and directory identity.
Success adds only the checksum file and reports its size, digest and complete
checksum read-back. A failure after reservation retains diagnostic output;
retry requires a fresh stage. The caller must own a quiescent stage.

This API does not authenticate the provenance bundle, execute compilers or
admit publication. The explicit family-v2 final index CLI stage uses this writer.
Protected workflow integration, A/B, compatibility, relocation,
signature/provenance and independent release-download gates remain required.
