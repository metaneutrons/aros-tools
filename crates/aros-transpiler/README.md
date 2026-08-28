# AROS MetaMake transpiler contracts

The transpiler follows upstream MetaMake declarations and must not silently
freeze third-party packages or ordinary repository files at locally selected
bytes.

## Pin policy

- `%fetch` keeps the upstream archive name, version, URL, suffix and patch
  declaration. The transpiler does not add an archive checksum that upstream
  did not declare.
- Repository scripts and patches are direct build dependencies. Editing one
  invalidates the corresponding generated output or fetch result; it does not
  require updating a second checksum.
- Checked input manifests are path-only capability source closures. CMake
  calculates live per-file hashes during configuration, watches every listed
  input and passes the resulting snapshot to the runner only to detect changes
  between configuration and execution. Ordinary source edits never require a
  checked-in digest update.
- A fixed fingerprint is permitted only for an opaque recipe fragment or
  source inventory that the transpiler expands into a hard-coded job graph.
  The complete allowlist and rationale are in
  `capability-fingerprints.pins`. Fingerprint drift is fatal and tells the user
  that the capability and transpiler must be reviewed and updated.
- A component that CMake downloads directly without an upstream `%fetch`
  integrity mechanism must retain a supply-chain checksum. Currently this is
  the closed GRUB 2.12 source download in `cmake/GrubSourceLock.cmake`. This is
  not an `aros-cli` package pin.
- Released host and cross toolchains use separate, explicit release locks.
  Those locks are part of the reproducible distribution contract and are not
  transpiler capability fingerprints.

Never update a capability fingerprint merely to make a build pass. First
reduce the fingerprint to the smallest opaque input, review the semantic
change, update the hard-coded jobs if necessary, and exercise all supported
target profiles.

## Failure policy

Filesystem traversal, fetch discovery, MetaMake parsing and recognised
capability drift are fatal. Parallel failures are collected, sorted and
deduplicated so one invocation reports the complete deterministic set. Fatal
diagnostics carry stable `AT0001`–`AT0009` codes, a typed stage and severity,
an optional source location and an actionable hint. `--diagnostic-format json`
emits the versioned `aros-tool-diagnostics-v1` document on stderr; progress and
local logs cannot contaminate that stream. `AT0008` identifies invalid command
invocations and `AT0009` identifies observability failures. A capability deliberately excluded
from another supported architecture is not drift; an owned declaration whose
recipe no longer matches is. Ownership is decided by typed parser paths, never
by searching rendered diagnostic text.

Coverage gaps outside a recognised capability remain reports beside the
generated CMake graph. `generated_targets.coverage.json` indexes every report,
including zero-count reports, under stable `AT1001`–`AT1032` codes and explicit
`info` or `warning` severity. This is deliberately not a hidden baseline:
counts are current observations, not accepted-count pins. Gaps must remain
visible until they are either modelled or promoted to a release gate.

The CMake graph, source inventory, spec-switch manifest, coverage index and all
reports are rendered and fsynced into sibling staging files before any current
artifact is replaced. Replacement is one rollback-capable transaction and the
CMake graph is its last commit marker. A staging, replacement, report-removal
or directory-sync error is fatal and restores the previous generation when
rollback succeeds. Tests inject both a staging failure and a mid-commit
failure. The embedded fingerprint registry also has a non-panicking startup
validation gate before any source scanning begins.

## Local logging

Logging uses the shared AROS observability implementation and is disabled by
default. Enabling it requires an explicit local file:

```console
aros-transpiler --log-level info --log-format jsonl \
  --log-file build/aros-transpiler.jsonl
```

Levels are `off`, `error`, `warn`, `info`, `debug`, and `trace`; formats are
`human` and `jsonl`. The environment equivalents are
`AROS_TRANSPILER_LOG_LEVEL`, `AROS_TRANSPILER_LOG_FORMAT`, and
`AROS_TRANSPILER_LOG_FILE`. JSONL records use `aros-transpiler-log-v1` and omit
ambient timestamps and host identity so local logs remain explicit
observations, not inputs to deterministic artifacts.
