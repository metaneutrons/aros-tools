# Compiler-family compatibility boundaries

Native package verification and two-root extraction admit GNU compiler-family-v2
packages through the family-derived package format. Extraction alone does not
qualify relocation or a working AROS consumer.

`aros toolchain producer compatibility --package-format family-v2` explicitly
selects compiler-family-v2 packages for LLVM as well as GNU. Omission preserves
LLVM's historical v1 format and GNU's v2 format. Both relocation roots verify
the complete four-member package independently with the same selection; there
is no filename detection or fallback after a verification failure. LLVM
family-v2 execution emits a new bound receipt (schema v4); historical LLVM v1
packages retain their unbound schema-v2 receipt. This does not extend release
qualification or recovery admission.

## Standalone output verification

`compatibility::verify_standalone_outputs_with_compilers` accepts the exact
compiler identity for each requested standalone target. The identity map must
cover the same one or two target triples as the output request. Callers must
bind these identities to independently verified manifests and selected
recipe/profile inputs; this API does not authenticate a declaration.

GNU RISC-V C and C++ outputs are checked against their complete target contract:
machine, ELF width, relocatable link type, floating-point ABI, canonical ISA
attributes, stack alignment, atomic/register conventions and AROS ABI marking.
Explicit LLVM outputs also require the selected CPU's ELF machine and
relocatable link type. Language-specific collector symbols must be defined in
an existing section or as absolute symbols; undefined or invalid section
references are rejected. No ISA, ABI or board
selection is inferred from an output filename. The legacy output verifier still
rejects RISC-V when no explicit compiler identity is supplied.

This is a read-only byte-validation boundary. Synthetic fixtures qualify its
acceptance/rejection behavior, not a compiler build, AROS final link, execution,
hardware boot, signature or release. No new public CLI command is provided.

## GNU consumer execution

The six-phase adapter accepts verified GNU packages. It resolves C and C++
drivers from the inventoried tool-layout document, preserves their invocation
names, and uses the package's exact GCC/Binutils versions and RISC-V contract.
Both extracted inventories are checked before execution and after all phases.
The caller must exclusively own the roots; remeasurement is not a concurrent
filesystem lock.

GNU requires `--source-preset`, selected explicitly from the measured consumer
source's `aros-targets.toml`. Its `toolchain_profile` must select the producer
profile; CPU, platform and GNU family must match. Source/board selectors and MMU
policy come from that declaration. The CMake source preset and compiler package
profile remain separate namespaces. No built-in fallback or filename inference
is used. LLVM retains its legacy adapter and does not accept this option.

GNU receipts use schema v3 and bind the compiler/package identity, source preset
and measured consumer source tree. LLVM family-v2 receipts use schema v4 and
bind the same package fields and consumer tree without a GNU source preset.
Both bind the verified archive hash/size, canonical parsed-manifest hash and
payload tree hash. Both require the manifest's exact package-source commit
from the verified recipe and raw profiles-document digest and remeasure both
extracted inventories before and
after execution. Their standalone outputs also pass the explicit compiler-bound
ELF verifier, including both x86-64 and i386 for the PC profile.

The package-build source and upstream consumer snapshot are distinct inputs.
The recipe binds the former; `profiles.upstream_commit` binds the latter, and
the pristine upstream checkout is audited against that exact commit. A ports
lock for upstream consumption must bind the consumer commit, not silently
substitute the package-build commit. These inputs may differ. The engine-free
CMake source has its own measured content digest, which is not a Git object ID
or independent source authentication.

A receipt is not a signature or publication
admission. Synthetic six-phase tests establish command and validation behavior,
not actual compiler execution. Real native consumers remain required. Complete
V2 release-evidence, recovery and workflow integration are still separate work;
the legacy qualification path does not admit v3 or v4 receipts. Legacy LLVM v1
execution and receipt serialization remain unchanged.

## Retained receipt read-back

`compatibility::readback_native_compatibility_receipt` joins exact GNU v3 or LLVM
family-v2 v4 aggregate bytes with all six retained phase reports and their
ordered stdout/stderr logs. It requires independently supplied package,
profile, compiler/ABI, engine/helper, source, ports, environment and standalone
output identities. The package-build commit, engine-free consumer source CAS
and pristine upstream commit/tree remain distinct inputs.

The expected CMake command count comes from the validated source-owned
execution plan: configure only, or configure plus the declared GNU native
consumer build. LLVM selects configure only. The retained report cannot choose
or downgrade this expectation.

`compatibility::derive_native_compatibility_environment_identity` derives the
expected SDK/upstream and standalone digests from the prepared Python runtime
and revalidated host-tool closure. It shares the exact execution policy; it
does not copy an environment claim from a retained report. It checks the exact
host-specific role set, interpreter/`python3` binding and executable hashes.
The standalone expectation is the fixed poisoned environment with no host
tools. Derivation runs the prepared runtime's bounded interpreter version
probe but creates no output roots and executes no compatibility phase.

The host-closure PATH is normalized under the existing report-v5 policy;
Python executable and private import paths are not. These digests bind the
exact prepared runtime layout, not an arbitrary reconstruction into new
directories. Callers must also bind the runtime and tools to verified release
inputs; deriving an environment does not authenticate it or grant release
admission.
Python root-directory validation does not remeasure module contents. Files
modified under an unchanged import path do not change this environment digest;
release admission must separately verify those runtime bytes against the
locked inputs.

This bounded in-memory API checks canonical closed metadata, report/log hashes,
phase coverage and exact identity matches. It neither reads files nor
authenticates the declarations. Callers must obtain stable byte snapshots,
derive expectations from verified release inputs and runtime measurements, and
verify external signatures and attestations separately. Synthetic fixtures do
not establish an actual compiler or consumer qualification.

`compatibility::readback_retained_native_compatibility` acquires and validates
the existing local filesystem evidence using an independently selected
`NativeCompatibilityRequest` and profiles. It rejects legacy LLVM v1 before
preparation; it never falls back to executing compatibility. The request must
remain bound to the indexed lane's verified package and source/runtime inputs.
It checks the same exact report/log inventory and standalone ELF outputs as
the execution adapter below, without running compatibility phases or creating
or changing their output roots.

Validation still performs bounded offline Git/source inspection and a prepared
Python version probe. When the source declares host-generator inputs, it
verifies cached bytes through private temporary sealed copies and removes those
copies on return. This is not a no-subprocess or no-temporary-write API. It uses
the original prepared host paths, not a portable reconstruction on another
runner. It does not independently reconstruct command digests from a saved
command plan or prove that commands executed; those require separate trusted
execution evidence. Package signatures, runtime bytes, A/B independence and
release admission remain separate gates.

Without `--evidence-dir`, the CLI's `producer compatibility` command uses
`execute_native_compatibility_with_readback` for family-v2 packages. Before any
phase starts it derives the source-owned CMake command count and independently
measures the upstream tree and host environments. After execution it reads the
closed inventory of aggregate receipt, six reports and exact ordered logs,
reparses the standalone ELF outputs, and revalidates the source, extracted
packages, helpers and environments. Missing, extra, symlinked, oversized or
inconsistent retained evidence fails the command. LLVM v1 retains its previous
execution and receipt format.
The report directory also contains the durable publisher's persistent empty
advisory locks. Only the lock name derived for each expected report/log/receipt
is admitted; unknown locks, nonempty locks and unfinished journals are rejected.
The standalone directory must contain exactly the declared C/C++ outputs.
The adapter compares its receipt digest to the executor's in-memory result;
this preserves continuity of command reports, not an independent reconstruction
of command and executable digests from a saved command plan.

This is a read-back of exclusively owned local execution roots, not an atomic
snapshot or authenticated recovery into a new workspace. Family-v2 release
recording, recovery and protected workflow admission remain separate
integration requirements.

## Separate local input observation

The library's `execute_native_compatibility_with_export` captures a canonical
`aros-toolchain-compatibility-inputs-v1` document before executing a family-v2
lane and exports the retained reports, logs and ELF outputs separately. It
records package/profile, source, engine, helper and environment identities,
including the raw SHA-256 and size of each standalone C/C++ source fixture.
Each fixture must be a stable, no-follow regular file of at most 1 MiB; its
contents are rechecked before execution, after execution and after export.
Changed fixtures prevent a successful export.

The CLI exposes this owning-host operation through optional
`producer compatibility --package-format family-v2 --evidence-dir ABSENT_DIR`.
The directory must be absolute, have an existing non-symlink parent, and remain
separate from every input and execution root. Legacy-v1 export is rejected
before extraction. Without this option, execution and stdout keep their prior
shape.

Overlap checks compare existing directory identities, including filesystem
aliases. Missing path suffixes are compared conservatively without ASCII case;
choose distinct component names even on a case-sensitive filesystem.

Only a complete successful execution/export creates the final output: one
no-clobber directory rename exposes exact `inputs.json` and the closed flat
`evidence/` inventory together. Publisher-owned empty advisory lock files may
also remain in the outer directory; they are not evidence members. An error
may retain an owned sibling stage for diagnosis. A post-rename durability error
may leave the complete destination; inspect it rather than retrying over it.
The JSON result adds `local_evidence.directory`, `inputs_sha256` and
`manifest_sha256`. Retain these exact digests independently of uploaded bytes;
do not select them from a downloaded report's own claims.

This is not the aggregate compatibility collector. The input-document parser
returns unauthenticated claims; protected job/artifact origin and selected
runtime/input bytes still need independent verification.
The caller must exclusively own quiescent roots. Remeasurement is not a defense
against deliberate mutate-and-restore races and does not authenticate command
execution or admit a release.
