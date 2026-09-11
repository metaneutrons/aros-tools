# TCP-M5 native compatibility, replay and recovery evidence

Status: accepted implementation evidence, recorded 2026-09-08.

This ledger qualifies the M5 implementation boundary only. It is not a
toolchain release, a publication record, or a substitution for TCP-M7's live
three-host, three-profile qualification. Intel macOS remains schema-supported
and historical four-host evidence remains readable, but new Intel macOS
qualification is suspended until the initial release under
[aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27).

## Immutable identities

- Integrated implementation: [`3a229c9ae76849393d9051edfcf298ca1b0d488f`](https://github.com/metaneutrons/aros-tools/commit/3a229c9ae76849393d9051edfcf298ca1b0d488f)
  (merged PR [#93](https://github.com/metaneutrons/aros-tools/pull/93)).
- The independently reviewed PR candidate `4093de3` and the integrated commit
  have the same source tree:
  `8d52004ac4fa5e7e9817ade7c86d21df7ffeb56d`.
- The source-aware checkpoints used a clean, recursively initialized detached
  AROS-NX source tree at
  [`f3cfc243a84065166a46da28b0a5b22bbd0f8869`](https://github.com/metaneutrons/AROS-NX/commit/f3cfc243a84065166a46da28b0a5b22bbd0f8869).

## Compatibility execution

Merged PR [#92](https://github.com/metaneutrons/aros-tools/pull/92) introduced
the typed, tools-owned compatibility executor. Its six measured phases are
engine materialization, engine-free source validation, host-command closure,
closed Python environment, two-root relocation, and CMake/upstream/standalone
consumer probes. In particular, standalone C/C++ commands run with
`PATH=/nonexistent`; the upstream configure/Make invocation gets only the
measured owned closure.

The executor's dynamic contract test,
`compatibility::execution::tests::executes_every_phase_with_two_roots_and_closed_environments`,
and its negative root/closure tests pass on all four PR lanes:

| Host | Result | Duration |
| --- | --- | ---: |
| Linux x86_64 | passed | 8m 10s |
| Linux ARM64 | passed | 3m 34s |
| macOS ARM64 | passed | 7m 18s |
| macOS x86_64 | passed | 9m 24s |

## Evidence-bound replay and packaging

Merged PR [#93](https://github.com/metaneutrons/aros-tools/pull/93) adds the
native `RepackageRequest` and `repackage()` boundary. It accepts only evidence
already eligible under the native recovery policy and validates both output
roots. It has no credentials or network transport and no authority to create
tags, releases, indexes, or archive entries.

The policy test
`recovery::tests::repackage_requires_two_independent_verified_outputs_before_success`
and the complete `aros-toolchain` test suite passed on the four-host matrix:

| Host | Result | Duration |
| --- | --- | ---: |
| Linux x86_64 | passed | 8m 06s |
| Linux ARM64 | passed | 3m 25s |
| macOS ARM64 | passed | 8m 41s |
| macOS x86_64 | passed | 11m 59s |

Negative fixtures cover expired, missing and tampered evidence; non-packaging
failure recovery; tag and draft conflicts; interrupted hand-offs; changed and
non-regular assets; and missing or mismatched source and signer claims.

## Closed recovery-input handoff

The native producer records a recovery-eligible candidate in two deliberately
separate steps. For the active initial-release matrix,
`record-qualification` measures the final 44-member inventory through
no-follow file handles, verifies its checksum/index/package closure, and binds
all 18 build, 9 comparison and 9 compatibility receipts to one short-lived
qualification record. `prepare-recovery` then accepts that record only after
the protected workflow has independently verified the original attestation and
re-observed both annotated tags. It writes a closed recovery request, which
`validate-recovery` measures again at the point of use. The parser and recovery
validator retain the historical 56-member/four-host format only when its
measured release index proves that exact complete matrix.

The active recovery request cannot be constructed from nine retained archives
alone. A failed draft handoff without the complete final inventory, exact
qualification record, valid attestation, or immutable new tag is therefore a
fresh-qualification case. The native commands have no network, credential,
tag, release or publication authority.

## Source-aware checkpoints

The integrated Linux checkpoint was [Workspace CI run
34216774161](https://github.com/metaneutrons/aros-tools/actions/runs/34216774161)
on the exact integrated commit. Formatting/Clippy, planning and the Linux
x86_64 test job all completed successfully; the test job ran from 10:41:39 to
10:50:01 UTC.

An isolated macOS execution against the clean AROS-NX source root used:

```text
AROS_TEST_SOURCE_ROOT=/Volumes/Dev/Build/aros-nx-m5-qualified-source \
  scripts/check-workspace.sh test
```

It exited zero. The source-aware Rust suite exercised the real transpiler
configuration path, including
`real_aros_source_sync_runs_the_real_transpiler_when_explicitly_configured`.
The CMake product suite then reported `35 executed`, `0 host-qualified
omission(s)`, including the GRUB build and ISO-assets probes.

## Deliberate limit

These results establish native compatibility/replay/recovery parity at the M5
boundary. They do **not** establish a distributable toolchain. TCP-M7 still
requires one fresh live run of all nine active compatibility and relocation
lanes, 18 independent builds, 9 byte comparisons, isolated complete-draft
download verification, and unchanged publication plus consumer-promotion
checks. Intel macOS is deferred under
[aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27).
