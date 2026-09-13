---
title: "Generated CLI contract: toolchain"
description: Source-derived structural facts for the public aros toolchain command family.
---

This page is generated from the `aros` Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The `constraint` column includes required exclusive groups.

| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `aros toolchain build` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain build` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain build` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | work_dir | --work-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | output_dir | --output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | cache_dir | --cache-dir | — | required | 1 |  |  | — |  |
| `aros toolchain build` | jobs | --jobs | — | required | 1 |  |  | — |  |
| `aros toolchain build` | timeout_seconds | --timeout-seconds | — | required | 1 |  |  | — |  |
| `aros toolchain build` | release_id | --release-id | — | required | 1 |  |  | — |  |
| `aros toolchain build` | resume_from | --resume-from | — | optional | 1 |  |  | — |  |
| `aros toolchain build` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain gc` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain gc` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros toolchain gc` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain import` | source | --source | — | required | 1 |  |  | — |  |
| `aros toolchain import` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain import` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros toolchain import` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain install` | preset | -p, --preset | — | required | 1 |  |  | — |  |
| `aros toolchain install` | force | -f, --force | — | optional | 0 | false |  | — | local, offline |
| `aros toolchain install` | offline | --offline | — | optional | 0 | false |  | AROS_OFFLINE | force |
| `aros toolchain install` | local | --local | — | optional | 1 |  |  | — | force |
| `aros toolchain inventory` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain inventory` | max_entries | --max-entries | — | optional | 1 | 10000 |  | — |  |
| `aros toolchain inventory` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain list` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain path` | preset | -p, --preset | — | required | 1 |  |  | — |  |
| `aros toolchain path` | local | --local | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain plan` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain plan` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain plan` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros toolchain plan` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros toolchain plan` | work_dir | --work-dir | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | output_dir | --output-dir | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | cache_dir | --cache-dir | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | jobs | --jobs | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | timeout_seconds | --timeout-seconds | — | optional | 1 |  |  | — |  |
| `aros toolchain plan` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer compare` | left | --left | — | required | 1 |  |  | — |  |
| `aros toolchain producer compare` | right | --right | — | required | 1 |  |  | — |  |
| `aros toolchain producer compare` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer compare` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer compatibility` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | source_lock | --source-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | release_id | --release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | build_environment | --build-environment | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | forbidden_prefixes | --forbidden-prefix | — | optional | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | package_dir | --package-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | first_root | --first-root | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | second_root | --second-root | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | engine_work_dir | --engine-work-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | helpers_dir | --helpers-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cmake_program | --cmake-program | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ninja_program | --ninja-program | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | upstream_source_dir | --upstream-source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | upstream_build_dir | --upstream-build-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | python_cache_dir | --python-cache-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_lock | --ports-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_cache_dir | --ports-cache-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | ports_sources_dir | --ports-sources-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | python_environment_dir | --python-environment-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host_tools_dir | --host-tools-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | host_tools | --host-tool | — | optional | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cmake_build_dir | --cmake-build-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | c_fixture | --c-fixture | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | cxx_fixture | --cxx-fixture | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | standalone_output_dir | --standalone-output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | reports_dir | --reports-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | jobs | --jobs | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | timeout_seconds | --timeout-seconds | — | required | 1 |  |  | — |  |
| `aros toolchain producer compatibility` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer compatibility-host-tools` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer environment` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer environment` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer environment` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer index` | directory | --directory | — | required | 1 |  |  | — |  |
| `aros toolchain producer index` | release_id | --release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer index` | base_url | --base-url | — | required | 1 |  |  | — |  |
| `aros toolchain producer index` | source_lock_filename | --source-lock-filename | — | required | 1 |  |  | — |  |
| `aros toolchain producer index` | stage | --stage | — | required | 1 |  | pre-attestation, final | — |  |
| `aros toolchain producer index` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer materialize-engine-free-source` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | output_dir | --output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer materialize-engine-free-source` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer package` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | source_lock | --source-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | release_id | --release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | build_environment | --build-environment | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | forbidden_prefixes | --forbidden-prefix | — | optional | 1 |  |  | — |  |
| `aros toolchain producer package` | input_dir | --input-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | output_dir | --output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer package` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer prepare-recovery` | qualification_evidence | --qualification-evidence | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | release_dir | --release-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_tag_object | --source-tag-object | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_tag_commit | --source-tag-commit | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_release_id | --recovery-release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_tag_object | --recovery-tag-object | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | recovery_tag_commit | --recovery-tag-commit | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_repository | --source-repository | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | source_workflow | --source-workflow | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_repository | --attestation-repository | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_workflow | --attestation-workflow | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | attestation_signer | --attestation-signer | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | now | --now | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer prepare-recovery` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer profile` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer profile` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer profile` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain producer profile` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer recipe` | source_dir | --source-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | producer_dir | --producer-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | tools_dir | --tools-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | source_lock | --source-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer recipe` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer record-qualification` | release_dir | --release-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_lock_filename | --source-lock-filename | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | lifecycle_reports_dir | --lifecycle-reports-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | comparison_reports_dir | --comparison-reports-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | compatibility_reports_dir | --compatibility-reports-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_repository | --source-repository | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_workflow | --source-workflow | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_run_id | --source-run-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag | --source-tag | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag_object | --source-tag-object | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | source_tag_commit | --source-tag-commit | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_repository | --attestation-repository | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_workflow | --attestation-workflow | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | attestation_signer | --attestation-signer | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | created_at | --created-at | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | expires_at | --expires-at | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer record-qualification` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer repackage` | recovery_request | --recovery-request | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_package_dir | --source-package-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_release_id | --source-release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | source_lock | --source-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | build_environment | --build-environment | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | forbidden_prefixes | --forbidden-prefix | — | optional | 1 |  |  | — |  |
| `aros toolchain producer repackage` | first_extraction_dir | --first-extraction-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | second_extraction_dir | --second-extraction-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | first_output_dir | --first-output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | second_output_dir | --second-output-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | comparison_output | --comparison-output | — | required | 1 |  |  | — |  |
| `aros toolchain producer repackage` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer validate-recovery` | recovery_request | --recovery-request | — | required | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | release_dir | --release-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | output | --output | — | required | 1 |  |  | — |  |
| `aros toolchain producer validate-recovery` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain producer verify-package` | recipe | --recipe | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | source_lock | --source-lock | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | profiles | --profiles | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | preset | --preset | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | release_id | --release-id | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | host | --host | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | build_environment | --build-environment | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | forbidden_prefixes | --forbidden-prefix | — | optional | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | input_dir | --input-dir | — | required | 1 |  |  | — |  |
| `aros toolchain producer verify-package` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain register` | source | --source | — | required | 1 |  |  | — |  |
| `aros toolchain register` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain register` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros toolchain register` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain remove` | managed_id | --managed-id | — | required | 1 |  |  | — |  |
| `aros toolchain remove` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain remove` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros toolchain remove` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain select` | release_lock | --release-lock | — | required | 1 |  |  | — |  |
| `aros toolchain select` | store | --store | — | optional | 1 |  |  | — |  |
| `aros toolchain select` | apply | --apply | — | optional | 1 |  |  | — |  |
| `aros toolchain select` | format | --format | — | optional | 1 | human | human, json | — |  |
| `aros toolchain verify` | preset | -p, --preset | — | required | 1 |  |  | — |  |
| `aros toolchain verify` | local | --local | — | optional | 1 |  |  | — |  |
