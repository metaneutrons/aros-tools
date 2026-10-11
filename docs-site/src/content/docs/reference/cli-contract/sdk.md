---
title: "Generated CLI contract: sdk"
description: Source-derived structural facts for the public aros sdk command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros sdk download` | release_lock | `--release-lock` | — | required | 1 |  |  | — |  |
| `aros sdk download` | lock_sha256 | `--lock-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk download` | cache | `--cache` | — | optional | 1 |  |  | — |  |
| `aros sdk download` | offline | `--offline` | — | optional | 0 | false |  | — |  |
| `aros sdk extract` | identity | `--identity` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | identity_sha256 | `--identity-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | tree_sha256 | `--tree-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | forbidden_prefixes | `--forbidden-prefix` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | package_dir | `--package-dir` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | archive_sha256 | `--archive-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | archive_size | `--archive-size` | — | required | 1 |  |  | — |  |
| `aros sdk extract` | output_root | `--output-root` | — | required | 1 |  |  | — |  |
| `aros sdk install` | release_lock | `--release-lock` | — | required | 1 |  |  | — |  |
| `aros sdk install` | lock_sha256 | `--lock-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk install` | store | `--store` | — | optional | 1 |  |  | — |  |
| `aros sdk install` | archive | `--archive` | — | optional | 1 |  |  | — | cache, offline |
| `aros sdk install` | cache | `--cache` | — | optional | 1 |  |  | — |  |
| `aros sdk install` | offline | `--offline` | — | optional | 0 | false |  | — |  |
| `aros sdk list` | store | `--store` | — | optional | 1 |  |  | — |  |
| `aros sdk list` | max_entries | `--max-entries` | — | optional | 1 | 10000 |  | — |  |
| `aros sdk package` | identity | `--identity` | — | required | 1 |  |  | — |  |
| `aros sdk package` | identity_sha256 | `--identity-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk package` | tree_sha256 | `--tree-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk package` | forbidden_prefixes | `--forbidden-prefix` | — | required | 1 |  |  | — |  |
| `aros sdk package` | candidate_root | `--candidate-root` | — | required | 1 |  |  | — |  |
| `aros sdk package` | output_dir | `--output-dir` | — | required | 1 |  |  | — |  |
| `aros sdk path` | release_lock | `--release-lock` | — | optional | 1 |  |  | — | project_lock |
| `aros sdk path` | lock_sha256 | `--lock-sha256` | — | optional | 1 |  |  | — | project_lock |
| `aros sdk path` | project_lock | `--project-lock` | — | optional | 1 |  |  | — | release_lock, lock_sha256 |
| `aros sdk path` | store | `--store` | — | optional | 1 |  |  | — |  |
| `aros sdk select` | release_lock | `--release-lock` | — | required | 1 |  |  | — |  |
| `aros sdk select` | lock_sha256 | `--lock-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk select` | project_lock | `--project-lock` | — | required | 1 |  |  | — |  |
| `aros sdk select` | store | `--store` | — | optional | 1 |  |  | — |  |
| `aros sdk select` | apply | `--apply` | — | optional | 1 |  |  | — |  |
| `aros sdk verify` | release_lock | `--release-lock` | — | optional | 1 |  |  | — | project_lock |
| `aros sdk verify` | lock_sha256 | `--lock-sha256` | — | optional | 1 |  |  | — | project_lock |
| `aros sdk verify` | project_lock | `--project-lock` | — | optional | 1 |  |  | — | release_lock, lock_sha256 |
| `aros sdk verify` | store | `--store` | — | optional | 1 |  |  | — |  |
| `aros sdk verify-package` | identity | `--identity` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | identity_sha256 | `--identity-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | tree_sha256 | `--tree-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | forbidden_prefixes | `--forbidden-prefix` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | package_dir | `--package-dir` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | archive_sha256 | `--archive-sha256` | — | required | 1 |  |  | — |  |
| `aros sdk verify-package` | archive_size | `--archive-size` | — | required | 1 |  |  | — |  |

## Command notes

### `aros sdk path`

Select either `--release-lock FILE --lock-sha256 SHA256` or `--project-lock FILE`. A project lock is explicit local application configuration, not publisher authentication.

### `aros sdk verify`

Select either `--release-lock FILE --lock-sha256 SHA256` or `--project-lock FILE`. A project lock is explicit local application configuration, not publisher authentication.
