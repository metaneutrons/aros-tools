---
title: Installation
description: Build the complete tool suite from a reviewed source checkout.
---

## Requirements

- Rust stable with Cargo
- Git
- CMake and Ninja for the optional translated build engine
- the host packages required by the selected AROS target

## Build from source

```sh
git clone https://github.com/metaneutrons/aros-tools.git
cd aros-tools
cargo build --release --workspace --all-features
```

The executables are written to `target/release`. Keep the suite together: the
`aros` frontend resolves build tools as separate processes so their command,
logging and failure boundaries remain independently testable.

Before installing a development build, run the same gates used by CI:

```sh
cargo fmt --all -- --check
sh scripts/check-architecture.sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
AROS_TEST_SOURCE_ROOT=/absolute/path/to/AROS cargo test --workspace --all-features
```

:::note[Binary packages]
Native release archives, Debian packages, Homebrew and AUR installation will
be documented here only after their public artifacts pass clean-room tests.
:::
