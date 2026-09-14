# macOS Intel release-target policy

## Decision

As of 2026-09-14, macOS Intel (`x86_64-apple-darwin`) is not an
`aros-tools` native release target. The maintained native distribution matrix
is:

- Linux x86-64 (`x86_64-unknown-linux-gnu`);
- Linux ARM64 (`aarch64-unknown-linux-gnu`); and
- macOS Apple silicon (`aarch64-apple-darwin`).

This is a permanent product-boundary decision, not a paused runner lane or a
temporary Homebrew exception. A future return of Intel macOS requires a new
reviewed policy, release-contract expansion and fresh native qualification; it
must not be inferred from historical archives, source builds or translated
Apple-silicon execution.

## Consequences

- The tools release workflow, independent A/B, signing, closed inventory and
  Homebrew formula consume exactly the three maintained release targets.
- Homebrew receives no Intel archive selection. The formula remains supported
  on Apple silicon and Linux only.
- The legacy `HB-2026-09-05` exception is retired. No expiry or fallback
  exists because release qualification no longer schedules the Intel lane.
- Historical four-host toolchain evidence and parser compatibility remain
  readable. They do not create a current `aros-tools` Intel distribution
  claim.
- A technically usable Intel host may build from source at its own risk, but
  that is outside the native archive and package-manager support contract.

## Evidence retained

The earlier Homebrew investigation identified a real Homebrew API-cache
installer defect that Intel exposed. It is retained in Git history and must
not be described as an unresolved release blocker after this policy takes
effect: Intel is outside the declared release matrix.
