# Public CLI contracts and continuously aligned documentation

Epic: [#148](https://github.com/metaneutrons/aros-tools/issues/148).
Decision state: CLI-M1 through CLI-M6 accepted. The exact integrated evidence
is recorded in [the CLI-M6 ledger](public-cli-m6-evidence.md).

## Outcome and boundaries

Make `aros` understandable from its help, predictable in scripts, and faithful
to its documented effects. Every accepted option must express an actual choice
or a useful explicit assertion. Required inputs, invalid combinations, path
origins, output formats and mutation scope must agree between parser, owner
implementation, tests and the public Astro/Starlight documentation.

The [source-backed baseline audit](public-cli-audit.md) records the exact source
revision, complete 53-leaf inventory, reproduced failures and inspection
limits. It separates defects from intentionally different protocols and
proposed new capabilities. This plan owns requirements; its epic owns the
initiative index and milestone issues own execution state and evidence.

Scope: the full visible `aros` frontend, its native producer commands, shared
process/diagnostic contracts, and the seven other distributed programs at
their documented invocation/observability boundaries. Specialized execution
remains in the owning crates. Companion algorithms, compiler qualification,
board boot support, cache storage algorithms and release publication are not
reimplemented or certified by this initiative.

"Complete" means every criterion is evidenced and no in-scope finding remains
unresolved. It is not a claim of mathematical perfection or newly qualified
hardware, operating systems or compiler outputs.

## Ownership and relationship to the cache initiative

The proposed [cache plan](https://github.com/metaneutrons/aros-tools/blob/fix/cache-management-plan/docs/cache-management-plan.md)
is tracked by [epic #139](https://github.com/metaneutrons/aros-tools/issues/139)
and [PR #147](https://github.com/metaneutrons/aros-tools/pull/147).
Keep one normative owner for each change:

| Contract | Implementation owner | Integration obligation here |
| --- | --- | --- |
| Public command facts, examples, path/argument contracts and shared output/diagnostics | This plan | CLI-M1 through CLI-M6 |
| Current install `force/local/offline` validation | CLI-M2; evidence reusable by CACHE-M3-A3 | Coordinate future `--refresh` spelling with the cache plan |
| Native `build`/`plan` cache-only invariant and flag removal | CLI-M3; evidence reusable by CACHE-M7-A1 | Migrate exact pinned producer callers before removing their accepted syntax |
| `aros cache compiler` hierarchy; redundant `ccache --stats`; false sccache clear; backend/root selection | CACHE-M1, issue #140 | CLI-M6 requires the F05/F06 repair evidence under CACHE-M1-A3, without claiming the whole cache milestone is accepted |
| Source/archive/vendor/GenMF operations, shared leases and retention | CACHE-M2 through CACHE-M6 | New surfaces use the CLI-M1 docs/semantic contract when introduced |
| Final cache frontend/consumer migration | CACHE-M7, issue #146 | Reuse CLI-M2/M3 evidence and extend the same docs gate; do not create another command catalog |

The false-clear regression can be fixed immediately within CACHE-M1. This
epic does not require the entire cache lifecycle to finish. It may close with
the current cache frontend only if its supported effects are truthful and the
remaining hierarchy migration is explicitly still tracked by #139. Never
report that the cache initiative is complete from this epic's closure.

Dependencies are acyclic: cache storage implementation is not a prerequisite
of CLI-M1; CLI-M3 does not wait for CACHE-M7; CACHE-M7 can consume earlier CLI
evidence. Resolve overlapping plan wording by recording that shared evidence,
not by waiting for both epics to close. Changes to a normative cache decision
need a coordinated change to its existing plan.

## Design decisions

### One command model, two kinds of evidence

Derive structural reference facts from the actual Clap command model, using
`CommandFactory::command()` and its reflection API. Keep parsing definitions
in a testable frontend module; do not create a second hand-maintained parser
schema. The development exporter records command paths, public role, option
names, arity, requiredness, defaults, value sets, environment binding names and
available argument relations. Conditional/custom validation remains covered
by executable semantic cases where reflection cannot express it completely.
Do not serialize ambient environment values, host paths or secrets.

A reviewed generated reference fragment supplies mechanical facts to Astro.
Handwritten workflow pages explain tasks, effects, limits and recovery. A
small source-linked semantic catalog connects documented cases to parser and
owning-handler tests. It records facts that Clap does not know: path origin,
repository requirement, reads/writes, network/daemon effects, output contract
and unsupported cases. It must not duplicate backend algorithms or become
a parallel global policy engine.

The existing leaf check remains useful as an inventory guard; replace its
manually maintained numeric authority with the derived inventory. A changed
command or option requires an intentional reviewed reference diff. Positive
and negative examples must assert parser classification and observable
effects, not merely that the same help string appears in another file.

### Astro alignment is required in every implementation slice

Every implementation PR must identify the changed commands/options, update
the generated facts and affected Astro reference/workflow/configuration/
troubleshooting examples, and link executable evidence. There is no final
documentation milestone that permits earlier stale documentation.

The docs gate checks generated-reference drift, links and recorded example
contracts, followed by the existing locked Astro build. Safe examples execute
as copied in isolated fixtures. Expensive build, live-network and hardware
examples receive parser checks and owner-level fixture coverage during
iteration, plus their existing native/hardware qualification only when the
claimed behavior requires it. Do not run all website shell blocks blindly.
Preserve multiline arguments and literal escapes when extracting examples.

Every relevant fenced CLI example is linked to an executable case or an
explicitly justified non-executable example class. A newly uncovered example
fails the coverage gate; labeling all examples "manual" cannot satisfy it.
Tests must prove the gate detects a deliberately stale model/default/flag and
an invalid example as well as accepting valid documentation. Narrative
semantics still require source review; generated facts cannot prove prose.

Keep Astro/Starlight and its documentation-focused design. This work adds no
hosting, account, token, deployment or private operational details to public
documentation. Proposed interfaces appear in repository plans until their
implementation ships; public usage pages must match the actual release or
clearly identify an unreleased change and its version boundary.

### Parse intent before resolving inputs

Reject statically contradictory options before logger initialization,
repository discovery or input inspection. Keep owner-side validation for
library callers and conditions dependent on real files or selected profiles.
Use typed request variants for different operations, with shared context
fields where they genuinely agree.

Current install commands reject `--force --local` and `--force --offline`;
`--local --offline` remains valid. Include environment-selected offline mode.
Do not impose that conflict on the legacy-compatible fetcher's different
`--force` operation. Board apply/preview conflicts belong to parsing; keep
`--dry-run` as a useful assertion. Zero resource limits fail early with the
correct unit and accepted domain. Do not invent universal timeout/memory caps.

Retain `--profile` for a named board configuration, `--model` for hardware and
`--transport` for a supported transport. Unsupported model/transport options
must fail with the supported alternatives before writing a profile. Template
availability, build readiness and observed boot support remain separate.

Rename only `source sync --ref` to `--branch`; initialization keeps `--ref`
and its documented detached-checkout semantics. Source root help mentions both
creation and synchronization. No silent compatibility aliases are introduced.

Make whole-build cleanup explicit: `clean --preset NAME` or `clean --all`,
mutually exclusive, with `--dry-run` showing exactly the selected build path.
Omitted selection prints actionable usage without removing anything. Cleanup
implementation keeps its existing containment/ownership boundary and must not
expand into caches, logs outside that build tree or installed toolchains.
This deliberate behavior change is recorded in migration notes.

### Stable path origins

Capture the invocation directory once. User-supplied relative filesystem
paths resolve against that directory before any command changes execution
directory. Prefer passing the discovered repository root explicitly and using
per-child `current_dir` over changing the entire process directory. Defaults
defined as checkout-relative remain checkout-relative. Preserve explicitly
documented producer-root-relative recipe members and config-file-relative
paths. Absolute-only managed-store paths and token validation keep their
existing restrictions.

Do not canonicalize an absent output path just to make it absolute. Preserve
non-UTF-8 paths where the existing platform API permits them, spaces, lexical
containment checks and each owner's no-follow validation. The semantic catalog
names each path's origin; it must not treat all `PathBuf` values identically.

### Native execution is cache-only by construction

Remove the public `--offline` and its environment-derived choice from native
`toolchain build` and `toolchain plan`. Encode prepared-input-only execution
in the typed producer request so a normal caller cannot choose an unsupported
online build. If a persisted plan/receipt retains an `offline` field, it records
the actual true invariant; stale contradictory input is rejected explicitly.
Keep readiness checks for recipe, roots, resource budgets and source identity.
Planning still neither scans/populates caches nor reserves build roots.

Keep offline switches on actual acquisition operations and ordinary product
builds. Missing producer inputs report the missing identity/root and the
currently available preparation command; never advise a removed `--offline`
switch or invent a future `aros cache` command before it exists. This describes
controlled fetch policy, not an OS-level network sandbox.

Split producer package and verify-package arguments: package requires output;
verification has no output option. Retain shared recipe/profile/context types.
Publish a coordinated invocation migration for exact tools pins in
`aros-toolchains`; immutable old releases retain their original contracts.

### Diagnostic, result and mutation outcomes

`--diagnostic-format json` controls diagnostics on stderr; `--format json`
controls the successful result on stdout only where supported. In managed
machine mode, stderr is empty on ordinary success or one complete versioned
diagnostic envelope when diagnostics exist. Successful child warnings must
be captured/logged or represented structurally; they cannot precede later JSON
as raw bytes. Preserve meaningful warnings with bounded context. Interactive
serial-terminal passthrough keeps its explicit terminal contract.

Retain diagnostic codes, detailed causes, exact command paths, process status,
signal/timeout fields and specific remediation. Parser errors use the
invocation boundary; missing recipe inputs do not masquerade as compiler
installation failures. Do not invent failure-code renumbering or a universal
JSON result for interactive commands.

The frontend must retain actual operation outcomes through final stdout and
log reporting. A reporting failure after mutation reports committed or
indeterminate state as evidenced by the owner. A preview never reports a
commit merely because `--apply` appeared. Existing rollback and broken-pipe
semantics remain tested. Add failure injection at the actual final boundary,
not only a preflight unwritable-log test.

Unify the effective logging rule through shared code where possible: explicit
CLI level overrides environment; explicit off from either source disables
logging without creating/opening a sink; file-only logging defaults to info;
no file and no level leaves logging off; a non-off level without a sink fails
actionably. Component environment prefixes and upstream protocol argument
forwarding remain distinct. Document the behavior transition for companions
that previously left file-only logging disabled.

### Bounded discoverability improvements

Add `aros completions bash|zsh|fish` as a pure stdout generator derived from the
same public command model. It performs no repository discovery, network call
or file installation. Test generation and shell syntax on available shells;
package-manager installation of completions is a separate publication change.

Add versioned `--format human|json` results to `info` and `toolchain list`,
using their existing observations. Preserve metadata-only/verified/unavailable
distinctions and checkout requirements. Do not introduce hidden network or
compiler execution for formatting. Keep `toolchain path` a composable single
path result. Help should direct normal users to installation/build workflows
and identify low-level producer commands as maintainer operations.

## Delivery and acceptance

The Astro alignment contract above applies to **every** milestone and every
implementation PR. The explicit documentation criterion in each milestone is
mandatory, even when the last milestone has not started.

### CLI-M1

Command inventory, semantic contracts and continuous Astro alignment.
Execution: [#149](https://github.com/metaneutrons/aros-tools/issues/149). Dependencies: none.

- CLI-M1-A1: Review the baseline findings, path/role taxonomy, cache ownership
  mapping and migration decisions; freeze an exact plan revision before
  accepting implementation. Record whether a change is a defect or deliberate
  interface improvement.
- CLI-M1-A2: Derive all public command/option facts from the actual parser and
  source-linked protocol adapters; exclude hidden/internal forms explicitly.
  No manually maintained command count or second parser specification owns
  facts. Existing eight-program distribution membership remains unchanged.
- CLI-M1-A3: Implement a semantic-case and example-coverage mechanism covering
  required/default/conflict/value/environment contracts and actual effects.
  New unclassified options/examples fail; documented assertion switches such
  as dry-run remain legitimate. Valid and deliberately stale cases prove both
  acceptance and detection.
- CLI-M1-A4: Correct F01/F14/F15 now, including exact board model/transport
  defaults, compiler-only resume, producer diagnostics and the actual suite
  install verification boundary. Connect generated facts and
  example checking to the canonical docs/portable gates. Astro references and
  workflows must agree at this milestone, without waiting for CLI-M6.

### CLI-M2

Predictable consumer arguments, source terminology and path resolution.
Execution: [#150](https://github.com/metaneutrons/aros-tools/issues/150). Dependencies: CLI-M1 contracts.

- CLI-M2-A1: Reject force/local/offline conflicts for all three applicable
  install entry points, including offline selected by environment. Prove valid
  local/offline and ordinary cached install remain valid. Conflicts fail at
  invocation before opening logs, resolving checkout or touching the cache.
- CLI-M2-A2: Source sync uses `--branch`; init `--ref` semantics stay exact.
  Reject the removed sync spelling; validate branch values at the appropriate
  boundary and preserve the existing clean/fast-forward/rollback guarantees.
- CLI-M2-A3: All affected explicit path arguments use the documented origin.
  Nested-cwd fixtures prove local prefix, board config, artifact, engine,
  module/evidence and logging paths; producer/config-relative exceptions and
  absent output destinations keep their own validated contracts.
- CLI-M2-A4: Positive resource domains and board mutation conflicts are
  parser-enforced. Clean requires one explicit scope, supports a non-mutating
  preview and preserves other build presets/caches/stores. Tests use isolated
  fixture trees and fake child programs; no real board or user build is removed.
- CLI-M2-A5: Update Astro setup/toolchain/source/build/board and configuration
  examples, including migration from sync `--ref` and bare clean. Lift the
  changed examples into the semantic tests; run the docs gate in this PR set.

### CLI-M3

Native producer invocation and cache-only execution contracts.
Execution: [#151](https://github.com/metaneutrons/aros-tools/issues/151). Dependencies: CLI-M1 contracts.

- CLI-M3-A1: Native build/plan accept no offline switch or environment choice;
  the request/readiness/receipt contracts express prepared-input-only execution
  coherently. Missing-cache failures name actual preparation requirements;
  ordinary build/fetch online/offline behavior is unchanged and covered.
- CLI-M3-A2: Package requires output in help and parsing; verify-package rejects
  that option in parsing. Shared package context stays single-sourced. Invalid
  invocations fail with specific invocation diagnostics before reading inputs.
- CLI-M3-A3: Inventory all active pinned producer callers, update them in linked
  narrow PRs and record exact compatible tools revisions before removing their
  syntax. Do not retarget old tags or leave current workflows on broken calls.
  Synthetic native lifecycle/package tests prove unchanged owned outputs,
  cancellation, resume boundaries and cache-miss behavior.
- CLI-M3-A4: Update the native-producer Astro workflow, reference, configuration
  and troubleshooting together, including compiler-only resume and cache
  preparation available at that code revision. Validate copied argument
  sequences and their readiness/result interpretation.

### CLI-M4

Reliable diagnostics, logging and post-operation reporting.
Execution: [#152](https://github.com/metaneutrons/aros-tools/issues/152). Dependencies: CLI-M1 contracts.

- CLI-M4-A1: One successful noisy child followed by one failing child still
  yields exactly one parseable stderr diagnostic document. Preserve bounded
  warnings and process causes. Verify independent success-result JSON,
  diagnostic JSON, human mode and explicit interactive-console behavior.
- CLI-M4-A2: Implement shared effective logging precedence across the eight
  programs or document a narrowly required protocol exception with source and
  tests. Explicit off cannot create a log. Cover omitted/CLI/environment level,
  sink-only activation, invalid sink and malformed invocation.
- CLI-M4-A3: Propagate actual mutation outcomes through final stdout/log errors
  for source, installation, toolchain management and board publication paths.
  Failure injection proves committed/rolled-back/indeterminate/preview states
  without automatic retry, duplicate mutations or fabricated rollback.
- CLI-M4-A4: Specific leaf-command context and actionable error hints survive
  parser/library/child boundaries. Preserve current stable codes and existing
  broken-pipe, timeout and cancellation behavior. Do not mark a partial result
  or blocked plan as qualified just because process exit is zero.
- CLI-M4-A5: Update Astro diagnostics, configuration and recovery guidance in
  each reporting change. Example JSON is validated against actual envelopes;
  documented logging precedence is exercised through real process boundaries.

### CLI-M5

Discoverable help, structured inspection and shell integration.
Execution: [#153](https://github.com/metaneutrons/aros-tools/issues/153). Dependencies: CLI-M1, CLI-M4 output contract.

- CLI-M5-A1: Root/group/leaf help describes intent, input scope, default effect
  and supported alternatives, and separates ordinary workflows from maintainer
  stages. Complete help traversal and invalid-input probes work without a
  checkout, configured board or external tools.
- CLI-M5-A2: `info` and `toolchain list` expose explicit human/JSON results with
  versioned schemas and truthful availability/verification states. Preserve
  each command's existing I/O and checkout policy. `toolchain path` remains a
  single composable path; JSON diagnostics never imply JSON success output.
- CLI-M5-A3: Static bash/zsh/fish completion comes from the actual command model,
  includes current public options and excludes removed/internal forms. Validate
  deterministic generation without network/config probing, with shell syntax
  evidence and precise omissions where a shell is unavailable.
- CLI-M5-A4: Update Astro CLI, automation and installation usage for the newly
  shipped commands/formats, with copyable examples and source-derived accepted
  values. Do not claim package-installed completions before that channel ships
  them. Keep upstream companion invocation syntax intact.

### CLI-M6

Integrated CLI and documentation qualification.
Execution: [#154](https://github.com/metaneutrons/aros-tools/issues/154). Dependencies: CLI-M1 through CLI-M5 and
the F05/F06 repair evidence under CACHE-M1-A3.

- CLI-M6-A1: Map every F01–F15 finding and D01–D03 improvement to its exact
  implementation/evidence; require linked CACHE-M1 evidence for F05/F06.
  Remaining cache-family delivery stays explicitly in #139. The entire cache
  epic is not a dependency or a claimed accomplishment of this milestone.
- CLI-M6-A2: All public leaves/options and all relevant Astro CLI examples have
  current structural and semantic coverage. Reconcile migrated README,
  configuration/troubleshooting, repository contracts and pinned workflow
  callers. No shipped-command example points to a removed flag or unimplemented
  replacement; all qualified limitations remain visible.
- CLI-M6-A3: On the final executable candidate, record Linux x86-64 and macOS
  ARM64 evidence for parser/output/path/fixture workflows, plus active CI
  coverage. Exercise source init/sync, setup/local verification, build command
  forwarding, native readiness, board preview and toolchain lifecycle using
  isolated fixtures and existing qualified inputs. Hardware boot is not
  inferred from template or fixture success.
- CLI-M6-A4: Run the canonical integration gate at affected source/CMake/
  runner boundaries once on the final relevant candidate, as required by
  CONTRIBUTING. Retain existing supported-host policy, Intel-macOS suspension
  and identity-affecting producer qualification gates. No unconditional full
  compiler A/B matrix is added for help/parser/documentation changes.
- CLI-M6-A5: Record exact plan/code/input revisions, durable evidence per
  criterion, supported hosts and omissions. All milestones' Astro evidence
  must already be current. A changed executable contract requalifies affected
  evidence; unrelated prose does not require another compiler build. Close
  only after all dependencies and criteria are satisfied.

## Migration, ordering and verification cost

Start with the reference correction and contract foundation, then prioritize
JSON stream repair and narrow cache false-clear repair. Consumer/parser/path
work and native producer work can proceed independently against the accepted
contracts. Add discovery once output contracts are stable, then qualify the
combined candidate. Milestones may contain several functional PRs; a partial
PR uses a non-closing issue reference.

The repository already has released interfaces; no "unreleased, so nothing
can break" assumption is allowed. Record each incompatible flag/default/logging
change with English Conventional Commits and breaking-change notes as required
by CONTRIBUTING. Release Please remains version authority. No hidden alias,
silent parser fallback or mutable producer pin is used for migration. Reverting
a change also restores its docs/callers coherently; old immutable versions and
their receipts are retained rather than rewritten.

Use focused parser/fixture and docs gates during iteration. Native integration
is required at the affected cross-cutting boundaries, not after each prose
change. The planning PR is documentation-only and does not require a product
or compiler build. No deadlines or runner-duration estimates are asserted
without measured implementation/fixture timings.

Open implementation investigations are bounded by criteria: complete the path
origin inventory, prove final-reporting failure states and enumerate exact
active producer callers. They are not waivers or evidence of completion.
No general CLI redesign, new crate hierarchy, CI-policy relaxation or public
hosting change is required merely to add these contracts.

Handle suspected vulnerabilities through [SECURITY.md](../SECURITY.md), not
public reproduction issues. Any privately tracked defect affecting an in-scope
boundary must be resolved or explicitly accounted for before final acceptance;
the public milestone must not claim that an unverified boundary is qualified.

## Reference decisions and completion

- [Clap Command reflection](https://docs.rs/clap/latest/clap/struct.Command.html)
  and [Arg contracts](https://docs.rs/clap/latest/clap/struct.Arg.html): derive
  mechanical facts from the parser and keep executable validation of effects.
- [Command Line Interface Guidelines](https://clig.dev/): useful guidance for
  help, explicit intent, scripting and output; project compatibility and
  measurable behavior decide this plan's actual requirements.
- [CONTRIBUTING](../CONTRIBUTING.md) owns current integration/release policy;
  the [producer contract](toolchain-producer-contract.md) and
  [management contract](toolchain-management-contract.md) retain their owning
  boundaries unless an implementation PR explicitly migrates them.

This initiative completes when all six milestone criteria and their exact
external evidence dependencies are satisfied, with continuously aligned public
documentation. Merge, implementation qualification and publication are
separate outcomes. Releasing packages or deploying a website is not an implicit
effect of closing this planning epic.

## Decision changes

2026-09-13: Initial proposal following the full frontend inventory and focused
process probes. Astro alignment is a per-slice requirement. Cache ownership
remains with #139 except for the explicitly allocated parser/native-invariant
slices whose evidence the cache plan may reuse.

2026-09-13: CLI-M1 through CLI-M5 were accepted through #157, #158, #159,
#160 and #161. The narrow CACHE-M1-A3 F05/F06 repair was merged in #163;
CACHE-M1 and cache epic #139 remain open. CLI-M6 collects integrated final
evidence without claiming either cache milestone complete.

2026-09-13: CLI-M6 was accepted after #164 merged as
[`bd9346d`](https://github.com/metaneutrons/aros-tools/commit/bd9346da7f5fcffb7f1b7f1e4b3099a995259a94).
The [CLI-M6 evidence ledger](public-cli-m6-evidence.md) records the exact
candidate tree, source input, supported hosts, CI, parser/documentation
coverage and deliberate omissions. It reuses the narrow F05/F06 evidence from
#163 without closing CACHE-M1 or cache epic #139.
