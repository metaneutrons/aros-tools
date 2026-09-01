#!/usr/bin/env bash

# Create only a missing output parent. Existing caller-owned directories keep
# their exact mode and must be real, no-follow directories.

set -euo pipefail

fail() {
    printf '::error::AP7241 %s\n' "$*" >&2
    exit 1
}

path=
mode=0755
while (($#)); do
    case "$1" in
        --path) path=${2:-}; shift 2 ;;
        --mode) mode=${2:-}; shift 2 ;;
        *) fail "unknown output-parent argument: $1" ;;
    esac
done

[[ -n "$path" && "$path" != / ]] || fail 'output path must name a non-root leaf'
[[ "$mode" == 0700 || "$mode" == 0755 ]] || fail 'output-parent mode must be 0700 or 0755'
parent=$(dirname -- "$path")
if [[ -L "$parent" ]]; then
    fail "output parent is a symbolic link: $parent"
fi
if [[ -e "$parent" ]]; then
    [[ -d "$parent" ]] || fail "output parent is not a directory: $parent"
    exit 0
fi
install -d -m "$mode" -- "$parent"
[[ -d "$parent" && ! -L "$parent" ]] || \
    fail "created output parent is not a real directory: $parent"
