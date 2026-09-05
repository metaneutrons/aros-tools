---
title: Pristine upstream AROS
description: Use the tools beside canonical AROS while keeping the full-product compatibility boundary explicit.
---

The tools discover an AROS source tree from its layout, not its directory
name. They do not require AROS-NX metadata just to initialize or inspect a
checkout.

## Create a checkout

```sh
aros source init ~/Source/AROS
cd ~/Source/AROS
aros info
```

This uses `https://github.com/aros-development-team/AROS.git`.
To work through a fork, add `--fork git@github.com:YOUR-NAME/AROS.git`
when initializing a new destination.

## What works without source-side tools metadata

- Clone and recursively validate upstream source.
- Inspect host information and the four embedded target defaults.
- Validate and fast-forward a clean branch with `aros source sync`.
- Invoke the [standalone tools](/aros-tools/reference/standalone-tools/)
  against their supported source/input contracts.

Missing `aros-targets.toml` selects the embedded defaults. An existing file
is authoritative and must validate; it is not silently ignored when malformed.

## Full product builds

:::caution[Current qualification boundary]
The CMake engine is owned by the tools, but engine ownership alone does not
make every pristine upstream tree a qualified complete product.
The integrated flow still needs compatible source recipes/patches and a
verified cross-toolchain selection.
:::

Use upstream's documented configure/MetaMake path for a complete pristine
product. For the currently integrated tools-driven path, follow
[AROS-NX](/aros-tools/workflows/aros-nx/).

An explicit local AROS-built cross-toolchain can be inspected with:

```sh
aros toolchain verify --preset pc-x86_64 --local /absolute/path/to/crosstools
```

That validates the prefix; it does not supply missing source compatibility or
establish release provenance. The embedded host LLVM defaults also lack the
digests needed for managed host-compiler installation.

## Keep source current

```sh
aros source sync --ref master
```

The command requires an attached, clean branch and clean recursive submodules,
including ignored files. It validates one candidate in an independent
repository before a fast-forward publication.

See [source synchronization](/aros-tools/workflows/source/) for fork
identities, refusal conditions and recovery. A sync that validates target
graphs is not a substitute for compiling and booting the resulting product.
