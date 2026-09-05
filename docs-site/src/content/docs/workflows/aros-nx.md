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

For repeatable qualification, add `--ref` with the exact reviewed source
commit; an explicit ref leaves HEAD detached. The tools' test source identity is recorded in
[`contracts/aros-source-v1.toml`](https://github.com/metaneutrons/aros-tools/blob/main/contracts/aros-source-v1.toml);
this documentation does not claim every later `main` revision is qualified.

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

## Rebuild or synchronize

Ordinary `build` reuses the configured build directory.
`build --clean --preset pc-x86_64` removes it first, including retained
evidence. `aros clean --preset pc-x86_64` only cleans that preset;
`aros clean` removes the whole checkout build tree.

For source updates, follow [source synchronization](/aros-tools/workflows/source/#synchronize-upstream).
It requires a clean tree, including ignored build outputs.
