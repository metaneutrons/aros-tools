---
title: Physical boards
description: Configure, diagnose, build, deploy and boot physical AROS targets through fail-closed local board profiles.
---

Board workflows are explicit and dry-run-first. A profile names one physical
device, its target preset, network identity, deployment paths and serial
configuration. No command guesses a raw disk from a mount point or silently
selects the first network interface.

## Create and diagnose a profile

Preview a profile without writing configuration:

```sh
aros board init --board pi5
```

Create it only after reviewing the output:

```sh
aros board init --board pi5 --apply
aros board scan
```

After filling the printed values in `~/.config/aros/boards.toml`, enter the AROS
checkout and run:

```sh
aros board doctor --board pi5
```

The doctor is non-mutating. It verifies profile identity, target configuration,
interfaces, external programs and available artifacts.

## Build, deploy and serve

```sh
aros board build --board pi5
aros board deploy --board pi5                 # preview
aros board deploy --board pi5 --apply
aros board serve --board pi5 --dry-run
aros board serve --board pi5
```

DHCP binds to the configured interface and board identity. TFTP serves only the
validated deployment root. Use the external serial terminal through
`aros board console --board pi5`; the CLI does not embed a second UART stack.

## SD-card safety sequence

Image creation, disk discovery, optional unmount and writing are separate
operations:

```sh
aros board sd image --board pi5 --boot-bundle /verified/bundle --output /new/artifact
aros board sd image --board pi5 --boot-bundle /verified/bundle --output /new/artifact --apply
aros board sd scan --artifact /new/artifact
aros board sd write --board pi5 --artifact /new/artifact --device SCAN_ID --dry-run
aros board sd write --board pi5 --artifact /new/artifact --device SCAN_ID --confirm TOKEN
```

Only whole, removable, writable, unmounted physical media with stable identity
are candidates. Raw device paths are rejected at the CLI boundary. The final
token binds the verified image and rescanned device; changing either invalidates
it.

The repository's detailed Raspberry Pi lab contract lives in
[`support/rpi-debug/README.md`](https://github.com/metaneutrons/aros-tools/blob/main/support/rpi-debug/README.md).
