---
title: Choose and verify a toolchain
description: Use the checkout's measured release lock or explicitly opt into a local AROS-built prefix.
---

Run these commands from the AROS checkout you intend to build.
A tools release and a cross-toolchain release are independent products.

Maintainers who need to build a local compiler candidate from exact source
checkouts use the separate [native producer workflow](/aros-tools/workflows/toolchain-producer/).
That is not a substitute for installing a released compiler and never gives a
local prefix release provenance.

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

## Inspect the local store without running a toolchain

```sh
aros toolchain inventory
aros toolchain inventory --format json
```

Unlike `toolchain list`, `inventory` does not need an AROS checkout. It scans
the normal cross-toolchain store, or an explicitly supplied absolute
`--store DIR`, and reports every legacy release envelope and managed local
import it can inspect. The scan reads only bounded fixed-layout metadata: path
selectors, the completion marker, embedded manifest and, for an import, its
ownership receipt. It does not download, measure the payload tree, or run a
compiler or collector.

The result therefore separates **valid metadata** from integrity,
compatibility, provenance and qualification. `not-checked` and `unknown` are
honest states, not failures that `inventory` silently turns into success. A
malformed envelope is reported alongside the other entries. The default scan
budget is 10,000 fixed-layout entries; a truncated or incomplete coverage state
is explicitly marked and is not proof that the store is complete. Increase the
budget deliberately when needed:

```sh
aros toolchain inventory --max-entries 25000 --format json
```

The command is read-only. It does not alter a project's toolchain selection or
make anything eligible for cleanup.

## Import or register a local candidate

If you have a self-describing toolchain prefix produced by compatible AROS
tooling, inspect the import first:

    aros toolchain import --source /absolute/path/to/crosstools --store /absolute/path/to/store

The preview copies the candidate only into private temporary staging. It
measures the source through no-follow descriptors, checks the embedded manifest
against a fresh canonical inventory, and prints an apply token. It does not
execute a compiler, select the candidate for a project, or publish anything.
Commit only that exact preview:

    aros toolchain import --source /absolute/path/to/crosstools \
      --store /absolute/path/to/store --apply TOKEN_FROM_PREVIEW

The managed envelope has a deterministic identity derived from its verified
manifest and payload-tree digests, is no-clobber, and contains an ownership
receipt published atomically with the copied payload. An imported candidate is
local evidence only; it is not a release, an attestation, or a replacement for
the checkout's release lock.

To retain non-owning evidence for an existing prefix without copying it:

    aros toolchain register --source /absolute/path/to/crosstools --store /absolute/path/to/store
    aros toolchain register --source /absolute/path/to/crosstools \
      --store /absolute/path/to/store --apply TOKEN_FROM_PREVIEW

Registration records the external path and observed identity but never owns,
updates, selects, or removes it. Both commands require absolute paths and
refuse changed sources or mismatched tokens. Registration does not make the
prefix selectable: use an explicit `--local` path for that existing workflow.

## Select a released lock for one project

Selection changes only the `aros-toolchains.lock.toml` in the current AROS
checkout. Start from an explicit, absolute TOML release lock:

    aros toolchain select --release-lock /absolute/path/to/release.lock.toml \
      --store /absolute/path/to/store

The preview validates a coherent v1 lock: its HTTPS release base URL must bind
the declared release ID, every checkout target profile must be covered by the
same host matrix, and each target triple must match the checkout contract. It
then reports the old and new lock identities plus an apply token. Commit only
that exact plan:

    aros toolchain select --release-lock /absolute/path/to/release.lock.toml \
      --store /absolute/path/to/store --apply TOKEN_FROM_PREVIEW

The command holds store and project locks in a fixed order and publishes the
project lock as a no-clobber creation or an identity-and-digest CAS replacement.
It refuses a changed source or project lock. It neither downloads an archive
nor proves that a supplied lock was published or attested; use the normal
release verification workflow for that evidence. Imported and registered local
prefixes remain unavailable to selection.

Removal and garbage collection remain unavailable until their own M8 safety
gates are implemented.

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
