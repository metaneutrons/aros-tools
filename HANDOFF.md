# Development handoff

Checkpoint verified on 2026-09-12. This is a compact entry point, not a second
backlog, live CI dashboard, or authorization to build, merge, or publish.
Recheck linked issues, runs, and release objects before acting. Git history
preserves earlier checkpoints.

## Last verified implementation checkpoint

- TCP-M0 through TCP-M6 remain accepted. Their durable implementation and
  evidence boundaries are recorded in the M3, M4, M5, and M6 evidence ledgers.
- M7's native executor is aros-tools
  [`253c11a52af6c4eff8d0e3db2ea2d33b740bef79`](https://github.com/metaneutrons/aros-tools/commit/253c11a52af6c4eff8d0e3db2ea2d33b740bef79).
  It accepts the canonical nested compatibility fetch markers and contains the
  Linux process-group PID-reuse repair exercised by the final qualification.
- The immutable
  [`toolchain-v1-20260912-rc8`](https://github.com/metaneutrons/aros-toolchains/releases/tag/toolchain-v1-20260912-rc8)
  prerelease was built from producer commit
  `d018d11dd6f995fc1f37d0b2b431f94a6ec78fd5` and AROS-NX
  `9369cc8f8ba4f7d320945c78788c6e2a6d0d1eab`. Its tag object is
  `6b904632d5fda7fea8f9259de27b34e77c1556db`.
- Producer run
  [`34699919725`](https://github.com/metaneutrons/aros-toolchains/actions/runs/34699919725)
  passed 18 independent builds, nine byte-identical A/B comparisons, nine
  compatibility/relocation lanes, and draft creation for the active 3×3 matrix:
  Linux x86-64, Linux AArch64, and macOS AArch64 across `pc-x86_64`,
  `arm-raspi`, and `rpi-aarch64`.
- Independent draft and published audits each verified the exact 44-asset
  inventory, all 43 non-self checksums, manifests, SBOMs, qualification
  receipts, 42 signed pre-provenance subjects, and all final URLs. The release
  was published unchanged as a prerelease only after the draft audit passed.
- A fresh macOS AArch64 aros-cli store passed `toolchain list`, online install,
  verification, and offline re-install for all three released profiles.

## Current bounded action

The remaining integration step is AROS-NX
[PR #30](https://github.com/metaneutrons/AROS-NX/pull/30), commit
`f5f968973d`. It copies only measured RC8 release-index values into the
consumer lock, activates the nine qualified entries, suspends Intel macOS
pending [aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27),
and preserves the disabled RISC-V entries. Merge it normally without changing
the immutable release or deriving values from local build output.

Intel macOS and RISC-V are explicit subsequent qualifications, not reasons to
change or retarget RC8.

## Resume safely

1. Inspect the current tag object, release state, producer run, PR #30, and
   working trees before taking action.
2. Use the published release index and manifests as the sole source of lock
   values. Never recover hashes, sizes, or tree digests from retained output.
3. Follow [the test-stage policy](CONTRIBUTING.md#test-stages-and-integration-checkpoints).
   Do not repeat expensive compiler or A/B work for documentation-only changes.
4. Keep raw logs, machine-specific paths, private operational notes, and
   credentials outside Git. This handoff is developer documentation, not a
   public-site operations manual.
