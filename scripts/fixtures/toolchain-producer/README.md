# Toolchain producer conformance inputs

These are maintained TCP-M0 acceptance fixtures, not release artifacts and not
throwaway tests. Nothing here is an installable toolchain or evidence that the
future Rust producer is implemented. Artificial repeated-digit Git identities
and recipe hashes are intentionally confined to synthetic fixtures.

- `package-v1.json` captures an 848-byte PAX/XZ archive, embedded manifest,
  canonical UTF-8 encoding and exact historical producer identity. The manifest
  reuses the existing CLI tree-digest fixture; its SHA-256 detects fixture drift.
  The capture uses the legacy `add_tar_entry` and `json_bytes` functions. It does
  not run compiler, layout, SBOM or attestation qualification.
- `manifest-cases.json` contains 16 structural cases. `legacy` is measured by
  replay; `native` is the required decision for M2/M4. The changed path is an
  array of object keys/array indices and replaces only its final value. The
  base is the `manifest` field of the named package fixture.
- `plan-v1.json`, `result-v1.json`, `receipt-v1.json` illustrate the proposed
  machine interface with synthetic identities. They are not CLI output yet.

The ordinary workspace quality gate runs `toolchain_producer_contract_test.py`
without network, external checkouts, extraction into the workspace or compiler
builds. It checks the frozen artifacts/specification, not a substitute producer.
M1/M2/M4 must wire these inputs into the actual parser/serializer/package tests.

To replay the historical producer, use a reviewed isolated checkout at the
`producer_commit` recorded in the package fixture:

```sh
python3 scripts/capture-toolchain-baseline.py \
  --producer-dir /path/to/isolated/aros-toolchains --check
```

The command executes code from that explicit checkout; do not run it on
untrusted branches. Without `--check`, it prints measured JSON to stdout and
never overwrites a golden file. Review any change, particularly Python/liblzma
encoding differences; do not refresh expected bytes merely to make a test pass.
Native byte parity on all four hosts remains a later milestone gate.
