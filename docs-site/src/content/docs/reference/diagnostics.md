---
title: Diagnostics and logs
description: Stable errors for people and automation without leaking host state.
---

User-facing tools emit the versioned `aros-tool-diagnostics-v1` contract.
Human output explains the failed stage and remediation; JSON mode emits one
document on standard error with stable component codes for automation.

```sh
aros --diagnostic-format json build --preset pc-x86_64
```

Every fatal diagnostic contains a code, severity, stage, message, actionable
hint and optional deterministic context. Code families identify the owner:

| Prefix | Owner |
| --- | --- |
| `AR` | `aros` orchestration |
| `AT` | MetaMake transpiler |
| `AV` | independent verifier |
| `AC` | collector/linker driver |
| `AF` | source fetcher |
| `AH` | AHI build runner |
| `AG` | module generator |
| `RM` | ROM tool |
| `AP` | release producer and publication scripts |

Existing codes are not reused when an error is retired. CLI parse errors,
missing inputs, validation failures, unsafe publication and internal invariants
are distinct classes; tests assert their machine-readable representation and
non-zero exit behavior.

Filesystem publication additionally carries a typed internal remediation class:
destination/CAS conflict, unsafe target, unsupported durability, incomplete
recovery, uncertain post-rename commit, or other I/O. `aros-genmodule` and
`aros-romtool` preserve their stable `AG`/`RM` codes while selecting actionable
hints from that class; automation still branches on the public diagnostic code,
not prose. Completed predecessor recovery is an explicit
`publication.recovery` log event (`rolled_back`, `completed_cleanup`, or
`removed_tree_stage`). Failure to write that post-recovery log is a warning and
cannot recast a completed filesystem mutation as failed.

Source lifecycle failures have stable sub-boundaries: `AR0111` input/ref,
`AR0112` transport, `AR0113` repository lock, `AR0114` mutable repository
state, `AR0115` standalone-candidate validation and `AR0116` publication/CAS
materialization. Automation should branch on these codes, not parse prose.
The `AR0113` lock file is a persistent owner record guarded by the operating
system; its presence alone is not a stale lock and it must not be deleted.
`AR0116` carries a typed `context.commit_state` value. `rolled_back` means the
exact branch/index/submodule snapshot was restored, `indeterminate` means that
neither final state could be proven, and `committed` means the mutation
succeeded but reporting it failed. Automation reads this field rather than
parsing a `committed=…` phrase from the message.

Local logs are opt-in and require an explicit destination. Deterministic logs
exclude timestamps, host identity, environment values and raw invocations.
They must never be included in release archives.

```sh
aros build --preset pc-x86_64 \
  --log-level debug --log-format jsonl --log-file ./aros.jsonl
```

Supplying a log level without a file is an error. Logs and fatal diagnostics
are separate contracts, and secrets are never recorded. Captured child stdout
and stderr are drained concurrently while retaining at most 64 KiB per stream;
oversized output has an explicit truncation marker. Interactive console
commands retain their terminal instead of being redirected.
Source Git operations carry a 30-minute process-group deadline and source graph
transpilation a 10-minute deadline. JSON failures retain `tool`, `timed_out`,
`timeout_ms`, exit code or signal, and bounded stream evidence through cleanup
error wrapping. Cleanup trouble after successful source publication is a
warning instead of a false operation failure; a closed stdout pipe is normal
consumer termination.

Failures are not converted into guessed output. In particular, the transpiler
stops when a reviewed opaque recipe changes, the fetcher refuses unverifiable
archives under strict policy, and board workflows reject ambiguous disks or
hardware identities before mutation.
