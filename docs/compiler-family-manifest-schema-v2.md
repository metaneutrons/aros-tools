# Compiler-family manifest schema v2

The structural JSON Schema fixture is
`crates/aros-common/tests/fixtures/toolchain-manifest-v2.schema.json` (Draft
2020-12). It describes manifest schema `2` with a required, closed compiler
identity: LLVM has `family` and `version`; GNU has GCC and binutils versions
and an explicit RISC-V target contract. `llvm_version` is not a v2 field,
including when its value is `null`. Historical schema `1` is unchanged.

The schema constrains lower-case hashes and Git object IDs, bounded numeric
version components, unsigned 64-bit values, target declarations and typed
inventory entries. GNU target CPU width must match its ABI. Host and profile
are safe segments, not a fixed board enum. Optional inventory fields may be
omitted or explicitly null when the Rust representation permits that form.
The synthetic examples cover LLVM, GNU RV32/P4 and GNU RV64; their Rust tests
exercise the authoritative manifest validator. They are not real compiler
packages, SDKs, native build receipts or hardware evidence.

The schema is a structural aid, not a substitute for
`ArosToolchainManifest::validate`, strict native package read-back or release
qualification. JSON Schema does not establish sorted inventory, path-relative
symlink containment, hashes over actual bytes, recipe/input binding or file
presence. Standard JSON Schema integer semantics also do not establish the
exact lexical representation accepted by Rust's JSON deserializer. Source
selector and package-path validation may impose stricter release constraints
than the common manifest's structural contract.

Hash and object-ID rules include exact lengths. Version and target rules
exclude unsupported characters explicitly: `$` alone may match before a final
newline in a regular-expression implementation. The same distinction is used
for literal dot path components, so the structural schema does not mistake a
different common-contract filename for `.` or `..`.

The indexed package read-back boundary binds this schema and the tree-digest
fixture to the exact bytes embedded in the tools runtime. The [family-v2 index
CLI stages](compiler-family-index-cli-v2.md) use measured index and checksum
writers; qualification/recovery and protected workflow integration remain
separate. Fixture binding alone does not establish a complete v2 release.
