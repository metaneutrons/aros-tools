# Development handoff

Checkpoint verified on 2026-09-07. This is a compact entry point, not a second
backlog, live CI dashboard or authorization to build, merge or publish.
Recheck linked issues and runs before acting; replace stale checkpoint text
instead of appending session history. Git history preserves earlier versions.

## Last verified implementation checkpoint

- Baseline: [`6f2956bcbde283732a134de0f1aad3a8cab470e9`](https://github.com/metaneutrons/aros-tools/commit/6f2956bcbde283732a134de0f1aad3a8cab470e9),
  the merged [PR #60](https://github.com/metaneutrons/aros-tools/pull/60),
  which supplies the declaration-bound native local lifecycle.
- TCP-M0, [TCP-M1 / #29](https://github.com/metaneutrons/aros-tools/issues/29)
  and [TCP-M2 / #30](https://github.com/metaneutrons/aros-tools/issues/30) are
  accepted. [TCP-M3 / #31](https://github.com/metaneutrons/aros-tools/issues/31)
  is the active issue.
- Branch `fix/tcp-m3-completion` extends the merged lifecycle with one safe,
  explicit recovery boundary: `--resume-from compiler` reacquires only the
  retained private roots, revalidates the snapshot/cache/receipt/output chain,
  then builds the collector in a fresh Cargo target root. It rejects partial
  compiler state, completed collector state and all malformed or changed input.
  The same change binds recursive snapshot digests and effective build flags to
  phase inputs, gives Cargo the declared job budget, and adds native failure,
  deadline, cancellation and retained-root tests.
- Current local verification is green: `cargo fmt --check`, `cargo clippy
  --workspace --all-targets --all-features --locked -- -D warnings`,
  `cargo test --locked -p aros-toolchain --test native_lifecycle`, `cargo test
  --locked -p aros-toolchain --test workspace_guards`, `cargo test --locked -p
  aros-toolchain --test executor`, `cargo test --locked -p aros-cli --test
  toolchain_plan_cli`, and `python3 scripts/toolchain_producer_contract_test.py`.
  These are not real compiler qualification.

## Next bounded action

Merge the M3 completion change before changing `aros-toolchains`. The official
producer currently has no committed `toolchains/producer-executor-v1.toml`; add
it only after the merged tools commit is known, so its contract digest and
`tools_commit` are measured rather than fabricated. Then create a clean recipe
and execute the six required native single-build diagnostics: all three
profiles on Linux x86-64 and macOS AArch64. Verify each produced local prefix
with `aros toolchain verify --local`; do not run a complete A/B matrix for this
milestone.

Issue #31 remains open until the M3 completion change is merged and the six
requested host/profile reports have recorded exact identities, resource
observations and local-prefix verification. Do not claim M3 completion from
synthetic lifecycle tests or a partial host/profile set.

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
