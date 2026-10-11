# Native SDK application-link evidence

The source-owned `aros-native-consumer-contract-v2` seals one C and one C++
application source and an ordered list of optional additional SDK libraries.
These are application links, not the existing freestanding collector probes.
Historical consumer-v1 contracts remain readable but cannot qualify this proof.

The tools-owned CMake engine retains its validated source response as
`aros-native-consumer-binding.json`. The SDK root is derived from the selected
source configuration, not a board name or a Rust path constant. The executor
repeats validation with the measured transpiler and requires the exact same
source, contract, profile, ABI and SDK-root selection.

## Owning-host operation

`aros_toolchain::compatibility::execute_native_sdk_links` consumes an explicit
preparation, source profile, completed CMake build and independently verified
GNU compiler identity. Its absent output root must be disjoint from all inputs.
It does not download, build an SDK, select a release or authorize publication.

The operation:

1. Measures and relocates the complete SDK, including regular files, empty
   directories, relative links and its embedded metadata. Copying preserves
   filesystem modes; the shared inventory uses canonical portable modes.
2. Runs C and C++ with the selected ABI and normal startup/runtime defaults,
   once against each SDK root. No `-r`, `-nostdlib`, `-nostartfiles` or
   `-nodefaultlibs` override is permitted. Normal AROS GNU application output
   is an AROS ET_REL ELF; ET_REL alone does not mean a freestanding probe.
3. Checks target ABI, original/relocated byte equality, linker input closure,
   declared libraries resolved inside the selected SDK, and absence of the old
   SDK path in relocated trace/map output. Only owned direct GCC temporary
   object names may be absent after the driver returns.
4. Rechecks the complete SDK/compiler/source identities and every retained
   output, map, report, log and inventory before writing a success receipt.
   A later C++ driver changing an earlier C result prevents that receipt.

SDK inventory traversal is limited to 100,000 entries, 2 GiB of regular files,
128 path components and 16 MiB of encoded metadata. File hashing streams
through bounded no-follow readers. Absolute, escaping and dangling SDK links
are rejected. Existing output roots are never adopted or overwritten.

`readback_native_sdk_links` reopens the exact retained proof without rerunning
processes. It requires the caller's independently selected receipt digest,
rederives command/helper/environment identities and checks all original roots.
It therefore runs only on the owning host, not a rootless release collector.

## Admission boundary

The receipt is `aros-native-sdk-link-receipt-v1` with qualification
`local-links-not-release-admission`. Its version is independent of the source
contract version. A receipt self-hash is not execution authentication.

The executor and readback have synthetic positive and corruption coverage;
fake drivers in those tests do not qualify a real compiler. The separate local
native SDK build and real application links do not automatically qualify this
new executor.

Selected consumer-v2 GNU compatibility lanes require all four SDK links after
the six existing phases. Success writes compatibility receipt-v5, input
observation-v2 and portable measurement-v3. The portable closure includes
exactly 21 additional `sdk-` members: the source binding, link receipt,
inventory, four application ELFs, four maps, two reports and eight logs.
Legacy consumer-v1 and LLVM evidence keep their existing schemas.

The rootless reader validates those bytes without opening original runner
paths. It joins reconstructed commands, logs, maps and ABI-checked ELFs to the
prior source/compiler selection, then joins the SDK digests to the aggregate.
The protected collector must independently select `native_sdk_required` from
the pinned source contract; downloaded input observations cannot disable it.
Parsing or rehashing a complete forged export does not authenticate execution.

`producer profile` accepts paired `--source-dir` and `--source-preset` options
to inspect that policy through the shared source-contract loader. The clean
checkout must match the recipe's source commit/tree, and its source preset must
select the requested compiler profile. The optional `source_policy` JSON
projection binds the exact source/profile/contract bytes and derives the SDK
requirement without reading compatibility exports. No policy is emitted when
the source options are absent. A protected producer must require this projection
and independently pin its source; a projection does not authenticate execution.

Still required before release: actual execution through this new integration,
authenticated owning-job/artifact provenance, a released runtime and the
complete selected three-host release matrix.

The existing `producer compatibility` CLI executes this gate automatically for
a source-v2 consumer and reports its SDK receipt. There is no separate opt-out
or new SDK product command. No released SDK product, physical P4 boot or
completed toolchain release is claimed by this integration.

## Focused verification

```sh
cargo test -p aros-common --lib native_consumer_contract
cargo test -p aros-cmake-engine --test sdk_include_root
cargo test -p aros-toolchain --lib native_sdk
cargo clippy -p aros-common -p aros-cmake-engine -p aros-toolchain --all-targets -- -D warnings
```

Counterprobes cover wrong ABI, foreign linker inputs, declared-library compiler
fallback, changed evidence, stale source binding, unsafe temporary inputs and
pre-existing output roots. CMake tests also preserve literal `$<...>` paths
without generator-expression substitution and reject stale bindings after
failed reconfiguration.
