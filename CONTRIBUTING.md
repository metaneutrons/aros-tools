# Contributing to aros-tools

`aros-tools` accepts focused changes that preserve compatibility with pristine
upstream AROS and keep AROS-NX-specific extensions explicit. A pull request is
ready for review only when its behavior, failure contract, tests and user
documentation agree.

For ongoing implementation work, use the project issues as the live execution
record. The producer plan owns acceptance criteria and links to verified
milestone evidence.

## Development environment

The supported Rust toolchain is pinned in `rust-toolchain.toml`. The canonical
development-runtime contract additionally requires Python 3.11 or newer (with
`tomllib`) and Node.js 24 or newer with npm. Install Git, CMake, Ninja,
actionlint, ShellCheck, `jq`, GnuPG (`gpg` and `gpgv`), `dpkg-deb`, `gzip`,
`tar`, `ar`, curl and a SHA-256 implementation as well. The quality gate checks
these prerequisites before starting an expensive build; versions live in
`contracts/development-runtimes-v1.toml`, not in this prose.
Native GNU lifecycle fixtures also require GNU Make 4.0 or newer, bison, flex,
patch and pkg-config. On macOS, Homebrew's `make` formula provides `gmake`;
Apple's Make 3.81 is rejected. CI installs these test prerequisites explicitly
rather than relying on a particular runner image.
Source-value parity fixtures require GNU sed as an independent test oracle;
install Homebrew's `gnu-sed` on macOS (`gsed`). Native extraction itself does
not execute sed or depend on it.
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
| Source/transpiler behavior changes and PR source lane | `AROS_TEST_SOURCE_ROOT=... AROS_TEST_MESA20_SOURCE_ROOT=... AROS_TEST_MESA26_SOURCE_ROOT=... scripts/check-workspace.sh source-test` | Full locked Rust suite; the current source and historical Mesa 20 regression trees are validated separately. No CMake fixtures. |
| Documentation changes | `scripts/check-workspace.sh docs` | Locked npm/Astro build, generated links and deployment dry-run. |
| Integrated `main` or explicit integration run | `AROS_TEST_SOURCE_ROOT=... AROS_TEST_MESA20_SOURCE_ROOT=... AROS_TEST_MESA26_SOURCE_ROOT=... scripts/check-workspace.sh test` | Full Rust suite + every host-compatible CMake fixture. |
| Feature/milestone acceptance or release candidate | `AROS_TEST_SOURCE_ROOT=... AROS_TEST_MESA20_SOURCE_ROOT=... AROS_TEST_MESA26_SOURCE_ROOT=... scripts/check-workspace.sh all` | Quality + docs + integration; requires the host coverage below. |

Every pull request retains commit hygiene, the stable Linux x86-64 check,
quality gate and documentation build. A deliberately narrow documentation-only
change set
(`README.md`, `CONTRIBUTING.md`, `docs/**` or `docs-site/**`) uses that Linux
lane's source-independent `portable-test`. Every other change — including an
unknown path, a workflow, contract, dependency, script or Rust change — uses
the active three native hosts. Linux x86-64 runs the source-coupled Rust suite;
Linux ARM64 and macOS ARM64 run `portable-test`. The planner is fail-closed: an
empty, malformed or unclassified change list selects all three active hosts.

On a push to `main`, the Linux source lane also runs all compatible CMake
fixtures. **Workspace CI → Run workflow** defaults to the active Linux
x86-64/Linux ARM64/macOS ARM64 matrix and makes the cheaper Linux-only scope an
explicit operator choice. A weekly three-host sweep detects runner and
toolchain drift. macOS Intel is not an `aros-tools` native release target.
Thus repeated documentation edits do not consume macOS capacity, while
executable changes retain native coverage. The separate tools
release/package qualification runs only for immutable tags or an explicit
Release qualification dispatch; ordinary PRs validate the
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
`ninja` are required; the native-core fixture also needs `ld.lld`, and the
module source-group fixture needs a host compiler that builds Objective-C
(`gobjc` on Debian and Ubuntu). The local invocation is:

```sh
AROS_TEST_SOURCE_ROOT=/absolute/path/to/current/AROS-NX \
AROS_TEST_MESA20_SOURCE_ROOT=/absolute/path/to/AROS-NX-at-cb6974f \
AROS_TEST_MESA26_SOURCE_ROOT=/absolute/path/to/current/AROS-NX \
  scripts/check-workspace.sh all
```

This policy schedules existing tests; it does not grant release authority,
replace toolchain A/B/compatibility checks, change pins or waive a failing
required check. The weekly host sweep is coverage evidence, not release
evidence.

`AROS_TEST_SOURCE_ROOT` enables the otherwise skipped real
`aros source init` → `aros source sync` → `aros-transpiler` integration case.
The configured source is read-only input: the test creates and removes only its
own temporary checkout. The real sync regression proves rejection without branch
mutation for the full Mesa 26/RISC-V manifest, then a successful fast-forward
with an explicitly reduced three-profile test fixture. It does not qualify
whole-source synchronization of the current four-profile NX manifest.
Workspace tests normally find all six required real
build-tool executables in the Cargo target directory; set
`AROS_TEST_TOOLS_DIR` explicitly when testing prebuilt binaries from another
directory.
The current source must match `[integration]` in `contracts/aros-source-v1.toml`
(or `[source]` in an older contract without that table). The `[source]` and
`[producer]` pins remain the paired identities of the qualified toolchain
producer; advancing development integration does not retarget that release.
`AROS_TEST_MESA26_SOURCE_ROOT` selects that same current tree for Mesa 26
capability probes. The separate `AROS_TEST_MESA20_SOURCE_ROOT` must be the
clean, recursively initialized AROS-NX checkout at
`cb6974f1c3de43c6f1168d69039af7c32e56153c`. The source-coupled Rust
suite uses the current source by default. Only the Mesa 20 patch/inventory and
Mesa 20 Nouveau Gallium regression tests explicitly use the historical oracle;
whole-tree inventories, CLI source workflows and Mesa 26 probes use the current
source. CI checks out and validates both trees. The current source's complete
product graph is qualified separately by the AROS-NX product matrix.

Build the documentation with the checked-in JavaScript lockfile:

```sh
cd docs-site
npm ci --ignore-scripts
python3 ../scripts/check-docs-audit.py
npm run build
```

The documentation audit still rejects high and critical vulnerabilities. The
only temporary exception is the public advisory
[GHSA-ch52-4w7c-c8xp](https://github.com/advisories/GHSA-ch52-4w7c-c8xp),
approved by the maintainer for the pinned, static documentation build through
10 October 2026 (22:00 UTC). Its exact dependency chain, lockfile identities,
configuration hashes and expiry are recorded in
`contracts/docs-audit-exception-v1.json`. The gate runs a fresh npm audit and
rejects additional advisories, changed dependencies or configuration, malformed
reports and use after expiry. This is not an exemption for SSR, an application
runtime or another repository. Replace the affected dependency when a verified
fix is available, then remove the exception; do not extend it implicitly.

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

- [Public CLI contract plan](docs/public-cli-contract-plan.md): command semantics,
  diagnostics, discoverability and continuous source-aligned Astro documentation;
  its [audit baseline](docs/public-cli-audit.md) distinguishes measured findings
  from proposed capabilities.
- [Cache management plan](docs/cache-management-plan.md): proposed resource-based
  CLI, typed cache ownership, missing capabilities and CACHE milestone gates.
- [Toolchain producer integration plan](docs/toolchain-producer-plan.md):
  native producer design, staged migration and TCP-M0 through TCP-M7 gates.
  Its [M0 contract](docs/toolchain-producer-contract.md) and
  [baseline evidence](docs/toolchain-producer-baseline.md) distinguish specified
  interfaces from implemented and qualified behavior.
- [CMake engine migration](docs/cmake-engine-migration.md): implemented
  ownership changes, measured evidence and the remaining source boundary.
- [Boot media initiative plan](docs/boot-media-plan.md): portable media
  profiles, native build closure, image composition and platform-specific
  qualification gates.
- [Declarative boards and RISC-V integration](docs/riscv-board-integration-plan.md):
  board-registry migration, RV32/P4 and RV64 compiler contracts, native P4
  artifacts and the dependency on BM5's Titan hardware acceptance.

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
