---
title: "Generated CLI contract: image"
description: Source-derived structural facts for the public aros image command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros image build` | profile | --profile | — | required | 1 |  |  | — |  |
| `aros image build` | build_root | --build-root | — | required | 1 |  |  | — |  |
| `aros image build` | receipt | --receipt | — | required | 1 |  |  | — |  |
| `aros image build` | output | --output | — | required | 1 |  |  | — |  |
| `aros image build` | lock | --lock | — | optional | 1 |  |  | — |  |
| `aros image build` | external | --external | — | optional | 1 |  |  | — |  |
| `aros image build` | apply | --apply | — | optional | 0 | false |  | — | dry_run |
| `aros image build` | dry_run | --dry-run | — | optional | 0 | false |  | — | apply |
| `aros image build` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros image inspect` | artifact | --artifact | — | required | 1 |  |  | — |  |
| `aros image inspect` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros image verify` | artifact | --artifact | — | required | 1 |  |  | — |  |
| `aros image verify` | format | --format | — | optional | 1 | human | human, json | — |  |
