# Developer SDK artifact contract v2

Related initiative: [declarative boards and RISC-V integration](riscv-board-integration-plan.md).

## Identity and ownership

An SDK is one source-produced Developer sysroot for one target/ABI. It is not
a host compiler archive, a board firmware image, or a universal SDK for every
target. The selected AROS source owns its headers, libraries and ABI facts;
`aros-tools` packages and verifies those bytes. Compiler family, compiler
release and build host remain separate identities.

`SdkIdentity` records a portable SDK ID, an independent canonical SemVer
version, a source-selected target triple and scalar ABI facts, exact source
commit/Git tree/content digest, the source-owned SDK contract digest and a
source-date epoch. No board enum, host selector or executable extension is
introduced. ABI map keys are unique; map order does not change its canonical
digest. Changing a source fact changes that descriptor digest. Validation
checks representation and consistency, not that the chosen ABI is implemented.

The caller must select the identity document and its raw SHA-256 independently.
Packaging does not derive a version from the installed compiler or manufacture
a released source identity from an uncommitted tree. The first public SDK
version and repository/channel selection remain release-integration decisions;
local package success does not make them published.

## Complete local package

The archive name is `aros-sdk-<sdk-id>-<version>.tar.xz`; there is no host
suffix. A complete local set contains exactly:

- the deterministic archive;
- `<archive>.manifest.json`, byte-identical to the embedded manifest;
- `<archive>.sha256`, selecting the archive filename and measured digest.

The archive begins with `sdk/`. Its `sdk-manifest.json` uses schema
`aros-sdk-manifest-v2`, the exact identity, canonical tree digest and complete
sorted payload inventory. Only this embedded manifest is excluded from the
payload digest. Every other input is preserved, including a file named
`toolchain-manifest.json`. An existing SDK manifest in the candidate is rejected
rather than removed or silently adopted.

The SDK shares the normalized inventory-entry and JSON-line digest primitive
with compiler packages, not their host/profile manifest. Files use modes 0644
or 0755, directories 0755, internal symbolic links 0777. Links must resolve
inside the payload to declared entries or its root; dangling/cyclic links,
case-folding aliases, special objects and archive hardlinks are rejected.
Source hardlinks are not accepted as independent package inputs.

Limits are 100,000 payload entries, 2 GiB of regular payload bytes, 128 path
components and 16 MiB of metadata. Inventory acquisition and file hashing are
bounded; the parser also has bounded XZ memory and stream budgets. Tar metadata
uses root ownership, the selected epoch and global lexical ordering. Long names
use the same deterministic PAX encoding as compiler archives.

## Writer and read-back

The candidate is read-only. The writer requires an independently selected tree
digest, acquires a no-follow bounded snapshot, copies it into private staging,
rechecks the complete inventory and scans selected absolute build prefixes.
It creates the entire package under an existing real output parent and makes
the final absent directory visible with no-clobber publication only after
successful package read-back. Source inputs are revalidated before commit.
It never adopts an existing output or deletes an unrelated directory.

Verification requires an independently selected identity, tree digest, archive
digest and archive byte size. A sidecar alone is not a trusted selection.
It checks the exact local member set, closed bounded metadata, identical
embedded/external manifest bytes, every decoded payload member and hash,
normalized metadata, link containment and the stream terminator. It rechecks
package bytes before returning. Extraction repeats verified stream parsing
into one fresh root, then inventories the materialized payload. A failed
extraction retains its root for diagnosis; it is never adopted as success.

These operations assume a quiescent owned workspace. Descriptor-bound
acquisition and before/after checks detect ordinary mutation; they do not claim
isolation from a hostile concurrent filesystem writer.

## CLI and assurance

`aros sdk package`, `aros sdk verify-package` and `aros sdk extract` are explicit local
stages, usable outside an AROS checkout. Their required selectors are shown by
the generated CLI reference. JSON results state
`local-byte-consistency-not-release-admission`.

This boundary does not authenticate the producer, prove that a compiler ran,
establish cross-host SDK reproducibility, substitute for C/C++ relocated-link
qualification, or grant publication authority. The native SDK link receipt
remains separate evidence; an inventory assertion is not SDK archive bytes.

## Consumer release lock

`aros-sdk-lock-v2` selects an exact release tag, canonical credential-free
HTTPS base URL, SDK identity, payload tree digest, archive SHA-256/size and
manifest SHA-256/size. Its release version may differ from the SDK version:
one release can distribute independently versioned compiler and SDK artifacts.
Asset names are derived from the SDK identity, not a host or board selector.
Unknown or duplicate fields, URL aliases/credentials, zero digests and oversized
metadata are rejected. The lock itself has a 1 MiB limit and its URL a 4096-byte
limit.

The consumer read-back API measures the exact opened archive, checks the
embedded canonical manifest against the independent lock, and verifies the
complete payload inventory. Installed verification repeats bounded no-follow
inventory acquisition, compares all bytes and rechecks the tree and manifest
before returning. Installed payload modes must equal the normalized archive
modes; changed permissions or set-ID bits are rejected, not normalized away.
The enclosing installation root remains the store's private access boundary.
Consumer verification does not infer build-root prefixes;
those are producer qualification inputs. A matching consumer lock proves
selected byte consistency, not publisher authentication or release admission.

## Managed store boundary

The Unix store API uses this host-independent namespace:

```text
<store>/v2/<sdk-id>/<sdk-version>/<canonical-lock-sha256>/
  lock.json
  .complete
  sdk/
    sdk-manifest.json
    ...complete SDK payload...
```

It verifies the selected archive before creating a store namespace. A writer
lock serializes installation; a private sibling stage is fully read back before
durable no-clobber publication. Reuse requires complete installed verification.
Corrupt envelopes are rejected, never repaired or overwritten. Prepublication
private staging is cleaned automatically; an uncertain published destination
is retained and reported with an explicit commit state. Persistent empty
publication locks are recognized only by names derived from actual envelopes.

Read-only inventory has an explicit namespace-entry budget and reports malformed
entries and incomplete coverage. It reads bounded control metadata, does not
hash or execute payloads, and marks every listed SDK `not-checked`. Full
verification is a separate operation. An absent store stays absent; a symlinked
store is reported as unsafe and is not followed. There is no global active SDK,
automatic removal or project selection in this API.

`aros sdk download` acquires the exact locked manifest/archive into a dedicated
lock-digest cache namespace and verifies the complete archive. `--offline`
uses existing verified bytes only; poisoned cache entries are not replaced.
`aros sdk install` uses the same retained private snapshots or one explicitly
selected local archive, then installs or strictly verifies reuse. The local
archive selector conflicts with cache/transport options, so no argument is
silently ignored. SDK lock files are bounded, independently hash-selected and
remeasured before/after the operation.

`aros sdk list` exposes bounded metadata-only observation; `aros sdk verify`
checks the complete selected installation. `aros sdk path` verifies before
printing an application-build input root. The default store is `AROS_HOME/sdks`;
the default dedicated archive-cache parent is the AROS archive cache's `sdk-v2`
directory. Neither root is created by read-only observation. SDK input paths
never override `AROS_SDK_INCLUDE_DIR`, which is an AROS source-build output.

This is not a claim of a published SDK. Project-bound selection,
source-input SBOM/signatures/attestations, a published SDK index/consumer lock
and release/consumer qualification remain required distribution work. The
three local files are not an SDK release inventory and do not expand or
replace the compiler release asset contract.
