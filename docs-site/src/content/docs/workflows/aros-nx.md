---
title: Build with AROS-NX
description: Configure an integrated product build, choose a target, and inspect the resulting PC boot evidence.
---

Start with [the installed tools](/aros-tools/getting-started/installation/),
the [host prerequisites](/aros-tools/getting-started/prerequisites/), and an
AROS-NX checkout. The checkout supplies source compatibility and compiler
selection; the tools supply the CMake engine.

## Select source and compiler

```sh
aros source init ~/Source/AROS-NX \
  --upstream https://github.com/metaneutrons/AROS-NX.git
cd ~/Source/AROS-NX
aros info
aros toolchain list
aros setup --preset pc-x86_64
aros toolchain verify --preset pc-x86_64
```

For a tested source revision, pass its exact commit with `--ref`; this leaves
HEAD detached. The tools' current test pin is in
[`contracts/aros-source-v1.toml`](https://github.com/metaneutrons/aros-tools/blob/main/contracts/aros-source-v1.toml).

## Build

```sh
aros build --preset pc-x86_64
```

The build materializes its engine in `build/pc-x86_64/cmake-engine`,
configures the generated graph, and invokes Ninja. It verifies the six required
build helpers before configuring.

Useful variations:

```sh
aros build --preset pc-x86_64 --target kernel-exec --jobs 8
aros build --preset pc-x86_64 --debug
aros build --preset pc-x86_64 --offline --require-fetch-checksums
```

A named target must exist in that source's generated graph.
Strict fetch policy can stop on upstream recipes that have no checksum
declarations; it does not fill them in.

`--engine-dir DIR` is an explicit developer override. The ordinary build
uses the engine embedded in the tools, even if the source checkout contains
another CMake directory.

## Check a PC boot

Install `qemu-system-x86_64` first, then run:

```sh
aros test --preset pc-x86_64 --timeout 20
```

Each invocation retains a private evidence directory under
`build/pc-x86_64/boot-check`. The result is based on positive milestones,
serial failures and exception evidence, not simply on QEMU's exit status.
`--packages` includes built packages; otherwise those modules are not tested.

The checker expects the PC bootstrap/kernel layout and uses the x86 emulator.
Use [board workflows](/aros-tools/workflows/boards/) and actual UART evidence
for physical targets.

## Create a BIOS-bootable PC ISO

Install `xorriso` and run from a clean AROS checkout:

```sh
aros build --preset pc-x86_64 --target boot-iso
```

The `boot-iso` target depends on the native `AROS` SYS producer and the
audited GRUB host assets. It stages their output, verifies the required
bootstrap and kernel modules, records the complete staged tree in
`build/pc-x86_64/media-build-receipt.json`, and verifies the El Torito catalog
before publishing `build/pc-x86_64/aros-x86_64-pc.iso`. A successful build
does not establish that the ISO boots. UEFI boot is not claimed.

Test the generated ISO through BIOS/GRUB rather than the direct-kernel loader:

```sh
aros test --preset pc-x86_64 --iso build/pc-x86_64/aros-x86_64-pc.iso \
  --timeout 45 --memory 1024 --evidence build/pc-x86_64/iso-check
```

The CLI boots a verified, read-only ISO snapshot and retains its digest and
serial/exception logs. ISO mode rejects `--packages` and
`--module`; without `--iso`, `aros test` remains the direct-kernel check.
For the separate experimental GLSL/JIT proof, see
[native llvmpipe development](/aros-tools/contributing/development/#exercise-the-native-pc-llvmpipe-path).

## Rebuild or synchronize

Ordinary `build` reuses the configured build directory.
`build --clean --preset pc-x86_64` removes it first, including retained
evidence. `aros clean --preset pc-x86_64` only cleans that preset;
`aros clean --all` removes the whole checkout build tree. Add `--dry-run` to
either form to inspect the exact directory first.

For source updates, follow [source synchronization](/aros-tools/workflows/source/#synchronize-upstream).
It requires a clean tree, including ignored build outputs.
