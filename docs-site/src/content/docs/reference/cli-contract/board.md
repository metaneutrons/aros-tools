---
title: "Generated CLI contract: board"
description: Source-derived structural facts for the public aros board command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros board build` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board build` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board build` | target | -t, --target | — | optional | 1 |  |  | — |  |
| `aros board build` | jobs | -j, --jobs | — | optional | 1 |  |  | — |  |
| `aros board build` | clean | --clean | — | optional | 0 | false |  | — |  |
| `aros board build` | verbose | -v, --verbose | — | optional | 0 | false |  | — |  |
| `aros board build` | compiler_cache | --compiler-cache | — | optional | 1 | auto | auto, off, sccache, ccache | — |  |
| `aros board build` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros board build` | require_fetch_checksums | --require-fetch-checksums | — | optional | 0 | false |  | AROS_FETCH_REQUIRE_CHECKSUMS |  |
| `aros board build` | toolchain_dir | --toolchain-dir | — | optional | 1 |  |  | — |  |
| `aros board build` | debug | --debug | — | optional | 0 | false |  | — |  |
| `aros board build` | engine_dir | --engine-dir | — | optional | 1 |  |  | — |  |
| `aros board build` | dtb_path | --dtb-path | — | optional | 1 |  |  | — |  |
| `aros board build` | core_kobj_dir | --core-kobj-dir | — | optional | 1 |  |  | — |  |
| `aros board console` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board console` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board console` | program | --program | — | optional | 1 | auto | auto, picocom, screen, minicom | — |  |
| `aros board console` | device | --device | — | optional | 1 |  |  | — |  |
| `aros board console` | baud | --baud | — | optional | 1 |  |  | — |  |
| `aros board console` | dry_run | --dry-run | — | optional | 0 | false |  | — |  |
| `aros board deploy` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board deploy` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board deploy` | artifact_dir | --artifact-dir | — | optional | 1 |  |  | — |  |
| `aros board deploy` | apply | --apply | — | optional | 0 | false |  | — | dry_run |
| `aros board deploy` | dry_run | --dry-run | — | optional | 0 | false |  | — | apply |
| `aros board doctor` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board doctor` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board init` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board init` | model | --model | — | required | 1 |  | rpi3, rpi4, rpi5, milk-v-titan | — |  |
| `aros board init` | transport | --transport | — | optional | 1 |  | native-tftp, uboot-usb-ecm, uefi-esp | — |  |
| `aros board init` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board init` | apply | --apply | — | optional | 0 | false |  | — |  |
| `aros board sd image` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board sd image` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board sd image` | boot_bundle | --boot-bundle | — | required | 1 |  |  | — |  |
| `aros board sd image` | output | --output | — | required | 1 |  |  | — |  |
| `aros board sd image` | apply | --apply | — | optional | 0 | false |  | — | dry_run |
| `aros board sd image` | dry_run | --dry-run | — | optional | 0 | false |  | — | apply |
| `aros board sd scan` | artifact | --artifact | — | optional | 1 |  |  | — |  |
| `aros board sd unmount` | device | --device | — | optional | 1 |  |  | — |  |
| `aros board sd unmount` | apply | --apply | — | optional | 0 | false |  | — | dry_run |
| `aros board sd unmount` | dry_run | --dry-run | — | optional | 0 | false |  | — |  |
| `aros board sd write` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board sd write` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board sd write` | artifact | --artifact | — | required | 1 |  |  | — |  |
| `aros board sd write` | device | --device | — | required | 1 |  |  | — |  |
| `aros board sd write` | confirm | --confirm | — | optional | 1 |  |  | — |  |
| `aros board sd write` | dry_run | --dry-run | — | optional | 0 | false |  | — |  |
| `aros board serve` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros board serve` | config | --config | — | optional | 1 |  |  | AROS_BOARDS_FILE |  |
| `aros board serve` | dry_run | --dry-run | — | optional | 0 | false |  | — |  |
