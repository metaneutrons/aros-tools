---
title: Generated CLI contract
description: Source-derived structural facts for the current public aros command model.
---

This reference is generated from the `aros` Clap command model and committed for review. It records visible commands and structural argument facts; task semantics, side effects, and recovery remain in the [command reference](/aros-tools/reference/cli/). Hidden lifecycle bridges are deliberately excluded.

The `position` column is one-based for positional arguments and `—` for options. Empty `default`, `values`, `environment`, and `conflicts` cells mean that Clap declares none.

## Global arguments

| ID | Spelling | Position | Required | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| diagnostic_format | --diagnostic-format | — | no | 1 | human | human, json | AROS_DIAGNOSTIC_FORMAT |  |
| log_level | --log-level | — | no | 1 | off | off, error, warn, info, debug, trace | AROS_LOG_LEVEL |  |
| log_format | --log-format | — | no | 1 | human | human, jsonl | AROS_LOG_FORMAT |  |
| log_file | --log-file | — | no | 1 |  |  | AROS_LOG_FILE |  |

## Command sections

- [`aros board`](/aros-tools/reference/cli-contract/board/)
- [`aros build`](/aros-tools/reference/cli-contract/build/)
- [`aros build-tools`](/aros-tools/reference/cli-contract/build-tools/)
- [`aros ccache`](/aros-tools/reference/cli-contract/ccache/)
- [`aros clean`](/aros-tools/reference/cli-contract/clean/)
- [`aros completions`](/aros-tools/reference/cli-contract/completions/)
- [`aros golden`](/aros-tools/reference/cli-contract/golden/)
- [`aros host-compiler`](/aros-tools/reference/cli-contract/host-compiler/)
- [`aros info`](/aros-tools/reference/cli-contract/info/)
- [`aros install`](/aros-tools/reference/cli-contract/install/)
- [`aros setup`](/aros-tools/reference/cli-contract/setup/)
- [`aros source`](/aros-tools/reference/cli-contract/source/)
- [`aros test`](/aros-tools/reference/cli-contract/test/)
- [`aros toolchain`](/aros-tools/reference/cli-contract/toolchain/)
