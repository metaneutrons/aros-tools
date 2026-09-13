# Public CLI audit baseline

Date: 2026-09-13. Source baseline:
[`1aed0c970f9a4df2fb047a1b5603ecea1d1a6bed`](https://github.com/metaneutrons/aros-tools/tree/1aed0c970f9a4df2fb047a1b5603ecea1d1a6bed).
The [public CLI contract plan](public-cli-contract-plan.md) owns proposed
solutions and acceptance criteria. This document records observations; it is
not an implementation or qualification report.

## Coverage and method

The complete visible `aros` command tree was traversed through `--help` after
`cargo build -p aros-cli --locked`: **53 leaf commands**, all help invocations
successful. The executable SHA-256 was
`ec751b039d2c52aca7e0e10a57d213cbcba370e3108d529f9058659586208f2b`.
This is a local diagnostic executable identity, not a release checksum.
The checkout also contained the documentation-only cache-plan commits; its
executable source and Cargo inputs matched the source baseline above.

Parser declarations, dispatch, owning implementations, existing process tests
and public Astro source were inspected. Safe runtime probes used an isolated
temporary working directory, removed ambient `AROS_*` overrides and never used
a real compiler cache, board, installed toolchain or source checkout as a write
target. Source-only findings are identified separately from reproduced cases.

| Frontend domain | Visible leaves inspected |
| --- | --- |
| Setup and tools | `setup`, `host-compiler install`, `build-tools build`, `build-tools check`, `install` |
| Source | `source init`, `source sync` |
| Product workflows | `build`, `clean`, `test`, `golden capture`, `golden verify`, `info`, `ccache` |
| Toolchain consumer | `install`, `list`, `inventory`, `import`, `register`, `select`, `remove`, `gc`, `verify`, `path` under `toolchain` |
| Native producer frontend | `toolchain plan`, `toolchain build` |
| Producer input and package stages | `recipe`, `cache`, `compatibility-ports`, `environment`, `profile`, `materialize-engine-free-source`, `package`, `verify-package`, `compare`, `repackage` under `toolchain producer` |
| Producer evidence and compatibility | `validate-recovery`, `record-qualification`, `prepare-recovery`, `index`, `compatibility-host-tools`, `compatibility` under `toolchain producer` |
| Boards | `init`, `scan`, `doctor`, `build`, `deploy`, `serve`, `console` under `board` |
| SD workflows | `image`, `scan`, `unmount`, `write` under `board sd` |

The seven other installed programs were inspected at their parser and process
boundaries: `aros-ahi-runner`, `aros-collect`, `aros-fetch`, `aros-genmodule`,
`aros-romtool`, `aros-transpiler` and `aros-verify`. Their algorithms were not
requalified. Collector driver aliases, legacy fetch argument normalization and
the hidden MetaMake fetch bridge are compatibility protocols, not extra
ordinary frontend leaves. Internal `aros-release` is outside this public CLI
initiative except for any affected packaging callers.

Documentation coverage includes the CLI, configuration, diagnostics,
standalone-tool and troubleshooting references; source, toolchain, native
producer, upstream, AROS-NX, board and cross-development workflows; and
getting-started and README entry points. Source inspection and help traversal
do not constitute successful full builds, hardware tests, release checks or
exhaustive combinations of every option.

## Confirmed findings

### CLI-F01 — P1: conflicting board initialization descriptions

The public CLI overview still describes a Pi-4 USB-ECM template, while the
detailed section correctly describes typed models. See
[`reference/cli.md:104`](../docs-site/src/content/docs/reference/cli.md) and
[`main.rs:457`](../crates/aros-cli/src/main.rs). Runtime previews succeeded for
`rpi3`, `rpi4`, `rpi5` and `milk-v-titan`. Their default transports were
`native-tftp`, `native-tftp`, `native-tftp` and `uefi-esp`, respectively.
The local profile name is independent of the model. Template availability
does not prove boot readiness on that hardware.

### CLI-F02 — P2: documentation gate proves names, not semantics

[`public_command_documentation.rs:71`](../crates/aros-cli/tests/public_command_documentation.rs)
asserts a fixed count of 53 and the presence of each command name. It does not
compare required options, defaults, accepted values, conflicts, environment
precedence or documented effects. F01 passes this gate. Other existing tests
do cover selected semantics; this is a coverage gap, not absence of all CLI
testing.

### CLI-F03 — P1: optional native-build flag is mandatory at execution

[`toolchain_build.rs:54`](../crates/aros-cli/src/toolchain_build.rs) makes
`--offline` optional, but
[`native_lifecycle.rs:54`](../crates/aros-toolchain/src/native_lifecycle.rs)
rejects its absence immediately with AX0202. No online native build exists.
`toolchain plan --offline` records the proposed execution policy and affects
readiness; it is **not currently a no-op**. Its removal requires changing the
readiness/request contract along with the CLI, not merely removing help text.
Planning itself never fetches. Cache acquisition and ordinary product builds
have different, real online/offline choices.

### CLI-F04 — P2: accepted acquisition modes conflict or are ignored

`setup --preset P --local DIR --force` and
`toolchain install --preset P --local DIR --force` parse successfully.
[`toolchain.rs:178`](../crates/aros-cli/src/toolchain.rs) then takes the local
return path without using `force`. `--force --offline` is accepted by `setup`,
`host-compiler install` and `toolchain install`, but
[`artifact.rs:143`](../crates/aros-cli/src/artifact.rs) skips the cache hit when
forced and reaches the offline failure even if valid cached bytes exist.
Safe probes outside a checkout reached AR0101 repository discovery, proving
that the parser did not reject these combinations. The later cache behavior
was established by source inspection. `aros-fetch --force` has different,
documented extraction-publication semantics; do not apply this conflict to it.

### CLI-F05 — P2: redundant compiler-statistics flag

[`main.rs:239`](../crates/aros-cli/src/main.rs) declares `ccache --stats` with a
true default; the flag adds no choice and `--stats=false` is not accepted.
[`commands.rs:506`](../crates/aros-cli/src/commands.rs) uses that value to query
statistics, including after clear. Final cache command design belongs to
[CACHE-M1](https://github.com/metaneutrons/aros-tools/issues/140), not a second
compiler-cache API in this initiative.

### CLI-F06 — P1: false sccache-clear success

[`build.rs`](../crates/aros-cli/src/build.rs) maps the sccache clear operation
to `-z`; that operation resets statistics. The frontend then prints
`Compiler cache cleared.`. Local backend help and the previous source-backed
cache analysis distinguish it from ccache `-C`. No real cache was cleared in
this audit. The repair and its backend evidence remain owned by CACHE-M1;
the shared diagnostic-stream defect below is a different responsibility.

### CLI-F07 — P3: source command and ref terminology

[`main.rs:137,280,296`](../crates/aros-cli/src/main.rs) describes `source` only
as checkout creation and uses `--ref` for different accepted input domains.
Initialization takes full branch/tag refs or an exact OID and detaches HEAD
when a ref is provided; synchronization takes a branch name below
`refs/heads/`. The sync selector should say `--branch`. Detached initialization
is deliberate and documented; do not change it incidentally during renaming.

### CLI-F08 — P2: producer package argument belongs to the wrong parser shape

`package` and `verify-package` share `PackageArgs`, including optional
`output_dir`. See
[`toolchain_producer.rs:216,691,730`](../crates/aros-cli/src/toolchain_producer.rs).
Runtime probes with all common arguments and nonexistent input paths produced
AR0401: package requires `--output-dir`, while verification refuses it.
These are statically knowable invocation errors currently reported as tool
resolution problems with a generic toolchain-install hint. Separate argument
structures can share the common package context without sharing this field.

### CLI-F09 — P2: user paths change origin after repository discovery

[`main.rs:1037`](../crates/aros-cli/src/main.rs) changes the process directory
to the discovered checkout after parsing. Explicit relative `PathBuf` values
are generally resolved later. From a nested working directory, local-prefix,
module, evidence, engine and board-config paths can therefore refer to a
different location than the caller supplied. Global board commands do not
perform this change, so the same relative `--config` differs across board
commands. This is source-confirmed; nested-directory fixture verification is
required for the implementation. Documented producer-root-relative recipe
members are a separate intentional contract.

### CLI-F10 — P2: explicit logging off is overridden

[`main.rs:76`](../crates/aros-cli/src/main.rs) promotes every off level to info
if a file is supplied. An isolated `aros --log-level off --log-file FILE
--log-format jsonl info` exited 0 and created a nonempty log. The public
diagnostics page accurately describes this surprising current behavior.
Companions differ: collector retains explicit-level provenance, while other
companions leave a file-only request off. The proposed shared policy must
distinguish omitted level from an explicitly selected off value.

### CLI-F11 — P1: successful child stderr breaks later JSON diagnostics

[`observability.rs:253,483`](../crates/aros-cli/src/observability.rs) captures
children in JSON diagnostic mode but replays successful child stderr raw.
A later failure appends the versioned diagnostic document to those bytes.
The whole stream cannot be parsed as the promised JSON document.

Reproduced without a real backend: an isolated executable named `sccache`
printed a warning and returned 0 for `-z`, then returned 17 for the statistics
query. `aros --diagnostic-format json ccache --clear` exited 1, and stderr was
`successful-child-warning\n` followed by the JSON envelope. Parsing all stderr
as JSON failed. This exercises the shared subprocess wrapper, also used by
configure/build chains; fixing only ccache would leave the general defect.
Interactive console passthrough is an explicitly different stream contract.

### CLI-F12 — P2: zero resource values reach execution

[`main.rs:218,234`](../crates/aros-cli/src/main.rs) accepts zero QEMU timeout
and memory; [`commands.rs:474`](../crates/aros-cli/src/commands.rs) forwards
them to the boot runner. Zero is neither a useful run duration nor a valid
memory budget. Reject these at parsing and retain owner-side validation for
non-CLI callers. Existing positive-job and producer timeout parsers are useful
precedents; retain their domain-specific bounds rather than one arbitrary cap.

### CLI-F13 — P2: mutation-mode conflicts are checked too late

Board deploy and SD-image parsers accept `--apply --dry-run` together, then
[`commands.rs:305,344,393`](../crates/aros-cli/src/commands.rs) rejects them.
Deploy has already passed root discovery before dispatch; both have passed
logging initialization. Neither reaches board loading inside the handler.
Move statically known conflicts into the parser. Explicit `--dry-run` remains
a useful assertion of no mutation even where preview is the default; it is
not classified as a useless option.

### CLI-F14 — P2: stale producer capability descriptions

[`reference/diagnostics.md:75`](../docs-site/src/content/docs/reference/diagnostics.md)
says AX0801 is not connected to a public build command, although
`toolchain build` executes the ownership lifecycle. The CLI reference also
describes receipt reuse/resume and candidate inventory as future work while
[`toolchain_build.rs:60`](../crates/aros-cli/src/toolchain_build.rs) exposes
the bounded `--resume-from compiler` path and reports measured outputs.
Update each precise supported boundary; do not turn compiler-only resume into
a claim that arbitrary partial builds are resumable.

### CLI-F15 — P1: suite-install reference overstates version verification

[`reference/cli.md:123`](../docs-site/src/content/docs/reference/cli.md) calls
`install` a publication of eight version-matched executables.
[`aros-release/src/install.rs:89`](../crates/aros-release/src/install.rs)
checks the exact eight-file inventory, file type, executable mode, size and
snapshotted bytes, but does not read version or release-manifest identity.
Its existing fixture even installs executable marker files successfully.
The invocation is a publication primitive for already verified extracted
inputs; describe the actual boundary and retain external release verification
as its prerequisite. Do not silently execute arbitrary source programs to
make this prose claim true, or imply an attestation check that is not present.

## Additional design work, not newly proven runtime defects

- **CLI-D01: post-commit reporting context.** `main.rs:755` recognizes only
  suite install in `commits_on_success`. Other handlers carry publication state
  for their own errors, but successful dispatch loses the result before the
  final logger event. The deferred stdout-error branch also lacks a general
  committed outcome. Establish failure-injection evidence for each affected
  mutation and propagate actual outcomes through final reporting. Never infer
  commit state only from the command name or presence of `--apply`.
- **CLI-D02: discovery for scripts and shells.** `info` and `toolchain list`
  expose human results only, while inventory/producer operations have explicit
  structured results. No completion command/generator is declared in the
  frontend. Add bounded structured discovery and static shell completion from
  the same command model. These are proposed capabilities, not broken promises
  of the current release.
- **CLI-D03: clean scope.** Bare `aros clean` intentionally deletes the entire
  checkout `build/` tree. Public docs say so, but short help does not state the
  default scope. Prefer an explicit `--preset NAME` or `--all` selection, with
  a documented preview, in the contract change. This is a deliberate UX change
  requiring migration notes, not a claim that current documented behavior is
  secretly different.

## Existing guarantees to preserve

Help/version have a read-only early exit. Invocation failures already use
AR0001 in the shared diagnostic envelope. Missing checkout has its own AR0101
boundary. Broken stdout pipes have regression coverage. Source synchronization
uses clean-state validation, candidate checks, fast-forward-only publication
and explicit rollback/indeterminate states. Managed toolchain operations use
preview tokens and distinguish installed stores from external prefixes.
Ordinary builds retain explicit source-integrity and offline input policies.
Producer commands have no release publication authority.

No new hardware support, cache deletion algorithm, release acceptance, complete
transpiler correctness or universal OS network sandbox is established by this
CLI audit. Existing public limitations must remain precise throughout the work.
