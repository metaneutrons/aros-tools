---
title: Prerequisites
description: Prepare a macOS or Linux host and install only the tools your workflow needs.
---

## Supported hosts

| Host | Native archive target | Binary compatibility floor |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | glibc 2.36 |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | glibc 2.36 |
| macOS Intel | `x86_64-apple-darwin` | macOS 13 |
| macOS Apple silicon | `aarch64-apple-darwin` | macOS 13 |

These are the native release targets. Public availability is listed separately
under [release status](/aros-tools/reference/release-status/).
Windows is not a supported native host.

## Build and use the tools

Install Rust through your normal Rust toolchain manager. The repository's
`rust-toolchain.toml` selects Rust 1.98.0 automatically when you run Cargo.
You need Git and a working native compiler/linker to build the Rust workspace.

AROS workflows additionally use CMake, Ninja, Python 3.11 or newer, curl,
a POSIX `patch`, and system CA certificates. Git manages sources; CMake and
Ninja build products; the fetcher may invoke curl and patch; the independent
verifier invokes upstream's Python GenMF implementation.

### macOS

Install the Xcode Command Line Tools if they are not already present:

```sh
xcode-select --install
brew install git cmake ninja python@3.14 curl pkg-config
```

The command-line tools include the system compiler and `patch`.

### Debian or Ubuntu

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates git cmake \
  ninja-build python3 curl patch pkg-config
```

Check `python3 --version`; older distribution releases may need a newer Python.
Target-specific AROS dependencies are additional to these host tools.

## Optional workflows

| Task | Additional tools |
| --- | --- |
| PC boot check with `aros test` | `qemu-system-x86_64` (Homebrew `qemu`; Debian `qemu-system-x86`) |
| Compiler caching | `sccache` or `ccache`, discovered on `PATH` |
| Serial board console | `picocom`, `screen` or `minicom`; inspect `aros board console --help` |
| Verify a native release | `jq`, GitHub CLI with `gh attestation verify`, cosign, tar, SHA-256 utility |
| Install from signed APT | curl, GnuPG (`gpg` and `gpgconf`), dpkg |

For signature verification, check the installed CLI supports
`gh attestation verify --deny-self-hosted-runners` and
`cosign verify-blob --bundle`. Follow the
[GitHub CLI](https://cli.github.com/manual/installation) and
[cosign](https://docs.sigstore.dev/cosign/system_config/installation/)
installation guides when the distribution versions are too old.

Node.js, npm, ShellCheck and the Rust audit helpers are needed to
[contribute to the tools](/aros-tools/contributing/development/), not to run
an already built suite.

## Network and storage

Initial source and compiler downloads need access to their declared origins.
`--offline` on supported build/toolchain commands requires local inputs;
it does not turn source cloning into an offline operation.

Keep AROS sources and builds on a **case-sensitive filesystem**. On macOS,
a case-sensitive APFS volume is suitable. Avoid cloud-synchronized build
directories. Space requirements depend on the target and retained build trees;
the tools do not promise a fixed minimum.

Next: [build and install the suite](/aros-tools/getting-started/installation/).
