---
title: Choose and verify a toolchain
description: Use the checkout's measured release lock or explicitly opt into a local AROS-built prefix.
---

Run these commands from the AROS checkout you intend to build.
A tools release and a cross-toolchain release are independent products.

## Inspect the selected inputs

```sh
aros info
aros toolchain list
```

`info` reports the effective profiles and state locations.
`toolchain list` reads `aros-toolchains.lock.toml` for the current host.
A missing lock is an error; embedded target defaults do not create one.

## Install a released cross-toolchain

```sh
aros toolchain install --preset pc-x86_64
aros toolchain verify --preset pc-x86_64
aros toolchain path --preset pc-x86_64
```

Installation checks the selected archive and manifest, then publishes it into
the content-addressed store. `path` prints the verified prefix for use by
another build tool.

`aros setup --preset pc-x86_64` performs the same target installation.
`aros setup --all` attempts every configured target; a profile without a
usable lock entry stops the operation. It does not skip unsupported entries.

`--force` refreshes the archive cache; it does not authorize overwriting an
installed tree.

## Use an existing AROS-built prefix

```sh
aros toolchain verify --preset pc-x86_64 --local /absolute/path/to/crosstools
aros build --preset pc-x86_64 --toolchain-dir /absolute/path/to/crosstools
```

The prefix is checked in place, not copied. When it carries a manifest, that
manifest is authoritative. A legacy prefix is checked against the supported
layout, tools and target markers. Local validation does not prove that the
prefix is byte-reproducible or came from a published release.

The option names differ deliberately: toolchain commands and `setup` use
`--local`; `build` and `board build` use `--toolchain-dir`.

## Repeat without network access

After the required inputs are present:

```sh
aros toolchain install --preset pc-x86_64 --offline
aros build --preset pc-x86_64 --offline
```

The build also passes offline policy to source fetching. A missing compiler
archive or third-party source remains an error. Source initialization and
synchronization are separate network operations.

## Host LLVM is a separate installation

```sh
aros host-compiler install
```

This is equivalent to `aros setup` without a preset. It selects the effective
`[host_compiler]` contract and requires a SHA-256 for the host archive.
The built-in configuration names LLVM assets but has no host digests;
it is not sufficient to authorize their installation. Supply reviewed
checkout metadata when using this path.

## Know which checksums apply

| Input | Integrity policy |
| --- | --- |
| Released host or cross-compiler | Explicit release/configuration digest required |
| Ordinary AROS source fetch | Upstream checksum honored when declared |
| Strict source-fetch policy | `--require-fetch-checksums` requires complete declarations |
| Opaque recipe expanded into a fixed graph | Small reviewed capability fingerprint; drift requires a tools update |
| Explicit local compiler prefix | Checked layout and target identity; no inferred release provenance |

The transpiler does not invent package hashes. See
[standalone tools](/aros-tools/reference/standalone-tools/#aros-fetch) for
the fetch contract and [configuration](/aros-tools/reference/configuration/)
for state-directory overrides.

Source: [toolchain resolution](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/toolchain.rs)
and [host compiler installation](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/host_compiler.rs).
