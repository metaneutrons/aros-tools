# Documentation source audit — 2026-09-05

Scope: the public AROS tools documentation, README and contributor entry points.
Implementation reference: `9be605ca980cc673d698001e5ab12b2beaf034f4`.
This overhaul changes documentation and its presentation, not Rust behavior,
release inputs or qualification policy.

## Coverage and source boundaries

| Area | Evidence inspected | Result documented |
| --- | --- | --- |
| Frontend | `crates/aros-cli/src/main.rs`, `commands.rs`, freshly built `aros --help` recursively | All 29 leaf commands, their options, checkout requirements and mutation boundaries |
| Source lifecycle | `crates/aros-cli/src/source.rs` | Init refs are explicit and detached; sync expects a branch name without a namespace; ignored files and submodules are part of clean-tree checks |
| Compiler selection | CLI `toolchain.rs`, `host_compiler.rs`, `aros-common` target defaults | Cross-toolchain locks differ from host LLVM metadata; built-in host assets lack required digests; explicit local prefixes do not acquire release provenance |
| Product build and boot | CLI `build.rs`, `boot.rs`, `build_tools.rs`, `aros-cmake-engine` | Embedded engine ownership; six CMake helpers versus eight installed programs; PC x86 QEMU boot implementation, not automatic architecture selection |
| Boards | `aros-board` schema, templates, media and console implementation | Four models with explicit transport restrictions; init always uses the Pi-4 USB-ECM template; no automated JTAG/SWD or power control |
| Companion tools | Parsers, implementations and crate contracts for all seven public companions | GenMF architecture verification only; collector direct/driver distinction; optional fetch checksums; PKG-only ROM tool; restricted AHI modes; generator publication |
| Configuration and diagnostics | `aros-common` schema/codes, public environment contract and component logging setup | Versioned diagnostic array, source publication states, complete environment list, component-specific file-only logging defaults |
| Application development | `engine/toolchains/AROS.cmake` and frontend model | Compiler selection is reusable, but does not constitute a complete exported SDK or application-packaging frontend |
| Versions and installation | Release policy, installer contract and release inventory | Separate tools/compiler identities, eight-program suite, no-clobber installer, conditional A/B policy and verified future package procedures |

Each guide/reference links to its owning implementation or canonical contract.
All linked `aros-tools/blob/main` file paths were checked against the workspace.
The configuration reference retains every declared environment name; the
existing environment gate and its regression test pass.

## Measured checks

- `cargo build --locked -p aros-cli`: passed. The resulting binary enumerated
  29 leaf command paths; every path occurs in the command reference.
- Invalid `build --jobs 0` with JSON diagnostics: exit 1, `AR0001`, the
  `aros-tool-diagnostics-v1` envelope with a `diagnostics` array.
- `scripts/check-workspace.sh docs`: passed; zero npm vulnerabilities,
  Astro check with zero errors/warnings/hints, 25 generated pages and
  1,648 checked links. The Astro build emits no route warnings.
- `python3 scripts/check-environment-contract.py` and
  `python3 scripts/environment_contract_test.py`: passed.
- `scripts/release/test-release-policy.sh`: passed. Its maintainer-tooling
  documentation assertion follows the moved installation instructions into the
  contributor guide without relaxing the expected tooling contract.
- Chrome production-preview sweep: all 24 content pages at 390, 768, 1280,
  1440 and 1920 pixels, dark and light themes — 240 page checks, no page-level
  horizontal overflow, missing selected navigation or JavaScript/console errors.
- Interaction checks: persistent theme selection, code copy, expandable
  installation procedures, desktop/mobile search, correct search URL prefix,
  keyboard skip link, mobile menu focus isolation and navigation — passed.
- Additional 320-pixel spot checks of overview, CLI, configuration and boards,
  plus the custom 404 page and its search-index exclusion — passed.
- Screenshots reviewed for overview, long reference, mobile tables and search.
  Command tokens remain intact; long tables/code blocks scroll within content.
- Public-content review excludes service-operating details. Package endpoints
  and consumer verification instructions remain public.
- `git diff --check`: passed.

## Limits

These checks validate documentation coverage, source consistency and the local
production rendering. They are not a new complete AROS product build, four-host
binary release qualification, accessibility certification or physical UART boot
test. No public tools releases were listed at audit time; guides explicitly
mark native packages as pending and recommend source installation today.

At completion of this local audit, the changed site had not yet been published.
Temporary browser automation and screenshots are local QA evidence, not shipped
dependencies. Publication is a separate step after the pull request checks.
