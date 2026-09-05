---
title: Cross-development
description: Reuse a matching AROS compiler and SDK for an external application, with the current integration limits made explicit.
---

You can keep application source outside the operating-system tree and use the
same cross-compiler and generated SDK. There is currently no `aros app`
command, application manifest, standalone SDK installer or application-package
frontend.

## Prepare matching inputs

From a compatible AROS checkout:

```sh
aros setup --preset pc-x86_64
aros build --preset pc-x86_64
aros toolchain path --preset pc-x86_64
```

Record the source commit, preset, tools version and compiler release identity.
The generated SDK and libraries must come from that same source/target build.
A cross-compiler prefix alone is not a complete application SDK.

## External CMake application

The configured build materializes a compiler-selection file at
`build/pc-x86_64/cmake-engine/toolchains/AROS.cmake`.
Its required inputs are `AROS_CROSS_TOOLCHAIN_ROOT` and
`AROS_TARGET_CPU`. For an application already configured for the AROS SDK,
the compiler-selection part can be passed as follows:

```sh
# Run from the configured AROS source root. Replace the application paths.
cmake -S /path/to/application -B /path/to/application-build -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE="$PWD/build/pc-x86_64/cmake-engine/toolchains/AROS.cmake" \
  -DAROS_CROSS_TOOLCHAIN_ROOT="$(aros toolchain path --preset pc-x86_64)" \
  -DAROS_TARGET_CPU=x86_64
```

This file selects the compiler, target triple and binary tools. It does **not**
export a ready-to-use application package with all SDK include paths,
libraries, module-link rules and installation targets. Supply those through
the application's reviewed AROS build configuration. Use its upstream
MetaMake build when that is the maintained integration.

Source: [CMake toolchain contract](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cmake-engine/engine/toolchains/AROS.cmake).

## Keep an application build reproducible

- Match headers and libraries to the selected source commit and target.
- Record the cross-toolchain identity; do not substitute a host compiler.
- Keep application build output separate from both source trees.
- Resolve local paths at configure time instead of committing machine-specific paths.
- Verify the produced module's linking/loading behavior on the intended target.

Use [the collector reference](/aros-tools/reference/standalone-tools/#aros-collect)
when debugging AROS link semantics, and
[platform support](/aros-tools/reference/platform-support/) before making a
host or board support claim.
