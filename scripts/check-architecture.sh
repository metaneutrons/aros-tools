#!/bin/sh
# Fail when the Rust tool workspace regresses into known architectural debt.

set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$workspace"

failed=0
max_production_lines=2000

for source in $(find crates -path '*/src/*.rs' -type f ! -name '*_tests.rs' | sort); do
    lines=$(wc -l < "$source" | tr -d ' ')
    if [ "$lines" -gt "$max_production_lines" ]; then
        echo "$source has $lines production lines (limit: $max_production_lines)" >&2
        failed=1
    fi
done

for source in graph generator copy_includes parser; do
    path="crates/aros-transpiler/src/$source.rs"
    if grep -Eq '^mod tests \{' "$path"; then
        echo "$path embeds its test suite; keep it in ${source}_tests.rs" >&2
        failed=1
    fi
done

for source in crates/aros-board/src/*.rs crates/aros-cli/src/*.rs crates/aros-cli/src/board/*.rs; do
    case "$source" in
        *_tests.rs) continue ;;
    esac
    if ! grep -q '^//!' "$source"; then
        echo "$source has no module-level documentation" >&2
        failed=1
    fi
done

if grep -R -n --include='*.rs' --include='Cargo.toml' '\banyhow\b' crates/aros-cli; then
    echo "aros-cli must use its single miette error boundary" >&2
    failed=1
fi

direct_output=$(grep -R -n --include='*.rs' '\.output()' crates/aros-cli/src \
    | grep -v 'crates/aros-cli/src/observability.rs:' || true)
if [ -n "$direct_output" ]; then
    echo "$direct_output" >&2
    echo "structured subprocess output must pass through observability.rs" >&2
    failed=1
fi

for component in aros-ahi-runner aros-board aros-collect aros-verify; do
    direct_process=$(grep -R -n --include='*.rs' -E '\.(output|status)\(\)' "crates/$component/src" || true)
    if [ -n "$direct_process" ]; then
        echo "$direct_process" >&2
        echo "$component subprocesses must pass through aros-common process primitives" >&2
        failed=1
    fi
done

if grep -R -n --include='*.rs' --include='*.md' --include='*.toml' 'aros pi' \
    crates scripts README.md; then
    echo "the unreleased CLI has no legacy 'aros pi' command or documentation" >&2
    failed=1
fi

if grep -R -n --include='*.rs' -E 'PiCommand|Commands::Pi|mod pi|crate::pi' crates/aros-cli; then
    echo "the CLI board surface must not retain legacy Pi command aliases" >&2
    failed=1
fi

exit "$failed"
