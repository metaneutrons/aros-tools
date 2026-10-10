# Native executor input groups v2

`aros toolchain plan` and `aros toolchain build` can select several compiler
input groups from one committed producer revision. The declaration stays at
`toolchains/producer-executor-v1.toml`: this pathname identifies the unchanged
native execution protocol. Selection schema 2 does not change recipe v2,
executor identity, phase receipts, prepared-cache policy or origin requirements.

Schema 1 remains supported as the existing single source-lock/profile pair.
Schema 2 replaces those two top-level fields with a closed `groups` array:

```toml
schema_version = 2
contract_id = "aros-toolchain-producer-v1"
contract_path = "contracts/toolchain-producer-v1.toml"
contract_sha256 = "<measured contract digest>"
tools_commit = "<exact tools commit>"

[[groups]]
id = "gnu-rv32"
source_lock = "toolchains/gnu.sources.json"
source_lock_sha256 = "<measured raw source-lock digest>"
profiles = "toolchains/gnu-profiles.json"
profiles_sha256 = "<measured raw profiles digest>"

[[groups]]
id = "llvm"
source_lock = "toolchains/llvm.sources.json"
source_lock_sha256 = "<measured raw source-lock digest>"
profiles = "toolchains/llvm-profiles.json"
profiles_sha256 = "<measured raw profiles digest>"
```

Placeholders describe the shape, not usable identities. Both schema variants
reject unknown or duplicate TOML fields. No schema fallback is permitted.

## Selection and verification

- The declaration is limited to 1 MiB and 1–32 groups. IDs are sorted,
  unique lower-case kebab identifiers, at most 64 bytes long.
- Every document path is a distinct safe producer-relative path under
  `toolchains/`. Source locks end in `.sources.json`; profiles end in `.json`.
- Every group's raw source-lock and profile bytes are read, hash checked and
  parsed before selection, including groups not requested by the caller.
  Each parser retains its own 1 MiB limit and semantic bounds.
- Lock and profiles must declare the same compiler family. Profile names must
  be unique across all groups, not just within the selected group.
- Exactly one group must match both recipe digests and contain the requested
  `--preset`. A preset from another group or an undeclared digest pair fails.
- The selected pair passes the existing native binding checks: exact tools
  commit, contract digest, recipe digests and source patch closure.

Planning reads exact committed regular blobs. Fresh builds and explicit
compiler-phase resumes repeat selection from the isolated producer snapshot.
The recipe binds the selected raw digests; its producer commit/tree binds the
whole declaration and all other group inputs. Phase inputs and retained
snapshots retain these identities. Group IDs are selection labels, not new
receipt fields or alternate execution backends.

## Scope

This is native input selection, not release orchestration or an SDK installer.
The [release input collection](compiler-family-release-inputs-v2.md) separately
binds each group's recipe and derives the release matrix. This declaration
does not create that collection, publish artifacts, attest a frontend, qualify
another host, or establish a successful compiler build.
