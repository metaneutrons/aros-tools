# Compiler-family release index v2

`NativeReleaseIndexV2` is a pure, bounded parser for compiler-family release
metadata. It binds the index to one validated
[release-inputs-v2 collection](compiler-family-release-inputs-v2.md), derives
the complete host/profile matrix and canonical package names from those bound
inputs, and exposes the declared package/support filename inventory.

The closed schema uses integer `schema: 2` and declares a safe release ID,
canonical credential-free HTTPS base URL, exact collection SHA-256, producer
and tools Git identities, and asset-sorted artifacts. Each artifact binds its
group, canonical asset, host, profile, target triple, source commit and
compiler identity to one input-derived lane. It also declares lowercase
archive/tree SHA-256 values, a size from 1 byte through 32 GiB, enabled status,
one strip component and sorted safe required paths. The parser rejects
duplicate keys, unknown fields, incomplete or extra lanes, unsafe paths, and
exact or ASCII case-folded collisions in derived inventory names.

The JSON document is limited to 16 MiB before parsing. Release IDs contain
1–128 ASCII bytes from `a-z`, `0-9`, `-`, `_` and `.`; `.` and `..` alone
are rejected. Each artifact declares 1–500,000 required paths of at most
1,024 bytes each. Paths cannot contain control characters, backslashes,
empty segments, `.` or `..` segments, or a leading slash.

The expected inventory contains these six fixed support files:

- `toolchain-release-inputs-v2.json`
- `toolchain-index-v2.json`
- `SHA256SUMS`
- `toolchain-provenance.sigstore.json`
- `toolchain-manifest-v2.schema.json`
- `tree-digest-v1.fixture.json`

It also contains each group's exact recipe, source-lock and profiles filenames.
For every lane it contains the canonical archive and its `.manifest.json`,
`.sha256` and `.spdx.json` sidecars. Thus two groups supplying three profiles
across the three hosts declare 48 filenames, not the historical v1 inventory's
44. Inventory size is derived from the selected inputs, never fixed to one
board list.

Archive hashes, sizes and tree digests are producer declarations here; the
parser does not independently measure or verify them. The expected inventory
is a name set, not a statement that files exist. Parsing performs no filesystem
or network access and makes no signature, provenance, publication or
readiness claim. It does not change the v1 index's admitted hosts or matrix.

## Measured construction

`release_index_v2_builder::build_measured_index_v2` constructs an index from a
complete local pre-index stage. This stage contains the exact input documents,
the runtime's manifest schema/tree fixture bytes, and four package members for
every input-derived lane. The index, `SHA256SUMS` and provenance bundle must be
absent. Unexpected or incomplete inventories fail; final directories are not
silently interpreted as pre-index stages.

The caller supplies independent expected build environments and sorted required
payload paths, each keyed by exactly the canonical archive names. The builder
verifies every package using the bounded compiler-family-v2 reader. It measures
archive hashes/sizes and payload tree digests rather than accepting them as
caller declarations. Manifests, sidecars, SBOMs and compiler/input identities
must agree; required paths must be present. The complete measured record then
passes the same closed index parser described above.

The operation is read-only. It checks real directory ancestors, exact regular
inventory, bound input bytes and static support bytes before and after package
verification. There is no ownership lock or snapshot guarantee against concurrent
writers. Required-path material is bounded before output-record allocation,
and the encoded index retains the 16-MiB parser limit. No partial index is
returned on failure.

Independent environment maps are checked before cloning: nesting is limited to
64 levels and the aggregate budget is 16 MiB of key/string bytes plus JSON-node
storage accounting. Forbidden-prefix selectors are a small operational list,
limited to 64 entries and 16 KiB of aggregate OS-string bytes because the package
reader scans payload bytes against each selector. These request limits bound local measurement work;
they do not estimate compiler memory requirements.

## Exclusive local index output

`release_index_v2_writer::write_measured_index_v2` runs the complete measured
builder before creating any output. The writer is available only on Unix hosts,
matching the native Linux/macOS release matrix and descriptor-traversal contract.
It exclusively creates the canonical index
relative to a held no-follow directory descriptor, synchronizes file/directory,
and reopens the file to verify its exact bytes and identity. Existing outputs,
including symlinks and directories, are never replaced. A successful result
reports the measured index and its persisted size/hash; all other stage files
remain unchanged. The stage still lacks final checksums and provenance.

The caller must own a quiescent stage. Descriptor identities and inventory checks
are not a snapshot or ownership lock against concurrent package writers. A failure
after output reservation retains the possibly incomplete file for diagnosis;
there is no rollback or replacement retry. Use a fresh stage after such a failure.
Final complete package read-back remains mandatory before release admission.

These contracts are not release qualification. Local construction, index output
and indexed package read-back do not execute compilers, prove A/B/relocation,
authenticate provenance or admit publication. A separate
[v2 qualification-evidence contract](compiler-family-qualification-evidence-v2.md)
binds compiler/input/index/report claims. The [family-v2 index CLI
stages](compiler-family-index-cli-v2.md) connect local measured construction and
finalization; aggregate qualification report read-back, recovery admission and
protected workflow integration remain outstanding. Final checksum verification and
exclusive local checksum output are
[separate boundaries](compiler-family-final-checksums-v2.md).
The acceptance target remains
[RV4: Immutable toolchain releases](riscv-board-integration-plan.md#rv4-immutable-toolchain-releases).
