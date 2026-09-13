---
title: "Generated CLI contract: cache"
description: Source-derived structural facts for the public aros cache command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros cache archives fetch` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives fetch` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives fetch` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives fetch` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives fetch` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE | refresh |
| `aros cache archives fetch` | refresh | --refresh | — | optional | 0 | false |  | — | offline |
| `aros cache archives fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives list` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives list` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives list` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives list` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives list` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives verify` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives verify` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives verify` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives verify` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives verify` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo fetch` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros cache cargo fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo list` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo status` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo verify` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler status` | backend | --backend | — | optional | 1 | auto | auto, sccache, ccache | — |  |
| `aros cache compiler status` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources fetch` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros cache sources fetch` | allow_unverified | --allow-unverified | — | optional | 0 | false |  | — | source_lock, compatibility_ports_lock |
| `aros cache sources fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources list` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources status` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources verify` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache status` | format | --format | — | optional | 1 | human | human, json | — |  |
