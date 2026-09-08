# TCP-M5 qualification-evidence contract

`aros-toolchain-qualification-evidence-v1` is the closed, offline record that
binds a candidate release index to the inputs and reports required for native
compatibility, replay and packaging-only recovery. It is a library contract,
not a public CLI command, a release format extension or publication evidence.

The implementation lives in `aros-toolchain::qualification_evidence`. It has no
network, credential, GitHub API, tag, draft or release authority. A protected
workflow obtains GitHub attestation claims through the established verifier;
the Rust library validates their policy binding and the exact measured release
index an operation is permitted to use. Full outer-asset measurement is added
with the replay/recovery operation.

## Closed record

The document has these top-level fields:

| Field | Rule |
| --- | --- |
| `schema` | Exactly `aros-toolchain-qualification-evidence-v1`. |
| `created_at`, `expires_at` | Unsigned Unix epochs; expiry is strictly later than creation and is checked against an explicit validation time. |
| `source_run` | Credential-free producer repository, repository-relative workflow, nonzero immutable run ID, exact producer/source commits, immutable source tag and annotated tag object. |
| `release` | Release ID/base URL, recipe/lock/profiles/source/producer/tools identities and SHA-256 values for the exact index, final checksums and provenance bundle. |
| `attestation` | Repository, workflow, signer and checksum-subject claims returned by an external verifier. The claim subject must equal `release.checksums_sha256`. |
| `lanes` | Native build-A, build-B, byte-comparison and compatibility-report digest for each host/profile lane. |
| `coverage` | `diagnostic` for one or more bounded lanes, or `release-candidate` for the exact closed 4-by-3 v1 matrix. |

Unknown or duplicate fields, malformed digests/Git IDs, unsafe repository or
workflow names, duplicate lanes/report digests and incomplete
`release-candidate` coverage fail with `AX0901`.

## Binding and trust boundary

`validate_against_index` accepts the exact serialized release-index bytes and
an explicit policy containing expected producer/signer repository, workflow,
signer identity and validation time. It performs all of the following before a
later operation can use the evidence:

1. measures the supplied bytes and requires `release_index_sha256`;
2. parses the closed native v1 index and requires release/source/producer/tools
   identities, base URL and every selected lane to agree;
3. rejects expired evidence and mismatched source/signer policy claims; and
4. requires every lane target triple to match the measured index.

This does **not** turn a JSON claim into a cryptographic attestation. M5
replay/recovery will additionally measure every outer regular release asset,
verify the v1 package/index relationship and consume claims only after the
workflow verifier has checked repository, signer, workflow, source revision and
subject policy. Missing, changed or non-regular assets are therefore rejected
at the filesystem boundary, not trusted because an evidence document names
them.

## Recovery eligibility

Diagnostic coverage supports local compatibility evidence only. It cannot
authorize replay or repackage. A later packaging-only recovery requires
`release-candidate` coverage, all twelve exact lanes, independently recorded
build-A/build-B, comparison and compatibility reports, a valid final index and
separately verified attestation claims. The recovery policy also verifies that
the original failure was packaging alone; this record does not bypass compiler,
comparison or compatibility failures.

No field permits overwriting an artifact, moving a tag, creating a release,
or using a credential. Those decisions stay in protected workflow code.

## Compatibility preparation

TCP-M5.2 begins with a separate, offline preparation boundary. It creates the
embedded `aros-cmake-engine` below a fresh owned work root and reads every
materialized file back against the compiled-in resource and digest. The probe
source is explicitly engine-free: a top-level `cmake/` file, directory or link
fails. Each required helper (`aros-transpiler`, `aros-genmodule`,
`aros-collect`, `aros-ahi-runner`, `aros-fetch`) must be an executable regular
file directly below one explicit fresh target root and is measured before later
process phases may use it. It does not run a process or accept a shared Cargo
target directory as origin evidence.

### Relocation package roots

The native producer has a separate package-root boundary for the later
two-root relocation probe. For each root it first verifies one complete native
package set in place: the archive, external manifest, checksum sidecar and
SBOM must be the exact closed four-member set. It then creates one absent,
owned destination, remeasures the archive through a no-follow descriptor, and
requires that its measured size and SHA-256 still equal the just-verified
package identity. The same bounded tar parser used by package verification
streams that descriptor into the fresh root; it rechecks canonical headers,
ordering, paths, links, resource limits and forbidden producer prefixes.

The extracted manifest and complete tree inventory are read back before the
root is accepted. A pre-existing output is never adopted or overwritten. If
creation has begun and a later check fails, the root and its partial material
remain for diagnosis. This is producer-owned compatibility preparation, not
the `aros-cli` installation path. A future two-root probe invokes it twice
with independently owned destinations; it must not reuse one extracted tree.

## Compatibility probe reports

The next M5.2 boundary records one completed process per closed phase:
consumer CMake configuration, pristine upstream configuration, upstream
includes, upstream link libraries, standalone C and standalone C++. A caller
supplies an absolute executable, explicit UTF-8 argument vector, real working
directory, fresh report directory and positive deadline. The runner neither
looks up `PATH` nor evaluates a shell command.

Before a process starts, it revalidates the materialized embedded engine and
all five helper file identities. It then binds the report to their API version,
digest and measured helper hashes. Standard output and error are captured with
the common bounded process supervisor and durably published under fixed,
phase-specific no-clobber names. Successful reports use the closed
`aros-toolchain-compatibility-report-v1` JSON schema and bind the executable,
argument identity and both persisted log hashes.

A nonzero exit, timeout or cancellation never yields a success report. Where a
process started, its durable logs remain available for diagnosis. Existing
report outputs are rejected rather than adopted or overwritten. This is still
the reusable process/report boundary; the following M5 work supplies the
concrete CMake, upstream, standalone and relocation commands.
