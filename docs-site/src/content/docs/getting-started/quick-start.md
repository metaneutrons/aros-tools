---
title: First checkout and build
description: Create a checkout, verify the selected inputs, and attempt your first AROS-NX build or upstream workflow.
---

First complete [installation](/aros-tools/getting-started/installation/) and
run `aros build-tools check`. The examples assume the complete suite is on
PATH. Choose AROS-NX for the integrated product flow or pristine upstream for
source/component work. The CMake engine is embedded in the tools in both cases.

## Path A: build an AROS-NX product

This path needs usable cross-toolchain entries in the checkout's lock. A source
clone alone does not install compilers or establish that every target is qualified.

Create and enter the AROS-NX checkout:

```sh
aros source init ~/Source/AROS-NX \
  --upstream https://github.com/metaneutrons/AROS-NX.git
cd ~/Source/AROS-NX
```

Inspect the exact checkout and release contracts, then install and verify the
locked inputs for one target:

```sh
aros info
aros toolchain list
aros setup --preset pc-x86_64
aros toolchain verify --preset pc-x86_64
```

Build and run the bounded boot test:

Install `qemu-system-x86_64` from the [prerequisites](/aros-tools/getting-started/prerequisites/)
before the test. This checker implements the PC x86 boot path only.

```sh
aros build --preset pc-x86_64
aros test --preset pc-x86_64 --timeout 20
```

The test verdict requires a positive boot milestone in the serial or exception
evidence; an empty log set, an unexplained early QEMU exit, or a timeout without
the expected evidence is a failure. Every invocation retains a new private
evidence directory below `build/pc-x86_64/boot-check` (or the root selected with
`--evidence`). Add `--offline` to setup/build only after the exact archives are
present in the verified cache and toolchain store.

## Path B: work with pristine upstream AROS

Create and enter a canonical upstream checkout:

```sh
aros source init ~/Source/AROS
cd ~/Source/AROS
aros info
```

`aros info` reports the four embedded target defaults and labels them as the
pristine-upstream contract. It does not create `aros-targets.toml` or any other
file in the checkout.

To develop through your own fork while retaining the canonical project as the
reviewed `upstream` remote, create the checkout this way instead:

```sh
aros source init ~/Source/AROS \
  --fork git@github.com:YOUR-NAME/AROS.git
cd ~/Source/AROS
aros info
```

Use upstream's own configure/MetaMake instructions for a complete product
build. The embedded profiles let `aros source sync` validate a candidate graph
without checkout-owned tools metadata, but they do not invent a cross-toolchain
lock or source compatibility patches. The standalone helpers remain directly
useful in this checkout; inspect their closed contracts before invoking one:

```sh
aros-transpiler --help
aros-genmodule --help
aros-romtool --help
aros-collect --help
aros-verify --help
```

The integrated locked cross-toolchain and translated-CMake product flow still
require the reviewed consumer metadata and source compatibility carried by
AROS-NX; they are not presented as pristine-upstream product commands. See the
[upstream workflow](/aros-tools/workflows/upstream-aros/) for component examples
and the current qualification boundary.

## Continue from here

- [Build a selected target or debug variant](/aros-tools/workflows/aros-nx/).
- [Update source safely](/aros-tools/workflows/source/).
- [Diagnose a failed setup, build or boot](/aros-tools/reference/troubleshooting/).

## Reproducible source selection

On `source init`, `--ref` accepts only an unambiguous full branch such as `refs/heads/master`, a
full tag such as `refs/tags/v1.2.3`, or an exact 40/64-digit commit OID. Short
names such as `main` are rejected rather than guessed. Any explicit ref leaves
HEAD detached at the resolved commit. Omit the option for the ordinary
clone-and-sync workflow, or attach a local branch before later synchronization.
The destination must not
already exist. Clone and recursive submodule validation occur in a sibling
staging directory, and publication is recursively synchronized and
no-clobber: a failed pre-publication initialization never leaves a partial
checkout at the requested path. A rare post-rename durability failure is
reported explicitly with an indeterminate commit state and retains the complete
tree for inspection.
