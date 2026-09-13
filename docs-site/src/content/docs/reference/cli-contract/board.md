---
title: "Generated CLI contract: board"
description: Source-derived structural facts for the public aros board command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/).

| Command | ID | Spelling | Position | Required | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros board build` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board build` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board build` | target | -t, --target | — | no | 1 |  |  | — |  |
| `aros board build` | jobs | -j, --jobs | — | no | 1 |  |  | — |  |
| `aros board build` | clean | --clean | — | no | 0 | false |  | — |  |
| `aros board build` | verbose | -v, --verbose | — | no | 0 | false |  | — |  |
| `aros board build` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE |  |
| `aros board build` | require_fetch_checksums | --require-fetch-checksums | — | no | 0 | false |  | AROS_FETCH_REQUIRE_CHECKSUMS |  |
| `aros board build` | toolchain_dir | --toolchain-dir | — | no | 1 |  |  | — |  |
| `aros board build` | debug | --debug | — | no | 0 | false |  | — |  |
| `aros board build` | engine_dir | --engine-dir | — | no | 1 |  |  | — |  |
| `aros board build` | dtb_path | --dtb-path | — | no | 1 |  |  | — |  |
| `aros board build` | core_kobj_dir | --core-kobj-dir | — | no | 1 |  |  | — |  |
| `aros board console` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board console` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board console` | program | --program | — | no | 1 | auto | auto, picocom, screen, minicom | — |  |
| `aros board console` | device | --device | — | no | 1 |  |  | — |  |
| `aros board console` | baud | --baud | — | no | 1 |  |  | — |  |
| `aros board console` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
| `aros board deploy` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board deploy` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board deploy` | artifact_dir | --artifact-dir | — | no | 1 |  |  | — |  |
| `aros board deploy` | apply | --apply | — | no | 0 | false |  | — |  |
| `aros board deploy` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
| `aros board doctor` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board doctor` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board init` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board init` | model | --model | — | yes | 1 |  | rpi3, rpi4, rpi5, milk-v-titan | — |  |
| `aros board init` | transport | --transport | — | no | 1 |  | native-tftp, uboot-usb-ecm, uefi-esp | — |  |
| `aros board init` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board init` | apply | --apply | — | no | 0 | false |  | — |  |
| `aros board sd image` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board sd image` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board sd image` | boot_bundle | --boot-bundle | — | yes | 1 |  |  | — |  |
| `aros board sd image` | output | --output | — | yes | 1 |  |  | — |  |
| `aros board sd image` | apply | --apply | — | no | 0 | false |  | — |  |
| `aros board sd image` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
| `aros board sd scan` | artifact | --artifact | — | no | 1 |  |  | — |  |
| `aros board sd unmount` | device | --device | — | no | 1 |  |  | — |  |
| `aros board sd unmount` | apply | --apply | — | no | 0 | false |  | — | dry_run |
| `aros board sd unmount` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
| `aros board sd write` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board sd write` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board sd write` | artifact | --artifact | — | yes | 1 |  |  | — |  |
| `aros board sd write` | device | --device | — | yes | 1 |  |  | — |  |
| `aros board sd write` | confirm | --confirm | — | no | 1 |  |  | — |  |
| `aros board sd write` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
| `aros board serve` | profile | --profile | — | yes | 1 |  |  | — |  |
| `aros board serve` | config | --config | — | no | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board serve` | dry_run | --dry-run | — | no | 0 | false |  | — |  |
