---
title: "Generated CLI contract: toolchain"
description: Source-derived structural facts for the public aros toolchain command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/).

| Command | ID | Spelling | Position | Required | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros toolchain build` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | source_dir | --source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | producer_dir | --producer-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | tools_dir | --tools-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | work_dir | --work-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | output_dir | --output-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | cache_dir | --cache-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | jobs | --jobs | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | timeout_seconds | --timeout-seconds | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE |  |
| `aros toolchain build` | release_id | --release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain build` | resume_from | --resume-from | — | no | 1 |  |  | — |  |
| `aros toolchain build` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain gc` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain gc` | apply | --apply | — | no | 1 |  |  | — |  |
| `aros toolchain gc` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain import` | source | --source | — | yes | 1 |  |  | — |  |
| `aros toolchain import` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain import` | apply | --apply | — | no | 1 |  |  | — |  |
| `aros toolchain import` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain install` | preset | -p, --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain install` | force | -f, --force | — | no | 0 | false |  | — | local, offline |
| `aros toolchain install` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE | force |
| `aros toolchain install` | local | --local | — | no | 1 |  |  | — | force |
| `aros toolchain inventory` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain inventory` | max_entries | --max-entries | — | no | 1 | 10000 |  | — |  |
| `aros toolchain inventory` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain path` | preset | -p, --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain path` | local | --local | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain plan` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain plan` | source_dir | --source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain plan` | producer_dir | --producer-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain plan` | tools_dir | --tools-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain plan` | work_dir | --work-dir | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | output_dir | --output-dir | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | cache_dir | --cache-dir | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | jobs | --jobs | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | timeout_seconds | --timeout-seconds | — | no | 1 |  |  | — |  |
| `aros toolchain plan` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE |  |
| `aros toolchain plan` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer cache` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer cache` | cache_dir | --cache-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer cache` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE |  |
| `aros toolchain producer cache` | verify_only | --verify-only | — | no | 0 | false |  | — |  |
| `aros toolchain producer cache` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer compare` | left | --left | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compare` | right | --right | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compare` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compare` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer compatibility` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | release_id | --release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | build_environment | --build-environment | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | forbidden_prefixes | --forbidden-prefix | — | no | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | package_dir | --package-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | first_root | --first-root | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | second_root | --second-root | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | source_dir | --source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | engine_work_dir | --engine-work-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | helpers_dir | --helpers-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cmake_program | --cmake-program | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ninja_program | --ninja-program | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | upstream_source_dir | --upstream-source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | upstream_build_dir | --upstream-build-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | python_cache_dir | --python-cache-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_lock | --ports-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_cache_dir | --ports-cache-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_sources_dir | --ports-sources-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | python_environment_dir | --python-environment-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host_tools_dir | --host-tools-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host_tools | --host-tool | — | no | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cmake_build_dir | --cmake-build-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | c_fixture | --c-fixture | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cxx_fixture | --cxx-fixture | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | standalone_output_dir | --standalone-output-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | reports_dir | --reports-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | jobs | --jobs | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | timeout_seconds | --timeout-seconds | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer compatibility-host-tools` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility-ports` | ports_lock | --ports-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility-ports` | cache_dir | --cache-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer compatibility-ports` | offline | --offline | — | no | 0 | false |  | AROS_OFFLINE |  |
| `aros toolchain producer compatibility-ports` | verify_only | --verify-only | — | no | 0 | false |  | — |  |
| `aros toolchain producer compatibility-ports` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer environment` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer environment` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer environment` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer index` | directory | --directory | — | yes | 1 |  |  | — |  |
| `aros toolchain producer index` | release_id | --release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer index` | base_url | --base-url | — | yes | 1 |  |  | — |  |
| `aros toolchain producer index` | source_lock_filename | --source-lock-filename | — | yes | 1 |  |  | — |  |
| `aros toolchain producer index` | stage | --stage | — | yes | 1 |  | pre-attestation, final | — |  |
| `aros toolchain producer index` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer materialize-engine-free-source` | source_dir | --source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | output_dir | --output-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer package` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | release_id | --release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | build_environment | --build-environment | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | forbidden_prefixes | --forbidden-prefix | — | no | 1 |  |  | — |  |
| `aros toolchain producer package` | input_dir | --input-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer package` | output_dir | --output-dir | — | no | 1 |  |  | — |  |
| `aros toolchain producer package` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer prepare-recovery` | qualification_evidence | --qualification-evidence | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | release_dir | --release-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_tag_object | --source-tag-object | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_tag_commit | --source-tag-commit | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_release_id | --recovery-release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_tag_object | --recovery-tag-object | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_tag_commit | --recovery-tag-commit | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_repository | --source-repository | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_workflow | --source-workflow | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_repository | --attestation-repository | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_workflow | --attestation-workflow | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_signer | --attestation-signer | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | now | --now | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer profile` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer profile` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer profile` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain producer profile` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer recipe` | source_dir | --source-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | producer_dir | --producer-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | tools_dir | --tools-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer recipe` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer record-qualification` | release_dir | --release-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_lock_filename | --source-lock-filename | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | lifecycle_reports_dir | --lifecycle-reports-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | comparison_reports_dir | --comparison-reports-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | compatibility_reports_dir | --compatibility-reports-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_repository | --source-repository | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_workflow | --source-workflow | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_run_id | --source-run-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag | --source-tag | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag_object | --source-tag-object | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag_commit | --source-tag-commit | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_repository | --attestation-repository | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_workflow | --attestation-workflow | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_signer | --attestation-signer | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | created_at | --created-at | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | expires_at | --expires-at | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer repackage` | recovery_request | --recovery-request | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_package_dir | --source-package-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_release_id | --source-release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | build_environment | --build-environment | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | forbidden_prefixes | --forbidden-prefix | — | no | 1 |  |  | — |  |
| `aros toolchain producer repackage` | first_extraction_dir | --first-extraction-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | second_extraction_dir | --second-extraction-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | first_output_dir | --first-output-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | second_output_dir | --second-output-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | comparison_output | --comparison-output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer repackage` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer validate-recovery` | recovery_request | --recovery-request | — | yes | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | release_dir | --release-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | output | --output | — | yes | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain producer verify-package` | recipe | --recipe | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | source_lock | --source-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | profiles | --profiles | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | preset | --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | release_id | --release-id | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | host | --host | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | build_environment | --build-environment | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | forbidden_prefixes | --forbidden-prefix | — | no | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | input_dir | --input-dir | — | yes | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | output_dir | --output-dir | — | no | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain register` | source | --source | — | yes | 1 |  |  | — |  |
| `aros toolchain register` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain register` | apply | --apply | — | no | 1 |  |  | — |  |
| `aros toolchain register` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain remove` | managed_id | --managed-id | — | yes | 1 |  |  | — |  |
| `aros toolchain remove` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain remove` | apply | --apply | — | no | 1 |  |  | — |  |
| `aros toolchain remove` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain select` | release_lock | --release-lock | — | yes | 1 |  |  | — |  |
| `aros toolchain select` | store | --store | — | no | 1 |  |  | — |  |
| `aros toolchain select` | apply | --apply | — | no | 1 |  |  | — |  |
| `aros toolchain select` | format | --format | — | no | 1 | human | human, json | — |  |
| `aros toolchain verify` | preset | -p, --preset | — | yes | 1 |  |  | — |  |
| `aros toolchain verify` | local | --local | — | no | 1 |  |  | — |  |
