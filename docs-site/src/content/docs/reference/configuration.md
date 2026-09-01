---
title: Configuration
description: Source contracts, target profiles, toolchain locks, caches and local board state without hidden configuration precedence.
---

`aros-tools` separates versioned checkout configuration from machine-local
state. Repository files may select reproducible inputs; local files may identify
hardware and storage but must not redefine release integrity.

## Checkout-owned files

| File | Owner | Purpose |
| --- | --- | --- |
| `aros-targets.toml` | selected AROS checkout | Target presets, architectures and build paths |
| `aros-toolchains.lock.toml` | selected AROS checkout | Exact release archive, host/profile, size and SHA-256 |
| generated CMake graph | configured build | Transactional output of the selected transpiler version |

Unknown fields and unsupported schema versions fail closed. Relative paths are
resolved against the discovered checkout, not the caller's arbitrary working
directory.

Each target may declare the complete MetaMake selector set directly:

```toml
[[targets]]
name = "pc-x86_64"
arch = "x86_64"
platform = "pc"
bsp = "generic"

[targets.transpiler]
family = ""
variant = ""
toolchain = "llvm"
cpu32 = "i386"
use_mmu = true
```

`float_abi`, when needed, remains a target-level field. These selectors are
configuration, not architecture guesses: `source sync` passes every value to
the isolated transpiler validation. For already-published AROS-NX revisions
without this table, a compatibility bridge accepts only explicit matching
`CMakePresets.json` values and the reviewed CMake defaults known to that tools
version. If those defaults change, synchronization stops with a request to add
the explicit table or update `aros-tools`; it never silently invents a new
target context.

## Tools-owned source contract

`contracts/aros-source-v1.toml` pins the AROS-NX source revision used by the
complete workspace test and the exact `aros-toolchains` producer commit that
selected it. CI checks both files rather than duplicating a hash in workflow
YAML. This contract affects development qualification; it does not force an
end user to rename or relocate an AROS checkout.

## Local board profiles

Board profiles default to `~/.config/aros/boards.toml`. Override the file with
`--config PATH` or `AROS_BOARDS_FILE`. The file may contain local interface,
serial and physical-device identity, so it should not be committed to an AROS
source repository.

Use `aros board init --board NAME` to print a schema-correct template and add
`--apply` only when you intend to create the file. Existing profiles are never
silently overwritten.

## Environment variables

`contracts/public-environment-v1.toml` is the versioned allowlist for every
environment value read by shipped Rust code. The workspace gate compares that
allowlist with code and this page, so a new hidden override fails CI. Public,
ambient-host and test-only names are deliberately separate.

Where a command offers the same setting directly, the explicit command-line
option wins. Otherwise the order is the documented environment variable,
versioned checkout configuration, then the documented default. Setting an
explicit directory override is fail-closed: an invalid override is reported;
the command does not continue searching a lower-priority location.

### Frontend, source and integrity controls

| Variable | Effect |
| --- | --- |
| `AROS_OFFLINE` | Set to `1` to forbid network access and require verified local content |
| `AROS_FETCH_OFFLINE` | Standalone `aros-fetch` equivalent of offline mode |
| `AROS_FETCH_REQUIRE_CHECKSUMS` | Set to `1` to reject third-party AROS sources without SHA-256 |
| `AROS_UPSTREAM_URL` | Expected canonical URL for `aros source sync`; an explicit `--upstream` wins |
| `AROS_HOME` | Absolute root for tools-owned state; defaults to `$HOME/.aros` |
| `AROS_CACHE_DIR` | Absolute archive-cache override |
| `AROS_HOST_COMPILER_DIR` | Absolute managed host-compiler location override |
| `AROS_CROSS_TOOLCHAINS_DIR` | Absolute content-addressed cross-toolchain store override |
| `AROS_BUILD_TOOLS_DIR` | Exact binary suite directory; all required programs must pass bounded version probes |
| `AROS_TOOLS_SOURCE_DIR` | Select a reviewed `aros-tools` source workspace for helper builds |
| `AROS_HOST_COMPILER_URL` | Credential-free HTTPS base override for the checkout-selected host LLVM asset; the checkout's exact version and SHA-256 remain mandatory |
| `AROS_BOARDS_FILE` | Local board-profile file; explicit `--config` wins, then this value, `XDG_CONFIG_HOME`, and `HOME` |
| `AROS_DIAGNOSTIC_FORMAT` | `human` or `json` frontend diagnostics |
| `AROS_LOG_LEVEL`, `AROS_LOG_FORMAT`, `AROS_LOG_FILE` | Explicit local frontend logging |
| `AROS_VERIFY_GENMF_TIMEOUT_SECONDS` | Verifier GenMF deadline in seconds (1–3600; default 30) |
| `SOURCE_DATE_EPOCH` | Standard deterministic timestamp consumed by release assembly |

`AROS_HOST_COMPILER_URL` changes transport location, not artifact identity.
Downloads and redirects remain within credential-free HTTPS and the selected
checkout's SHA-256 must verify before extraction. `AROS_BUILD_TOOLS_DIR` is a
code-execution boundary: use only a directory you control. Every required tool
must be executable, report the running `aros-tools` version within the bounded
probe deadline, and the explicit directory never falls back to ambient `PATH`.
`AROS_BOARDS_FILE` may describe physical devices and network interfaces; keep
it machine-local and review it before deploy or removable-media operations.

### Component diagnostics and logging

Each component uses the same `DIAGNOSTIC_FORMAT`, `LOG_LEVEL`, `LOG_FORMAT`,
and `LOG_FILE` suffix contract. The exact public names are:

- `AROS_AHI_DIAGNOSTIC_FORMAT`, `AROS_AHI_LOG_LEVEL`, `AROS_AHI_LOG_FORMAT`, `AROS_AHI_LOG_FILE`
- `AROS_COLLECT_DIAGNOSTIC_FORMAT`, `AROS_COLLECT_LOG_LEVEL`, `AROS_COLLECT_LOG_FORMAT`, `AROS_COLLECT_LOG_FILE`
- `AROS_FETCH_DIAGNOSTIC_FORMAT`, `AROS_FETCH_LOG_LEVEL`, `AROS_FETCH_LOG_FORMAT`, `AROS_FETCH_LOG_FILE`
- `AROS_GENMODULE_DIAGNOSTIC_FORMAT`, `AROS_GENMODULE_LOG_LEVEL`, `AROS_GENMODULE_LOG_FORMAT`, `AROS_GENMODULE_LOG_FILE`
- `AROS_RELEASE_DIAGNOSTIC_FORMAT`, `AROS_RELEASE_LOG_LEVEL`, `AROS_RELEASE_LOG_FORMAT`, `AROS_RELEASE_LOG_FILE`
- `AROS_ROMTOOL_DIAGNOSTIC_FORMAT`, `AROS_ROMTOOL_LOG_LEVEL`, `AROS_ROMTOOL_LOG_FORMAT`, `AROS_ROMTOOL_LOG_FILE`
- `AROS_TRANSPILER_DIAGNOSTIC_FORMAT`, `AROS_TRANSPILER_LOG_LEVEL`, `AROS_TRANSPILER_LOG_FORMAT`, `AROS_TRANSPILER_LOG_FILE`
- `AROS_VERIFY_DIAGNOSTIC_FORMAT`, `AROS_VERIFY_LOG_LEVEL`, `AROS_VERIFY_LOG_FORMAT`, `AROS_VERIFY_LOG_FILE`

Logging remains off unless a non-off level and an explicit local log file are
selected. `COLLECT_AROS_DEBUG` is the collector's public debugging switch for
retaining its temporary directory; it can disclose intermediate object and
link state and should not be set in routine or release builds.

### Ambient host fallbacks

`HOME` supplies the default AROS state/configuration roots,
`XDG_CONFIG_HOME` supplies the preferred board-config root, `PATH` is searched
only for a complete version-matched build-tool suite when no
`AROS_BUILD_TOOLS_DIR` is set, and `CARGO` selects the Cargo executable for the
explicit `aros build-tools build` source workflow. These ambient variables do
not replace checkout locks, source identities, or artifact digests.

### Qualification-only variables

The following names are explicitly test-internal, are not supported user
configuration, and may be compiled out or inert in release builds:

- `AROS_TEST_SOURCE_ROOT`, `AROS_TEST_TOOLS_DIR`
- `AROS_PUBLICATION_TEST_FAIL_AT`, `AROS_PUBLICATION_TEST_PAUSE_AT`, `AROS_PUBLICATION_TEST_PAUSE_MS`, `AROS_PUBLICATION_TEST_CRASH_AT`
- `AROS_FETCH_TEST_LOG_FAIL_AT`, `AROS_FETCH_TEST_PAUSE_AT`, `AROS_FETCH_TEST_PAUSE_MS`

The host LLVM version comes exclusively from the selected checkout's
`[host_compiler]` contract; no ambient version variable overrides it.
The toolchain lock is always `<checkout>/aros-toolchains.lock.toml`, and an
unlocked local cross-toolchain is selected only with the explicit `--local` or
`--toolchain-dir` command-line option. There are no hidden environment
variables that replace either selection.

## Precedence and inspection

The effective order is explicit CLI option, documented environment variable,
versioned checkout configuration, then a documented constant default. There is
no fallback to a neighboring checkout or an arbitrary compiler on `PATH`.

Use `aros info`, `aros toolchain list` and `aros board doctor` to inspect the
selected values without mutating source or hardware. `aros info` prints the
effective state root, archive cache, cross-toolchain store, and whether a
managed host compiler was verified against the current checkout's digest.
