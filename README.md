# AROS tools

**A rigorous, upstream-compatible tool suite for building, verifying, and
deploying AROS.**

`aros-tools` turns the historically distributed AROS build tooling into a
reviewable Rust workspace with explicit contracts for source inputs, build
execution, diagnostics, releases, and physical-board workflows. It is designed
to work beside an AROS checkout rather than modify or own it.

Documentation: [start here](https://metaneutrons.github.io/aros-tools/) ·
[installation](https://metaneutrons.github.io/aros-tools/getting-started/installation/) ·
[release status](https://metaneutrons.github.io/aros-tools/reference/release-status/) ·
[architecture](https://metaneutrons.github.io/aros-tools/reference/architecture/)

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

- Rust stable and Cargo
- Git
- CMake and Ninja for the translated CMake build path
- host packages required by the AROS target you intend to build

```console
git clone https://github.com/metaneutrons/aros-tools.git
cd aros-tools
cargo build --release --workspace --all-features
./target/release/aros --help
```

The resulting executables live in `target/release`. Keep them together: the
`aros` frontend deliberately invokes the specialised tools as separate
processes, so their command contracts, logs and failure boundaries remain
independent.

For an AROS checkout, begin with the workflow guide that matches your source:

| Source tree | Guide | Current boundary |
| --- | --- | --- |
| Pristine upstream AROS | [Upstream AROS workflow](https://metaneutrons.github.io/aros-tools/workflows/upstream-aros/) | checkout discovery and installed-tool resolution work; complete native GNU Make support is still being qualified |
| AROS-NX | [AROS-NX workflow](https://metaneutrons.github.io/aros-tools/workflows/aros-nx/) | reviewed target and release contracts are available through the selected checkout |

Do not use an unpublished release URL as a shortcut. The
[installation guide](https://metaneutrons.github.io/aros-tools/getting-started/installation/)
is the source of truth for supported installation paths.

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

- **Explicit inputs:** target profiles and host-compiler declarations belong to
  the selected AROS checkout. Released cross-toolchains are selected through
  that checkout's `aros-toolchains.lock.toml` and release manifests.
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
  Local logging is opt-in and requires both a level and destination.

See [diagnostics](https://metaneutrons.github.io/aros-tools/reference/diagnostics/),
[architecture](https://metaneutrons.github.io/aros-tools/reference/architecture/),
and [release engineering](https://metaneutrons.github.io/aros-tools/reference/releases/)
for the exact contracts.

## Development

Use an explicit, qualified AROS-NX checkout for the complete workspace gate:

```console
cargo fmt --all -- --check
sh scripts/check-architecture.sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  cargo test --workspace --all-features
```

The source path is intentionally never inferred from a neighbouring directory.
CI pins the exact AROS-NX revision used by the toolchain producer, which keeps
a moving upstream branch from changing the meaning of a tools commit.

The architecture gate protects the long-term shape of the workspace: it
enforces one-way crate dependencies, keeps production modules bounded, requires
module documentation in `aros-cli`, and prevents subprocesses from bypassing
the shared execution and observability layers.

## Release model

GitHub Releases are the canonical source for native archives. Package channels
are consumers of those immutable, measured artifacts – never independent binary
builds.

Before the first stable tag, every supported native archive and downstream
package path must pass its own gate:

1. four native archive builds, deterministic production and clean-room smoke
   tests;
2. archive manifests, checksums, SPDX SBOMs, Sigstore evidence and GitHub
   provenance;
3. isolated download and verification of an immutable GitHub draft;
4. byte-for-byte Debian package installation checks, signed APT verification,
   Homebrew installation on all supported hosts, and AUR qualification on both
   Linux architectures; and
5. promotion only after every public channel verifies its measured bytes.

The definitive details, including failure and credential boundaries, live in
[release engineering](https://metaneutrons.github.io/aros-tools/reference/releases/)
and [package publication](https://metaneutrons.github.io/aros-tools/reference/publication/).

## Documentation

The documentation is versioned with the source and built with Astro Starlight:

```console
cd docs-site
npm ci
npm run build
```

The checked-in lockfile is authoritative. The same `npm ci` build is used for
the GitHub Pages deployment, so the published documentation and pull-request
checks exercise the same dependency graph.

## License

Unless a file states otherwise, the Rust workspace and documentation are
available under either the [Apache License 2.0](LICENSE-APACHE) or the
[MIT License](LICENSE-MIT), at your option. Vendored and AROS-derived inputs
retain their own notices and licenses.
