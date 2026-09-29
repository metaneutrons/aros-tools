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

# Hosted macOS images can retain a linked openssl@1.1 alongside the current
# openssl@3 dependency. Remove only this exact, known conflicting formula link.
if [[ -L "$link" && $(readlink "$link") == "$legacy" ]]; then
    brew unlink openssl@1.1
    if [[ -e "$link" || -L "$link" ]]; then
        echo "legacy OpenSSL link remains after brew unlink: $link" >&2
        exit 1
    fi
fi
