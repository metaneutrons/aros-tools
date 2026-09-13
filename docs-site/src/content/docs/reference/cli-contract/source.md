---
title: "Generated CLI contract: source"
description: Source-derived structural facts for the public aros source command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros source init` | path | path | 1 | required | 1 |  |  | — |  |
| `aros source init` | upstream | --upstream | — | optional | 1 | https://github.com/aros-development-team/AROS.git |  | — |  |
| `aros source init` | fork | --fork | — | optional | 1 |  |  | — |  |
| `aros source init` | source_ref | --ref | — | optional | 1 |  |  | — |  |
| `aros source sync` | upstream | --upstream | — | optional | 1 | https://github.com/aros-development-team/AROS.git |  | AROS_UPSTREAM_URL |  |
| `aros source sync` | upstream_branch | --branch | — | optional | 1 | master |  | — |  |
| `aros source sync` | transpile | --no-transpile | — | optional | 0 | true |  | — |  |
