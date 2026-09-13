#!/bin/sh
# Verify the distinct statistics-reset and entry-lifecycle semantics of the
# supported compiler-cache backends without reading or mutating user state.
#
# This is a focused M1 contract probe, not a public cache-management command.
# Every config path, cache root, daemon socket, home directory and compilation
# output lives below one mktemp directory. The trap stops only the server bound
# to that private socket before deleting the validated temporary root.

set -eu

ccache_bin=${CCACHE_BIN:-$(command -v ccache || true)}
sccache_bin=${SCCACHE_BIN:-$(command -v sccache || true)}
c_compiler=${CC:-$(command -v cc || true)}

for requirement in "$ccache_bin" "$sccache_bin" "$c_compiler" python3; do
    if [ -z "$requirement" ] || ! command -v "$requirement" >/dev/null 2>&1; then
        echo "required executable is unavailable: ${requirement:-<empty>}" >&2
        exit 2
    fi
done

ccache_version=$("$ccache_bin" --version | sed -n '1p')
sccache_version=$("$sccache_bin" --version | sed -n '1p')
python3 - "$ccache_version" "$sccache_version" <<'PY'
import re
import sys


def version(line, label):
    match = re.search(r"\b(\d+)\.(\d+)(?:\.(\d+))?\b", line)
    if match is None:
        raise SystemExit(f"cannot determine {label} semantic version from: {line!r}")
    major, minor, patch = match.groups()
    return int(major), int(minor), int(patch or 0)


ccache = version(sys.argv[1], "ccache")
sccache = version(sys.argv[2], "sccache")
if ccache < (4, 14, 0):
    raise SystemExit(f"ccache {ccache!r} is older than the qualified 4.14.0 floor")
if sccache < (0, 17, 0):
    raise SystemExit(f"sccache {sccache!r} is older than the qualified 0.17.0 floor")
PY

temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/aros-cache-backends.XXXXXX")
private_sccache_env() {
    env -i \
        PATH="$PATH" \
        HOME="$temporary_root/home" \
        SCCACHE_CONF="$temporary_root/sccache-config" \
        SCCACHE_SERVER_UDS="$temporary_root/sccache.sock" \
        SCCACHE_IDLE_TIMEOUT=0 \
        "$@"
}

cleanup() {
    private_sccache_env "$sccache_bin" --stop-server >/dev/null 2>&1 || true
    case "$temporary_root" in
        "${TMPDIR:-/tmp}"/aros-cache-backends.*) command rm -rf -- "$temporary_root" ;;
        *) echo "refusing to remove unexpected temporary root: $temporary_root" >&2; exit 70 ;;
    esac
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$temporary_root/home" "$temporary_root/ccache" "$temporary_root/sccache"
printf '%s\n' 'int cache_backend_fixture(void) { return 0; }' > "$temporary_root/fixture.c"
printf '%s\n' '[cache.disk]' "dir = \"$temporary_root/sccache\"" \
    'size = 1073741824' > "$temporary_root/sccache-config"

ccache_env() {
    env -i \
        PATH="$PATH" \
        HOME="$temporary_root/home" \
        CCACHE_DIR="$temporary_root/ccache" \
        CCACHE_CONFIGPATH="$temporary_root/ccache-config" \
        "$@"
}

ccache_env "$ccache_bin" --zero-stats >/dev/null
ccache_env "$ccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/ccache-first.o"
ccache_env "$ccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/ccache-second.o"
ccache_env "$ccache_bin" --zero-stats >/dev/null
ccache_env "$ccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/ccache-after-reset.o"
ccache_env "$ccache_bin" --format json --print-stats > "$temporary_root/ccache-after-reset.json"

ccache_env "$ccache_bin" --clear >/dev/null
ccache_env "$ccache_bin" --zero-stats >/dev/null
ccache_env "$ccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/ccache-after-clear.o"
ccache_env "$ccache_bin" --format json --print-stats > "$temporary_root/ccache-after-clear.json"

private_sccache_env "$sccache_bin" --start-server >/dev/null
private_sccache_env "$sccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/sccache-first.o"
private_sccache_env "$sccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/sccache-second.o"
private_sccache_env "$sccache_bin" --zero-stats >/dev/null
private_sccache_env "$sccache_bin" "$c_compiler" -c "$temporary_root/fixture.c" \
    -o "$temporary_root/sccache-after-reset.o"
private_sccache_env "$sccache_bin" --show-stats --stats-format json \
    > "$temporary_root/sccache-after-reset.json"

python3 - "$temporary_root" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])


def metric_value(value):
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    if isinstance(value, dict):
        counts = value.get("counts")
        if isinstance(counts, dict):
            numbers = [
                item
                for item in counts.values()
                if isinstance(item, int) and not isinstance(item, bool)
            ]
            return sum(numbers)
    return None


def metric(document, names):
    if isinstance(document, dict):
        total = 0
        found = False
        for key, value in document.items():
            if key in names:
                measured = metric_value(value)
                if measured is not None:
                    total += measured
                    found = True
            else:
                measured = metric(value, names)
                if measured is not None:
                    total += measured
                    found = True
        return total if found else None
    elif isinstance(document, list):
        total = 0
        found = False
        for value in document:
            measured = metric(value, names)
            if measured is not None:
                total += measured
                found = True
        return total if found else None
    return None


def require(path, hit_names, miss_names, expected_hits, expected_misses):
    document = json.loads(path.read_text(encoding="utf-8"))
    hits = metric(document, hit_names)
    misses = metric(document, miss_names)
    if hits != expected_hits or misses != expected_misses:
        raise SystemExit(
            f"{path.name}: expected hits={expected_hits}, misses={expected_misses}; "
            f"observed hits={hits!r}, misses={misses!r}; document={document!r}"
        )


require(
    root / "ccache-after-reset.json",
    {"direct_cache_hit", "preprocessed_cache_hit"},
    {"cache_miss"},
    1,
    0,
)
require(
    root / "ccache-after-clear.json",
    {"direct_cache_hit", "preprocessed_cache_hit"},
    {"cache_miss"},
    0,
    1,
)
require(
    root / "sccache-after-reset.json",
    {"cache_hits"},
    {"cache_misses"},
    1,
    0,
)
PY

printf '%s\n' "compiler-cache backend contract: ccache $ccache_version reset preserved entries, ccache clear removed entries, sccache $sccache_version reset preserved entries"
