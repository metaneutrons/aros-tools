---
title: "Generated CLI contract: source"
description: Source-derived structural facts for the public aros source command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/).

| Command | ID | Spelling | Position | Required | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros source init` | path | path | 1 | yes | 1 |  |  | — |  |
| `aros source init` | upstream | --upstream | — | no | 1 | https://github.com/aros-development-team/AROS.git |  | — |  |
| `aros source init` | fork | --fork | — | no | 1 |  |  | — |  |
| `aros source init` | source_ref | --ref | — | no | 1 |  |  | — |  |
| `aros source sync` | upstream | --upstream | — | no | 1 | https://github.com/aros-development-team/AROS.git |  | AROS_UPSTREAM_URL |  |
| `aros source sync` | upstream_ref | --ref | — | no | 1 | master |  | — |  |
| `aros source sync` | transpile | --no-transpile | — | no | 0 | true |  | — |  |
