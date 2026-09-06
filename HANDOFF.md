# Development handoff

Checkpoint verified on 2026-09-06. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending a session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`a0ca5bacd3550249f037f7a5b17dce380f263e75`](https://github.com/metaneutrons/aros-tools/commit/a0ca5bacd3550249f037f7a5b17dce380f263e75),
  the regular squash merge of [PR #52](https://github.com/metaneutrons/aros-tools/pull/52).
- M0 is [accepted](docs/toolchain-producer-baseline.md#m0-acceptance);
  [M1 / #29](https://github.com/metaneutrons/aros-tools/issues/29) remains open.
- Read-only planning, owned work roots, raw snapshots, isolated legacy Git
  views and their storage-failure tests are implemented. The views are library
  primitives, not an integrated compiler adapter. Planning remains blocked;
  ordinary downloaded-toolchain consumer commands are unchanged.
- PR #52's [final-head integration](https://github.com/metaneutrons/aros-tools/actions/runs/34047346292),
  four-host PR tests, package qualification, documentation and CodeQL passed.
  Its merge's [Workspace CI](https://github.com/metaneutrons/aros-tools/actions/runs/34053399214),
  [CodeQL](https://github.com/metaneutrons/aros-tools/actions/runs/34053399346)
  and [Release Please](https://github.com/metaneutrons/aros-tools/actions/runs/34053399264)
  also passed. These are recorded runs, not evidence for later changes.

## Next implementation slice

Implement the executor-identity/origin and prerequisite/cache/environment
readiness boundary before allowing a compiler start. Distinguish the running
executor from the recipe-selected tools checkout; a self-declared digest is
not trusted origin. Keep failure diagnostics and cancellation aligned with the
existing shared boundaries.

Use [M1's acceptance criteria](docs/toolchain-producer-plan.md#tcp-m1--shared-library-boundary-and-cli-preview)
and the [producer contract](docs/toolchain-producer-contract.md), not a copied
checklist here. Adapter integration and real Linux x86-64/macOS AArch64 PC
builds with verified prefix use remain subsequent acceptance work.

## Resume safely

1. Read applicable repository instructions and inspect the working tree before
   choosing a branch. Preserve unrelated changes and retained run directories.
2. Recheck the [active issue](https://github.com/metaneutrons/aros-tools/issues/29),
   current PR heads and required checks. Never infer current completion from
   an older handoff or reuse stale qualification for changed executable code.
3. Resolve source/producer identities from the versioned contracts. Do not
   substitute neighboring checkouts, moving branches or retained build output.
4. Follow the [test-stage policy](CONTRIBUTING.md#test-stages-and-integration-checkpoints).
   Use one final-candidate integration checkpoint when required; do not repeat
   expensive compiler/GRUB/A-B builds for documentation-only changes.

## Records and ownership

- [Producer plan](docs/toolchain-producer-plan.md#9-repository-tracking-and-handoff):
  design, dependencies and acceptance criteria; issues own live execution state.
- [Implementation scope](crates/aros-toolchain/README.md) and
  [legacy-view evidence and limits](docs/toolchain-legacy-execution-view.md):
  implemented guarantees, measured evidence and explicit omissions.
- Keep raw logs, machine-specific paths, private operations notes and credential
  references outside Git in a stable private state directory, not a disposable
  build directory. Keep an index and checksums there. Retained snapshots are
  evidence, not resumable guards or permission to execute archived scripts.
- This handoff is developer documentation in Git; it is not a public-site
  operations manual. Do not add hosting configuration or secrets here.
