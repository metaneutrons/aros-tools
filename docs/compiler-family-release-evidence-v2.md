# Complete compiler-family evidence read-back

`aros toolchain producer verify-release-evidence` reads a **final family-v2**
release and the complete separately retained A/B and compatibility evidence.
It writes nothing, runs no compiler and never opens original runner roots.
Success means `assurance: byte-consistency-only`, not execution authentication,
qualification, signature verification or permission to publish.

The selected release-input collection is the matrix authority. Every indexed
archive needs two complete package/build exports, one comparison and one
complete compatibility export. A diagnostic subset, extra lane or mixed V1
release is rejected. The collector checks every package, final checksum member
and unchanged pre-attestation subject before returning any successful result.

## Independent selections

Supply the canonical absolute final directory, expected release ID and download
base URL. Retain raw digests for the release-input collection, index, subject
manifest and selection document independently of the files being checked.
Do not compute a new expected digest from an untrusted download to bypass a
failed check. The byte reader does not authenticate how these digests were
obtained; that remains the protected workflow's responsibility.

The external selection is bounded to 16 MiB and uses the closed JSON schema
`aros-toolchain-release-evidence-selection-v2`:

| Field | Selection |
| --- | --- |
| `schema` | Exact schema above |
| `lanes` | Map keyed by every exact index archive basename |
| `lanes[asset].build_environment` | Independently retained host-environment object, not copied from a package manifest |
| `lanes[asset].required_paths` | Reviewed required paths; must equal the indexed lane's paths |
| `lanes[asset].builds` | Exactly two records in A/B order |
| `builds[n].package_dir` | Canonical directory containing exactly the four downloaded package members |
| `builds[n].measurement` | `{ "path": ABSOLUTE_FILE, "sha256": RAW_DIGEST }` for the complete owning-host build export |
| `builds[n].executor` | Independently selected `contract_sha256` and `binary_sha256` |
| `lanes[asset].comparison` | Selected-file record for the exact retained comparison bytes |
| `lanes[asset].compatibility.directory` | Canonical closed `evidence/` directory from the owning-host export |
| `lanes[asset].compatibility.inputs` | Selected-file record for the separate pre-execution `inputs.json` |
| `lanes[asset].compatibility.manifest_sha256` | Independently retained raw compatibility manifest digest |

Unknown or duplicate object keys are rejected at every nesting depth, including
environment objects. Selected paths must be absolute and free of parent
traversal. Package/evidence roots must be distinct and nonoverlapping. The
selection, compatibility input observations and subject list remain outside
the final public inventory; the first two also remain outside every selected
package and compatibility root. Filesystem identities guard containment against
case and Unicode aliases. All files must be quiescent for the operation;
repeated observations are not a concurrent-writer snapshot.

Build source, producer, tools, host and profile expectations come from the
selected release inputs, not the downloaded build export. Compatibility input
claims are rebound to actually verified indexed archives and selected profiles
before the complete report/log/ELF join. The collector rereads selection,
index, collection and compatibility input bytes before reporting success.

## Result and release boundary

With `--format json`, the result contains the selected input/index/selection
digests, measured final checksum and subject-manifest digests, exact lane/build
counts, and each lane's build-result, export, finished-record, comparison,
compatibility input/receipt/manifest and standalone ELF measurements. Finished-record
self-digests are labeled separately from raw file digests.

Every lane is returned together or the operation fails. A rehashed report is
not enough to admit changed package, log or ELF bytes. A valid selection still
does not prove independent A/B execution, original environment authenticity,
relocation execution, signer identity or owning-job/artifact provenance.

The protected producer must authenticate the runtime, repository, workflow,
run attempt, owning jobs and exact artifact bytes separately, bind the selection
to those verified observations, and require V2 qualification/recovery admission.
Do not use this command's output as a `verified` flag for publication. Historical
V1 recovery remains a separate operation.

## Join qualification claims to actual bytes

`aros toolchain producer verify-qualification` accepts the same complete
selection arguments as `verify-release-evidence`, plus:

| Required argument | Independently selected input |
| --- | --- |
| `--qualification-evidence` | Closed `aros-toolchain-qualification-evidence-v2` document |
| `--qualification-sha256` | Raw digest of that exact document |
| `--policy` | Closed repository/workflow/signer/epoch `EvidencePolicy` document |
| `--policy-sha256` | Raw digest of that exact policy document |

Qualification and policy files must remain outside every selected release,
package and compatibility root. The supplied index bytes must match the exact
canonical index verified in the final inventory; semantically equivalent JSON
with a different raw digest is rejected.

Only complete `release-candidate` coverage is accepted. The operation acquires
all selected evidence itself: the two build-report claims must equal the raw
complete finished-package export digests; the comparison claim must equal the
raw comparison file digest; the compatibility claim must equal the raw portable
manifest digest covering every receipt, report, log and ELF. A receipt's
self-digest cannot replace any of these raw file hashes. Final checksum,
pre-attestation subject-list and provenance-bundle claims must equal measured
bytes. `source_run.run_attempt` is mandatory and positive; it is still a claim
until the external verifier binds it to provider/certificate observations.

The command writes nothing. JSON remains `assurance: byte-consistency-only`
and adds the selected qualification/policy document digests. It does not call
GitHub, verify signatures, prove independent execution or authorize recovery
or publication. A complete, internally consistent forged evidence set still
requires rejection by the independent authentication boundary.

## Record complete qualification claims

`aros toolchain producer record-qualification --release-format family-v2`
uses the same complete byte collector. Select `--release-dir`, independent
input/index/selection/subject digests and paths, and the explicit source-run
repository, workflow, ID, **attempt**, tag object and peeled producer commit.
The selected index digest binds release ID and download URL; the command does
not accept a caller's JSON summary as evidence.

Supply attestation repository/workflow/signer claims, `--created-at`,
`--expires-at` and an absent absolute `--output` outside all release, package,
compatibility and input paths. Its real parent must already exist without
symlink components. Only after every lane and final checksum member passes
does one atomic no-clobber write expose the complete claim file. Existing
outputs remain unchanged. JSON reports its raw digest and
`assurance: byte-consistency-only`.
The durable publisher may retain an empty advisory lock in the output parent;
keep that coordination file outside the release inventory.

Creation and expiry must be positive and ordered. Source-run and signer values
remain external claims: recording does not verify provider execution, tag
identity or signatures. Rechecking the recorded file with
`verify-qualification` still requires a separately selected policy and raw
digest; it does not grant recovery or publication authority.

Omitting `--release-format` retains historical V1 recording. Explicit
`legacy-v1` uses that same layout. V1 source-lock/report-root flags cannot be
combined with the V2 selection; V2 requires every independent digest and the
positive run attempt. There is no automatic schema fallback.
