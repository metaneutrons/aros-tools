# TCP-M6 native cutover evidence

This record closes TCP-M6's implementation boundary. It is evidence for the
native local producer, not a release record. No toolchain tag, archive,
attestation, package-channel update or A/B qualification was created here.

## Exact accepted identities

| Component | Exact commit | Purpose |
| --- | --- | --- |
| `aros-tools` | `016a15b41cc6de346fb81487328d099b4705ccad` | Native frontend; legacy adapter retired |
| `aros-toolchains` | `03506956b25944f1476acf8c9398bbdf714638fe` | Exact executor declaration and cross-repository drift guards |
| AROS-NX source | `f3cfc243a84065166a46da28b0a5b22bbd0f8869` | Versioned source contract |
| Recipe | `c967d865eb213a6b0c91f83281b53456459fee1dc8aaf609766ea281b8c342e4` | `pc-x86_64` local candidate inputs |
| Executor contract | `d9707287ce1ab011e4272ce412640835eef7807890ec9720cc3f64abc71501e3` | Bound native executor contract bytes |

The native recovery boundary merged in [aros-tools PR #96](https://github.com/metaneutrons/aros-tools/pull/96).
[PR #97](https://github.com/metaneutrons/aros-tools/pull/97) removed the
retired adapter and its public command path. The exact producer declaration
landed through [aros-toolchains PR #13](https://github.com/metaneutrons/aros-toolchains/pull/13),
[PR #14](https://github.com/metaneutrons/aros-toolchains/pull/14), and
[PR #15](https://github.com/metaneutrons/aros-toolchains/pull/15). The final
contract run [34260382908](https://github.com/metaneutrons/aros-toolchains/actions/runs/34260382908)
checked out the declared source and executor commits and passed the complete
offline producer contract suite.

## Native frontend and drift guards

The final `aros-tools` main run
[34259378712](https://github.com/metaneutrons/aros-tools/actions/runs/34259378712)
passed formatting, architecture, dependency, release-fixture and test gates.
Its Workspace CI host matrix passed on Linux x86-64, Linux ARM64, macOS ARM64
and macOS x86-64. The preceding PR matrix
[34258156107](https://github.com/metaneutrons/aros-tools/actions/runs/34258156107)
passed the same four native frontend lanes before merge.

The canonical gates now reject legacy entry points and duplicated producer
orchestration, enforce the crate dependency direction, and compare all declared
executor, source-lock, profile and contract identities. The producer workflow,
recovery workflow and compatibility replay must select the same reviewed tools
commit. A mismatch fails before a build root is created.

## Lean real PC candidates

Two independent real native builds used the exact identities above, eight jobs,
fresh owned work/output roots, and `--offline`. The cache contained the
lock-selected source closure plus a 244-package Cargo vendor closure generated
from the selected tools `Cargo.lock`. Cache preparation is separate from the
offline producer run; neither successful build performed a producer-controlled
network fetch.

| Native host | Target | Wall time | Prefix files | Prefix size | `aros-collect` SHA-256 | Publish receipt SHA-256 |
| --- | --- | ---: | ---: | ---: | --- | --- |
| macOS ARM64 | `pc-x86_64` | 31m 37s | 2,760 | 171 MiB | `dcabd47800a4e9815b39a1a202abed265d3aea4cc42f4047c7af0f819e375b04` | `3e206fc61cfab528a539a7cb4cd66a9400d1ef957fe94075c324fe50d5f338aa` |
| Linux x86-64 | `pc-x86_64` | 29m 39s | 2,760 | 205 MiB | `6929ef11bbeb4b80e3dd4c924d407a7c0bdb7d0ba6080e3263fb2b42fba5d8cd` | `4e738e1ffa17ba3a8fcb7cddd582b5e835f6909555cab5cf5bbc3cfc483058fd` |

All six lifecycle receipts (`preflight`, `environment`, `configure`,
`compiler`, `collector`, `publish`) passed on both hosts. The generated
prefixes each passed `aros toolchain verify --preset pc-x86_64 --local …` from
the exact AROS-NX checkout. Their installed `clang` and `ld.lld` report LLVM
11.0.0 and target `x86_64-unknown-aros`.

The macOS executor binary was
`14637c3ca0fc550659dec3243e2d1b18fd381e25462f5530c5e4ff0f39f17d85`;
the Linux executor binary was
`dd414620915f93543008f317ebd2d4961cf3aabce982dd2ac7829b8696e98050`.
They are native host binaries, so their different hashes are expected.

These are intentionally lean diagnostics, not a four-host release matrix and
not a byte-reproducibility comparison. Each result has
`qualification: local-only` and `origin: not-run`.

## Consumer and documentation boundary

The successful workspace quality gate exercised the GitHub archive, Debian,
Homebrew and AUR policy/install fixtures. It did not publish a tool package or
claim that a package channel is available. The public guide now keeps source
candidate prerequisites separate from ordinary released-toolchain consumption,
and states the Cargo-vendor cache precondition explicitly.

The remaining Python use is upstream-owned: AROS configure/template handling
uses a host interpreter with the lock-selected Mako and MarkupSafe archives.
The Rust lifecycle prepares and verifies a private import environment; it does
not invoke `pip` or inherit site packages. The source build's Make/CMake and
the selected Cargo executable remain external build prerequisites, not restored
producer orchestration.

## Rollback and next boundary

No immutable output exists to retarget or delete. If a defect is found before
TCP-M7, revert the affected `aros-tools` change through a normal reviewed PR,
then update the producer declaration to the exact replacement tools commit in
a separate reviewed `aros-toolchains` PR and rerun the contract gate. Retain
failed local roots as evidence; do not adopt them as a resume source. Do not
restore a legacy fallback, hand-edit a release version, retarget a tag, or
promote a local prefix into a consumer lock.

TCP-M7 remains the only boundary that may choose an immutable tag, run the
four-host/three-profile A/B and compatibility matrix, create a draft, or
publish measured artifacts.
