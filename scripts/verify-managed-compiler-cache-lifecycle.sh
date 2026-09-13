#!/bin/sh
# Exercise the public managed compiler-cache lifecycle against real local
# ccache and sccache backends. Every home, cache, configuration, output and
# Unix-domain socket is private to one mktemp root; no user cache or daemon is
# inspected or mutated. This is an explicit CACHE-M6 acceptance probe, not a
# general-purpose cleanup command.

set -eu

script_root=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repository_root=$(CDPATH='' cd -- "$script_root/.." && pwd -P)
ccache_bin=${CCACHE_BIN:-$(command -v ccache || true)}
sccache_bin=${SCCACHE_BIN:-$(command -v sccache || true)}
c_compiler=${CC:-$(command -v cc || true)}

for requirement in cargo "$ccache_bin" "$sccache_bin" "$c_compiler" python3; do
    if [ -z "$requirement" ] || ! command -v "$requirement" >/dev/null 2>&1; then
        printf 'required executable is unavailable: %s\n' "${requirement:-<empty>}" >&2
        exit 2
    fi
done

temporary_root=$(mktemp -d /tmp/aros-managed-cache-lifecycle.XXXXXX)
managed_home="$temporary_root/aros-home"
fixture="$temporary_root/fixture.c"

managed_aros() {
    AROS_HOME="$managed_home" \
        cargo run --quiet --manifest-path "$repository_root/Cargo.toml" --bin aros -- "$@"
}

sccache_environment() {
    root=$1
    shift
    env -i \
        PATH="$PATH" \
        HOME="$temporary_root/home" \
        SCCACHE_CONF="$root/configuration.toml" \
        SCCACHE_DIR="$root/data" \
        SCCACHE_SERVER_UDS="$root/server.sock" \
        SCCACHE_IDLE_TIMEOUT=0 \
        "$@"
}

ccache_environment() {
    root=$1
    shift
    env -i \
        PATH="$PATH" \
        HOME="$temporary_root/home" \
        CCACHE_CONFIGPATH="$root/configuration.toml" \
        CCACHE_DIR="$root/data" \
        "$@"
}

cleanup() {
    exit_status=$?
    trap - EXIT HUP INT TERM
    managed_sccache_root="$managed_home/cache/compiler/v1/sccache"
    foreign_sccache_root="$temporary_root/foreign-sccache"
    if [ -d "$managed_sccache_root" ]; then
        sccache_environment "$managed_sccache_root" "$sccache_bin" --stop-server >/dev/null 2>&1 || true
    fi
    if [ -d "$foreign_sccache_root" ]; then
        sccache_environment "$foreign_sccache_root" "$sccache_bin" --stop-server >/dev/null 2>&1 || true
    fi
    case "$temporary_root" in
        /tmp/aros-managed-cache-lifecycle.*) command rm -rf -- "$temporary_root" ;;
        *) printf 'refusing to remove unexpected temporary root: %s\n' "$temporary_root" >&2; exit 70 ;;
    esac
    exit "$exit_status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

preview_token() {
    python3 - "$1" <<'PY'
import json
import sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
token = document.get("preview", {}).get("apply_token")
if not isinstance(token, str) or not token:
    raise SystemExit(f"preview has no apply_token: {document!r}")
print(token)
PY
}

require_nonempty_tree() {
    if ! find "$1" -mindepth 1 -print -quit | grep -q .; then
        printf 'expected managed cache data below %s\n' "$1" >&2
        exit 1
    fi
}

require_empty_tree() {
    if find "$1" -mindepth 1 -print -quit | grep -q .; then
        printf 'expected empty managed cache data below %s\n' "$1" >&2
        exit 1
    fi
}

mkdir -p "$temporary_root/home"
printf '%s\n' 'int managed_cache_fixture(void) { return 0; }' > "$fixture"

managed_ccache_root="$managed_home/cache/compiler/v1/ccache"
managed_aros cache compiler prepare --backend ccache --format json \
    > "$temporary_root/ccache-prepare.json"
ccache_environment "$managed_ccache_root" "$ccache_bin" "$c_compiler" -c "$fixture" \
    -o "$temporary_root/ccache-first.o"
ccache_environment "$managed_ccache_root" "$ccache_bin" "$c_compiler" -c "$fixture" \
    -o "$temporary_root/ccache-second.o"
require_nonempty_tree "$managed_ccache_root/data"

managed_aros cache compiler reset-stats --backend ccache --format json \
    > "$temporary_root/ccache-reset-preview.json"
ccache_reset_token=$(preview_token "$temporary_root/ccache-reset-preview.json")
managed_aros cache compiler reset-stats --backend ccache --apply "$ccache_reset_token" --format json \
    > "$temporary_root/ccache-reset-apply.json"
require_nonempty_tree "$managed_ccache_root/data"

managed_aros cache compiler clear --backend ccache --format json \
    > "$temporary_root/ccache-clear-preview.json"
ccache_clear_token=$(preview_token "$temporary_root/ccache-clear-preview.json")
managed_aros cache compiler clear --backend ccache --apply "$ccache_clear_token" --format json \
    > "$temporary_root/ccache-clear-apply.json"
ccache_environment "$managed_ccache_root" "$ccache_bin" --format json --print-stats \
    > "$temporary_root/ccache-after-clear.json"
python3 - "$temporary_root/ccache-after-clear.json" <<'PY'
import json
import sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
for key in ("files_in_cache", "cache_size_kibibyte"):
    if document.get(key) != 0:
        raise SystemExit(f"ccache clear left {key}={document.get(key)!r}: {document!r}")
PY

managed_sccache_root="$managed_home/cache/compiler/v1/sccache"
managed_aros cache compiler prepare --backend sccache --format json \
    > "$temporary_root/sccache-prepare.json"
sccache_environment "$managed_sccache_root" "$sccache_bin" "$c_compiler" -c "$fixture" \
    -o "$temporary_root/sccache-first.o"
sccache_environment "$managed_sccache_root" "$sccache_bin" "$c_compiler" -c "$fixture" \
    -o "$temporary_root/sccache-second.o"
require_nonempty_tree "$managed_sccache_root/data"

foreign_sccache_root="$temporary_root/foreign-sccache"
mkdir -p "$foreign_sccache_root/data"
printf '%s\n' '[cache.disk]' "dir = \"$foreign_sccache_root/data\"" 'size = 1073741824' \
    > "$foreign_sccache_root/configuration.toml"
sccache_environment "$foreign_sccache_root" "$sccache_bin" --start-server >/dev/null
sccache_environment "$foreign_sccache_root" "$sccache_bin" --show-stats >/dev/null

managed_aros cache compiler reset-stats --backend sccache --format json \
    > "$temporary_root/sccache-reset-preview.json"
sccache_reset_token=$(preview_token "$temporary_root/sccache-reset-preview.json")
managed_aros cache compiler reset-stats --backend sccache --apply "$sccache_reset_token" --format json \
    > "$temporary_root/sccache-reset-apply.json"
require_nonempty_tree "$managed_sccache_root/data"

managed_aros cache compiler clear --backend sccache --format json \
    > "$temporary_root/sccache-clear-preview.json"
sccache_clear_token=$(preview_token "$temporary_root/sccache-clear-preview.json")
managed_aros cache compiler clear --backend sccache --apply "$sccache_clear_token" --format json \
    > "$temporary_root/sccache-clear-apply.json"
require_empty_tree "$managed_sccache_root/data"
if [ -e "$managed_sccache_root/server.sock" ]; then
    printf 'managed sccache socket remained after clear: %s\n' "$managed_sccache_root/server.sock" >&2
    exit 1
fi
sccache_environment "$foreign_sccache_root" "$sccache_bin" --show-stats >/dev/null

printf '%s\n' 'managed compiler-cache lifecycle: ccache and sccache reset preserve entries; clear is contained and sccache leaves its private socket stopped'
