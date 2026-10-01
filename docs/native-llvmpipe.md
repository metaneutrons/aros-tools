# Native PC llvmpipe development

This is a development capability, not a claim about a published tools release
or ARM graphics qualification. The native Target-LLVM producer currently
supports `pc-x86_64`, LLVM 11 and Mesa 26. The AROS source must contain the
reviewed Gallivm inventory and genuine LLVM RTTI references; zero-valued RTTI
substitutes are rejected.

## Build contract

The transpiler admits a fingerprinted LLVM recipe, a checksum-locked archive
and its source-owned AROS patch. The generic external-CMake producer builds
44 explicit static components, installs LLVM headers, verifies the declared
products before stamping success and exposes a grouped link interface.
Its TableGen executable comes from the verified cross-toolchain and must
report LLVM 11.0.0. It never runs a target executable as a host generator.
The external archive-group contract requires CMake 3.24 or newer.

The SDK must preserve Classic configure's public platform ABI padding even
when kernel SMP is disabled. `__AROSPLATFORM_SMP__` and `__AROSEXEC_SMP__`
are different contracts; omitting the former changes Exec/pthread layouts
and makes target LLVM incompatible with the released libc++ archives.
External producers fingerprint the staged public SDK after header generation.
Header content changes, additions and deletions invalidate their archives;
unchanged reconfiguration and timestamp-only updates do not.

Gallivm (41 C and two C++ sources), llvmpipe (58 C sources), the LLVM draw
archive and the tessellator have real producer edges. The HIDD retains the
complete Gallivm archive and links the Target-LLVM interface. Runtime filenames
use the source module identity, so the loader can open `llvmpipe.hidd`.
The ISO/probe dependency closure also includes `gl.library`,
`mesa3dgl26-0.library` and its source-selected `SYS/GL.default` setting.

Rebuild the CLI, transpiler and SDK header generator after changing their
source or the engine:

```sh
cargo build -p aros-genmodule -p aros-transpiler -p aros-cli
```

From the AROS checkout, using that newly built `aros`:

```sh
aros build --preset pc-x86_64 --target hidd-llvmpipe --jobs 12 --compiler-cache off
```

Do not switch `--engine-dir` on an existing configured tree. The CLI updates
its embedded engine at the cached source path without cleaning the build.

## Opt-in guest probe

```sh
AROS_LLVMPIPE_RUNTIME_PROBE=1 aros build --preset pc-x86_64 \
  --target boot-iso --jobs 12 --compiler-cache off
aros test --preset pc-x86_64 --iso build/pc-x86_64/aros-x86_64-pc.iso \
  --require-llvmpipe-jit --timeout 120 --memory 1024
```

This additionally builds `tools-test-llvmpipe-jit`, instruments the HIDD's
`LLVMGetPointerToGlobal` call with an address-logging wrapper and runs the
shader probe through the isolated ISO's `User-Startup`. A pre-existing
`User-Startup` is rejected rather than overwritten. Neither the source checkout
nor the native SYS tree receives the test startup. The resulting image is a
test artifact; do not distribute it as an uninstrumented release image.
Set `AROS_LLVMPIPE_RUNTIME_PROBE=0` for a subsequent uninstrumented build.
This also clears a probe option previously enabled in the CMake cache.

The probe checks the llvmpipe/LLVM renderer identity, compiles and links vertex
and fragment shaders, draws a triangle, and checks its center pixel against
RGBA `64 128 191 255` with tolerance eight. GL errors and failed prerequisites
produce a failure marker, never a pass marker.

A host build is not a JIT run. Guest evidence must contain both a passing
shader/pixel result and a named, non-null MCJIT shader-function address. The
wrapper alone demonstrates address materialization, not entry into machine
code; the rendered result supplies the execution evidence. Retain the exact
ISO hash, source/tools/compiler identities and complete serial log.

The ISO test uses BIOS/GRUB under QEMU TCG, not the direct multiboot loader.
`--iso` cannot be combined with `--packages` or `--module`.
`--require-llvmpipe-jit` requires the ISO mode and fails if any proof element
is absent, a probe reports failure, or the guest raises a classified fault.
After complete proof or a definitive failure, the CLI stops QEMU's process
group and validates the final logs. Deadline expiry fails the strict test;
it is distinct from proof-triggered shutdown. The probe emits PASS only after
GL and window cleanup.
The evidence directory retains a read-only ISO snapshot and records its
SHA-256 and original canonical path together with this run's serial and
exception logs. QEMU boots the verified snapshot, not the mutable build-tree
image. A timeout alone is never a pass.

Without `--iso`, `aros test` remains the ordinary direct-multiboot smoke check.
It does not run the image's startup or qualify the shader/JIT path.
