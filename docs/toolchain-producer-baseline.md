# TCP-M0 baseline evidence

Recorded: 2026-09-05. Scope: isolated historical producer contract replay,
package-format capture and source review. No compiler production, recursive
source build, release matrix, publication or consumer-lock promotion occurred.

## Inputs and isolation

| Input | Exact identity |
| --- | --- |
| Producer | `c8039cf2b7291097ad62c6750bd7367e91a068f4` |
| AROS source for structural tests | `f3cfc243a84065166a46da28b0a5b22bbd0f8869` |
| Tools source inspected | `39a7738d7b7d09d96fad6787afa45823629ea77c` |
| Merged planning PR | [#36](https://github.com/metaneutrons/aros-tools/pull/36), `1fab25fd4bcda09f29b393ab177e324149ee8b43` |
| Host | macOS / Darwin, arm64; Python 3.14.7 |

Producer and AROS structural tests used fresh detached worktrees. Both were
clean before and after testing. AROS submodules were **not initialized**:
these tests inspect the tracked source graph/patches and use synthetic build
fixtures; they do not need a complete recursive compilation checkout. A real
producer must reject that incomplete input at preflight. This result must not
be reported as a real four-host build or clean-snapshot implementation test.

The existing developer producer worktree had unrelated edits, and an older
test worktree had a broken Git administrative path. Neither was modified or
repaired for this exercise. Historical identities in this report and golden
fixture are evidence, not new production pins or implicit default sources.

## Executed checks

The historical suite was executed with `AROS_TEST_SOURCE_ROOT` pointing at the
isolated source and exited 0:

```sh
AROS_TEST_SOURCE_ROOT=/path/to/isolated/AROS \
  /path/to/isolated/aros-toolchains/scripts/toolchain/tests/test-producer.sh
```

Its reported successful gates were private host Python environment, LLVM 11
patch applicability, `crosstools-release` MetaMake graph, synthetic two-root
relocation and the aggregate producer contract suite. The suite also checks
recipe/checkout/source policy, mock packages, compare/index/recovery and
workflow inventory contracts. Mock tools do not prove compiler behavior.

The retained local suite log has SHA-256
`bd8d5b89e5585d41be45817f387791009a92cd372e600b13e9e300703a68719f`.
It is diagnostic evidence outside the release inventory, not a signed CI
attestation. Temporary paths and fixture recipe hashes in that log are not
golden outputs; replay success, not byte-identical test logs, is the criterion.

Additional maintained M0 checks:

- `python3 -m unittest discover -s scripts -p '*_test.py'`: 19 tests passed,
  including 10 new offline producer contract/vector tests.
- `python3 scripts/capture-toolchain-baseline.py --producer-dir <isolated-producer> --check`:
  exact captured vector reproduced and all 16 legacy manifest cases matched.
- The captured AST inventory contains all 36 top-level `producer.py` functions;
  the contract test requires an ownership entry for every function.

These tests deliberately distinguish specification consistency, measured old
behavior and future native acceptance requirements. They do not instantiate a
native producer, and do not claim that the Rust consumer was dynamically tested
against every new case. Rust/schema differences are source-reviewed decisions;
actual cross-parser conformance is mandatory in M2/M4.

## Small format vector

The [checked-in vector](../scripts/fixtures/toolchain-producer/package-v1.json)
was measured using the historical producer's `add_tar_entry`, `tree_inventory`,
`validate_manifest` and `json_bytes` functions. It contains a synthetic
manifest and the existing UTF-8 file/directory/relative-symlink tree fixture.
It does not contain runnable compilers, an SBOM or release attestation.

| Measurement | Value |
| --- | --- |
| Compressed PAX/XZ archive size | 848 bytes |
| Archive SHA-256 | `555c7ff6340d9ddfc704e8a1a64f3b8a88d41afe444bfc3972b6bae532b2f01c` |
| Embedded manifest SHA-256 | `09f095c5c2292c53363c50b85471cc968d4075c31c2c58e53207d99ac1573a61` |
| Payload tree SHA-256 | `11cbd45962f89c54c02fc9c1ae55eb283774b76425c08564da060bd5ca9c840b` |
| Shared tree fixture file SHA-256 | `b8472bdf29379d2956995cf60d90503c8581615cc0f80aa90e6d44f11758dbb7` |
| Historical producer.py SHA-256 | `c9f954f47dfa89dca2767e550d3f0561e91151b05355def9333ddf5df69a46d5` |

The fast test decodes the captured bytes without extracting them to disk and
checks sorted members, types, uid/gid/names, modes, epoch, link target, file
contents and manifest agreement. Captured canonical JSON uses unescaped UTF-8
and one final LF. Native XZ byte parity across library versions and four hosts
remains M4 evidence, not something inferred from this local capture.

## Resource measurements and open gates

No representative compiler wall time, peak RAM, scratch space or optimal
parallelism was measured in M0. Fixture-suite resource usage is not a substitute.
The proposed first build interface therefore requires explicit job and deadline
budgets. Real Linux x86-64 and macOS AArch64 preview measurements belong to M1;
the complete native four-host envelope is a later qualification gate.

The [contract](toolchain-producer-contract.md) records the upstream capability,
credential, path/cache, snapshot, parser and recovery decisions. In particular:

1. the consumer index is not silently accepted as a lock;
2. the source checkout is never patched or switched implicitly;
3. the fetch guard is not called an OS network sandbox;
4. current broad release tests remain unchanged until native parity;
5. tools-owned CMake compatibility, native recursive source audits and trusted
   executor evidence still require implementation and measured tests.

M0's evidence is ready for the contract/architecture review. Its acceptance
issue stays open until the PR is merged with green checks. No later milestone
is complete on the strength of these fixtures.
