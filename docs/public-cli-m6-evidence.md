# CLI-M6 integrated public CLI qualification evidence

Status: acceptance evidence completed on 2026-09-14.

This ledger records the evidence originally assembled for
[CLI-M6](https://github.com/metaneutrons/aros-tools/issues/154) of the
[public CLI contract plan](public-cli-contract-plan.md). It includes the exact
pinned caller migration required by CLI-M3. It does not qualify hardware boot,
compiler artifacts, a toolchain release, or the remaining cache-management
initiative.

## Exact accepted identities

| Component | Identity | Role |
| --- | --- | --- |
| Integrated `aros-tools` | [`bd9346da7f5fcffb7f1b7f1e4b3099a995259a94`](https://github.com/metaneutrons/aros-tools/commit/bd9346da7f5fcffb7f1b7f1e4b3099a995259a94) | Squash merge of [PR #164](https://github.com/metaneutrons/aros-tools/pull/164) |
| Qualified PR head | [`780fa5bb71db17f9653db010c61d376ccdbb1f74`](https://github.com/metaneutrons/aros-tools/commit/780fa5bb71db17f9653db010c61d376ccdbb1f74) | Exact input of the successful PR workflows and local integration gate |
| Candidate source tree | `ab9684990c16ee6c29eef7586b320c5a06d761dc` | Measured identical Git tree for the qualified head and squash merge |
| Source-aware AROS-NX input | [`f3cfc243a84065166a46da28b0a5b22bbd0f8869`](https://github.com/metaneutrons/AROS-NX/commit/f3cfc243a84065166a46da28b0a5b22bbd0f8869) | Clean recursive source input for source-init, source-sync and transpiler integration |
| Native producer contract | [`c8039cf2b7291097ad62c6750bd7367e91a068f4`](https://github.com/metaneutrons/aros-toolchains/commit/c8039cf2b7291097ad62c6750bd7367e91a068f4) | Pinned producer-side contract consumed by the native lifecycle fixtures |
| Final caller migration | [`6266ab047058cc2b87e38d9f6018bb25d7b658a2`](https://github.com/metaneutrons/aros-toolchains/commit/6266ab047058cc2b87e38d9f6018bb25d7b658a2) | Merged [aros-toolchains PR #53](https://github.com/metaneutrons/aros-toolchains/pull/53), pinning the active producer to `2474bc3c89c21c80d23197c28ef63cd3c18603a4` without reintroducing `--offline` or legacy `aros ccache` syntax |

The measured tree equality means the PR workflows and local gate exercised the
exact integrated implementation, not merely a related branch.

## Acceptance criteria and evidence

| Criterion | Measured evidence | Result |
| --- | --- | --- |
| CLI-M6-A1 | Findings F01--F15 and improvements D01--D03 are mapped below. F05/F06 use [PR #163](https://github.com/metaneutrons/aros-tools/pull/163), merged as [`41fe09c`](https://github.com/metaneutrons/aros-tools/commit/41fe09c98dece5d9b56eee91d650ba23561040b1). | passed |
| CLI-M6-A2 | The source-derived public command reference covers every visible leaf. [PR #164](https://github.com/metaneutrons/aros-tools/pull/164) adds parser-backed validation of every fenced public `aros` invocation in all 12 documented pages and requires each page to name its semantic fixture owner. | passed |
| CLI-M6-A3 | The successful three-host [Workspace CI run 34760149199](https://github.com/metaneutrons/aros-tools/actions/runs/34760149199) covers Linux x86-64 source-coupled workflows, Linux AArch64 portable workflows and macOS AArch64 portable workflows. Existing isolated CLI fixtures cover source init/sync, local setup verification, build forwarding, native readiness, board preview and toolchain lifecycle. | passed |
| CLI-M6-A4 | On Darwin 25.5.0/arm64, `AROS_TEST_SOURCE_ROOT=/Volumes/Dev/Build/aros-tools-m6-aros-nx bash scripts/check-workspace.sh test` passed on the qualified tree: full locked Rust suite, the real source-init/sync/transpiler flow, and all 35 discovered host-compatible CMake fixtures, including the real GRUB host build. | passed |
| CLI-M6-A5 | This ledger records exact source/code identities, supported hosts, commands, CI and omissions. Public Astro documentation and migration text were updated in [PR #164](https://github.com/metaneutrons/aros-tools/pull/164). | passed |

The final caller migration passed its [pull-request producer contract](https://github.com/metaneutrons/aros-toolchains/actions/runs/34789852879) and the identical [main contract](https://github.com/metaneutrons/aros-toolchains/actions/runs/34789884726). That fulfills CLI-M3-A3 and removes the final dependency of CLI-M6.

The Workspace CI run also passed formatting, architecture and Clippy. Its
individual [Linux x86-64](https://github.com/metaneutrons/aros-tools/actions/runs/34760149199/job/103731457623),
[Linux AArch64](https://github.com/metaneutrons/aros-tools/actions/runs/34760149199/job/103731457569)
and [macOS AArch64](https://github.com/metaneutrons/aros-tools/actions/runs/34760149199/job/103731457635)
jobs all succeeded. The same PR head passed
[CodeQL](https://github.com/metaneutrons/aros-tools/actions/runs/34760149164)
and [the documentation gate](https://github.com/metaneutrons/aros-tools/actions/runs/34760149214).

## Finding and improvement disposition

| Baseline item | Resolution and durable evidence |
| --- | --- |
| F01 board model/template ambiguity | [PR #157](https://github.com/metaneutrons/aros-tools/pull/157) made model-specific profile selection typed and tested; the public reference is checked against that contract. |
| F02 incomplete documentation coverage | [PR #157](https://github.com/metaneutrons/aros-tools/pull/157) established source-derived command facts; [PR #164](https://github.com/metaneutrons/aros-tools/pull/164) adds the all-pages parser-backed example gate. |
| F03 native `--offline` contradiction | [PR #159](https://github.com/metaneutrons/aros-tools/pull/159) encoded cache-only native requests and removed the false choice; #164 aligns public and producer-contract prose. |
| F04 ignored/conflicting acquisition switches | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) makes invalid local/force and offline/force combinations parser errors. |
| F05 redundant compiler-statistics switch | [PR #163](https://github.com/metaneutrons/aros-tools/pull/163) removed the no-op `--stats` switch. CACHE-M7 subsequently removes the unscoped legacy frontend entirely in favour of `aros cache compiler stats`. |
| F06 false sccache-clear success | [PR #163](https://github.com/metaneutrons/aros-tools/pull/163) rejects unsupported sccache clearing before backend execution and retains real ccache clearing evidence. |
| F07 ambiguous source selector | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) distinguishes `source init --ref` from `source sync --branch`. |
| F08 producer package parser shape | [PR #159](https://github.com/metaneutrons/aros-tools/pull/159) separates packaging output from verification arguments. |
| F09 relative-path origin drift | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) captures invocation-directory origins before repository discovery. |
| F10 explicit logging-off override | [PR #160](https://github.com/metaneutrons/aros-tools/pull/160) preserves explicit off and tests the shared logging rule. |
| F11 mixed child stderr and JSON diagnostics | [PR #160](https://github.com/metaneutrons/aros-tools/pull/160) preserves one versioned diagnostic envelope. |
| F12 zero resource budgets | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) rejects invalid static values at the invocation boundary. |
| F13 late board mutation conflicts | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) moves `--apply`/`--dry-run` conflicts into parsing. |
| F14 stale native capability prose | [PR #157](https://github.com/metaneutrons/aros-tools/pull/157) records the actual compiler-only resume and producer diagnostic boundaries. |
| F15 overstated suite-install verification | [PR #157](https://github.com/metaneutrons/aros-tools/pull/157) documents exact inventory/snapshot verification without inventing version verification. |
| D01 lost post-operation reporting | [PR #160](https://github.com/metaneutrons/aros-tools/pull/160) carries source, installation, management and board operation outcomes through final reporting. |
| D02 missing structured discovery/completions | [PR #161](https://github.com/metaneutrons/aros-tools/pull/161) supplies deterministic completions and versioned `info`/`toolchain list` JSON results. |
| D03 implicit whole-tree clean | [PR #158](https://github.com/metaneutrons/aros-tools/pull/158) requires `--all` or `--preset`; #164 retains an explicit migration note for older unreleased revisions. |

## Documentation boundary

The new parser gate covers every fenced invocation beginning with `aros` or
`$AROS` below `docs-site/src/content/docs`. It verifies parser acceptance in
help-only mode, so documentation validation has no network, repository or
mutation side effect. The 12 pages with public examples each name one or more
owner fixtures; new example-bearing pages fail until that ownership is made
explicit. Generated command facts and the existing semantic fixtures remain
the authority for behavior; the page mapping is an accountability boundary,
not a second command schema.

The root README contains no copyable `aros` command invocation, so it has no
example to include in that parser gate. The baseline audit remains historical
diagnostic evidence, and the cache-management plan remains a future-facing
cache owner; neither is a statement that superseded syntax is still shipped.

## Explicit limits

This acceptance does not run a compiler release A/B matrix, publish a package,
produce a toolchain archive, change a release lock, deploy a board, or infer a
hardware boot result from a template or fixture. Intel macOS remains suspended
under [aros-toolchains#27](https://github.com/metaneutrons/aros-toolchains/issues/27);
it is not represented as a passing host. Cache epic
[#139](https://github.com/metaneutrons/aros-tools/issues/139) and CACHE-M1
[#140](https://github.com/metaneutrons/aros-tools/issues/140) remain open.
Only the narrow F05/F06 repair evidence is reused here.

The documentation-only record itself does not require another native or
compiler gate: it changes only `docs/**`, while the final executable candidate
was already qualified above. The repository policy assigns such a change the
source-independent Linux documentation/portable check; any later executable,
test, workflow, contract or source-input change invalidates the relevant
candidate evidence and requires requalification.
