# Development handoff

Checkpoint verified on 2026-09-07. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`0fa0a37375afc00aafe2c848f1a2a58896d05512`](https://github.com/metaneutrons/aros-tools/commit/0fa0a37375afc00aafe2c848f1a2a58896d05512),
  the merged [PR #72](https://github.com/metaneutrons/aros-tools/pull/72).
- TCP-M0 through TCP-M4 are accepted. M3 local-build limits are recorded in
  [the TCP-M3 evidence ledger](docs/tcp-m3-native-evidence.md); M4 native
  packaging/read-back results and its strict boundary are recorded in
  [the TCP-M4 evidence ledger](docs/tcp-m4-native-evidence.md).
- M4 owns deterministic synthetic package inventory, manifests, sidecars,
  SBOMs and bounded archive read-back. The integrated main checkpoint is
  [Workspace CI run 34165692830](https://github.com/metaneutrons/aros-tools/actions/runs/34165692830),
  green on exactly the baseline commit. Its preceding four-host M4
  qualification is recorded in the evidence ledger.
- This remains neither compiler nor release qualification: it creates no tag,
  publication, attestation, A/B result or hardware evidence.

## Next bounded action

Begin TCP-M5 from the accepted M4 package boundary. Port compatibility,
replay and recovery without treating the M4 synthetic envelopes as released
toolchains. Keep all release, A/B and publication gates pending until TCP-M7.

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
  [producer contract](docs/toolchain-producer-contract.md) and
  [legacy-view evidence and limits](docs/toolchain-legacy-execution-view.md):
  implemented guarantees, measured evidence and explicit omissions.
- Keep raw logs, machine-specific paths, private operations notes and credential
  references outside Git in a stable private state directory, not a disposable
  build directory. Retained snapshots are evidence, not resumable guards or
  permission to execute archived scripts.
- This handoff is developer documentation in Git; it is not a public-site
  operations manual. Do not add hosting configuration or secrets here.
