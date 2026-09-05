---
title: Update and uninstall
description: Change the tools suite without losing AROS source, compiler stores, or board configuration.
---

Use the same installation method for updates. The frontend and all seven
companions must remain at one version; do not replace just one executable.

## Source build

Stop active tool commands before rebuilding. From the reviewed tools checkout:

```sh
cd /path/to/aros-tools
git pull --ff-only
cargo build --release --workspace --all-features --locked
./target/release/aros --version
./target/release/aros build-tools check
```

Running Cargo inside the repository applies its pinned Rust toolchain.
If your PATH points at this checkout's `target/release`, the next invocation
uses the rebuilt suite.

To stop using that source installation, remove its PATH entry.
Deleting the tools checkout is a separate decision; it may contain your
uncommitted work.

## Release archive

Verify the new archive's complete evidence as described in
[installation](/aros-tools/getting-started/installation/#native-release-archive).
The native installer is **no-clobber**; it does not update an occupied prefix.

For an upgrade, install into a new empty version-specific prefix that you own,
check the suite, then switch your PATH to its `bin` directory.
Stop active builds before switching. Keep the old prefix until the new
installation has been checked.

There is no `aros uninstall` command. Remove only the eight files you
installed: `aros`, `aros-ahi-runner`, `aros-collect`, `aros-fetch`,
`aros-genmodule`, `aros-romtool`, `aros-transpiler` and `aros-verify`.
Do not delete a shared `bin` directory.

## Package managers

Use these only after the relevant channel is
[publicly available](/aros-tools/reference/release-status/).
Upgrade using the manager that installed the suite:

```sh
# Debian/Ubuntu
sudo apt-get update
sudo apt-get install --only-upgrade aros-tools

# Homebrew
brew upgrade metaneutrons/tap/aros-tools

# Arch Linux / AUR helper example
paru -Syu aros-tools-bin
```

To uninstall, choose the matching command:

```sh
sudo apt-get remove aros-tools       # Debian/Ubuntu
brew uninstall aros-tools           # Homebrew
paru -Rns aros-tools-bin             # Arch/AUR
```

## Optional local state

Removing the tools does not automatically remove:

- local board profiles (normally `~/.config/aros/boards.toml`);
- downloaded archives and compiler stores reported by `aros info`;
- AROS source checkouts, build directories and retained evidence.

Review those exact paths separately if you want to remove them.
`aros clean` is a build-output command, not an uninstall or general
state-purge command.
