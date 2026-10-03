# Native GNU compiler proof — 2026-10-03

This record supplements the [retained RV32/P4 reference measurements](riscv-compiler-probes-20261002.md).
The [reviewed RV2 plan](riscv-board-integration-plan.md#rv2-compiler-and-producer-contracts)
owns acceptance criteria; [#321](https://github.com/metaneutrons/aros-tools/issues/321)
owns current execution and delivery status. Local production, workspace
qualification and compiler publication are separate claims.

## Exact local inputs

The build ran on Darwin ARM64 with 12 jobs and a 10,800-second whole-build
deadline. It used the actual `aros toolchain build` native lifecycle, not a
manual configure/make invocation or copied Developer payload.

| Identity | Measured value |
| --- | --- |
| Tools commit | `eee9d247fce1702f0740f98d6b1ba75f1ca6954c` |
| Tools tree | `2291b885aab1b63547bd1285b55bdf943cf6bf52` |
| Local AROS-NX source commit | `b2366310038aa579af5591dbba01ffa0ff97842c` |
| Local AROS-NX source tree | `81cda983c0449a770935dfa16608e581d513b815` |
| Local producer commit | `4468a5e1279a15299e77e2c8666dcf54eb34728d` |
| Recipe SHA-256 | `703eb50f93df465190f7084516518ff9bbc1951eed1001dac6c55977ba812d7b` |
| Source-lock SHA-256 | `69830b2e5ef0173019f067d8e00449ef0394102f38b2b9b38c1a3d5dca205d45` |
| Profiles SHA-256 | `8e7ae9319d7a0a82ab7e669b8077d6305bb47c1574dbee641be8400cc3081a45` |
| Executor contract SHA-256 | `d9707287ce1ab011e4272ce412640835eef7807890ec9720cc3f64abc71501e3` |
| Executed local CLI SHA-256 | `15287ea79a6567e9b24060350e36855b30c6809f098ef57b015f4d077a71be73` |
| Verified 244-package Cargo generation | `ef365a72587e2dcb7971ea9480a38df708f98f55204a7fe0dd777277075cc42e` |

The profile selects `riscv64-aros`, GCC 16.2.0, Binutils 2.47,
`rva22u64`, `lp64d`, and `medany`. Twelve locked source/Python payloads were
reverified. Source-owned patch identities are:

| Patch | SHA-256 |
| --- | --- |
| Binutils 2.47 | `14d579bc846668f22d35a6c589b25f94b573c77c4445009942633cedfd07ef52` |
| GCC 16.2.0 | `0c8791ceed20437e523089ce4eade41b009a4ee1873028978821be9d8cb79eb6` |
| GMP 6.3.0 | `85251691199049174d7aca16e0e5d1f89b0e1056b32bedd038ec4d0b97fbe507` |
| Codesets 6.22 | `44d1d188b405858b780f6b8b492d37a723a3f6e2210ef38d294af597f3cd7c52` |

The source changes were explicitly approved for local functional commits only.
They have not been pushed or submitted as an AROS-NX PR. The executor is a
local development binary; this is not signed-release origin qualification.

## Native lifecycle result

The fresh build exited zero. Preflight, environment, configure, compiler,
collector and local-prefix publication all produced passing chained receipts.
Publication means the owned local installation, not GitHub publication.

| Receipt | SHA-256 |
| --- | --- |
| Preflight | `38418dc3118641f673797ae6738e1faaa7766ec3b5010ee5fe31ca694053daa2` |
| Environment | `08296273cff04048aac192220dcf0a86bd450f4fcaecb72bf48792dc7e3e8b92` |
| Configure | `08280040ab95bcbc97cbc775aaf9219cd0599b87d59b65d68e10e2bb2bdda1d3` |
| Compiler | `56d0d88e0fbd4d322187f0267c1417afd4ddb4b613b2fef013099fd03b5bcede` |
| Collector | `be29e181b5ade03614f3641b9c0acc9ec892b7d74dd0581aaffacd3164c9b17b` |
| Local publication | `6d6ebf738ca200b0342edf79bafc245777dcfb94211b274f289209e26a204110` |

Result stdout SHA-256:
`ca5805924c3df06b7318650b3ba22aa6c9b502b50962b13f02c35157be0ddf01`.
Stderr is empty. The source graph generated and installed its target sysroot;
the proof did not manually copy a Developer tree into the candidate.

Two earlier attempts remain failed or cancelled, not passing: target CFLAGS
leaked into Binutils host configure, then recursive MetaMake selected Apple's
Make 3.81 despite the qualified top-level GNU Make 4.4.1. GNU-only flag
isolation and an owned alias to the observed Make executable corrected those
distinct failures. LLVM does not use that alias or the GNU collector layout.

## Default package and independent audit

Actual `aros toolchain producer package` and `verify-package` completed.
The package is local and unpublished.

- Archive: `aros-toolchain-v2-gcc16.2.0-binutils2.47-macos-aarch64-rv64-reference.tar.xz`.
- Size: 87,110,772 bytes.
- SHA-256: `bccb5522a71ef0abfc27fe2b78d9b3adaf0e6d99f943c9b10fd98262b085e24f`.
- Closed four-file inventory: archive, manifest, checksum sidecar and SPDX SBOM.
- Payload inventory: 3,768 entries.
- Tree SHA-256: `b9041a7b35e0993d0d9475d1a819a03a28d374677c349141b7020e3dbfb56382`.

An independent streaming tar reader checked unique safe member paths, allowed
types, modes, owners, timestamps, embedded manifest equality, every payload
hash/size/link target and the canonical tree digest. A separately copied archive
changed by one byte failed both native verification (AX0602) and independent
hash verification. The native verifier's result-log SHA-256 is
`501f1c19a5d225fb2dd2986fe9edbb99de29db815a013ddde4b6d330f12b06b9`.

All ten GNU layout v2 roles resolve to executable files in the package.
Read-only `file`/`otool -L` inspection found 53 unique regular ARM64 Mach-O
files and only macOS system dependencies: libSystem, libc++ and libiconv.
A mock external-dependency counterprobe detected a deliberately supplied
Homebrew install name. Audit JSON SHA-256:
`5d695a3afc9b85f0be8e6750f649848ae4249b3b0d9cdb050f3e852849d28d01`.
This does not audit every possible indirect runtime load.

## Real final links and extracted relocation

The checked-in C/C++/assembly fixtures were rebuilt with explicit target flags.
They include AROS headers, assert 64-bit pointers, force wide integer division,
and prevent C++ allocation elision. Their actual map selects `libgcc` division
and `libstdc++` allocation members. The final Rust collector's two linker passes
produce ELF64, little-endian, machine 243, AROS OSABI 15/revision 1, type REL,
flags 5 (RVC/double-float ABI), stack alignment 16 and the selected architecture.

After extraction, both the original installation and source-owned Developer
build were temporarily moved out of reach. Fresh compilation and final linking
passed without a sysroot override; every selected map/runtime input and both
collector linker paths resolved in the extracted package. Original directories
were restored through an EXIT trap and checked afterward.

| Result | Bytes | SHA-256 |
| --- | ---: | --- |
| Original-prefix final ELF | 11,990,568 | `6188cdd0387993a9d7efae453dff4eaaeed23c5638264ad89f6e66d06bb2dec8` |
| Extracted-prefix final ELF | 11,990,560 | `a5afecdcf15486e3f966c85c5602262c509aba432f7d856cc63f7c55be62b1c8` |

The bounded shared target verifier accepted both actual files against the
manifest's target contract, SHA-256
`864a79dacf3ad1923bdbe5f56188070443d29fcc3f83146ec3daf5dd2349ca66`.
Verification JSONL SHA-256:
`b9eb9961c7aee5db40ff098356181cc800d0ff22b1f00589f59f29066d148f5c`.
The same verifier accepts the matching compilation unit in its explicitly
selected `unit` role and rejects actual wrong-floating-ABI, wrong-width and
host objects. It also rejects the matching unit when requested as a final AROS
application because its AROS ABI marking is absent.
Positive/counterprobe log SHA-256:
`d89bfc3b0b713488173ba5bf5d064b2d8f59b8c02074f2a1ca878cbf947dfa9a`.
Wrong soft-float ABI, RV32 width, x86-64 host object and an incompatible RV32
runtime archive each fail at the expected linker incompatibility and preserve
the previously valid output. Relocation log SHA-256:
`a9431ba312f66b4094c41f13b5cdeb998981191c343f2c01f860cc9d7fdfff56`.

These ELF files are not byte-identical. The C++ assertion string in `.rodata`
names the selected `stl_vector.h` at its new prefix; debug metadata also depends
on compilation location. Functional relocation is proven, not path-independent
binary reproducibility or a full compiler A/B qualification.

## Explicit remaining limits

The optional package forbidden-prefix scan failed AX0601. Generated headers,
GCC configuration/debug metadata, runtime archives, libtool/pkg-config files
and linker scripts still contain absolute build-root strings. No strict-scan
package was published. The successful default package is not a waiver of that
failure. A future release must satisfy its actual producer policy, including
any selected prefix scan, rather than silently omit a failing release gate.

This is one-host local GNU/RV64 production and final-link evidence. It does not
establish RISC-V hardware execution, constructors/JIT behavior, a complete SDK,
fresh P4 image production or public compiler support. RV3 owns the native P4
consumer; RV4 owns new tools/compiler publication and full target release
qualification. Existing PC/ARM/AArch64 release identities remain unchanged.
The accompanying issue records final workspace checks and reviewed deliveries;
neither synthetic metadata nor this report alone closes RV2.

## Independent review follow-up

The compiler build, archive and ELF measurements above remain bound to their
recorded Tools input, not to a later executable. Independent reviews then added
two boundary corrections, delivered separately in
[#335](https://github.com/metaneutrons/aros-tools/pull/335) and
[#336](https://github.com/metaneutrons/aros-tools/pull/336):

- Delayed pipe readers now distinguish finite buffered output after all writers
  close from a still-open writer. A zero-timeout Unix HUP probe permits only
  the former to drain beyond the cleanup deadline. Empty/buffered positive
  fixtures, open-writer counterprobes and a real anonymous-pipe check pass.
  Removing the HUP exception makes the buffered regression fail with exit 101.
- The private MetaMake fetch boundary rejects explicit offline/checksum policy
  overrides before invoking its source-owned helper. Both compiler families
  have parser probes; actual CLI probes leave the helper uncalled and ledger
  empty for overrides. A real helper exit 7 also leaves the ledger empty.
  Removing the argument guard makes its counterprobe fail with exit 101.

The pinned source helper already rejects unknown long options; no bypass on
that helper was observed. The argument guard makes this invariant independent
of a future helper's forwarding behavior. Neither correction changes the
compiler flags, source lock, collector layout or measured archive. They do
change executable code and tests, so the earlier workspace runs below are
historical, not acceptance of the updated executable tree. Final same-candidate
Linux/Darwin gates and reviewed deliveries are tracked on #321.

## Existing-target workspace coverage

On exact Tools tree `2291b885aab1b63547bd1285b55bdf943cf6bf52`, the Darwin
ARM64 `scripts/check-workspace.sh all` gate passed quality, Astro, 1,686 Rust
tests and all 45 CMake fixtures, including actual PC/EFI64/EFI32 GRUB host
builds. Its inputs were clean recursively initialized integration source
`0cae0b0116b6246b2175c3b8b97719a81f40dcfd` and the separate Mesa 20 oracle
`cb6974f1c3de43c6f1168d69039af7c32e56153c`. Log SHA-256:
`5b78134e16bb437a1e10e5c181e7c1085e05162de71f853870765c3c450589f1`.

Same-candidate nonpublishing Linux quality/integration
[run 37143788270](https://github.com/metaneutrons/aros-tools/actions/runs/37143788270)
completed successfully: 1,687 Rust tests, 44 executed CMake fixtures and one
explicit Darwin-only GRUB omission, 45 discovered. Complete run-log SHA-256:
`298e57d2947f39b441d644b8f18e11040550c29dc32685221e82d8d797a3f43d`.
Its quality and full `test` steps, combined with the same-candidate Darwin
quality/docs/integration gate, provide the required two-host coverage; it is
not a Linux GRUB or RISC-V compiler-release matrix claim. Final reviewed
delivery checks remain on #321.
