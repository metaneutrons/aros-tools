---
title: AROS-NX
description: Use the optional translated build engine and locked cross-toolchains.
---

AROS-NX adds a consumer bridge around upstream AROS. The operating-system tree
selects target and toolchain policy; `aros-tools` provides the host executables.

Create a checkout explicitly, or enter an existing one:

```sh
aros source init ./AROS-NX \
  --upstream https://github.com/metaneutrons/AROS-NX.git
cd ./AROS-NX
```

From the AROS-NX checkout:

```sh
/path/to/aros-tools/target/release/aros toolchain install --preset pc-x86_64
/path/to/aros-tools/target/release/aros build --clean --preset pc-x86_64
```

The build frontend verifies its toolchain lock, translates current MetaMake
contracts transactionally and leaves caches and generated build state outside
authoritative sources by default.

The repository contract in `contracts/aros-source-v1.toml` records the exact
AROS-NX source revision and toolchain-producer revision used by this tools
commit. CI validates that contract before tests; it does not silently follow a
moving `main` branch. Product qualification evidence belongs to the producer
run and release inventory for the selected toolchain, not to an undocumented
local build claim.

AROS-NX can therefore carry reviewed build integration while `aros-tools`
remains usable as a standalone suite. Board support is a separate boundary:
successful product compilation is not UART or physical-boot evidence.
