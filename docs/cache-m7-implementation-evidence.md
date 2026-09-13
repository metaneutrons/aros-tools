# CACHE-M7 implementation evidence

Status: acceptance evidence completed on 2026-09-14.

This record covers the cache-frontend migration implemented by
[`cef4f0e`](https://github.com/metaneutrons/aros-tools/commit/cef4f0e).
Publication is a separate maintainer action: the acceptance authority requires
a coordinated consumer pin to an exact reviewed tools revision, not a new
package or tag. This record includes that consumer migration and the complete
CACHE-M7 evidence for [issue #146](https://github.com/metaneutrons/aros-tools/issues/146).

## Exact inputs and scope

| Input | Identity | Role |
| --- | --- | --- |
| Parent release-candidate tree | [`807f828d1deae003e35c7c462618ca06511840de`](https://github.com/metaneutrons/aros-tools/commit/807f828d1deae003e35c7c462618ca06511840de) | Reviewed base before this breaking CLI migration |
| M7 implementation | [`cef4f0e`](https://github.com/metaneutrons/aros-tools/commit/cef4f0e) | Removes the visible `aros ccache` command family without an alias |
| Integrated M7 tree | [`2474bc3c89c21c80d23197c28ef63cd3c18603a4`](https://github.com/metaneutrons/aros-tools/commit/2474bc3c89c21c80d23197c28ef63cd3c18603a4) | Squash merge of [PR #175](https://github.com/metaneutrons/aros-tools/pull/175), with successful [main Workspace CI](https://github.com/metaneutrons/aros-tools/actions/runs/34789234321), [documentation](https://github.com/metaneutrons/aros-tools/actions/runs/34789234327) and [CodeQL](https://github.com/metaneutrons/aros-tools/actions/runs/34789234329) gates |
| Toolchain consumer audit input | [`2f371bf900cfd93ee26fd861ec1333432d7561ba`](https://github.com/metaneutrons/aros-toolchains/commit/2f371bf900cfd93ee26fd861ec1333432d7561ba) | Current producer-executor contract examined for legacy callers |
| Coordinated consumer migration | [`6266ab047058cc2b87e38d9f6018bb25d7b658a2`](https://github.com/metaneutrons/aros-toolchains/commit/6266ab047058cc2b87e38d9f6018bb25d7b658a2) | Squash merge of [aros-toolchains PR #53](https://github.com/metaneutrons/aros-toolchains/pull/53), pinning every active producer workflow and the executor declaration to `2474bc3c89c21c80d23197c28ef63cd3c18603a4` |

The change removes only the obsolete root command. `ccache` remains a supported
compiler-cache backend value alongside `sccache`; CMake and build launchers
therefore retain their backend integration unchanged. The one supported
management surface is `aros cache compiler`.

## Criterion evidence

| Criterion | Evidence | Result |
| --- | --- | --- |
| CACHE-M7-A1 | The Clap `Commands` variant, dispatcher, diagnostic boundary and generated contract page for `aros ccache` are removed. The black-box parser test proves `aros --diagnostic-format json ccache` fails before a hostile `ccache` executable can run. The deterministic Bash, Zsh and Fish completion test proves `root:ccache` and `root/ccache` are absent while `root/cache` remains. Consumer PR #53 adds a producer-contract regression that rejects a reintroduced legacy call in every active producer workflow and script. | passed |
| CACHE-M7-A2 | `docs-site/src/content/docs/workflows/cache.md` documents passive inspection, prepared offline compiler-cache namespaces, both backends, strict integrity handling, retention and token-bound preview/apply cleanup. `cargo test -p aros-cli --test public_command_documentation --offline` passed all parser/example-owner checks; `npm run build` passed Astro check and static rendering. The acceptance-scope documentation merge passed [Workspace CI](https://github.com/metaneutrons/aros-tools/actions/runs/34790055117) and [CodeQL](https://github.com/metaneutrons/aros-tools/actions/runs/34790055137). | passed |
| CACHE-M7-A3 | On macOS ARM64 and CachyOS Linux x86-64, `verify-compiler-cache-backends.sh` proved warm reuse survives counter reset and that ccache clear removes entries. `verify-managed-compiler-cache-lifecycle.sh` proved managed ownership, preview/apply containment, both backends, and a stopped private sccache socket. `compiler_cache_launcher_reaches_c_cxx_and_asm_build_rules` passed on both hosts. The local `aros-cache` offline-policy and active-reader lease tests, plus `aros-common` cancellation process-group test, passed. | passed |
| CACHE-M7-A4 | `cargo test -p aros-toolchain --test native_lifecycle --offline` passed 11 producer cases, including cold vendor generation offline, receipt-chain execution, cancellation, deadline and retained-root behavior. `toolchain_fetch_bridge_cli` and the 28-case `toolchain_plan_cli` suite passed. The exact consumer commit passed [PR contract run 34789852879](https://github.com/metaneutrons/aros-toolchains/actions/runs/34789852879) and [main contract run 34789884726](https://github.com/metaneutrons/aros-toolchains/actions/runs/34789884726). No compiler-output identity algorithm changed, so a compiler A/B qualification is not implicated by this migration. | passed |
| CACHE-M7-A5 | This document records input identities, commands, host scope and omitted claims. The full local `aros-cli` suite (134 unit, 4 discoverability, 27 observability, 15 semantic, 3 documentation, 34 source cases), `aros-cache` (38 cases), `aros-cmake-engine`, formatter and architecture contract passed after the change. | passed |

## Supported and excluded storage scopes

The verified mutation scope is an AROS-owned compiler namespace below
`AROS_HOME/cache/compiler/v1/<backend>` or an explicitly prepared absolute
directory. Neither probe read or changed ambient ccache/sccache configuration,
remote storage, user cache roots, installed toolchains, compiler archives, or
source caches. Each host probe created a private temporary root, private HOME,
backend configuration, data root and (for sccache) Unix-domain socket.

## Explicit limits

This evidence does not create, retarget or publish a package, tag, toolchain
archive, board deployment or compiler A/B matrix. Any such activity remains a
separate maintainer decision and must use its own qualification policy.
