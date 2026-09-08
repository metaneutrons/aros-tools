# Development handoff

Checkpoint verified on 2026-09-08. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`3a229c9ae76849393d9051edfcf298ca1b0d488f`](https://github.com/metaneutrons/aros-tools/commit/3a229c9ae76849393d9051edfcf298ca1b0d488f),
  the merged [PR #93](https://github.com/metaneutrons/aros-tools/pull/93).
- TCP-M0 through TCP-M5 are accepted. M3 local-build limits are recorded in
  [the TCP-M3 evidence ledger](docs/tcp-m3-native-evidence.md); M4 native
  packaging/read-back results and its strict boundary are recorded in
  [the TCP-M4 evidence ledger](docs/tcp-m4-native-evidence.md). M5 native
  compatibility execution, evidence-bound replay/repackage and source-aware
  checkpoints are recorded in [the TCP-M5 evidence ledger](docs/tcp-m5-native-evidence.md).
- M5 does not qualify a distributable compiler or release. There is no tag,
  publication, live A/B result or hardware evidence. The fresh 12-lane,
  four-host/three-profile release qualification remains TCP-M7 work.

## Current bounded action

TCP-M6 is in progress. `aros-tools` PR #96 adds the native closed recovery
qualification command; it must pass its four-host Workspace CI and merge before
`aros-toolchains` PR #13 may replace its provisional executor commit with the
exact merged `main` commit. This branch removes the retired public
`legacy-preview` adapter and its isolated Git-view implementation; it is not
mergeable before that dependency. Keep all release, A/B and publication gates
pending until TCP-M7.

## Resume safely

1. Read applicable repository instructions and inspect the working tree before
   choosing a branch. Preserve unrelated changes and retained run directories.
2. Recheck [issue #33](https://github.com/metaneutrons/aros-tools/issues/33),
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
  [TCP-M5 evidence ledger](docs/tcp-m5-native-evidence.md): implemented
  guarantees, measured evidence and explicit omissions.
- Keep raw logs, machine-specific paths, private operations notes and credential
  references outside Git in a stable private state directory, not a disposable
  build directory. Retained snapshots are evidence, not resumable guards or
  permission to execute archived scripts.
- This handoff is developer documentation in Git; it is not a public-site
  operations manual. Do not add hosting configuration or secrets here.
