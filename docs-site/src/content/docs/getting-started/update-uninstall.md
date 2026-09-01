---
title: Update and uninstall
description: Upgrade or remove source builds, release archives, Homebrew, APT and AUR installations without losing AROS source or toolchain stores.
---

`aros-tools` never owns the AROS source checkout or its build products. Updating
or removing the host tools does not remove AROS sources, build directories,
download caches or installed cross-toolchains.

## Source build

Update the reviewed tools checkout, rebuild with the lockfile, and replace the
same installed binaries:

```sh
git -C /path/to/aros-tools pull --ff-only
cargo build --release --workspace --all-features --locked \
  --manifest-path /path/to/aros-tools/Cargo.toml
```

Remove the copied executables from the directory where you installed them. Do
not delete an entire shared `bin` directory.

## Release archive

Verify the new archive and its checksum before replacing the eight executables.
Keep the suite at one version; mixing `aros` with older helper binaries is not a
supported state. To uninstall, remove only the `aros`, `aros-ahi-runner`,
`aros-collect`, `aros-fetch`, `aros-genmodule`, `aros-romtool`,
`aros-transpiler` and `aros-verify` files installed from the archive.

## Package managers

After the first public release, use the package manager that installed the
suite:

```sh
# Debian/Ubuntu
sudo apt-get update && sudo apt-get install --only-upgrade aros-tools
sudo apt-get remove aros-tools

# Homebrew
brew upgrade aros-tools
brew uninstall aros-tools

# Arch Linux / AUR helper example
paru -Syu aros-tools-bin
paru -Rns aros-tools-bin
```

Package-manager availability remains governed by the
[release-status page](/aros-tools/reference/release-status/). Do not configure
an unpublished repository or formula from an unverified snippet.

## Optional local state

Configuration and caches are intentionally retained during uninstall:

- board profiles: `~/.config/aros/boards.toml`;
- tool download/store state: the paths reported by `aros info`;
- per-checkout build directories: `<checkout>/build`.

Review these exact paths before removing them. The CLI does not offer a broad
recursive purge command.
