---
title: Prerequisites
description: Supported hosts, required tools, and the boundary between host tooling and target-specific AROS dependencies.
---

## Supported hosts

Native `aros-tools` archives target:

| Host | Release target | Minimum platform |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | glibc 2.36 |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | glibc 2.36 |
| macOS Intel | `x86_64-apple-darwin` | macOS 13 |
| macOS Apple silicon | `aarch64-apple-darwin` | macOS 13 |

Windows is not a release host. Cross-toolchain **targets** such as x86, ARM,
AArch64 and RISC-V are independent from the host architecture.

## Common tools

Install Git, CMake, Ninja, Python 3.11 or newer, curl, a POSIX `patch` implementation and
the platform CA certificates. These are runtime dependencies of public
workflows, not merely build dependencies: source synchronization uses Git,
product builds use CMake and Ninja, FTP-compatible fetch declarations use curl,
source patch declarations use `patch`, and `aros-verify genmf` uses Python 3.
Building `aros-tools` from source additionally requires Rust 1.98.0; the
repository's `rust-toolchain.toml` selects it.

The complete contributor quality gate also requires Node.js 24 or newer with
npm, actionlint, ShellCheck, `jq`, GnuPG (`gpg` and `gpgv`), `dpkg-deb`,
`gzip`, `tar`, `ar`, curl and a SHA-256 implementation. Its Python release
fixtures use the standard-library `tomllib` module. The checked-in
`contracts/development-runtimes-v1.toml` file is the version SSOT; the gate
fails before compiling when a required runtime or command is missing.

On macOS, install the Xcode Command Line Tools first:

```sh
xcode-select --install
brew install actionlint cmake coreutils cosign curl dpkg gh git gnupg jq ninja node pkg-config python@3.14 shellcheck
```

On Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates cmake curl dpkg-dev gh git gnupg golang-go jq ninja-build patch pkg-config python3 shellcheck
go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.7
```

Install a supported Node.js release (24 or newer) from your normal Node
distribution or version manager; the Ubuntu archive's default `nodejs` package
is not assumed to satisfy that contract. Ensure Go's binary directory is on
`PATH` after installing the pinned actionlint command.

Verified native-archive installation additionally requires GitHub CLI with
`gh attestation verify` and the `--deny-self-hosted-runners` option, plus
cosign with `verify-blob` and Sigstore-bundle support. If the versions provided
by the host package manager do not expose those exact commands, use the
[official GitHub CLI installation instructions](https://cli.github.com/manual/installation)
and the [official cosign installation instructions](https://docs.sigstore.dev/cosign/system_config/installation/),
then confirm `gh attestation verify --help` and `cosign verify-blob --help`
before using the copy/paste verification path. Debian/Ubuntu APT installation
also requires the `curl`, `gpg`, `gpgconf` and `dpkg` commands supplied by the
packages shown above.

AROS products may need additional host libraries. Those belong to the selected
AROS target, not to the `aros-tools` installer. Run the checkout's documented
configure path or `aros setup` to discover target-specific requirements.

## Network and storage

Initial source, host-compiler and released-toolchain installation requires
HTTPS access to the repositories and release hosts recorded in the selected
checkout. Every downloaded toolchain is verified before use. Set
`AROS_OFFLINE=1` to forbid network access and require already verified cache or
store content.

Builds are storage intensive. Keep the source checkout, build directory and
toolchain store on a case-sensitive filesystem; do not place active build trees
inside a cloud-synchronized folder.
