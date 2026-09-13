# CACHE-M1 qualification record

This record maps the acceptance authority in
[CACHE-M1](cache-management-plan.md#cache-m1) to the implementation and
evidence collected on 2026-09-13. It is an implementation qualification record,
not an acceptance or release claim. The linked tracking issue remains open until
the implementation PR is reviewed and its required CI evidence is attached.

## Reviewed contract

| Criterion | Evidence |
| --- | --- |
| CACHE-M1-A1 | The reviewed cache-management plan merged in [PR #147](https://github.com/metaneutrons/aros-tools/pull/147), commit `3ede9f6`; it defines the versioned family, root, capability, JSON and side-effect contracts. The parser-derived public CLI contract and semantic-example gate were accepted in [PR #164](https://github.com/metaneutrons/aros-tools/pull/164). |
| CACHE-M1-A2 | `b1b6e9e1286200bedf790b1e2a238439abde1f25` adds the `aros-cache` core and passive `aros cache status` / `aros cache compiler status` surface. `crates/aros-cli/tests/discoverability_cli.rs` proves that a hostile apparent backend is never run, roots are observed without creation and JSON marks the operation passive. `cargo test -p aros-cache` passed with 12 tests. |
| CACHE-M1-A3 | [PR #163](https://github.com/metaneutrons/aros-tools/pull/163), merged as `41fe09c`, removed the redundant `aros ccache --stats` switch and established the false-clear regression coverage. `b1b6e9e` removes public clear entirely until the owned lifecycle exists. The repeatable probe introduced in `04ea8c73a59d33e489d96f8c0033c0a8f6c7b8bc` proves the distinct isolated backend operations described below. |
| CACHE-M1-A4 | `05f176093430a41a11a34cdbc4aaef7ba0838f00` creates one typed Rust selection, uses it for product and board build, disables automatic caching offline, and passes only the exact executable to CMake. `crates/aros-cmake-engine/src/tests.rs` runs a real CMake build and observes launcher use for C and C++; it also proves the required ASM bypass because CMake has no supported ASM-specific launcher property. The native deterministic compatibility fixture passed with the compiler-cache mode forced off. |
| CACHE-M1-A5 | `docs-site/src/content/docs/reference/cli.md` and `docs-site/src/content/docs/workflows/cache.md` describe the passive, daemon-safe status and build boundaries. Generated build/board contracts include `--compiler-cache`. `bash scripts/check-workspace.sh docs` completed successfully for this candidate. |

## Isolated backend evidence

The qualification script is
[`verify-compiler-cache-backends.sh`](../scripts/verify-compiler-cache-backends.sh).
It uses a new `mktemp` root as its only Home, configuration, cache, output and
sccache Unix-domain socket location. Its cleanup first stops only that private
server, then removes only the validated temporary root. No user cache, daemon,
credentials or source tree is read or changed.

On source commit `04ea8c73a59d33e489d96f8c0033c0a8f6c7b8bc`, the command passed on:

| Host | Backend versions | Assertion |
| --- | --- | --- |
| macOS ARM64 local executor | ccache 4.14; sccache 0.17.0 | ccache reset retained the compiled entry; ccache clear removed it; sccache reset retained the compiled entry. |
| CachyOS Linux x86-64 executor | ccache 4.14; sccache 0.17.0 | Same three assertions under an independent private temporary root and Unix-domain server socket. |

The script fails closed below ccache 4.14.0 or sccache 0.17.0. Passive status
does not require those versions; the floor applies only to this qualified
backend-operation evidence. It deliberately does not claim sccache entry
clearing: upstream exposes `--zero-stats`, not an entry-deletion operation.

## Candidate commands and gates

The following passed after `04ea8c7`:

```text
scripts/verify-compiler-cache-backends.sh
ssh cachy 'sh -s' < scripts/verify-compiler-cache-backends.sh
cargo test -p aros-cache
cargo test -p aros-cmake-engine
cargo test -p aros-toolchain compatibility::execution::tests::executes_every_phase_with_two_roots_and_closed_environments -- --exact
sh scripts/check-architecture.sh
bash scripts/check-workspace.sh docs
```

The full `cargo test -p aros-cli --no-fail-fast` suite passed for the preceding
build-selection commit `05f1760`; this record does not substitute that local
evidence for the implementation PR's required CI results.
