# AROS tools

[![Workspace CI](https://github.com/metaneutrons/aros-tools/actions/workflows/ci.yml/badge.svg)](https://github.com/metaneutrons/aros-tools/actions/workflows/ci.yml)
[![CodeQL](https://github.com/metaneutrons/aros-tools/actions/workflows/codeql.yml/badge.svg)](https://github.com/metaneutrons/aros-tools/actions/workflows/codeql.yml)
[![Documentation](https://github.com/metaneutrons/aros-tools/actions/workflows/docs.yml/badge.svg)](https://aros.metaneutrons.cc/aros-tools/)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](#license)

**A rigorous, upstream-compatible tool suite for building, verifying, and
deploying AROS.**

`aros-tools` turns the historically distributed AROS build tooling into a
reviewable Rust workspace with explicit contracts for source inputs, build
execution, diagnostics, releases, and physical-board workflows. It is designed
to work beside an AROS checkout rather than modify or own it.

Documentation: [start here](https://aros.metaneutrons.cc/aros-tools/) ·
[installation](https://aros.metaneutrons.cc/aros-tools/getting-started/installation/) ·
[first build](https://aros.metaneutrons.cc/aros-tools/getting-started/quick-start/) ·
[command reference](https://aros.metaneutrons.cc/aros-tools/reference/cli/) ·
[release status](https://aros.metaneutrons.cc/aros-tools/reference/release-status/)

> **Project status:** the workspace and its release engineering are under
> active qualification. Build from source is the supported installation path.
> Package-manager commands and download URLs are published only after their
> artifacts have passed the documented clean-room and supply-chain gates.

## Why this exists

AROS has a powerful build system, but its host-side development experience has
traditionally been spread across scripts, MetaMake recipes, external tools and
machine-local conventions. `aros-tools` makes those boundaries explicit:

- one `aros` command coordinates an existing AROS checkout without embedding a
  second copy of AROS;
- independent tools parse, translate, collect, verify, fetch and package with
  stable machine-readable diagnostics;
- releases select exact, measured toolchain artifacts rather than guessing a
  compiler or silently falling back to a host installation;
- risky board and removable-media operations have narrow, validated ownership
  boundaries; and
- deterministic artifacts, manifests, SBOMs, checksums and provenance are
  release requirements, not afterthoughts.

The goal is practical: contributors can develop against a pristine upstream
checkout, while AROS-NX can add reviewed extensions without forking the tool
experience.

## Quick start: build from source

### Prerequisites

- Rust 1.98.0 and Cargo (selected by `rust-toolchain.toml`)
- Git, CMake and Ninja
- Python 3.11 or newer, curl, a POSIX `patch`, and platform CA certificates used by public
  source-fetch, verification, and build workflows
- host packages required by the AROS target you intend to build

```console
git clone https://github.com/metaneutrons/aros-tools.git
cd aros-tools
cargo build --release --workspace --all-features --locked
./target/release/aros --help
```

The resulting executables live in `target/release`. Keep the eight public
executables together: the `aros` frontend deliberately invokes the specialised
tools as separate processes, so their command contracts, logs and failure
boundaries remain independent. A workspace build also creates the internal
`aros-release` qualification producer; it is not part of an installation.

For an AROS checkout, begin with the workflow guide that matches your source:

| Source tree | Guide | Current boundary |
| --- | --- | --- |
| Pristine upstream AROS | [Upstream AROS workflow](https://aros.metaneutrons.cc/aros-tools/workflows/upstream-aros/) | source initialization, graph validation and independent component tools use built-in target defaults; a complete product still needs the reviewed source compatibility layer |
| AROS-NX | [AROS-NX workflow](https://aros.metaneutrons.cc/aros-tools/workflows/aros-nx/) | reviewed target and release contracts are available through the selected checkout |

Do not use an unpublished release URL as a shortcut. The
[installation guide](https://aros.metaneutrons.cc/aros-tools/getting-started/installation/)
is the source of truth for supported installation paths.

Create a new explicit checkout with the just-built local frontend, without
relying on `PATH` contents or sibling-directory naming:

```console
./target/release/aros source init ~/Source/AROS
cd ~/Source/AROS
```

Use `--fork`, `--upstream` and an unambiguous `--ref` on that same local binary
to select a fork, a different canonical source or an immutable revision. A ref
must be a full `refs/heads/NAME`, full `refs/tags/NAME`, or exact 40/64-digit
commit OID; for example, `--ref refs/heads/master` is directly copyable while
the short name `--ref master` is deliberately rejected. Initialization is
staged and never reuses an existing destination.

## What is included

| Area | Commands / crates | Responsibility |
| --- | --- | --- |
| Frontend | `aros`, `aros-cli` | repository discovery, orchestration, host compilers, released toolchains and board presentation |
| Translation and verification | `aros-transpiler`, `aros-verify` | transactional MetaMake-to-CMake publication and an independent historic-semantics oracle |
| Linking and SDK generation | `aros-collect`, `aros-genmodule`, `aros-romtool` | two-pass AROS linking, ABI checks, module sources and ROM layout validation |
| Inputs and external contracts | `aros-fetch`, `aros-ahi-runner` | verified source transport, safe extraction, patches and validated AHI builds |
| Hardware workflows | `aros-board`, `aros-macos-disk-claim` | board identity, deploy/network boot/removable-media safety, and the narrow macOS disk-claim lifetime |
| Shared foundations | `aros-common` | diagnostics, opt-in logging, hashes, ELF and toolchain contracts |

`aros-cli` is intentionally an orchestrator, not a monolith: it executes the
other tools rather than linking their implementations. `aros-verify` is
intentionally independent from the transpiler, so a shared implementation bug
cannot validate itself.

## Trust, reproducibility, and failure behavior

The project is deliberately strict about what it may infer:

- **Explicit inputs:** a checkout's `aros-targets.toml`, when present, is the
  complete authoritative override. A pristine checkout uses the documented
  target and host-compiler contract embedded in the installed tools; a broken
  override never falls back silently. Released cross-toolchains are selected
  through that checkout's `aros-toolchains.lock.toml` and release manifests.
- **No hidden dependency pins:** ordinary packages and sources are not silently
  pinned. The transpiler only records documented capability fingerprints for
  genuinely opaque recipe inputs, and fails with a precise diagnostic when it
  needs an update.
- **Independent verification:** MetaMake translation is checked against a
  separate historic-semantics reference implementation.
- **Safe publication:** release archives are normalized, read back and
  verified; promotion requires measured checksums, SPDX SBOMs, provenance and
  isolated-download verification.
- **Useful errors:** every user-facing command provides human diagnostics and
  a stable JSON alternative on the shared `aros-tool-diagnostics-v1` contract.
  Diagnostics go to `stderr`; intentional command output goes to `stdout`.
  Local logging is off by default and writes only to an explicit `--log-file`.
  A non-off `--log-level` without that destination fails. Portable invocations
  should pass both options: the `aros` frontend promotes file-only logging to
  `info`, while specialised tools leave their explicit `off` level unchanged.
  Logs never replace the terminal diagnostic or determine command success.

See [diagnostics](https://aros.metaneutrons.cc/aros-tools/reference/diagnostics/),
[architecture](https://aros.metaneutrons.cc/aros-tools/reference/architecture/),
and [release engineering](https://aros.metaneutrons.cc/aros-tools/reference/releases/)
for the exact contracts.

## Development

Install the three pinned Cargo audit tools once, then use the canonical gate
script with Node.js 24 or newer, npm, actionlint, ShellCheck, and an explicit, qualified
AROS-NX checkout:

```console
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-machete --version 0.9.2 --locked
# Also install actionlint and ShellCheck from your platform package manager.
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh
```

The source path is intentionally never inferred from a neighbouring directory.
CI pins the exact AROS-NX revision used by the toolchain producer, which keeps
a moving upstream branch from changing the meaning of a tools commit.

`scripts/check-workspace.sh` is the single source of truth used locally and by
CI. The default `all` mode also performs actionlint, ShellCheck and the locked
Astro documentation build. Its `quality`, `docs`, `test`, and `portable-test`
modes split runner responsibilities without changing that complete local
contract. The closed portable crate suite runs on all four supported hosts and
compiles every transpiler/verifier test target there. One Linux lane alone
executes the source-coupled transpiler/verifier tests against the large
recursive checkout and exact AROS-NX source qualification; they are never
silently run or skipped without that oracle.
That exact-source lane also discovers every checked-in CMake-engine fixture and
executes every host-compatible one; the real GRUB host-build fixture is an
explicit Darwin/arm64 release qualification and is visibly omitted elsewhere.
Adding a fixture therefore broadens the gate rather than creating a manual test
convention.
The separate documentation workflow calls `docs` directly, stages the verified
output below `/aros-tools/`, and hands it to a protected, path-scoped
Cloudflare Static Assets deployment. Pull requests receive no deployment
credential.

The architecture gate protects the long-term shape of the workspace: it
enforces one-way crate dependencies, keeps production modules bounded, requires
module documentation in `aros-cli`, and prevents subprocesses from bypassing
the shared execution and observability layers.

## Release model

GitHub Releases are the canonical source for native archives. Package channels
are consumers of those immutable, measured artifacts – never independent binary
builds.

Release Please derives the workspace SemVer and changelog from Conventional
Commits. It opens a reviewable release pull request but never creates a tag or
GitHub Release. Only a separately protected annotated tag on qualified `main`
starts release production and promotion.

Before the first stable tag, every supported native archive and downstream
package path must pass its own gate:

1. four native archive builds, deterministic production and clean-room smoke
   tests;
2. archive manifests, checksums, SPDX SBOMs, Sigstore evidence and GitHub
   provenance;
3. an immutable private Actions staging set, followed by isolated draft
   verification and one-time GitHub publication with final status;
4. byte-for-byte Debian package installation checks, signed APT verification,
   Homebrew installation on all supported hosts, and AUR qualification on both
   Linux architectures; and
5. explicit roll-forward and public verification of APT, Homebrew and AUR from
   the canonical immutable GitHub release.

The definitive details, including failure and credential boundaries, live in
[release engineering](https://aros.metaneutrons.cc/aros-tools/reference/releases/)
and [package publication](https://aros.metaneutrons.cc/aros-tools/reference/publication/).

## Documentation

The documentation is versioned with the source and built with Astro Starlight:

```console
cd docs-site
npm ci --ignore-scripts
npm audit --audit-level=high
npm run build
```

The checked-in lockfile is authoritative. Lifecycle scripts are disabled and
the locked graph must pass the high-severity audit before Astro runs. The same
gate prepares the path-nested static asset tree consumed by the Cloudflare
Worker at `https://aros.metaneutrons.cc/aros-tools/`, so the published
documentation and pull-request checks exercise the same dependency graph. Pull
requests perform a credential-free Wrangler dry run; only protected `main` may
enter the isolated `docs-publication` environment and deploy the verified
handoff.

## License

Unless a file states otherwise, the Rust workspace and documentation are
available under the [GNU General Public License, version 3 or later](LICENSE).

That covers this tooling, not what it produces. AROS sources, AROS-NX sources
and every build product keep the licenses of their own upstreams, and nothing
here changes them.

## Contributing and security

Development and review rules are in [CONTRIBUTING.md](CONTRIBUTING.md), the
two-stage release procedure in [RELEASING.md](RELEASING.md), and private
vulnerability reporting in [SECURITY.md](SECURITY.md). Participation is subject
to the [community code of conduct](CODE_OF_CONDUCT.md).
