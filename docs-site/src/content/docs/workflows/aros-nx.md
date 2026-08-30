---
title: AROS-NX
description: Use the optional translated build engine and locked cross-toolchains.
---

AROS-NX adds a consumer bridge around upstream AROS. The operating-system tree
selects target and toolchain policy; `aros-tools` provides the host executables.

From the AROS-NX checkout:

```sh
/path/to/aros-tools/target/release/aros toolchain install --preset pc-x86_64
/path/to/aros-tools/target/release/aros build --clean --preset pc-x86_64
```

The build frontend verifies its toolchain lock, translates current MetaMake
contracts transactionally and leaves caches and generated build state outside
authoritative sources by default.

The first local migration qualification completed the complete `pc-x86_64`
graph with the published deterministic RC3 toolchain: 14,204 Ninja steps,
including C++ runtimes, Mesa, external CMake projects, AHI and the final kernel
bootstrap. This is build evidence, not physical-board boot evidence.
