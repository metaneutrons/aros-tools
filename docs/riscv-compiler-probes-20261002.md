# RISC-V compiler probes — 2026-10-02

This is a partial RV2 measurement record, not milestone acceptance. Requirements
remain in the [reviewed plan](riscv-board-integration-plan.md#rv2-compiler-and-producer-contracts);
[issue #321](https://github.com/metaneutrons/aros-tools/issues/321) owns execution status.

## Inputs and scope

Host: Darwin/ARM64. Reference port source:
`763a536f26a5e94176c821d48cedbcbff03b4189`. RV64 source inspection:
`0cae0b0116b6246b2175c3b8b97719a81f40dcfd`. Tools implementation started from
`d94f7db092d46e34f8d21f3507fdff0a068ce2b4`.

The existing P4 compiler reports GCC 16.2.0, target `riscv-aros`, and GNU
Binutils 2.47.20260726. Its configured defaults are
`rv32imafc_zicsr_zifencei_zaamo_zalrsc`, `ilp32f`, `medany`. The source-owned
RV64 selection is separately `rva22u64`, `lp64d`, `medany`.

Measured retained input and installed-tool identities:

| Input | SHA-256 |
| --- | --- |
| `gcc-16.2.0.tar.xz` | `e6738e29597f733270731aa90600f37ffdc045079dfc27ec7e8192cc81085c3e` |
| `binutils-2.47.tar.bz2` | `3068128c75cda9f898ccb4211d360246e8e195ffcc9dfb655b23ae23a54800e8` |
| Source `gcc-16.2.0-aros.diff` | `2baf43143061c7709411cf396b40e44e3ece33729c69ad73facd5c92b339740c` |
| Source `binutils-2.47-aros.diff` | `14d579bc846668f22d35a6c589b25f94b573c77c4445009942633cedfd07ef52` |
| Installed `riscv-aros-gcc` | `618ee5475a88015dfdeaabd723531f6a3aa2e4f64ef5d31c2ce4c2204142732c` |
| Installed `riscv-aros-g++` | `e9eddb3a1c3d8261d0938c17f60a130c9c4ad5508be2385e0fde0287117b9a22` |
| Installed `riscv-aros-ld` | `2dbab4d314368a8cb6d4399f6cd8388cb04000b5cde38cdb0da27750e07c1a82` |
| Installed `riscv-aros-readelf` | `101bc902e401250490bcace260653ce1de874c8b18a84573cc1b468a25f1124e` |
| RV32 `libgcc.a` | `3febaf8629566ee3361cf196ae59477952e633e473980fd1f7416842d7f9d13b` |
| RV32 `libstdc++.a` | `34da60c67a953ad431a710a6e2dd517cd5aee91bc5eac1517bfc89043261c8b3` |
| RV32 `libsupc++.a` | `ffb8d57b81ad65acac54b991554033f5c30b3ede0f1f2ee6d7dc4c2620c4f401` |

Retained GNU host-component archives, measured rather than inferred from their
names:

| Archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `gcc-16.2.0.tar.xz` | 107,200,820 | `e6738e29597f733270731aa90600f37ffdc045079dfc27ec7e8192cc81085c3e` |
| `binutils-2.47.tar.bz2` | 40,570,778 | `3068128c75cda9f898ccb4211d360246e8e195ffcc9dfb655b23ae23a54800e8` |
| `gmp-6.3.0.tar.bz2` | 2,643,888 | `ac28211a7cfb609bae2e2c8d6058d66c8fe96434f740cf6fe2e47b000d1c20cb` |
| `isl-0.27.tar.bz2` | 2,417,649 | `626335529331f7c89fec493de929e2e92fb3d8cc860fc7af554e0518ee0029ee` |
| `mpfr-4.2.2.tar.bz2` | 1,753,999 | `9ad62c7dc910303cd384ff8f1f4767a655124980bb6d8650fe62c815a231bb7b` |
| `mpc-1.4.1.tar.xz` | 531,992 | `91204cd32f164bd3b7c992d4a6a8ce6519511aadab30f78b6982d0bf8d73e931` |

These retained inputs, patches and binaries do not prove a fresh compiler build
or reproducible provenance. No AROS source was edited for these probes.

## RV32 reference links

Fresh C and C++ translation units include AROS `exec/types.h` and assert
32-bit pointers. Assembly supplies an externally referenced function. Explicit
ISA, ABI, code model and Developer sysroot are passed to each compilation and
the final GNU driver link.

The C unsigned-64-bit division pulls `libgcc.a(_udivdi3.o)`. A separate
`std::vector<ULONG>` probe passes its storage address to an opaque assembly
function, preventing allocation elision. Its map selects
`libstdc++.a(new_op.o)` and other C++ runtime members. The initial optimized
vector-only probe did not select those members and is not counted as runtime
library-link evidence.

| Output | Bytes | SHA-256 |
| --- | ---: | --- |
| Basic C/C++/assembly AROS link | 94,316 | `3110522a4a1cbc42f301c196a10a81548ab664529b68044eb7ee0524f720e932` |
| Allocation-preserving C++ AROS link | 3,191,172 | `677521891849fdbd59179b126faa2c2d97f2e101f6a6a0119fd542f8fb8a01aa` |

Both final files are ELF32, machine 243, flags 3, AROS OS ABI 15/revision 1,
and ELF type `REL`. AROS's final application link is relocatable; requiring
`EXEC` would reject this real result. Measured architecture attribute:

```text
rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0_zca1p0_zcf1p0
```

Actual negative links exit nonzero for incompatible RV32 double-float ABI,
RV64 width, and a host i386 ELF object. An isolated archive containing an
ELF64 RISC-V member also exits 1 with the expected ELFCLASS64/ELFCLASS32
diagnostic. These are link failures, not inferred failures from filenames.

## Relocation result and defect

A copied compiler prefix and copied Developer tree compile all three inputs
again. With identical assembly input basenames, the final C++ ELF is exactly
byte-identical to the reference above. The linker map's 40 `LOAD` entries are
under the copied roots: 34 Developer, three compiler and three fresh object
inputs. The earlier seven-byte difference was solely the assembly filename
in `.strtab`, not generated instructions or runtime selection.

**This is not a self-contained compiler relocation.** The driver still has its
original absolute default sysroot; the probe overrides it explicitly. The GNU
`collect-aros` binary also embeds original tool paths: the rejected archive
link names the original linker. A relocated target-input map therefore does
not establish relocated host-tool execution. The producer must eliminate this
dependency before RV2-A2 can pass.

Mach-O dependency inspection also finds `/opt/homebrew/opt/zstd/lib/libzstd.1.dylib`
in the copied GNU linker. GCC and `cc1` list only macOS system libraries.
Replacing the collector's absolute linker path does not eliminate this separate
host-library dependency. A fresh packaged host-tool closure must address it;
the present installed compiler is not a self-contained distribution.

Retained relocation trace SHA-256:
`499b977744a9dd17cc547110eac077e841cba1abbc6fdb2b814e830d7566f832`.
Incompatible archive trace SHA-256:
`5b6973f7daac09a2143ea33702ffad57a2cb1534258f7451072d46ad9fee5929`.

### Rust collector interoperability

The copied GCC driver resolves `collect-aros` in its target `riscv-aros/bin`
directory, not the flat prefixed alias. A first attempt that replaced only the
flat alias produced no Rust diagnostic log and is not counted as evidence.
Replacing the actual target-directory executable and declaring adjacent GNU
`ld` and `strip` in `aros-collector-tools-v1` makes both link stages select
that copied linker. The manifest explicitly maps GCC's measured
`elf32lriscv` driver selection to the patched `riscvelf_aros` linker emulation;
the collector does not guess an emulation from a target name.

The same C/C++/assembly inputs linked with this Rust collector to an AROS
ELF32/REL application with flags 3 and the exact RV32 architecture attribute
above. The 3,193,172-byte result has SHA-256
`002350edb92bb9bb623940b5d14da63d93bcea04b0ab1137f3bbb2348bfd7291`.
The actual child-tool log has SHA-256
`f5fcc944af479dc845f4f66b162ea5113f7e3a6f637280e3690479ba9c5ab645`.
The checked-in qualification fixtures independently compiled and linked:
3,193,168 bytes, SHA-256
`57b9eaaaffc0265216136d548a2434e39c9dd90aa3d80e5f0d63df2a03d2ee02`.
Both maps select startup, `libgcc.a(_udivdi3.o)` and
`libstdc++.a(new_op.o)` from the chosen copied roots, and both final ELFs pass
the bounded target-contract reader.

Real wrong-float, wrong-width, host-i386 and forced incompatible runtime-archive
links each fail for the intended GNU linker incompatibility. All four failures
leave a pre-existing valid output byte-identical; diagnostic records identify
the copied linker in every failed first stage. Combined negative child-tool
log SHA-256:
`8ec48957bdd3aa0f2579f4020d25165645a8a136c37ede6839adc9f91057ff6f`.
Subprocess regressions separately reject ambiguous emulation operands and
duplicate output options before invoking any tool.

The final collector review found and corrected a prefixed-symlink fallback:
only exact legacy invocation names may use the manifest-free LLVM layout.
An actual prefixed symlink fails before either sibling tool runs and preserves
the existing output. The deployed legacy `collect-aros -> aros-collect` alias
still works. All 58 collector unit/process/observability tests and strict
Clippy passed; independent follow-up review found no remaining blocker in that
slice. The real positive link and all four negative links above were repeated
after this correction and retained the same output and trace digests.

A real `-s` link with the checked-in fixtures also passed the final AROS target
contract: 377,940 bytes, SHA-256
`f7487c993165581616e494071aafdf748273f5f16911f3cb7817c476c5db7e36`.
This is a measured stripped link, not runtime execution evidence.

This output is **not byte-identical to the C collector's result**. The existing
Rust set script puts symbol sets in `.aros.sets` rather than the C collector's
combined `.rodata` and leaves additional GNU orphan sections separate. This
measurement proves link and ELF-contract interoperability, not runtime
equivalence, C++ constructor execution, boot or a self-contained packaged host
closure. The external `libzstd` dependency remains unresolved.

## RV64 frontend/linker probe only

The measured GCC backend independently compiled 64-bit C and C++ with
`-march=rva22u64 -mabi=lp64d -mcmodel=medany`. The patched linker accepted
`-m riscv64elf_aros -r` for these objects and 64-bit assembly. The guessed
`elf64lriscv_aros` emulation was rejected; the supported emulation was measured
using `ld -V`.

The combined object is 2,584 bytes, SHA-256
`4e570520df8bf40e460f9ada68286f27da874968774d3fe3152ce291881c6015`:
ELF64, machine 243, flags 5, type `REL`, OS ABI 0. This is **not** an AROS
application or a verified RV64 runtime. Its 128-bit division still needs the
separately built, ABI-matching target runtime. No RV64 Developer link or
runtime execution is claimed.

## Measurement implementation

`aros-common::elf` measures class, machine, type, flags and OS ABI directly.
Its RISC-V reader obtains the attributes from the same ELF bytes; callers
cannot combine one parsed object with another byte stream. Header versions,
sizes, section/symbol spans and copied-name budgets are checked before
untrusted table counts drive allocation. The reader supports little-endian
objects and u16 section indices. The inspection example emits structured JSON
and can apply an explicitly selected `aros-riscv-target-v1` contract. It checks
width, machine, floating ABI, stack alignment, architecture and declared ABI
attributes, and distinguishes an intermediate unit from an AROS-marked final
relocatable application. This is conformance to supplied expectations, not a
decision that a board or compiler release is supported. The code model still
requires a real compile/link probe; it is not recorded in these ELF attributes.

The RV32 target-contract probe passed on the allocation-preserving AROS link.
The RV64 contract passed on the intermediate object and correctly rejected it
as a final AROS application. Actual wrong-width, double-float and host-machine
objects were rejected. New stack, RVE, unknown flags, RVC/TSO, atomic ABI and
global-pointer usage counter-probes cover the pure validator. A zero/missing
atomic or global-pointer annotation remains unknown, not a runtime guarantee.
Attribute semantics follow the [RISC-V ELF psABI specification](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/).

Standalone target declarations use the bounded `TargetContract::parse` entry
point. Embedded Serde callers must bound their enclosing input before decoding;
semantic validation alone cannot bound the raw stream before field allocation.

Synthetic parser counter-probes and real GNU objects serve different roles:
the former test corruption handling, the latter supply measured compiler
evidence. ISA strings are not proof of execution on hardware.

## Strategy and remaining qualification

Use the demonstrated GNU family as the starting implementation for these two
RISC-V contracts; do not upgrade the existing LLVM PC/ARM/AArch64 releases.
Bind source family/version/patches once in the source lock, preserve legacy
LLVM readability, and propagate explicit tool roles and target/runtime facts
through producer, collector, package, verification and managed consumers.

The partial implementation accepts source-lock v3 with explicit GNU components
and profiles v2 with embedded RISC-V target expectations. Profile clones retain
their family; native binding rejects mismatched source/profile families, and
LLVM v1 package/read-back paths reject GNU inputs in either role. These checks
prevent accidental LLVM routing; they do not implement GNU execution or
packaging. The reusable C/C++/assembly qualification inputs are retained in
`crates/aros-toolchain/tests/fixtures/riscv/` with an explicit pointer-width
selector and allocation-preserving runtime probe.

Read-only source inspection identifies an incomplete GNU release graph in
AROS-NX: `crosstools-release` expects GNU release runtime targets that are not
defined, the stage compiler reaches aggregate includes, and the minimal
five-linklib closure is currently ordered only for LLVM. The broad GNU
`LIB_SPEC` requires actual runtime configure/link evidence; the LLVM closure
cannot be assumed sufficient. Targeted source changes require separate approval.

Still required: fresh source-bound GNU compiler/runtime builds, a separate
RV64 AROS runtime/sysroot link, family-aware native execution and package
contracts, self-contained host-tool relocation, existing-target integration
regressions and complete RV2 acceptance evidence. No compiler release, full
SDK, board image, boot or physical-device success is established here.
