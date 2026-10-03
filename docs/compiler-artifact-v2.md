# Compiler-family artifact contract

RV2 implementation slice; not compiler qualification or release authority.
Requirements remain in [the RV2 plan](riscv-board-integration-plan.md#rv2-compiler-and-producer-contracts).

## Formats

Existing schema-1 locks/manifests and LLVM asset names remain readable.
Schema 1 does not accept a `compiler` field, including `null`. Its legacy
`llvm_version` semantics remain unchanged.

Schema-2 locks/manifests require a non-null `compiler` record and prohibit
`llvm_version`, including `null`. The compiler record is closed:

| Family | Required fields |
| --- | --- |
| `llvm` | `family`, `version` |
| `gnu` | `family`, `gcc_version`, `binutils_version`, `target` |

`target` is the closed `aros-riscv-target-v1` contract. GNU's target triple must
name the corresponding `riscv` or `riscv64` CPU and end in `aros`; it is not a
board identity. GCC/LLVM versions have three numeric components; Binutils
has two through four. Schema-2 versions have at most ten digits per component,
with values from 0 through 2,147,483,647. GNU source-lock v3 uses the same
version validator, so an accepted compiler input cannot exceed its artifact's
version grammar. Legacy LLVM source-lock validation is unchanged. Unknown,
duplicate, mixed-family and missing fields are rejected. Selecting an ISA or
code model is not measured compiler execution.

Source locks and cloned selected profiles retain the SHA-256 of their exact
parsed bytes. A GNU package must match both recipe-bound digests and the exact
patch set. Reserialization or whitespace equivalence cannot substitute another
document. Read-back verifies compiler, linker version and the complete target
contract in addition to the existing source/producer/tools identities.

## Packaging

LLVM retains its existing schema-1 manifest and asset names. GNU's separate
schema-2 name is:

```text
aros-toolchain-v2-gcc<GCC>-binutils<BINUTILS>-<HOST>-<PROFILE>.tar.xz
```

`PROFILE` comes from validated producer data, not a board-name enum. GNU
packaging accepts only Linux x86-64, Linux ARM64 and macOS ARM64 hosts. The
same deterministic tar/XZ writer, closed four-file package inventory, embedded
manifest, SHA sidecar, SPDX source graph and bounded read-back/extraction
implementation serve both families. Mixed source/profile families fail before
filesystem mutation.

## Installed GNU executable layout

A GNU candidate must include `toolchain-tools.json` in its payload inventory.
The closed `aros-toolchain-tools-v1` document contains the exact `compiler`
record, `target_triple`, and eight `tools` paths: `c`, `cxx`, `assembler`,
`linker`, `archive`, `ranlib`, `strip`, and `collector`. It contains no commands
or environment variables. Paths are portable, bounded and relative to the
payload root; flat GNU prefixes and nested target-tool directories are both
valid. Internal executable aliases retain their declared invocation name.
Missing roles, non-executable files and links outside the payload are rejected.

Writer, archive read-back, extracted-tree and consumer checks bind the layout
to the compiler identity and inventory. A GNU release lock must require both
the layout document and every declared role path. Consumers require explicit
`transpiler.toolchain = "gnu"` and a matching `float_abi` in the selected
checkout profile. The public `riscv32` CPU selector maps to AROS's GNU `riscv`
triple spelling; no board identity participates in this mapping. Legacy LLVM
profiles, schema-1 inventories and filenames are unchanged.

Installation and `toolchain verify` resolve only the declared payload paths.
Both GNU frontends must report the exact target and GCC version through
`-dumpmachine` and `-dumpfullversion`; all required executables have bounded
`--version` probes. These checks do not prove ISA code generation, target
runtime completeness, dynamic host-library relocation or a native AROS build.

## Qualification boundary

Synthetic RV32/RV64 package fixtures test repeatable bytes, extraction and
identity-substitution rejection. They are deliberately not compiler binaries.
Real compile/link, fresh compiler/runtime production, self-contained host
relocation and target-runtime verification remain separate requirements.

The native GNU CLI producer, compatibility runner and release-index rollout
are not complete. Managed GNU installation accepts a bound schema-2 payload;
it does not create one or supply a missing compiler release. Schema-1 release
assembly rejects GNU matrices instead of silently publishing LLVM metadata.
No new compiler release, Developer SDK, board boot or device deployment follows
from accepting schema-2 metadata.
