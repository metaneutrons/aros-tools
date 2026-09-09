# Contributing to aros-tools

`aros-tools` accepts focused changes that preserve compatibility with pristine
upstream AROS and keep AROS-NX-specific extensions explicit. A pull request is
ready for review only when its behavior, failure contract, tests and user
documentation agree.

For ongoing implementation work, start with [HANDOFF.md](HANDOFF.md). It links
the last verified checkpoint and next bounded slice; issues remain the live
execution record, and the producer plan owns acceptance criteria.

## Development environment

The supported Rust toolchain is pinned in `rust-toolchain.toml`. The canonical
development-runtime contract additionally requires Python 3.11 or newer (with
`tomllib`) and Node.js 24 or newer with npm. Install Git, CMake, Ninja,
actionlint, ShellCheck, `jq`, GnuPG (`gpg` and `gpgv`), `dpkg-deb`, `gzip`,
`tar`, `ar`, curl and a SHA-256 implementation as well. The quality gate checks
these prerequisites before starting an expensive build; versions live in
`contracts/development-runtimes-v1.toml`, not in this prose.
Source-contract tests need the immutable AROS-NX revision named in
`contracts/aros-source-v1.toml`; do not substitute a moving branch or infer a
neighboring checkout.

Install the pinned audit helpers once and run the canonical iteration gate. The
script resolves the repository root itself, so it is safe to invoke from a
subdirectory:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-machete --version 0.9.2 --locked
# Also install the platform tools listed above from your package manager.
scripts/check-workspace.sh
```

`scripts/check-workspace.sh` is the workspace-gate SSOT. Its default `check`
mode runs quality and portable tests: formatting, architecture, Actions/APT/
governance/release-policy fixtures, actionlint, ShellCheck, locked strict Clippy,
rustdoc, audit, deny and machete, followed by the closed source-independent
Rust suite. It does **not** run the source-coupled Rust suite, Astro build or
real CMake/GRUB product fixtures. `check` and `portable-test` reject a configured
`AROS_TEST_SOURCE_ROOT` instead of silently treating partial coverage as full.
The previous full default remains available only through explicit `all`.

### Test stages and integration checkpoints

| When | Gate | Evidence and limits |
| --- | --- | --- |
| Editing a focused crate | `cargo test -p <crate> --locked` | Fast regression feedback; not the workspace gate. |
| Ordinary local iteration / before pushing | `scripts/check-workspace.sh` | Quality + portable Rust; no source checkout or product build. |
| Source/transpiler behavior changes and PR source lane | `AROS_TEST_SOURCE_ROOT=... scripts/check-workspace.sh source-test` | Full locked Rust suite against the exact clean recursive source; no CMake fixtures. |
| Documentation changes | `scripts/check-workspace.sh docs` | Locked npm/Astro build, generated links and deployment dry-run. |
| Integrated `main` or explicit integration run | `AROS_TEST_SOURCE_ROOT=... scripts/check-workspace.sh test` | Full Rust suite + every host-compatible CMake fixture. |
| Feature/milestone acceptance or release candidate | `AROS_TEST_SOURCE_ROOT=... scripts/check-workspace.sh all` | Quality + docs + integration; requires the host coverage below. |

Every pull request retains commit hygiene, the stable Linux x86-64 check,
quality gate and documentation build. A deliberately narrow documentation-only
change set
(`README.md`, `CONTRIBUTING.md`, `docs/**` or `docs-site/**`) uses that Linux
lane's source-independent `portable-test`. Every other change — including an
unknown path, a workflow, contract, dependency, script or Rust change — uses
all four native hosts. Linux x86-64 runs the source-coupled Rust suite; the
other three hosts run `portable-test`. The planner is fail-closed: an empty,
malformed or unclassified change list selects all four hosts.

On a push to `main`, the Linux source lane also runs all compatible CMake
fixtures. **Workspace CI → Run workflow** defaults to all four hosts and makes
the cheaper Linux-only scope an explicit operator choice. A weekly four-host
sweep detects runner and toolchain drift. Thus repeated documentation edits do
not consume macOS capacity, while executable changes retain native coverage.
The separate four-host release/package qualification runs only for immutable
tags or an explicit Release qualification dispatch; ordinary PRs validate the
release policy in the Linux quality gate without rebuilding distributable
archives.

Before merging changes to the CMake engine, source translation, AROS source
contract, fetch/build runner boundaries or this gate itself, run `test` once
on the final implementation candidate; use the explicit CI dispatch for its
Linux coverage and a local Darwin/arm64 run where GRUB is affected. This is an
integration checkpoint, not a command to repeat after each edit. Complete
cross-cutting feature/milestone acceptance and release-candidate qualification
require both Linux x86-64 and Darwin/arm64 evidence on the same candidate tree
and qualified source. Record exact identities, commands, hosts, result and
omissions in the PR or milestone issue. Any later executable/test/contract
change invalidates that candidate evidence; a docs-only change does not require
rebuilding GRUB. A failed checkpoint blocks acceptance/promotion; fix and rerun
it rather than accepting a green partial stage.

The real GRUB fixture builds PC, EFI64 and EFI32 host tools **only on
Darwin/arm64**; Linux explicitly reports that host-qualified omission. A green
Linux CI run alone is therefore not the complete macOS/GRUB evidence. The
explicit `test`/`all` gate discovers every `*Test.cmake`, so newly added fixtures
cannot silently fall out of the integration inventory. `clang`, `cmake` and
`ninja` are required. The local invocation is:

```sh
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh all
```

This policy schedules existing tests; it does not grant release authority,
replace toolchain A/B/compatibility checks, change pins or waive a failing
required check. The weekly host sweep is coverage evidence, not release
evidence.

`AROS_TEST_SOURCE_ROOT` enables the otherwise skipped real
`aros source init` → `aros source sync` → `aros-transpiler` integration case.
The configured source is read-only input: the test creates and removes only its
own temporary checkout. Workspace tests normally find all six required real
build-tool executables in the Cargo target directory; set
`AROS_TEST_TOOLS_DIR` explicitly when testing prebuilt binaries from another
directory.

Build the documentation with the checked-in JavaScript lockfile:

```sh
cd docs-site
npm ci --ignore-scripts
npm audit --audit-level=high
npm run build
```

## Design rules

- Keep `aros-cli` an orchestrator. Specialized behavior belongs in its existing
  crate and is invoked through the shared process and observability boundaries.
- Treat the selected AROS tree, target profiles and toolchain locks as explicit
  inputs. Do not guess a sibling checkout, silently select a host compiler or
  introduce an undocumented dependency pin.
- Validate before mutating. Publish files through same-filesystem staging and
  atomic replacement, and preserve an existing valid destination on failure.
- Never report success after dropping an error. Every user-facing failure must
  return a non-zero status and one stable `aros-tool-diagnostics-v1` document.
- Add a process-boundary regression test for CLI parsing, exit status, JSON
  diagnostics and destructive or publication behavior.
- Keep pristine-upstream behavior independent from optional AROS-NX bridges.
- Do not add automated-assistant authorship, `Co-Authored-By` trailers or
  generated-by marketing to commits, source files or generated artifacts.

The automated architecture gate enforces dependency direction, module size,
module documentation and subprocess boundaries. A passing gate is necessary,
not a substitute for explaining a new contract in the pull request.

## Architecture and delivery plans

Cross-repository changes need a versioned plan with ownership, compatibility
boundaries, milestones and evidence-based acceptance criteria. Keep design
decisions in the repository and execution status in linked issues/PRs; planned
capabilities must not be documented as already shipped.

- [Toolchain producer integration plan](docs/toolchain-producer-plan.md):
  native producer design, staged migration and TCP-M0 through TCP-M7 gates.
  Its [M0 contract](docs/toolchain-producer-contract.md) and
  [baseline evidence](docs/toolchain-producer-baseline.md) distinguish specified
  interfaces from implemented and qualified behavior.
- [CMake engine migration](docs/cmake-engine-migration.md): implemented
  ownership changes, measured evidence and the remaining source boundary.

## Commits and pull requests

Use a Conventional Commit pull-request title because squash merge makes that
title the commit from which Release Please derives SemVer and the changelog.
CI reruns this check whenever the title changes. Typical prefixes are `feat:`,
`fix:`, `docs:`, `test:`,
`refactor:`, `perf:`, `build:` and `ci:`. Mark an incompatible public contract
with a `BREAKING CHANGE:` footer.

Keep commits functional: production code, regression tests and the directly
affected documentation belong together. A pull request should state:

1. the user-visible problem and affected contract;
2. why the selected boundary owns the fix;
3. the exact tests and platforms exercised; and
4. any compatibility or migration consequence.

Do not mix generated files, unrelated formatting or dependency churn into a
behavioral change. Security-sensitive findings must follow `SECURITY.md` rather
than a public issue.

## Updating the AROS source contract

Change `contracts/aros-source-v1.toml` only after the referenced AROS-NX commit
and the pinned `aros-toolchains` producer commit select the same immutable
source revision. `scripts/validate-source-contract.py` and CI verify that
relationship. Include the producer qualification evidence in the pull request.

## Release changes

Do not create, move or delete a release tag from a feature branch. Release
Please owns version and changelog pull requests; the separately protected,
annotated tag only starts qualification after that pull request is merged.
Follow `RELEASING.md` for the complete promotion and recovery contract.
