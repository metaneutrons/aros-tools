# SDK generation readback CLI

`aros sdk verify-generation` exposes the native generation readback boundary.
It observes existing files only: no source program, compiler, CMake or Ninja is
executed, and no output file is created. The JSON result is a byte-consistency
join, not an authenticated execution claim or release admission.

## Independent selections

The caller selects the release-input directory and its raw collection SHA-256,
the SDK product, compiler group/profile, build host and release ID. The native
release-input validator owns product membership and source/ABI identity. A
separate selection document and its raw SHA-256 supply expected bytes and local
paths. They must be retained independently of the downloaded records.

The closed selection schema is `aros-sdk-generation-readback-selection-v1`:

| Field | Selected material |
| --- | --- |
| `compiler.directory` | Complete Family-V2 compiler package directory |
| `compiler.archive_sha256`, `compiler.archive_size` | Expected compiler archive bytes |
| `compiler.environment.path`, `compiler.environment.sha256` | Exact original build-environment document |
| `sdk.directory` | Complete three-member SDK package directory |
| `sdk.tree_sha256`, `sdk.archive_sha256`, `sdk.archive_size` | Expected SDK inventory and archive bytes |
| `generation.path`, `generation.sha256` | Exact generation record |
| `compatibility.directory`, `compatibility.manifest_sha256` | Complete portable six-phase compatibility evidence |
| `compatibility.inputs.path`, `compatibility.inputs.sha256` | Exact retained compatibility input observation |

Paths are explicit normalized absolute paths. The selection document remains
outside the input, package and evidence roots. Metadata reads are bounded and
reject linked files; unknown or duplicate fields and malformed selectors fail.
At least one explicit `--forbidden-prefix` is required for payload readback.

For an export produced by `aros sdk build --evidence-dir`, select its inner
`evidence/` directory and the sibling `inputs.json` file, not the export root
as the compatibility directory. Keep selected paths quiescent during readback.
Repeated observations detect drift; they do not create filesystem isolation or
an atomic snapshot against concurrent writers.

## Verification

The command validates the source-selected compiler profile and SDK association,
reads back the compiler package and checks its independently selected archive
hash and size. It then verifies the SDK package, generation record, portable
compatibility closure and four original/relocated C/C++ links through the native
Rust validators. It does not reopen paths recorded on the original build host.
Selected documents are measured again before success is emitted.

## Release boundary

The caller must separately authenticate the original owning CI job, workflow,
run attempt, artifact identity and downloaded bytes. Supplying matching hashes
does not perform that authentication. The readback result also does not prove
all hosts, cross-host SDK reproducibility, the complete release inventory,
signatures, public downloads or consumer installation.

Complete generation coverage is the input-derived set of SDK product,
referenced compiler group/profile and active host tuples. One successful host
does not qualify a host-neutral SDK. Each host's verified SDK package must
match the same index-bound package identity and archive bytes; generation
records and compatibility receipts are host-bound and need not be identical.
This coverage and authenticated transport are not implemented by this command.

The frontend was exercised against retained GNU compiler, SDK, generation and
portable compatibility artifacts. A canonical isolated version substitution
was rejected even after its selection hashes were updated. This establishes
the GNU frontend byte join, not CI origin or release qualification. LLVM
frontend replay and the complete newly selected CI generation matrix remain
required; neither follows from help output or package verification alone.
