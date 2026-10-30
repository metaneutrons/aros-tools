# LLVM 23 target-side static component inventories

These inventories describe LLVM 23.1.0 libraries that run on an AROS target.
They are separate from the native host cross compiler and from the host
`llvm-tblgen` executable used while cross-compiling the libraries.

| Source profile | Backend | Target and LLVM host triple | Components |
| --- | --- | --- | --- |
| pc-x86_64 | X86 | x86_64-unknown-aros | 65 |
| arm-raspi | ARM | arm-unknown-aros | 63 |
| rpi-aarch64 | AArch64 | aarch64-unknown-aros | 66 |

## Dependency measurement

The measured upstream input is `llvm-project-23.1.0.src.tar.xz`, SHA-256
`ab1f0e3ec52448c33e8782eaf0422504b87c7b016b22514653ee0d8fcee479ff`.
LLVM's CMake project was configured separately for X86, ARM, and AArch64 with
the corresponding `LLVM_TARGETS_TO_BUILD`, `LLVM_HOST_TRIPLE`, and
`LLVM_DEFAULT_TARGET_TRIPLE`. The generated
`tools/llvm-config/LibraryDependencies.inc` supplied the component graph.

The dependency roots are Mesa's static LLVM roots: `bitwriter`, `engine`,
`mcdisassembler`, `mcjit`, `core`, `executionengine`, `scalaropts`,
`transformutils`, `instcombine`, `coroutines`, and `native`. The closed
inventories are the transitive library products reachable from those roots.
Virtual components contribute dependencies but no fictitious archives.

LLVM 23's generated graph currently maps the virtual `engine` component to the
interpreter while the explicit `mcjit` root retains MCJIT. Both closures are
therefore preserved instead of silently substituting the LLVM 11 inventory.

This measurement establishes component selection, not successful AROS target
compilation, Mesa linking, or runtime behavior. Those require qualification
against the exact released `aros-tools`, AROS-NX source, and compiler SDK.
