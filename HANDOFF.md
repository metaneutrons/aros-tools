# Development handoff

Checkpoint verified on 2026-09-07. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`1a77bc1ad24180893410d353792a20f98c4b489d`](https://github.com/metaneutrons/aros-tools/commit/1a77bc1ad24180893410d353792a20f98c4b489d),
  the merge of [PR #59](https://github.com/metaneutrons/aros-tools/pull/59),
  which establishes the risk-based CI matrix.
- TCP-M0, [TCP-M1 / #29](https://github.com/metaneutrons/aros-tools/issues/29)
  and [TCP-M2 / #30](https://github.com/metaneutrons/aros-tools/issues/30) are
  accepted. [TCP-M3 / #31](https://github.com/metaneutrons/aros-tools/issues/31)
  is the active issue.
- Branch `fix/tcp-m3-native-lifecycle` implements the native local lifecycle:
  declaration-bound snapshots and cache, private locked Python/Cargo inputs,
  source `configure`, source-owned `crosstools-release`, the internal verified
  MetaMake fetch bridge, exact `aros-collect` installation, process-group
  cancellation/deadlines and ordered self-verifying phase receipts. It produces
  a local-only candidate; it has no packaging, publication or origin-attestation
  authority.
- The branch's local verification is green: `cargo check -p aros-toolchain -p
  aros-cli`, `python3 scripts/toolchain_producer_contract_test.py`, the native
  synthetic lifecycle test, executor tests, and CLI plan/fetch-bridge tests.
  Re-run them after any change. These are not real compiler qualification.

## Next bounded action

Finish and review the TCP-M3 implementation PR before changing
`aros-toolchains`. The official producer currently has no committed
`toolchains/producer-executor-v1.toml`; add it only after the merged tools
commit is known, so its contract digest and `tools_commit` are measured rather
than fabricated. Then run the two required real single-build PC diagnostics:
Linux x86-64 and macOS AArch64. Do not run a complete A/B matrix for this
milestone.

Issue #31 remains open until it has the required receipt-revalidation/resume
boundary, fault/resource coverage and the six requested host/profile reports.
Do not claim M3 completion from the synthetic lifecycle or the two PC
diagnostics alone.

## Resume safely

1. Read applicable repository instructions and inspect the working tree before
   choosing a branch. Preserve unrelated changes and retained run directories.
2. Recheck [issue #31](https://github.com/metaneutrons/aros-tools/issues/31),
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
