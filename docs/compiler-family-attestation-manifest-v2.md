# Compiler-family attestation subjects v2

The Unix `release_attestation_manifest_v2` library writes and verifies a measured
subject list after the V2 index has been written. This is a local byte-binding
boundary, not signature verification or permission to publish.

## Separate identities

The subject manifest covers the index-derived inventory except `SHA256SUMS`
and `toolchain-provenance.sigstore.json`. Those two files do not yet exist at
the pre-attestation boundary. The manifest itself is neither a release asset
nor an attestation subject: its absolute path must be outside the release
directory, under real directory ancestors.

Each line uses the same canonical encoding as final checksums: lowercase
SHA-256, two spaces, flat filename and LF; complete lines are lexically sorted.
Every listed file is independently streamed, with archive digests and sizes
bound to the index and on-disk index bytes bound to canonical serialization.
Returned member observations are filename-sorted, not full-line-sorted.

For two groups with three LLVM profiles and one GNU RV32 profile on the three
active hosts, the derived final inventory has 60 assets. The pre-attestation
manifest selects 58 subjects. Final `SHA256SUMS` covers 59 files, including the
later provenance bundle. None of these counts is a fixed selection rule: every
inventory is derived from validated inputs and index artifacts.

## Stage order

1. Verify the complete pre-index stage and write its canonical V2 index.
2. Call `write_attestation_manifest_v2` with the indexed package-readback
   request and one absent external manifest path. The release directory must
   contain exactly the indexed inventory minus final checksums and provenance.
3. An external verifier/signing workflow consumes the selected subjects and
   authenticates the actual Sigstore/GitHub identities. Retain the exact manifest
   bytes and their measured SHA-256 with that independent result.
4. Add only the provenance bundle to the release directory. Verify the unchanged
   manifest with `AttestationManifestStageV2::PreChecksums`, then write final
   checksums through `write_final_checksums_v2`.
5. Verify the same manifest with `AttestationManifestStageV2::Final`. This checks
   the complete final inventory, all packages, every unchanged subject and
   final checksum coverage, including the provenance file.

`PreAttestation`, `PreChecksums` and `Final` select exact inventories; an extra
file or a missing stage member is an error. At every boundary the package reader
verifies bound input/support/index documents, independent build environments,
required paths and forbidden prefixes. Changing a valid package and rehashing
the manifest cannot bypass the validated index identity.

## Output and verification guarantees

The writer validates the stage before reserving output. Creation is exclusive
and descriptor-relative, without following links. Existing regular files,
symlinks, directories, FIFOs or raced outputs are never replaced. File and
parent synchronization precede independent manifest and package read-back.
Manifest reads reject hardlinks, non-regular files and oversized documents.
The writer and verifier check file bytes and descriptor/directory identities
again before returning.

The caller must own a quiescent stage. Observations do not provide a lock or a
stable snapshot against concurrent writers. Failure after output reservation
retains diagnostic output; retry needs a fresh output, not replacement.

The manifest digest binds the list given to external attestation. It is distinct
from the digest of final `SHA256SUMS`. Neither this API nor the V2 evidence parser
authenticates a signer or infers a compiler, A/B, compatibility, relocation or
application execution result. The [local index CLI](compiler-family-index-cli-v2.md)
connects these byte boundaries; release-workflow and recovery integration remain
separate work.
