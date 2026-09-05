# Contributing to aros-tools

`aros-tools` accepts focused changes that preserve compatibility with pristine
upstream AROS and keep AROS-NX-specific extensions explicit. A pull request is
ready for review only when its behavior, failure contract, tests and user
documentation agree.

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

Install the pinned audit helpers once and run the canonical complete gate. The
script resolves the repository root itself, so it is safe to invoke from a
subdirectory:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-machete --version 0.9.2 --locked
# Also install the platform tools listed above from your package manager.
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh
```

`scripts/check-workspace.sh` is the workspace-gate SSOT. Its default `all`
mode includes formatting, architecture, Actions/APT/governance/release-policy
fixtures, actionlint,
ShellCheck, locked strict Clippy, locked rustdoc, audit, deny, machete, the
locked Astro build, and locked workspace tests. CI runs the closed
source-independent `portable-test` suite on all
four supported hosts and the recursive exact-source `test` gate on one Linux
lane. This reduces runner work without weakening the default local `all`
contract. The exact-source gate also builds the workspace executables and runs
every host-compatible `aros-cmake-engine` CMake fixture against that same
qualified checkout; `clang`, `cmake` and `ninja` are therefore required for this gate.
The real GRUB host-build fixture is an explicit Darwin/arm64 release
qualification and is reported as an omission on other hosts. The separate
documentation workflow runs the same locked npm/Astro contract through `docs`,
nests the verified output below the public `/aros-tools/` prefix, and proves
the pinned Wrangler deployment locally. A protected credential-bearing job
publishes only that build-job handoff to the path-scoped Cloudflare Worker;
pull requests receive no deployment credential.

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
