# `aros` CLI

`aros` is the user-facing orchestration boundary for an AROS-NG checkout. It
discovers the repository, selects declarative target and toolchain state,
builds checkout-local helpers, configures and builds AROS, validates boot
results, and drives guarded Raspberry Pi workflows. It invokes the transpiler,
collector, generators, and runners as independent processes rather than
linking their implementations.

## Command model

| Command | Responsibility |
| --- | --- |
| `setup` | install the host compiler or locked cross-toolchains |
| `host-compiler` | manage downloaded host LLVM (`host-tools` compatibility alias) |
| `build-tools` | build/check checkout-local Rust tools (`hosttools` compatibility alias) |
| `toolchain` | install, list, verify, and locate released AROS cross-toolchains |
| `build` / `clean` | configure and build a declared target profile |
| `test` | run evidence-producing, non-interactive QEMU boot validation |
| `pi` | validated board, network boot, serial, and removable-media workflows |
| `golden` | capture or compare deterministic transpiler output |
| `sync` | integrate upstream AROS and regenerate derived state |
| `info` | report active compiler, toolchain lock, and configured targets |

The root `aros-targets.toml` is the sole source of target profiles and host
compiler assets. Missing, invalid, or empty target configuration is fatal.
`AROS_HOST_COMPILER_DIR` and `AROS_HOST_COMPILER_URL` are the canonical local
overrides; the former `AROS_HOST_TOOLS_DIR` and `AROS_HOST_TOOLS_URL` names
remain compatibility fallbacks.
Released cross-toolchains are selected exclusively through
`aros-toolchains.lock.toml`; an explicit local toolchain remains an auditable
override and is never silently copied into the immutable store.

Raspberry Pi disk writes are a separate safety boundary. They require a fresh
scan identity, whole/removable/unmounted-media validation, explicit apply
intent, staged image verification, a platform-native exclusive claim, and
complete readback verification. The macOS claim helper does not replace any
of those checks.

## Diagnostics and local logging

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
