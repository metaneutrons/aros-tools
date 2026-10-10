# Compiler-family release integration audit

Source inspection dated 2026-10-10. This is a bounded integration audit, not a
release qualification or a parallel milestone status ledger. Requirements remain
in [RV4](riscv-board-integration-plan.md#rv4-immutable-toolchain-releases);
milestone issues own execution evidence. A local API test does not establish a
released runtime, successful remote build or authenticated workflow execution.

## Selected first RV32 release

The first mixed release selects the existing LLVM profiles `pc-x86_64`,
`arm-raspi` and `rpi-aarch64`, plus GNU `rv32-esp32p4`, on Linux x86-64,
Linux AArch64 and macOS AArch64. RV64 is a separate follow-on, not an omitted
lane in this selected release. The wider initiative remains open until its
other acceptance requirements are satisfied.

The reviewed release-input collection must be the single matrix authority:
two groups, four profiles, twelve lanes, twenty-four first-baseline A/B builds,
twelve four-member byte comparisons and twelve compatibility/relocation lanes.
Each group owns its exact source revision, recipe, lock and profiles; the tools
and producer identities are shared. Do not infer lanes from discovered files.

For this collection the complete index-derived inventory has sixty members:
forty-eight package members, six group documents and six fixed support files.
The pre-attestation list has fifty-eight subjects; provenance and final
checksums are absent then. Final checksums cover fifty-nine members, including
provenance but excluding themselves. These counts follow the collection and
stage rules; they must not become a second hard-coded workflow inventory.

## Inspected boundaries

| Boundary | Source evidence | Remaining integration finding |
| --- | --- | --- |
| Input selection | `release_inputs.rs`, `native_declaration.rs`, `toolchain_producer/release_plan.rs` | The library binds multiple compiler groups; `producer release-plan` projects the complete digest-selected group/host/profile set without building or qualifying it. The producer must supply and consume that same reviewed set. |
| Local execution | `native_lifecycle.rs`, `executor.rs`, `native_candidate.rs` | `publish`/result outputs remain collector-only; a separate finished record measures the complete tree and ordered chain. Its guarded package join is local library evidence, not authenticated workflow admission. |
| Packaging and index | `release_index_v2_builder.rs`, `release_index_v2_readback.rs`, `toolchain_producer/release_index_family.rs` | Explicit family-v2 CLI stages measure packages and index/checksum inputs. They do not qualify execution or authenticate provenance. |
| A/B comparison | `native_candidate/comparison.rs`, `native_candidate/release_readback.rs`, `release_index.rs::PackageComparisonReport` | Local proofs and complete portable byte acquisition join both packages, retained exports/reports, selected groups and final checksum/subject bytes. `verify-release-evidence` exposes the complete byte collector; external execution/artifact authentication remains separate. |
| Compatibility | `compatibility/execution_retained.rs`, `compatibility/execution_portable.rs`, `native_candidate/release_compatibility.rs` | Owning-host export and rootless read-back validate the exact report/log/ELF closure against independent expectations. The CLI exposes owning-host exports through `--evidence-dir` and complete indexed acquisition through `verify-release-evidence`. Authenticated recovery admission remains incomplete; byte consistency is not proof of execution. |
| Qualification | `qualification_evidence_v2.rs` | The V2 record validates claims and their index/policy joins, not actual report acquisition. `record-qualification` still uses V1. |
| Recovery | `recovery.rs`, `toolchain_producer.rs` | Admission, inventory measurement and CLI still select the V1 index/evidence and active/historical V1 inventory shapes. |

Paths in the table are relative to `crates/aros-toolchain/src/` except
`toolchain_producer*`, which belong to `crates/aros-cli/src/`.

### Build evidence must bind the finished payload

The compiler phase measures regular outputs before collector installation.
LLVM collector installation deliberately removes producer-only inputs,
including `llvm-config` and LLVM CMake metadata. Requiring every old compiler
output to remain in the finished prefix would therefore reject legitimate
builds. Conversely, accepting the later collector-only receipt would leave the
remaining compiler bytes unbound. A self-hash proves neither execution nor
independence; the legacy qualification collector currently checks that hash and
the publish phase/schema without joining a complete payload to the indexed lane.

The complete release integration must bind the finished compiler candidate to a measured
package/payload inventory, independently selected group/recipe/source/tools/host
identity and the ordered lifecycle chain. Read retained metadata through bounded,
closed, duplicate-rejecting, no-follow acquisition. Do not widen the legacy
resume parser into a publication gate by assertion. Keep A and B roots distinct,
bind both complete package sets to the comparison and retain external execution
provenance separately. A copied report with a recomputed self-hash must not
qualify a substituted lane or compiler payload.

The [finished-candidate boundary](compiler-family-finished-candidate-v2.md)
provides the local full-tree/chain reader and guarded V2 package operation.
The producer CLI can explicitly select the guarded operation through a complete
build-result file, external file digest and original work root. Ordinary
packaging remains separate. The complete byte collector consumes these
portable measurements explicitly; local read-back does not close the external
provenance or independent A/B requirements.

### Local consistency and portable admission are different

The retained compatibility reader uses the original host paths, bounded offline
Git inspection, a prepared Python version probe and temporary sealed source
inputs. A Linux collector cannot validate a macOS result by pretending those
paths or tools are present. Validate complete evidence on its owning host and
transport closed byte-bound records under authenticated run/artifact provenance.
Portable admission must independently bind those records to the selected lane,
actual downloaded package set, reports, log/output closure and fixed verifier
policy. It must not silently execute missing evidence or trust its own policy
claims. Hashing an opaque provenance bundle is not signature verification.

The remaining portable handoff must keep three inputs separate:

- Independently selected release inputs, index, lane and executor policy.
- Owning-host measurements: exact build-result bytes, six ordered phase
  receipts, complete finished record, raw payload identity, guarded four-member
  package measurements and the independently measured environment. A closed
  exported record must be created only after the local proof is revalidated;
  parsing its fields later cannot recreate that local proof.
- External run/artifact and signature verification: exact repository, workflow,
  run attempt, producer revision, immutable tag, owning job and artifact-byte
  identities. A report's own `verified` flag, self-hash or copied package
  environment must never supply this authority.

The aggregate collector must acquire the complete selected report set with
bounded, duplicate-rejecting, no-follow reads; join both build records and
comparison to measured packages and each group; and join compatibility to its
complete reports/logs/outputs and independent environment. Original absolute
paths are owning-host preconditions, not portable evidence. Separate A/B jobs
and authenticated origin establish execution separation; two copied local
records in different directories do not. The same admitted record set and
unchanged pre-attestation subjects must feed V2 qualification and recovery.
The owning-host package export and bounded portable read-back are implemented
in `native_candidate/portable.rs`. Guarded package JSON retains the exact export
string and its raw digest. The reader checks all eight documents, independently
selected lane/executor/environment and all four downloaded package members; it
does not open original roots or authenticate execution. The complete build byte
reader in `native_candidate/release_readback.rs` acquires exactly the indexed
A/B lane set, re-verifies and compares both four-member packages, and joins the
retained reports to final checksums and the unchanged pre-attestation subjects.
It accepts no diagnostic subset and grants no execution authority. The portable
compatibility reader now acquires a closed owning-host export, reparses actual
ELF objects and joins every report/log to independently retained expectations
without reopening original roots. The complete compatibility collector joins
every independent package expectation to actual verified A/B and final indexed
package bytes and rebinds all lane selectors to the selected input groups.
It admits no diagnostic subset and does not authenticate execution.
The owning-host CLI can now atomically publish the separate input observation
and exact portable file set after complete execution. It does not admit the
aggregate report set or authenticate the owning job. The separate
[`verify-release-evidence` CLI](compiler-family-release-evidence-v2.md) acquires
the complete indexed build and compatibility bytes without authenticating
execution. External origin and V2 qualification/recovery admission remain
implementation requirements, not authenticated evidence from this audit.

### Producer audit boundary

The inspected `aros-toolchains` tree at
`f35ecea2918c8c8314bf6878b4401af6ae45132d` still selects a single LLVM lock,
recipe and profile document. `toolchain-release.yml` expands three fixed
profiles, uploads lifecycle receipts, defaults to legacy package/index formats,
records V1 qualification and enforces forty-four final files.
`toolchain-compatibility-replay.yml`, `select-replay-matrix.py` and
`toolchain-release-recovery.yml` retain the same nine-lane assumptions. Their
tests must change with the workflow, not be relaxed to accept arbitrary subsets.

The recovery workflow currently derives `build_environment` from the source
package manifest. That is not an independent expectation for the V2 package
reader. The new recovery path must obtain the original environment from
separately verified execution evidence and bind it to the selected package.
Do not copy the manifest into the expected input merely to make verification
pass. The unchanged runtime host map and V1 tree fixture need no new versions;
new tools runtime pins still require measured published bytes. This audit did
not query live GitHub checks or attestations.

The concrete producer handoffs are:

| Handoff | Current writer and reader | Coordinated change |
| --- | --- | --- |
| Inputs to plan | `toolchain-release.yml` writes one recipe and expands three fixed profiles on three hosts. | Use `producer release-plan` on the reviewed group collection to derive its complete lanes; preserve exact independent group inputs and keep runner/A/B scheduling in workflow policy. |
| Build to package | The build step defaults to human stdout; packaging selects `publish.json` and ordinary format defaults. | Retain exact build JSON and its externally selected digest; use explicit guarded family-V2 packaging on the owning host. |
| Packages to comparison | Candidate and lifecycle artifacts are uploaded separately; the comparison job reads four package files per side. | Bind complete A/B candidate/package measurements to authenticated job/artifact origin. Do not replay original macOS roots on a Linux collector. |
| Compatibility to collector | One compatibility job per lane uploads retained results. | Validate original-root evidence on its owning host and transport closed, byte-bound records with independent environment expectations. |
| Collector to signing | The draft job runs V1 indexing and signs the current checksum file. | Select V2 indexing and preserve the exact pre-attestation subject manifest through external verification and final indexing. |
| Signing to qualification | V1 recording assembles eighteen publish receipts, nine comparisons and nine compatibility reports. | Acquire the complete input-derived report set and populate V2 claims from verified bytes, not discovered subsets or self-hash claims. |
| Qualification to recovery | V1 recovery downloads three profile patterns, reads environment expectations from manifests and repackages nine lanes. | Introduce explicit V2 admission, input-derived inventory and independently verified original environments; keep historical V1 recovery separate. |
| Replay selection | The replay workflow and `select-replay-matrix.py` repeat fixed host/profile lists and legacy archive names. | Derive replay selection and archive identities from the same input collection; diagnostic subsets remain nonqualifying. |

`scripts/toolchain/tests/test-producer.sh` and the release/recovery documentation
assert these current handoffs. Change them with the workflows, rather than
weakening tests or replacing forty-four with another unconditional constant.
Producer inspection is static evidence of integration work still required,
not proof of current GitHub execution or authentication.

## Three delivery blocks

These blocks organize implementation and verification; they do not replace
the RV milestone requirements or create a second execution-status ledger.

### Block 1: End-to-end boundary audit

Trace the selected inputs through recipe, native build, finished candidate,
package, A/B comparison, compatibility, index, qualification, recovery and
consumer installation. For every handoff identify the writer, reader, exact
selected bytes, identity joins and verification authority. Include the producer
workflows, replay and signing permission boundaries, not just Rust parsers.
The inspected-boundaries table above records the starting findings.

Close this block only with a complete handoff map and a coherent set of missing
joins. An internally valid self-hash or green local API test is not evidence of
execution, independent A/B runs or authenticated release admission.

### Block 2: Cohesive local implementation and counterprobes

Close the finished-payload/lifecycle/package/comparison joins together. Then
acquire complete V2 report evidence, package inventory, unchanged subject list,
final checksums and separately verified provenance. Explicit format dispatch
must reject mixed schemas and diagnostic subsets. Recovery cannot rescue failed
compilation, comparison or compatibility; historical V1 behavior stays separate.

Update producer source preparation, recipes, cache namespaces, runtime identity,
matrix, qualification, replay, recovery and signing as one integration block.
The reviewed input groups supply the lane set and explicit family-v2 stages.
Before compiler builds, exercise the complete synthetic handoff below with
positive cases and expected-diagnostic, no-mutation counterprobes. Documentation
must distinguish implemented commands from release-qualified behavior.

### Block 3: Real qualification and public cutover

Release and independently audit the required tools runtime first; pin only its
measured published values in the producer. Qualify one complete real RV32 lane,
including independent A/B builds, compatibility, AROS final links and relocation.
Only after that integration point run the complete selected twelve-lane matrix.
Do not restart a compiler matrix after each local parser or documentation fix.

After every required gate succeeds, audit the isolated draft, publish unchanged,
verify public URLs and immutable state, and update consumer locks with measured
values only. Fresh public installation and native consumer evidence close the
cutover, not the existence of a tag or draft.

Before expensive builds, run local contract/CLI/workflow checks and synthetic
complete-matrix acquisition with counterprobes. These checks cannot replace
actual host builds, A/B comparison, AROS SDK links, relocation, signature checks
or public consumer installation. Astro must describe the shipped command and
capability boundaries, not the audit's planned result. No hardware readiness or
separately distributable Developer SDK is inferred from a compiler release.

The complete synthetic handoff must exercise every downstream gate, not stop
after the first working stage. Counterprobes must include a missing selected
lane, a V1 manifest/index substitution, wrong group/source/compiler identity,
collector-only build evidence, reused A/B roots, changed report/log/output bytes,
self-rehashed payload substitution, expectations copied from package metadata,
changed subject-list bytes and a wrong signer/workflow. Require the expected
diagnostic and unchanged protected outputs, not merely any nonzero exit.

## Schema policy

The new release chain must use release-inputs-v2, manifest schema 2, index schema 2,
V2 qualification and V2 recovery without a V1 release-contract fallback.
Versions describe independently evolving contracts, not a cosmetic repository
generation. Unchanged tree-digest, native execution, comparison or metadata
envelopes need not be renamed. LLVM and GNU compatibility receipts keep their
own semantic versions. Production release SemVer remains Release Please owned.
