---
title: Diagnostics and logs
description: Capture actionable failures, collect local execution events, and interpret uncertain publication state.
---

## Capture an error

Human-readable errors are the default. For automation, request JSON:

```sh
aros --diagnostic-format json build --preset pc-x86_64 2>diagnostic.json
```

Errors go to stderr; intentional command output stays on stdout.
The versioned envelope has a `schema` field of
`aros-tool-diagnostics-v1` and a `diagnostics` array. Inspect each
diagnostic's `code`, `severity`, `stage`, `message`, `hint`,
and any optional source/context fields. Do not parse human wording.

A failed invocation exits nonzero. A diagnostic document can contain multiple
findings, including warnings; the envelope is not a single error object.

## Collect a local log

```sh
aros build --preset pc-x86_64 \
  --log-level debug --log-format jsonl --log-file ./aros-debug.jsonl
```

Supported levels are `off`, `error`, `warn`, `info`, `debug`,
`trace`; formats are `human` and `jsonl`.
Logging is off by default and requires an explicit local file.

Use both `--log-level` and `--log-file` in portable examples.
The frontend promotes an effective `off` level to `info` when a file is
supplied. The collector promotes file-only logging when no level was explicitly
selected. Other companions keep an explicit/default `off` unchanged.

Logs are local observations and are not uploaded automatically. Standard
records omit ambient timestamps and host identity, but explicit paths,
board identifiers and bounded subprocess output can be present. Review the
file before sharing it. A log file is not part of a deterministic artifact.

## Identify the owning component

| Prefix | Owner |
| --- | --- |
| `AR` | Frontend orchestration |
| `AT` | MetaMake transpiler |
| `AV` | Independent verifier |
| `AC` | Collector/linker driver |
| `AF` | Source fetcher |
| `AH` | AHI build runner |
| `AG` | Module generator |
| `RM` | ROM tool |
| `AP` | Internal release/installation boundary |

Codes are stable identifiers and are not reused for a different retired error.
See [the diagnostic model](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-common/src/diagnostic.rs)
for the exact code definitions.

Captured child stdout/stderr are drained concurrently and bounded to 64 KiB
per stream, with explicit truncation. Interactive console commands retain
their terminal. Source Git operations have a 30-minute process-group deadline;
source graph transpilation has a 10-minute deadline.

## Interpret source synchronization failures

| Code | Boundary |
| --- | --- |
| `AR0111` | Input or ref |
| `AR0112` | Transport |
| `AR0113` | Repository lock |
| `AR0114` | Mutable repository state |
| `AR0115` | Candidate graph validation |
| `AR0116` | Publication and materialization |

An `AR0113` owner-record file persists by design. Its presence is not proof
of an active or stale lock; the operating system owns the actual lock.

An `AR0116` diagnostic may carry `context.commit_state`:

| Value | Meaning |
| --- | --- |
| `rolled_back` | The original branch/index/submodule snapshot was restored |
| `committed` | The mutation succeeded but a later reporting operation failed |
| `indeterminate` | Neither final state could be proved |

Preserve the reported state and inspect it before retrying an indeterminate
operation. Do not infer success or rollback from message wording.

## Generated-file recovery

Module and ROM generation distinguish destination conflicts, unsafe targets,
incomplete recovery, durability failures and uncertain commits.
They preserve their `AG`/`RM` codes and provide a remediation hint for the
specific failure. A completed predecessor recovery can appear as a
`publication.recovery` log event.

Follow the [troubleshooting guide](/aros-tools/reference/troubleshooting/)
for the next diagnostic step, and use the
[configuration reference](/aros-tools/reference/configuration/#component-diagnostics-and-logging)
for component-specific environment names.
