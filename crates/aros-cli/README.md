# `aros` diagnostics and local logging

`aros` uses the same versioned diagnostic document as the transpiler,
`aros-collect`, and the AHI runner. Human output is the default. Automation can
select one JSON document on standard error:

```console
aros --diagnostic-format json build --preset pc-x86_64
```

The setting is global, so it may also follow a subcommand. The equivalent
environment variable is `AROS_DIAGNOSTIC_FORMAT=human|json`.

Every fatal diagnostic has a stable code, stage, severity, message, actionable
hint, and optional deterministic context. The schema is
`aros-tool-diagnostics-v1`. Current code families are:

| Code | Boundary |
| --- | --- |
| `AR0001` | command-line invocation |
| `AR0002` | diagnostic rendering or local logging |
| `AR0101` | repository discovery |
| `AR0201` | workspace or profile configuration |
| `AR0301` | host-tool resolution or execution |
| `AR0401` | AROS toolchain selection, verification, or installation |
| `AR0501` | upstream/network operation |
| `AR0601` | CMake configuration |
| `AR0602` | build execution |
| `AR0701` | boot validation |
| `AR0801` | Raspberry Pi operation |
| `AR0802` | removable-media safety |
| `AR0901` | output publication, cleanup, deployment, or golden data |
| `AR0999` | internal invariant |

Child-process exit codes and signals are preserved as structured context. In
JSON mode, non-interactive child output is isolated from the diagnostic
stream. A failed child's standard output and error are included in the
diagnostic and bounded to 64 KiB per stream. Successful output is replayed
unchanged. An explicitly interactive serial-console process retains its
terminal.

## Opt-in local logs

Logging is disabled by default and never selects an implicit destination:

```console
aros build --preset pc-x86_64 \
  --log-level info \
  --log-format jsonl \
  --log-file build/aros-cli.jsonl
```

`--log-level` accepts `off`, `error`, `warn`, `info`, `debug`, and `trace`;
`--log-format` accepts `human` and `jsonl`. Supplying only `--log-file` enables
the `info` level. The environment equivalents are `AROS_LOG_LEVEL`,
`AROS_LOG_FORMAT`, and `AROS_LOG_FILE`.

JSONL records use `aros-cli-log-v1`. They contain the selected command context
but no timestamp, hostname, CI-runner observation, or other ambient metadata.
Logs and fatal diagnostics are separate contracts; a logging failure becomes
an `AR0002` diagnostic and fails the command.
