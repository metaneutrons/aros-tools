---
title: Platform support
description: Distinguish native host support, implemented target profiles, available artifacts, and real boot evidence.
---

## Native host matrix

| Archive target | Qualification host | Binary compatibility contract |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Native Linux x86-64 | glibc 2.36 or newer |
| `aarch64-unknown-linux-gnu` | Native Linux ARM64 | glibc 2.36 or newer |
| `x86_64-apple-darwin` | Native macOS Intel | macOS 13 or newer |
| `aarch64-apple-darwin` | Native macOS Apple silicon | macOS 13 or newer |

These are native tools archive contracts, not a claim that stable archives are
already published. Check [release status](/aros-tools/reference/release-status/).

The host mapping is implemented in
[`host_compiler.rs`](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/host_compiler.rs).
Release checks inspect Linux GLIBC symbol requirements and macOS deployment
targets as well as running binaries natively.

## AROS target profiles

| Built-in profile | Target CPU | Platform | Compiler target triple |
| --- | --- | --- | --- |
| `pc-x86_64` | x86-64 | PC | `x86_64-unknown-aros` |
| `arm-raspi` | ARM, hard float | Raspberry Pi | `arm-unknown-aros` |
| `rpi-aarch64` | AArch64 | Raspberry Pi | `aarch64-unknown-aros` |
| `opensbi-riscv64` | RISC-V 64 | OpenSBI | `riscv64-unknown-aros` |

A checkout's `aros-targets.toml`, when present, replaces the
[built-in profile contract](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-common/config/aros-targets.toml).
Compiler selection is separately validated by the
[CMake toolchain](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cmake-engine/engine/toolchains/AROS.cmake).

An implemented profile does not mean that a matching released toolchain entry
or all source/legacy inputs are available. In particular, RISC-V profile support
must not be read as a complete four-host RISC-V release or boot claim.

## Physical boards

The board schema models Pi 3, Pi 4, Pi 5 and Milk-V Titan.
Model-specific DTBs, legacy KOBJ objects, firmware and transports are separate
requirements. USB-ECM is reviewed only for Pi 4; Milk-V uses the OpenSBI/UEFI
backend. See the [board matrix](/aros-tools/workflows/boards/#implemented-models-and-transports).

The `aros test` implementation uses `qemu-system-x86_64` and PC boot paths.
It is not a multi-architecture emulator frontend. Physical boards need their
own boot evidence.

## Component-specific limits

| Component | Implemented boundary |
| --- | --- |
| Transpiler | Supported MetaMake semantics and explicit coverage reports; not arbitrary GNU Make execution |
| Verifier | `architecture` coverage profile; no core/distribution reachability profile |
| AHI runner | x86-64, ARM and AArch64 modes |
| ROM tool | PKG create/list/extract |
| External applications | Compiler/SDK reuse; no application packaging frontend |
| Board debugging | External serial terminal; JTAG/SWD and power fields are descriptive |

[Standalone tools](/aros-tools/reference/standalone-tools/) links these
claims to their implementations.

## Not currently supported

- Windows as a native release host.
- Published Linux binaries on hosts below glibc 2.36, or macOS below 13.
- A general claim of complete translated products from entirely pristine
  upstream AROS.
- A blanket boot guarantee for every configured board or named profile.

Source builds on another host may be an experiment, but are not thereby a
supported release target.
