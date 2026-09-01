---
title: Cross-development
description: Use the exact AROS SDK and cross-toolchain selected by a checkout to build an external application.
---

`aros-tools` can provide the same compiler, sysroot and generated SDK used by an
AROS product build. It does not require application source to live inside the
AROS repository.

## Prepare the SDK

From the selected AROS checkout, install and verify the target toolchain, then
build the SDK-producing targets:

```sh
aros setup --preset pc-x86_64
aros build --preset pc-x86_64
aros toolchain path --preset pc-x86_64
```

The last command prints the verified cross-toolchain prefix. Treat that path as
machine output: do not replace it with a similarly named compiler found on
`PATH`.

The configured build directory contains the generated headers, libraries and
CMake metadata for that exact source revision and target. An application build
must keep these inputs together; combining headers from one checkout with a
toolchain or libraries from another is unsupported.

## External CMake application

Use the toolchain file and SDK paths emitted by the selected AROS build. Keep
the application build directory outside both source trees:

```sh
cmake -S /path/to/application -B /path/to/application-build -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE=/path/reported/by/the/AROS/build
cmake --build /path/to/application-build
```

Exact variable names differ between pristine upstream MetaMake applications
and the translated AROS-NX CMake bridge. Prefer an application's checked-in
build instructions over reconstructing compiler flags manually.

## Reproducibility checklist

- Record the AROS source commit and target preset.
- Record the toolchain release ID and manifest SHA-256 reported by the lock.
- Keep generated headers and libraries from the same configured build.
- Use `--offline` in repeat builds when all inputs are already verified.
- Never commit absolute local toolchain or build paths to a portable project.

An application-specific packaging frontend is not yet part of the public CLI;
`aros-tools` supplies and verifies the build inputs rather than inventing an
application manifest format before upstream compatibility is established.
