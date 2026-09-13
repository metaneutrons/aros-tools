---
title: "Generated CLI contract: cache"
description: Source-derived structural facts for the public aros cache command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros cache archives fetch` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives fetch` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives fetch` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives fetch` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives fetch` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE | refresh |
| `aros cache archives fetch` | refresh | --refresh | — | optional | 0 | false |  | — | offline |
| `aros cache archives fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives keep` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives keep` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives keep` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives keep` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives keep` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives keep` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache archives keep` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives list` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives list` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives list` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives list` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives list` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives release` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache archives release` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache archives release` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives remove` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives remove` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives remove` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives remove` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives remove` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives remove` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache archives remove` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache archives verify` | project | --project | — | required | 1 |  |  | — |  |
| `aros cache archives verify` | host_compiler | --host-compiler | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives verify` | toolchain | --toolchain | — | exactly one of archive_selector | 0 | false |  | — |  |
| `aros cache archives verify` | preset | --preset | — | optional | 1 |  |  | — |  |
| `aros cache archives verify` | host | --host | — | optional | 1 |  |  | — |  |
| `aros cache archives verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo fetch` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo fetch` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros cache cargo fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo keep` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo keep` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo keep` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo keep` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo keep` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache cargo keep` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo list` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo list` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo release` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo release` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache cargo release` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache cargo release` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo remove` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo remove` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo remove` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo remove` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo remove` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache cargo remove` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo status` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache cargo verify` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache cargo verify` | cargo | --cargo | — | optional | 1 |  |  | — |  |
| `aros cache cargo verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler clear` | backend | --backend | — | required | 1 |  | sccache, ccache | — |  |
| `aros cache compiler clear` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler clear` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache compiler clear` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler prepare` | backend | --backend | — | required | 1 |  | sccache, ccache | — |  |
| `aros cache compiler prepare` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler prepare` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler reset-stats` | backend | --backend | — | required | 1 |  | sccache, ccache | — |  |
| `aros cache compiler reset-stats` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler reset-stats` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache compiler reset-stats` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler stats` | backend | --backend | — | required | 1 |  | sccache, ccache | — |  |
| `aros cache compiler stats` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler stats` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache compiler status` | backend | --backend | — | optional | 1 | auto | auto, sccache, ccache | — |  |
| `aros cache compiler status` | dir | --dir | — | optional | 1 |  |  | — |  |
| `aros cache compiler status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf keep` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros cache genmf keep` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf keep` | python | --python | — | optional | 1 |  |  | — |  |
| `aros cache genmf keep` | timeout_seconds | --timeout-seconds | — | optional | 1 | 30 |  | AROS_CACHE_GENMF_TIMEOUT_SECONDS |  |
| `aros cache genmf keep` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache genmf keep` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf list` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros cache genmf list` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf list` | python | --python | — | optional | 1 |  |  | — |  |
| `aros cache genmf list` | timeout_seconds | --timeout-seconds | — | optional | 1 | 30 |  | AROS_CACHE_GENMF_TIMEOUT_SECONDS |  |
| `aros cache genmf list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf refresh` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros cache genmf refresh` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf refresh` | python | --python | — | optional | 1 |  |  | — |  |
| `aros cache genmf refresh` | timeout_seconds | --timeout-seconds | — | optional | 1 | 30 |  | AROS_CACHE_GENMF_TIMEOUT_SECONDS |  |
| `aros cache genmf refresh` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf release` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf release` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache genmf release` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache genmf release` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf remove` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros cache genmf remove` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf remove` | python | --python | — | optional | 1 |  |  | — |  |
| `aros cache genmf remove` | timeout_seconds | --timeout-seconds | — | optional | 1 | 30 |  | AROS_CACHE_GENMF_TIMEOUT_SECONDS |  |
| `aros cache genmf remove` | source | --source | — | required | 1 |  |  | — |  |
| `aros cache genmf remove` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache genmf remove` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf status` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache genmf verify` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros cache genmf verify` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache genmf verify` | python | --python | — | optional | 1 |  |  | — |  |
| `aros cache genmf verify` | timeout_seconds | --timeout-seconds | — | optional | 1 | 30 |  | AROS_CACHE_GENMF_TIMEOUT_SECONDS |  |
| `aros cache genmf verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources fetch` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources fetch` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources fetch` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE |  |
| `aros cache sources fetch` | allow_unverified | --allow-unverified | — | optional | 0 | false |  | — | source_lock, compatibility_ports_lock |
| `aros cache sources fetch` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources keep` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources keep` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources keep` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources keep` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources keep` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache sources keep` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources list` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources list` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources release` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources release` | name | --name | — | required | 1 |  |  | — |  |
| `aros cache sources release` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache sources release` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources remove` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources remove` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources remove` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources remove` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources remove` | role | --role | — | required | 1 |  |  | — |  |
| `aros cache sources remove` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros cache sources remove` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources status` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources status` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache sources verify` | source_lock | --source-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | compatibility_ports_lock | --compatibility-ports-lock | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | source_fetch_plan | --source-fetch-plan | — | exactly one of source_selector | 1 |  |  | — |  |
| `aros cache sources verify` | dir | --dir | — | required | 1 |  |  | — |  |
| `aros cache sources verify` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros cache status` | format | --format | — | optional | 1 | human | human, json | — |  |
