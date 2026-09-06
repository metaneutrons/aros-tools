# aros-toolchain

The native producer library, currently the first **non-executing TCP-M1
foundation**, not a compiler driver or a released management feature.

Implemented:

- closed recipe-v2 parsing with duplicate/unknown-field rejection, canonical
  lowercase identities, strict integer fields and safe, sorted patch paths;
- bounded recipe input and canonical UTF-8 JSON self-digest verification;
- the shared `aros-tool-diagnostics-v1` envelope with AX0101 (contract) and
  AX0102 (identity) errors and actionable hints, without input-document dumps;
- maintained native conformance/counter-probes against the independent M0
  encoding vectors. No second SHA-256 implementation or source-lock copy.

The library performs no filesystem, environment, subprocess or network access.
A self-consistent recipe is not a verified checkout, trusted executor, valid
source cache, runnable plan, release attestation or permission to publish.
It does not apply declared patches. Callers must not interpret successful
parsing as build readiness.

The CLI consumer `install`, `list`, `verify` and `path` commands are unchanged.
`plan`, `build`, the legacy adapter, state ownership/cancellation, and real
Linux x86-64/macOS AArch64 preview evidence remain later M1 work. No stub command
or additional executable is exposed. `aros-fetch` is added as a dependency
only when native transport is actually implemented, not as an unused promise.

See the [producer contract](../../docs/toolchain-producer-contract.md),
[delivery plan](../../docs/toolchain-producer-plan.md) and
[M1 issue](https://github.com/metaneutrons/aros-tools/issues/29).

Run the maintained native tests with:

```sh
cargo test --locked -p aros-toolchain
```

The normal workspace test gate includes this library on each native CI host.
No compiler build or full A/B matrix is needed for this non-executing change.
