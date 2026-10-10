# Compiler-family local index stages

`aros toolchain producer index` selects `legacy-v1` by default. Existing LLVM
release commands retain their source-lock argument and byte formats. Explicit
`--release-format family-v2` selects the mixed-compiler local boundary described
here. The format number is not release SemVer or a compiler version.

Both formats require `--directory`, `--release-id`, `--base-url` and `--stage`.
V1 additionally requires `--source-lock-filename`; it rejects V2-only arguments.
V2 requires `--lane-inputs` and `--subject-manifest`, rejects the V1 source-lock
argument, and loads the fixed `toolchain-release-inputs-v2.json` collection from
the release stage. The collection selects each group's bound recipe, source
lock and profiles. Every active host/profile lane is required.

## Independent lane inputs

The lane file is an absolute, regular, non-hardlinked metadata file outside
the release directory, limited to 16 MiB. Its closed JSON shape is:

```json
{
  "schema": "aros-toolchain-index-lanes-v1",
  "lanes": {
    "canonical-archive-name.tar.xz": {
      "build_environment": {},
      "required_paths": ["declared/payload/path"]
    }
  }
}
```

The example describes fields, not a valid release candidate. Actual archive
names come from the selected input collection. Both independent maps must cover
that exact set: no missing or extra lanes. Environments come from measured
build expectations, not manifests extracted from the packages being checked.
Required paths come from the reviewed producer contract and must be sorted,
safe and present. Duplicate object keys are rejected at every JSON nesting
level, including lane names and open-ended environment objects. Unknown schema
fields and trailing input fail. Existing library depth, aggregate environment,
required-path and forbidden-prefix budgets apply before package verification.

Repeat `--forbidden-prefix` for absolute build roots that must not occur in
archive payloads. The file map itself is an input, not a release asset.

## Pre-attestation

`--stage pre-attestation` requires a complete pre-index inventory. Index,
provenance and final checksums must be absent. The command measures all packages,
exclusively writes the canonical index, then writes the external subject
manifest. The manifest output must be absent, absolute and outside the release
directory, with existing real directory ancestors. Existing outputs are never
replaced. `--subject-manifest-sha256` is invalid at this stage.

The JSON result (`--format json`) uses `aros-toolchain-producer-stage-v2` and
reports the index hash, exact external subject-list hash/size, lane count,
subject count and derived final inventory count. It does not create
`SHA256SUMS` or authenticate anything. Retain that exact subject-list digest
with the independent signing/attestation result.

## Final

After the external workflow authenticates the selected subjects and supplies
the provenance bundle, `--stage final` additionally requires
`--subject-manifest-sha256` with the exact lowercase digest reported by
pre-attestation. The command rebinds the existing index to the release ID,
base URL, input collection and independent lane maps, verifies the unchanged
subject list including its expected digest, exclusively writes final checksums,
then repeats complete package, checksum and manifest readback. Provenance must
be present and checksums absent before finalization; an existing final stage is
not silently rewritten.

The result additionally measures `checksums_sha256`, `provenance_sha256` and
provenance size. These are byte identities, not cryptographic authentication.
The subject list excludes provenance and final checksums; final checksums cover
provenance and every other final inventory member except themselves.

## Failure and authority

The caller owns a quiescent stage. These operations provide no lock or snapshot
against concurrent writers. Index and external manifest are two exclusive
outputs, not an atomic multi-file transaction: a later failure retains earlier
outputs for diagnosis. A retry needs a fresh stage and output, never deletion
or replacement by this command. No successful result is emitted on failure.

This local CLI cannot tag, publish, execute A/B compiler builds, authenticate
Sigstore/GitHub attestations or admit a release. V2 qualification-evidence,
recovery and protected release-workflow integration remain separate work.
