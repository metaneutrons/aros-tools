# PC BIOS ISO qualification

Date: 2026-10-02. Scope: a fresh native `pc-x86_64` SYS/GRUB/ISO build
on macOS ARM64 and BIOS boot in QEMU. Acceptance authority:
[BM3-A1 and BM3-A2, plan revision 4f66415](https://github.com/metaneutrons/aros-tools/blob/4f6641512cfd27d675e3ca8cbde228e4583706e2/docs/boot-media-plan.md#bm3-pc-bios-iso-from-a-native-cmake-build).

## Inputs

| Input | Qualified identity |
| --- | --- |
| AROS-NX source | `7d0a50971cea607daa0b196b72731895d736ef07` |
| Source tree | `77a85c8620141d2a6123bca7e7c590fc7cfa99de` |
| Tools | Released `v0.3.19`, source `b399c714560e6629b4084290ce5575513521c8e3`, release ID `401698102` |
| macOS tools archive SHA-256 | `8eb7c8e1ea40dae35943295c01b208a07b4264cf6c41e9b498f3fb0493e44625` |
| Cross-toolchain | `v0.1.4`, Clang/LLD 11, `macos-aarch64` / `pc-x86_64` |
| Toolchain manifest SHA-256 | `5623c63fe1d68fd3b222cefa63e641ceea904dbd0d2c4e58cbe3acc91b83f303` |
| Toolchain payload tree SHA-256 | `b24eaba7a1593e137f4457f1915c237163e91035107b3b92c8dc591d708781c0` |
| Media profile | `pc-bios-iso`, SHA-256 `98fe7513d3ecefce4ec96e71dba173da1162eac5ac3cdd5077ea3aefb607bddc` |
| Host tools | macOS 26.5.1 ARM64; CMake 4.4.3; Ninja 1.13.2; xorriso 1.5.8.pl02; QEMU 11.1.1 |

All 76 submodules were initialized and clean. There was no initial `build/`
or `SYS/` directory. The eight published tools were copied into a private,
read-only binary directory; no Cargo build, engine override or pre-existing
SYS output supplied the native build. Source, tools, compiler and image
identities were rechecked after the final guest run.

The tools release was independently downloaded by numeric release ID and
verified against final public URLs: exactly 40 assets, SHA256SUMS, native
manifests/SBOMs, 20 Sigstore subjects, 40 GitHub attestations and public
APT/Homebrew/AUR channels. This acceptance used the published
[v0.3.19 binaries](https://github.com/metaneutrons/aros-tools/releases/tag/v0.3.19),
not locally compiled replacements.

## Results

| Check | Normal ISO | Opt-in shader/JIT ISO |
| --- | --- | --- |
| Native build | Passed, 534.20 seconds | Passed, 513.55 seconds |
| Read-back file count | 5,669 | 5,673 |
| Image size | 252,108,800 bytes | 325,777,408 bytes |
| BIOS El Torito entry | Verified | Verified |
| QEMU limit / guest memory | 45 seconds / 1 GiB | 120 seconds / 1 GiB |
| Guest readiness | User mode reached; no classified failure or exception | User mode reached; strict JIT proof passed |

Normal ISO SHA-256:
`618e9a6ebefad491ae336ed28e95164ebc987d35cfb17c08121a0ab166982d32`.

Shader/JIT ISO SHA-256:
`6386cb55dd850596c26766d459d47e2fb5d855f71685001921696a18f596fcf6`.

The native bootstrap's Multiboot-1 header was verified at file offset 4096.
The strict graphics run reported `llvmpipe (LLVM 11.0.0, 128 bits)`, MCJIT
function `fs_variant_partial` at non-null address `000000000ca14a40`, and pixel
RGBA `64,128,191,255`. The guest supervisor confirmed successful return and
ELF unloading. The qualification completed at `2026-10-02T10:38:32Z`.

Both outer build jobs and nested CMake parallelism were explicitly limited to
4 from the first build; compiler caching was disabled. These are per-build
limits, not a claimed global jobserver. A shared budget remains a separate
[issue #313](https://github.com/metaneutrons/aros-tools/issues/313).

The qualified command shapes are:

```sh
CMAKE_BUILD_PARALLEL_LEVEL=4 AROS_LLVMPIPE_RUNTIME_PROBE=0 \
  aros build --preset pc-x86_64 --target boot-iso --jobs 4 --compiler-cache off
aros test --preset pc-x86_64 --iso build/pc-x86_64/aros-x86_64-pc.iso \
  --timeout 45 --memory 1024

CMAKE_BUILD_PARALLEL_LEVEL=4 AROS_LLVMPIPE_RUNTIME_PROBE=1 \
  aros build --preset pc-x86_64 --target boot-iso --jobs 4 --compiler-cache off
aros test --preset pc-x86_64 --iso build/pc-x86_64/aros-x86_64-pc.iso \
  --require-llvmpipe-jit --timeout 120 --memory 1024
```

The harness retained a frozen copy of each ISO before the next build changed
the output path. Each CLI test also used its own read-only ISO snapshot and
retained serial and exception evidence.
The normal ISO came from the fresh tree; the opt-in probe was added by a
subsequent incremental build in that same owned tree.

## Failure-path checks

On the exact tools release commit, the following focused tests passed:

- `aros-cmake-engine::tests::pc_boot_iso_uses_eltorito_and_the_source_module_order`:
  synthetic native producer fixtures reject missing kernel and altered GRUB
  input before an ISO is published.
- CLI `iso_cli_plans_composes_and_readback_verifies_without_device_write`:
  composition/read-back and altered-input rejection through the process boundary.
- Board `iso_is_reproducible_and_independently_read_back`: deterministic fixture
  images and rejection of a damaged image.

The actual released CLI also rejected two isolated copies of the fresh native
artifact: an unsupported manifest version and a one-byte change in the ISO.
Both exited 1 with `AR0802`; the untouched original was independently verified
again after each rejection. No source tree or original image was corrupted.

## Integration and limits

AROS-NX [PR #48](https://github.com/metaneutrons/AROS-NX/pull/48) carries the
approved LLVM/Gallivm source corrections and measured runtime pins. Its final
nine product lanes and locked-source preflight passed in
[run 36994682680](https://github.com/metaneutrons/AROS-NX/actions/runs/36994682680)
on the exact qualified source above. The PR merged normally, preserving Vanilla
ancestry, at `b171bfcc6b5942a29d03c1cf1ce1ad31b74e974a`. The protected-main
merge tree is exactly `77a85c8620141d2a6123bca7e7c590fc7cfa99de`, the tree
used for both native ISO and guest proofs. BM3-A1 and BM3-A2 are satisfied;
the separate job-budget issue remains open.

Image verification alone still reports `boot_qualified: false` and
`provenance_authenticated: false`: it proves read-back integrity and checked
source/toolchain bindings, not an authenticated image attestation or a guest
boot. The separate QEMU runs establish the boot claim above.

This evidence does not qualify UEFI, physical PC/ARM boards, Raspberry Pi or
Milk-V media, ARM graphics, general Mesa conformance, every AROS program, or
full BIOS ISO builds on Linux hosts. No installation image was published.
BM4–BM6 and the boot-media epic remain separate acceptance work.
