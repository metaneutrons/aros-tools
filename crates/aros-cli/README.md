# `aros` CLI

`aros` is the user-facing orchestration boundary for an AROS checkout. It
discovers the repository, selects declarative target and toolchain state,
resolves the installed build-tool suite, configures and builds AROS, validates
boot results, and drives guarded physical-board workflows. It invokes the
transpiler, collector, generators, and runners as independent processes rather
than linking their implementations.

## Command model

| Command | Responsibility |
| --- | --- |
| `setup` | install the host compiler or locked cross-toolchains |
| `host-compiler` | manage downloaded host LLVM |
| `build-tools` | build/check the complete Rust build-tool suite |
| `toolchain` | install, list, verify, and locate released AROS cross-toolchains |
| `build` / `clean` | configure and build a declared target profile |
| `test` | run evidence-producing, non-interactive QEMU boot validation |
| `board` | validated physical-board, network boot, serial, and removable-media workflows |
| `source init` | atomically clone and configure an upstream AROS checkout, optionally from a fork |
| `install` | validate and transactionally install one complete native release suite |
| `golden` | capture or compare deterministic transpiler output |
| `source sync` | validate and fast-forward a clean branch from a reviewed upstream remote |
| `info` | report active compiler, toolchain lock, and configured targets |

The root `aros-targets.toml` is the sole source of target profiles and host
compiler assets. Missing, invalid, or empty target configuration is fatal for
commands that consume target profiles; global commands and `info` remain useful
outside a configured checkout.
`AROS_HOST_COMPILER_DIR` and `AROS_HOST_COMPILER_URL` are the explicit local
overrides.
Released cross-toolchains are selected exclusively through
`aros-toolchains.lock.toml`; an explicit local toolchain remains an auditable
override and is never silently copied into the immutable store.

## Source checkout and upstream synchronization

Create a pristine upstream checkout from any directory:

```console
aros source init ./AROS
```

Fork contributors can keep the fork as `origin` while recording the reviewed
upstream identity separately:

```console
aros source init ./AROS-NX \
  --fork git@github.com:example/AROS-NX.git \
  --upstream https://github.com/aros-development-team/AROS.git \
  --ref refs/heads/master
```

The destination must not exist. Clone, remote setup, optional ref checkout,
level-by-level recursive submodule initialization, layout validation, and
clean-tree checks run in sibling staging; each nested `.gitmodules` file is
parsed and its transport validated before that level is contacted. The checkout
appears at the requested path only after all steps pass. Publication uses the
host kernel's atomic no-replace rename, so a destination created concurrently
wins without being overwritten. An explicit
ref must be an explicit `refs/heads/NAME`, `refs/tags/NAME`, or full 40/64-digit
commit OID. It is resolved once, reported as an exact commit, and checked out
detached so the result cannot be mistaken for a moving tracking branch.

`aros source sync` accepts only a clean attached branch and verifies the
configured `upstream` URL against the official URL or an explicit
`--upstream`. It fetches the `refs/heads/BRANCH` selected by `--ref` into an
isolated quarantine, resolves one exact commit OID, imports only that OID under
a run-owned ref, and leaves the caller's `FETCH_HEAD` untouched. It rejects
divergence and validates recursive submodules plus every declared target graph
in a standalone temporary repository with its own object database. Replacement
refs, grafts, repository-local attributes, filters, sparse-checkout controls,
URL rewrites, credential helpers and other checkout-affecting local Git config
are rejected.

Target-graph validation supplies the complete MetaMake selector context. New
profiles declare `[targets.transpiler]` in `aros-targets.toml`; the bridge for
older AROS-NX revisions accepts only a matching named CMake preset and reviewed
CMake defaults. A changed or incomplete context fails closed instead of being
re-derived from architecture guesses.

A persistent owner record protected by a non-blocking kernel lock serializes
`aros` source operations; its file is deliberately reused and must not be
deleted as a "stale lock." The branch advances through a compare-and-swap only
if the lock identity and captured branch, index, worktree, recursive submodules,
and repository semantics still match. Submodule objects are copied from the
validated candidate before the branch CAS, and publication uses candidate-local
URLs with network protocols disabled. Any post-CAS failure attempts a
non-forcing rollback and reports typed diagnostic context
`commit_state: "rolled_back"` or `commit_state: "indeterminate"`; final-output
failure after a successful mutation reports `commit_state: "committed"`. The
command never runs `reset --hard`, overwrites concurrent
user changes, or treats post-commit status-output/temporary-cleanup trouble as a
failed source mutation.
A pristine upstream checkout without
`aros-targets.toml` can be synchronized deliberately with `--no-transpile`;
the skipped validation is reported rather than presented as success.

Source transport is deliberately narrow: explicit HTTPS, SSH/SCP, and local
paths only. Embedded web credentials, arbitrary remote-helper schemes,
system/global Git configuration injection and local URL rewrites or credential
overrides are rejected or disabled. HTTPS and SSH run non-interactively; use
an SSH agent for authenticated forks rather than putting a secret in a URL.
Reviewed relative local sources are canonicalized before isolated staging and
stored as absolute remote URLs.

Git subprocesses have a 30-minute process-group deadline; each target-graph
transpilation has a 10-minute deadline. Captured diagnostic streams retain at
most 64 KiB per stream and explicitly report truncation or timeout context.

## Build-tool resolution

All required Rust build tools must come from one directory. The CLI checks, in
order, an explicit `AROS_BUILD_TOOLS_DIR`, the directory containing the running
`aros` executable, and directories on `PATH`. It passes the verified directory
to CMake as `AROS_RUST_TOOLS_DIR`; no ambient executable can replace an
individual member of the set.

Packaged installations ship the complete suite together. Developers who need
`aros build-tools build` set `AROS_TOOLS_SOURCE_DIR` to an aros-tools checkout.
The command builds the required packages into that checkout's
`target/release`. An embedded `tools/aros-tools` workspace is recognized only
as a temporary migration fallback.

Raspberry Pi disk writes are a separate safety boundary. They require a fresh
scan identity, whole/removable/unmounted-media validation, explicit apply
intent, staged image verification, a platform-native exclusive claim, and
complete readback verification. The macOS claim helper does not replace any
of those checks.

## Third-party source integrity and offline builds

`aros build --offline` is a complete network policy for the build transaction:
both released toolchain acquisition and `%fetch` port-source access are
restricted to already installed, cached, or explicitly local inputs. An
offline cache miss fails with the archive name and cache path; it never falls
through to HTTP, FTP, the AROS external-source cache, or a named mirror.

Upstream-compatible `%fetch` declarations may optionally provide exact
archive contracts:

```make
%fetch mmake=example-fetch archive=example-1.0 suffixes="tar.xz tar.gz" \
    checksums="example-1.0.tar.xz=sha256:<digest> example-1.0.tar.gz=sha256:<digest>"
```

The transpiled CMake path delegates to the native `aros-fetch` verifier. It checks
downloads and cache hits before unpacking, rejects malformed or incomplete
multi-suffix declarations, and reports expected and actual digests on mismatch.
The classic upstream GNU Make path retains `scripts/fetch.sh` as its compatible
fallback; it consumes the same explicit declarations but is not silently used
by the transpiled build. No hash is inferred or emitted into generated CMake.
Release and CI validation can add `--require-fetch-checksums` (or set
`AROS_FETCH_REQUIRE_CHECKSUMS=1`) to reject hashless source archives while
ordinary upstream builds remain additive and compatible.

## Diagnostics and local logging

`aros` uses the same versioned diagnostic document as the transpiler,
`aros-collect`, and the AHI runner. Human output is the default. Automation can
select one JSON document on standard error:

```console
aros --diagnostic-format json build --preset pc-x86_64
```

The setting is global, so it may also follow a subcommand. The equivalent
environment variable is `AROS_DIAGNOSTIC_FORMAT=human|json`.

Every fatal diagnostic has a stable code, stage, severity, message, actionable
hint, and optional deterministic context. The schema is
`aros-tool-diagnostics-v1`. Current code families are:

| Code | Boundary |
| --- | --- |
| `AR0001` | command-line invocation |
| `AR0002` | diagnostic rendering or local logging |
| `AR0101` | repository discovery |
| `AR0111` | source input or ref contract |
| `AR0112` | hardened source transport |
| `AR0113` | repository-wide source lock |
| `AR0114` | source branch, index, worktree, submodule, or remote state |
| `AR0115` | standalone source-candidate validation |
| `AR0116` | atomic source publication or compare-and-swap materialization |
| `AR0201` | workspace or profile configuration |
| `AR0301` | host-tool resolution or execution |
| `AR0401` | AROS toolchain selection, verification, or installation |
| `AR0501` | non-source network operation |
| `AR0601` | CMake configuration |
| `AR0602` | build execution |
| `AR0701` | boot validation |
| `AR0801` | physical-board operation |
| `AR0802` | removable-media safety |
| `AR0901` | output publication, cleanup, deployment, or golden data |
| `AR0999` | internal invariant |

Child-process exit codes and signals are preserved as structured context. In
JSON mode, non-interactive child output is isolated from the diagnostic
stream. A failed child's standard output and error are included in the
diagnostic and bounded to 64 KiB per stream. Captured processes are drained
concurrently without retaining more than that limit; oversized output carries
an explicit truncation marker. An explicitly interactive serial-console
process retains its terminal.

## Opt-in local logs

Logging is disabled by default and never selects an implicit destination:

```console
aros build --preset pc-x86_64 \
  --log-level info \
  --log-format jsonl \
  --log-file build/aros-cli.jsonl
```

`--log-level` accepts `off`, `error`, `warn`, `info`, `debug`, and `trace`;
`--log-format` accepts `human` and `jsonl`. Supplying only `--log-file` enables
the `info` level. The environment equivalents are `AROS_LOG_LEVEL`,
`AROS_LOG_FORMAT`, and `AROS_LOG_FILE`.

JSONL records use `aros-cli-log-v1`. They contain the selected command context
but no timestamp, hostname, CI-runner observation, or other ambient metadata.
Logs and fatal diagnostics are separate contracts; a logging failure becomes
an `AR0002` diagnostic and fails the command.
