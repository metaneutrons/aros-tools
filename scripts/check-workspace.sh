#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/check-workspace.sh [all|quality|docs|test|portable-test]

Run the canonical aros-tools workspace gate from any directory.
  all      Run quality, documentation, and test gates (default).
  quality  Run formatting, policy, lint, Rust documentation, and dependency gates.
  docs     Audit locked web dependencies and build the Astro documentation.
  test     Run all locked tests against the exact qualified AROS-NX source.
  portable-test
           Run the closed cross-host suite without a qualified source checkout.
           Source-coupled transpiler/verifier tests are compiled but execute
           only in the exact-source `test` gate.

The test gate requires AROS_TEST_SOURCE_ROOT to name the exact qualified
AROS-NX checkout used by the versioned source contract.
EOF
}

mode=${1:-all}
if [[ $# -gt 1 ]]; then
    usage >&2
    exit 2
fi
case "$mode" in
    all | quality | docs | test | portable-test) ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        printf 'error: unsupported workspace gate mode: %s\n' "$mode" >&2
        usage >&2
        exit 2
        ;;
esac

script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
repository_root=$(unset CDPATH; cd -- "$script_root/.." && pwd -P)
cd "$repository_root"

require_quality_tools() {
    local command_name
    if ! command -v python3 >/dev/null 2>&1; then
        printf '%s\n' \
            'error: Python >= 3.11 with tomllib is required by the workspace quality gate; see CONTRIBUTING.md' >&2
        return 1
    fi
    python3 scripts/check-development-runtimes.py
    for command_name in actionlint ar awk cargo cmp curl diff dpkg-deb find git gpg \
        gpgconf gpgv gzip jq rustc sed shellcheck sort tar wc; do
        if ! command -v "$command_name" >/dev/null 2>&1; then
            printf 'error: %s is required by the workspace quality gate; see CONTRIBUTING.md\n' \
                "$command_name" >&2
            return 1
        fi
    done
    for command_name in fmt clippy; do
        if ! cargo "$command_name" --version >/dev/null 2>&1; then
            printf 'error: cargo-%s is required by the workspace quality gate; see CONTRIBUTING.md\n' \
                "$command_name" >&2
            return 1
        fi
    done
    if ! command -v sha256sum >/dev/null 2>&1 && \
       ! command -v shasum >/dev/null 2>&1; then
        printf '%s\n' \
            'error: sha256sum or shasum is required by the workspace quality gate; see CONTRIBUTING.md' >&2
        return 1
    fi
    for command_name in audit deny machete; do
        if ! cargo "$command_name" --version >/dev/null 2>&1; then
            printf 'error: cargo-%s is required by the workspace quality gate; see CONTRIBUTING.md\n' \
                "$command_name" >&2
            return 1
        fi
    done
}

run_quality() {
    require_quality_tools
    cargo fmt --all -- --check
    sh scripts/check-architecture.sh
    python3 scripts/check-environment-contract.py
    python3 -m unittest discover -s scripts -p '*_test.py'
    scripts/release/check-actions-policy.sh
    scripts/release/verify-apt-workflow-contract.sh
    scripts/release/test-governance-policy.sh
    scripts/release/test-homebrew-app.py
    scripts/release/test-release-policy.sh
    actionlint
    # All checked-in shell programs form one lint boundary. The NUL-delimited
    # handoff keeps paths safe without relying on Bash 4's mapfile on macOS.
    sh -n scripts/check-architecture.sh
    find scripts -type f -name '*.sh' ! -path 'scripts/check-architecture.sh' -print0 \
        | xargs -0 bash -n
    find scripts -type f -name '*.sh' -print0 | xargs -0 shellcheck
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
    cargo audit --deny warnings
    cargo deny check
    cargo machete
}

run_docs() {
    if ! command -v python3 >/dev/null 2>&1; then
        printf '%s\n' 'error: Python >= 3.11 is required by the documentation gate' >&2
        return 1
    fi
    if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
        printf '%s\n' 'error: Node.js >= 24 with npm is required by the Astro documentation gate' >&2
        return 1
    fi
    python3 scripts/check-development-runtimes.py --require-node
    (
        cd docs-site
        npm ci --ignore-scripts
        npm audit --audit-level=high
        npm run build
        python3 ../scripts/check-doc-links.py \
            --directory dist --base /aros-tools/
        npm run worker:check
    )
}

run_tests() {
    if [[ -z "${AROS_TEST_SOURCE_ROOT:-}" || ! -d "$AROS_TEST_SOURCE_ROOT" ]]; then
        printf '%s\n' \
            'error: AROS_TEST_SOURCE_ROOT must name the qualified AROS-NX checkout required by workspace tests' >&2
        return 1
    fi
    local expected_source_commit actual_source_commit source_top source_root
    expected_source_commit=$(python3 - "$repository_root/contracts/aros-source-v1.toml" <<'PY'
import re
import sys
import tomllib

with open(sys.argv[1], 'rb') as stream:
    commit = tomllib.load(stream)['source']['commit']
if re.fullmatch(r'[0-9a-f]{40}', commit) is None:
    raise SystemExit('error: source contract contains a malformed commit identity')
print(commit)
PY
    )
    if ! source_top=$(git -C "$AROS_TEST_SOURCE_ROOT" rev-parse --show-toplevel 2>/dev/null); then
        printf 'error: AROS_TEST_SOURCE_ROOT is not a Git checkout: %s\n' \
            "$AROS_TEST_SOURCE_ROOT" >&2
        return 1
    fi
    source_root=$(unset CDPATH; cd -- "$AROS_TEST_SOURCE_ROOT" && pwd -P)
    source_top=$(unset CDPATH; cd -- "$source_top" && pwd -P)
    if [[ "$source_root" != "$source_top" ]]; then
        printf 'error: AROS_TEST_SOURCE_ROOT must name the checkout root, not a subdirectory: %s\n' \
            "$AROS_TEST_SOURCE_ROOT" >&2
        return 1
    fi
    actual_source_commit=$(git -C "$source_root" rev-parse --verify 'HEAD^{commit}')
    if [[ "$actual_source_commit" != "$expected_source_commit" ]]; then
        printf 'error: AROS_TEST_SOURCE_ROOT is at %s; source contract requires %s\n' \
            "$actual_source_commit" "$expected_source_commit" >&2
        return 1
    fi
    local source_status submodule_status
    source_status=$(git -C "$source_root" status --porcelain=v1 --untracked-files=all)
    if [[ -n "$source_status" ]]; then
        printf 'error: AROS_TEST_SOURCE_ROOT must be completely clean; first entries: %s\n' \
            "$(printf '%s\n' "$source_status" | head -n 8 | tr '\n' ';')" >&2
        return 1
    fi
    submodule_status=$(git -C "$source_root" submodule status --recursive)
    if printf '%s\n' "$submodule_status" | grep -Eq '^[-+U]'; then
        printf '%s\n' \
            'error: AROS_TEST_SOURCE_ROOT has uninitialized, mismatched, or conflicted submodules' >&2
        printf '%s\n' "$submodule_status" | grep -E '^[-+U]' | head -n 8 >&2
        return 1
    fi
    cargo test --workspace --all-features --locked

    # The engine's CMake fixtures are product-contract tests, not Rust unit
    # tests. Build the normal executables they drive, then execute every
    # host-compatible fixture against the same exact source identity validated
    # above. Keeping discovery in the canonical gate prevents a newly added
    # fixture from existing only as a manually remembered test.
    local command_name discovered_count executed_count host_machine host_system
    local skipped_count test_case test_name tools_directory
    for command_name in clang cmake ninja; do
        if ! command -v "$command_name" >/dev/null 2>&1; then
            printf 'error: %s is required by the exact-source engine tests; see CONTRIBUTING.md\n' \
                "$command_name" >&2
            return 1
        fi
    done
    cargo build --workspace --all-features --locked
    tools_directory="$repository_root/target/debug"
    host_system=$(uname -s)
    host_machine=$(uname -m)
    discovered_count=0
    executed_count=0
    skipped_count=0
    while IFS= read -r test_case; do
        discovered_count=$((discovered_count + 1))
        test_name=${test_case##*/}
        if [[ "$test_name" == 'GrubBuildTest.cmake' ]] &&
            [[ "$host_system" != 'Darwin' || "$host_machine" != 'arm64' ]]; then
            skipped_count=$((skipped_count + 1))
            printf 'engine test omitted: %s requires Darwin/arm64; current host is %s/%s\n' \
                "$test_name" "$host_system" "$host_machine"
            continue
        fi
        executed_count=$((executed_count + 1))
        printf 'engine test %d: %s\n' "$executed_count" "$test_name"
        AROS_TEST_TOOLS_DIR="$tools_directory" \
            cmake -P "$test_case"
    done < <(find "$repository_root/crates/aros-cmake-engine/engine/tests" \
        -maxdepth 1 -type f -name '*Test.cmake' -print | sort)
    if [[ "$discovered_count" -eq 0 ]]; then
        printf '%s\n' 'error: no CMake engine contract tests were discovered' >&2
        return 1
    fi
    printf 'engine tests passed: %d executed, %d host-qualified omission(s), %d discovered\n' \
        "$executed_count" "$skipped_count" "$discovered_count"
}

run_portable_tests() {
    if [[ -n "${AROS_TEST_SOURCE_ROOT:-}" ]]; then
        printf '%s\n' \
            'error: portable-test must not receive AROS_TEST_SOURCE_ROOT; use test for exact-source qualification' >&2
        return 1
    fi
    # aros-transpiler and aros-verify deliberately contain white-box tests
    # whose oracle is the exact AROS-NX tree. Running those tests without that
    # input is an invalid qualification, not a portable skip. Compile every one
    # of their test targets on each host, execute their source-independent bin
    # tests, and leave the complete runtime suite to run_tests above.
    cargo test --workspace --all-features --locked \
        --exclude aros-transpiler --exclude aros-verify
    cargo test --locked -p aros-transpiler -p aros-verify \
        --all-features --no-run
    cargo test --locked -p aros-transpiler --bin aros-transpiler \
        --all-features
    cargo test --locked -p aros-verify --bin aros-verify \
        --all-features
}

if [[ "$mode" == all || "$mode" == quality ]]; then
    run_quality
fi
if [[ "$mode" == all || "$mode" == docs ]]; then
    run_docs
fi
if [[ "$mode" == all || "$mode" == test ]]; then
    run_tests
fi
if [[ "$mode" == portable-test ]]; then
    run_portable_tests
fi
