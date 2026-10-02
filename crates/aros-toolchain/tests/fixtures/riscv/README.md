# RISC-V compiler qualification inputs

These C, C++ and assembly files are inputs for real source-bound compiler
probes, not fake test compilers or a board runtime. Compile with an explicit
`RV2_POINTER_BYTES=4` or `8`, source-selected ISA/ABI/code-model flags and the
matching AROS Developer sysroot.

The C division needs `__udivdi3` on RV32 or `__udivti3` on RV64. The C++ vector
address escapes to separately compiled assembly, preventing allocation elision.
Qualification must inspect the actual link map for the selected libgcc and
C++ runtime members, verify the final ELF contract and reject incompatible
objects and archives. Compilation alone is not a complete AROS runtime link.

Do not use an RV32 Developer tree to claim an RV64 SDK. The fixture does not
define source selection, tool versions, target support or hardware readiness.
