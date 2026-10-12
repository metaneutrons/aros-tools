---
title: Manage an AROS Developer SDK
description: Generate, select and use a target-specific AROS SDK with its compiler kept as a separate artifact.
---

An AROS cross-compiler and an AROS Developer SDK are different inputs. The
compiler is a host executable toolchain; the SDK is a source-built set of target
headers and libraries for one target and ABI. A compiler prefix alone is not a
complete application SDK.

## Availability and identity

The `aros sdk` commands described here are development workflows in the current
`aros-tools` source. They do not imply that a public SDK release is available.
The initial SDK set is not published yet:

| Target | Compiler profile | Initial SDK version |
| --- | --- | --- |
| PC x86-64 | `pc-x86_64` | `0.1.0` |
| Raspberry Pi ARM32 | `arm-raspi` | `0.1.0` |
| Raspberry Pi ARM64 | `rpi-aarch64` | `0.1.0` |
| ESP32-P4 RV32 | `rv32-esp32p4` | `0.1.0` |

Each SDK has its own source-bound identity and version, independent of the
compiler release and build host. All four SDKs are intended to ship with the
compilers in one `aros-toolchains` release. See [release status](/aros-tools/reference/release-status/)
for published artifacts and the
[SDK artifact contract](https://github.com/metaneutrons/aros-tools/blob/main/docs/sdk-artifact-contract-v2.md)
for the identity and release boundary.

Before publication, each SDK must be generated with every selected compiler
profile on all three build hosts. Its package bytes must match across hosts;
the pipeline must also verify the original CI job for each generation.

## Acquire and install an explicitly selected SDK

External release-lock operations require an independently selected SHA-256 of
the exact lock-file bytes. There is no `latest` selector or implicit release
discovery:

```sh
aros sdk download --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256
aros sdk install --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256
aros sdk verify --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256
aros sdk path --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256
```

Replace the example paths and `REVIEWED_SHA256` with values selected by your
application or release process. Alternatively, `download`, `install`, `verify`
and `path` accept an explicit application project lock:

```sh
aros sdk install --project-lock /absolute/path/to/application/sdk.lock.json
aros sdk verify --project-lock /absolute/path/to/application/sdk.lock.json
aros sdk path --project-lock /absolute/path/to/application/sdk.lock.json
```

A project lock is application-owned configuration, not publisher
authentication. Keep one lock per target/ABI; multi-target applications use
separate files. To create or replace one, `select` previews an external lock
against an already installed SDK and writes nothing until given the exact
preview token:

```sh
aros sdk select --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256 \
  --project-lock /absolute/path/to/application/sdk.lock.json
aros sdk select --release-lock /absolute/path/to/sdk-release-lock.json \
  --lock-sha256 REVIEWED_SHA256 \
  --project-lock /absolute/path/to/application/sdk.lock.json \
  --apply PREVIEW_TOKEN_SHA256
```

Replace `PREVIEW_TOKEN_SHA256` with the confirmation token printed by the
first command, not with the release-lock digest.

`download` verifies the locked archive, manifest and complete payload without
installing; `install` verifies before publishing into the SDK store. `--offline`
uses only verified cached bytes. Installation can also use one explicit local
archive with `--archive FILE`. `verify` checks the complete installed payload, while
`path` prints its verified root; neither repairs an installation or changes a
build configuration. `aros sdk list` provides bounded store metadata only; it
does not hash or execute payloads. The default store is `AROS_HOME/sdks`.

```sh
aros sdk list
```

For local package verification rather than managed installation,
`verify-package` requires `--identity-sha256`, `--tree-sha256`,
`--archive-sha256`, `--archive-size` and at least one `--forbidden-prefix`, in
addition to the selected identity and package directory. See the
[CLI reference](/aros-tools/reference/cli/#developer-sdk-artifacts-and-management)
for the exact extraction selectors as well.

## Use the SDK with CMake

`aros sdk path` reports an installed SDK root; it does not configure CMake or
select include and library directories. For AROS source builds, those paths
come from the checked-in source SDK contract. For an external application,
provide the verified SDK through that application's reviewed build
configuration. The AROS CMake toolchain selects the cross-compiler and binary
tools, but does not by itself supply an application SDK integration. See
[cross-development](/aros-tools/workflows/cross-development/) for the current
integration boundary.

## Generate from a selected source

`aros sdk verify-source` checks an explicit SDK identity and its SHA-256 against
a clean checkout, its source projection and one source preset. It does not
build anything. `aros sdk build` then generates the selected SDK, runs the six
compatibility phases and four original/relocated C/C++ links, and packages the
result. The source contract selects GNU or LLVM; it is not inferred from a
board name.

Generation requires exact compiler, source, cache and execution inputs. See
the [SDK CLI reference](/aros-tools/reference/cli/#developer-sdk-artifacts-and-management)
for all selectors and the [producer workflow](/aros-tools/workflows/toolchain-producer/)
for preparation. `--evidence-dir` retains the portable local checks alongside
the generation record; neither output authorizes publication.

`aros sdk verify-generation` reads an existing generation record, compiler
package, SDK package and complete portable compatibility evidence. The release
input collection selects the product and compiler; a separate digest-bound
selection supplies the expected bytes and local paths. This command never
rebuilds the SDK or reopens paths recorded on the original build host. It checks
byte consistency, not the identity of the CI job that produced the files.
An authenticated release pipeline must establish that origin separately.

The source-owned CMake contract selects the generated SDK path and link
probes. `AROS_SDK_INCLUDE_DIR` is a source-build output, not an installed-SDK
selector. A completed local build still needs the authenticated shared
release pipeline before its SDK can become a published product.
