#!/usr/bin/env bash

set -euo pipefail

target=${1:?expected release target}
[[ $# -eq 1 ]] || { echo 'expected exactly one release target' >&2; exit 2; }
case "$target" in
    aarch64-apple-darwin) ;;
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) exit 0 ;;
    *) echo "unsupported Homebrew release target: $target" >&2; exit 2 ;;
esac

prefix=$(brew --prefix)
[[ "$prefix" == /* && "$prefix" != / ]] || {
    echo "invalid Homebrew prefix: $prefix" >&2
    exit 1
}
link="$prefix/bin/openssl"
legacy="$prefix/opt/openssl@1.1/bin/openssl"

# Hosted macOS images can retain an orphaned openssl@1.1 symlink alongside the
# current openssl@3 dependency. Homebrew's own unlink removes zero links in
# that state, so remove only this exact symlink after verifying its target.
if [[ -L "$link" && $(readlink "$link") == "$legacy" ]]; then
    unlink "$link"
    if [[ -e "$link" || -L "$link" ]]; then
        echo "legacy OpenSSL link remains after unlink: $link" >&2
        exit 1
    fi
fi
