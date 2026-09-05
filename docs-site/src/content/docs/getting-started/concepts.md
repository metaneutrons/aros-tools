---
title: How the pieces fit
description: Understand the host tools, AROS source tree, compiler, and generated build before choosing a workflow.
---

## Four inputs to a build

| Piece | What it contains | Where it belongs |
| --- | --- | --- |
| AROS tools | The `aros` frontend, seven companion programs, and the embedded CMake engine | A separate tools checkout or installation |
| AROS source | Operating-system code, MetaMake recipes, and optional target overrides | Your chosen upstream or AROS-NX checkout |
| Cross-toolchain | A compiler, linker, target runtime, headers, and collector drivers | A verified release store or explicitly selected local prefix |
| Build tree | Generated CMake, SDK outputs, objects, packages, and boot evidence | `<AROS checkout>/build/<preset>` |

Your **host** is the computer running these tools. Your **target** is the AROS
system being built. For example, an Apple-silicon Mac can build the
`pc-x86_64` target. A host archive and a target profile are different choices.

## Upstream AROS or AROS-NX?

[Pristine upstream](/aros-tools/workflows/upstream-aros/) works with source
initialization, synchronization and standalone components. Its target defaults
are embedded in the tools. Full product builds still require source compatibility
work and an explicit cross-toolchain choice.

[AROS-NX](/aros-tools/workflows/aros-nx/) carries reviewed source integration and
consumer metadata for the integrated build workflow. It is the source used by
the current qualification contract. The tools do not require that directory
name; discovery inspects the source layout.

## What a preset selects

The four built-in target profiles are `pc-x86_64`, `arm-raspi`,
`rpi-aarch64` and `opensbi-riscv64`. An `aros-targets.toml` in the selected
checkout replaces that default configuration in full.

A profile describes target selectors; it does not prove that a release archive,
working driver, or bootable board image exists. See
[platform support](/aros-tools/reference/platform-support/) for those distinctions.

## Host compiler or cross-toolchain?

`aros setup` with no preset selects the managed **host LLVM compiler**.
`aros setup --preset pc-x86_64` selects an **AROS cross-toolchain**.
They are separate installation operations. The host installer requires an
explicit archive digest; the embedded host defaults alone do not supply one.

Released cross-toolchains are selected by the AROS checkout's
`aros-toolchains.lock.toml`. An explicitly supplied local AROS-built prefix
is also supported, but does not acquire release provenance by passing a probe.

## Generated output and source updates

The CMake engine is embedded in the tools and materialized under
`build/<preset>/cmake-engine`. It consumes the source tree as input.
Changing tools can therefore change the engine even if the AROS commit stays
the same.

Build output lives inside the checkout's `build/` directory.
`aros source sync` requires a clean tree, **including ignored files**.
Preserve any outputs or boot evidence you need before preparing a checkout
for synchronization.

Continue with [installation](/aros-tools/getting-started/installation/) or
[the first build](/aros-tools/getting-started/quick-start/).
