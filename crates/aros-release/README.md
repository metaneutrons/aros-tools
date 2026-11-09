# aros-release

`aros-release` is the repository-internal, fail-closed producer for native
`aros-tools` release archives. It consumes binaries that were built once for a
specific host, writes a normalized archive plus a machine-readable manifest and
SHA-256 sidecar, and verifies the result by reading it back.

The tool is deliberately not part of the public installation. Release CI uses
it so archive creation and validation have the same structured diagnostics and
opt-in logging contract as the shipped AROS command-line tools.

## Dispatcher controller (`aros-release dispatcher`)

The protected V2 release start of `metaneutrons/aros-toolchains` is controlled
from Rust. `aros-release dispatcher` carries the dispatcher-side commands. They
are inactive building blocks: no workflow calls them yet, and a zero exit status
is never native qualification or release authority. The Python adapters in
`aros-toolchains/scripts/toolchain/` are the behavioural oracle; the closed
contracts (fixed fields, byte limits, canonical JSON, digest joins, fail closed
on 204 or any unknown answer, no retry of a mutation) are kept.

| Command | Does | Does not |
| --- | --- | --- |
| `select` | Reads the runner event and GitHub metadata (job token, read-only), joins the dispatcher identity, downloads the one start archive and writes record, signature bundle and a selection report as exclusive private files. | Verify the start signature or grant any authority. |
| `verify-claim` | Authenticates the signed start while exactly one selected claim asset exists (byte pins, claim join, provider run/job/tag/candidate, exact numeric claim readback, `gh attestation verify`, frozen inputs, runtime byte pins). | Verify the claim signature, the native run or grant dispatch authority. |
| `dispatch` | Fixes the input contract and refuses fail closed. | Contact any provider. The single native dispatch, its binding and once-only consumption are `aros-toolchains#115`. |
| `verify-binding` | Authenticates the original dispatcher's signature of the post-dispatch run binding against the independently observed native context. | Authenticate the native run, consume the claim or qualify a release. |

Every provider access goes through one `Provider` trait with a fixed `gh`
implementation (github.com, job token, bounded output, no credential fallback).
Tests substitute an offline provider; `AROS_TOOLCHAINS_CHECKOUT` additionally
enables byte-for-byte comparisons of claim, binding and canonical JSON against
the Python oracle.

Known differences from the oracle, all fail-closed:

- JSON floats are rejected at parse time (every contract carries integers only).
- The start archive is read from the downloaded bytes in memory instead of a
  private temporary file, with the same member, size, CRC and local-header
  checks.
- Runtime evidence is joined by byte pins (lock, receipt, interface, `bin/aros`
  digest). Re-deriving the materialization is `aros-toolchains#114`.
- The runner OIDC acquisition is not wired: `GhProvider` refuses, so `select`
  and `verify-claim` cannot run live until the workflow wiring lands
  (`aros-toolchains#106`). The OIDC claims check is implemented and tested.
