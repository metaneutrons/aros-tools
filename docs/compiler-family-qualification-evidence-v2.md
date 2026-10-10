# Compiler-family qualification evidence v2

`QualificationEvidenceV2` is a pure contract for claims about a compiler-family
release candidate. It is separately versioned; the v1 parser, fixed matrix and
recovery admission remain unchanged.

The closed JSON schema is `aros-toolchain-qualification-evidence-v2`. Parsing is
limited to 16 MiB. It rejects unknown or duplicate fields, unsafe repository,
workflow or asset identities, empty/unsorted/duplicate lanes and reused report
digests. Evidence has a nonzero creation time and a later expiry time.

`validate_against_index` takes exact index bytes, a validated release-inputs
collection and an explicit verifier policy. It checks the collection/index
digests, repository/workflow/signer claims and validation time. Each asset-sorted
lane must match its input-derived archive, group, host, profile, target triple,
compiler/ABI, source commit, recipe self-digest, source-lock and profiles
digests, archive hash/length and payload tree digest.

Source commits belong to input groups, not to the release as a whole. RV32 and
RV64 may therefore use different qualified source revisions without weakening
the binding of either lane. The matrix comes from the validated input/index
pair, not from a Rust board list. Release-candidate coverage requires every
indexed lane exactly once. Diagnostic coverage permits nonempty subsets; a
subset does not become a qualified release candidate.

The record names four report digests per lane: build A, build B, byte comparison
and compatibility/relocation. These are claims, not independently measured
report contents. This contract does not execute a compiler, measure an archive,
authenticate an attestation, or validate a native compatibility receipt. The
caller must separately verify package/report bytes and cryptographic evidence.
The [measured subject-manifest library](compiler-family-attestation-manifest-v2.md)
provides the distinct pre-attestation byte identity. The V2 attestation claim
binds the digest of the pre-attestation checksum
manifest used by the external workflow to select individual subject files. It
does not claim that GitHub attests the manifest file itself: the attested
subjects are the individual files selected from that manifest. The final
`SHA256SUMS` digest is a separate release identity and may include the
provenance bundle added after attestation. The pure parser only checks that the
claim's manifest digest equals `pre_attestation_checksums_sha256`; it performs
no cryptographic verification. The caller must separately authenticate the
attestation and read back the checksum and provenance files.

`PackageComparisonReport::parse` provides a bounded, closed parser for the
four-member comparison declaration. It checks member ordering, exact filenames
and the canonical package-set digest and rejects duplicate or unknown JSON
fields. It does not measure those members or establish independent builds.
`validate_against_checksums_v2` revalidates the report, binds the selected indexed
archive and joins all four names, hashes and sizes with an independently
measured `FinalChecksumsReadbackV2`. Manifest, SHA sidecar and SBOM claims must
match too. This join does not reread files, create a stable snapshot, verify
package semantics, authenticate the report's origin or prove independent builds.
The caller still has to bind the raw report digest and enforce those separate
requirements; the legacy CLI does not yet use this v2 join.

This boundary adds no CLI, recovery or publication authority. The current v1
qualification/recovery CLI does not accept a v2 evidence record, GNU v3
compatibility receipt or LLVM family-v2 v4 compatibility receipt. Report
byte read-back has a separate bounded library API documented in
[compiler-family compatibility](compiler-family-compatibility.md#retained-receipt-read-back).
Binding that API to independently verified release inputs, versioned recovery
and protected workflow integration remains necessary for
[RV4 release admission](riscv-board-integration-plan.md#rv4-immutable-toolchain-releases).
