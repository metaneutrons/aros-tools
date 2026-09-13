---
title: Inspect cache state
description: Inspect, populate, retain, and safely remove exact reviewed AROS cache objects.
---

`aros cache` is the resource-oriented cache interface. Passive status commands
do not create a directory, acquire a lock, hash a tree, access a network, start
a compiler-cache daemon, or change a backend. Source-cache and compiler-archive
`fetch` are separate, explicit population boundaries. Every lifecycle mutation
begins with a preview and requires the matching short-lived apply token:
`release` removes one named protection receipt, while `remove` deletes one
selected immutable object.

```sh
aros cache status
aros cache status --format json

aros cache compiler status
aros cache compiler status --backend ccache --format json
aros cache compiler status --backend sccache --dir /work/aros-compiler-cache
aros cache compiler prepare --backend sccache --dir /work/aros-compiler-cache
aros cache compiler stats --backend sccache --dir /work/aros-compiler-cache --format json
aros cache compiler reset-stats --backend sccache --dir /work/aros-compiler-cache --format json
aros cache compiler clear --backend sccache --dir /work/aros-compiler-cache --format json

aros cache archives status
aros cache archives list --project /work/AROS --toolchain --preset pc-x86_64 \
  --host linux-x86_64 --format json
aros cache archives fetch --project /work/AROS --host-compiler --offline
aros cache archives verify --project /work/AROS --toolchain --preset pc-x86_64
aros cache archives keep --project /work/AROS --toolchain --preset pc-x86_64 \
  --name release-candidate
aros cache archives remove --project /work/AROS --toolchain --preset pc-x86_64 --format json
aros cache archives release --name release-candidate --format json

aros cache cargo status --dir /work/aros-source-cache
aros cache cargo list --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --format json
aros cache cargo fetch --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache
aros cache cargo verify --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --format json
aros cache cargo keep --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --name release-candidate
aros cache cargo remove --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --format json
aros cache cargo release --dir /work/aros-source-cache --name release-candidate --format json

aros cache genmf status --dir /work/aros-genmf-cache
aros cache genmf list --source-dir /work/AROS --dir /work/aros-genmf-cache --format json
aros cache genmf verify --source-dir /work/AROS --dir /work/aros-genmf-cache
aros cache genmf refresh --source-dir /work/AROS --dir /work/aros-genmf-cache
aros cache genmf keep --source-dir /work/AROS --dir /work/aros-genmf-cache \
  --name release-candidate
aros cache genmf remove --source-dir /work/AROS --dir /work/aros-genmf-cache \
  --source rom/mmakefile --format json
aros cache genmf release --dir /work/aros-genmf-cache --name release-candidate --format json

aros cache sources status --dir /work/aros-source-cache
aros cache sources list --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache --format json
aros cache sources fetch --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources verify --compatibility-ports-lock toolchains/ports.json \
  --dir /work/aros-source-cache --format json
aros cache sources keep --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache --name release-candidate
aros cache sources remove --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache --role producer:toolchain_component:llvm-project@20.1.7 \
  --format json
aros cache sources release --dir /work/aros-source-cache --name release-candidate --format json
```

Use the JSON documents for scripts. `--format json` changes normal stdout only;
use the global `--diagnostic-format json` option if an error must be parsed.

## What the status reports

`aros cache status` reports five cache families without scanning their
contents:

| Family | Current status boundary |
| --- | --- |
| `compiler` | Passively discovers `sccache` and `ccache`; `prepare` can claim one empty private directory as an AROS-owned, local-only namespace. It never adopts an existing cache. |
| `archives` | Inspects, verifies, and explicitly populates selected host/cross-compiler archive bytes. Installed host compilers and cross-toolchains are outside this cache family. |
| `sources` | `status` never guesses a root. `list`, `fetch`, `verify`, `keep`, and role-selected preview/apply `remove` require an explicit reviewed selector and root. |
| `cargo` | `status` observes an explicit parent root. `list`, `fetch`, `verify`, `keep`, and preview/apply `remove` require explicit producer, tools, Cargo, and cache inputs. Global Cargo state is excluded. |
| `genmf` | `status` observes an explicit parent root. `list`, `verify`, `refresh`, `keep`, and input-selected preview/apply `remove` require explicit source, interpreter, and cache inputs. Verification reports and build trees are excluded. |

Archive-root resolution is deterministic: `AROS_CACHE_DIR` wins when set;
otherwise AROS uses `AROS_HOME/cache`, and `AROS_HOME` defaults to
`$HOME/.aros`. Every configured root must be absolute. Status does not follow a
root symlink: it reports `symlink` instead, so a later lifecycle operation can
make an explicit safety decision.

## Compiler backend observations

`aros cache compiler status --backend auto` uses the stable selection order:
`sccache`, then `ccache`. `--backend sccache` and `--backend ccache` project
one backend without silently substituting the other. An unavailable backend is
a successful status result with `selected_backend: null`; it is not a build
fallback.

The report intentionally labels backend storage as `unconfigured` or
`configuration_uninspected`. Reading a backend's effective settings may require
processing a configuration file or talking to a server, and sccache statistics
can start that server. This passive command therefore reports only the names of
recognized environment variables, never their potentially sensitive values, and
does not claim that a configured backend is local, remote, or mixed.

`--dir DIR` requires an absolute path and observes that root without following
a symlink, creating it, or assigning it to sccache or ccache. Without `--dir`,
the selected backend's future AROS-owned candidate is shown below
`AROS_HOME/cache/compiler/v1/<backend>`. Both are status-only candidates, not
evidence that the running backend currently uses that location. JSON reports
the same boundary through `root_binding: "status_only_not_applied"`, an
explicit `selected_backend` value (or `null`), `selection_basis`,
`effective_build_selection`, and a complete `side_effects` object. This makes
the limits safe for automation to check rather than infer from prose.

### Managed local namespaces

`prepare` is the explicit ownership boundary. It accepts exactly one concrete
backend and, without `--dir`, uses that backend's candidate below
`AROS_HOME/cache/compiler/v1/`. An explicit path must be absolute. The selected
path must be an empty private directory (or not exist yet); a non-empty
directory, symbolic link, malformed marker, or a marker for the other backend
is rejected. For sccache, the generated `<namespace>/server.sock` must be at
most 103 bytes long, which is safe on every supported Unix host; choose a
shorter `AROS_HOME` or `--dir` if needed. AROS writes a generated local-only
configuration, a private
`data/` directory, and a no-clobber ownership marker. Running the command again
only revalidates that exact state.

```sh
aros cache compiler prepare --backend sccache --dir /work/cache/aros-sccache
aros build --compiler-cache sccache \
  --compiler-cache-dir /work/cache/aros-sccache --offline
```

For the default namespace, omit `--dir`:

```sh
aros cache compiler prepare --backend sccache
aros build --compiler-cache sccache --offline
```

The generated environment removes every ambient `SCCACHE_*` and `CCACHE_*`
variable before setting the selected backend's paths. `sccache` receives a
private disk store, configuration and Unix-domain server socket; `ccache`
receives a private store and generated configuration. A build retains a shared
lifecycle lease from CMake configure through the final compile command. This is
why a prepared cache can be used offline without trusting ambient compiler-cache
configuration.

### Statistics, counter reset and clear

`stats` operates only on a namespace that `prepare` has already claimed. It
holds a shared lifecycle lease while it invokes the selected local executable.
Although statistics look observational, both supported backends may materialize
local metadata and sccache may start its private server. Use
`cache compiler status` when a strictly passive result is required.
The managed operation floor is ccache 4.14.0 or sccache 0.17.0; preparation
and passive status do not invoke a backend and have no version floor.

```sh
aros cache compiler stats --backend ccache --dir /work/cache/aros-ccache --format json
aros cache compiler reset-stats --backend ccache --dir /work/cache/aros-ccache --format json
aros cache compiler clear --backend ccache --dir /work/cache/aros-ccache --format json
```

The last two commands first return a JSON or human preview with an exact
five-minute `apply_token`; they make no backend call at that stage. Re-run the
same command with `--apply TOKEN` only after inspecting the preview:

```sh
aros cache compiler reset-stats --backend ccache --dir /work/cache/aros-ccache \
  --apply "$TOKEN"
aros cache compiler clear --backend ccache --dir /work/cache/aros-ccache \
  --apply "$TOKEN"
```

Apply acquires an exclusive lease that excludes `aros build` and `aros board
build` for that namespace. `reset-stats` invokes only the backend's counter
reset and never selects compiler output files for deletion. `clear` measures
the local `data/` tree before issuing its token, with a hard limit of 200,000
entries and 6 GiB of regular-file data. ccache clearing uses the controlled
ccache command to remove compiler-cache entries; ccache may retain or recreate
its own statistics and sharding metadata. sccache clearing first stops its
generated private Unix-domain server, proves that its exact socket no longer
accepts connections, descriptor-unlinks any stale socket name, then
descriptor-removes exactly the still-matching measured `data/` tree and
recreates an empty owned directory. Neither operation can adopt, scan, clear,
or fall back to ambient, foreign, remote, symlinked, or unprepared storage. A
changed ownership marker, generated configuration, token expiry, active build
reader, unsafe socket, or budget overrun fails closed for either operation; a
changed data-tree snapshot also rejects `clear`. Run a fresh preview after
correcting the condition.

## Compiler archive operations

Compiler archives are a shared, content-addressed byte cache beneath the
archive root: `downloads/sha256/<archive-sha256>.tar.xz`. It is not the
host-compiler installation and not the cross-toolchain store. `status` is
passive and does not enumerate its contents. `list` reads metadata for exactly
one declared archive; it reports `missing`, `present_unverified`, `unsafe`, or
`inaccessible` and never hashes it.

Every non-status archive operation requires an explicit `--project DIR` that
resolves to an AROS source checkout, plus exactly one purpose:

- `--host-compiler` reads the selected host LLVM asset from that checkout's
  `aros-targets.toml`.
- `--toolchain --preset NAME` reads the selected AROS cross-toolchain archive
  from its `aros-toolchains.lock.toml` and checks the target triple against the
  checkout's target contract.

The optional `--host HOST` chooses a release-matrix host without running,
extracting, or installing a foreign binary. Without it, AROS selects the
running host. The JSON result records the resolved project, configuration file,
selection origin, host, release/profile where applicable, expected size and
SHA-256, cache path, HTTPS archive URL, and transport provenance. For a host
compiler, the explicit `AROS_HOST_COMPILER_URL` transport override is honored;
it changes no version or SHA-256 identity.

```sh
# Observe only the archive-root metadata.
aros cache archives status --format json

# Prepare a Linux cross-toolchain archive from macOS without executing it.
aros cache archives fetch --project /work/AROS --toolchain --preset pc-x86_64 \
  --host linux-x86_64

# Inspect or prove only the selected archive bytes.
aros cache archives list --project /work/AROS --host-compiler --format json
aros cache archives verify --project /work/AROS --toolchain --preset pc-x86_64
aros cache archives keep --project /work/AROS --toolchain --preset pc-x86_64 \
  --name release-candidate
aros cache archives remove --project /work/AROS --toolchain --preset pc-x86_64 --format json
```

`fetch` uses the same verified acquisition primitive as `aros host-compiler
install` and `aros toolchain install`: its cache identity is the declared
SHA-256, an available object is reverified, a new transfer stages privately and
is published without clobbering an existing object. It does not extract or
install the archive. `--offline` forbids every transfer and succeeds only when
the selected object is already verified. `--refresh` reacquires the exact same
declared identity, conflicts with `--offline`, and still never replaces the
content-addressed cache object or an installed tree.

`verify` checks only exact archive size (when the lock declares it) and
SHA-256. It deliberately does **not** validate extraction safety, payload-tree
identity, a host-compiler/toolchain receipt, release provenance, or an
attestation; installation owns those stronger checks. A host compiler may have
an unknown declared size, in which case download remains bounded by the
consumer's hard archive limit and the output says so explicitly.

### Archive retention and exact removal

Archive cleanup is deliberately selector-based. `keep` creates one durable,
no-clobber named reference for the archive selected by the same reviewed
`--project` / `--host-compiler` or `--toolchain --preset` inputs used by
`fetch` and `verify`. A retained object cannot be removed. `release --name`
first previews only that reference; its exact token is required to remove the
reference. It never removes archive bytes.

```sh
release_preview="$(aros cache archives release --name release-candidate --format json)"
release_token="$(printf '%s' "$release_preview" | jq -r '.preview.apply_token')"
aros cache archives release --name release-candidate --apply "$release_token"
```

`remove` without `--apply` is a non-mutating preview. It hashes exactly the
selected archive, records every retention blocker, and emits a short-lived
`apply_token`. Review the JSON or human output, then pass that exact token
unchanged to the same selector:

```sh
preview="$(aros cache archives remove --project /work/AROS --toolchain \
  --preset pc-x86_64 --format json)"
token="$(printf '%s' "$preview" | jq -r '.preview.apply_token')"
aros cache archives remove --project /work/AROS --toolchain --preset pc-x86_64 \
  --apply "$token"
```

Apply remeasures the archive and repeats root, policy, retention, identity and
SHA-256 checks under an exclusive lifecycle lease. It refuses an expired or
tampered token, any changed object, and active cooperating readers or writers.
It never scans an archive root, infers unused data, or removes a different
object. `prune` is intentionally unavailable until every cache family has a
verified ownership and reader-lease contract.

## Cargo vendor generations

`aros cache cargo` owns only immutable, AROS-managed Cargo vendor generations.
It is the required cache handoff before a native toolchain producer builds its
Rust collector offline; it neither builds a collector nor changes a selected
`Cargo.lock`.

`status --dir DIR` observes only that explicit parent root. The other commands
select a generation from four inputs:

- the producer's `toolchains/rust-toolchain.toml` pin;
- a clean tools checkout's committed Git tree plus its `Cargo.toml` and
  `Cargo.lock`;
- the exact Cargo executable and its version under that pin; and
- the explicit managed cache root.

The resulting object is published once at
`cargo/v1/<selection-sha256>/` with `cargo-vendor/`, a single-placeholder
`cargo-vendor-config.toml`, and `receipt.json`. The receipt binds the portable
input identity and measured vendor-tree/template digests. Checkout paths are
reported for diagnosis but do not select an object, so the producer can consume
the same generation after it snapshots the selected committed sources into a
private work directory.

```sh
# No dependency resolution, vendor-tree hashing, or cache mutation. The command
# runs bounded Git and `cargo --version` probes to prove the exact selection.
aros cache cargo list --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --format json

# The only population boundary. Cargo runs with a private CARGO_HOME, HOME,
# temporary directory and working directory.
aros cache cargo fetch --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache

# Rehash every vendored package and compare it with Cargo.lock and receipt.
aros cache cargo verify --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache

# Require the existing verified object and prohibit Cargo resolution.
aros cache cargo fetch --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --offline

# Retain a fully revalidated generation. Release first returns a token-bound
# receipt preview; applying it removes only the named receipt, never vendor bytes.
aros cache cargo keep --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --name release-candidate
aros cache cargo remove --producer-dir /work/aros-toolchains \
  --tools-dir /work/aros-tools --dir /work/aros-source-cache --format json
aros cache cargo release --dir /work/aros-source-cache --name release-candidate --format json
```

`fetch` invokes Cargo's own `vendor --locked --versioned-dirs` through bounded
process control; AROS does not reimplement Cargo's dependency resolver. Cargo
may use network transport only during this explicit online operation. The
generated configuration is parsed and rewritten by Rust only when it contains
the one expected vendor-directory mapping. Registry and HTTPS Git dependencies
must match the selected lock closure; unsupported source mappings, a missing
Git dependency, a checksum mismatch, a cancellation, or an invalid published
generation are failures. Fetchers selecting the same missing generation
cooperate through a cancellable, bounded cache lock: one publishes the
complete object and waiters independently reverify it before reuse. An
existing generation is never repaired or replaced.

Neither global `CARGO_HOME` nor user Cargo configuration, credentials, or
temporary data enters the published generation. `--cargo FILE` selects a
specific executable when `PATH` is not the intended one; otherwise `aros`
records the absolute Cargo path it resolved. The native lifecycle holds a
shared lifecycle lease while it revalidates and copies the selected generation
into a fresh private collector environment, then always passes Cargo
`--locked --offline`. `keep` revalidates the same exact generation while an
exclusive lifecycle lease is held and writes a no-clobber named receipt.
`remove` never scans or clears the cache root: it first returns an exact,
five-minute preview and only removes that generation when its token, retained
references, object snapshot, and reader/writer lease state still match.
`cache cargo list` does not hash a vendor tree, but it runs bounded Git and
`cargo --version` probes before it reads the selected generation receipt.

## GenMF reference expansions

`aros cache genmf` owns only immutable reference expansions used to compare
transpiled CMake output with upstream MetaMake. It is not a build-output cache,
does not own verifier reports, and does not inspect or delete legacy flat
mtime entries from older unreleased tooling.

Every `list`, `verify`, `refresh`, `keep`, and `remove` command needs three
explicit inputs:

- `--source-dir DIR`: an existing no-follow AROS checkout containing
  `config/make.tmpl`, its complete `%include` closure, `tools/genmf/genmf.py`,
  and MMake files;
- `--dir DIR`: an existing no-follow parent root, under which only
  `genmf/v1/` belongs to this cache family; and
- optionally `--python FILE`: an exact absolute interpreter. Without it,
  `aros` resolves `python3` once from `PATH` and records that absolute path.

The selection identity hashes the source MMake bytes, recursive template
closure, GenMF script, resolved Python executable and normalized version, plus
the fixed generator format/options. Checkout absolute paths and mtimes do not
select an object. Each completed object lives at
`genmf/v1/<selection-sha256>/` and contains exactly `expansion.mk` and a
receipt binding those identities and the measured output. This prevents both
stale preserved-mtime results and historical slash-to-percent filename
collisions.

```sh
# Observe only root metadata. No source selection, payload reads, locks, or
# Python process are involved.
aros cache genmf status --dir /work/aros-genmf-cache --format json

# Hash current selection inputs, make a bounded `python --version` probe, and
# inspect direct final-path metadata only.
aros cache genmf list --source-dir /work/AROS --dir /work/aros-genmf-cache

# Rehash every selected immutable generation without invoking GenMF. Selection
# still makes the bounded `python --version` probe needed for interpreter identity.
aros cache genmf verify --source-dir /work/AROS --dir /work/aros-genmf-cache

# The explicit refresh boundary. Ctrl-C is cooperative; the lifecycle lease,
# generation lock, and GenMF process share the selected bounded timeout.
aros cache genmf refresh --source-dir /work/AROS --dir /work/aros-genmf-cache \
  --timeout-seconds 60

# Retain the exact fully verified current selection as one closed reference.
aros cache genmf keep --source-dir /work/AROS --dir /work/aros-genmf-cache \
  --name release-candidate

# Preview exactly one current source-root-relative input; review its JSON
# blockers and apply_token before passing that token back with --apply.
aros cache genmf remove --source-dir /work/AROS --dir /work/aros-genmf-cache \
  --source rom/mmakefile --format json

# Preview release of only the named retention receipt. Applying its returned
# token does not delete a generation.
aros cache genmf release --dir /work/aros-genmf-cache --name release-candidate --format json
```

`refresh` runs only the selected resolved interpreter and upstream GenMF in a
private environment. It stages output before publication. A missing generation
is published atomically only after source stability and receipt measurement;
an existing generation is reverified and must match the fresh bytes exactly.
It is never repaired or replaced. Missing includes, symlinked inputs, source
mutation, cancellation, timeout, unsafe final state, and a byte mismatch fail
closed. `verify` never invokes GenMF and never repairs cache state; it makes
only the bounded Python version probe needed to reconstruct the selection.
The verifier keeps a shared lifecycle lease from successful materialization
through the reference-shape read, so a cooperating removal cannot delete a
generation after it was verified but before its contents are consumed.

`keep` reselects and fully verifies every current immutable generation while
exclusive lifecycle locks are held, then writes one no-clobber named receipt
for the complete selection. `release` first returns a five-minute preview for
one named receipt; its exact token is required before the receipt is removed.
`remove` accepts only an exact current
source-root-relative MMake path via `--source`; it cannot infer or search an
object from a cache filename. Without `--apply` it returns a five-minute
preview with retention blockers and an apply token. With that exact token,
removal rechecks the selected identity, object snapshot, retention references,
and active reader/writer leases before deleting only that one generation.

## Reviewed source-cache operations

Source-cache commands always require an explicit absolute `--dir` that already
exists and has no symlink components. A selector is exactly one of:

- `--source-lock FILE` for `aros-toolchain-source-lock-v2` producer inputs;
- `--compatibility-ports-lock FILE` for
  `aros-toolchain-compatibility-ports-v2` inputs; or
- `--source-fetch-plan FILE` for one product
  `aros-cache-source-fetch-plan-v1` closure.

Selectors are bounded regular files read without following their final
symlink. The command records the SHA-256 of the exact selector bytes as
`request_sha256`; filenames and checkout state never select a closure
implicitly.

`status` observes root metadata only. `list` reports direct-child metadata
states (`missing`, `present_unverified`, `unsafe`, or `inaccessible`) and never
hashes a payload. `verify` takes private no-follow snapshots, measures every
object, and requires exact size/SHA-256 agreement for strict declarations.
`fetch` verifies every already-present object in the selected closure before
any transport; a missing object may then be acquired from its reviewed HTTPS
candidates in declaration order. It uses a per-object no-clobber guard and
private staging, so cancellation or a competing writer cannot expose a partial
file. It never refreshes, replaces, deletes or repairs an existing cache
object. `--offline` forbids every transfer and turns a selected miss into a
diagnostic.

### Product source-fetch plans

A product plan has stable entry roles, ordered credential-free HTTPS candidates
that all select one direct cache filename, an explicit `archive` or `patch`
representation, a normalization policy, and an integrity declaration. `locked`
contains exact `sha256` and `size`. An
`unverified` entry must additionally set a finite `max_size`, may use only
`exact-bytes-v1`, and requires the deliberate opt-in below:

```json
{
  "schema": "aros-cache-source-fetch-plan-v1",
  "entries": [
    {
      "role": "product:grub@2.12",
      "filename": "grub-2.12.tar.xz",
      "candidates": [
        {
          "url": "https://mirror.example.invalid/grub-2.12.tar.xz"
        }
      ],
      "representation": "archive",
      "normalization": "exact-bytes-v1",
      "integrity": {
        "kind": "locked",
        "sha256": "8e7e7c3d2f3a8ecdd1224c7984436b074bfe04c6e9a35a10c129cd4ec5e734ef",
        "size": 123456
      }
    }
  ]
}
```

This is an input schema, not a source recipe: it does not execute a script,
discover additional files, extract an archive, or apply a patch. A direct
patch uses `"representation": "patch"`, a required `patch` object with a
relative optional `subdirectory` and reviewed `options` (`-p0` through `-p9`,
`-f`, `-N`, or `--forward`), and remains `exact-bytes-v1`. Patch application
stays with the consumer that owns its target tree; the metadata remains part of
the measured request identity.

```sh
aros cache sources fetch --source-fetch-plan grub.fetch-plan.json \
  --dir /work/aros-source-cache --allow-unverified --format json
```

The result labels such an object's locally measured digest and size as
`measured_unpinned`. It is not an upstream checksum, is never converted into a
hidden pin, and cannot satisfy a producer or compatibility lock. Prefer a
strict product plan whenever the upstream offers a stable content identity.

### Producer migration

The historical `aros toolchain producer cache` and `aros toolchain producer
compatibility-ports` frontends are removed. Bootstrap and prove native inputs
with the common commands instead:

```sh
aros cache sources fetch --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources verify --source-lock toolchains/llvm.sources.json \
  --dir /work/aros-source-cache
aros cache sources fetch --compatibility-ports-lock toolchains/ports.json \
  --dir /work/aros-source-cache
```

The native producer and compatibility executor independently reverify the
same typed request before consuming its cache objects. The native producer
holds a shared lifecycle lease across its source-lock closure while upstream
Configure and MetaMake may read it. `keep` retains a whole closure under one
named reference; `release` is preview-first and removes only that receipt with
its exact token; `remove` is deliberately role-selected, preview-first, and
blocked until every reference is released. A pinned workflow must replace every
removed producer-cache invocation before upgrading aros-tools.

## Current limits

Root-wide pruning remains intentionally unavailable. Do not replace it with
ad-hoc directory deletion. Compiler reset and clear are limited to their
prepared AROS-owned namespace and their documented preview/apply operation;
they are not a generic local or remote cache-management facility.

`aros ccache` remains the legacy statistics frontend during the transition. It
may start sccache because it queries backend statistics. Its former `--clear`
flag is intentionally rejected at parser level: the command had neither a
shared ownership boundary nor preview/apply protection, and `sccache -z`
resets counters rather than deleting entries. Use `aros cache compiler
reset-stats` or `aros cache compiler clear` for the managed lifecycle.
Retention for compiler-result caches remains deliberately unavailable because
the backends do not expose a portable immutable-object retention model.

## Build launcher policy

`aros build` and `aros board build` share
`--compiler-cache auto|off|sccache|ccache`. The frontend resolves the exact
absolute executable once and passes that selection to CMake for C and C++;
CMake never independently chooses a backend. CMake does not expose a supported
language-specific compiler launcher for ASM, so assembly stays a direct
deterministic invocation. `off` also removes stale launcher settings from an
existing CMake build tree on the next configure.

`auto` selects the first available prepared AROS-owned namespace in stable
order: sccache, then ccache. If neither has been prepared, it selects `off`
without starting a backend. An explicit backend requires its default managed
root to be prepared; use `--compiler-cache-dir DIR` only with an explicit
backend to select another prepared root. Both explicit and automatic managed
selection work offline because ambient compiler-cache configuration is removed
and the generated namespace is local-only.

For every other state path and precedence rule, see
[configuration](/aros-tools/reference/configuration/). For a toolchain install
or an offline build, use the current documented
[toolchain workflow](/aros-tools/workflows/toolchains/) rather than treating a
status result as an integrity or installation verification.
