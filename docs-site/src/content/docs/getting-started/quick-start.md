---
title: First checkout and build
description: Two complete, linear starts for an AROS-NX product build or a pristine upstream AROS checkout.
---

Choose one path and keep its checkout name throughout. The AROS-NX path owns
the reviewed CMake/product contracts. The pristine-upstream path deliberately
stays within commands that upstream AROS can consume without AROS-NX metadata.

## Path A: build an AROS-NX product

Create and enter the AROS-NX checkout:

```sh
aros source init ~/Source/AROS-NX \
  --upstream https://github.com/metaneutrons/AROS-NX.git \
  --ref refs/heads/main
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
aros source init ~/Source/AROS \
  --ref refs/heads/master
cd ~/Source/AROS
aros info
```

To develop through your own fork while retaining the canonical project as the
reviewed `upstream` remote, create the checkout this way instead:

```sh
aros source init ~/Source/AROS \
  --fork git@github.com:YOUR-NAME/AROS.git \
  --ref refs/heads/master
cd ~/Source/AROS
aros info
```

Use upstream's own configure/MetaMake instructions for a complete product
build. The standalone helpers remain directly useful in this checkout; inspect
their closed contracts before invoking one:

```sh
aros-transpiler --help
aros-genmodule --help
aros-romtool --help
aros-collect --help
aros-verify --help
```

The integrated `aros setup`, locked cross-toolchain and translated-CMake
product flow require the reviewed consumer metadata carried by AROS-NX; they
are not presented as pristine-upstream commands. See the
[upstream workflow](/aros-tools/workflows/upstream-aros/) for component examples
and the current qualification boundary.

## Reproducible source selection

`--ref` accepts only an unambiguous full branch such as `refs/heads/master`, a
full tag such as `refs/tags/v1.2.3`, or an exact 40/64-digit commit OID. Short
names such as `main` are rejected rather than guessed. The destination must not
already exist. Clone and recursive submodule validation occur in a sibling
staging directory, and publication is recursively synchronized and
no-clobber: a failed pre-publication initialization never leaves a partial
checkout at the requested path. A rare post-rename durability failure is
reported explicitly with an indeterminate commit state and retains the complete
tree for inspection.
