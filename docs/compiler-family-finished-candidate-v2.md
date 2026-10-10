# Finished native-candidate evidence

The native lifecycle writes `native-lifecycle/receipts/finished-candidate.json`
after collector installation and durable local candidate publication. Its
schema is `aros-toolchain-finished-candidate-v2`. The six existing phase
receipts keep their independent V1 encoding. This is a local measurement
contract, not release admission or authenticated execution provenance.

## Complete finished tree

The record binds the exact recipe, source and producer revisions, selected
tools revision, frontend observation, host, profile, compiler identity, target
triple, source-lock/profile document hashes and six ordered phase digests. It
also records the raw candidate-content digest, descendant entry count and
total regular-file bytes. Measurement includes empty directories, original
descendant modes and symbolic-link target bytes; it does not follow links.
Root inode identity is a local snapshot precondition, not portable payload
identity. Root-directory mode is not part of the raw content digest.

Compiler checkpoint outputs precede collector normalization. LLVM intentionally
removes `llvm-config` and producer-only CMake metadata. The finished record
therefore measures the resulting tree rather than pretending all checkpoint
files must survive. `BuildResult.outputs` and the historical publish receipt
remain collector-file inventories. The result's new `finished-candidate`
evidence entry names the independently checkable complete-tree self-digest.

## Local read-back and package join

`native_candidate::readback_finished_candidate` requires the original explicit
work/output roots, validated recipe/lock/profile, expected native identity,
all six expected phase digests and the expected finished-record digest. These
expectations must come from separately retained evidence, not whichever
receipts are discovered. The reader rejects unknown/duplicate fields,
noncanonical JSON, missing/extra receipts, unsafe acquisition, changed
identities, roots, predecessor digests, collectors or complete payloads.

Acquisition uses no-follow descriptors and a 1 MiB bound per receipt. Payload
measurement is bounded to 499,998 descendants and 32 GiB minus 16 MiB of regular
bytes, reserving archive-root and generated-manifest headroom. The shared
traversal also limits every descendant path to 128 components. The reader
executes no commands and writes no files. It is a same-host/original-root
check, not portable recovery. Exclusive ownership is required while checking:
this is not an atomic transaction against arbitrary concurrent owner writes.

`native_candidate::package_finished_candidate` accepts only an opaque valid
read-back proof and exactly matching package inputs. Family-V2 packaging first
copies the complete raw tree from its bounded descriptor-measured snapshot
into a private raw staging directory. The guarded join uses the exact snapshot
retained in its read-back proof, never a newly accepted live-tree measurement.
Only that copy undergoes the existing
package transform: removal of `.DS_Store` and `.installflag-*` entries and the
old root manifest, portable path/link checks, and directory/file-mode
normalization. The four generated package members undergo bounded read-back
before the package directory is atomically committed. Candidate and receipt
bytes are revalidated immediately before that commit.

The resulting proof joins the finished-record/raw-candidate digests to the
verified archive and normalized package inventory. Raw candidate and package
tree digests use different encodings and transformations; they must not be
compared for equality. This proof is not a serialized transport attestation.
The caller's `build_environment` map is package metadata checked for read-back
consistency, not independently attested execution-environment evidence. The
final archive limits remain enforced independently before guarded publication.

## Local A/B package join

`native_candidate::compare_finished_candidate_packages` borrows two complete
candidate proofs and their guarded packages. It requires equal recipe, source,
producer, tools, host, profile and executor observations. Canonical work,
output and package roots must not overlap across A and B. Both packages are
reverified against their retained, independently selected package requests;
all four package members are compared byte for byte. Revalidation rejects
later changes to either payload, receipt chain or package member.

The optional final-checksum join also takes the exact validated release inputs
and index. It checks index membership, both candidates and manifests against
the selected group, and the measured final index and input-document bytes
before joining the four package members to final checksums. The checksum read-back is a retained
snapshot, not a fresh filesystem read or signature verification.

This is an original-host library boundary, not a new comparison CLI or portable
workflow attestation. Disjoint roots and matching bytes do not establish that
two independent compiler executions occurred. A remote collector still needs
authenticated, byte-bound execution evidence from both owning jobs.

## Portable package measurements

`export_finished_package_measurement` creates a closed
`aros-toolchain-finished-package-measurement-v2` document from a live finished
candidate and its guarded package. Both proofs are revalidated. The export
retains the exact UTF-8 bytes of the CLI build result, six ordered phase
receipts and finished record, plus measured names, sizes and hashes of all four
package members. The export is bounded to 16 MiB; each retained document keeps
its 1 MiB bound. Direct candidate proofs without a retained CLI result cannot
produce this export.

`readback_finished_package_measurement` takes an externally selected raw export
digest, independently selected lane/executor identity and package-verification
request. It acquires a bounded no-follow regular file, rejects duplicate or
unknown fields, checks the complete result/phase/finished chain, and verifies
the downloaded package against the selected inputs and environment. Published
collector hashes and sizes must also match the verified archive inventory. The
original host paths are checked lexically but never opened. A collector can
therefore read back this transport after the original build tree is unavailable.

The read-back does not reconstruct the owning-host proof or remeasure its raw
payload. Its raw tree identity remains an owning-host claim; the normalized
package tree is freshly verified. Neither self-digests nor a correctly rehashed
export authenticate who executed commands. Artifact origin, run attempt, owning
job, signer and A/B execution separation need independent external verification.
Retained receipts contain absolute paths: keep these exports in protected
workflow evidence artifacts, not in public package manifests or release assets.

## Complete release build read-back

`native_candidate::readback_release_builds_v2` consumes a validated release-input
collection and V2 index, the complete final release directory, independently
selected per-lane environments and exactly two build exports per indexed archive.
Each side also supplies an independently selected executor identity and raw
export digest. The retained comparison report and pre-attestation subject list
have separately selected raw file digests. Missing or extra lanes are rejected;
there is no diagnostic-subset mode.

Recipes, locks, profiles, hosts and release identities come from the selected
inputs and index, not from exported claims. Both downloaded four-member package
sets are verified against those inputs, freshly compared byte for byte, joined
to the complete closed comparison report and matched to all four final checksum
members. The original subject list must still match the measured final package,
input, index and support bytes. Evidence files remain outside the closed package
and release inventories. Collector package directories cannot overlap or be
reused, including distinct paths with the same device/inode identity. Evidence
files cannot be assigned to multiple selections, share a
device/inode identity or have multiple hard links.

The reader writes nothing and returns an opaque result only after all lanes
pass. It uses bounded no-follow metadata reads and repeats observations of
exports, comparisons, A/B package members, final checksums and subjects to reject
observed changes. It does not acquire a filesystem lock or promise an atomic
snapshot against concurrent writers. The caller must own quiescent inputs.

This is a library byte-acquisition boundary, not a qualification CLI or an
authenticated recovery route. Export and report digests must be selected by a
separate job/artifact verifier. Different collector directories do not prove
different compiler executions. Source run, run attempt, owning jobs, artifact
origin, compatibility logs/ELF outputs and signature verification remain
separate release gates. The reader never reopens the original host roots or
turns raw-tree claims into independently measured complete-tree proof.

## Portable compatibility evidence

`compatibility::export_retained_native_compatibility` revalidates a complete
compiler-family compatibility execution on its owning host and returns an
opaque flat file set. It retains the aggregate receipt, six exact phase
reports, every ordered stdout/stderr log and the actual standalone C/C++ ELF
objects. `compatibility-measurement.json` binds every member's exact size and
SHA-256; its raw digest must be retained separately by the job/artifact verifier.
Upload this closure as protected workflow evidence, not as public release assets.
Original receipt/log bytes are preserved, including private absolute paths.

`readback_portable_native_compatibility` takes a downloaded canonical directory,
that independently selected manifest digest and separately retained package,
profile, source, engine/helper, host-tool, ports and environment expectations.
It never reconstructs expectations from receipt fields. The reader checks the
raw manifest digest before parsing closed, duplicate-free canonical metadata,
requires the exact flat inventory and measures each bounded regular file
without following links. Hard-linked or physically reused files are rejected.
Receipt/report metadata keeps its 1 MiB limit, logs the existing rendered-log
limit and each standalone object the existing 128 MiB limit. A PC lane includes
its x86-64 and i386 objects; other profiles select their own target triple.

Downloaded ELF objects undergo the existing class, machine, AROS ABI, collector
symbol and source-bound GNU RISC-V checks. Their actual measurements join the
exact acquired bytes through the same six-phase receipt validator used by local
read-back. Inventory, file identities and bytes are observed again before
returning. The repeated byte pass retains only one additional bounded file at a
time, not a second complete ELF closure. Caller-exclusive quiescent inputs are
required; this is not an atomic snapshot against concurrent writers.

Export validation can inspect local Git/source state, probe prepared Python and
use temporary sealed host-generator inputs, just like the retained reader; it
does not execute compatibility phases or write the returned export. Rootless
read-back writes nothing, executes no process and never opens original host
roots. The collector must separately bind the independently supplied package
identity to its actual verified indexed package, and authenticate job/artifact
origin, runtime and signatures. Byte consistency and valid ELF outputs alone
do not prove command execution, relocation execution or release eligibility.

## Complete indexed compatibility join

`native_candidate::readback_release_compatibility_v2` joins the complete indexed
A/B byte read-back to exactly one portable compatibility closure per archive.
It compares each independently supplied package expectation with the actual
verified package, and rebinds compiler/source/host/profile/upstream selection to
the indexed input group before accepting its reports, logs or ELF outputs.
Compatibility roots cannot overlap package/final inventories or build evidence,
or reuse physical directory identities. Both compatibility passes use the
existing portable validator. Finally the collector remeasures the already
parsed build/package/final closure against the factory's retained exact hashes;
it does not decompress unchanged archives solely for another byte observation.

The returned opaque build/compatibility join remains byte evidence. It does not
authenticate original job/artifact observations, environments or source/runtime
origins, verify signatures, or admit V2 qualification, recovery or publication.

## Remaining release integration

The build CLI emits the finished receipt. `producer package` selects the guarded
join with `--build-result FILE --build-result-sha256 SHA --build-work-dir DIR`
and explicit `--package-format family-v2`. The SHA selects the exact serialized
build-result bytes, not a receipt self-digest. The closed result reader requires
the complete ordered phase/finished evidence, committed local state, matching
host/roots and the exact published collector inventory. It retains the result
bytes for revalidation before package commit. Recipe, lock, profile and host
remain separately selected package inputs. The result's executor observation is
byte-bound, not authenticated.

Guarded JSON output adds `finished_candidate`, joining the selected result,
finished receipt, raw payload and normalized package-tree digests. It also
contains the exact export string in `portable_measurement` and its raw SHA-256
in `portable_measurement_sha256`. Preserve the string bytes without adding a
newline when retaining a separate transport file. Without the
three evidence options, `producer package` retains its ordinary local packaging
behavior and does not assert finished-build provenance. Complete V2
qualification/recovery and producer workflow admission must consume these
proofs explicitly and retain independently authenticated execution evidence.
A self-hash, a locally consistent tree or a valid archive does not establish
fresh compilation, independent A/B executions, compatibility, signer identity,
public release readiness or physical board boot.
