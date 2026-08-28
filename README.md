# AROS-NG Rust tools

This workspace contains the host-side programs used to configure, build,
verify, package, and deploy AROS-NG. It is intentionally compatible with the
upstream AROS source tree: the tools model existing MetaMake and build
contracts and keep generated state outside authoritative upstream inputs.

## Architecture and ownership

| Crate | Owns | Must not own |
| --- | --- | --- |
| `aros-common` | stable diagnostics, local logging, target/config parsing, SHA-256, ELF and toolchain contracts | command policy or component-specific workflows |
| `aros-cli` | repository orchestration, host compiler and released cross-toolchain selection, builds, tests, Pi workflows | transpiler or collector implementation |
| `aros-transpiler` | MetaMake parsing, capability modelling, dependency graph and transactional CMake publication | reference-verifier semantics or package-version policy |
| `aros-verify` | independent historic MetaMake expansion and differential verification | transpiler implementation details |
| `aros-collect` | two-pass AROS linking, set collection, ABI checks and atomic output publication | general build orchestration |
| `aros-genmodule` | `.conf` parsing and generated module/SDK sources | build scheduling |
| `aros-romtool` | ROM package layout and image validation | target selection |
| `aros-ahi-runner` | validated execution of external AHI build contracts | generic external-project execution |
| `aros-macos-disk-claim` | the narrow macOS whole-disk claim lifetime | device selection or removable-media policy |

The dependency direction is deliberately one-way:

```text
aros-common
  ├── aros-cli ──process──> build tools
  ├── aros-transpiler
  ├── aros-verify          (independent reference semantics)
  ├── aros-collect
  ├── aros-genmodule
  ├── aros-romtool
  └── aros-ahi-runner

aros-cli ──macOS only──> aros-macos-disk-claim
```

`aros-cli` invokes the other build tools as standalone processes; it does not
link their implementation crates. This keeps their command contracts,
release artifacts, and failure boundaries explicit. `aros-verify` intentionally
duplicates the relevant MetaMake semantics as an independent oracle. Sharing
the transpiler implementation with its verifier would allow one defect to pass
both sides of the comparison.

## Sources of truth

- Repository target profiles and downloadable host-compiler declarations:
  root `aros-targets.toml`. Missing, malformed, or empty target data is fatal.
- Released AROS cross-toolchains: root `aros-toolchains.lock.toml` and the
  corresponding release manifests.
- Transpiler capability fingerprints: only
  `aros-transpiler/capability-fingerprints.pins`, for explicitly documented
  opaque recipe inputs. Ordinary packages and source files are not pinned.
- Stable diagnostics and local-log schema mechanics: `aros-common`.
- Component-specific diagnostic code, hint, and logging policy: the owning
  component.

## Tool names

The terminology distinguishes three layers:

- **host compiler**: downloaded LLVM used to bootstrap builds; managed by
  `aros host-compiler` (`host-tools` remains a compatibility alias);
- **build tools**: checkout-local Rust executables consumed by CMake; managed
  by `aros build-tools` (`hosttools` remains a compatibility alias);
- **AROS cross-toolchain**: released target compiler, runtime, collector, and
  sysroot selected by `aros toolchain`.

## Development gates

Run from this directory:

```console
cargo fmt --all -- --check
sh scripts/check-architecture.sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The architecture check caps production source files at 2,000 lines, requires
module-level documentation throughout `aros-cli`, keeps the largest test
suites outside production modules, enforces `miette` as the CLI's sole error
boundary, and prevents subprocesses from bypassing the shared execution and
observability layers. Clippy additionally rejects undocumented public error
paths and functions over 500 lines; the three ordered translation/serialization
pipelines above that limit carry explicit, reasoned `expect` markers. The file
cap is a regression limit, not a target size: cohesive modules should remain
substantially smaller.

Every user-facing component must keep human and JSON diagnostics on the shared
`aros-tool-diagnostics-v1` contract. Local logging is opt-in, requires an
explicit destination, and must never enter deterministic release archives.
