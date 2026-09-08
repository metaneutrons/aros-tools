# Development handoff

Checkpoint verified on 2026-09-08. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`016a15b41cc6de346fb81487328d099b4705ccad`](https://github.com/metaneutrons/aros-tools/commit/016a15b41cc6de346fb81487328d099b4705ccad),
  the merged [PR #97](https://github.com/metaneutrons/aros-tools/pull/97).
- TCP-M0 through TCP-M6 are accepted. M3 local-build limits are recorded in
  [the TCP-M3 evidence ledger](docs/tcp-m3-native-evidence.md); M4 native
  packaging/read-back results and its strict boundary are recorded in
  [the TCP-M4 evidence ledger](docs/tcp-m4-native-evidence.md). M5 native
  compatibility execution, evidence-bound replay/repackage and source-aware
  checkpoints are recorded in [the TCP-M5 evidence ledger](docs/tcp-m5-native-evidence.md).
  The CI cutover, legacy retirement and lean real PC candidates are recorded in
  [the TCP-M6 evidence ledger](docs/tcp-m6-native-evidence.md).
- The final cross-repository producer declaration is
  [`03506956b25944f1476acf8c9398bbdf714638fe`](https://github.com/metaneutrons/aros-toolchains/commit/03506956b25944f1476acf8c9398bbdf714638fe).
  Its [contract run](https://github.com/metaneutrons/aros-toolchains/actions/runs/34260382908)
  and the final [four-host workspace run](https://github.com/metaneutrons/aros-tools/actions/runs/34259378712)
  are green.
- M6 does not qualify a distributable compiler or release. There is no tag,
  publication or live A/B result. The fresh 12-lane, four-host/three-profile
  release qualification remains TCP-M7 work.

## Current bounded action

TCP-M7 is next. Start only with clean, explicitly selected AROS-NX,
`aros-toolchains`, and `aros-tools` identities. It alone may select a new
immutable annotated toolchain tag and run the complete four-host,
three-profile A/B, compatibility, draft-verification and consumer-promotion
sequence. Do not use M6 local candidates or a retained build root as a release
input.

## Resume safely

1. Read applicable repository instructions and inspect the working tree before
   choosing a branch. Preserve unrelated changes and retained run directories.
2. Recheck [issue #34](https://github.com/metaneutrons/aros-tools/issues/34),
   current PR heads and required checks. Never infer current completion from
   this handoff or reuse stale qualification for changed executable code.
3. Resolve source/producer identities from the versioned contracts. Do not
   substitute neighboring checkouts, moving branches or retained build output.
4. Follow the [test-stage policy](CONTRIBUTING.md#test-stages-and-integration-checkpoints).
   Use a final-candidate integration checkpoint only when required; do not
   repeat expensive compiler/GRUB/A-B builds for documentation-only changes.

## Records and ownership

- [Producer plan](docs/toolchain-producer-plan.md#9-repository-tracking-and-handoff):
  design, dependencies and acceptance criteria; issues own live execution state.
- [Implementation scope](crates/aros-toolchain/README.md),
  [producer contract](docs/toolchain-producer-contract.md) and the
  [TCP-M6 evidence ledger](docs/tcp-m6-native-evidence.md): implemented
  guarantees, measured evidence and explicit omissions.
- Keep raw logs, machine-specific paths, private operations notes and credential
  references outside Git in a stable private state directory, not a disposable
  build directory. Retained snapshots are evidence, not resumable guards or
  permission to execute archived scripts.
- This handoff is developer documentation in Git; it is not a public-site
  operations manual. Do not add hosting configuration or secrets here.
