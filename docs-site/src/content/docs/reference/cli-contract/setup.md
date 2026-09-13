---
title: "Generated CLI contract: setup"
description: Source-derived structural facts for the public aros setup command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros setup` | force | -f, --force | — | optional | 0 | false |  | — | local, offline |
| `aros setup` | preset | -p, --preset | — | optional | 1 |  |  | — | all |
| `aros setup` | all | --all | — | optional | 0 | false |  | — | preset, local |
| `aros setup` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE | force |
| `aros setup` | local | --local | — | optional | 1 |  |  | — | all, force |
