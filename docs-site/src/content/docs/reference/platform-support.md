---
title: Platform support
description: Host operating systems, binary compatibility floors and target architectures covered by release qualification.
---

## Native host matrix

| Archive target | Build environment | Compatibility contract |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Debian Bookworm x86-64 | glibc 2.36 or newer |
| `aarch64-unknown-linux-gnu` | Debian Bookworm ARM64 | glibc 2.36 or newer |
| `x86_64-apple-darwin` | Native macOS Intel | macOS 13 or newer |
| `aarch64-apple-darwin` | Native Apple silicon | macOS 13 or newer |

Release qualification inspects Linux GLIBC symbol requirements and macOS Mach-O
deployment targets in addition to executing native smoke and package tests.
Rosetta and QEMU user emulation are not substitutes for a native release lane.

## AROS target profiles

The host matrix does not limit the AROS CPU being built. The current source and
toolchain contracts include x86, ARM, AArch64 and RISC-V profiles, including
`pc-x86_64`, `arm-raspi`, `rpi-aarch64` and `opensbi-riscv64` where the selected
checkout declares them.

An available compiler is not a boot-support claim. Physical-board support,
legacy KOBJ inputs, DTBs and UART evidence are tracked separately by the board
profile and AROS source project.

## Not currently supported

- Windows is not a native archive or CI host.
- Linux distributions older than glibc 2.36 are outside the binary contract;
  build from source on that host if its Rust toolchain and dependencies permit.
- macOS older than 13 is outside the release contract.
- A complete translated product build from an entirely pristine upstream AROS
  checkout remains pending the native GNU Make frontend qualification.

Support means the exact tagged release passed its documented lane. It does not
mean every AROS target or third-party package is buildable on every host.
