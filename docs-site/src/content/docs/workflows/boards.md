---
title: Physical boards
description: Match a local profile to real hardware, validate boot inputs, and preview deployment before writing.
---

A board profile identifies one physical device. Its **name** is your local
label; its **model**, **backend** and **transport** determine the supported
operations. A profile or successful image build is not proof of a UART boot.

## Implemented models and transports

| Model | Backend | Supported transport | Required core inputs |
| --- | --- | --- | --- |
| `rpi3` (Pi 3B+ DTB contract) | `raspberry-pi` | `native-tftp` | ARM KOBJ triplet and model DTB |
| `rpi4` | `raspberry-pi` | `native-tftp`, `uboot-usb-ecm` | AArch64 KOBJ triplet and model DTB |
| `rpi5` | `raspberry-pi` | `native-tftp` | AArch64 KOBJ triplet and model DTB |
| `milk-v-titan` | `opensbi-uefi` | `uefi-esp` | RISC-V legacy core objects |

The validator rejects USB-ECM on Pi 3/5 and rejects a Pi transport for the
OpenSBI/UEFI backend. Debug transport and power-control fields are metadata;
the CLI does not automate JTAG, SWD or power equipment.

Source: [board schema and validation](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-board/src/config.rs).

## Create and diagnose a profile

For a **Pi 4 USB-ECM** profile, preview the generated template:

```sh
aros board init --board rpi4-usb
```

Then create a new config file explicitly:

```sh
aros board init --board rpi4-usb --apply
aros board scan
```

:::note[The name does not select a model]
`board init` always emits the Pi-4 USB-ECM template. Naming it `pi5`
does not make it a Pi-5 template. For Pi 3, Pi 5, native TFTP or Milk-V,
adapt the matching profile from the
[reviewed example registry](https://github.com/metaneutrons/aros-tools/blob/main/support/rpi-debug/boards.example.toml).
:::

Edit the printed file (normally `~/.config/aros/boards.toml`) with the real
paths, interfaces, serial device and device identity. Use a matching target
preset declared by your selected AROS checkout. The built-in four toolchain
profiles are not the example registry's board-specific debug presets.

For Pi 4 USB-ECM, copy the stable USB descriptor values from `board scan`;
do not persist a guessed dynamic interface name. Then, inside the AROS tree:

```sh
aros board doctor --board rpi4-usb
```

## Build, deploy and serve

With the profile's exact DTB and legacy core objects present:

```sh
aros board build --board rpi4-usb
aros board deploy --board rpi4-usb
aros board deploy --board rpi4-usb --apply
aros board serve --board rpi4-usb --dry-run
aros board serve --board rpi4-usb
```

Deploy previews by default; `--apply` stages the bundle to the configured
TFTP destination. Serve binds restricted DHCP and read-only TFTP to the
validated interface and board identity. The host address must already be
configured. These network commands are not the Milk-V UEFI-ESP boot path.

Open a serial console separately:

```sh
aros board console --board rpi4-usb --dry-run
aros board console --board rpi4-usb --program picocom
```

Supported terminal programs are `picocom`, `screen` and `minicom`.
Auto mode searches in that order. Save and interpret the resulting UART
evidence using your board's bring-up procedure.

## SD-card safety sequence

Image creation needs an external `boot-bundle.toml` plus its hash-declared
firmware and artifact inputs. A build directory alone is not a boot bundle.
Use the prepared bundle for the exact board/transport.

```sh
aros board sd image --board rpi4-usb --boot-bundle /verified/bundle --output /new/artifact
aros board sd image --board rpi4-usb --boot-bundle /verified/bundle --output /new/artifact --apply
aros board sd scan --artifact /new/artifact
```

If the intended removable disk is mounted, inspect and explicitly unmount it:

```sh
aros board sd unmount
aros board sd unmount --device SCAN_ID --apply
```

Run `sd scan --artifact` again after unmounting. Substitute its exact scan
ID and confirmation token in the write sequence:

```sh
aros board sd write --board rpi4-usb --artifact /new/artifact --device SCAN_ID --dry-run
aros board sd write --board rpi4-usb --artifact /new/artifact --device SCAN_ID --confirm TOKEN
```

The final command writes the selected medium. Candidates must be whole,
removable, writable and unmounted. Raw device paths are rejected; the token
binds the measured image and device identity. The writer performs read-back
verification. SD image creation uses the implemented MBR/FAT32 format;
it is not a general disk-partitioning frontend.

## Evidence still required

For a physical-boot claim, record the model, firmware, source commit,
cross-toolchain identity, legacy core inputs, artifact digests and UART log.
Do not infer boot support from an available profile or compiler.

The detailed [Raspberry Pi lab guide](https://github.com/metaneutrons/aros-tools/blob/main/support/rpi-debug/README.md)
covers the prepared firmware and external-debugger workflow.
