# AROS MetaMake transpiler contracts

The transpiler follows upstream MetaMake declarations and must not silently
freeze third-party packages or ordinary repository files at locally selected
bytes.

## Pin policy

- `%fetch` keeps the upstream archive name, version, URL, suffix and patch
  declaration. The transpiler does not add an archive checksum that upstream
  did not declare.
- Repository scripts and patches are direct build dependencies. Editing one
  invalidates the corresponding generated output or fetch result; it does not
  require updating a second checksum.
- Checked input manifests are path-only capability source closures. CMake
  calculates live per-file hashes during configuration, watches every listed
  input and passes the resulting snapshot to the runner only to detect changes
  between configuration and execution. Ordinary source edits never require a
  checked-in digest update.
- A fixed fingerprint is permitted only for an opaque recipe fragment or
  source inventory that the transpiler expands into a hard-coded job graph.
  The complete allowlist and rationale are in
  `capability-fingerprints.pins`. Fingerprint drift is fatal and tells the user
  that the capability and transpiler must be reviewed and updated.
- A component that CMake downloads directly without an upstream `%fetch`
  integrity mechanism must retain a supply-chain checksum. Currently this is
  the closed GRUB 2.12 source download in `cmake/GrubSourceLock.cmake`. This is
  not an `aros-cli` package pin.
- Released host and cross toolchains use separate, explicit release locks.
  Those locks are part of the reproducible distribution contract and are not
  transpiler capability fingerprints.

Never update a capability fingerprint merely to make a build pass. First
reduce the fingerprint to the smallest opaque input, review the semantic
change, update the hard-coded jobs if necessary, and exercise all supported
target profiles.

## Failure policy

Filesystem traversal, fetch discovery, MetaMake parsing and recognised
capability drift are fatal. Parallel failures are collected, sorted and
deduplicated so one invocation reports the complete deterministic set. A
capability deliberately excluded from another supported architecture is not
drift; an owned declaration whose recipe no longer matches is.

Coverage gaps outside a recognised capability remain reports beside the
generated CMake graph. They must remain visible until they are either modelled
or promoted to a release gate.

The error handling is improved but not yet enterprise-complete. Remaining
work includes typed diagnostic codes and source spans, atomic publication of
the generated graph plus all sidecars, fatal report-write failures, a
machine-readable diagnostic mode, and removal of internal panic-based
invariants at embedded-data boundaries. `OPEN-POINTS.md` tracks this work.
