---
title: Troubleshooting
description: Diagnose repository, toolchain, network, build and board failures from stable diagnostic codes without guesswork.
---

## Capture a machine-readable failure

Re-run the failing command with JSON diagnostics:

```sh
aros --diagnostic-format json COMMAND ... 2>diagnostic.json
```

The process writes exactly one `aros-tool-diagnostics-v1` document on failure
and exits non-zero. To collect local execution events as well:

```sh
aros --log-level debug --log-format jsonl --log-file ./aros-debug.jsonl COMMAND ...
```

Logs are never uploaded automatically. Review them before sharing because local
paths and board identifiers may be present.

## Common boundaries

| Code family | Boundary | First check |
| --- | --- | --- |
| `AR01xx` | repository discovery | Enter the intended checkout or use a global command such as `source init` |
| `AR02xx` | checkout configuration | Validate `aros-targets.toml` and the requested preset |
| `AR03xx` / `AR04xx` | helper or toolchain resolution | Run `build-tools check` or `toolchain verify` |
| `AR05xx` | network transfer | Check URL, proxy and offline policy; do not disable checksums |
| `AR06xx` | configure/build | Inspect the named build step and its captured output |
| `AR08xx` | board/media safety | Re-run scan or dry-run; never substitute a raw device path |
| `AT…` | transpiler | Update the transpiler when a capability fingerprint changed |
| `AG…` | module generation | Treat partial SDK output as invalid and rebuild after fixing the reported path |
| `RM…` | ROM/package tool | Fix the input; the previous destination remains intact |
| `AV…` | independent verifier | Inspect deterministic reports in the named work directory |

## Checkout not found

Commands classified as checkout-required search upward for the canonical AROS
source layout: `configure`, `Makefile.in`, and the `arch/`, `compiler/` and
`rom/` directories. `aros-targets.toml` is optional: a pristine checkout uses
the target contract embedded in `aros-tools`, while an existing file is an
authoritative override and must validate completely. The CLI does not infer a
sibling directory. Use `aros source init PATH`, enter an existing AROS
checkout, or choose a command that is explicitly global.

## Offline or checksum failure

`--offline` means no network request may occur. Install the exact artifact once
online or populate the verified cache through the documented release path.
Never copy an unmeasured archive into the cache.

`--require-fetch-checksums` intentionally rejects an AROS recipe without a
declared SHA-256. Add evidence to the source recipe or update the transpiler;
do not invent a hash for a moving URL.

## Sync refused

`aros source sync` requires a clean attached branch, clean recursive
submodules, the expected canonical `upstream` URL and a fast-forward
relationship. Commit or stash work deliberately, correct the remote, or
reconcile divergent history manually. A repository-wide lock reports
`AR0113`; wait for the descriptor owner to exit. The owner-record file is
persistent by design, is reused after crashes, and must not be removed. The
command also rejects replacement refs, grafts, repository-local attributes,
filters, sparse-checkout controls, URL rewrites and credential overrides because
they could make validation and publication observe different source trees.

The command uses compare-and-swap publication and never creates a merge commit,
runs `reset --hard`, or overwrites concurrent user changes. If both a post-CAS
step and safe rollback fail, preserve the checkout and inspect the reported
branch, index and submodules before retrying. In JSON diagnostics,
`context.commit_state: "indeterminate"` means the CLI deliberately made no
stronger claim; do not infer state from the prose message.

## Package or release installation

Verify the release checksum, signature/attestation and target triple. Keep all
eight binaries on one version. Package-manager repositories are supported only
after the [release-status page](/aros-tools/reference/release-status/) marks
their public verification complete.

If the documented checks do not explain a failure, open a GitHub issue with the
tool version, host target, stable diagnostic document and minimal reproduction.
Use the private process in `SECURITY.md` for security-sensitive details.
