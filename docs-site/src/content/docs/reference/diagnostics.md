---
title: Diagnostics and logs
description: Stable errors for people and automation without leaking host state.
---

User-facing tools emit the versioned `aros-tool-diagnostics-v1` contract.
Human output explains the failed stage and remediation; JSON mode provides
stable component codes for automation.

Local logs are opt-in and require an explicit destination. Deterministic logs
exclude timestamps, host identity, environment values and raw invocations.
They must never be included in release archives.

Failures are not converted into guessed output. In particular, the transpiler
stops when a reviewed opaque recipe changes, the fetcher refuses unverifiable
archives under strict policy, and board workflows reject ambiguous disks or
hardware identities before mutation.
