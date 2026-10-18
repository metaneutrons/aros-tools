# LLVM 11 target-side static component inventories

These inventories describe LLVM libraries that run on the selected AROS
target. They are separate from the native host cross compiler and its host
`llvm-tblgen` tool. The source recipe selects `LLVM_TARGETS_TO_BUILD` using
`LLVM_TARGET_CPU` and the default triple using `AROS_TARGET_CPU`.

| Source profile | Backend | Target and LLVM host triple | Components |
| --- | --- | --- | --- |
| pc-x86_64 | X86 | x86_64-unknown-aros | 44 |
| arm-raspi | ARM | arm-unknown-aros | 45 |
| rpi-aarch64 | AArch64 | aarch64-unknown-aros | 45 |

The explicit LLVM host triple names the platform on which the resulting LLVM
libraries run. LLVM 11's `cmake/config-ix.cmake` derives native MCJIT
initialization from it; using the build machine's architecture here would be
incorrect for a cross build.

## Dependency measurement

The measured upstream input is `llvm-11.0.0.src.tar.xz`, SHA-256
`913f68c898dfb4a03b397c5e11c6a2f39d0f22ed7665c9cefa87a34423a72469`.
Its own `utils/llvm-build/llvm-build` was executed separately for X86, ARM and
AArch64, with matching native-target and enabled-target selections, to emit
the component dependency tables used by LLVM's CMake integration and
`llvm-config`. No LLVM compiler or target library was built for this measurement.

The roots are Mesa's static MCJIT components: bitwriter, engine,
mcdisassembler, mcjit, core, executionengine, scalaropts, transformutils,
instcombine, coroutines and native. Mesa 26.0.0's upstream `meson.build`
declares these dependencies for LLVM draw/llvmpipe; its archive SHA-256 is
`2a44e98e64d5c36cec64633de2d0ec7eff64703ee25b35364ba8fcaa84f33f72`.
The newer-LLVM optional LTO path and unrelated AMD/CLC/ORCJIT features are not
part of this LLVM 11 MCJIT contract.

Dependency expansion traverses the enabled upstream component table, rejects
missing or disabled dependencies, deduplicates visited components and emits
the explicit static library products. Virtual components such as engine and
native contribute dependencies, not fictitious archives. The X86 result
reproduces exactly the existing 44-library set. Each ARM result contains 45
libraries, including its own backend Utils library; substituting the X86 list
does not match either result. Link order may differ between valid traversals;
all declared products remain in the existing static rescan group.

These measurements establish component dependency selection, not successful
target compilation, complete Mesa linking, SDK generation or runtime behavior.
Those still require qualification on the exact released candidate. Recipe and
patch checks, host-tool version checks, archive readback and explicit product
inventory enforcement remain in place.
