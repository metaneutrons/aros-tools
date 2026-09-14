# Documentation source-parity audit — 2026-09-13

Scope: the public AROS tools documentation, README and contributor entry
points. This record is a maintenance evidence ledger, not release or hardware
qualification evidence.

## Current source boundaries

| Area | Source checked | Current public claim |
| --- | --- | --- |
| Frontend command tree | `crates/aros-cli/src/main.rs`, `commands.rs`, and recursive built `--help` | 53 visible leaf commands; the hidden MetaMake bridge is intentionally excluded |
| Native producer control plane | `crates/aros-cli/src/toolchain_producer.rs` | All 16 public producer stages are named with their authority boundary in the CLI reference |
| Toolchain lifecycle | `toolchain_management.rs`, `toolchain_lifecycle/` and `toolchain_selection.rs` | Inventory, import, register, selection, removal and garbage collection distinguish preview from token-confirmed mutation |
| Board profiles | `aros-board` schema, template generator and CLI parser | Init requires an explicit model; defaults and optional transports are restricted to reviewed model/transport pairs |
| CI matrix | `scripts/plan_ci_platform_matrix.py` and `.github/workflows/ci.yml` | Linux x86-64, Linux AArch64 and macOS AArch64 are the complete maintained release hosts; Intel macOS is not a release target |
| Release targets and channels | Release policy plus native archive/package contracts | Archive targets are distinct from active CI coverage and from published availability |
| Public-service boundary | Documentation workflow and site content | Endpoint and verification instructions are public; credentials, account setup and service-operation procedures are not |

## Durable regression coverage

- `crates/aros-cli/tests/public_command_documentation.rs` recursively invokes
  the built public CLI help tree, asserts the 53-leaf inventory and requires
  each command to appear in `reference/cli.md`.
- `scripts/ci_platform_matrix_test.py` proves the documentation-only allowlist,
  the three active native hosts and the fail-closed fallback for unknown paths.
- `scripts/check-doc-links.py` validates generated paths and anchors after the
  Astro build; the documentation gate also checks locked npm dependencies,
  static output and Worker asset constraints.

## Verification for this audit

- `cargo test -p aros-cli --test public_command_documentation --locked`: passed
  (1 test).
- `scripts/check-workspace.sh docs`: passed with 27 generated pages, 1,874
  checked internal links, zero Astro diagnostics and 81 validated publication
  assets.
- Public delivery is verified separately by the deployment workflow. A green
  local documentation build is not a claim that a new documentation revision
  has already been deployed.

## Deliberate limits

This audit does not assert a tools release, toolchain release, full product
build, three-host artifact matrix, A/B determinism, external attestation or
physical hardware boot. Those claims require their own immutable evidence.
