# `aros-collect`

`aros-collect` links AROS relocatable objects and materialises AROS symbol
sets and library-version requirements. The same executable also provides the
released `collect-aros` and `collect-aros32` compiler-driver aliases.

## One collection engine

Both invocation forms are parsed into one engine request and use the same
staging, ELF inspection, symbol-set and library-requirement discovery, linker
script generation, second link, cleanup, diagnostics, logging, and atomic
publication path. The front ends retain only their deliberate policy
differences:

| Behaviour | direct `aros-collect --ld` | `collect-aros[32]` alias |
| --- | --- | --- |
| first-link arguments | preserve the explicit CMake contract | add `-r` when absent |
| empty collection | publish the first pass immediately | retain the reference two-pass flow |
| collector-owned extras and library resupply | disabled | enabled from the explicit sysroot |
| undefined-symbol audit, AROS ABI byte, executable permissions | disabled | enabled |
| report and retained linker-script paths | accepted | not exposed |

Both paths stage beside the requested output. A failed first or second link
therefore leaves an existing good output untouched. Temporary files are
removed unless `COLLECT_AROS_DEBUG` is set; an explicitly requested retained
script is never treated as temporary.

## Diagnostics

Human-readable diagnostics are the default. Machine consumers can request one
versioned JSON document on `stderr`:

```text
--diagnostic-format json
```

The document uses the shared `aros-tool-diagnostics-v1` schema. Warnings and a
possible terminal error are collected and rendered together exactly once per
invocation. `stdout` remains available for help, version, and intentional tool
output.

Collector codes are stable command-line API:

| Code | Meaning |
| --- | --- |
| `AC0001` | invalid collector invocation |
| `AC0002` | diagnostics or local logging failure |
| `AC0101` | required tool cannot be resolved |
| `AC0102` | invalid or incomplete AROS sysroot |
| `AC0201` | response-file expansion failure |
| `AC0301` | first relocatable link failure |
| `AC0302` | set-collection link failure |
| `AC0401` | linked-object inspection failure |
| `AC0501` | symbol-set or library-requirement collection issue |
| `AC0502` | collector-required sysroot input is missing |
| `AC0601` | undefined symbols remain after the final link |
| `AC0701` | AROS ELF ABI marking failure |
| `AC0702` | output stripping failure |
| `AC0801` | atomic output publication failure |
| `AC0901` | internal collector invariant failure |

Diagnostics may include typed context such as the tool, link mode, output,
argument index, exit code, signal, and log path. They deliberately exclude
timestamps, host names, and ambient environment snapshots.

## Local logging

Logging is opt-in and never sends telemetry. When enabled, a local file is
mandatory:

```text
--log-level info --log-format human --log-file build/collector.log
--log-level debug --log-format jsonl --log-file build/collector.jsonl
```

Supported levels are `off`, `error`, `warn`, `info`, `debug`, and `trace`.
The default is `off`; specifying only `--log-file` selects `info`. Log files
are opened in append mode. Strictly concurrent builds should select one file
per collector invocation and merge JSONL records afterwards. JSONL records use
the stable `aros-collect-log-v1` schema. Diagnostic events carry the same
stable code and stage as the corresponding human or JSON diagnostic.

Records do not automatically add a timestamp or machine identity. Paths that
are explicit parts of the invocation may still appear. Logs are observational
data and must not be included in byte-deterministic release archives.

Both the direct command and the compiler-driver aliases accept the same
settings. Namespaced spellings such as `--aros-log-file` are also accepted.
Environment equivalents are:

- `AROS_COLLECT_DIAGNOSTIC_FORMAT`
- `AROS_COLLECT_LOG_LEVEL`
- `AROS_COLLECT_LOG_FORMAT`
- `AROS_COLLECT_LOG_FILE`

Command-line settings take precedence. An explicit `--` ends collector-option
processing and preserves every following argument for the linker.
