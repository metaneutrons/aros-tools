---
title: Architecture
description: Crate ownership and process boundaries.
---

The workspace separates shared contracts from product-specific policy.

| Boundary | Owner |
| --- | --- |
| diagnostics, local-log mechanics, hashes, ELF and toolchain schemas | `aros-common` |
| repository orchestration and user-facing commands | `aros-cli` |
| MetaMake translation | `aros-transpiler` |
| independent reference verification | `aros-verify` |
| two-pass linking and set collection | `aros-collect` |
| physical board safety and deployment | `aros-board` |
| source transport, cache, extraction and patching | `aros-fetch` |

The CLI executes build tools as standalone programs. It does not link their
implementations into one process. This preserves explicit contracts and keeps
component failures attributable. The verifier intentionally does not reuse the
transpiler implementation, because a shared defect must not satisfy both sides
of a differential check.
