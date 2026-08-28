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

direct_process=$(grep -R -n --include='*.rs' -E '\.(output|status)\(\)' crates/aros-cli/src \
    | grep -v 'crates/aros-cli/src/observability.rs:' \
    | grep -v 'crates/aros-cli/src/artifact.rs:.*response\.status()' || true)
if [ -n "$direct_process" ]; then
    echo "$direct_process" >&2
    echo "CLI subprocesses must pass through observability.rs" >&2
    failed=1
fi

for component in aros-ahi-runner aros-board aros-collect aros-fetch aros-verify; do
    direct_process=$(grep -R -n --include='*.rs' -E '\.(output|status)\(\)' "crates/$component/src" \
        | grep -v ':.*response\.status()' || true)
    if [ -n "$direct_process" ]; then
        echo "$direct_process" >&2
        echo "$component subprocesses must pass through aros-common process primitives" >&2
        failed=1
    fi
done

write_sd_body=$(sed -n '/^pub fn write_sd_image(/,/^}/p' crates/aros-cli/src/board/mod.rs)
if printf '%s\n' "$write_sd_body" | grep -Eq 'sd_disk::(scan|confirmation_token)'; then
    echo "aros-cli write previews must use aros-board's atomic write-plan contract" >&2
    failed=1
fi

directory_helper_count=$(grep -R -h --include='*.rs' \
    '^pub(crate) fn canonical_existing_directory' crates/aros-board/src | wc -l | tr -d ' ')
if [ "$directory_helper_count" -ne 1 ]; then
    echo "aros-board must have exactly one canonical_existing_directory helper" >&2
    failed=1
fi

foreign_schema_literals=$(
    {
        cat crates/aros-board/src/sd_disk.rs
        sed '/^mod tests {$/,$d' crates/aros-board/src/sd_unmount.rs
    } | grep -n -E '"(blockdevices|pkname|hotplug|mountpoints|children|DeviceIdentifier|WholeDisk|ParentWholeDisk|DeviceNode|MountPoint|Internal|Writable|Ejectable|VirtualOrPhysical|SerialNumber|MediaName|RemovableMedia|physical|-plist)"' || true
)
if [ -n "$foreign_schema_literals" ]; then
    echo "$foreign_schema_literals" >&2
    echo "destructive board paths must use disk_inventory schema constants" >&2
    failed=1
fi

cache_policy_count=$(grep -R -h --include='*.rs' 'which::which("sccache")' \
    crates/aros-cli/src | wc -l | tr -d ' ')
if [ "$cache_policy_count" -ne 1 ]; then
    echo "aros-cli must have exactly one sccache-before-ccache selection policy" >&2
    failed=1
fi

if grep -R -n --include='*.rs' 'PathBuf::from(format!("build/{preset}"))' crates/aros-cli/src; then
    echo "preset build directories must pass through build::build_dir" >&2
    failed=1
fi

if grep -R -n --include='*.rs' 'sha256_file_with_size' crates/aros-cli/src; then
    echo "aros-cli hashing must use aros-common directly without a duplicate size adapter" >&2
    failed=1
fi

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
