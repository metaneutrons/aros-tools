---
title: "Generated CLI contract: build"
description: Source-derived structural facts for the public aros build command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros build` | preset | -p, --preset | — | optional | 1 | pc-x86_64 |  | — |  |
| `aros build` | target | -t, --target | — | optional | 1 |  |  | — |  |
| `aros build` | jobs | -j, --jobs | — | optional | 1 |  |  | — |  |
| `aros build` | clean | --clean | — | optional | 0 | false |  | — |  |
| `aros build` | verbose | -v, --verbose | — | optional | 0 | false |  | — |  |
| `aros build` | compiler_cache | --compiler-cache | — | optional | 1 | auto | auto, off, sccache, ccache | — |  |
| `aros build` | compiler_cache_dir | --compiler-cache-dir | — | optional | 1 |  |  | — |  |
| `aros build` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros build` | require_fetch_checksums | --require-fetch-checksums | — | optional | 0 | false |  | AROS_FETCH_REQUIRE_CHECKSUMS |  |
| `aros build` | toolchain_dir | --toolchain-dir | — | optional | 1 |  |  | — |  |
| `aros build` | debug | --debug | — | optional | 0 | false |  | — |  |
| `aros build` | engine_dir | --engine-dir | — | optional | 1 |  |  | — |  |
