---
title: "Generated CLI contract: setup"
description: Source-derived structural facts for the public aros setup command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/).

| Command | ID | Spelling | Position | Required | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros setup` | force | -f, --force | — | no | 0 | false |  | — | local, offline |
| `aros setup` | preset | -p, --preset | — | no | 1 |  |  | — | all |
| `aros setup` | all | --all | — | no | 0 | false |  | — | preset, local |
| `aros setup` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE | force |
| `aros setup` | local | --local | — | no | 1 |  |  | — | all, force |
