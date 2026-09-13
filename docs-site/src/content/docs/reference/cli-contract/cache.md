---
title: "Generated CLI contract: cache"
description: Source-derived structural facts for the public aros cache command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
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
